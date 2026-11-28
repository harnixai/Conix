//! 流程（architecture §8.6）：分叉自 Harnix 的流程库（`crates/flow`），装在实例里。内核与存储、
//! 流程包、派工器都用库的；会话由 [`session_host`] 经 Hub 开在嵌入的 Agix 上。端口的流程命令、
//! `flow_*` 宿主工具、流程作业的触发都从这里进。

mod approvals;
pub(crate) mod authorize;
pub mod builtin;
pub mod doctor;
mod frames;
mod goto;
pub mod library;
pub mod manage;
mod map;
pub mod package;
pub(crate) mod policy;
pub(crate) mod session_host;
mod swap;
#[cfg(test)]
mod tests;
pub(crate) mod tools;
mod typed;

use crate::sync::Locked;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use harnix_flow::author::AuthorTools;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig};
use harnix_flow::{
    Actor, CreateIn, DirPacks, Engine, FileStore, Packs, ProjectLayout, RunQuery, RunStatus,
    Trigger,
};
use port::{Command, Event, Job, PortError, Reply, RunId, RunOp};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::hub::{FlowPort, Hub, tools::Scope};
use session_host::ConixSessionHost;

/// 运行的 worktree 分支：`conix/flow/<运行>`，与引擎自己的 worktree 分开。
const BRANCH_PREFIX: &str = "conix/flow";
/// 流程库替运行做的提交（分叉、交付、合回）落在用户的历史里：写 Conix 的名字，不写分叉来源的。
const COMMIT_WORD: &str = "Conix run";

/// 项目层目录（`.conix`）里引擎的沙箱让任务会话写的子目录：Agix 0.3.20 起只有 `state/` 下这两个
/// （Agix#93），其余归应用。
const AGENT_DIRS: [&str; 2] = ["state/scratchpad", "state/memory"];

/// 运行的过程目录（相对 `.conix`）：放进 Conix 的 `state/conix/`，跟着 `state/` 不进 git，也跟着认领
/// 一起进 quarantine；不用库缺省的 `runs/`——那要往随仓库提交的 `.conix/.gitignore` 里加一行（Harnix#69）。
const RUNS_DIR: &str = "state/conix/runs";

/// 定义里写的场景就是 Agix 的场景名；Conix 只开 `code`。
pub(crate) const SCENE: &str = "code";

/// 引擎放 worktree 的目录名（Agix 的 `worktree_dir` 缺省值）：在仓库根，Agix 认得它下面第一层是
/// worktree，把那里的路径映射回主仓库做保护，所以运行的 worktree 只能是 `.wkts/<运行>`，不能再分一层。
pub const WORKTREES_DIR: &str = ".wkts";

pub struct FlowsConfig {
    /// 运行记录：`.conix/state/conix/flow-runs/`。
    pub runs_dir: PathBuf,
    /// 运行的 worktree：[`worktrees_root`]。
    pub worktrees_dir: PathBuf,
    /// 没有项目的运行在这里各有一个目录。
    pub workspaces_dir: PathBuf,
    /// 作业起的运行在会话上多出的限制：`state/conix/run-policies.json`（[`policy`]）。
    pub policies_file: Option<PathBuf>,
    /// 命令门禁跑不跑（`conix.yaml` 的 `flows.command_gates`）；跑也只在受信的项目里（流程库自己判）。
    pub command_gates: bool,
    /// 项目的制品目录名（docs/design/layout.md）：运行的目录、流程包的数据与归档都在它下面；`None` 是流程库原来的布局。
    pub artifacts_dir: Option<String>,
    /// 全局目录（`~/.conix`）：它下面的流程库（[`library`]）不参与运行，只用来在找不到包时提示
    /// `conix flow install`；`None` 不提示。
    pub library_home: Option<PathBuf>,
}

/// 制品目录缺省的名字。
pub const ARTIFACTS_DIR: &str = "conix";

/// 制品目录名能不能用：一个普通的名字，不以 `.` 开头（`.conix`、`.git` 都不行）。
pub fn artifacts_dir_ok(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
}

/// 项目与包起的进程（命令门禁、`plan` 的 run、`flow.exec`）永远拿不到的环境变量，`env` 写了也不给：
/// 模型 provider 的密钥、Conix 与 Agix 自己的。以 `*` 开头的是后缀。
pub const GATE_ENV_DENY: &[&str] = &[
    "*_API_KEY",
    "*_AUTH_TOKEN",
    "ANTHROPIC_*",
    "OPENAI_*",
    "DEEPSEEK_*",
    "GEMINI_*",
    "CONIX_*",
    "AGIX_*",
];

/// 一个项目的流程布局，与实例里的一样（`start`）：命令行（`conix flow doctor`）跑包的体检用。
pub fn project_layout(project: &Path, artifacts_dir: Option<&str>) -> ProjectLayout {
    let mut layout = ProjectLayout::new(".conix", worktrees_root(project), BRANCH_PREFIX)
        .with_agent_dirs(AGENT_DIRS)
        .with_runs_dir(RUNS_DIR)
        .with_commit_word(COMMIT_WORD);
    if let Some(a) = artifacts_dir.filter(|a| artifacts_dir_ok(a)) {
        layout = layout.with_artifacts_dir(a);
    }
    layout
}

/// 项目所在仓库的根下的 [`WORKTREES_DIR`]；不在仓库里就是项目下的（那样的运行开不出 worktree）。
pub fn worktrees_root(project: &Path) -> PathBuf {
    let root = project
        .ancestors()
        .find(|d| d.join(".git").exists())
        .unwrap_or(project);
    root.join(WORKTREES_DIR)
}

/// 引擎只在自己第一次建 worktree 时往仓库根的 `.gitignore` 加 `.wkts/`；流程先建的话主仓库会把
/// 它当成未跟踪的目录。目录里放一个忽略一切的 `.gitignore`，不用去改仓库自己的。
fn prepare_worktrees_dir(dir: &Path) {
    if dir.parent().is_none_or(|r| !r.join(".git").exists()) {
        return;
    }
    let ignore = dir.join(".gitignore");
    let made = std::fs::create_dir_all(dir).and_then(|_| {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&ignore)
        {
            Ok(mut f) => std::io::Write::write_all(&mut f, b"*\n"),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
            Err(e) => Err(e),
        }
    });
    if let Err(e) = made {
        tracing::warn!(dir = %dir.display(), "worktrees directory not prepared: {e}");
    }
}

