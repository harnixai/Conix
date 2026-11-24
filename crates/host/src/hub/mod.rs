//! 会话代理（architecture §8.4）：端口命令 → Agix 方法，Agix 的帧 → 领域事件，再加上作业的
//! 触发。实例是引擎唯一的客户端：它订阅内存里的每个会话，所以状态不依赖 TUI 在不在。

mod attention;
mod config_tools;
mod dispatch;
mod guard;
mod history;
mod jobs;
mod plugins;
mod registry;
mod session_opts;
mod sessions;
mod state;
mod tool_calls;
pub(crate) mod tools;
mod turns;

use crate::sync::Locked;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, Weak};

use async_trait::async_trait;
use port::link::ServerFrame;
use port::{
    Command, Event, GoalAction, InstanceInfo, InstanceName, InstanceState, Job, JobEntry, JobId,
    JobKind, JobOutcome, JobTarget, Origin, PortError, Reply, SessionId, SlashCommand, SubmitMode,
    TaskId, Usage, Watched,
};
use serde_json::{Value, json};
use tokio::sync::mpsc;
use wire::frames::{DaemonEvent, HistoryMessage, SessionEvent, SessionInfo};

pub use config_tools::ConixConfigStore;
pub use registry::{Record, Registry};
pub use session_opts::cli_settings;
pub(crate) use session_opts::{SessionSettings, rule_strings};
use state::{Out, Sess, St, Todo2};

use crate::engine::{Engine, Notification, RpcError};
use crate::jobs::{Fire, JobStore, Scheduler};
use crate::service::{Service, Sink};

pub type Clock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// 流程那一侧（`crate::flows`）：Hub 经它碰流程，不认识流程库的类型。
#[async_trait]
pub(crate) trait FlowPort: Send + Sync {
    /// 端口的流程命令（ListFlows … RunOp）。
    async fn command(&self, cmd: Command) -> Result<Reply, PortError>;
    /// 宿主工具里归流程管的（`flow_*`，以及运行里的会话自己的那三个）；不是它的回 `None`。
    async fn tool(
        &self,
        rpc: &str,
        scope: &tools::Scope,
        name: &str,
        input: &Value,
    ) -> Option<Result<String, String>>;
    /// 流程作业的一次触发：启动一次运行，回运行 ID 与主题。
    async fn start_for_job(
        &self,
        flow: &str,
        subject: &str,
        inputs: Value,
        job: &Job,
    ) -> Result<(port::RunId, String), String>;
    /// 作业起的、还没结束的运行：护栏按它们记账、看超时（architecture §8.5）。
    async fn scheduled_runs(&self) -> Vec<ScheduledRun>;
    /// 流程作业的定义要用的流程：`#n` 或名字认出是哪个，`inputs` 合它的 schema。回流程名。
    async fn check_job_flow(&self, key: &str, inputs: &Value) -> Result<String, String>;
    /// 挂在连接上的流程工具的声明。
    fn tool_declarations(&self) -> Vec<Value>;
    /// 实例项目的信任变了：重新问引擎，项目自己的流程跟着出现或消失。
    async fn trust_changed(&self);
    /// 启用着的插件变了：重建流程包（plugins.md §4）。
    async fn plugins_changed(&self, mounted: Vec<crate::plugins::Mounted>);
    /// 插件之间同名、被遮住的：插件名 → 一条一句。
    fn plugin_conflicts(&self) -> std::collections::BTreeMap<String, Vec<String>>;
}

/// 作业起的一次还没结束的运行。
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ScheduledRun {
    pub job: String,
    pub run: port::RunId,
    pub subject: String,
    pub started_ms: u64,
    /// 运行里所有会话到此刻的用量合计。
    pub tokens: u64,
    /// 已经暂停或正在暂停。
    pub paused: bool,
}

/// 一个会话的帧，原样分给看着它的流程派工器（`Hub::tap`）。
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Tapped {
    /// `text`：这一帧是 `turn_start` 时，这一轮是用哪句话起的（经 `run_turn` 发的才知道）。
    Event {
        turn: Option<String>,
        event: Value,
        text: Option<String>,
    },
    /// `run_turn` 回了错误：这一轮不会有 `turn_complete`（错误帧只发给别的订阅者）。
    /// `text`：这一轮还没开始就失败了时，它是用哪句话起的。
    Failed {
        turn: String,
        text: Option<String>,
        kind: String,
        retryable: bool,
        message: String,
    },
}

