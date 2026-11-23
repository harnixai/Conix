//! 作业的一次触发：简单作业建会话、流程作业起运行（architecture §8.5）。

use super::*;

impl Hub {
    /// 一次触发：到点的先过护栏（`guard.rs`），人"立即运行"的直接跑。
    pub(super) async fn fire_job(&self, job: Job, fired_ms: u64, scheduled: bool) {
        if scheduled && !self.admit(&job, fired_ms).await {
            return;
        }
        self.launch(job, fired_ms).await;
    }

    pub(super) async fn launch(&self, job: Job, fired_ms: u64) {
        let Ok(sched) = self.scheduler() else { return };
        let store = sched.store();
        let (entry, run) = match &job.kind {
            JobKind::Simple {
                prompt,
                skills,
                scene,
                model,
                ..
            } => {
                let extra = json!({"attended": false, "auto_name": false});
                let guard = self.guard_config().effective(&job.guard).0;
                let settings = SessionSettings {
                    // 引擎在每次调用前按剩余预算拦（含子 Agent、旁路调用），不会像事后数帧那样冲过头；
                    // Codix 的计数仍在，管每日额度与界面（`meter_frame`）。一次触发就是一个新会话，
                    // 所以会话预算就是这次触发的预算。
                    token_budget: guard.max_tokens,
                    network: guard
                        .network
                        .and_then(port::Network::sandbox_mode)
                        .map(Into::into),
                    // 无人值守的会话里模型再自建定时任务，就在 Codix 的作业与护栏之外了。
                    deny: vec!["CronCreate".into()],
                    prompt_append: (!skills.is_empty()).then(|| {
                        format!(
                            "This is the scheduled job \"{}\". Use these skills: {}.",
                            job.name,
                            skills.join(", ")
                        )
                    }),
                    ..SessionSettings::default()
                };
                let origin = Origin::Job {
                    job: job.id.clone(),
                    name: job.name.clone(),
                    fired_ms,
                };
                match self
                    .create_session(
                        origin,
                        Some(job.name.clone()),
                        scene.clone(),
                        model.clone(),
                        extra,
                        settings,
                    )
                    .await
                {
                    Ok(sid) => (
                        JobEntry {
                            fired_ms,
                            outcome: JobOutcome::Running,
                            elapsed_ms: 0,
                            usage: Usage::default(),
                            target: Some(JobTarget::Session {
                                id: SessionId(sid.clone()),
                                title: job.name.clone(),
                            }),
                            audit: None,
                        },
                        Some((sid, prompt.clone())),
                    ),
                    Err(e) => (
                        JobEntry {
                            fired_ms,
                            outcome: JobOutcome::Failed {
                                message: e.to_string(),
                            },
                            elapsed_ms: 0,
                            usage: Usage::default(),
                            target: None,
                            audit: None,
                        },
                        None,
                    ),
                }
            }
            // 流程作业：启动一次运行。运行有它自己的结局，触发记录只记“启动了没有”。
            JobKind::Flow {
                flow,
                subject,
                inputs,
            } => {
                let started = match self.flows() {
                    Some(f) => f.start_for_job(flow, subject, inputs.clone(), &job).await,
                    None => Err("flows are not available in this instance".into()),
                };
                let (outcome, target) = match started {
                    Ok((id, subject)) => (JobOutcome::Done, Some(JobTarget::Run { id, subject })),
                    Err(message) => (JobOutcome::Failed { message }, None),
                };
                (
                    JobEntry {
                        fired_ms,
                        outcome,
                        elapsed_ms: 0,
                        usage: Usage::default(),
                        target,
                        audit: None,
                    },
                    None,
                )
            }
        };
        let _ = store.locked().append(&job.id.0, &entry);
        let mut events = vec![Event::JobFired {
            job: job.id.clone(),
            entry,
        }];
        if let Some(j) = sched.get(&job.id.0) {
            events.push(Event::JobUpserted(j));
        }
        self.emit_events(events);
        if let Some((sid, prompt)) = run {
            self.0
                .job_runs
                .locked()
                .insert(sid.clone(), (job.id.0.clone(), fired_ms, self.now()));
            self.meter_start(&sid, &job.id.0, fired_ms);
            self.run_turn(&sid, &prompt, "queue");
        } else {
            // 这一次在这里就有了结局：流程作业（触发记录只记启动成没成）、没起来的简单作业。
            self.after_finish(&job.id.0);
        }
    }
}

