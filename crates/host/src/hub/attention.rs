//! 等人处理的事（architecture §8.10）：从实例推出去的领域事件里认出来，去重；没 TUI 接着时攒成
//! "离开期间"，配了通知出口就合并一阵再送出去。

use super::*;
use port::{Alert, AlertKind, AlertTarget, NoticeLevel, RunStatus};

use crate::notify::{Notifier, When};

#[derive(Default)]
pub(super) struct Alerts {
    /// 去重：同一件事（种类加对象加状态）只算一次。
    seen: HashSet<String>,
    /// 没 TUI 接着时攒下的，接入时作为"离开期间"推出去。
    away: Vec<Alert>,
    away_since: u64,
    attached: bool,
    /// 等着合并送出的一批。
    batch: Vec<Alert>,
    flushing: bool,
    /// 此刻在等人的运行（`Hello` 的"等你"计数）。
    waiting_runs: HashSet<String>,
}

impl Alerts {
    pub(super) fn waiting_runs(&self) -> usize {
        self.waiting_runs.len()
    }
}

/// 可能是等人处理的事的那几种事件。
pub(super) fn notable(e: &Event) -> bool {
    matches!(
        e,
        Event::PromptPending { .. }
            | Event::RunUpserted(_)
            | Event::JobFired { .. }
            | Event::JobUpserted(_)
    )
}

impl Hub {
    pub fn set_notifier(&self, n: Notifier) {
        *self.0.notifier.locked() = Some(Arc::new(n));
    }

    /// 推出去的事件里认出等人处理的事。
    pub(super) fn observe(&self, events: &[Event]) {
        for e in events {
            if let Event::RunUpserted(r) = e {
                let mut a = self.0.alerts.locked();
                if matches!(r.status, RunStatus::Waiting { .. }) {
                    a.waiting_runs.insert(r.id.0.clone());
                } else {
                    a.waiting_runs.remove(&r.id.0);
                }
            }
            if let Some((key, alert)) = self.alert_of(e) {
                self.raise(key, alert);
            }
        }
    }

    fn alert_of(&self, e: &Event) -> Option<(String, Alert)> {
        let now = self.now();
        let mk = |kind, level, title: String, text: String, target| Alert {
            kind,
            level,
            title,
            text,
            at_ms: now,
            target,
        };
        match e {
            Event::PromptPending { session, prompt } => {
                let title = self
                    .summary(&session.0)
                    .map(|s| s.title)
                    .unwrap_or_default();
                let what = match &prompt.kind {
                    port::PromptKind::Permission { tool, summary, .. } => {
                        format!("permission for {tool}: {summary}")
                    }
                    port::PromptKind::Question { question, .. } => question.clone(),
                };
                Some((
                    format!("prompt:{}", prompt.id),
                    mk(
                        AlertKind::Prompt,
                        NoticeLevel::Warn,
                        format!("session \"{title}\" is waiting for you"),
                        what,
                        Some(AlertTarget::Session(session.clone())),
                    ),
                ))
            }
            Event::RunUpserted(r) => {
                let target = Some(AlertTarget::Run {
                    flow: r.flow.clone(),
                    run: r.id.clone(),
                });
                match &r.status {
                    RunStatus::Waiting { reason } => Some((
                        format!("run:{}:{reason}", r.id),
                        mk(
                            AlertKind::Station,
                            NoticeLevel::Warn,
                            format!("run \"{}\" is waiting for you", r.subject),
                            reason.clone(),
                            target,
                        ),
                    )),
                    RunStatus::Finished | RunStatus::Cancelled => Some((
                        format!("ended:{}", r.id),
                        mk(
                            AlertKind::RunEnded,
                            NoticeLevel::Info,
                            format!(
                                "run \"{}\" {}",
                                r.subject,
                                if r.status == RunStatus::Finished {
                                    "finished"
                                } else {
                                    "was cancelled"
                                }
                            ),
                            r.flow.clone(),
                            target,
                        ),
                    )),
                    _ => None,
                }
            }
            Event::JobFired { job, entry } if entry.outcome.is_failure() => {
                let name = self.job_name(job);
                let why = match &entry.outcome {
                    JobOutcome::Failed { message } => message.clone(),
                    JobOutcome::Stopped { reason } => format!("stopped: {reason:?}").to_lowercase(),
                    _ => String::new(),
                };
                Some((
                    format!("fired:{job}:{}", entry.fired_ms),
                    mk(
                        AlertKind::JobFailed,
                        NoticeLevel::Error,
                        format!("job \"{name}\" failed"),
                        why,
                        Some(AlertTarget::Job(job.clone())),
                    ),
                ))
            }
            Event::JobUpserted(j) => j.auto_disabled.as_ref().map(|a| {
                (
                    format!("disabled:{}:{}", j.id, a.at_ms),
                    mk(
                        AlertKind::JobDisabled,
                        NoticeLevel::Error,
                        format!(
                            "job \"{}\" is disabled after {} failures in a row",
                            j.name, a.after
                        ),
                        String::new(),
                        Some(AlertTarget::Job(j.id.clone())),
                    ),
                )
            }),
            _ => None,
        }
    }