pub struct Flows {
    hub: crate::hub::WeakHub,
    engine: Arc<Engine>,
    runner: Arc<Runner>,
    project: String,
    /// 流程库的编写与运行工具；派工器起来之后才有。
    author: std::sync::OnceLock<AuthorTools>,
    author_host: Arc<tools::ConixAuthorHost>,
    /// 推给过界面的流程名：流程包变了时据此推 `FlowRemoved`。
    published: std::sync::Mutex<BTreeSet<String>>,
    /// 已结束的运行的摘要：第一次要时把运行记录扫一遍，之后跟着派工器的变化更新，
    /// 免得「已结束」每取一页都把所有运行记录读一遍。
    ended: std::sync::Mutex<Option<Vec<port::RunSummary>>>,
    /// 流程包：插件启停时整体换（[`swap::SwapPacks`]）。
    swap: Arc<swap::SwapPacks>,
    /// 启用着的插件带来的门禁与步骤（`PluginGates` / `PluginPrograms` 按它找插件函数）。
    plugin_map: Arc<std::sync::RwLock<crate::plugins::PluginMap>>,
    /// 插件之间同名、没提供出来的流程与件。
    shadowed: std::sync::Mutex<Vec<harnix_flow::Shadowing>>,
    /// 全局流程库（[`FlowsConfig::library_dir`]）。
    library: Option<library::Library>,
}

/// 垫在项目下面的流程包：只有插件的（按名字），都只读。没有内置层、也不读全局库：项目跑的只有装进项目的
/// `.conix/flows/` 与插件带的（architecture §8.1、flows.md §7.12）。插件带来的门禁与步骤名进目录，流程只能用目录里有的名字。
fn build_packs(mounted: &[crate::plugins::Mounted], map: &crate::plugins::PluginMap) -> DirPacks {
    let mut dir = DirPacks::new(Vec::<PathBuf>::new());
    for m in mounted {
        if let Some(d) = m.flows_dir() {
            dir = dir.with_plugin(m.name.clone(), d);
            if let Some(flows) = m.section.governed.flows() {
                dir = dir.with_governed(m.name.clone(), flows);
            }
        }
    }
    dir.with_catalog(harnix_flow::Catalog {
        gates: map.gates.keys().cloned().collect(),
        steps: map.steps.keys().cloned().collect(),
        ..Default::default()
    })
}

/// 派工器看到的会话宿主，单独拿出来给一致性套件查（`live_flows.rs`）。
pub fn session_host(hub: &Hub) -> Arc<dyn harnix_flow::runner::SessionHost> {
    ConixSessionHost::new(hub.downgrade(), Arc::new(policy::RunPolicies::in_memory()))
}

/// 装好流程、接到 Hub 上、起派工器（它把上次没做完的活接着做）。
pub async fn start(hub: &Hub, cfg: FlowsConfig) -> anyhow::Result<Arc<Flows>> {
    let rcfg = RunnerConfig {
        profiles: vec![SCENE.into()],
        submit_profiles: BTreeSet::from([SCENE.to_string()]),
        main_profile: Some(SCENE.into()),
        project_profiles: BTreeSet::from([SCENE.to_string()]),
        hook_headers: "X-Conix".into(),
        command_gates: cfg.command_gates,
        env_deny: GATE_ENV_DENY.iter().map(|s| s.to_string()).collect(),
        // 项目与包要起的每个进程都经实例的执行服务（flows.md §7.14）。
        exec: Some(crate::exec::service()),
        ..RunnerConfig::default()
    };
    let mounted = hub.mounted_plugins().await;
    let map = crate::plugins::PluginMap::build(&mounted);
    let dir = build_packs(&mounted, &map);
    let shadowed = dir.shadowed();
    let swap = Arc::new(swap::SwapPacks::new(Arc::new(dir)));
    let plugin_map = Arc::new(std::sync::RwLock::new(map));
    let calls = crate::plugins::gates::PluginCalls::new(hub.downgrade(), plugin_map.clone());
    let rcfg = RunnerConfig {
        approvals: Some(Arc::new(approvals::McpApprovals(hub.downgrade()))),
        gates: Some(Arc::new(crate::plugins::gates::PluginGates(calls.clone()))),
        programs: Some(Arc::new(crate::plugins::gates::PluginPrograms(
            calls.clone(),
        ))),
        ..rcfg
    };
    prepare_worktrees_dir(&cfg.worktrees_dir);
    let mut layout = ProjectLayout::new(".conix", cfg.worktrees_dir, BRANCH_PREFIX)
        .with_agent_dirs(AGENT_DIRS)
        .with_runs_dir(RUNS_DIR)
        .with_commit_word(COMMIT_WORD);
    if let Some(a) = cfg.artifacts_dir.as_deref().filter(|a| artifacts_dir_ok(a)) {
        layout = layout.with_artifacts_dir(a);
    }
    let packs = Arc::new(CatalogPacks::over(&rcfg, vec![], swap.clone()));
    packs.set_layout(layout.clone());
    let packs: Arc<dyn Packs> = packs;
    let engine = Arc::new(
        Engine::new(Arc::new(FileStore::new(cfg.runs_dir)), packs)
            .with_lease_ms(rcfg.lease_ms)
            .with_guarded(rcfg.guarded.clone())
            .with_submit_scenes(rcfg.profiles.clone())
            .with_main_scene(SCENE)
            .with_workspaces(cfg.workspaces_dir)
            // 不放进流程包目录（`.conix/flows/`）：那里的每个 yaml 都当流程读。
            .with_authorizer(Arc::new(authorize::ConixAuthorizer {
                roles_file: std::path::Path::new(hub.project()).join(".conix/roles.yaml"),
            }))
            .with_layout(layout),
    );
    let project = hub.project().to_string();
    match hub.project_trusted(&project).await {
        Ok(t) => rcfg.trust.set(&project, t),
        Err(e) => tracing::warn!("project.status: {e}"),
    }
    let policies = Arc::new(match cfg.policies_file {
        Some(p) => policy::RunPolicies::open(p),
        None => policy::RunPolicies::in_memory(),
    });
    let host = ConixSessionHost::new(hub.downgrade(), policies);
    let _ = host.engine.set(engine.clone());
    let _ = calls.engine.set(engine.clone());
    let sandbox = Arc::new(Sandbox::new(
        engine.packs().clone(),
        SandboxConfig {
            guarded: rcfg.guarded.clone(),
            submit_scenes: rcfg.profiles.clone(),
            main_scene: Some(SCENE.into()),
        },
    ));
    let runner = Runner::new(engine.clone(), host, rcfg);
    let flows = Arc::new_cyclic(|me| Flows {
        hub: hub.downgrade(),
        engine,
        runner: runner.clone(),
        project,
        author: std::sync::OnceLock::new(),
        published: Default::default(),
        ended: Default::default(),
        author_host: Arc::new(tools::ConixAuthorHost::new(me.clone(), cfg.command_gates)),
        swap,
        plugin_map,
        shadowed: std::sync::Mutex::new(shadowed),
        library: cfg.library_home.as_deref().map(library::Library::new),
    });
    let _ = flows.author.set(AuthorTools::new(
        &runner,
        sandbox,
        flows.author_host.clone() as Arc<dyn harnix_flow::author::AuthorHost>,
    ));
    tokio::spawn(follow(flows.clone(), runner.changes()));
    hub.set_flows(flows.clone()).await;
    runner.start().await;
    Ok(flows)
}

