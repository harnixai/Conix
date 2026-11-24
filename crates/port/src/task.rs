use serde::{Deserialize, Serialize};

use crate::{Millis, PromptId, SessionId, TaskId, Usage};

/// 一件在会话之外继续跑的工作。`owner` 是派出它的会话；子 Agent 的 `agent_session`
/// 是它自己的侧链会话，它再派的任务以那个会话为 `owner`，所以任务树是递归的。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub owner: SessionId,
    pub kind: TaskKind,
    pub status: TaskStatus,
    /// 子 Agent 的描述、Shell 的命令行、监视器的说明……
    pub label: String,
    #[serde(default)]
    pub agent_type: Option<String>,
    #[serde(default)]
    pub agent_session: Option<SessionId>,
    /// 子 Agent、团队成员实际用的模型与 provider：起时是引擎的预测，调用后按真正答的那家改写。
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub provider: Option<String>,
    #[serde(default)]
    pub started_ms: Millis,
    #[serde(default)]
    pub ended_ms: Option<Millis>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub waiting_for: Vec<PromptId>,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskKind {
    Agent,
    Teammate,
    Shell,
    Monitor,
    Code,
    Mcp,
    Cron,
    #[serde(untagged)]
    Unknown(String),
}

impl TaskKind {
    pub fn has_session(&self) -> bool {
        matches!(self, TaskKind::Agent | TaskKind::Teammate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Running,
    Waiting,
    Done,
    Failed,
    Stopped,
}

impl TaskStatus {
    pub fn finished(self) -> bool {
        matches!(
            self,
            TaskStatus::Done | TaskStatus::Failed | TaskStatus::Stopped
        )
    }
}
