//! 实例记着的会话状态，以及 Agix 的帧 → 领域事件的翻译（architecture §8.4 的对照表）。
//! 这里不做 IO：帧进来、状态变、吐出事件与要 `Hub` 去做的事，所以整张对照表能用 JSON 夹具测。

use std::collections::{BTreeMap, HashMap};

use port::{
    Event, Goal, GoalPhase, Origin, Prompt, PromptId, PromptKind, PromptSource, Provenance,
    Resolution, SessionId, SessionState, SessionSummary, StopReason, Task, TaskId, TaskKind,
    TaskStatus, Todo, ToolId, Usage,
};
use serde_json::Value;
use wire::frames::{GoalView, PromptPayload, SessionEvent, TaskInfo};

use super::history;

/// 一个会话。侧链（子 Agent）也是一个，`parent` 指向派它的会话。
#[derive(Debug, Clone, Default)]
pub struct Sess {
    pub id: String,
    pub title: String,
    pub project: Option<String>,
    pub scene: String,
    pub model: String,
    pub origin: Origin,
    pub parent: Option<String>,
    /// 引擎的这个会话在内存里、实例订阅着它。
    pub live: bool,
    pub running: bool,
    pub prompts: Vec<Prompt>,
    pub tasks: BTreeMap<String, Task>,
    pub goal: Option<Goal>,
    pub todos: Vec<Todo>,
    pub usage: Usage,
    pub updated_ms: u64,
    pub created_ms: u64,
    /// 会话此刻的权限模式：建会话时给的、`session.resume` 报的、`permission_mode_changed` 帧改的。
    pub permission: port::PermissionMode,
    pub(super) last_step: Option<u32>,
    pub(super) step_mark: usize,
    /// 这一轮到此刻的回答文字：快照里补上正在写的那段，作业与侧问取它当结果。
    pub turn_text: String,
    pub(super) tools_started: Vec<String>,
}

impl Sess {
    /// 正在写、还没随 `step_end` 落盘的那一步的文字。
    pub fn pending_step_text(&self) -> String {
        self.turn_text
            .get(self.step_mark..)
            .unwrap_or_default()
            .to_string()
    }
}

/// 回答一个提问要知道的：用哪个会话答、答哪一种、选项的 key。
#[derive(Debug, Clone, PartialEq)]
pub struct PromptMeta {
    pub rpc_session: String,
    pub prompt_type: String,
    pub keys: Vec<String>,
}

/// 翻译之外要 `Hub` 去做的事。
#[derive(Debug, Clone, PartialEq)]
pub enum Todo2 {
    /// 宿主工具被调用了：执行后用 `rpc_session` 回答。
    HostCall {
        rpc_session: String,
        call_id: String,
        name: String,
        input: Value,
    },
    /// 一轮结束了（成功与否）：作业的触发记录、侧问的结果从这里补。
    TurnDone {
        session: String,
        ok: bool,
        text: String,
        usage: Usage,
        /// 引擎因为 token 预算用完结束了这一轮（会话、轮或目标的预算）。
        budget: bool,
    },
    /// 订阅被引擎摘掉了（慢连接），重新订阅。
    Resubscribe { session: String },
}

#[derive(Debug, Default)]
pub struct Out {
    pub events: Vec<Event>,
    pub todo: Vec<Todo2>,
}

#[derive(Debug, Clone, Default)]
pub struct Ident {
    pub instance: String,
    pub host: String,
}

#[derive(Debug, Default)]
pub struct St {
    pub ident: Ident,
    pub sessions: BTreeMap<String, Sess>,
    pub prompts: HashMap<String, PromptMeta>,
    /// 任务 → 派它的会话。停任务、读输出要带会话 id。
    pub task_owner: HashMap<String, String>,
}

impl St {
    pub fn summary(&self, id: &str) -> Option<SessionSummary> {
        Some(self.summary_of(self.sessions.get(id)?))
    }