impl Flows {
    /// 实例收尾：派工器的任务不再等（运行不取消，下次启动接着做）。
    pub async fn stop(&self) {
        if !self.runner.stop(std::time::Duration::from_secs(5)).await {
            tracing::warn!(
                left = self.runner.tasks_left(),
                "flow tasks still running after the stop's grace"
            );
        }
    }

    fn packs(&self) -> Arc<dyn Packs> {
        self.engine.packs_for(Some(&self.project))
    }

    /// TUI 里的操作：实例的主人亲手做的（[`authorize::person`]）。
    fn by() -> Actor {
        authorize::person()
    }

    pub(crate) async fn summaries(&self) -> Vec<port::FlowSummary> {
        let active = self.active_counts().await;
        let packs = self.packs();
        harnix_flow::wire::defs_list(packs.as_ref())
            .into_iter()
            .map(|d| {
                let mut s = map::flow_summary(&d, active.get(&d.name).copied().unwrap_or(0));
                s.bundle = bundle_of(packs.as_ref(), &d.name);
                s
            })
            .collect()
    }

    /// 流程包的目录（`ListBundles`）：每个有 `bundle.yaml` 的包一项。不在包里的流程没有项，TUI 把流程名当包名。
    pub(crate) fn bundles(&self) -> Vec<port::BundleInfo> {
        let packs = self.packs();
        let mut names: BTreeSet<String> = BTreeSet::new();
        for flow in packs.flows() {
            if let Some(b) = bundle_of(packs.as_ref(), &flow) {
                names.insert(b);
            }
        }
        names
            .into_iter()
            .map(|name| {
                let meta = harnix_flow::bundle::meta_of(packs.as_ref(), &name).unwrap_or_default();
                // 选路的顺序（`flows`，没写就按名字）；本地名是文件名的主干，包的主流程是库的 `MAIN_FLOW`。
                let offered = harnix_flow::bundle::offered_stems(packs.as_ref(), &name);
                let mut locals = meta.flow_order(&offered);
                if offered.contains(harnix_flow::bundle::MAIN_FLOW)
                    && !locals.iter().any(|l| l == harnix_flow::bundle::MAIN_FLOW)
                {
                    locals.push(harnix_flow::bundle::MAIN_FLOW.to_string());
                }
                let mut stations: Vec<port::StationId> = vec![];
                for l in &locals {
                    let full = harnix_flow::bundle::flow_name(&name, l);
                    for s in packs
                        .flow(&full)
                        .map(|(d, _)| d.stations)
                        .unwrap_or_default()
                    {
                        let id = port::StationId(s.id);
                        if !stations.contains(&id) {
                            stations.push(id);
                        }
                    }
                }
                port::BundleInfo {
                    title: if meta.title.is_empty() {
                        name.clone()
                    } else {
                        meta.title.clone()
                    },
                    flows: locals,
                    commands: meta
                        .commands
                        .0
                        .iter()
                        .map(|(n, d)| port::CommandInfo {
                            name: n.clone(),
                            read_only: d.read_only.clone(),
                            description: d.description.clone(),
                        })
                        .collect(),
                    stations,
                    triage: meta.start.triage.is_some(),
                    pace: meta.pace.map(map::pace).unwrap_or_default(),
                    name,
                }
            })
            .collect()
    }

    /// 人跑包的命令（`/flow:<包>:<命令> …`）：写的子命令人在运行之外也能跑（模型不能，库按 `by` 拦）。
    pub(crate) async fn flow_command(
        &self,
        bundle: &str,
        command: &str,
        args: Vec<String>,
        run: Option<&str>,
    ) -> Result<String, PortError> {
        let by = Self::by();
        let mut call = CommandCall::new(bundle, command, args, CommandBy::person(by.id));
        call.run = run.map(str::to_string);
        call.project_root = Some(PathBuf::from(&self.project));
        self.runner
            .run_command(call)
            .await
            .map(|r| r.shown())
            .map_err(PortError::Failed)
    }

