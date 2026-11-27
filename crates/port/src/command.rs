use serde::{Deserialize, Serialize};

use crate::{
    Answer, ClientInfo, FlowDetail, FlowSummary, Goal, HistoryItem, InstanceInfo, Job, JobCheck,
    JobEntry, JobId, MessageId, Millis, OutcomeKind, Prompt, PromptId, RunId, RunOp, RunSummary,
    RunView, SessionId, SessionSummary, Task, TaskId, Todo,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    // 连接
    /// 链路上的第一帧（architecture §8.3）。实例已被接入时，`force` 为真才踢掉对方。
    Hello {
        client: ClientInfo,
        #[serde(default)]
        force: bool,
    },
    Heartbeat {
        last_input_ms: u64,
    },
    Detach,
    StopInstance,

    // 会话
    ListSessions {
        filter: SessionFilter,
    },
    CreateSession {
        project: Option<String>,
        scene: Option<String>,
        model: Option<String>,
        /// 草稿里 Shift+Tab 选的权限模式，建会话时带上；`None` 用设置的。
        #[serde(default)]
        permission: Option<PermissionMode>,
    },
    /// 订阅一个会话的细节事件，回 `Watched` 快照；会话不在内存时先恢复。
    Watch {
        session: SessionId,
    },
    Unwatch {
        session: SessionId,
    },
    Rename {
        session: SessionId,
        title: String,
    },
    Close {
        session: SessionId,
    },

    // 对话
    Submit {
        session: SessionId,
        text: String,
        mode: SubmitMode,
    },
    Interrupt {
        session: SessionId,
    },
    Answer {
        session: SessionId,
        prompt: PromptId,
        answer: Answer,
    },
    SetModel {
        session: SessionId,
        model: String,
        /// 换到别的 provider 的模型时给它的 id；省略 = 照设置。
        #[serde(default)]
        provider: Option<String>,
    },
    /// 配置了的 provider 各自有哪些模型（`/model` 的选择框）。
    ListModels,
    /// 项目里的文件（`@` 提及的补全）：跟着 `.gitignore`，至多一万个。
    ProjectFiles,
    /// 回滚菜单：还在对话里的每一轮（Agix 的 `session.checkpoints`）。
    Checkpoints {
        session: SessionId,
    },
    /// 退回到某一轮刚结束时（`to_turn` 省略 = 第一轮之前），撤文件、对话或两者（`session.rewind`）。
    Rewind {
        session: SessionId,
        #[serde(default)]
        to_turn: Option<String>,
        scope: RewindScope,
    },
    /// Shift+Tab：切这个会话的权限模式，立即生效、记在会话上（需求 C-20，Agix
    /// `session.setPermissionMode`）。回 `PermissionMode`，以其中实际生效的为准。
    SetPermissionMode {
        session: SessionId,
        mode: PermissionMode,
    },
    /// 信任 / 不再信任实例的项目（Agix 的 `project.trust`）：受信的项目才读它自己的设置、
    /// 流程包，才能往它里面存流程。只由人发（`/trust`），模型的工具做不了这件事。
    TrustProject {
        trusted: bool,
    },
    Commands {
        session: SessionId,
    },
    SideQuestion {
        session: SessionId,
        text: String,
    },

    // goal
    SetGoal {
        session: SessionId,
        objective: String,
        max_rounds: Option<u32>,
    },
    UpdateGoal {
        session: SessionId,
        action: GoalAction,
    },

    // 任务
    StopTask {
        task: TaskId,
    },
    MessageTask {
        task: TaskId,
        text: String,
    },
    TaskOutput {
        task: TaskId,
        from_byte: u64,
    },
    /// 一个会话与它的子 Agent 的整棵任务树（流程、作业页画运行与触发的树时，要没被 Watch 的会话的任务）。
    SessionTasks {
        session: SessionId,
    },

    /// 在 Conix 里关掉的会话（转录还在），关掉的时刻新的在前（交互 §5.2「已关闭」）。
    ListClosedSessions,
    /// 已关闭的会话放回会话列表。
    ReopenSession {
        session: SessionId,
    },
    /// 删掉一个会话：转录、附属数据、子 Agent 的侧链都删，删了就没了。
    DeleteSession {
        session: SessionId,
    },

    // 流程
    ListFlows,
    GetFlow {
        name: String,
    },
    ListRuns {
        flow: String,
    },
    /// 已结束（完成、取消）的运行，不分流程，结束时刻新的在前：至多 `limit` 条，给了 `before_ms`
    /// 就只要结束得比它早的（"更多"）。
    ListEndedRuns {
        limit: u32,
        #[serde(default)]
        before_ms: Option<Millis>,
    },
    GetRun {
        run: RunId,
    },
    /// `flow` 可以是流程名、`<包>/<流程>`，或只有包名（实例按输入选路；输入定不了路又有分诊时，
    /// 往 `session`（人敲命令时的对话）发一轮分诊，回 `Reply::Triage`）。`step` 为真是步进。
    StartRun {
        flow: String,
        subject: String,
        inputs: serde_json::Value,
        #[serde(default)]
        step: bool,
        #[serde(default)]
        session: Option<SessionId>,
    },
    /// 流程包的目录（补全与地址解析用）。
    ListBundles,
    /// 跑包的命令（`/flow:<包>:<命令> …`、流程上下文里 `/<包>:<命令> …`）。`run` 缺省时只有数据目录，
    /// 要用运行目录的子命令报"要在运行里"。回 `Reply::Text`。
    FlowCommand {
        bundle: String,
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        run: Option<RunId>,
    },
    RunOp {
        run: RunId,
        op: RunOp,
    },

    /// 项目的流程包在实例之外变了（`conix flow install` 等）：重列流程。命令行可以不接入、只发这一帧
    /// 当连接的第一帧（architecture §8.3），回 `Ok` 就断开。
    ReloadFlows,
    /// 全局流程库里有、这个项目没装的包名（flows.md §7.12）：找不到包时提示 `conix flow install`。
    ListLibrary,

    // 作业
    ListJobs,
    /// 项目的作业定义在实例之外变了（`conix flow install` 加了包建议的作业）：重读。同 `ReloadFlows`，
    /// 命令行可以不接入、只发这一帧，回 `Ok` 就断开。
    ReloadJobs,
    SaveJob {
        job: Job,
    },
    DeleteJob {
        job: JobId,
    },
    SetJobEnabled {
        job: JobId,
        enabled: bool,
    },
    /// 立即跑一次；这一次可以换主题（流程作业）或补一句（简单作业），不改定义。
    RunJobNow {
        job: JobId,
        #[serde(default)]
        subject: Option<String>,
        #[serde(default)]
        prompt_append: Option<String>,
    },
    /// 新的在前；`before_ms` 翻页，`limit` 缺省 20，`outcome` 按结局筛。
    JobHistory {
        job: JobId,
        #[serde(default)]
        before_ms: Option<Millis>,
        #[serde(default)]
        limit: Option<u32>,
        #[serde(default)]
        outcome: Option<OutcomeKind>,
    },
    /// 校验一份定义，不落盘：下几次触发、护栏的生效值、存不进去的原因。
    ValidateJob {
        job: Job,
    },
    /// 经配置的通知出口发一条测试通知（architecture §8.10）。
    TestNotify,
    /// 自检：引擎的 `daemon.doctor` 加上实例自己的几项。
    Doctor,
    /// `/plugin`：可装的包与装好的插件。
    ListPlugins,
    /// 装一个包（名字或 zip 路径）。装完是停用的：回两份披露，人确认后 `SetPluginEnabled`，
    /// 不确认就 `UninstallPlugin`（plugins.md §3）。
    InstallPlugin {
        source: String,
    },
    SetPluginEnabled {
        name: String,
        enabled: bool,
    },
    UninstallPlugin {
        name: String,
    },
    /// 再看一次一个装好的插件的披露。
    PluginDisclosure {
        name: String,
    },
    /// 停下正在跑的那一次（`fired_ms` 省略 = 正在跑的那次）。
    StopFiring {
        job: JobId,
        #[serde(default)]
        fired_ms: Option<Millis>,
    },
}