/// 协议 §9：会话正在跑一轮（或原地重建），`on_busy: "reject"` 的消息没有投递。
const SESSION_BUSY: i64 = -32015;

/// 协议 §9：没有这个会话。
const SESSION_NOT_FOUND: i64 = -32000;

/// 快照里带多少条历史：再早的翻页没有做，500 条足够接上下文。
const HISTORY_TAIL: u64 = 500;

pub struct HubConfig {
    pub instance: String,
    pub host: String,
    pub project: PathBuf,
    pub version: String,
    pub started_ms: u64,
    pub registry: Registry,
    pub jobs: Option<JobStore>,
    pub clock: Clock,
    /// 引擎自带 provider 的模型（`engine::Embedded::model`）：设置里没定义 `providers` 时的缺省模型。
    pub engine_model: String,
}

#[derive(Clone)]
pub struct Hub(Arc<Inner>);

struct Inner {
    engine: Arc<dyn Engine>,
    instance: String,
    host: String,
    project: String,
    version: String,
    started_ms: u64,
    clock: Clock,
    st: Mutex<St>,
    reg: Mutex<Registry>,
    link: Mutex<Link>,
    jobs: Mutex<Option<Scheduler>>,
    /// 作业触发建的会话 → (作业, 触发时刻, 开始时刻)：那一轮结束时补上结局。
    job_runs: Mutex<std::collections::HashMap<String, (String, u64, u64)>>,
    /// 作业触发出来的会话被停下的原因：那一轮结束时记成 `Stopped` 而不是 `Failed`。
    stop_causes: Mutex<HashMap<String, port::StopCause>>,
    /// 护栏（`guard.rs`）：在跑的简单作业触发的账、没能马上跑的到点触发、作业起的运行上次记到的用量、
    /// 护栏暂停过的运行、额度用完的那一天。
    meters: Mutex<HashMap<String, guard::Meter>>,
    waiting: Mutex<Vec<guard::Waiting>>,
    run_seen: Mutex<HashMap<String, u64>>,
    paused_runs: Mutex<HashSet<String>>,
    exhausted: Mutex<Option<String>>,
    /// 等人处理的事与通知出口（`attention.rs`）。
    alerts: Mutex<attention::Alerts>,
    /// 配置里读不懂、退回缺省的项（`conix.yaml` 的 jobs、notify）：`/doctor` 报出来。
    config_problems: Mutex<Vec<String>>,
    notifier: Mutex<Option<Arc<crate::notify::Notifier>>>,
    /// 侧问分叉出的会话：那一轮结束时把回答作为提示推出去。
    side: Mutex<HashSet<String>>,
    default_model: Mutex<String>,
    /// 可装的插件包在哪些目录（`plugins::package_dirs`）。
    plugin_dirs: Mutex<Vec<PathBuf>>,
    /// `conix.yaml` 的读写（`conix` crate 给），会话里的 `conix_config_set` 用。
    conix_config: OnceLock<Arc<dyn ConixConfigStore>>,
    /// `~/.conix`：切换 provider 前的备份放在它下面的 `config-backups/`。
    config_home: Mutex<Option<PathBuf>>,
    /// 测通 provider 用的临时会话：不收进会话列表。
    probes: Mutex<HashSet<String>>,
    flows: OnceLock<Arc<dyn FlowPort>>,
    /// 流程派工器看着的会话：每帧分一份给它们。
    taps: Mutex<HashMap<String, Vec<mpsc::UnboundedSender<Tapped>>>>,
    /// 经 `run_turn` 发出、还没开始的轮：会话 → (轮, 那句话)，按发出的先后。
    sent: Mutex<HashMap<String, VecDeque<(String, String)>>>,
    /// 实例起来时就有、要等到有人接入才说得出的话（如项目状态被挪进 quarantine）。
    on_attach: Mutex<Vec<String>>,
}

