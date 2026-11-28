use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use port::{
    Backend, Command, Event, GoalAction, GoalPhase, JobEntry, JobOutcome, JobTarget, Millis,
    PortError, Reply, Resolution, RunOp, RunStatus, RunSummary, RunView, SessionId, SessionState,
    SessionSummary, StationStatus, StopReason, SubmitMode, TaskStatus, Usage,
};
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::scenario::{RuleReply, Scenario, Step, StepAction};
use crate::state::MockState;

/// 测试断言用：后端收到过的每一条命令，按到达顺序。
pub type CallLog = Arc<Mutex<Vec<Command>>>;

#[derive(Debug, Clone, Copy)]
pub struct MockOptions {
    /// 时间线加速倍数（`conix --mock … --speed 4`）。
    pub speed: f64,
    /// 场景里写的毫秒数以这个时刻为"墙上时间"的零点，用于填 `*_ms` 字段。
    pub epoch_ms: Millis,
}

impl Default for MockOptions {
    fn default() -> Self {
        MockOptions {
            speed: 1.0,
            epoch_ms: 1_763_280_000_000,
        }
    }
}

enum Sched {
    At {
        due: Instant,
        tag: Option<SessionId>,
        event: Box<Event>,
    },
    /// 取消某会话还没发生的事件：中断一轮时用。
    Cancel(SessionId),
}

pub struct ScriptedBackend {
    state: Arc<Mutex<MockState>>,
    scenario: Scenario,
    used_rules: Mutex<HashSet<usize>>,
    sched: mpsc::UnboundedSender<Sched>,
    events: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
    calls: CallLog,
    seq: AtomicU64,
    current_message: Mutex<Option<port::MessageId>>,
    /// 流程 / 作业各自的编辑会话：同一个对象总是同一个会话。
    opts: MockOptions,
    started: Instant,
}

impl ScriptedBackend {
    /// 必须在 tokio 运行时里调用：时间线从这一刻开始走。
    pub fn new(state: MockState, scenario: Scenario, opts: MockOptions) -> Arc<Self> {
        let state = Arc::new(Mutex::new(state));
        let (ev_tx, ev_rx) = mpsc::unbounded_channel();
        let (sched_tx, sched_rx) = mpsc::unbounded_channel();
        tokio::spawn(scheduler(sched_rx, state.clone(), ev_tx));
        let me = Arc::new(ScriptedBackend {
            state,
            scenario,
            used_rules: Mutex::new(HashSet::new()),
            sched: sched_tx,
            events: Mutex::new(Some(ev_rx)),
            calls: CallLog::default(),
            seq: AtomicU64::new(1),
            current_message: Mutex::new(None),
            opts,
            started: Instant::now(),
        });
        let timeline: Vec<_> = me
            .scenario
            .timeline
            .iter()
            .map(|t| (t.at, t.action.clone()))
            .collect();
        for (at, action) in timeline {
            me.schedule_action(me.started + me.scaled(at), &action, 0);
        }
        me
    }

    pub fn calls(&self) -> CallLog {
        self.calls.clone()
    }

    pub fn state(&self) -> MockState {
        self.state.lock().unwrap().clone()
    }

    fn scaled(&self, ms: Millis) -> Duration {
        Duration::from_secs_f64(ms as f64 / 1000.0 / self.opts.speed.max(0.01))
    }

    fn now_ms(&self) -> Millis {
        self.opts.epoch_ms + self.started.elapsed().as_millis() as Millis
    }

    fn next_id(&self, prefix: &str) -> String {
        format!("{prefix}{}", self.seq.fetch_add(1, Ordering::Relaxed))
    }

    fn emit_now(&self, event: Event) {
        // 新建类的事件先在状态里生效：紧跟着的 Watch / GetRun 要拿得到它（重复生效无害）。
        if matches!(
            event,
            Event::SessionUpserted(_) | Event::RunDetail(_) | Event::JobUpserted(_)
        ) {
            self.state.lock().unwrap().apply(&event);
        }
        self.emit_at(Instant::now(), event);
    }

    fn emit_at(&self, due: Instant, event: Event) {
        let tag = event.session().cloned();
        let _ = self.sched.send(Sched::At {
            due,
            tag,
            event: Box::new(event),
        });
    }

    fn cancel(&self, session: &SessionId) {
        let _ = self.sched.send(Sched::Cancel(session.clone()));
    }

    /// 排一串步骤，返回最后一步的时刻：`after` 相对上一步，`play` 的下一步接在片段结束之后。
    fn schedule_steps(
        &self,
        start: Instant,
        steps: &[Step],
        session: Option<&SessionId>,
    ) -> Instant {
        let mut t = start;
        for step in steps {
            t += self.scaled(step.after);
            t = self.schedule_one(t, &step.action, session, 0);
        }
        t
    }

    fn schedule_action(&self, at: Instant, action: &StepAction, depth: u8) {
        self.schedule_one(at, action, None, depth);
    }