    /// 分诊的一轮：包的 `start.triage` skill 的正文、主题与已给的输入、各候选流程各收什么输入，
    /// 以及判完怎么起（`flow_start` 带上判出的输入与理由）。写成给模型的英文，同 `flow_start` 自己的回答。
    fn triage_turn(
        &self,
        bundle: &str,
        skill: &str,
        candidates: &[harnix_flow::kernel::route::RouteCandidate],
        subject: &str,
        inputs: &Value,
    ) -> String {
        let body = self
            .packs()
            .skill(skill)
            .map(|(t, _)| harnix_flow::skill_body(&t))
            .unwrap_or_default();
        let flows: Vec<String> = candidates
            .iter()
            .map(|c| {
                format!(
                    "- {} {}: inputs {}",
                    c.flow,
                    if c.title.is_empty() {
                        String::new()
                    } else {
                        format!("({})", c.title)
                    },
                    c.inputs
                )
            })
            .collect();
        format!(
            "Start the flow bundle `{bundle}` for: {subject}\n\n\
             The inputs given ({inputs}) do not decide which of its flows fits. Judge them as the triage skill \
             below says (read what you need of the project), then call `flow_start` with flow `\"{bundle}\"`, \
             `subject`, the `inputs` you judged and your `reason`.\n\n\
             Flows:\n{}\n\nTriage skill ({skill}):\n{body}",
            flows.join("\n")
        )
    }

    /// 换路（`RunOp::Switch`）：`to` 是包里流程的本地名，库认完整名或文件名的主干。
    pub(crate) async fn switch(
        &self,
        run: &str,
        to: &str,
        reason: Option<&str>,
    ) -> Result<String, PortError> {
        let bundle = self
            .engine
            .record(run)
            .await
            .map_err(flow_err)?
            .def
            .bundle
            .ok_or_else(|| {
                PortError::Failed(format!(
                    "run {run} is in no bundle: no other flow to switch to"
                ))
            })?;
        let full = harnix_flow::bundle::flow_name(&bundle, to);
        let reason = reason.unwrap_or("switch from Conix");
        let r = self
            .engine
            .switch(run, &full, reason, None, Self::by())
            .await;
        self.runner.settle(r).await.map_err(flow_err)?;
        Ok(format!("switched the run to {full}"))
    }

    /// 站操作（[`goto`]）：回做了什么或在等什么。
    pub(crate) async fn goto(
        &self,
        run: &str,
        station: &str,
        reason: Option<&str>,
    ) -> Result<String, PortError> {
        let view = self.engine.get(run).await.map_err(flow_err)?;
        let by = Self::by();
        let e = &self.engine;
        let said = match goto::plan(&view, station).map_err(PortError::Failed)? {
            goto::Plan::Say(text) => return Ok(text),
            goto::Plan::Release => {
                self.runner
                    .settle(e.release(run, station, by).await)
                    .await
                    .map_err(flow_err)?;
                format!("released station {station}")
            }
            goto::Plan::Retry(tasks) => {
                for t in &tasks {
                    self.runner
                        .settle(e.retry(run, station, t, by.clone()).await)
                        .await
                        .map_err(flow_err)?;
                }
                format!("retried {} of station {station}", tasks.join(", "))
            }
            goto::Plan::RerunGates => {
                self.runner
                    .settle(e.rerun_gates(run, station, by).await)
                    .await
                    .map_err(flow_err)?;
                format!("ran the gates of station {station} again")
            }
            goto::Plan::Rework => {
                let reason = reason.unwrap_or("rework from Conix");
                self.runner
                    .settle(e.rework(run, station, None, reason, by).await)
                    .await
                    .map_err(flow_err)?;
                format!("sent the run back to station {station}")
            }
        };
        Ok(said)
    }

    /// 每个流程还没结束的运行有几个。
    async fn active_counts(&self) -> std::collections::HashMap<String, u32> {
        let mut out = std::collections::HashMap::new();
        if let Ok(runs) = self.runs(None, Some(RunStatus::Active)).await {
            for r in runs {
                *out.entry(r.flow).or_insert(0) += 1;
            }
        }
        out
    }

    pub(crate) async fn runs(
        &self,
        flow: Option<&str>,
        status: Option<RunStatus>,
    ) -> harnix_flow::Result<Vec<harnix_flow::RunSummary>> {
        self.engine
            .list(RunQuery {
                flow: flow.map(str::to_string),
                status,
                project: Some(self.project.clone()),
                ..RunQuery::default()
            })
            .await
    }

    async fn summary_of(&self, name: &str) -> Option<port::FlowSummary> {
        self.summaries().await.into_iter().find(|f| f.name == name)
    }

    pub(crate) async fn detail(&self, name: &str) -> Result<port::FlowDetail, PortError> {
        let packs = self.packs();
        let (def, _) = packs.flow(name).ok_or_else(|| self.missing(name))?;
        let summary = self
            .summary_of(name)
            .await
            .ok_or_else(|| PortError::Failed(format!("no flow {name}")))?;
        Ok(port::FlowDetail {
            summary,
            stations: map::def_stations(&def),
            yaml: packs.flow_text(name).unwrap_or_default(),
        })
    }

    pub(crate) async fn view(&self, run: &str) -> Result<port::RunView, PortError> {
        let v = self.engine.get(run).await.map_err(flow_err)?;
        let mut view = map::run_view(&v);
        view.def_state = match self.packs().flow(&v.record.flow) {
            None => port::DefState::Deleted,
            Some((_, digest)) if digest != v.record.def_digest => port::DefState::Changed,
            Some(_) => port::DefState::Current,
        };
        Ok(view)
    }

    /// 已结束的运行，结束时刻新的在前。库的列表摘要不带结束时刻与站数，要从运行记录算：第一次
    /// 扫一遍记下来，之后由 `follow` 跟着派工器的变化补（`note_ended`）。
    async fn ended_runs(
        &self,
        limit: u32,
        before_ms: Option<u64>,
    ) -> Result<Vec<port::RunSummary>, PortError> {
        if self.ended.locked().is_none() {
            let mut all = vec![];
            for s in self.runs(None, None).await.map_err(flow_err)? {
                if s.status == RunStatus::Active {
                    continue;
                }
                if let Ok(v) = self.view(&s.id).await {
                    all.push(v.summary);
                }
            }
            all.sort_by_key(|r| std::cmp::Reverse((r.ended_ms.unwrap_or(0), r.id.0.clone())));
            *self.ended.locked() = Some(all);
        }
        let ended = self.ended.locked();
        Ok(ended
            .as_deref()
            .unwrap_or_default()
            .iter()
            .filter(|r| before_ms.is_none_or(|b| r.ended_ms.unwrap_or(0) < b))
            .take(limit as usize)
            .cloned()
            .collect())
    }