#[derive(Default)]
struct Link {
    sink: Option<Sink>,
    watched: HashSet<String>,
    /// 接入者每换一次加一；会话每被 Unwatch 一次加一。Watch 要等好几个调用，期间换了
    /// 接入者或被 Unwatch 了，它就不该在收尾时把会话加回去。
    epoch: u64,
    unwatched: std::collections::HashMap<String, u64>,
}

impl Link {
    fn generation(&self, id: &str) -> (u64, u64) {
        (self.epoch, self.unwatched.get(id).copied().unwrap_or(0))
    }
}

fn send(link: &Link, st: &St, events: Vec<Event>) {
    if let Some(sink) = &link.sink {
        for ev in events {
            if wanted(&ev, &link.watched, st) {
                let _ = sink.send(ServerFrame::Event { event: ev });
            }
        }
    }
}

fn rpc_err(e: RpcError) -> PortError {
    match e {
        RpcError::Closed => PortError::Disconnected,
        RpcError::Remote { message, .. } => PortError::Failed(message),
    }
}

fn str_field(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn parse_rfc3339_ms(s: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.timestamp_millis().max(0) as u64)
}

impl Hub {
    /// 连上引擎后的第一件事：订阅实例级通知、读出这个项目的会话、订阅其中在内存里的，再起作业调度。
    pub async fn start(
        engine: Arc<dyn Engine>,
        mut notes: mpsc::UnboundedReceiver<Notification>,
        cfg: HubConfig,
    ) -> Result<Hub, RpcError> {
        let project = cfg.project.to_string_lossy().into_owned();
        let st = St {
            ident: state::Ident {
                instance: cfg.instance.clone(),
                host: cfg.host.clone(),
            },
            ..St::default()
        };
        let hub = Hub(Arc::new(Inner {
            engine,
            instance: cfg.instance,
            host: cfg.host,
            project,
            version: cfg.version,
            started_ms: cfg.started_ms,
            clock: cfg.clock,
            st: Mutex::new(st),
            reg: Mutex::new(cfg.registry),
            link: Mutex::new(Link::default()),
            jobs: Mutex::new(None),
            job_runs: Mutex::default(),
            stop_causes: Mutex::default(),
            meters: Mutex::default(),
            waiting: Mutex::default(),
            run_seen: Mutex::default(),
            paused_runs: Mutex::default(),
            exhausted: Mutex::default(),
            alerts: Mutex::default(),
            config_problems: Mutex::default(),
            notifier: Mutex::default(),
            side: Mutex::default(),
            default_model: Mutex::default(),
            plugin_dirs: Mutex::default(),
            conix_config: OnceLock::new(),
            config_home: Mutex::default(),
            probes: Mutex::default(),
            flows: OnceLock::new(),
            taps: Mutex::default(),
            sent: Mutex::default(),
            on_attach: Mutex::default(),
        }));
        // 先认引擎：协议不对时后面的每个调用都会以 INVALID_PARAMS 之类的面目失败，不如在这里一句说清。
        let ping = hub.call("daemon.ping", json!({})).await?;
        crate::engine::check_engine(&ping)?;
        hub.call("daemon.subscribeEvents", json!({})).await?;
        hub.register_tools().await;
        let effective = hub
            .call(
                "config.get",
                json!({"scene": "code", "project_root": hub.0.project, "tier": "effective"}),
            )
            .await;
        if let Err(e) = &effective {
            tracing::warn!("config.get: {e}");
        }
        if let Ok(effective) = effective {
            tracing::debug!(settings = %effective["settings"], "effective settings");
            let name = default_model(&effective["settings"]).unwrap_or(cfg.engine_model);
            tracing::info!(model = %name, "default model");
            *hub.0.default_model.locked() = name;
        }
        hub.load_sessions().await?;
        // 通知在自己的任务里按到达顺序处理：同一会话的帧不能乱序。
        let weak = Arc::downgrade(&hub.0);
        tokio::spawn(async move {
            while let Some(n) = notes.recv().await {
                let Some(inner) = weak.upgrade() else { break };
                Hub(inner).on_notification(n).await;
            }
        });
        if let Some(mut store) = cfg.jobs {
            // 作业定义随项目提交，项目受信才读（architecture §8.5）；先定下来再起调度，免得补跑按空列表算。
            let project = hub.0.project.clone();
            match hub.project_trusted(&project).await {
                Ok(t) => store.set_trusted(t),
                Err(e) => tracing::warn!("project.status: {e}"),
            }
            let fire = Arc::new(Firer(Arc::downgrade(&hub.0)));
            let sched = Scheduler::start(Arc::new(Mutex::new(store)), hub.0.clock.clone(), fire);
            // 上一次实例退出时还没跑完的触发：那一轮已经不会有结局了。按触发记录找，定义读不到的也收拾。
            {
                let store = sched.store();
                let s = store.locked();
                for id in s.history_ids() {
                    for e in s.history(&id, 20) {
                        if e.outcome == JobOutcome::Running {
                            let _ = s.finish(
                                &id,
                                e.fired_ms,
                                JobOutcome::Stopped {
                                    reason: port::StopCause::InstanceStopped,
                                },
                                e.elapsed_ms,
                                e.usage,
                                None,
                            );
                        }
                    }
                }
            }
            *hub.0.jobs.locked() = Some(sched);
            // 护栏的定时检查：超时、作业起的运行的用量、排着的触发。
            let weak = Arc::downgrade(&hub.0);
            tokio::spawn(async move {
                let mut tick =
                    tokio::time::interval(std::time::Duration::from_millis(guard::TICK_MS));
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    let Some(inner) = weak.upgrade() else { break };
                    Hub(inner).guard_tick().await;
                }
            });
        }
        Ok(hub)
    }

    /// 工具挂在连接上：每个会话都有，此后建起来（新建、恢复、重建）的引擎都带上。流程接上后
    /// 再挂一次，连同 `flow_*`。
    async fn register_tools(&self) {
        let mut all = tools::declarations();
        if let (Some(f), Some(a)) = (self.0.flows.get(), all.as_array_mut()) {
            a.extend(f.tool_declarations());
        }
        match self
            .call("daemon.setHostTools", json!({"host_tools": all}))
            .await
        {
            Ok(r) if r["refused"].as_array().is_some_and(|a| !a.is_empty()) => {
                tracing::warn!(refused = %r["refused"], "host tools refused")
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("daemon.setHostTools: {e}"),
        }
    }

    /// 接上流程（实例起来时一次）。
    pub(crate) async fn set_flows(&self, flows: Arc<dyn FlowPort>) {
        if self.0.flows.set(flows).is_ok() {
            self.register_tools().await;
        }
    }

    fn flows(&self) -> Option<Arc<dyn FlowPort>> {
        self.0.flows.get().cloned()
    }

    /// 流程的端口命令（宿主库 `conix.flow.*` 走这里，与界面同一条路）。
    pub(crate) async fn flow_command(&self, cmd: Command) -> Result<Reply, PortError> {
        match self.flows() {
            Some(f) => f.command(cmd).await,
            None => Err(PortError::Failed(
                "flows are not available in this instance".into(),
            )),
        }
    }

    pub(crate) fn downgrade(&self) -> WeakHub {
        WeakHub(Arc::downgrade(&self.0))
    }

    /// 从现在起这个会话的每一帧。会话关掉时流结束。
    pub(crate) fn tap(&self, session: &str) -> mpsc::UnboundedReceiver<Tapped> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.0
            .taps
            .locked()
            .entry(session.to_string())
            .or_default()
            .push(tx);
        rx
    }

    fn tap_send(&self, session: &str, t: Tapped) {
        let mut taps = self.0.taps.locked();
        if let Some(v) = taps.get_mut(session) {
            v.retain(|tx| tx.send(t.clone()).is_ok());
            if v.is_empty() {
                taps.remove(session);
            }
        }
    }

    pub(crate) fn project(&self) -> &str {
        &self.0.project
    }

    /// 护栏的全局缺省与天花板（`conix.yaml` 的 `jobs:`），存在作业存储里：派生的生效值由它算。
    pub fn set_guard_config(&self, g: crate::jobs::guard::GuardConfig) {
        if let Ok(sched) = self.scheduler() {
            sched.store().locked().set_guard_config(g);
        }
    }

    pub(crate) fn guard_config(&self) -> crate::jobs::guard::GuardConfig {
        self.scheduler()
            .map(|s| s.store().locked().guard_config().clone())
            .unwrap_or_default()
    }

    pub fn config_problem(&self, p: String) {
        self.0.config_problems.locked().push(p);
    }

    /// 重读配置之后：读不懂的项换成这一次的。
    pub fn set_config_problems(&self, ps: Vec<String>) {
        *self.0.config_problems.locked() = ps;
    }

    /// 下一个接入的 TUI 在 `Hello` 时收到这句提示，只说一次。
    pub fn notice_on_attach(&self, text: String) {
        self.0.on_attach.locked().push(text);
    }

    /// 实例要停了：调度器不再触发；在跑的作业会话接下来被引擎收掉，那一轮记成"实例停了"而不是失败
    /// （不是作业自己的错，不算进连续失败）。
    pub fn shutdown(&self) {
        if let Some(s) = self.0.jobs.locked().as_ref() {
            s.shutdown();
        }
        let running: Vec<String> = self.0.job_runs.locked().keys().cloned().collect();
        let mut causes = self.0.stop_causes.locked();
        for s in running {
            causes.entry(s).or_insert(port::StopCause::InstanceStopped);
        }
    }

    fn now(&self) -> u64 {
        (self.0.clock)()
    }

    pub(crate) async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.0.engine.call(method, params).await
    }

    fn scheduler(&self) -> Result<Scheduler, PortError> {
        self.0
            .jobs
            .locked()
            .clone()
            .ok_or_else(|| PortError::Unsupported("jobs".into()))
    }

    // ---- 事件出口 ----

    fn emit(&self, out: Out) {
        self.observe(&out.events);
        {
            let st = self.0.st.locked();
            let link = self.0.link.locked();
            send(&link, &st, out.events);
        }
        self.spawn_todo(out.todo);
    }

    /// 改状态与发出事件在同一把锁里：Watch 收尾时取的快照与之后发出的事件之间没有缝，
    /// 快照里有的不会再作为事件来一遍，快照里没有的一定作为事件来。
    fn apply(&self, f: impl FnOnce(&mut St, &mut Out)) {
        let mut out = Out::default();
        let notable = {
            let mut st = self.0.st.locked();
            f(&mut st, &mut out);
            let events = std::mem::take(&mut out.events);
            // 认等人处理的事要再读会话状态，放了锁再认；文字增量这类不挑出来，免得每帧都复制。
            let notable: Vec<Event> = events
                .iter()
                .filter(|e| attention::notable(e))
                .cloned()
                .collect();
            let link = self.0.link.locked();
            send(&link, &st, events);
            notable
        };
        self.observe(&notable);
        self.spawn_todo(out.todo);
    }

    fn spawn_todo(&self, todo: Vec<Todo2>) {
        for t in todo {
            let hub = self.clone();
            tokio::spawn(async move { hub.follow_up(t).await });
        }
    }

    pub(crate) fn emit_events(&self, events: Vec<Event>) {
        self.emit(Out {
            events,
            todo: vec![],
        });
    }

    fn upsert(&self, id: &str) {
        let s = self.0.st.locked().summary(id);
        if let Some(s) = s {
            self.emit_events(vec![Event::SessionUpserted(s)]);
        }
    }
}

