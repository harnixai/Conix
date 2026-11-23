//! 无人值守的护栏的执行（architecture §8.5）：到点触发前过一遍（每日额度、重叠、并发），简单作业的会话
//! 按帧记账、看超时，作业起的运行按运行的用量记账、超了暂停，连续失败自动停用。

use super::*;
use crate::jobs::ledger::day_of;
use port::{Overlap, SkipCause, StopCause};

/// 一次简单作业的触发在跑：它的会话用了多少。
pub(super) struct Meter {
    job: String,
    fired: u64,
    started: u64,
    /// 上一次从引擎读到的会话累计（`session.get` 的 `usage`：含子 Agent、团队成员、旁路调用，后台的也算）。
    base: u64,
    /// 已经结束的各轮的全部用量之和（`turn_complete.usage`，0.4 起含子 Agent 与旁路调用）。
    turns: u64,
    /// 那之后主会话每次调用的用量之和（`step_end`），等下次读累计时归零。
    steps: u64,
    /// 后台子 Agent 各自到此刻的累计（任务帧带的是累计，不是增量）……
    tasks: HashMap<String, u64>,
    /// ……与上一次读累计时它们各自的值：累计里已经有那一部分了，只算之后多出来的。
    tasks_at_base: HashMap<String, u64>,
    /// 已经记进每日账的。
    counted: u64,
    /// 已经在停了：别再停一次。
    stopping: bool,
    /// 这一次做了什么：触发收尾时写进触发记录。
    audit: port::FiringAudit,
}

/// 没能马上跑的一次到点触发：重叠策略是 `queue`，或同时在跑的满了。过了 `deadline`（下一次到点）还没轮到就跳过。
pub(super) struct Waiting {
    job: Job,
    fired: u64,
    deadline: u64,
    why: SkipCause,
}

/// 多久看一次超时、运行的用量、排队的触发。
pub(super) const TICK_MS: u64 = 10_000;

impl Hub {
    /// 到点触发之前：每日额度、重叠、并发。回 `false` 就是这一次不跑（已经记下跳过，或排进了队）。
    pub(super) async fn admit(&self, job: &Job, fired: u64) -> bool {
        let Ok(sched) = self.scheduler() else {
            return false;
        };
        let cfg = self.guard_config();
        if self.daily_exhausted(&cfg) {
            self.record_skip(job, fired, SkipCause::DailyBudget);
            return false;
        }
        let (guard, _) = cfg.effective(&job.guard);
        if self.job_running(&job.id.0).await {
            match guard.overlap.unwrap_or(Overlap::Skip) {
                Overlap::Skip => {
                    self.record_skip(job, fired, SkipCause::Overlap);
                    return false;
                }
                Overlap::Queue => {
                    let mut q = self.0.waiting.locked();
                    if q.iter().any(|w| w.job.id == job.id) {
                        drop(q);
                        self.record_skip(job, fired, SkipCause::Overlap);
                    } else {
                        q.push(Waiting {
                            job: job.clone(),
                            fired,
                            deadline: next_due(job, fired),
                            why: SkipCause::Overlap,
                        });
                    }
                    return false;
                }
                Overlap::Replace => {
                    if let Err(e) = self.stop_firing(&job.id.0, None, StopCause::Replaced).await {
                        tracing::warn!(job = %job.id, "replacing the running firing: {e}");
                    }
                    return true;
                }
            }
        }
        if self.running_count().await >= cfg.max_concurrent as usize {
            self.0.waiting.locked().push(Waiting {
                job: job.clone(),
                fired,
                deadline: next_due(job, fired),
                why: SkipCause::Busy,
            });
            return false;
        }
        drop(sched);
        true
    }

    async fn job_running(&self, job: &str) -> bool {
        if self.0.meters.locked().values().any(|m| m.job == job) {
            return true;
        }
        match self.flows() {
            Some(f) => f
                .scheduled_runs()
                .await
                .iter()
                .any(|r| r.job == job && !r.paused),
            None => false,
        }
    }

