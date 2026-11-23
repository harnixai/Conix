//! 一轮：引擎的通知 → 领域事件、`run_turn` 的发出与收尾（architecture §8.4 的帧对应表）。

use super::*;

impl Hub {
    /// 派工器往会话里发一句：排在正在跑的那一轮后面。
    pub(crate) fn send_queued(&self, id: &str, text: &str) -> String {
        self.run_turn(id, text, "queue")
    }

    pub(super) async fn follow_up(&self, t: Todo2) {
        match t {
            Todo2::HostCall {
                rpc_session,
                call_id,
                name,
                input,
            } => {
                let (content, is_error) = match self.host_tool(&rpc_session, &name, &input).await {
                    Ok(c) => (c, false),
                    Err(e) => (e, true),
                };
                let _ = self
                    .call(
                        "session.respondToolCall",
                        json!({"session_id": rpc_session, "call_id": call_id,
                               "result": {"content": content, "is_error": is_error}}),
                    )
                    .await;
            }
            Todo2::TurnDone {
                session,
                ok,
                text,
                usage,
                budget,
            } => self.turn_done(&session, ok, text, usage, budget),
            Todo2::Resubscribe { session } => {
                let _ = self.subscribe(&session, false).await;
            }
        }
    }

    pub(super) async fn on_notification(&self, n: Notification) {
        let now = self.now();
        match n {
            Notification::Session {
                session_id,
                turn_id,
                event,
            } => {
                if self.0.taps.locked().contains_key(&session_id) {
                    let text = (event["kind"] == "turn_start")
                        .then(|| self.started(&session_id, turn_id.as_deref()))
                        .flatten();
                    self.tap_send(
                        &session_id,
                        Tapped::Event {
                            turn: turn_id.clone(),
                            event: event.clone(),
                            text,
                        },
                    );
                }
                let ev = SessionEvent::parse(&event);
                self.meter_frame(&session_id, &ev);
                self.apply(|st, out| st.on_event(&session_id, &session_id, ev, now, out));
            }
            Notification::Daemon(v) => match DaemonEvent::parse(&v) {
                DaemonEvent::SessionCreated {
                    session_id,
                    project_root,
                }
                | DaemonEvent::SessionResumed {
                    session_id,
                    project_root,
                } => {
                    let known = self.0.st.locked().sessions.contains_key(&session_id);
                    if known || project_root.as_deref() == Some(self.0.project.as_str()) {
                        if !known {
                            self.adopt(&session_id).await;
                        }
                        let _ = self.subscribe(&session_id, false).await;
                    }
                }
                DaemonEvent::SessionEvicted { session_id }
                | DaemonEvent::SessionClosed { session_id } => {
                    let mut st = self.0.st.locked();
                    if let Some(s) = st.sessions.get_mut(&session_id) {
                        s.live = false;
                        s.running = false;
                    }
                    drop(st);
                    self.upsert(&session_id);
                }
                DaemonEvent::SessionDeleted { session_id } => {
                    self.0.st.locked().sessions.remove(&session_id);
                    self.0.reg.locked().remove(&session_id);
                    self.emit_events(vec![Event::SessionRemoved {
                        session: SessionId(session_id),
                    }]);
                }
                DaemonEvent::TurnStarted { session_id, .. }
                | DaemonEvent::TurnCompleted { session_id, .. } => {
                    let running = matches!(DaemonEvent::parse(&v), DaemonEvent::TurnStarted { .. });
                    let changed = {
                        let mut st = self.0.st.locked();
                        match st.sessions.get_mut(&session_id) {
                            Some(s) if s.running != running => {
                                s.running = running;
                                s.updated_ms = now;
                                true
                            }
                            _ => false,
                        }
                    };
                    if changed {
                        self.upsert(&session_id);
                    }
                }
                DaemonEvent::EventsLagged {} | DaemonEvent::Unknown => {}
            },
        }
    }