    /// 不在会话表里的（已关闭的）也能拼一份摘要。
    pub fn summary_of(&self, s: &Sess) -> SessionSummary {
        let blocked = s
            .goal
            .as_ref()
            .is_some_and(|g| g.phase == GoalPhase::Blocked);
        let state = if !s.prompts.is_empty() || blocked {
            SessionState::Waiting
        } else if s.running {
            SessionState::Running
        } else {
            SessionState::Idle
        };
        SessionSummary {
            id: SessionId(s.id.clone()),
            title: s.title.clone(),
            state,
            waiting: s.prompts.len() as u32,
            usage: s.usage,
            provenance: Provenance {
                instance: port::InstanceName(self.ident.instance.clone()),
                host: self.ident.host.clone(),
                project: s.project.clone(),
                scene: s.scene.clone(),
                model: s.model.clone(),
                origin: s.origin.clone(),
            },
            updated_ms: s.updated_ms,
            created_ms: s.created_ms,
            permission: s.permission,
            held_by: None,
        }
    }

    fn upsert(&self, id: &str, out: &mut Out) {
        if let Some(s) = self.summary(id) {
            out.events.push(Event::SessionUpserted(s));
        }
    }

    /// 侧链会话第一次露面（任务帧或 `subagent_progress`）时建起来。
    fn ensure_sidechain(&mut self, child: &str, parent: &str, task: &str, label: &str) {
        if self.sessions.contains_key(child) {
            return;
        }
        let p = self.sessions.get(parent).cloned().unwrap_or_default();
        self.sessions.insert(
            child.to_string(),
            Sess {
                id: child.to_string(),
                title: label.to_string(),
                project: p.project.clone(),
                scene: p.scene.clone(),
                model: p.model.clone(),
                origin: Origin::SubAgent {
                    parent: SessionId(parent.to_string()),
                    parent_title: p.title.clone(),
                    task: TaskId(task.to_string()),
                },
                parent: Some(parent.to_string()),
                ..Sess::default()
            },
        );
    }

