//! `session.event` 与 `daemon.event` 的载荷（协议 §7、§8），以及几个方法的结果。只放 Codix
//! 读的字段；每个字段都有缺省，载荷里不认识的 `kind` 落到 `Unknown`。

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_creation_input_tokens: u64,
    pub cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    TurnStart {},
    StepStart {
        #[serde(default)]
        step: u32,
    },
    /// 一次模型调用结束；`usage` 只是这一次的（护栏按它记账）。
    StepEnd {
        #[serde(default)]
        usage: Usage,
    },
    ModelRetry {
        #[serde(default)]
        attempt: u32,
        #[serde(default)]
        delay_ms: u64,
        #[serde(default)]
        reason: String,
    },
    TextDelta {
        #[serde(default)]
        text: String,
    },
    ThinkingDelta {
        #[serde(default)]
        text: String,
    },
    ToolUseStart {
        id: String,
        #[serde(default)]
        name: String,
    },
    ToolUse {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    ToolProgress {
        tool_use_id: String,
        #[serde(default)]
        message: String,
    },
    ToolResult {
        id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        content: String,
        #[serde(default)]
        is_error: Option<bool>,
        #[serde(default)]
        file_changes: Vec<FileChange>,
    },
    Prompt(PromptPayload),
    PromptResolved {
        prompt_id: String,
        #[serde(default)]
        resolution: String,
        #[serde(default)]
        reason: Option<String>,
    },
    HostToolCall {
        call_id: String,
        #[serde(default)]
        name: String,
        #[serde(default)]
        input: Value,
    },
    SubagentProgress {
        #[serde(default)]
        agent_label: String,
        agent_session_id: String,
        #[serde(default)]
        task_id: Option<String>,
        event: Box<SessionEvent>,
    },
    SkillsChanged {},
    Notification {
        #[serde(default)]
        message: String,
    },
    Compact {
        #[serde(default)]
        tokens_before: u64,
        #[serde(default)]
        tokens_after: u64,
    },
    UserMessageDelivered {
        message_id: String,
    },
    UserMessagesReturned {
        #[serde(default)]
        messages: Vec<QueuedMessage>,
    },
    TurnComplete {
        #[serde(default)]
        stop_reason: String,
        #[serde(default)]
        usage: Usage,
        /// `budget_exceeded` 时说明是谁的预算（这一轮、会话还是目标）。
        #[serde(default)]
        message: Option<String>,
    },
    Error {
        #[serde(default)]
        message: String,
    },
    TaskStarted {
        task: TaskInfo,
    },
    TaskUpdated {
        task: TaskInfo,
    },
    TaskProgress {
        task_id: String,
        #[serde(default)]
        appended: String,
        #[serde(default)]
        next_byte: u64,
    },
    TaskFinished {
        task: TaskInfo,
    },
    GoalChanged {
        #[serde(default)]
        goal: Option<GoalView>,
    },
    Rewound {},
    /// 会话的权限模式变了：宿主 `session.setPermissionMode`（`source: "host"`）或模型的
    /// `EnterPlanMode` / `ExitPlanMode`（`"model"`）。
    PermissionModeChanged {
        mode: String,
        #[serde(default)]
        source: String,
    },
    SubscriptionDropped {},
    #[serde(other)]
    Unknown,
}