fn wanted(ev: &Event, watched: &HashSet<String>, st: &St) -> bool {
    match ev {
        Event::SessionUpserted(_) | Event::SessionRemoved { .. } | Event::TaskUpserted(_) => true,
        Event::TaskOutput { task, .. } => {
            st.task_owner
                .get(&task.0)
                .is_some_and(|o| watched.contains(o))
                || st
                    .sessions
                    .values()
                    .any(|s| s.tasks.contains_key(&task.0) && watched.contains(&s.id))
        }
        e => e.session().is_none_or(|s| watched.contains(&s.0)),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Titler {
    User,
    Engine,
    FirstText,
}

fn rand_suffix() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    format!(
        "{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    )
}

#[async_trait]
impl Service for Hub {
    fn attach(&self, sink: Option<Sink>) {
        let attached = sink.is_some();
        {
            let mut link = self.0.link.locked();
            link.sink = sink;
            link.watched.clear();
            link.epoch += 1;
        }
        if attached {
            let notices = std::mem::take(&mut *self.0.on_attach.locked());
            self.emit_events(
                notices
                    .into_iter()
                    .map(|text| Event::Notice {
                        level: port::NoticeLevel::Warn,
                        text,
                    })
                    .collect(),
            );
        }
        self.attached(attached);
    }

    fn hello(&self) -> (InstanceInfo, bool, bool) {
        let st = self.0.st.locked();
        let primary = st.sessions.values().filter(|s| s.parent.is_none());
        let (mut sessions, mut waiting) = (0, 0);
        for s in primary {
            sessions += 1;
            waiting += s.prompts.len() as u32;
        }
        drop(st);
        let job_list = self
            .0
            .jobs
            .locked()
            .as_ref()
            .map(|s| s.list())
            .unwrap_or_default();
        let jobs = job_list.len() as u32;
        // "等你"与界面的待处理同一个口径：提问、在等人的运行、上次失败或被自动停用的作业。
        waiting += self.0.alerts.locked().waiting_runs() as u32;
        waiting += job_list
            .iter()
            .filter(|j| {
                j.auto_disabled.is_some() || j.last.as_ref().is_some_and(JobOutcome::is_failure)
            })
            .count() as u32;
        (
            InstanceInfo {
                name: InstanceName(self.0.instance.clone()),
                host: self.0.host.clone(),
                pid: Some(std::process::id()),
                project: Some(self.0.project.clone()),
                state: InstanceState::Running,
                started_ms: Some(self.0.started_ms),
                stopped_ms: None,
                version: self.0.version.clone(),
                sessions,
                waiting,
                jobs,
                holder: None,
                restarts: 0,
            },
            self.0.flows.get().is_some(),
            self.0.jobs.locked().is_some(),
        )
    }

    async fn handle(&self, cmd: Command) -> Result<Reply, PortError> {
        self.dispatch(cmd).await
    }
}

/// 流程派工器持有的：Hub 也持有流程那一侧，用弱引用免得互相拽着。
#[derive(Clone)]
pub(crate) struct WeakHub(Weak<Inner>);

impl WeakHub {
    pub(crate) fn upgrade(&self) -> Option<Hub> {
        self.0.upgrade().map(Hub)
    }
}

/// 调度器持有它；用弱引用，免得调度器与 `Hub` 互相拽着谁也放不掉。
struct Firer(Weak<Inner>);

#[async_trait]
impl Fire for Firer {
    async fn fire(&self, job: Job, fired_ms: u64, scheduled: bool) {
        if let Some(inner) = self.0.upgrade() {
            Hub(inner).fire_job(job, fired_ms, scheduled).await;
        }
    }
}

/// 设置里的缺省模型：`model.order` 第一个 provider（没写 order 就是第一个定义的）的 `balanced` 档，
/// 没写就是 `strong` 档（Agix 的规矩：balanced 缺省取 strong）。没定义 `providers` 时引擎用自带的
/// `anthropic`，模型是进程参数，设置里没有，回 `None`。
fn default_model(settings: &Value) -> Option<String> {
    let providers = settings["providers"].as_array()?;
    let id = settings["model"]["order"]
        .as_array()
        .and_then(|o| o.first())
        .and_then(Value::as_str)
        .or_else(|| providers.first()?["id"].as_str())?;
    let p = providers.iter().find(|p| p["id"] == id)?;
    let tiers = &p["tiers"];
    tiers["balanced"]
        .as_str()
        .or_else(|| tiers["strong"].as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod default_model_tests {
    use super::*;

    #[test]
    fn the_first_provider_in_order_gives_its_balanced_tier() {
        let s = json!({"providers": [
            {"id": "a", "tiers": {"strong": "a-big"}},
            {"id": "b", "tiers": {"strong": "b-big", "balanced": "b-mid"}}],
            "model": {"order": ["b", "a"]}});
        assert_eq!(default_model(&s).as_deref(), Some("b-mid"));
        let s = json!({"providers": [{"id": "a", "tiers": {"strong": "a-big"}}]});
        assert_eq!(
            default_model(&s).as_deref(),
            Some("a-big"),
            "没 order 取第一个，没 balanced 取 strong"
        );
        assert_eq!(
            default_model(&json!({})),
            None,
            "没定义 providers：引擎自带的，设置里没有"
        );
    }
}