    fn schedule_one(
        &self,
        at: Instant,
        action: &StepAction,
        session: Option<&SessionId>,
        depth: u8,
    ) -> Instant {
        match action {
            StepAction::Event(ev) => {
                let message = self.current_message.lock().unwrap().clone();
                self.emit_at(at, substitute(ev, session, message.as_ref()));
                at
            }
            StepAction::Play(name) if depth < 8 => match self.scenario.scripts.get(name) {
                Some(steps) => {
                    let steps = steps.clone();
                    let mut t = at;
                    for step in &steps {
                        t += self.scaled(step.after);
                        t = self.schedule_one(t, &step.action, session, depth + 1);
                    }
                    t
                }
                None => {
                    self.emit_at(
                        at,
                        Event::Notice {
                            level: port::NoticeLevel::Error,
                            text: format!("mock：场景里没有片段 {name}"),
                        },
                    );
                    at
                }
            },
            StepAction::Play(_) => at,
        }
    }

    fn react(&self, cmd: &Command) -> Result<Reply, PortError> {
        let session = command_session(cmd);
        // 用户的话本来就在会话历史里（真实后端的快照会带上它），mock 也记一笔，
        // 这样提交之后再 Watch 拿到的快照与界面一致。
        if let Command::Submit { session, text, .. } = cmd
            && let Some(sess) = self.state.lock().unwrap().sessions.get_mut(session)
        {
            sess.history
                .push(port::HistoryItem::User { text: text.clone() });
        }
        // 提交的消息 id 先定下来：规则的片段里可以写 `$message` 引用它（插话被退回）。
        let message =
            matches!(cmd, Command::Submit { .. }).then(|| port::MessageId(self.next_id("m")));
        *self.current_message.lock().unwrap() = message.clone();
        let rule = {
            let mut used = self.used_rules.lock().unwrap();
            let hit = self
                .scenario
                .rules
                .iter()
                .enumerate()
                .find(|(i, r)| r.on.matches(cmd) && !(r.once && used.contains(i)));
            if let Some((i, _)) = hit {
                used.insert(i);
            }
            hit.map(|(_, r)| r.clone())
        };
        match rule {
            Some(rule) => {
                self.schedule_steps(Instant::now(), &rule.then, session.as_ref());
                match rule.reply {
                    Some(RuleReply::Ok) => Ok(Reply::Ok),
                    Some(RuleReply::Error(e)) => Err(PortError::Failed(e)),
                    None => self.default_reply(cmd, false),
                }
            }
            None => self.default_reply(cmd, true),
        }
    }