    async fn running_count(&self) -> usize {
        let sessions = self.0.meters.locked().len();
        let runs = match self.flows() {
            Some(f) => f
                .scheduled_runs()
                .await
                .iter()
                .filter(|r| !r.paused)
                .count(),
            None => 0,
        };
        sessions + runs
    }

    fn daily_exhausted(&self, cfg: &crate::jobs::guard::GuardConfig) -> bool {
        if cfg.daily_tokens == 0 {
            return false;
        }
        let Ok(sched) = self.scheduler() else {
            return false;
        };
        let today = day_of(self.now());
        sched.store().locked().ledger().total(&today) >= cfg.daily_tokens
    }

    /// 记一条跳过的触发并推出去。
    fn record_skip(&self, job: &Job, fired: u64, why: SkipCause) {
        let Ok(sched) = self.scheduler() else { return };
        let entry = JobEntry {
            fired_ms: fired,
            outcome: JobOutcome::Skipped { reason: why },
            elapsed_ms: 0,
            usage: Usage::default(),
            target: None,
            audit: None,
        };
        let _ = sched.store().locked().append(&job.id.0, &entry);
        let mut events = vec![Event::JobFired {
            job: job.id.clone(),
            entry,
        }];
        events.extend(sched.get(&job.id.0).map(Event::JobUpserted));
        self.emit_events(events);
    }

    /// 简单作业的会话开始跑了：从这一刻起记账、看超时。
    pub(super) fn meter_start(&self, session: &str, job: &str, fired: u64) {
        self.0.meters.locked().insert(
            session.to_string(),
            Meter {
                job: job.to_string(),
                fired,
                started: self.now(),
                base: 0,
                turns: 0,
                steps: 0,
                tasks: HashMap::new(),
                tasks_at_base: HashMap::new(),
                counted: 0,
                stopping: false,
                audit: Default::default(),
            },
        );
    }

    /// 触发收尾：不再记账，交出这一次做了什么。
    pub(super) fn meter_take(&self, session: &str) -> Option<port::FiringAudit> {
        self.0.meters.locked().remove(session).map(|m| m.audit)
    }

    /// 作业的会话来了一帧：用量记进账，超了单次上限或每日额度就停。
    ///
    /// 用量的准数是引擎的会话累计（`session.get`）：0.4 起它含子 Agent、团队成员与旁路调用（压缩、记忆、
    /// WebSearch、hook），口径与引擎的预算一致。帧里只看得到主会话自己的调用（`step_end`）与后台子 Agent
    /// 的累计，所以平时按帧估、每轮结束与后台任务结束时读一次累计校准。真正拦住超支的是引擎的
    /// `session.token_budget`（建会话时按这次触发的上限给），这里管的是每日额度与界面上的数。
    ///
    /// 一轮结束时直接用帧里这一轮的全部用量校准：紧接着 `turn_done` 就收走计数器，等不到异步读累计。
    pub(super) fn meter_frame(&self, session: &str, ev: &SessionEvent) {
        let sync = {
            let mut meters = self.0.meters.locked();
            let Some(m) = meters.get_mut(session) else {
                return;
            };
            match ev {
                SessionEvent::ToolUse { name, .. } => {
                    *m.audit.tools.entry(name.clone()).or_insert(0) += 1;
                    return;
                }
                SessionEvent::ToolResult {
                    is_error,
                    file_changes,
                    ..
                } => {
                    if *is_error == Some(true) {
                        m.audit.errors += 1;
                    }
                    for f in file_changes {
                        if !m.audit.files.contains(&f.path) {
                            m.audit.files.push(f.path.clone());
                        }
                    }
                    return;
                }
                SessionEvent::StepEnd { usage } => {
                    m.steps += billed(usage);
                    false
                }
                SessionEvent::TaskStarted { task } | SessionEvent::TaskUpdated { task } => {
                    if let Some(u) = &task.usage {
                        m.tasks.insert(task.task_id.clone(), billed(u));
                    }
                    false
                }
                SessionEvent::TaskFinished { task } => {
                    if let Some(u) = &task.usage {
                        m.tasks.insert(task.task_id.clone(), billed(u));
                    }
                    true
                }
                SessionEvent::TurnComplete { usage, .. } => {
                    m.turns += billed(usage);
                    m.base = m.turns;
                    m.steps = 0;
                    m.tasks_at_base = m.tasks.clone();
                    false
                }
                _ => return,
            }
        };
        self.meter_account(session);
        if sync {
            let hub = self.clone();
            let session = session.to_string();
            tokio::spawn(async move { hub.meter_sync(&session).await });
        }
    }

