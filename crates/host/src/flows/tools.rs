//! `flow_*` 宿主工具（architecture §8.5）：流程库的编写与运行工具（`harnix_flow::author`），
//! 声明与执行都是库的。Conix 在外面加三样：目标可以写 `#n`（换成名字再交给库）、按发起调用的会话
//! 限作用域、流程还有没结束的运行时不许改不许删。运行里的会话自己的三个工具（`flow_state`、
//! `flow_submit`、`flow_propose`）是派工器的，原样转给它。

use crate::sync::Locked;
use std::collections::HashMap;
use std::sync::{Mutex, Weak};

use async_trait::async_trait;
use harnix_flow::author::{self, AuthorHost, Notice, RUN_TOOLS};
use harnix_flow::runner::SessionId;
use harnix_flow::{Actor, RunStatus};
use serde_json::{Value, json};

use super::Flows;
use crate::hub::tools::Scope;

/// 说给模型：`flow` 参数也收 `#n`。
const FLOW_TARGET: &str = "The flow: its name, or \"#3\" (its number in the flow list right now). Omit for the flow this conversation just created, or the run's own flow in a session inside a run.";

pub(crate) fn declarations() -> Vec<Value> {
    author::tool_specs()
        .into_iter()
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

/// 这个会话能不能用这个工具。运行工具没写 `run` 的，库按会话自己的运行补（[`ConixAuthorHost::current_run`]）。
fn allowed(scope: &Scope, name: &str, read_only: bool, input: &Value) -> Result<(), String> {
    if read_only {
        return Ok(());
    }
    match scope {
        Scope::Full => Ok(()),
        Scope::Run(mine) if RUN_TOOLS.contains(&name) => match str_of(input, "run") {
            None => Ok(()),
            Some(r) if &r == mine => Ok(()),
            Some(_) => Err(format!(
                "not allowed: a session inside a flow run may operate only that run ({mine})"
            )),
        },
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
        let r = f.runner.tools().call(&session, name, input.clone()).await;
        return Some(if r.is_error { Err(r.text) } else { Ok(r.text) });
    }
    let author = f.author.get()?;
    let spec = author.specs().iter().find(|s| s.name == name)?;
    Some(
        async {
            let mut input = input.clone();
            resolve_number(f, &mut input).await?;
            allowed(scope, name, spec.read_only, &input)?;
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
}

impl ConixAuthorHost {
    pub(crate) fn new(flows: Weak<Flows>) -> Self {
        Self {
            flows,
            created: Mutex::default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_librarys_tools_are_declared_with_numbers_accepted_for_flows() {
        let d = declarations();
        let names: Vec<&str> = d.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"flow_update") && names.contains(&"flow_release"));
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
    fn a_run_session_may_operate_only_its_own_run_and_change_no_definition() {
        let mine = Scope::Run("r1".into());
        assert!(allowed(&mine, "flow_list", true, &json!({})).is_ok());
        assert!(allowed(&mine, "flow_pause", false, &json!({"run": "r1"})).is_ok());
        assert!(allowed(&mine, "flow_pause", false, &json!({})).is_ok());
        assert!(allowed(&mine, "flow_pause", false, &json!({"run": "r2"})).is_err());
        assert!(allowed(&mine, "flow_update", false, &json!({"flow": "x"})).is_err());
        assert!(allowed(&mine, "flow_start", false, &json!({"flow": "x"})).is_err());
        let job = Scope::Job("j".into());
        assert!(allowed(&job, "flow_show", true, &json!({})).is_ok());
        assert!(allowed(&job, "flow_cancel", false, &json!({"run": "r1"})).is_err());
        assert!(allowed(&Scope::Full, "flow_delete", false, &json!({})).is_ok());
    }
}