    /// 缺省反应。`effects` 为假时只给结果、不推事件（规则已经接管了后续）。
    fn default_reply(&self, cmd: &Command, effects: bool) -> Result<Reply, PortError> {
        use Command::*;
        let st = || self.state.lock().unwrap();
        match cmd {
            Hello { .. } => Ok(Reply::Hello {
                instance: st().instance.clone(),
                flows: true,
                jobs: true,
            }),
            Heartbeat { .. } | Detach | Unwatch { .. } | Rename { .. } => {
                if let (Rename { session, title }, true) = (cmd, effects)
                    && let Some(mut s) = self.summary(session)
                {
                    s.title = title.clone();
                    self.emit_now(Event::SessionUpserted(s));
                }
                Ok(Reply::Ok)
            }
            StopInstance => {
                if effects {
                    self.emit_now(Event::InstanceStopped {
                        reason: "已由你停止".into(),
                    });
                }
                Ok(Reply::Ok)
            }
            ListSessions { .. } => Ok(Reply::Sessions(st().summaries())),
            CreateSession {
                project,
                scene,
                model,
                permission,
            } => {
                let mut s = self.new_session(
                    "新会话",
                    project.clone(),
                    scene.clone(),
                    model.clone(),
                    port::Origin::User,
                );
                s.permission = permission.unwrap_or_default();
                self.emit_now(Event::SessionUpserted(s.clone()));
                Ok(Reply::Session(s))
            }
            Watch { session } => st()
                .watched(session)
                .map(|w| Reply::Watched(Box::new(w)))
                .ok_or_else(|| PortError::Failed(format!("没有会话 {session}"))),
            Close { session } => {
                if effects {
                    let mut state = st();
                    if let Some(mut s) = state.sessions.remove(session) {
                        s.closed = true;
                        state.closed.insert(0, s);
                    }
                    drop(state);
                    self.emit_now(Event::SessionRemoved {
                        session: session.clone(),
                    });
                }
                Ok(Reply::Ok)
            }
            Submit {
                session,
                text,
                mode,
                ..
            } => {
                let message = self
                    .current_message
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| port::MessageId(self.next_id("m")));
                if effects {
                    let running = st().sessions.get(session).is_some_and(|s| s.running);
                    if running && *mode == SubmitMode::Steer {
                        self.emit_at(
                            Instant::now() + self.scaled(300),
                            Event::SteerDelivered {
                                session: session.clone(),
                                message: message.clone(),
                            },
                        );
                    } else {
                        self.default_answer(session, text);
                    }
                }
                Ok(Reply::Submitted { message })
            }
            Interrupt { session } => {
                if effects {
                    self.cancel(session);
                    self.emit_now(Event::TurnEnded {
                        session: session.clone(),
                        stop: StopReason::Cancelled,
                        usage: Usage::default(),
                    });
                }
                Ok(Reply::Ok)
            }
            Answer {
                session, prompt, ..
            } => {
                if effects {
                    self.emit_now(Event::PromptResolved {
                        session: session.clone(),
                        id: prompt.clone(),
                        resolution: Resolution::Answered,
                    });
                }
                Ok(Reply::Ok)
            }
            TrustProject { .. } => Ok(Reply::Ok),
            SetPermissionMode { session, mode } => {
                let mut s = self
                    .summary(session)
                    .ok_or_else(|| PortError::Failed(format!("没有会话 {session}")))?;
                if effects {
                    s.permission = *mode;
                    self.emit_now(Event::SessionUpserted(s));
                }
                Ok(Reply::PermissionMode {
                    mode: *mode,
                    clamped: false,
                })
            }
            Checkpoints { .. } => Ok(Reply::Checkpoints(vec![
                port::Checkpoint {
                    turn_id: "t1".into(),
                    prompt: "看看认证模块现在怎么组织的".into(),
                    files: vec![],
                    restorable: true,
                },
                port::Checkpoint {
                    turn_id: "t2".into(),
                    prompt: "把 session 存储挪到 Redis".into(),
                    files: vec!["src/auth/session.rs".into()],
                    restorable: true,
                },
            ])),
            Rewind { .. } => Ok(Reply::Rewound {
                removed_turns: 1,
                restored_files: 1,
            }),
            ProjectFiles => Ok(Reply::Files(
                [
                    "src/auth/mod.rs",
                    "src/auth/oauth.rs",
                    "src/main.rs",
                    "README.md",
                ]
                .iter()
                .map(|f| f.to_string())
                .collect(),
            )),
            ListModels => Ok(Reply::Models(
                ["claude-sonnet-5-5", "claude-opus-5-5", "claude-haiku-5-5"]
                    .iter()
                    .map(|m| port::ModelInfo {
                        provider: "anthropic".into(),
                        id: m.to_string(),
                        display_name: None,
                    })
                    .collect(),
            )),
            SetModel { session, model, .. } => {
                if effects && let Some(mut s) = self.summary(session) {
                    s.provenance.model = model.clone();
                    self.emit_now(Event::SessionUpserted(s));
                }
                Ok(Reply::Ok)
            }
            Commands { .. } => Ok(Reply::Commands(st().commands.clone())),
            SideQuestion { text, .. } => {
                if effects {
                    self.emit_now(Event::Notice {
                        level: port::NoticeLevel::Info,
                        text: format!("（mock）侧问：{text}"),
                    });
                }
                Ok(Reply::Ok)
            }
            SetGoal {
                session,
                objective,
                max_rounds,
            } => {
                if effects {
                    self.emit_now(Event::GoalChanged {
                        session: session.clone(),
                        goal: Some(port::Goal {
                            objective: objective.clone(),
                            phase: GoalPhase::Active,
                            rounds_started: 0,
                            max_rounds: max_rounds.unwrap_or(256),
                            blocked_reason: None,
                        }),
                    });
                }
                Ok(Reply::Ok)
            }
            UpdateGoal { session, action } => {
                if effects {
                    let goal = st().sessions.get(session).and_then(|s| s.goal.clone());
                    let next = match (goal, action) {
                        (_, GoalAction::Clear) | (None, _) => None,
                        (Some(mut g), GoalAction::Pause) => {
                            g.phase = GoalPhase::Paused;
                            Some(g)
                        }
                        (Some(mut g), GoalAction::Resume) => {
                            g.phase = GoalPhase::Active;
                            g.blocked_reason = None;
                            Some(g)
                        }
                        (
                            Some(mut g),
                            GoalAction::Edit {
                                objective,
                                max_rounds,
                            },
                        ) => {
                            g.objective = objective.clone();
                            if let Some(m) = max_rounds {
                                g.max_rounds = *m;
                            }
                            Some(g)
                        }
                    };
                    self.emit_now(Event::GoalChanged {
                        session: session.clone(),
                        goal: next,
                    });
                }
                Ok(Reply::Ok)
            }
            StopTask { task } => {
                let found = st().tasks.get(task).cloned();
                if effects && let Some(mut t) = found {
                    t.status = TaskStatus::Stopped;
                    t.ended_ms = Some(self.now_ms());
                    self.emit_now(Event::TaskUpserted(t));
                }
                Ok(Reply::Ok)
            }
            MessageTask { .. } => Ok(Reply::Ok),
            SessionTasks { session } => Ok(Reply::Tasks(st().tasks_under(session))),
            TaskOutput { task, from_byte } => {
                let st = st();
                let all = st.task_output.get(task).cloned().unwrap_or_default();
                let from = (*from_byte as usize).min(all.len());
                let from = (0..=from)
                    .rev()
                    .find(|i| all.is_char_boundary(*i))
                    .unwrap_or(0);
                let finished = st.tasks.get(task).is_some_and(|t| t.status.finished());
                Ok(Reply::TaskOutput {
                    text: all[from..].to_string(),
                    next_byte: all.len() as u64,
                    finished,
                })
            }
            ListFlows => Ok(Reply::Flows(st().flow_summaries())),
            GetFlow { name } => st()
                .flows
                .iter()
                .find(|f| &f.summary.name == name)
                .map(|f| Reply::Flow(Box::new(f.detail())))
                .ok_or_else(|| PortError::Failed(format!("没有流程 {name}"))),
            ReloadFlows | ReloadJobs => Ok(Reply::Ok),
            ListLibrary => Ok(Reply::Library(st().library.clone())),
            ListSamples => Ok(Reply::Samples(st().samples.clone())),
            ListRuns { flow } => {
                let mut runs: Vec<_> = st()
                    .runs
                    .values()
                    .filter(|r| &r.summary.flow == flow)
                    .map(|r| r.summary.clone())
                    .collect();
                runs.sort_by_key(|r| std::cmp::Reverse(r.started_ms));
                Ok(Reply::Runs(runs))
            }
            ListEndedRuns { limit, before_ms } => {
                let mut runs: Vec<_> = st()
                    .runs
                    .values()
                    .map(|r| r.summary.clone())
                    .filter(|r| matches!(r.status, RunStatus::Finished | RunStatus::Cancelled))
                    .filter(|r| before_ms.is_none_or(|b| r.ended_ms.unwrap_or(0) < b))
                    .collect();
                runs.sort_by_key(|r| std::cmp::Reverse((r.ended_ms.unwrap_or(0), r.id.0.clone())));
                runs.truncate(*limit as usize);
                Ok(Reply::Runs(runs))
            }
            GetRun { run } => st()
                .runs
                .get(run)
                .map(|r| Reply::Run(Box::new(r.clone())))
                .ok_or_else(|| PortError::Failed(format!("没有运行 {run}"))),
            StartRun {
                flow,
                subject,
                inputs,
                step,
                session,
            } => {
                let (bundle, flows) = {
                    let state = st();
                    (
                        state.bundles.iter().find(|b| &b.name == flow).cloned(),
                        state
                            .flows
                            .iter()
                            .map(|f| f.summary.name.clone())
                            .collect::<Vec<_>>(),
                    )
                };
                let full = match crate::flowops::pick(bundle.as_ref(), flow, inputs, &flows)
                    .map_err(PortError::Failed)?
                {
                    crate::flowops::Pick::Flow(full) => full,
                    crate::flowops::Pick::Triage(b) => {
                        // 分诊：往主会话发一轮，由模型判断后自己再起运行。
                        let sid = match session {
                            Some(s) => s.clone(),
                            None => {
                                let s = self.new_session(
                                    &format!("{b} triage"),
                                    None,
                                    None,
                                    None,
                                    port::Origin::User,
                                );
                                self.emit_now(Event::SessionUpserted(s.clone()));
                                s.id
                            }
                        };
                        self.answer_with(&sid, crate::flowops::triage_reply(&b, subject));
                        return Ok(Reply::Triage { session: sid });
                    }
                };
                let view = self.start_run(&full, subject, *step)?;
                let summary = view.summary.clone();
                self.emit_now(Event::RunDetail(Box::new(view)));
                Ok(Reply::RunStarted(summary))
            }
            RunOp { run, op } => {
                if !effects {
                    return Ok(Reply::Ok);
                }
                let mut view = st()
                    .runs
                    .get(run)
                    .cloned()
                    .ok_or_else(|| PortError::Failed(format!("没有运行 {run}")))?;
                let text = match op {
                    port::RunOp::Goto { station, .. } => {
                        Some(crate::flowops::goto(&mut view, station).map_err(PortError::Failed)?)
                    }
                    port::RunOp::Switch { to, .. } => {
                        let full = crate::flowops::full_name(&view, to);
                        let stations = st()
                            .flows
                            .iter()
                            .find(|f| f.summary.name == full)
                            .map(|f| f.stations.clone())
                            .ok_or_else(|| PortError::Failed(format!("没有流程 {full}")))?;
                        Some(
                            crate::flowops::switch(&mut view, &full, stations)
                                .map_err(PortError::Failed)?,
                        )
                    }
                    port::RunOp::EndWorktree { how } => Some(
                        crate::flowops::end_worktree(&mut view, *how).map_err(PortError::Failed)?,
                    ),
                    _ => {
                        apply_run_op(&mut view, op);
                        None
                    }
                };
                self.emit_now(Event::RunUpserted(view.summary.clone()));
                self.emit_now(Event::RunDetail(Box::new(view)));
                Ok(text.map_or(Reply::Ok, |text| Reply::Text { text }))
            }
            ListBundles => Ok(Reply::Bundles(st().bundles.clone())),
            FlowCommand {
                bundle,
                command,
                args,
                run,
            } => {
                let info = st().bundles.iter().find(|b| &b.name == bundle).cloned();
                crate::flowops::command_text(info.as_ref(), command, args, run.is_some())
                    .map(|text| Reply::Text { text })
                    .map_err(PortError::Failed)
            }
            ListJobs => Ok(Reply::Jobs(
                st().jobs.values().map(|(j, _)| j.clone()).collect(),
            )),
            JobHistory {
                job,
                before_ms,
                limit,
                outcome,
            } => Ok(Reply::JobHistory(
                st().jobs
                    .get(job)
                    .map(|(_, h)| {
                        h.iter()
                            .filter(|e| before_ms.is_none_or(|b| e.fired_ms < b))
                            .filter(|e| outcome.is_none_or(|k| e.outcome.kind() == k))
                            .take(limit.unwrap_or(u32::MAX) as usize)
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default(),
            )),
            TestNotify => Ok(Reply::Notified(vec![])),
            Doctor => Ok(Reply::Doctor(vec![port::HealthCheck {
                name: "mock".into(),
                status: port::HealthStatus::Ok,
                summary: "演示后端".into(),
            }])),
            ListClosedSessions => Ok(Reply::Sessions(
                st().closed.iter().map(|s| s.summary.clone()).collect(),
            )),
            ReopenSession { session } => {
                let mut state = st();
                let Some(at) = state.closed.iter().position(|s| &s.summary.id == session) else {
                    return Err(PortError::Failed(format!("{session} 不是已关闭的会话")));
                };
                let mut s = state.closed.remove(at);
                s.closed = false;
                let summary = s.summary.clone();
                if effects {
                    state.sessions.insert(summary.id.clone(), s);
                }
                drop(state);
                if effects {
                    self.emit_now(Event::SessionUpserted(summary));
                }
                Ok(Reply::Ok)
            }
            DeleteSession { session } => {
                st().closed.retain(|s| &s.summary.id != session);
                if effects {
                    self.emit_now(Event::SessionRemoved {
                        session: session.clone(),
                    });
                }
                Ok(Reply::Ok)
            }
            ListPlugins => Ok(Reply::Plugins(crate::plugins::list(&st()))),
            InstallPlugin { source } => {
                let (plugin, disclosure) = crate::plugins::install(&mut st(), source)?;
                if effects {
                    self.emit_now(Event::PluginsChanged);
                }
                Ok(Reply::Plugin { plugin, disclosure })
            }
            SetPluginEnabled { name, enabled } => {
                let events = crate::plugins::set_enabled(&mut st(), name, *enabled)?;
                if effects {
                    for e in events {
                        self.emit_now(e);
                    }
                }
                Ok(Reply::Ok)
            }
            UninstallPlugin { name } => {
                let events = crate::plugins::uninstall(&mut st(), name)?;
                if effects {
                    for e in events {
                        self.emit_now(e);
                    }
                }
                Ok(Reply::Ok)
            }
            PluginDisclosure { name } => {
                let (plugin, disclosure) = crate::plugins::disclosure(&st(), name)?;
                Ok(Reply::Plugin { plugin, disclosure })
            }
            ValidateJob { job } => Ok(Reply::JobCheck(port::JobCheck {
                next_fire_ms: job.next_fire_ms.into_iter().collect(),
                guard: job.guard.clone(),
                capped: vec![],
                problems: vec![],
                notes: vec![],
            })),
            StopFiring { job, fired_ms } => {
                let mut state = st();
                let Some((_, history)) = state.jobs.get_mut(job) else {
                    return Err(PortError::Failed(format!("没有作业 {job}")));
                };
                let Some(e) = history.iter_mut().find(|e| {
                    e.outcome == port::JobOutcome::Running
                        && fired_ms.is_none_or(|f| e.fired_ms == f)
                }) else {
                    return Err(PortError::Failed("没有正在跑的触发".into()));
                };
                e.outcome = port::JobOutcome::Stopped {
                    reason: port::StopCause::Manual,
                };
                let entry = e.clone();
                drop(state);
                if effects {
                    self.emit_now(Event::JobFired {
                        job: job.clone(),
                        entry,
                    });
                }
                Ok(Reply::Ok)
            }
            SaveJob { job } => {
                self.emit_now(Event::JobUpserted(job.clone()));
                Ok(Reply::Job(job.clone()))
            }
            DeleteJob { job } => {
                if effects {
                    self.emit_now(Event::JobRemoved { job: job.clone() });
                }
                Ok(Reply::Ok)
            }
            SetJobEnabled { job, enabled } => {
                let found = st().jobs.get(job).cloned();
                if effects && let Some((mut j, _)) = found {
                    j.enabled = *enabled;
                    self.emit_now(Event::JobUpserted(j));
                }
                Ok(Reply::Ok)
            }
            RunJobNow { job, .. } => {
                let found = st().jobs.get(job).cloned();
                if effects && let Some((j, _)) = found {
                    let target = match &j.kind {
                        port::JobKind::Simple { .. } => {
                            let s = self.new_session(
                                &format!("{} {}", j.name, "（立即运行）"),
                                None,
                                None,
                                None,
                                port::Origin::Job {
                                    job: j.id.clone(),
                                    name: j.name.clone(),
                                    fired_ms: self.now_ms(),
                                },
                            );
                            self.emit_now(Event::SessionUpserted(s.clone()));
                            JobTarget::Session {
                                id: s.id,
                                title: s.title,
                            }
                        }
                        port::JobKind::Flow { flow, subject, .. } => {
                            let view = self.start_run(flow, subject, false)?;
                            let t = JobTarget::Run {
                                id: view.summary.id.clone(),
                                subject: view.summary.subject.clone(),
                            };
                            self.emit_now(Event::RunDetail(Box::new(view)));
                            t
                        }
                    };
                    self.emit_at(
                        Instant::now() + self.scaled(800),
                        Event::JobFired {
                            job: job.clone(),
                            entry: JobEntry {
                                fired_ms: self.now_ms(),
                                outcome: JobOutcome::Done,
                                elapsed_ms: 800,
                                usage: Usage {
                                    input: 3_000,
                                    output: 400,
                                    ..Usage::default()
                                },
                                target: Some(target),
                                audit: None,
                            },
                        },
                    );
                }
                Ok(Reply::Ok)
            }
        }
    }

    fn summary(&self, id: &SessionId) -> Option<SessionSummary> {
        self.state
            .lock()
            .unwrap()
            .sessions
            .get(id)
            .map(|s| s.summary.clone())
    }

    fn new_session(
        &self,
        title: &str,
        project: Option<String>,
        scene: Option<String>,
        model: Option<String>,
        origin: port::Origin,
    ) -> SessionSummary {
        let st = self.state.lock().unwrap();
        SessionSummary {
            id: SessionId(self.next_id("s-new-")),
            title: title.to_string(),
            state: SessionState::Idle,
            waiting: 0,
            usage: Usage::default(),
            provenance: port::Provenance {
                instance: st.instance.name.clone(),
                host: st.instance.host.clone(),
                project: project.or_else(|| st.instance.project.clone()),
                scene: scene.unwrap_or_else(|| "code".into()),
                model: model.unwrap_or_else(|| "claude-sonnet-5-5".into()),
                origin,
            },
            updated_ms: self.now_ms(),
            created_ms: self.now_ms(),
            held_by: None,
            permission: Default::default(),
        }
    }

    fn start_run(&self, flow: &str, subject: &str, step: bool) -> Result<RunView, PortError> {
        let (stations, bundle) = {
            let st = self.state.lock().unwrap();
            st.flows
                .iter()
                .find(|f| f.summary.name == flow)
                .map(|f| (f.stations.clone(), f.summary.bundle.clone()))
                .ok_or_else(|| PortError::Failed(format!("没有流程 {flow}")))?
        };
        let main = self.new_session(subject, None, None, None, port::Origin::User);
        let id = port::RunId(self.next_id("r-new-"));
        let mut main = main;
        main.title = format!("{subject}（主会话）");
        self.emit_now(Event::SessionUpserted(main.clone()));
        let mut view = RunView {
            summary: RunSummary {
                id,
                flow: flow.to_string(),
                subject: subject.to_string(),
                status: RunStatus::Running,
                current: None,
                stations_done: 0,
                stations_total: stations.len() as u32,
                started_ms: self.now_ms(),
                ended_ms: None,
                usage: Usage::default(),
                bundle,
                pace: if step {
                    port::Pace::Step
                } else {
                    port::Pace::Auto
                },
                parent: None,
            },
            main_session: main.id,
            stations: stations
                .into_iter()
                .map(|mut s| {
                    s.status = StationStatus::Pending;
                    s.tasks.clear();
                    s.elapsed_ms = 0;
                    s.usage = Usage::default();
                    s
                })
                .collect(),
            def_digest: String::new(),
            def_state: port::DefState::Current,
            proposals: vec![],
            worktree: None,
        };
        advance(&mut view);
        Ok(view)
    }

    /// 没有规则接管时对一句提交的回答：`default-answer` 片段（`$session` 会被换掉），
    /// 没有就回显一句。
    fn default_answer(&self, session: &SessionId, text: &str) {
        if self.scenario.scripts.contains_key("default-answer") {
            self.schedule_one(
                Instant::now(),
                &StepAction::Play("default-answer".into()),
                Some(session),
                0,
            );
            return;
        }
        self.answer_with(session, format!("（mock）收到：{text}"));
    }

    /// 一轮回答：开始、分块的文字、结束。
    fn answer_with(&self, session: &SessionId, reply: String) {
        let chars: Vec<char> = reply.chars().collect();
        let mut steps = vec![Step {
            after: 0,
            action: StepAction::Event(Box::new(Event::TurnStarted {
                session: session.clone(),
            })),
        }];
        for chunk in chars.chunks(6) {
            steps.push(Step {
                after: 40,
                action: StepAction::Event(Box::new(Event::TextDelta {
                    session: session.clone(),
                    text: chunk.iter().collect(),
                })),
            });
        }
        steps.push(Step {
            after: 40,
            action: StepAction::Event(Box::new(Event::TurnEnded {
                session: session.clone(),
                stop: StopReason::EndTurn,
                usage: Usage {
                    input: 1_200,
                    output: chars.len() as u64,
                    ..Usage::default()
                },
            })),
        });
        self.schedule_steps(Instant::now(), &steps, Some(session));
    }
}

#[async_trait]
impl Backend for ScriptedBackend {
    async fn call(&self, cmd: Command) -> Result<Reply, PortError> {
        self.calls.lock().unwrap().push(cmd.clone());
        self.react(&cmd)
    }

