use serde::{Deserialize, Serialize};

use crate::{
    ClientInfo, FileChange, FlowSummary, Goal, Job, JobEntry, JobId, MessageId, Millis, Prompt,
    PromptId, Resolution, RunSummary, RunView, SessionId, SessionSummary, StopReason, Task, TaskId,
    Todo, ToolId, Usage,
};

/// 后端推给 TUI 的领域事件。外部标签、snake_case：场景文件里写成
/// `text_delta: {session: s1, text: "…"}`，与线上形状相同。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    // 连接
    Connection(ConnState),
    Kicked {
        by: ClientInfo,
        at_ms: Millis,
    },
    IdleDetached {
        after_ms: Millis,
    },
    InstanceStopped {
        reason: String,
    },

    // 会话列表：所有会话都推，只推摘要
    SessionUpserted(SessionSummary),
    SessionRemoved {
        session: SessionId,
    },

    // 一轮：只推给 Watch 中的会话
    TurnStarted {
        session: SessionId,
    },
    /// 同一步被重发：丢掉这一步已画的文字。
    StepRestarted {
        session: SessionId,
        step: u32,
    },
    ModelRetry {
        session: SessionId,
        attempt: u32,
        delay_ms: Millis,
        reason: String,
    },
    /// 引擎这次模型调用换了 provider；`task` 不是 `main` 时说明是哪种调用（压缩、标题……）。
    ProviderSwitched {
        session: SessionId,
        from: Option<String>,
        to: String,
        reason: String,
        task: String,
    },
    TextDelta {
        session: SessionId,
        text: String,
    },
    ThinkingDelta {
        session: SessionId,
        text: String,
    },
    ToolStarted {
        session: SessionId,
        id: ToolId,
        name: String,
    },
    ToolInput {
        session: SessionId,
        id: ToolId,
        /// 给人看的参数摘要，如 `cargo test auth`、`src/auth/mod.rs`。
        summary: String,
    },
    ToolProgress {
        session: SessionId,
        id: ToolId,
        message: String,
    },
    ToolFinished {
        session: SessionId,
        id: ToolId,
        #[serde(default)]
        content: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        file_changes: Vec<FileChange>,
    },
    TurnEnded {
        session: SessionId,
        stop: StopReason,
        #[serde(default)]
        usage: Usage,
    },
    TurnFailed {
        session: SessionId,
        message: String,
    },

    // 插话
    SteerDelivered {
        session: SessionId,
        message: MessageId,
    },
    SteerReturned {
        session: SessionId,
        messages: Vec<MessageId>,
    },

    // 提问
    PromptPending {
        session: SessionId,
        prompt: Prompt,
    },
    PromptResolved {
        session: SessionId,
        id: PromptId,
        resolution: Resolution,
    },

    // 会话状态
    GoalChanged {
        session: SessionId,
        goal: Option<Goal>,
    },
    TodosChanged {
        session: SessionId,
        todos: Vec<Todo>,
    },
    Compacting {
        session: SessionId,
    },
    Compacted {
        session: SessionId,
        tokens_before: u64,
        tokens_after: u64,
    },
    CommandsChanged {
        session: SessionId,
    },
    Notice {
        level: NoticeLevel,
        text: String,
    },

    // 任务
    TaskUpserted(Task),
    TaskOutput {
        task: TaskId,
        text: String,
        next_byte: u64,
    },

    // 流程
    FlowUpserted(FlowSummary),
    /// 流程定义删掉了。
    FlowRemoved {
        name: String,
    },
    /// 流程定义变了（在编写会话里改的）：设计卡跟着刷新。
    FlowDetail(Box<crate::FlowDetail>),
    RunUpserted(RunSummary),
    RunDetail(Box<RunView>),

    // 插件
    /// 装、卸、启停之后：`/plugin` 浮层开着就重新列一次。
    PluginsChanged,

    // 作业
    JobUpserted(Job),
    JobRemoved {
        job: JobId,
    },
    JobFired {
        job: JobId,
        entry: JobEntry,
    },

    /// 接入时：上一次有人脱离之后攒下的等人处理的事（architecture §8.10）。
    AwaySummary {
        since_ms: Millis,
        alerts: Vec<crate::Alert>,
    },
}

impl Event {
    /// 事件归属的会话（只对一轮、提问、会话状态这类事件有意义）。
    pub fn session(&self) -> Option<&SessionId> {
        use Event::*;
        match self {
            TurnStarted { session }
            | StepRestarted { session, .. }
            | ModelRetry { session, .. }
            | ProviderSwitched { session, .. }
            | TextDelta { session, .. }
            | ThinkingDelta { session, .. }
            | ToolStarted { session, .. }
            | ToolInput { session, .. }
            | ToolProgress { session, .. }
            | ToolFinished { session, .. }
            | TurnEnded { session, .. }
            | TurnFailed { session, .. }
            | SteerDelivered { session, .. }
            | SteerReturned { session, .. }
            | PromptPending { session, .. }
            | PromptResolved { session, .. }
            | GoalChanged { session, .. }
            | TodosChanged { session, .. }
            | Compacting { session }
            | Compacted { session, .. }
            | CommandsChanged { session }
            | SessionRemoved { session } => Some(session),
            SessionUpserted(s) => Some(&s.id),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnState {
    Connected,
    Lost,
    Reconnecting {
        attempt: u32,
    },
    /// 重连后追平：之后紧跟新的快照。
    Resynced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}