    /// 读引擎的会话累计，校准估的数。
    async fn meter_sync(&self, session: &str) {
        let Ok(r) = self
            .call("session.get", json!({"session_id": session}))
            .await
        else {
            return;
        };
        let Ok(u) = serde_json::from_value::<wire::frames::Usage>(r["usage"].clone()) else {
            return;
        };
        {
            let mut meters = self.0.meters.locked();
            let Some(m) = meters.get_mut(session) else {
                return;
            };
            // 读到的可能比刚结束的那一轮校准的旧（帧先到）：不往回退。
            m.base = m.base.max(billed(&u));
            m.steps = 0;
            m.tasks_at_base = m.tasks.clone();
        }
        self.meter_account(session);
    }

    /// 按此刻估的总数记账（只增不减：校准后比估的少，也不倒扣），超了单次上限或每日额度就停。
    fn meter_account(&self, session: &str) {
        let (job, fired, delta, total) = {
            let mut meters = self.0.meters.locked();
            let Some(m) = meters.get_mut(session) else {
                return;
            };
            let total = meter_total(m);
            let delta = total.saturating_sub(m.counted);
            m.counted = m.counted.max(total);
            (m.job.clone(), m.fired, delta, total)
        };
        let Ok(sched) = self.scheduler() else { return };
        let cfg = self.guard_config();
        if delta > 0 {
            let today = day_of(self.now());
            let _ = sched.store().locked().ledger().add(&today, &job, delta);
        }
        let limit = sched
            .get(&job)
            .map(|j| cfg.effective(&j.guard).0.max_tokens.unwrap_or(u64::MAX))
            .unwrap_or(u64::MAX);
        if total > limit {
            self.stop_meter(session, &job, fired, StopCause::Budget);
        } else if self.daily_exhausted(&cfg) {
            self.exhaust();
        }
    }

    /// 停一个在跑的简单作业触发（只停一次）。
    fn stop_meter(&self, session: &str, job: &str, fired: u64, why: StopCause) {
        {
            let mut meters = self.0.meters.locked();
            match meters.get_mut(session) {
                Some(m) if !m.stopping => m.stopping = true,
                _ => return,
            }
        }
        let hub = self.clone();
        let job = job.to_string();
        tokio::spawn(async move {
            if let Err(e) = hub.stop_firing(&job, Some(fired), why).await {
                tracing::warn!(job, "stopping a firing ({why:?}): {e}");
            }
        });
    }