    fn events(&self) -> mpsc::UnboundedReceiver<Event> {
        self.events
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1)
    }
}

/// 片段里写 `$session` / `$message` 的地方换成实际的会话与消息 id：同一个片段可以对任意
/// 会话回放。
fn substitute(ev: &Event, session: Option<&SessionId>, message: Option<&port::MessageId>) -> Event {
    let Ok(mut text) = serde_json::to_string(ev) else {
        return ev.clone();
    };
    let mut changed = false;
    for (key, val) in [
        ("\"$session\"", session.map(|s| s.as_str())),
        ("\"$message\"", message.map(|m| m.as_str())),
    ] {
        if let Some(v) = val
            && text.contains(key)
        {
            text = text.replace(key, &serde_json::to_string(v).unwrap_or_default());
            changed = true;
        }
    }
    if !changed {
        return ev.clone();
    }
    serde_json::from_str(&text).unwrap_or_else(|_| ev.clone())
}

fn command_session(cmd: &Command) -> Option<SessionId> {
    use Command::*;
    match cmd {
        Watch { session }
        | Unwatch { session }
        | Rename { session, .. }
        | Close { session }
        | Submit { session, .. }
        | Interrupt { session }
        | Answer { session, .. }
        | SetModel { session, .. }
        | SetPermissionMode { session, .. }
        | Commands { session }
        | SideQuestion { session, .. }
        | SetGoal { session, .. }
        | UpdateGoal { session, .. } => Some(session.clone()),
        _ => None,
    }
}