impl SessionEvent {
    /// 读不懂的载荷（不认识的 `kind`，或认识的 `kind` 里字段形状不对）一律当作 `Unknown`。
    pub fn parse(v: &Value) -> SessionEvent {
        serde_json::from_value(v.clone()).unwrap_or(SessionEvent::Unknown)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QueuedMessage {
    pub message_id: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct FileChange {
    pub path: String,
    #[serde(default)]
    pub op: String,
    #[serde(default)]
    pub unified_diff: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "prompt_type", rename_all = "snake_case")]
pub enum PromptPayload {
    Permission {
        prompt_id: String,
        #[serde(default)]
        tool_name: String,
        #[serde(default)]
        message: String,
        #[serde(default)]
        paths: Vec<String>,
    },
    Question {
        prompt_id: String,
        #[serde(default)]
        question: String,
        #[serde(default)]
        header: Option<String>,
        #[serde(default)]
        options: Vec<QuestionOption>,
    },
    McpElicitation {
        prompt_id: String,
        #[serde(default)]
        server: String,
        #[serde(default)]
        message: String,
    },
}

impl PromptPayload {
    pub fn id(&self) -> &str {
        match self {
            PromptPayload::Permission { prompt_id, .. }
            | PromptPayload::Question { prompt_id, .. }
            | PromptPayload::McpElicitation { prompt_id, .. } => prompt_id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct QuestionOption {
    pub key: String,
    #[serde(default)]
    pub label: String,
}

/// `session.tasks` 的条目，也是 `task_*` 帧里的 `task`。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct TaskInfo {
    pub task_id: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub started_ms: i64,
    #[serde(default)]
    pub ended_ms: Option<i64>,
    #[serde(default)]
    pub detail: Value,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub waiting_for: Vec<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct GoalView {
    #[serde(default)]
    pub objective: String,
    #[serde(default)]
    pub phase: String,
    #[serde(default)]
    pub max_rounds: u32,
    #[serde(default)]
    pub rounds_started: u32,
    #[serde(default)]
    pub blocked_reason: Option<String>,
}

/// `daemon.event` 的载荷（协议 §8）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DaemonEvent {
    SessionCreated {
        session_id: String,
        #[serde(default)]
        project_root: Option<String>,
    },
    SessionResumed {
        session_id: String,
        #[serde(default)]
        project_root: Option<String>,
    },
    SessionEvicted {
        session_id: String,
    },
    SessionClosed {
        session_id: String,
    },
    SessionDeleted {
        session_id: String,
    },
    TurnStarted {
        session_id: String,
        #[serde(default)]
        turn_id: String,
        #[serde(default)]
        initiator: String,
    },
    TurnCompleted {
        session_id: String,
        #[serde(default)]
        turn_id: String,
    },
    EventsLagged {},
    #[serde(other)]
    Unknown,
}

impl DaemonEvent {
    pub fn parse(v: &Value) -> DaemonEvent {
        serde_json::from_value(v.clone()).unwrap_or(DaemonEvent::Unknown)
    }
}

/// `session.list` / `session.get` 的条目。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SessionInfo {
    pub session_id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub last_active: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub session_kind: String,
    #[serde(default)]
    pub scene: Option<String>,
    #[serde(default)]
    pub project_root: Option<String>,
    #[serde(default)]
    pub turn_state: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
}

/// `session.history` 里的一条消息（协议 §4.1 加上 `seq` / `origin` / `usage`）。
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct HistoryMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Value,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub usage: Option<Usage>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn unknown_and_malformed_kinds_are_skipped_not_errors() {
        assert_eq!(
            SessionEvent::parse(&json!({"kind":"brand_new","x":1})),
            SessionEvent::Unknown
        );
        assert_eq!(
            SessionEvent::parse(&json!({"kind":"tool_use"})),
            SessionEvent::Unknown
        );
        assert_eq!(
            SessionEvent::parse(&json!({"kind":"text_delta","text":"hi","extra":true})),
            SessionEvent::TextDelta { text: "hi".into() }
        );
        assert_eq!(
            SessionEvent::parse(
                &json!({"kind":"permission_mode_changed","mode":"plan","source":"model"})
            ),
            SessionEvent::PermissionModeChanged {
                mode: "plan".into(),
                source: "model".into()
            }
        );
        assert_eq!(
            DaemonEvent::parse(&json!({"kind":"mcp_connected","server":"x"})),
            DaemonEvent::Unknown
        );
    }

    #[test]
    fn subagent_frames_nest_and_prompts_dispatch_on_prompt_type() {
        let v = json!({"kind":"subagent_progress","agent_label":"探索","agent_session_id":"c1",
            "agent_type":"explore","parent_turn":1,"parent_tool_use_id":null,"task_id":"t1",
            "event":{"kind":"prompt","prompt_type":"question","prompt_id":"p1","tool_use_id":"x",
                     "question":"哪个？","header":null,"options":[{"key":"a","label":"main"}]}});
        let SessionEvent::SubagentProgress { event, task_id, .. } = SessionEvent::parse(&v) else {
            panic!()
        };
        assert_eq!(task_id.as_deref(), Some("t1"));
        let SessionEvent::Prompt(p) = *event else {
            panic!()
        };
        assert_eq!(p.id(), "p1");
    }
}