    /// 一个运行变了：结束了就记进「已结束」（还没扫过就等第一次要时再扫）。
    fn note_ended(&self, r: &port::RunSummary) {
        let done = is_ended(&r.status);
        let mut ended = self.ended.locked();
        let Some(list) = ended.as_mut() else { return };
        list.retain(|x| x.id != r.id);
        if done {
            list.push(r.clone());
            list.sort_by_key(|r| std::cmp::Reverse((r.ended_ms.unwrap_or(0), r.id.0.clone())));
        }
    }

    pub(crate) async fn start_run(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        trigger: Option<Trigger>,
    ) -> Result<port::RunSummary, PortError> {
        self.start_run_paced(flow, subject, inputs, trigger, false)
            .await
    }

    /// `step`：每一站进站都搁置，人用站操作一站站推（缺省的步调由流程与包定，这里只能要求步进）。
    /// `flow` 是包名时库按包的 `start.route` 选流程。
    pub(crate) async fn start_run_paced(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        trigger: Option<Trigger>,
        step: bool,
    ) -> Result<port::RunSummary, PortError> {
        self.installed_or_hint(flow)?;
        let input = CreateIn {
            flow: flow.to_string(),
            subject: subject.to_string(),
            inputs,
            project_root: Some(self.project.clone()),
            trigger,
            pace: step.then_some(harnix_flow::kernel::def::Pace::Step),
            ..CreateIn::default()
        };
        let out = self
            .runner
            .settle(self.runner.start_run(input, Self::by()).await)
            .await
            .map_err(flow_err)?;
        let id = out
            .get("run")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| PortError::Failed(format!("the run started without an id: {out}")))?;
        Ok(self.view(&id).await?.summary)
    }

    pub(crate) async fn op(&self, run: &str, op: RunOp) -> Result<(), PortError> {
        let by = Self::by();
        let e = &self.engine;
        let r = match &op {
            RunOp::Goto { station, reason } => {
                return self
                    .goto(run, &station.0, reason.as_deref())
                    .await
                    .map(|_| ());
            }
            RunOp::Switch { to, reason } => {
                return self.switch(run, to, reason.as_deref()).await.map(|_| ());
            }
            RunOp::Pause => e.pause(run, by).await,
            RunOp::Resume => e.resume(run, by).await,
            RunOp::Cancel => e.cancel(run, "cancelled from Conix", by).await,
            RunOp::Confirm {
                station,
                reject,
                reason,
            } => {
                e.confirm(run, &station.0, !reject, reason.as_deref(), by)
                    .await
            }
            RunOp::Release { station } => e.release(run, &station.0, by).await,
            RunOp::Force { station, reason } => e.force(run, &station.0, reason, by).await,
            RunOp::Rework { to, tasks, reason } => {
                let reason = reason.as_deref().unwrap_or("rework from Conix");
                let tasks =
                    (!tasks.is_empty()).then(|| tasks.iter().map(|t| t.0.clone()).collect());
                e.rework(run, &to.0, tasks, reason, by).await
            }
            RunOp::EndWorktree { how } => {
                return self.end_worktree(run, *how).await.map(|_| ());
            }
            // 有失败的任务就重做任务；没有就把没过的门禁重跑一遍（门禁 error 之后人看过了，或文件手工改好了）。
            RunOp::Retry { station } => {
                let rec = e.record(run).await.map_err(flow_err)?;
                let task = rec
                    .stations
                    .get(&station.0)
                    .and_then(|s| {
                        s.tasks
                            .iter()
                            .find(|t| t.status == harnix_flow::TaskStatus::Failed)
                    })
                    .map(|t| t.id.clone());
                match task {
                    Some(task) => e.retry(run, &station.0, &task, by).await,
                    None => e.rerun_gates(run, &station.0, by).await,
                }
            }
        };
        self.runner.settle(r).await.map(|_| ()).map_err(flow_err)
    }

    /// 人定结束了的运行的 worktree 怎么收尾（flows.md §6.4）：合进项目当前的分支、留分支、丢掉。回给人看的
    /// 一句；合并冲突时项目不动，报冲突的文件。
    async fn end_worktree(&self, run: &str, how: port::WorktreeEnd) -> Result<String, PortError> {
        use harnix_flow::WorktreeEnd as W;
        let kind = match how {
            port::WorktreeEnd::Merged => W::Merged,
            port::WorktreeEnd::Kept => W::Kept,
            port::WorktreeEnd::Discarded => W::Discarded,
        };
        let out = self
            .runner
            .end_worktree(run, kind, Self::by())
            .await
            .map_err(|e| {
                use harnix_flow::runner::EndError as E;
                match e {
                    E::Flow(f) => flow_err(f),
                    E::Conflict {
                        files,
                        branch,
                        worktree,
                    } => PortError::Failed(i18n::t!(
                        "flows.worktree_conflict",
                        branch = branch,
                        files = files.join(", "),
                        worktree = worktree
                    )),
                    E::Git { message, branch } => PortError::Failed(i18n::t!(
                        "flows.worktree_git_failed",
                        branch = branch,
                        message = message
                    )),
                    E::Lost(m) => PortError::Failed(m),
                }
            })?;
        self.runner.apply(out).await;
        let rec = self.engine.record(run).await.map_err(flow_err)?;
        let w = rec.worktree.as_ref();
        let o = w.and_then(|w| w.outcome.as_ref());
        let branch = w.map(|w| w.branch.as_str()).unwrap_or_default();
        Ok(match (how, o) {
            (port::WorktreeEnd::Merged, Some(o)) => i18n::t!(
                "flows.worktree_merged_at",
                branch = branch,
                into = o.into.as_deref().unwrap_or("-"),
                how = o.how.as_deref().unwrap_or("merge"),
                commit = o.commit.as_deref().map_or("", |c| &c[..c.len().min(7)])
            ),
            (port::WorktreeEnd::Kept, _) => i18n::t!("flows.worktree_kept", branch = branch),
            _ => i18n::t!("flows.worktree_discarded", branch = branch),
        })
    }

    /// 流程包变了（存、删了流程或件）：每个流程的摘要重推一遍，没了的推 `FlowRemoved`。
    pub(crate) async fn flows_changed(&self) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        let now = self.summaries().await;
        let mut events: Vec<Event> = now.iter().cloned().map(Event::FlowUpserted).collect();
        let mut known = self.published.locked();
        for gone in known.iter().filter(|n| !now.iter().any(|f| &f.name == *n)) {
            events.push(Event::FlowRemoved { name: gone.clone() });
        }
        *known = now.iter().map(|f| f.name.clone()).collect();
        drop(known);
        hub.emit_events(events);
    }

    /// 一次运行的过程目录（流程库的投影写在这里，Conix 的 `conix-report.json` 也写在这里）：有制品目录时是
    /// `<项目>/<制品>/runs/<运行>/_run`（docs/design/layout.md），否则是 `.conix/state/conix/runs/<运行>`。
    fn process_dir_of(&self, run: &str) -> Option<PathBuf> {
        let layout = self.engine.layout()?;
        let project = Path::new(&self.project);
        match layout.run_dir(project, run) {
            Some(d) => Some(d.join(harnix_flow::kernel::layout::PROCESS_DIR)),
            None => Some(layout.runs_dir(project).join(run)),
        }
    }

    /// 全局库里有、项目里没装的包：流程标签与地址找不到包时据此提示 `conix flow install`。
    pub(crate) fn library_only(&self) -> Vec<String> {
        let Some(lib) = &self.library else {
            return vec![];
        };
        let installed: BTreeSet<String> = self.bundles().into_iter().map(|b| b.name).collect();
        let flows = self.packs().flows();
        lib.names()
            .into_iter()
            .filter(|n| {
                !installed.contains(n) && !flows.iter().any(|f| f == n) && !self.in_project(n)
            })
            .collect()
    }

    /// 帮助浮层的「样例」：全局库里的内置包（删掉的不列），与这个项目装没装。只列内置的：库里别的包是这台机器上
    /// 的人自己导入、回填的，不是 Conix 拿来给人照着学的样例。
    pub(crate) fn samples(&self) -> Vec<port::SampleBundle> {
        let Some(lib) = &self.library else {
            return vec![];
        };
        builtin::bundles()
            .into_iter()
            .filter_map(|name| {
                let dir = lib.dir(name)?;
                let meta = harnix_flow::bundle::meta(&dir).ok()?;
                let stems = harnix_flow::bundle::flow_stems(&dir);
                Some(port::SampleBundle {
                    name: name.to_string(),
                    version: package::version_of(&meta),
                    title: meta.title.clone(),
                    description: meta.description.clone(),
                    flows: meta.flow_order(&stems),
                    installed: self.in_project(name),
                    jobs: meta.jobs.len() as u32,
                })
            })
            .collect()
    }

    /// 项目的 `.conix/flows/` 里有这个包（或同名的平铺流程）：不看信任——没受信时流程库不读它，但它装着。
    fn in_project(&self, bundle: &str) -> bool {
        let flows = Path::new(&self.project).join(package::PROJECT_FLOWS);
        harnix_flow::bundle::bundle_in(&flows, bundle).is_some()
            || flows.join(format!("{bundle}.yaml")).is_file()
    }

    /// 流程（或包名）在项目里找不到、而它的包在全局库里：回提示怎么装的错。别的情形交给流程库自己判。
    fn installed_or_hint(&self, flow: &str) -> Result<(), PortError> {
        let bundle = flow.split('/').next().unwrap_or(flow);
        if self.packs().flow(flow).is_none() && self.library_only().iter().any(|n| n == bundle) {
            return Err(self.missing(flow));
        }
        Ok(())
    }

    /// 找不到的流程或包：全局库里有就提示怎么装。
    fn missing(&self, name: &str) -> PortError {
        let bundle = name.split('/').next().unwrap_or(name);
        if self.library_only().iter().any(|n| n == bundle) {
            return PortError::Failed(format!(
                "no flow {name} in this project: the flow bundle {bundle} is in the global library but not installed here; run `conix flow install {bundle}` in a terminal"
            ));
        }
        PortError::Failed(format!("no flow {name}"))
    }

    /// 实例起来时：进行中的运行用的包不在项目里了（比如以前直接用内置的 xforge）——定义与门禁钉在运行里，
    /// skill 与模板不钉，跑下去会断。回给人看的提示。
    pub async fn orphaned_runs(&self) -> Vec<String> {
        let packs = self.packs();
        let flows = packs.flows();
        let mut missing: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for r in self
            .engine
            .list(RunQuery {
                status: Some(RunStatus::Active),
                project: Some(self.project.clone()),
                ..RunQuery::default()
            })
            .await
            .unwrap_or_default()
        {
            let bundle = r.flow.split('/').next().unwrap_or(&r.flow).to_string();
            let present = self.in_project(&bundle)
                || flows
                    .iter()
                    .any(|f| f == &r.flow || f.split('/').next() == Some(&bundle));
            if !present {
                missing.entry(bundle).or_default().push(r.id);
            }
        }
        missing
            .into_iter()
            .map(|(b, runs)| {
                format!(
                    "runs in flight use the flow bundle {b}, which is not in this project ({}): run `conix flow install {b}` in a terminal, or `conix flow doctor --fix`",
                    runs.join(", ")
                )
            })
            .collect()
    }

    /// 启用着的插件变了：重建流程包、换上新的门禁与步骤，列表跟着刷新。在跑的运行不受影响。
    pub(crate) async fn set_plugins(&self, mounted: &[crate::plugins::Mounted]) {
        let map = crate::plugins::PluginMap::build(mounted);
        let dir = build_packs(mounted, &map);
        *self.shadowed.locked() = dir.shadowed();
        self.swap.set(Arc::new(dir));
        *self.plugin_map.write().unwrap_or_else(|e| e.into_inner()) = map;
        self.flows_changed().await;
    }

    /// 插件之间同名、被遮住的：插件名 → 一条一句（流程与件来自 harnix-flow，门禁与步骤来自 [`PluginMap`]）。
    pub(crate) fn plugin_conflicts(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        let mut out = self
            .plugin_map
            .read()
            .map(|m| m.conflicts.clone())
            .unwrap_or_default();
        for s in self.shadowed.locked().iter() {
            let kind = s
                .kind
                .map_or("flow".to_string(), |k| format!("{k:?}").to_lowercase());
            out.entry(s.plugin.clone())
                .or_default()
                .push(format!("{kind} `{}` is taken by plugin `{}`", s.name, s.by));
        }
        out
    }

    /// 定义变了（存、删）：列表与设计卡跟着刷新。
    pub(crate) async fn flow_changed(&self, name: &str) {
        let Some(hub) = self.hub.upgrade() else {
            return;
        };
        match self.detail(name).await {
            Ok(d) => hub.emit_events(vec![
                Event::FlowUpserted(d.summary.clone()),
                Event::FlowDetail(Box::new(d)),
            ]),
            Err(_) => hub.emit_events(vec![Event::FlowRemoved {
                name: name.to_string(),
            }]),
        }
    }
}