/// 进站：所有前置都已通过（或跳过）的待进入站开始进行；没有可做的且都结束了则运行完成。
pub(crate) fn advance(view: &mut RunView) {
    let done =
        |s: &port::StationView| matches!(s.status, StationStatus::Passed | StationStatus::Skipped);
    let finished: HashSet<_> = view
        .stations
        .iter()
        .filter(|s| done(s))
        .map(|s| s.id.clone())
        .collect();
    let step = view.summary.pace == port::Pace::Step;
    let mut prev: Option<port::StationId> = None;
    let ids: Vec<_> = view.stations.iter().map(|s| s.id.clone()).collect();
    for (i, s) in view.stations.iter_mut().enumerate() {
        let needs: Vec<_> = if s.needs.is_empty() {
            prev.iter().cloned().collect()
        } else {
            s.needs.clone()
        };
        if matches!(s.status, StationStatus::Pending | StationStatus::Stale)
            && needs.iter().all(|n| finished.contains(n))
        {
            // 步进：进站都搁置，等人放行。
            s.status = if step {
                StationStatus::Held
            } else {
                StationStatus::Working
            };
        }
        prev = Some(ids[i].clone());
    }
    let total = view.stations.len() as u32;
    let passed = view.stations.iter().filter(|s| done(s)).count() as u32;
    view.summary.stations_done = passed;
    view.summary.current = view
        .stations
        .iter()
        .find(|s| !done(s))
        .map(|s| s.title.clone());
    if passed == total && total > 0 {
        view.summary.status = RunStatus::Finished;
        view.summary.current = None;
    } else if let Some(h) = view
        .stations
        .iter()
        .find(|s| matches!(s.status, StationStatus::Held | StationStatus::Unmet))
    {
        view.summary.status = RunStatus::Waiting {
            reason: match h.status {
                StationStatus::Held => format!("搁置：{}", h.title),
                _ => format!("未达标：{}", h.title),
            },
        };
    }
}