    /// 一帧 `session.event`。`sid` 是帧所在的会话；`rpc` 是回答提问、宿主工具要用的会话
    /// （子 Agent 的帧经父会话发出，答也用父会话）。
    pub fn on_event(&mut self, sid: &str, rpc: &str, ev: SessionEvent, now: u64, out: &mut Out) {
        let session = || SessionId(sid.to_string());
        // 宿主工具挂在连接上，哪个会话调都会来；不认识的会话也得回答，否则那一轮卡到超时。
        // 能动什么由 Hub 按会话查，不认识的什么都不给。
        if let SessionEvent::HostToolCall {
            call_id,
            name,
            input,
        } = ev
        {
            out.todo.push(Todo2::HostCall {
                rpc_session: rpc.to_string(),
                call_id,
                name,
                input,
            });
            return;
        }
        // 不认识的会话不凭一帧建出来：关掉的会话还会迟到几帧（任务收尾至多 15 秒），
        // 建出来就是一个没有项目、没有归属的幽灵条目。
        let Some(s) = self.sessions.get_mut(sid) else {
            return;
        };
        match ev {
            SessionEvent::TurnStart {} => {
                s.running = true;
                s.turn_text.clear();
                s.last_step = None;
                s.step_mark = 0;
                s.tools_started.clear();
                s.updated_ms = now;
                out.events.push(Event::TurnStarted { session: session() });
                self.upsert(sid, out);
            }
            SessionEvent::StepEnd { .. } => s.step_mark = s.turn_text.len(),
            SessionEvent::StepStart { step } => {
                if s.last_step == Some(step) {
                    s.turn_text.truncate(s.step_mark);
                    out.events.push(Event::StepRestarted {
                        session: session(),
                        step,
                    });
                } else {
                    s.step_mark = s.turn_text.len();
                }
                s.last_step = Some(step);
            }
            SessionEvent::ModelRetry {
                attempt,
                delay_ms,
                reason,
            } => out.events.push(Event::ModelRetry {
                session: session(),
                attempt,
                delay_ms,
                reason,
            }),
            SessionEvent::ProviderSwitched {
                from,
                to,
                reason,
                task,
            } => out.events.push(Event::ProviderSwitched {
                session: session(),
                from,
                to,
                reason,
                task,
            }),
            SessionEvent::TextDelta { text } => {
                s.turn_text.push_str(&text);
                out.events.push(Event::TextDelta {
                    session: session(),
                    text,
                });
            }
            SessionEvent::ThinkingDelta { text } => out.events.push(Event::ThinkingDelta {
                session: session(),
                text,
            }),
            SessionEvent::ToolUseStart { id, name } => {
                s.tools_started.push(id.clone());
                out.events.push(Event::ToolStarted {
                    session: session(),
                    id: ToolId(id),
                    name,
                });
            }
            SessionEvent::ToolUse { id, name, input } => {
                if !s.tools_started.contains(&id) {
                    s.tools_started.push(id.clone());
                    out.events.push(Event::ToolStarted {
                        session: session(),
                        id: ToolId(id.clone()),
                        name: name.clone(),
                    });
                }
                out.events.push(Event::ToolInput {
                    session: session(),
                    id: ToolId(id),
                    summary: history::tool_summary(&name, &input),
                });
                if name == "TodoWrite"
                    && let Some(t) = history::todos_from_input(&input)
                {
                    s.todos = t.clone();
                    out.events.push(Event::TodosChanged {
                        session: session(),
                        todos: t,
                    });
                }
            }
            SessionEvent::ToolProgress {
                tool_use_id,
                message,
            } => out.events.push(Event::ToolProgress {
                session: session(),
                id: ToolId(tool_use_id),
                message,
            }),
            SessionEvent::ToolResult {
                id,
                content,
                is_error,
                file_changes,
                ..
            } => out.events.push(Event::ToolFinished {
                session: session(),
                id: ToolId(id),
                content,
                is_error: is_error.unwrap_or(false),
                file_changes: history::file_changes(&file_changes),
            }),
            SessionEvent::Prompt(p) => {
                // 子 Agent 的询问带它的任务（`prompt_task`）；任务表里的 `waiting_for` 可能比询问晚到，只作退路。
                let from = p
                    .task_id()
                    .and_then(|id| s.tasks.get(id))
                    .or_else(|| {
                        s.tasks
                            .values()
                            .find(|t| t.waiting_for.iter().any(|w| w.0 == p.id()))
                    })
                    .map(|t| PromptSource {
                        label: t.label.clone(),
                        task: Some(t.id.clone()),
                    });
                self.add_prompt(sid, rpc, p, from, now, out);
            }
            SessionEvent::PromptResolved {
                prompt_id,
                resolution,
                reason,
            } => self.resolve_prompt(&prompt_id, &resolution, reason, out),
            SessionEvent::HostToolCall { .. } => {}
            SessionEvent::SubagentProgress {
                agent_label,
                agent_session_id,
                task_id,
                event,
            } => {
                let task = task_id.unwrap_or_default();
                self.ensure_sidechain(&agent_session_id, sid, &task, &agent_label);
                match *event {
                    // 子 Agent 的提问挂在父会话上、注明来源（交互 §4.6），答也经父会话。
                    SessionEvent::Prompt(p) => {
                        let from = Some(PromptSource {
                            label: agent_label,
                            task: (!task.is_empty()).then_some(TaskId(task)),
                        });
                        self.add_prompt(sid, rpc, p, from, now, out);
                    }
                    SessionEvent::PromptResolved {
                        prompt_id,
                        resolution,
                        reason,
                    } => self.resolve_prompt(&prompt_id, &resolution, reason, out),
                    inner => self.on_event(&agent_session_id, rpc, inner, now, out),
                }
            }
            SessionEvent::SkillsChanged {} => out
                .events
                .push(Event::CommandsChanged { session: session() }),
            SessionEvent::Notification { message } => out.events.push(Event::Notice {
                level: port::NoticeLevel::Info,
                text: message,
            }),
            SessionEvent::Compact {
                tokens_before,
                tokens_after,
            } => out.events.push(Event::Compacted {
                session: session(),
                tokens_before,
                tokens_after,
            }),
            // 自动（到阈值、请求太大）与手动的压缩都有这一帧；子 Agent 的经 `subagent_progress` 落到它的侧链。
            SessionEvent::CompactStarted { .. } => {
                out.events.push(Event::Compacting { session: session() })
            }
            SessionEvent::CompactEnded { outcome, reason } => {
                let outcome = match outcome.as_str() {
                    "declined" => port::CompactOutcome::Declined,
                    "nothing_to_compact" => port::CompactOutcome::NothingToCompact,
                    "cancelled" => port::CompactOutcome::Cancelled,
                    _ => port::CompactOutcome::Failed,
                };
                out.events.push(Event::CompactEnded {
                    session: session(),
                    outcome,
                    reason: reason.filter(|r| !r.is_empty()),
                })
            }
            SessionEvent::UserMessageDelivered { message_id } => {
                out.events.push(Event::SteerDelivered {
                    session: session(),
                    message: port::MessageId(message_id),
                })
            }
            SessionEvent::UserMessagesReturned { messages } => {
                out.events.push(Event::SteerReturned {
                    session: session(),
                    messages: messages
                        .into_iter()
                        .map(|m| port::MessageId(m.message_id))
                        .collect(),
                })
            }
            SessionEvent::TurnComplete {
                stop_reason,
                usage,
                message,
            } => {
                let usage = map_usage(&usage);
                s.running = false;
                s.usage.add(&usage);
                s.updated_ms = now;
                let text = s.turn_text.clone();
                let stop = stop_reason_of(&stop_reason, message.as_deref());
                let budget = matches!(stop, StopReason::Budget(_));
                out.events.push(Event::TurnEnded {
                    session: session(),
                    stop,
                    usage,
                });
                self.upsert(sid, out);
                out.todo.push(Todo2::TurnDone {
                    session: sid.to_string(),
                    ok: true,
                    text,
                    usage,
                    budget,
                });
            }
            SessionEvent::Error { message } => {
                self.turn_failed(sid, message, now, out);
            }
            SessionEvent::TaskStarted { task }
            | SessionEvent::TaskUpdated { task }
            | SessionEvent::TaskFinished { task } => {
                let t = task_of(sid, &task);
                if let Some(child) = &t.agent_session {
                    let label = t.label.clone();
                    self.ensure_sidechain(&child.0, sid, &t.id.0, &label);
                    // 侧链会话起时抄的是父会话的模型；子 Agent 可以走别的模型、别的 provider，以任务上报的为准。
                    if let Some(m) = t.model.as_ref().filter(|m| !m.is_empty())
                        && let Some(c) = self.sessions.get_mut(&child.0)
                        && c.model != *m
                    {
                        c.model = m.clone();
                        self.upsert(&child.0, out);
                    }
                }
                self.task_owner.insert(t.id.0.clone(), rpc.to_string());
                if let Some(s) = self.sessions.get_mut(sid) {
                    s.tasks.insert(t.id.0.clone(), t.clone());
                }
                out.events.push(Event::TaskUpserted(t));
            }
            SessionEvent::TaskProgress {
                task_id,
                appended,
                next_byte,
            } => out.events.push(Event::TaskOutput {
                task: TaskId(task_id),
                text: appended,
                next_byte,
            }),
            SessionEvent::GoalChanged { goal } => {
                s.goal = goal.as_ref().map(goal_of);
                out.events.push(Event::GoalChanged {
                    session: session(),
                    goal: s.goal.clone(),
                });
                self.upsert(sid, out);
            }
            SessionEvent::SubscriptionDropped {} => out.todo.push(Todo2::Resubscribe {
                session: sid.to_string(),
            }),
            SessionEvent::PermissionModeChanged { mode, .. } => {
                if let Some(m) = permission_mode(&mode)
                    && s.permission != m
                {
                    s.permission = m;
                    self.upsert(sid, out);
                }
            }
            SessionEvent::Rewound {} | SessionEvent::Unknown => {}
        }
    }

