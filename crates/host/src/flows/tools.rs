//! `flow_*` 宿主工具（architecture §8.5）：流程库的编写与运行工具（`harnix_flow::author`），
//! 声明与执行都是库的。Conix 在外面加三样：目标可以写 `#n`（换成名字再交给库）、按发起调用的会话
//! 限作用域、流程还有没结束的运行时不许改不许删。运行里的会话自己的三个工具（`flow_state`、
//! `flow_submit`、`flow_propose`）是派工器的，原样转给它。

use crate::sync::Locked;
use std::collections::HashMap;
use std::sync::{Mutex, Weak};

use async_trait::async_trait;
use harnix_flow::author::{self, AuthorHost, Notice, RUN_TOOLS};
use harnix_flow::runner::{self, SessionId};
use harnix_flow::{Actor, RunStatus};
use serde_json::{Value, json};

use super::Flows;
use crate::hub::tools::Scope;

/// 说给模型：`flow` 参数也收 `#n`。
const FLOW_TARGET: &str = "The flow: its name, or \"#3\" (its number in the flow list right now). Omit for the flow this conversation just created, or the run's own flow in a session inside a run.";

/// 人的决定（design/flows.md §4）：确认、放行、强制放行只能人在流程标签里做（`authorize`），模型调了也一定被拒，
/// 所以不给会话这几个工具。
pub(crate) const PERSONS_ONLY: &[&str] = &[
    author::TOOL_CONFIRM,
    author::TOOL_FORCE,
    author::TOOL_RELEASE,
];

pub(crate) fn declarations() -> Vec<Value> {
    author::tool_specs()
        .into_iter()
        .filter(|t| !PERSONS_ONLY.contains(&t.name.as_str()))
        // 主会话也要 `flow_command`；运行里的会话不再单带一份（`session_host::options_of` 滤掉），
        // 一个会话里同名的工具只能有一个。
        .chain(std::iter::once(runner::command_tool_spec()))
        .map(|t| {
            let mut schema = t.input_schema;
            if let Some(p) = schema.pointer_mut("/properties/flow") {
                p["description"] = json!(FLOW_TARGET);
            }
            json!({
                "name": t.name,
                "description": t.description,
                "input_schema": schema,
                "read_only": t.read_only,
                "destructive": matches!(t.name.as_str(), "flow_delete" | "flow_cancel" | "flow_force"),
                "subagents": true
            })
        })
        .collect()
}

/// 只读的那些：建会话时给 allow 规则，查看不用每次问人。
pub(crate) fn read_only_names() -> Vec<String> {
    author::tool_specs()
        .into_iter()
        .filter(|t| t.read_only)
        .map(|t| t.name)
        .collect()
}

fn str_of(input: &Value, k: &str) -> Option<String> {
    input.get(k).and_then(Value::as_str).map(str::to_string)
}

/// 这个会话能不能用这个工具。运行里的会话连自己所在的运行也不能操作：一站自己给自己确认、放行、
/// 强制放行，出口条件就形同虚设；它交付走派工器的 `flow_submit`（architecture §8.5）。
fn allowed(scope: &Scope, name: &str, read_only: bool) -> Result<(), String> {
    if read_only {
        return Ok(());
    }
    match scope {
        Scope::Full => Ok(()),
        Scope::Run(mine) if RUN_TOOLS.contains(&name) => Err(format!(
            "not allowed: a session inside a flow run ({mine}) may not operate runs, its own included; it hands its work in with flow_submit, and the user decides the rest"
        )),
        Scope::Run(mine) => Err(format!(
            "not allowed: a session inside a flow run ({mine}) may not change flow definitions or start runs"
        )),
        Scope::Job(job) => Err(format!(
            "not allowed: a session started by a scheduled job ({job}) may only look at flows"
        )),
        Scope::None => Err("not allowed from this session".into()),
    }
}