impl Command {
    /// 用于 mock 规则匹配与事件日志的命令名。
    pub fn name(&self) -> &'static str {
        match self {
            Command::Hello { .. } => "hello",
            Command::Heartbeat { .. } => "heartbeat",
            Command::Detach => "detach",
            Command::StopInstance => "stop_instance",
            Command::ListSessions { .. } => "list_sessions",
            Command::CreateSession { .. } => "create_session",
            Command::Watch { .. } => "watch",
            Command::Unwatch { .. } => "unwatch",
            Command::Rename { .. } => "rename",
            Command::Close { .. } => "close",
            Command::Submit { .. } => "submit",
            Command::Interrupt { .. } => "interrupt",
            Command::Answer { .. } => "answer",
            Command::SetModel { .. } => "set_model",
            Command::ListModels => "list_models",
            Command::ProjectFiles => "project_files",
            Command::Checkpoints { .. } => "checkpoints",
            Command::Rewind { .. } => "rewind",
            Command::SetPermissionMode { .. } => "set_permission_mode",
            Command::TrustProject { .. } => "trust_project",
            Command::Commands { .. } => "commands",
            Command::SideQuestion { .. } => "side_question",
            Command::SetGoal { .. } => "set_goal",
            Command::UpdateGoal { .. } => "update_goal",
            Command::StopTask { .. } => "stop_task",
            Command::MessageTask { .. } => "message_task",
            Command::TaskOutput { .. } => "task_output",
            Command::SessionTasks { .. } => "session_tasks",
            Command::ListFlows => "list_flows",
            Command::GetFlow { .. } => "get_flow",
            Command::ListRuns { .. } => "list_runs",
            Command::ListClosedSessions => "list_closed_sessions",
            Command::ReopenSession { .. } => "reopen_session",
            Command::DeleteSession { .. } => "delete_session",
            Command::ListEndedRuns { .. } => "list_ended_runs",
            Command::GetRun { .. } => "get_run",
            Command::StartRun { .. } => "start_run",
            Command::RunOp { .. } => "run_op",
            Command::ListBundles => "list_bundles",
            Command::FlowCommand { .. } => "flow_command",
            Command::ReloadFlows => "reload_flows",
            Command::ListLibrary => "list_library",
            Command::ListJobs => "list_jobs",
            Command::ReloadJobs => "reload_jobs",
            Command::SaveJob { .. } => "save_job",
            Command::DeleteJob { .. } => "delete_job",
            Command::SetJobEnabled { .. } => "set_job_enabled",
            Command::RunJobNow { .. } => "run_job_now",
            Command::JobHistory { .. } => "job_history",
            Command::ValidateJob { .. } => "validate_job",
            Command::StopFiring { .. } => "stop_firing",
            Command::TestNotify => "test_notify",
            Command::Doctor => "doctor",
            Command::ListPlugins => "list_plugins",
            Command::InstallPlugin { .. } => "install_plugin",
            Command::SetPluginEnabled { .. } => "set_plugin_enabled",
            Command::UninstallPlugin { .. } => "uninstall_plugin",
            Command::PluginDisclosure { .. } => "plugin_disclosure",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFilter {
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub include_closed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubmitMode {
    Turn,
    Steer,
    Queue,
}

/// 与 Agix 的 `permission_mode` 取值一致（协议 §4.5；程序专用的 `bubble` 不会出现在会话上）。
/// Shift+Tab 不轮到 `DontAsk`，但设置可以把会话夹到它，所以得认得。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    #[default]
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
    DontAsk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalAction {
    Pause,
    Resume,
    Edit {
        objective: String,
        max_rounds: Option<u32>,
    },
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlashCommand {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// `builtin` / `user` / `project` / `plugin` / `mcp`……
    #[serde(default)]
    pub source: String,
}

/// `Watch` 的快照：之后的增量都接在它后面。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watched {
    pub session: SessionSummary,
    #[serde(default)]
    pub history: Vec<HistoryItem>,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub goal: Option<Goal>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub prompts: Vec<Prompt>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    Ok,
    /// `SetPermissionMode` 的回复：`mode` 是会话此刻实际的模式；`clamped` 表示设置把请求夹窄了。
    PermissionMode {
        mode: PermissionMode,
        clamped: bool,
    },
    Hello {
        instance: InstanceInfo,
        flows: bool,
        jobs: bool,
    },
    Sessions(Vec<SessionSummary>),
    Session(SessionSummary),
    Watched(Box<Watched>),
    Submitted {
        message: MessageId,
    },
    Commands(Vec<SlashCommand>),
    Tasks(Vec<Task>),
    TaskOutput {
        text: String,
        next_byte: u64,
        finished: bool,
    },
    Flows(Vec<FlowSummary>),
    Flow(Box<FlowDetail>),
    Runs(Vec<RunSummary>),
    Run(Box<RunView>),
    /// `ListLibrary`。
    Library(Vec<String>),
    RunStarted(RunSummary),
    /// `StartRun` 只给了包、输入定不了路：分诊在 `session` 里开始了，选了流程后模型自己再起运行。
    Triage {
        session: SessionId,
    },
    Bundles(Vec<crate::BundleInfo>),
    /// 命令的文本输出，或站操作 / 换路做了什么：界面当本地输出显示，不进模型。
    Text {
        text: String,
    },
    Jobs(Vec<Job>),
    Job(Job),
    JobHistory(Vec<JobEntry>),
    JobCheck(JobCheck),
    Notified(Vec<crate::SinkResult>),
    Doctor(Vec<crate::HealthCheck>),
    Plugins(crate::Plugins),
    /// 装好（或再看一次）的插件与它的披露。
    Plugin {
        plugin: crate::PluginInfo,
        disclosure: crate::PluginDisclosure,
    },
    Models(Vec<ModelInfo>),
    Files(Vec<String>),
    Checkpoints(Vec<Checkpoint>),
    Rewound {
        removed_turns: u32,
        restored_files: u32,
    },
}

/// 一个可选的模型：哪个 provider 的、叫什么。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelInfo {
    pub provider: String,
    pub id: String,
    #[serde(default)]
    pub display_name: Option<String>,
}

/// 回滚菜单的一轮。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Checkpoint {
    pub turn_id: String,
    /// 开启这一轮的那句话（前 200 个字）。
    pub prompt: String,
    /// 这一轮里被写过的文件。
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub restorable: bool,
}

/// 回滚撤什么。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RewindScope {
    Both,
    Conversation,
    Files,
}