    /// `run_turn` 的错误响应与别的订阅者收到的 `error` 帧走同一处。
    pub fn turn_failed(&mut self, sid: &str, message: String, now: u64, out: &mut Out) {
        let Some(s) = self.sessions.get_mut(sid) else {
            return;
        };
        s.running = false;
        s.updated_ms = now;
        out.events.push(Event::TurnFailed {
            session: SessionId(sid.to_string()),
            message: message.clone(),
        });
        self.upsert(sid, out);
        out.todo.push(Todo2::TurnDone {
            session: sid.to_string(),
            ok: false,
            text: message,
            usage: Usage::default(),
            budget: false,
        });
    }

    pub fn add_prompt(
        &mut self,
        sid: &str,
        rpc: &str,
        p: PromptPayload,
        from: Option<PromptSource>,
        now: u64,
        out: &mut Out,
    ) {
        let (kind, meta) = prompt_of(&p, rpc);
        let id = p.id().to_string();
        let prompt = Prompt {
            id: PromptId(id.clone()),
            kind,
            from,
            asked_ms: now,
            permit_always: p.permit_always(),
            via: p.via(),
        };
        self.prompts.insert(id.clone(), meta);
        let Some(s) = self.sessions.get_mut(sid) else {
            return;
        };
        if s.prompts.iter().any(|x| x.id.0 == id) {
            return;
        }
        s.prompts.push(prompt.clone());
        out.events.push(Event::PromptPending {
            session: SessionId(sid.to_string()),
            prompt,
        });
        self.upsert(sid, out);
    }