    fn job_name(&self, job: &JobId) -> String {
        self.scheduler()
            .ok()
            .and_then(|s| s.get(&job.0))
            .map(|j| j.name)
            .unwrap_or_else(|| job.0.clone())
    }

    /// 实例自己认出的事（每日额度用完、项目状态挪进 quarantine）。
    pub fn alert(
        &self,
        kind: AlertKind,
        level: NoticeLevel,
        key: String,
        title: String,
        text: String,
    ) {
        let a = Alert {
            kind,
            level,
            title,
            text,
            at_ms: self.now(),
            target: None,
        };
        self.raise(key, a);
    }

    fn raise(&self, key: String, a: Alert) {
        let notifier = self.0.notifier.locked().clone();
        let flush = {
            let mut s = self.0.alerts.locked();
            if !s.seen.insert(key) {
                return;
            }
            if !s.attached {
                s.away.push(a.clone());
            }
            let send = notifier
                .as_ref()
                .is_some_and(|n| n.wants(&a) && (n.cfg.when == When::Always || !s.attached));
            if send {
                s.batch.push(a);
            }
            let start = send && !s.flushing;
            if start {
                s.flushing = true;
            }
            start
        };
        if let (true, Some(n)) = (flush, notifier) {
            let hub = self.clone();
            tokio::spawn(async move {
                // 一阵里来的合成一条送：几件事一起出的时候不刷屏。
                tokio::time::sleep(std::time::Duration::from_millis(n.window_ms)).await;
                let batch = {
                    let mut s = hub.0.alerts.locked();
                    s.flushing = false;
                    std::mem::take(&mut s.batch)
                };
                for r in n.send(&batch).await.into_iter().filter(|r| !r.ok) {
                    tracing::warn!(sink = r.name, "notification not sent: {}", r.detail);
                }
            });
        }
    }

    /// TUI 接入或脱离。接入时把离开期间攒下的推过去。
    pub(super) fn attached(&self, yes: bool) {
        let away = {
            let mut s = self.0.alerts.locked();
            s.attached = yes;
            if !yes {
                s.away_since = self.now();
                return;
            }
            let since = s.away_since;
            (since, std::mem::take(&mut s.away))
        };
        if !away.1.is_empty() {
            self.emit_events(vec![Event::AwaySummary {
                since_ms: away.0,
                alerts: away.1,
            }]);
        }
    }

    /// `TestNotify`：马上发一条，不合并、不去重。
    pub(super) async fn test_notify(&self) -> Vec<port::SinkResult> {
        let Some(n) = self.0.notifier.locked().clone() else {
            return vec![];
        };
        let a = Alert {
            kind: AlertKind::Test,
            level: NoticeLevel::Error,
            title: "Codix test notification".into(),
            text: format!("from instance {} in {}", self.0.instance, self.0.project),
            at_ms: self.now(),
            target: None,
        };
        n.send(&[a]).await
    }
}