    /// 今天的额度用完了：在跑的都停、作业起的运行都暂停，说一次。
    fn exhaust(&self) {
        let today = day_of(self.now());
        {
            let mut ex = self.0.exhausted.locked();
            if ex.as_deref() == Some(today.as_str()) {
                return;
            }
            *ex = Some(today);
        }
        let running: Vec<(String, String, u64)> = self
            .0
            .meters
            .locked()
            .iter()
            .map(|(s, m)| (s.clone(), m.job.clone(), m.fired))
            .collect();
        for (s, job, fired) in running {
            self.stop_meter(&s, &job, fired, StopCause::DailyBudget);
        }
        let hub = self.clone();
        tokio::spawn(async move {
            if let Some(f) = hub.flows() {
                for r in f.scheduled_runs().await.into_iter().filter(|r| !r.paused) {
                    hub.pause_run(&f, &r, "the daily budget is used up").await;
                }
            }
        });
        let cfg = self.guard_config();
        let text = format!(
            "unattended work used up today's budget ({} tokens): running jobs stopped, the rest of today's firings are skipped",
            crate::jobs::guard::fmt_tokens(cfg.daily_tokens)
        );
        self.alert(
            port::AlertKind::DailyBudget,
            port::NoticeLevel::Warn,
            format!("daily:{}", day_of(self.now())),
            "today's unattended budget is used up".into(),
            text.clone(),
        );
        self.emit_events(vec![Event::Notice {
            level: port::NoticeLevel::Warn,
            text,
        }]);
    }

    async fn pause_run(&self, f: &Arc<dyn FlowPort>, r: &ScheduledRun, why: &str) {
        if !self.0.paused_runs.locked().insert(r.run.0.clone()) {
            return;
        }
        let res = f
            .command(Command::RunOp {
                run: r.run.clone(),
                op: port::RunOp::Pause,
            })
            .await;
        let text = match res {
            Ok(_) => format!("paused the run \"{}\" of job {}: {why}", r.subject, r.job),
            Err(e) => format!(
                "could not pause the run \"{}\" of job {}: {e}",
                r.subject, r.job
            ),
        };
        self.emit_events(vec![Event::Notice {
            level: port::NoticeLevel::Warn,
            text,
        }]);
    }

    /// 定时看一遍：简单作业的超时、作业起的运行的用量与超时、排队的触发。
    pub(super) async fn guard_tick(&self) {
        let Ok(sched) = self.scheduler() else { return };
        let cfg = self.guard_config();
        let now = self.now();
        let effective = |job: &str| {
            sched
                .get(job)
                .map(|j| cfg.effective(&j.guard).0)
                .unwrap_or_else(|| cfg.defaults.clone())
        };
        let overdue: Vec<(String, String, u64)> = self
            .0
            .meters
            .locked()
            .iter()
            .filter(|(_, m)| !m.stopping)
            .filter(|(_, m)| {
                now.saturating_sub(m.started) > effective(&m.job).timeout_ms.unwrap_or(u64::MAX)
            })
            .map(|(s, m)| (s.clone(), m.job.clone(), m.fired))
            .collect();
        for (s, job, fired) in overdue {
            self.stop_meter(&s, &job, fired, StopCause::Timeout);
        }

        if let Some(f) = self.flows() {
            let runs = f.scheduled_runs().await;
            let today = day_of(now);
            for r in &runs {
                let delta = {
                    let mut seen = self.0.run_seen.locked();
                    let last = seen.insert(r.run.0.clone(), r.tokens).unwrap_or(0);
                    r.tokens.saturating_sub(last)
                };
                let _ = sched.store().locked().ledger().add(&today, &r.job, delta);
                if r.paused {
                    continue;
                }
                let g = effective(&r.job);
                if now.saturating_sub(r.started_ms) > g.timeout_ms.unwrap_or(u64::MAX) {
                    self.pause_run(&f, r, "it ran past the job's timeout").await;
                } else if r.tokens > g.max_tokens.unwrap_or(u64::MAX) {
                    self.pause_run(&f, r, "it used more tokens than the job allows")
                        .await;
                }
            }
            // 结束了的运行不再记。
            self.0
                .run_seen
                .locked()
                .retain(|id, _| runs.iter().any(|r| &r.run.0 == id));
        }
        if self.daily_exhausted(&cfg) {
            self.exhaust();
        }

        let waiting: Vec<Waiting> = std::mem::take(&mut *self.0.waiting.locked());
        let mut keep = vec![];
        for w in waiting {
            if now >= w.deadline {
                self.record_skip(&w.job, w.fired, w.why);
                continue;
            }
            let free = match w.why {
                SkipCause::Overlap => !self.job_running(&w.job.id.0).await,
                _ => true,
            } && self.running_count().await < cfg.max_concurrent as usize
                && !self.daily_exhausted(&cfg);
            if free {
                let hub = self.clone();
                tokio::spawn(async move { hub.launch(w.job, now).await });
            } else {
                keep.push(w);
            }
        }
        self.0.waiting.locked().extend(keep);
    }