    /// `run_turn` 一跑就是几分钟：另起任务等它，错误响应就是这一轮的失败（协议 §5.5）。
    pub(super) fn run_turn(&self, id: &str, text: &str, on_busy: &str) -> String {
        let turn = format!("cx-{}-{}", self.now(), rand_suffix());
        if on_busy != "steer" {
            self.0
                .sent
                .locked()
                .entry(id.to_string())
                .or_default()
                .push_back((turn.clone(), text.to_string()));
        }
        let hub = self.clone();
        let (id, text, on_busy, t) = (
            id.to_string(),
            text.to_string(),
            on_busy.to_string(),
            turn.clone(),
        );
        tokio::spawn(async move {
            let r = hub
                .call(
                    "session.run_turn",
                    json!({"session_id": id, "message": text, "turn_id": t, "on_busy": on_busy}),
                )
                .await;
            match r {
                Ok(v) => {
                    if let Some(name) = v["name"].as_str() {
                        hub.retitle(&id, name, Titler::Engine);
                    }
                }
                // 会话正忙、消息没有投递（协议 §5.3）：不是这一轮失败，是这句话被退回了。
                Err(RpcError::Remote {
                    code: SESSION_BUSY, ..
                }) => {
                    hub.emit_events(vec![Event::SteerReturned {
                        session: SessionId(id.clone()),
                        messages: vec![port::MessageId(t.clone())],
                    }]);
                }
                Err(RpcError::Remote { message, data, .. }) => {
                    let text = hub.forget_sent(&id, &t);
                    hub.tap_send(
                        &id,
                        Tapped::Failed {
                            turn: t.clone(),
                            text,
                            kind: data["kind"].as_str().unwrap_or("error").to_string(),
                            retryable: data["retryable"].as_bool().unwrap_or(true),
                            message: message.clone(),
                        },
                    );
                    let now = hub.now();
                    hub.apply(|st, out| st.turn_failed(&id, message, now, out));
                }
                Err(RpcError::Closed) => {}
            }
        });
        turn
    }

    /// 一轮开始了：它是经 `run_turn` 发的哪一句。认得轮 id 就按 id 取；不认得（引擎自己
    /// 起的，或 id 没对上）就取最早发出的那一句。
    pub(super) fn started(&self, session: &str, turn: Option<&str>) -> Option<String> {
        let mut sent = self.0.sent.locked();
        let q = sent.get_mut(session)?;
        let i = turn
            .and_then(|t| q.iter().position(|(id, _)| id == t))
            .unwrap_or(0);
        let (_, text) = q.remove(i)?;
        if q.is_empty() {
            sent.remove(session);
        }
        Some(text)
    }

    pub(super) fn forget_sent(&self, session: &str, turn: &str) -> Option<String> {
        let mut sent = self.0.sent.locked();
        let q = sent.get_mut(session)?;
        let i = q.iter().position(|(id, _)| id == turn)?;
        q.remove(i).map(|(_, text)| text)
    }

    pub(super) fn turn_done(
        &self,
        session: &str,
        ok: bool,
        text: String,
        usage: Usage,
        budget: bool,
    ) {
        if self.0.side.locked().remove(session) {
            let first: String = text.chars().take(400).collect();
            self.emit_events(vec![Event::Notice {
                level: if ok {
                    port::NoticeLevel::Info
                } else {
                    port::NoticeLevel::Error
                },
                text: first,
            }]);
        }
        let run = self.0.job_runs.locked().remove(session);
        let audit = self.meter_take(session);
        if let (Some((job, fired, started)), Ok(sched)) = (run, self.scheduler()) {
            // 引擎按会话预算（作业的 max_tokens）停下的，和 Codix 自己数到超额停下的是一回事。
            let stopped = self
                .0
                .stop_causes
                .locked()
                .remove(session)
                .or(budget.then_some(port::StopCause::Budget));
            let outcome = match (stopped, ok) {
                (Some(reason), _) => JobOutcome::Stopped { reason },
                (None, true) => JobOutcome::Done,
                (None, false) => JobOutcome::Failed { message: text },
            };
            let elapsed = self.now().saturating_sub(started);
            let store = sched.store();
            let entry = {
                let s = store.locked();
                let _ = s.finish(&job, fired, outcome, elapsed, usage, audit);
                s.history(&job, 50)
                    .into_iter()
                    .find(|e| e.fired_ms == fired)
            };
            let mut events = vec![];
            if let Some(entry) = entry {
                events.push(Event::JobFired {
                    job: JobId(job.clone()),
                    entry,
                });
            }
            if let Some(j) = sched.get(&job) {
                events.push(Event::JobUpserted(j));
            }
            self.emit_events(events);
            self.after_finish(&job);
        }
    }
}