impl Hub {
    /// Codix 在 cli 层开了 `permissions.client_may_loosen`（流程站要 `acceptEdits`）；部署方的 managed 层
    /// 可以把它关掉，这时流程站的模式会被夹回去。按引擎实际的判定报（`config.explain`），不按我们写了什么。
    async fn loosen_check(&self) -> port::HealthCheck {
        use port::{HealthCheck, HealthStatus as H};
        let r = self
            .call(
                "config.explain",
                json!({"key": "permissions.client_may_loosen", "scene": "code",
                       "project_root": self.0.project}),
            )
            .await;
        let (status, summary) = match r {
            Ok(v) if v["keys"][0]["value"] == true => (
                H::Ok,
                "Codix lets sessions ask for looser settings than the project's (flow stations run acceptEdits); managed settings still cap it".to_string(),
            ),
            Ok(v) => (
                H::Degraded,
                format!(
                    "permissions.client_may_loosen is {} (managed settings?): flow stations' acceptEdits is clamped to the project's mode, so unattended runs may stop on permission questions",
                    v["keys"][0]["value"]
                ),
            ),
            Err(e) => (H::Degraded, format!("config.explain: {e}")),
        };
        HealthCheck {
            name: "codix.session_loosen".into(),
            status,
            summary,
        }
    }

    /// `/doctor`：引擎的检查加上实例自己的几项（项目状态、`.gitignore`、配置、通知出口、流程）。
    pub(super) async fn doctor(&self) -> Vec<port::HealthCheck> {
        use port::{HealthCheck, HealthStatus as H};
        let check = |name: &str, status, summary: String| HealthCheck {
            name: name.into(),
            status,
            summary,
        };
        let mut out = vec![];
        match self
            .call(
                "daemon.doctor",
                json!({"scene": "code", "project_root": self.0.project}),
            )
            .await
        {
            Ok(v) => out.extend(
                serde_json::from_value::<Vec<HealthCheck>>(v["health"]["checks"].clone())
                    .unwrap_or_default(),
            ),
            Err(e) => out.push(check("agix", H::Failing, e.to_string())),
        }
        let data = std::path::Path::new(&self.0.project).join(".codix");
        let ignored = std::fs::read_to_string(data.join(".gitignore"))
            .is_ok_and(|t| t.lines().any(|l| l.trim() == "state/"));
        out.push(check(
            "codix.project_state",
            H::Ok,
            format!(
                "{} (locked by this instance)",
                data.join("state/codix").display()
            ),
        ));
        out.push(if ignored {
            check(
                "codix.gitignore",
                H::Ok,
                ".codix/.gitignore keeps state/ out of git".into(),
            )
        } else {
            check(
                "codix.gitignore",
                H::Degraded,
                ".codix/.gitignore does not name state/: local state may end up in git".into(),
            )
        });
        let problems = self.0.config_problems.locked().clone();
        out.push(if problems.is_empty() {
            check(
                "codix.config",
                H::Ok,
                "codix.yaml jobs and notify parse".into(),
            )
        } else {
            check("codix.config", H::Degraded, problems.join("; "))
        });
        let sinks = self
            .0
            .notifier
            .locked()
            .as_ref()
            .map_or(0, |n| n.cfg.sinks.len());
        out.push(check(
            "codix.notify",
            H::Ok,
            if sinks == 0 {
                "no notification sinks: what needs you waits until you attach".into()
            } else {
                format!("{sinks} notification sink(s); /notify sends a test")
            },
        ));
        out.push(self.loosen_check().await);
        let network = self.guard_config().defaults.network.unwrap_or_default();
        out.push(match network {
            port::Network::Inherit => check(
                "codix.jobs_network",
                H::Degraded,
                "jobs.defaults.network is inherit: unattended, Agix refuses a job's shell commands that have no allow rule unless the network is restricted (see sandbox.unattended_network)".into(),
            ),
            n => check(
                "codix.jobs_network",
                H::Ok,
                format!(
                    "jobs' shell commands run with network {} unless a job says otherwise",
                    n.sandbox_mode().unwrap_or("inherit")
                ),
            ),
        });
        out.push(if self.flows().is_some() {
            check("codix.flows", H::Ok, "flows are running".into())
        } else {
            check(
                "codix.flows",
                H::Failing,
                "flows are not available (see host.log)".into(),
            )
        });
        out
    }
}