pub(crate) fn apply_run_op(view: &mut RunView, op: &RunOp) {
    let station = |view: &mut RunView, id: &port::StationId| -> Option<usize> {
        view.stations.iter().position(|s| &s.id == id)
    };
    match op {
        RunOp::Pause => view.summary.status = RunStatus::Paused,
        RunOp::Resume => view.summary.status = RunStatus::Running,
        RunOp::Cancel => view.summary.status = RunStatus::Cancelled,
        // 多人确认：一票一票记，够 `min` 才过；驳回一票就不过（`reject: any`）。
        RunOp::Confirm {
            station: id,
            reject,
            ..
        } if station(view, id).is_some_and(|i| view.stations[i].votes.is_some()) => {
            let i = station(view, id).unwrap_or_default();
            let st = &mut view.stations[i];
            if let Some(v) = st.votes.as_mut() {
                if *reject {
                    v.rejections += 1;
                    st.status = StationStatus::Unmet;
                    st.unmet = Some("rejected".into());
                } else {
                    v.approvals += 1;
                    if v.approvals >= v.min {
                        st.status = StationStatus::Passed;
                        st.unmet = None;
                    }
                }
            }
            view.summary.status = RunStatus::Running;
            advance(view);
            if view.stations[i].status == StationStatus::Working
                && let Some(v) = view.stations[i].votes
            {
                view.summary.status = RunStatus::Waiting {
                    reason: format!(
                        "待确认：{} {}/{}",
                        view.stations[i].title, v.approvals, v.min
                    ),
                };
            }
        }
        RunOp::Confirm { station: id, .. }
        | RunOp::Release { station: id }
        | RunOp::Force { station: id, .. } => {
            if let Some(i) = station(view, id) {
                view.stations[i].status = if matches!(op, RunOp::Release { .. }) {
                    StationStatus::Working
                } else {
                    StationStatus::Passed
                };
                view.stations[i].unmet = None;
            }
            view.summary.status = RunStatus::Running;
            advance(view);
        }
        // 站操作与换路由 `flowops` 定做什么，再落到上面这些。
        RunOp::Goto { .. } | RunOp::Switch { .. } | RunOp::EndWorktree { .. } => {}
        RunOp::Retry { station: id } => {
            if let Some(i) = station(view, id) {
                view.stations[i].status = StationStatus::Working;
                view.stations[i].unmet = None;
            }
            view.summary.status = RunStatus::Running;
        }
        RunOp::Rework { to, .. } => {
            // 接受建议就是打回到它建议的站；别的在等的建议随这次打回作废。
            for p in &mut view.proposals {
                if p.status == port::ProposalStatus::Open {
                    p.status = if &p.to == to {
                        port::ProposalStatus::Accepted
                    } else {
                        port::ProposalStatus::Dismissed
                    };
                }
            }
            if let Some(i) = station(view, to) {
                for s in &mut view.stations[i..] {
                    if s.status != StationStatus::Skipped {
                        s.status = StationStatus::Stale;
                    }
                }
                view.stations[i].status = StationStatus::Working;
            }
            view.summary.status = RunStatus::Running;
            advance(view);
        }
    }
}