    fn resolve_prompt(
        &mut self,
        id: &str,
        resolution: &str,
        reason: Option<String>,
        out: &mut Out,
    ) {
        self.prompts.remove(id);
        let owner = self
            .sessions
            .values_mut()
            .find(|s| s.prompts.iter().any(|p| p.id.0 == id));
        let Some(s) = owner else { return };
        s.prompts.retain(|p| p.id.0 != id);
        let sid = s.id.clone();
        out.events.push(Event::PromptResolved {
            session: SessionId(sid.clone()),
            id: PromptId(id.to_string()),
            resolution: if resolution == "answered" {
                Resolution::Answered
            } else {
                Resolution::Withdrawn {
                    reason: reason.unwrap_or_else(|| resolution.to_string()),
                }
            },
        });
        self.upsert(&sid, out);
    }
}

pub fn short(id: &str) -> String {
    id.chars().take(8).collect()
}

pub fn map_usage(u: &wire::frames::Usage) -> Usage {
    Usage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cache_read_input_tokens,
        cache_write: u.cache_creation_input_tokens,
    }
}

fn stop_reason_of(s: &str, message: Option<&str>) -> StopReason {
    match s {
        "budget_exceeded" => StopReason::Budget(message.unwrap_or_default().to_string()),
        // 0.3 的文档叫 max_turns_reached，0.4 叫 max_turns，两个都认。
        "max_turns" | "max_turns_reached" => StopReason::MaxTurns,
        "end_turn" | "stop_sequence" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "refusal" => StopReason::Refusal,
        "cancelled" | "interrupted" | "aborted" => StopReason::Cancelled,
        other => StopReason::Other(other.to_string()),
    }
}

pub fn goal_of(g: &GoalView) -> Goal {
    Goal {
        objective: g.objective.clone(),
        phase: match g.phase.as_str() {
            "paused" => GoalPhase::Paused,
            "blocked" => GoalPhase::Blocked,
            "complete" => GoalPhase::Complete,
            _ => GoalPhase::Active,
        },
        rounds_started: g.rounds_started,
        max_rounds: g.max_rounds,
        blocked_reason: g.blocked_reason.clone(),
    }
}

pub fn task_of(owner: &str, t: &TaskInfo) -> Task {
    let d = &t.detail;
    let field = |k: &str| d.get(k).and_then(Value::as_str).map(str::to_string);
    let kind = match t.kind.as_str() {
        "agent" => TaskKind::Agent,
        "teammate" | "member" => TaskKind::Teammate,
        "shell" => TaskKind::Shell,
        "monitor" => TaskKind::Monitor,
        "code" => TaskKind::Code,
        "mcp_task" => TaskKind::Mcp,
        "cron" => TaskKind::Cron,
        other => TaskKind::Unknown(other.to_string()),
    };
    let status = match t.status.as_str() {
        "waiting" => TaskStatus::Waiting,
        "done" => TaskStatus::Done,
        "failed" => TaskStatus::Failed,
        "stopped" => TaskStatus::Stopped,
        _ => TaskStatus::Running,
    };
    let label = field("description")
        .or_else(|| field("command"))
        .or_else(|| field("prompt"))
        .or_else(|| field("member"))
        .or_else(|| field("tool"))
        .unwrap_or_else(|| t.kind.clone());
    Task {
        id: TaskId(t.task_id.clone()),
        owner: SessionId(owner.to_string()),
        kind,
        status,
        label,
        agent_type: field("agent_type"),
        agent_session: field("agent_session_id").map(SessionId),
        model: field("model"),
        provider: field("provider"),
        started_ms: t.started_ms.max(0) as u64,
        ended_ms: t.ended_ms.map(|e| e.max(0) as u64),
        usage: t.usage.as_ref().map(map_usage).unwrap_or_default(),
        waiting_for: t.waiting_for.iter().cloned().map(PromptId).collect(),
        error: t.error.clone(),
    }
}

