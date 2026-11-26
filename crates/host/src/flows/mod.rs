//! 流程（architecture §8.6）：分叉自 Harnix 的流程库（`crates/flow`），装在实例里。内核与存储、
//! 流程包、派工器都用库的；会话由 [`session_host`] 经 Hub 开在嵌入的 Agix 上。端口的流程命令、
//! `flow_*` 宿主工具、流程作业的触发都从这里进。

mod approvals;
pub(crate) mod authorize;
pub mod builtin;
mod frames;
mod map;
pub(crate) mod policy;
pub(crate) mod session_host;
mod swap;
#[cfg(test)]
mod tests;
pub(crate) mod tools;

use crate::sync::Locked;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use harnix_flow::author::AuthorTools;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig};
use harnix_flow::{
    Actor, CreateIn, DirPacks, Engine, FileStore, MemPacks, Packs, ProjectLayout, RunQuery,
    RunStatus, Trigger,
};
use port::{Command, Event, Job, PortError, Reply, RunId, RunOp};
use serde_json::Value;
use tokio::sync::broadcast;

use crate::hub::{FlowPort, Hub, tools::Scope};
use session_host::ConixSessionHost;

/// 运行的 worktree 分支：`conix/flow/<运行>`，与引擎自己的 worktree 分开。
const BRANCH_PREFIX: &str = "conix/flow";

/// 项目层目录（`.conix`）里引擎的沙箱让任务会话写的子目录：Agix 0.3.20 起只有 `state/` 下这两个
/// （Agix#93），其余归应用。
const AGENT_DIRS: [&str; 2] = ["state/scratchpad", "state/memory"];

/// 运行的过程目录（相对 `.conix`）：放进 Conix 的 `state/conix/`，跟着 `state/` 不进 git，也跟着认领
/// 一起进 quarantine；不用库缺省的 `runs/`——那要往随仓库提交的 `.conix/.gitignore` 里加一行（Harnix#69）。
const RUNS_DIR: &str = "state/conix/runs";

/// 定义里写的场景就是 Agix 的场景名；Conix 只开 `code`。
const SCENE: &str = "code";

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
    /// 随 Conix 内置的流程包写到这里再读（[`builtin`]）；`None` 不带内置的包。
    pub builtin_dir: Option<PathBuf>,
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

/// 命令门禁的子进程永远拿不到的环境变量，门禁的 `env` 写了也不给：模型 provider 的密钥、Conix 与 Agix 自己的。
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
    /// 内置流程包写在哪（[`FlowsConfig::builtin_dir`]）：插件启停重建流程包时还要垫上它。
    builtin_dir: Option<PathBuf>,
}

/// 垫在项目下面的流程包：插件（按名字）> 库自带的示例包，都只读。没有全局可写层：流程与作业只在项目里，
/// 存的都进项目自己的 `.conix/flows/`（architecture §8.1）。插件带来的门禁与步骤名进目录，流程只能用目录里有的名字。
fn build_packs(
    mounted: &[crate::plugins::Mounted],
    map: &crate::plugins::PluginMap,
    builtin: Option<&Path>,
) -> DirPacks {
    let mut dir = DirPacks::new(builtin.map(Path::to_path_buf)).with_builtin(MemPacks::examples());
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
        ..RunnerConfig::default()
    };
    let mounted = hub.mounted_plugins().await;
    let map = crate::plugins::PluginMap::build(&mounted);
    if let Some(d) = &cfg.builtin_dir
        && let Err(e) = builtin::write(d)
    {
        tracing::warn!(dir = %d.display(), error = %e, "the built-in flow bundles were not written");
    }
    let dir = build_packs(&mounted, &map, cfg.builtin_dir.as_deref());
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
        .with_runs_dir(RUNS_DIR);
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
        builtin_dir: cfg.builtin_dir.clone(),
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
        harnix_flow::wire::defs_list(self.packs().as_ref())
            .into_iter()
            .map(|d| map::flow_summary(&d, active.get(&d.name).copied().unwrap_or(0)))
            .collect()
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
        let (def, _) = packs
            .flow(name)
            .ok_or_else(|| PortError::Failed(format!("no flow {name}")))?;
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
        let input = CreateIn {
            flow: flow.to_string(),
            subject: subject.to_string(),
            inputs,
            project_root: Some(self.project.clone()),
            trigger,
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
            RunOp::Goto { .. } | RunOp::Switch { .. } => {
                return Err(PortError::Unsupported(
                    "station operations by address are not supported yet".into(),
                ));
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
            RunOp::Rework { to, reason } => {
                let reason = reason.as_deref().unwrap_or("rework from Conix");
                e.rework(run, &to.0, None, reason, by).await
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

    /// 启用着的插件变了：重建流程包、换上新的门禁与步骤，列表跟着刷新。在跑的运行不受影响。
    pub(crate) async fn set_plugins(&self, mounted: &[crate::plugins::Mounted]) {
        let map = crate::plugins::PluginMap::build(mounted);
        let dir = build_packs(mounted, &map, self.builtin_dir.as_deref());
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
            // 流程库的包目录与包命令的地址接上之前，TUI 看到的是没有包。
            Command::ListBundles => Ok(Reply::Bundles(vec![])),
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
                ..
            } if step => {
                let _ = (flow, subject, inputs);
                Err(PortError::Unsupported("step pace is not supported yet".into()))
            }
            Command::StartRun {
                flow,
                subject,
                inputs,
                ..
            } => Ok(Reply::RunStarted(
                self.start_run(&flow, &subject, inputs, None).await?,
            )),
            Command::RunOp { run, op } => {
                self.op(&run.0, op).await?;
                Ok(Reply::Ok)
            }
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