impl Hub {
    /// 存之前：流程作业的 `flow`（`#n` 或名字）认成流程名、`inputs` 合它的 schema；简单作业要有提示词。
    pub(super) async fn normalize_job(&self, mut job: Job) -> Result<Job, String> {
        match &mut job.kind {
            JobKind::Flow { flow, inputs, .. } => {
                let f = self
                    .flows()
                    .ok_or("flows are not available in this instance")?;
                *flow = f.check_job_flow(flow, inputs).await?;
            }
            JobKind::Simple { prompt, .. } if prompt.trim().is_empty() => {
                return Err("prompt must not be empty".into());
            }
            JobKind::Simple { .. } => {}
        }
        Ok(job)
    }

    /// 作业此刻生效的网络（流程作业起的运行在第一次开站会话时取一次，见 `flows::policy`）。
    pub(crate) fn job_network(&self, job: &str) -> Option<port::Network> {
        let sched = self.scheduler().ok()?;
        let j = sched.get(job)?;
        self.guard_config().effective(&j.guard).0.network
    }

    /// 校验一份作业定义，不落盘（`ValidateJob`、`job_validate`）。
    pub(super) async fn check_job(&self, job: &Job) -> port::JobCheck {
        let mut problems = vec![];
        if job.name.trim().is_empty() {
            problems.push("name must not be empty".to_string());
        }
        if let Err(e) = crate::jobs::cron::validate(&job.cron) {
            problems.push(e);
        }
        match &job.kind {
            JobKind::Simple { prompt, .. } if prompt.trim().is_empty() => {
                problems.push("prompt must not be empty".into())
            }
            JobKind::Simple { .. } => {}
            JobKind::Flow {
                flow,
                subject,
                inputs,
            } => {
                if subject.trim().is_empty() {
                    problems.push("subject must not be empty".into());
                }
                match self.flows() {
                    Some(f) => {
                        if let Err(e) = f.check_job_flow(flow, inputs).await {
                            problems.push(e);
                        }
                    }
                    None => problems.push("flows are not available in this instance".into()),
                }
            }
        }
        let mut next = vec![];
        let mut at = self.now();
        while next.len() < 5 {
            match crate::jobs::cron::next_after(&job.cron, at) {
                Some(t) => {
                    next.push(t);
                    at = t;
                }
                None => break,
            }
        }
        let (guard, capped) = self.guard_config().effective(&job.guard);
        let mut notes = vec![];
        if guard.network == Some(port::Network::Inherit) {
            notes.push(
                "network: inherit — unattended, Agix refuses a shell command that has no allow rule in the settings unless the network is restricted (sandbox.unattended_needs_network_policy); give the commands this job needs permissions.allow rules in the project's settings, or use deny_all / allowlist"
                    .into(),
            );
        }
        port::JobCheck {
            next_fire_ms: next,
            guard,
            capped,
            problems,
            notes,
        }
    }

    /// 停下一个作业正在跑的那次触发（`fired_ms` 省略 = 正在跑的那次）。简单作业打断它的会话，那一轮
    /// 结束时记成 `Stopped{reason}`；流程作业取消它起的、还没结束的运行。
    pub(crate) async fn stop_firing(
        &self,
        job: &str,
        fired_ms: Option<u64>,
        reason: port::StopCause,
    ) -> Result<(), String> {
        let sched = self.scheduler().map_err(|e| e.to_string())?;
        let entry = {
            let store = sched.store();
            let s = store.locked();
            s.history(job, 100).into_iter().find(|e| {
                fired_ms.is_none_or(|f| e.fired_ms == f)
                    && (e.outcome == JobOutcome::Running
                        || matches!(e.target, Some(JobTarget::Run { .. })))
            })
        };
        match entry.and_then(|e| e.target) {
            Some(JobTarget::Session { id, .. }) => {
                self.0.stop_causes.locked().insert(id.0.clone(), reason);
                self.interrupt(&id.0).await.map_err(|e| e.to_string())
            }
            Some(JobTarget::Run { id, .. }) => {
                let f = self
                    .flows()
                    .ok_or("flows are not available in this instance")?;
                f.command(Command::RunOp {
                    run: id,
                    op: port::RunOp::Cancel,
                })
                .await
                .map(|_| ())
                .map_err(|e| e.to_string())
            }
            None => Err(format!("job {job} has no firing running")),
        }
    }
}