pub(crate) async fn call(
    f: &Flows,
    rpc: &str,
    scope: &Scope,
    name: &str,
    input: &Value,
) -> Option<Result<String, String>> {
    let session: SessionId = rpc.to_string();
    if f.runner.tools().specs().iter().any(|s| s.name == name) {
        // 主会话没有任务可绑，`flow_command` 要靠项目找包（库只认读的子命令给模型）。
        let r = f
            .runner
            .tools()
            .call_in(
                &session,
                name,
                input.clone(),
                Some(std::path::Path::new(&f.project)),
            )
            .await;
        return Some(if r.is_error { Err(r.text) } else { Ok(r.text) });
    }
    let author = f.author.get()?;
    let spec = author.specs().iter().find(|s| s.name == name)?;
    Some(
        async {
            let mut input = input.clone();
            resolve_number(f, &mut input).await?;
            allowed(scope, name, spec.read_only)?;
            if matches!(name, author::TOOL_UPDATE | author::TOOL_DELETE) {
                let flow = match str_of(&input, "flow") {
                    Some(n) => Some(n),
                    None => f.author_host.current(&session).await,
                };
                if let Some(flow) = flow {
                    unlocked(f, &flow).await?;
                }
            }
            let by = Actor::new(author::CONVERSATION_KIND, rpc);
            let r = author.call(&session, name, input, &by).await;
            if r.is_error { Err(r.text) } else { Ok(r.text) }
        }
        .await,
    )
}

/// `flow` 写的是 `#n`：换成那个流程的名字（右栏流程列表此刻的顺序，`port::order::flows`）。
async fn resolve_number(f: &Flows, input: &mut Value) -> Result<(), String> {
    let Some(key) = str_of(input, "flow") else {
        return Ok(());
    };
    let k = key.trim();
    if !k.starts_with('#') && k.parse::<usize>().is_err() {
        return Ok(());
    }
    let all = f.summaries().await;
    let name = port::order::resolve(&port::order::flows(&all), k, |s| &s.name)
        .map(|s| s.name.clone())
        .ok_or_else(|| format!("no flow {key} in the flow list"))?;
    input["flow"] = json!(name);
    Ok(())
}

/// 修改限制：这个流程还有没结束的运行时不许改、不许删。
async fn unlocked(f: &Flows, flow: &str) -> Result<(), String> {
    let active = f
        .runs(Some(flow), Some(RunStatus::Active))
        .await
        .map_err(|e| e.to_string())?;
    if active.is_empty() {
        return Ok(());
    }
    let ids: Vec<&str> = active.iter().map(|r| r.id.as_str()).collect();
    Err(format!(
        "refused: flow {flow} still has {} run(s) not finished ({}). Cancel them (flow_cancel) or let them finish first.",
        ids.len(),
        ids.join(", ")
    ))
}

/// 库要宿主答的：会话在哪个项目里、它的"当前流程"与"当前运行"、改了什么要告诉界面。
/// Conix 没有选中即当前这回事：会话刚建的流程算它的当前流程，运行里的会话的当前运行是它自己的。
pub(crate) struct ConixAuthorHost {
    pub(crate) flows: Weak<Flows>,
    created: Mutex<HashMap<SessionId, String>>,
    /// `conix.yaml` 的 `flows.command_gates`：指南里照实说命令门禁跑不跑。
    command_gates: bool,
}

impl ConixAuthorHost {
    pub(crate) fn new(flows: Weak<Flows>, command_gates: bool) -> Self {
        Self {
            flows,
            created: Mutex::default(),
            command_gates,
        }
    }

    fn run_of(&self, session: &SessionId) -> Option<String> {
        let hub = self.flows.upgrade()?.hub.upgrade()?;
        match hub.origin_of(session)? {
            port::Origin::FlowStation { run, .. } => Some(run.0),
            _ => None,
        }
    }
}

#[async_trait]
impl AuthorHost for ConixAuthorHost {
    async fn current(&self, session: &SessionId) -> Option<String> {
        if let Some(run) = self.run_of(session) {
            let f = self.flows.upgrade()?;
            return f.engine.record(&run).await.ok().map(|r| r.flow);
        }
        self.created.locked().get(session).cloned()
    }

