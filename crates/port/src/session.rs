use serde::{Deserialize, Serialize};

use crate::{
    InstanceName, JobId, Millis, PromptId, RunId, SessionId, StationId, TaskId, ToolId, Usage,
};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionSummary {
    pub id: SessionId,
    pub title: String,
    pub state: SessionState,
    /// 在等人回答的提问数，含它的子 Agent 的。
    #[serde(default)]
    pub waiting: u32,
    #[serde(default)]
    pub usage: Usage,
    pub provenance: Provenance,
    #[serde(default)]
    pub updated_ms: Millis,
    /// 建会话的时刻：会话列表按它排（`order::sessions`），序号因此不随活跃而跳。
    #[serde(default)]
    pub created_ms: Millis,
    /// 会话被另一个实例打开着（Agix 的会话写者锁在它那里）。
    #[serde(default)]
    pub held_by: Option<InstanceName>,
    /// 会话此刻的权限模式（输入框下显示的就是它，Shift+Tab 改它）。
    #[serde(default)]
    pub permission: crate::PermissionMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    Idle,
    Running,
    /// 有提问在等人，或 goal 受阻。
    Waiting,
}

/// 会话归属（需求 C-12）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    pub instance: InstanceName,
    pub host: String,
    #[serde(default)]
    pub project: Option<String>,
    pub scene: String,
    pub model: String,
    #[serde(default)]
    pub origin: Origin,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    #[default]
    User,
    Job {
        job: JobId,
        name: String,
        fired_ms: Millis,
    },
    FlowStation {
        run: RunId,
        flow_title: String,
        subject: String,
        station: StationId,
        station_title: String,
    },
    SubAgent {
        parent: SessionId,
        parent_title: String,
        task: TaskId,
    },
    Fork {
        from: SessionId,
        from_title: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Goal {
    pub objective: String,
    pub phase: GoalPhase,
    #[serde(default)]
    pub rounds_started: u32,
    pub max_rounds: u32,
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalPhase {
    Active,
    Paused,
    Blocked,
    Complete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Todo {
    pub text: String,
    pub status: TodoStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Done,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    Refusal,
    Cancelled,
    /// 一轮里模型调用次数到顶。
    MaxTurns,
    /// token 预算用完；内容是引擎说明的是谁的预算。
    Budget(String),
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: String,
    pub kind: FileChangeKind,
    /// unified diff；太大或二进制时为空。
    #[serde(default)]
    pub diff: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileChangeKind {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prompt {
    pub id: PromptId,
    pub kind: PromptKind,
    /// 来自子 Agent 或流程站时写明来源（交互 §4.6）。
    #[serde(default)]
    pub from: Option<PromptSource>,
    #[serde(default)]
    pub asked_ms: Millis,
    /// 给不给"本会话总是允许"。Agix 对 ask 规则问出的、插件命令子调用问的都不给（`permit_always:
    /// false`）：选了也只放行这一次，给了这一项就是在骗人。
    #[serde(default = "yes")]
    pub permit_always: bool,
    /// 插件命令的子调用问的：那条命令行（Agix 的 `origin.command`）。人按命令判，不按模型判。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
}

fn yes() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PromptKind {
    Permission {
        tool: String,
        summary: String,
        #[serde(default)]
        paths: Vec<String>,
    },
    Question {
        question: String,
        #[serde(default)]
        header: Option<String>,
        /// 为空表示自由回答。
        #[serde(default)]
        options: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromptSource {
    pub label: String,
    #[serde(default)]
    pub task: Option<TaskId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Answer {
    Permit,
    PermitSession,
    Deny {
        #[serde(default)]
        reason: Option<String>,
    },
    Choice {
        index: usize,
    },
    Text {
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Resolution {
    Answered,
    /// `timed_out` / `cancelled` / `policy`（流程按策略代答）。
    Withdrawn {
        reason: String,
    },
}

/// 会话历史里的一项：Watch 时随快照一起给出，model 据此重建转录。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HistoryItem {
    User {
        text: String,
    },
    Assistant {
        text: String,
    },
    Thinking {
        text: String,
    },
    Tool {
        id: ToolId,
        name: String,
        #[serde(default)]
        input: String,
        #[serde(default)]
        content: String,
        #[serde(default)]
        is_error: bool,
        #[serde(default)]
        file_changes: Vec<FileChange>,
    },
    Notice {
        text: String,
    },
}