fn prompt_of(p: &PromptPayload, rpc: &str) -> (PromptKind, PromptMeta) {
    let meta = |t: &str, keys: Vec<String>| PromptMeta {
        rpc_session: rpc.to_string(),
        prompt_type: t.to_string(),
        keys,
    };
    match p {
        PromptPayload::Permission {
            tool_name,
            message,
            paths,
            ..
        } => (
            PromptKind::Permission {
                tool: tool_name.clone(),
                summary: message.clone(),
                paths: paths.clone(),
            },
            meta("permission", vec![]),
        ),
        PromptPayload::Question {
            question,
            header,
            options,
            ..
        } => (
            PromptKind::Question {
                question: question.clone(),
                header: header.clone(),
                options: options
                    .iter()
                    .map(|o| {
                        if o.label.is_empty() {
                            o.key.clone()
                        } else {
                            o.label.clone()
                        }
                    })
                    .collect(),
            },
            meta("question", options.iter().map(|o| o.key.clone()).collect()),
        ),
        // MCP server 的询问按"允许 / 拒绝"给人看：accept 只对 url 模式有意义，form 要填的
        // 表单界面还没有，拒绝总是安全的。
        PromptPayload::McpElicitation {
            server, message, ..
        } => (
            PromptKind::Permission {
                tool: server.clone(),
                summary: message.clone(),
                paths: vec![],
            },
            meta("mcp_elicitation", vec![]),
        ),
    }
}

/// 端口的回答 → `session.respondToPrompt` 的参数（协议 §4.7）。
pub fn answer_params(meta: &PromptMeta, prompt: &str, answer: &port::Answer) -> Value {
    use port::Answer as A;
    use serde_json::json;
    let mut p = json!({
        "session_id": meta.rpc_session,
        "prompt_id": prompt,
        "prompt_type": meta.prompt_type,
    });
    match meta.prompt_type.as_str() {
        "question" => {
            p["answer"] = match answer {
                A::Choice { index } => json!({"type": "answer",
                    "text": meta.keys.get(*index).cloned().unwrap_or_else(|| index.to_string())}),
                A::Text { text } => json!({"type": "answer", "text": text}),
                A::Deny { reason } => json!({"type": "decline", "reason": reason}),
                A::Permit | A::PermitSession => json!({"type": "answer", "text": "yes"}),
            }
        }
        "mcp_elicitation" => {
            p["answer"] = match answer {
                A::Permit | A::PermitSession => json!({"type": "accept"}),
                _ => json!({"type": "decline"}),
            }
        }
        _ => {
            p["decision"] = match answer {
                A::Permit => json!({"type": "permit"}),
                A::PermitSession => json!({"type": "permit_always", "scope": "session"}),
                A::Deny { reason } => json!({"type": "deny", "reason": reason}),
                A::Choice { index: 0 } => json!({"type": "permit"}),
                A::Choice { index: 1 } => json!({"type": "permit_always", "scope": "session"}),
                A::Choice { .. } | A::Text { .. } => json!({"type": "deny"}),
            }
        }
    }
    p
}

/// Agix 报的模式名（§4.5，camelCase；`auto` / `yolo` 它已经报成 `bypassPermissions`）→ 端口的。
pub fn permission_mode(name: &str) -> Option<port::PermissionMode> {
    serde_json::from_value(serde_json::Value::String(name.to_string())).ok()
}

/// 端口的模式 → Agix 的名字。
pub fn permission_name(m: port::PermissionMode) -> String {
    serde_json::to_value(m)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "default".into())
}

#[cfg(test)]
mod stop_reason_tests {
    use super::*;

    #[test]
    fn the_two_new_stop_reasons_are_recognised() {
        assert_eq!(
            stop_reason_of("budget_exceeded", Some("this turn's budget")),
            StopReason::Budget("this turn's budget".into())
        );
        assert_eq!(
            stop_reason_of("budget_exceeded", None),
            StopReason::Budget(String::new())
        );
        assert_eq!(stop_reason_of("max_turns", None), StopReason::MaxTurns);
        assert_eq!(
            stop_reason_of("max_turns_reached", None),
            StopReason::MaxTurns
        );
        assert_eq!(
            stop_reason_of("weird", None),
            StopReason::Other("weird".into())
        );
    }
}

#[cfg(test)]
mod frame_tests {
    use super::*;

    #[test]
    fn provider_switched_becomes_a_domain_event() {
        let mut st = St::default();
        st.sessions.insert("s1".into(), Sess::default());
        let mut out = Out::default();
        st.on_event(
            "s1",
            "s1",
            SessionEvent::ProviderSwitched {
                from: None,
                to: "b".into(),
                reason: "a: quota".into(),
                task: "main".into(),
            },
            0,
            &mut out,
        );
        assert!(matches!(
            out.events.as_slice(),
            [Event::ProviderSwitched { to, .. }] if to == "b"
        ));
    }
}