async fn scheduler(
    mut rx: mpsc::UnboundedReceiver<Sched>,
    state: Arc<Mutex<MockState>>,
    tx: mpsc::UnboundedSender<Event>,
) {
    let mut heap: BinaryHeap<Reverse<(Instant, u64)>> = BinaryHeap::new();
    let mut pending: BTreeMap<u64, (Option<SessionId>, Event)> = BTreeMap::new();
    let mut seq = 0u64;
    loop {
        let next = heap.peek().map(|Reverse((due, _))| *due);
        tokio::select! {
            biased;
            msg = rx.recv() => match msg {
                None => return,
                Some(Sched::At { due, tag, event }) => {
                    seq += 1;
                    heap.push(Reverse((due, seq)));
                    pending.insert(seq, (tag, *event));
                }
                Some(Sched::Cancel(session)) => {
                    pending.retain(|_, (tag, _)| tag.as_ref() != Some(&session));
                }
            },
            _ = tokio::time::sleep_until(next.unwrap_or_else(Instant::now)), if next.is_some() => {
                let now = Instant::now();
                while let Some(Reverse((due, id))) = heap.peek().copied() {
                    if due > now {
                        break;
                    }
                    heap.pop();
                    let Some((_, event)) = pending.remove(&id) else { continue };
                    let changed = state.lock().unwrap().apply(&event);
                    if tx.send(event).is_err() {
                        return;
                    }
                    if let Some(id) = changed {
                        let summary = state.lock().unwrap().sessions.get(&id).map(|s| s.summary.clone());
                        if let Some(s) = summary {
                            let _ = tx.send(Event::SessionUpserted(s));
                        }
                    }
                }
            }
        }
    }
}