/// 流程所在的包：`<包>/<流程>`，或包的主流程（以包名出现、包里有 `bundle.yaml`）。
fn bundle_of(packs: &dyn Packs, flow: &str) -> Option<String> {
    match harnix_flow::bundle::split(flow) {
        Some((b, _)) => Some(b.to_string()),
        None => packs
            .bundle_file(flow, harnix_flow::bundle::BUNDLE_FILE)
            .map(|_| flow.to_string()),
    }
}

pub(crate) fn flow_err(e: harnix_flow::FlowError) -> PortError {
    PortError::Failed(e.to_string())
}

/// 派工器每改了运行就报一次：推运行的摘要与详情，以及它所属流程的摘要（在跑的个数）。
fn is_ended(s: &port::RunStatus) -> bool {
    matches!(s, port::RunStatus::Finished | port::RunStatus::Cancelled)
}

async fn follow(flows: Arc<Flows>, mut rx: broadcast::Receiver<harnix_flow::runner::Changed>) {
    loop {
        let changed = match rx.recv().await {
            Ok(c) => c,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        if !changed.substantive {
            continue;
        }
        let Some(hub) = flows.hub.upgrade() else {
            return;
        };
        let mut seen_flows = BTreeSet::new();
        for run in changed.runs {
            let Ok(v) = flows.view(&run).await else {
                continue;
            };
            seen_flows.insert(v.summary.flow.clone());
            flows.note_ended(&v.summary);
            if is_ended(&v.summary.status) {
                hub.write_run_report(&v);
            }
            hub.emit_events(vec![
                Event::RunUpserted(v.summary.clone()),
                Event::RunDetail(Box::new(v)),
            ]);
        }
        for f in seen_flows {
            if let Some(s) = flows.summary_of(&f).await {
                hub.emit_events(vec![Event::FlowUpserted(s)]);
            }
        }
    }
}

#[async_trait]
impl FlowPort for Flows {
    fn process_dir(&self, run: &str) -> Option<PathBuf> {
        self.process_dir_of(run)
    }

    async fn command(&self, cmd: Command) -> Result<Reply, PortError> {
        match cmd {
            Command::ListFlows => Ok(Reply::Flows(self.summaries().await)),
            Command::ListBundles => Ok(Reply::Bundles(self.bundles())),
            Command::FlowCommand {
                bundle,
                command,
                args,
                run,
            } => Ok(Reply::Text {
                text: self
                    .flow_command(&bundle, &command, args, run.as_ref().map(|r| r.0.as_str()))
                    .await?,
            }),
            Command::RunOp {
                run,
                op: RunOp::Goto { station, reason },
            } => Ok(Reply::Text {
                text: self.goto(&run.0, &station.0, reason.as_deref()).await?,
            }),
            Command::RunOp {
                run,
                op: RunOp::Switch { to, reason },
            } => Ok(Reply::Text {
                text: self.switch(&run.0, &to, reason.as_deref()).await?,
            }),
            Command::RunOp {
                run,
                op: RunOp::EndWorktree { how },
            } => Ok(Reply::Text {
                text: self.end_worktree(&run.0, how).await?,
            }),
            Command::GetFlow { name } => Ok(Reply::Flow(Box::new(self.detail(&name).await?))),
            Command::ListRuns { flow } => {
                // 库的列表摘要不带各站，站数要从运行记录算；读不到记录的退回列表摘要。
                let mut out = vec![];
                for s in self.runs(Some(&flow), None).await.map_err(flow_err)? {
                    out.push(match self.view(&s.id).await {
                        Ok(v) => v.summary,
                        Err(_) => map::run_summary(&s),
                    });
                }
                Ok(Reply::Runs(out))
            }
            Command::ListEndedRuns { limit, before_ms } => {
                Ok(Reply::Runs(self.ended_runs(limit, before_ms).await?))
            }
            Command::GetRun { run } => Ok(Reply::Run(Box::new(self.view(&run.0).await?))),
            Command::StartRun {
                flow,
                subject,
                inputs,
                step,
                session,
            } => {
                // 选路先于开运行，库在选路时就报找不到：全局库里有的包要在这之前说怎么装。
                self.installed_or_hint(&flow)?;
                let (subject, inputs) =
                    typed::person_start(self.packs().as_ref(), &flow, subject, inputs);
                // 只给包、输入定不了路：不开运行，让主会话里的模型读项目、判输入，再 `flow_start`。
                if let harnix_flow::kernel::route::Route::Triage {
                    bundle,
                    skill,
                    candidates,
                } = self
                    .engine
                    .route(&flow, &inputs, Some(&self.project))
                    .map_err(flow_err)?
                {
                    let Some(session) = session else {
                        return Err(PortError::Failed(format!(
                            "the inputs do not decide which flow of {bundle} to start, and judging them is a model's: \
                             start it from a chat, or name the flow ({bundle}/<flow>)"
                        )));
                    };
                    let text = self.triage_turn(&bundle, &skill, &candidates, &subject, &inputs);
                    let hub = self
                        .hub
                        .upgrade()
                        .ok_or_else(|| PortError::Failed("the instance is stopping".into()))?;
                    hub.submit_turn(&session.0, text).await?;
                    return Ok(Reply::Triage { session });
                }
                Ok(Reply::RunStarted(
                    self.start_run_paced(&flow, &subject, inputs, None, step)
                        .await?,
                ))
            }
            Command::RunOp { run, op } => {
                self.op(&run.0, op).await?;
                Ok(Reply::Ok)
            }
            Command::ReloadFlows => {
                self.flows_changed().await;
                Ok(Reply::Ok)
            }
            Command::ListLibrary => Ok(Reply::Library(self.library_only())),
            Command::ListSamples => Ok(Reply::Samples(self.samples())),
            other => Err(PortError::Unsupported(other.name().into())),
        }
    }

    async fn tool(
        &self,
        rpc: &str,
        scope: &Scope,
        name: &str,
        input: &Value,
    ) -> Option<Result<String, String>> {
        tools::call(self, rpc, scope, name, input).await
    }

    async fn start_for_job(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        job: &Job,
    ) -> Result<(RunId, String), String> {
        let trigger = Trigger::Schedule {
            schedule: job.id.0.clone(),
            fire_at: crate::instance::now_ms(),
        };
        let s = self
            .start_run(flow, subject, inputs, Some(trigger))
            .await
            .map_err(|e| e.to_string())?;
        Ok((s.id, s.subject))
    }

    async fn scheduled_runs(&self) -> Vec<crate::hub::ScheduledRun> {
        let mut out = vec![];
        for s in self
            .runs(None, Some(RunStatus::Active))
            .await
            .unwrap_or_default()
        {
            let Ok(v) = self.engine.get(&s.id).await else {
                continue;
            };
            let harnix_flow::Trigger::Schedule { schedule, .. } = &v.record.trigger else {
                continue;
            };
            let view = map::run_view(&v);
            out.push(crate::hub::ScheduledRun {
                job: schedule.clone(),
                run: view.summary.id.clone(),
                subject: view.summary.subject.clone(),
                started_ms: view.summary.started_ms,
                tokens: view.summary.usage.billed(),
                paused: matches!(
                    view.summary.status,
                    port::RunStatus::Paused | port::RunStatus::Pausing
                ),
            });
        }
        out
    }

    async fn check_job_flow(&self, key: &str, inputs: &Value) -> Result<String, String> {
        let flows = self.summaries().await;
        let name = port::order::resolve(&port::order::flows(&flows), key, |f| &f.name)
            .map(|f| f.name.clone())
            .ok_or_else(|| format!("no flow {key} in this project (flow_list lists them)"))?;
        let (def, _) = self
            .packs()
            .flow(&name)
            .ok_or_else(|| format!("no flow {name}"))?;
        let schema = def.inputs_schema();
        let empty = Value::Object(Default::default());
        let inputs = if inputs.is_null() { &empty } else { inputs };
        let v = jsonschema::validator_for(&schema)
            .map_err(|e| format!("flow {name}: its inputs schema is not valid: {e}"))?;
        let errs: Vec<String> = v
            .iter_errors(inputs)
            .map(|e| {
                let at = e.instance_path().to_string();
                if at.is_empty() {
                    e.to_string()
                } else {
                    format!("{at}: {e}")
                }
            })
            .collect();
        if errs.is_empty() {
            Ok(name)
        } else {
            Err(format!("inputs for flow {name}: {}", errs.join("; ")))
        }
    }

    fn tool_declarations(&self) -> Vec<Value> {
        tools::declarations()
    }

    async fn trust_changed(&self) {
        // hear 问的是引擎此刻的答案，并记进派工器与流程包共用的信任表。
        self.runner.hear(&self.project).await;
        self.flows_changed().await;
    }

    async fn plugins_changed(&self, mounted: Vec<crate::plugins::Mounted>) {
        self.set_plugins(&mounted).await;
    }

    fn plugin_conflicts(&self) -> std::collections::BTreeMap<String, Vec<String>> {
        Flows::plugin_conflicts(self)
    }

    async fn project_runs(&self, ended: u32) -> Result<Vec<port::RunSummary>, String> {
        let mut out = vec![];
        for s in self
            .runs(None, Some(RunStatus::Active))
            .await
            .map_err(|e| e.to_string())?
        {
            out.push(match self.view(&s.id).await {
                Ok(v) => v.summary,
                Err(_) => map::run_summary(&s),
            });
        }
        out.sort_by_key(|r| std::cmp::Reverse((r.started_ms, r.id.0.clone())));
        out.extend(
            self.ended_runs(ended, None)
                .await
                .map_err(|e| e.to_string())?,
        );
        Ok(out)
    }

    async fn run_view(&self, run: &str) -> Result<port::RunView, String> {
        self.view(run).await.map_err(|e| e.to_string())
    }

    async fn rerun(&self, run: &str) -> Result<port::RunSummary, String> {
        let v = self.engine.get(run).await.map_err(|e| e.to_string())?;
        let r = &v.record;
        if r.status == RunStatus::Active {
            return Err(format!(
                "run {run} has not ended: pause, resume or cancel it instead (flow_pause / flow_resume / flow_cancel)"
            ));
        }
        self.start_run(&r.flow, &r.subject, r.inputs.clone(), None)
            .await
            .map_err(|e| e.to_string())
    }
}