    /// 一次触发收尾之后：连续失败到了上限就在这台机器上停用这个作业。
    pub(super) fn after_finish(&self, job: &str) {
        let Ok(sched) = self.scheduler() else { return };
        let cfg = self.guard_config();
        let Some(j) = sched.get(job) else { return };
        let after = cfg.effective(&j.guard).0.disable_after.unwrap_or(0);
        if after == 0 || !j.enabled {
            return;
        }
        let streak = sched.store().locked().failure_streak(job);
        if streak < after {
            return;
        }
        let disabled = sched
            .store()
            .locked()
            .auto_disable(job, streak, self.now())
            .unwrap_or(false);
        if !disabled {
            return;
        }
        let mut events = vec![Event::Notice {
            level: port::NoticeLevel::Warn,
            text: format!(
                "job \"{}\" failed {streak} times in a row and is disabled on this machine",
                j.name
            ),
        }];
        events.extend(sched.get(job).map(Event::JobUpserted));
        self.emit_events(events);
    }
}

/// 下一次到点的时刻：排着的触发等到这时还没轮到就跳过。
fn next_due(job: &Job, fired: u64) -> u64 {
    crate::jobs::cron::next_after(&job.cron, fired).unwrap_or(fired + 3_600_000)
}

/// 计费口径与 Agix 的预算、配额一致：未命中缓存的输入 + 缓存写入 + 输出，缓存命中不计。
pub(super) fn billed(u: &wire::frames::Usage) -> u64 {
    u.input_tokens + u.cache_creation_input_tokens + u.output_tokens
}

fn meter_total(m: &Meter) -> u64 {
    let tasks: u64 = m
        .tasks
        .iter()
        .map(|(id, v)| v.saturating_sub(m.tasks_at_base.get(id).copied().unwrap_or(0)))
        .sum();
    m.base + m.steps + tasks
}

#[cfg(test)]
mod meter_tests {
    use super::*;

    fn usage(input: u64, write: u64, read: u64, output: u64) -> wire::frames::Usage {
        wire::frames::Usage {
            input_tokens: input,
            cache_creation_input_tokens: write,
            cache_read_input_tokens: read,
            output_tokens: output,
        }
    }

    #[test]
    fn cache_reads_are_not_billed() {
        assert_eq!(billed(&usage(100, 20, 5_000, 30)), 150);
    }

    #[test]
    fn a_sync_replaces_the_estimate_without_counting_tasks_twice() {
        let mut m = Meter {
            job: "j".into(),
            fired: 0,
            started: 0,
            base: 0,
            turns: 0,
            steps: 300,
            tasks: HashMap::from([("t".to_string(), 1_000)]),
            tasks_at_base: HashMap::new(),
            counted: 0,
            stopping: false,
            audit: Default::default(),
        };
        assert_eq!(meter_total(&m), 1_300);
        // 引擎的累计已含后台任务到此刻的 1000 与主会话的调用，再加旁路调用 200。
        m.base = 1_500;
        m.steps = 0;
        m.tasks_at_base = m.tasks.clone();
        assert_eq!(meter_total(&m), 1_500);
        m.tasks.insert("t".into(), 1_400);
        m.steps = 50;
        assert_eq!(meter_total(&m), 1_950, "之后只加多出来的");
    }
}
