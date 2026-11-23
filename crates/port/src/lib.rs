//! Codix 的后端端口：TUI 与后端之间唯一的接口（docs/design/architecture.md §3）。
//!
//! 阶段 2 由 `mock` 实现，阶段 3 由实例客户端实现。这里的消息都能序列化，序列化后就是
//! TUI 与实例之间的协议，所以领域类型只用 Codix 自己的词，不出现 Agix 或 Harnix 的类型。

mod alert;
mod backend;
mod command;
mod common;
mod event;
mod flow;
mod ids;
mod instance;
mod job;
pub mod link;
pub mod order;
mod session;
mod task;

pub use alert::{Alert, AlertKind, AlertTarget, HealthCheck, HealthStatus, SinkResult};
pub use backend::{Backend, Instances, PortError};
pub use command::{
    Checkpoint, Command, GoalAction, ModelInfo, PermissionMode, Reply, RewindScope, SessionFilter,
    SlashCommand, SubmitMode, Watched,
};
pub use common::{Millis, Usage};
pub use event::{ConnState, Event, NoticeLevel};
pub use flow::{
    DefState, FlowDetail, FlowOrigin, FlowSummary, RunOp, RunStatus, RunSummary, RunView,
    StationStatus, StationTask, StationView,
};
pub use ids::{
    InstanceName, JobId, MessageId, PromptId, RunId, SessionId, StationId, TaskId, ToolId,
};
pub use instance::{Attach, ClientInfo, InstanceInfo, InstanceState, NewInstance};
pub use job::{
    AutoDisabled, FiringAudit, Job, JobCheck, JobEntry, JobGuard, JobKind, JobOutcome, JobTarget,
    Network, OutcomeKind, Overlap, SkipCause, StopCause,
};
pub use session::{
    Answer, FileChange, FileChangeKind, Goal, GoalPhase, HistoryItem, Origin, Prompt, PromptKind,
    PromptSource, Provenance, Resolution, SessionState, SessionSummary, StopReason, Todo,
    TodoStatus,
};
pub use task::{Task, TaskKind, TaskStatus};