    /// 实例是项目的：流程从这个项目读、存进它（项目受信才写），运行也在它里面。
    async fn project(&self, _session: &SessionId) -> Option<String> {
        Some(self.flows.upgrade()?.project.clone())
    }

    async fn current_run(&self, session: &SessionId) -> Option<String> {
        self.run_of(session)
    }

    async fn created(&self, session: &SessionId, flow: &str) -> Option<String> {
        self.created
            .locked()
            .insert(session.clone(), flow.to_string());
        None
    }

    fn guide_notes(&self, topic: Option<&str>, zh: bool) -> Option<String> {
        guide_notes(topic, zh, self.command_gates)
    }

    async fn notice(&self, notice: Notice) {
        let Some(f) = self.flows.upgrade() else {
            return;
        };
        match notice {
            Notice::Packs => f.flows_changed().await,
            Notice::Deleted { flow, .. } => f.flow_changed(&flow).await,
            _ => {}
        }
    }
}

/// 指南里留给"应用"的，在 Conix 里是怎样：流程存哪、谁确认、命令门禁开没开、外部审批靠什么。
fn guide_notes(topic: Option<&str>, zh: bool, command_gates: bool) -> Option<String> {
    let cmd = match (command_gates, zh) {
        (true, true) => {
            "命令门禁是打开的（`conix.yaml` 的 `flows.command_gates: trusted`）：项目受信就跑。"
        }
        (false, true) => {
            "命令门禁关着（`conix.yaml` 的 `flows.command_gates: off`）：`kind: command` 的门禁存不进去，用 `check` 或 `script`。"
        }
        (true, false) => {
            "Command gates are on (`flows.command_gates: trusted` in `conix.yaml`): they run in a trusted project."
        }
        (false, false) => {
            "Command gates are off (`flows.command_gates: off` in `conix.yaml`): a `kind: command` gate is not saved; use `check` or `script`."
        }
    };
    let text = match (topic, zh) {
        (None, true) => format!(
            "## 在 Conix 里\n\n- 流程和件存在项目的 `.conix/flows/`（文中的 `.harnix/` 在 Conix 里是 `.conix/`），随项目提交；项目受信才读、才写。新写的流程放进一个流程包：存 `<包>/<流程>` 就建 `.conix/flows/<包>/`，一个包自洽地放它的流程和件（Conix 的 docs/design/layout.md）。\n\
             - 运行在项目的制品目录里有自己的目录（缺省 `conix/runs/<运行>/`，不进 git），包的流程结束后归档进 `conix/<包>/archive/`。Conix 内置一个开发流程包 `xforge`（`xforge/solid`、`xforge/quick`），可以直接用，也可以照它写。\n\
             - 确认、驳回、放行搁置的站、强制放行只能由用户做：你没有这些工具，需要时告诉用户在输入框里打 `/confirm`、`/reject <理由>`、`/release`、`/force <理由>`（可以带运行和站：`/confirm <运行> <站>`）。打回你有 `flow_rework`，用户也可以自己打 `/rework <站> [理由]`。\n\
             - {cmd}\n- 写门禁前读 `flow_guide {{topic: \"gates\"}}`；要人批的读 `flow_guide {{topic: \"confirm\"}}`。"
        ),
        (None, false) => format!(
            "## In Conix\n\n- Flows and parts are saved in the project's `.conix/flows/` (`.harnix/` above is `.conix/` in Conix), committed with it; read and written only while the project is trusted. Put a new flow in a bundle: saving `<bundle>/<flow>` makes `.conix/flows/<bundle>/`, which holds its flows and parts on its own (Conix's docs/design/layout.md).\n\
             - A run has its own directory in the project's artifacts (`conix/runs/<run>/` by default, not committed); a bundle's run is archived to `conix/<bundle>/archive/` once it finished. Conix ships a development bundle, `xforge` (`xforge/solid`, `xforge/quick`): use it as it is, or write one after it.\n\
             - Confirming, rejecting, releasing a held station and forcing are the user's: you have no tools for them; when one is needed, tell the user to type `/confirm`, `/reject <reason>`, `/release` or `/force <reason>` (a run and station may follow: `/confirm <run> <station>`). You send a run back with `flow_rework`; the user may also type `/rework <station> [reason]`.\n\
             - {cmd}\n- Read `flow_guide {{topic: \"gates\"}}` before writing a gate, `flow_guide {{topic: \"confirm\"}}` when someone must approve."
        ),
        (Some("gates"), true) => format!("## 在 Conix 里\n\n- {cmd}\n- `flow_gate_try` 不是只读工具（命令门禁会跑命令），按会话的权限设置可能先问用户。"),
        (Some("gates"), false) => format!("## In Conix\n\n- {cmd}\n- `flow_gate_try` is not read-only (a command gate runs a command): by the session's permission settings it may ask the user first."),
        (Some("confirm"), true) => "## 在 Conix 里\n\n\
            - 确认的人是实例的主人（操作系统用户），在输入框里打 `/confirm` 或 `/reject <理由>`。\n\
            - `who` 的角色在项目的 `.conix/roles.yaml` 里列出谁能确认（`reviewer: [alice, bob]`，按操作系统用户名）；没列出的角色任何人都行。主人不在名单里的站，要用 `via: mcp` 请名单上的人批。\n\
            - `via: mcp` 的站只能由审批系统决定，用户在 Conix 里不能代批。它经 Agix 调 MCP 工具（`mcp.callTool`）建单、轮询。".to_string(),
        (Some("confirm"), false) => "## In Conix\n\n\
            - The one who confirms is the instance's owner (the OS user), typing `/confirm` or `/reject <reason>`.\n\
            - A `who` role lists who may confirm in the project's `.conix/roles.yaml` (`reviewer: [alice, bob]`, by OS user name); a role not listed: anyone. Where the owner is not listed, use `via: mcp` to have a listed person approve.\n\
            - A `via: mcp` station is decided by the approval system alone; the user cannot approve it in Conix. It calls MCP tools through Agix (`mcp.callTool`) to open the request and poll it.".to_string(),
        _ => return None,
    };
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_guide_is_told_how_conix_does_what_it_leaves_to_the_application() {
        let n = guide_notes(None, true, true).unwrap();
        assert!(n.contains(".conix/flows/") && n.contains("trusted"), "{n}");
        assert!(
            guide_notes(Some("gates"), false, false)
                .unwrap()
                .contains("is not saved")
        );
        assert!(
            guide_notes(Some("confirm"), true, true)
                .unwrap()
                .contains("roles.yaml")
        );
        assert!(guide_notes(Some("deliver"), true, true).is_none());
    }

    #[test]
    fn the_librarys_tools_are_declared_with_numbers_accepted_for_flows() {
        let d = declarations();
        let names: Vec<&str> = d.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"flow_update") && names.contains(&"flow_rework"));
        // A person's decisions are not offered to a session at all.
        for t in PERSONS_ONLY {
            assert!(!names.contains(t), "{t}");
        }
        let show = d.iter().find(|t| t["name"] == "flow_show").unwrap();
        assert_eq!(show["read_only"], true);
        assert!(
            show["input_schema"]["properties"]["flow"]["description"]
                .as_str()
                .unwrap()
                .contains("#3")
        );
        assert!(read_only_names().contains(&"flow_list".to_string()));
        assert!(!read_only_names().contains(&"flow_delete".to_string()));
    }

    #[test]
    fn a_run_session_only_looks_and_operates_no_run_not_even_its_own() {
        let mine = Scope::Run("r1".into());
        assert!(allowed(&mine, "flow_list", true).is_ok());
        assert!(allowed(&mine, "flow_pause", false).is_err());
        assert!(allowed(&mine, "flow_force", false).is_err());
        assert!(allowed(&mine, "flow_update", false).is_err());
        assert!(allowed(&mine, "flow_start", false).is_err());
        let job = Scope::Job("j".into());
        assert!(allowed(&job, "flow_show", true).is_ok());
        assert!(allowed(&job, "flow_cancel", false).is_err());
        assert!(allowed(&Scope::Full, "flow_delete", false).is_ok());
    }
}
