//! 运行与触发：根入口、运行时目录里的报告、重跑、清理，以及会话里的诊断（architecture §8.12）。

use super::*;

use client::ProjectState;

/// 报告里留最后一轮回复的末尾多少字：够看出它做到哪、为什么停，又不把整段转录搬进来。
const REPLY_TAIL: usize = 4000;
/// `work_cleanup` 缺省清多久以前的。
const CLEANUP_DAYS: u64 = 30;
const DAY_MS: u64 = 24 * 3600 * 1000;

impl Hub {
    fn project_state(&self) -> ProjectState {
        ProjectState::new(&self.0.project)
    }

    /// 一个会话在报告里的样子：它是谁、用的什么模型、花了多少。
    fn session_brief(&self, id: &str) -> Value {
        let st = self.0.st.locked();
        let Some(sess) = st.session_or_closed(id) else {
            return json!({"id": id});
        };
        let sum = st.summary_of(sess);
        let tasks: Vec<Value> = sess.tasks.values().map(task_brief).collect();
        json!({
            "id": id, "title": sum.title, "state": sum.state, "model": sum.provenance.model,
            "usage": sum.usage, "tasks": tasks,
        })
    }

    /// 一次触发的 `report.json`：起来时写一份 `running` 的，结束时带上最后一轮回复重写。
    pub(super) fn write_firing_report(&self, job: &Job, entry: &JobEntry, reply: Option<&str>) {
        let dir = self.project_state().job_run_dir(&job.id.0, entry.fired_ms);
        let (kind, session) = match (&job.kind, &entry.target) {
            (JobKind::Simple { .. }, Some(JobTarget::Session { id, .. })) => {
                ("simple", Some(self.session_brief(&id.0)))
            }
            (JobKind::Simple { .. }, _) => ("simple", None),
            (JobKind::Flow { .. }, _) => ("flow", None),
        };
        let report = json!({
            "job": {"id": job.id, "name": job.name, "kind": kind},
            "entry": entry,
            "session": session,
            "reply_tail": reply.map(tail),
            "dir": dir,
            "written_ms": self.now(),
        });
        write_json(&dir.join("report.json"), &report);
    }

    /// 一次运行的过程目录：流程库说在哪就在哪（有制品目录时在 `<制品>/runs/<运行>/_run`）。
    fn run_process_dir(&self, run: &str) -> std::path::PathBuf {
        self.flows()
            .and_then(|f| f.process_dir(run))
            .unwrap_or_else(|| self.project_state().run_dir(run))
    }

    /// 一次运行结束时的 `conix-report.json`，写在流程库的过程目录里：运行的东西都在一个目录。
    pub(crate) fn write_run_report(&self, v: &port::RunView) {
        let dir = self.run_process_dir(&v.summary.id.0);
        write_json(&dir.join("conix-report.json"), &self.run_report(v, &dir));
    }

    fn run_report(&self, v: &port::RunView, dir: &std::path::Path) -> Value {
        let stations: Vec<Value> = v
            .stations
            .iter()
            .map(|st| {
                json!({
                    "id": st.id, "title": st.title, "status": st.status, "work": st.work,
                    "elapsed_ms": st.elapsed_ms, "usage": st.usage, "unmet": st.unmet,
                    "session": st.session.as_ref().map(|s| self.session_brief(&s.0)),
                    "tasks": st.tasks.iter().map(|t| json!({
                        "id": t.id, "label": t.label, "status": t.status,
                        "session": t.session.as_ref().map(|s| self.session_brief(&s.0)),
                    })).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "run": v.summary,
            "def_digest": v.def_digest,
            "main_session": self.session_brief(&v.main_session.0),
            "stations": stations,
            "dir": dir,
            "written_ms": self.now(),
        })
    }

    pub(super) async fn work_tool(
        &self,
        scope: &tools::Scope,
        name: &str,
        input: &Value,
    ) -> Result<String, String> {
        match name {
            tools::WORK_LIST => self.work_list(input).await,
            tools::WORK_SHOW => self.work_show(input).await,
            tools::CONIX_DOCTOR => {
                let Reply::Doctor(checks) = self
                    .dispatch(Command::Doctor)
                    .await
                    .map_err(|e| e.to_string())?
                else {
                    return Err("no checks".into());
                };
                Ok(checks
                    .iter()
                    .map(|c| format!("{:?}  {}  {}", c.status, c.name, c.summary).to_lowercase())
                    .collect::<Vec<_>>()
                    .join("\n"))
            }
            // 往外发、改东西的：只给人开的会话。
            _ if *scope != tools::Scope::Full => Err(tool_calls::out_of_scope(scope)),
            tools::FLOW_RERUN => {
                let run = tools::str_of(input, "run").ok_or("run is required")?;
                let f = self
                    .flows()
                    .ok_or("flows are not available in this instance")?;
                let s = f.rerun(run.trim()).await?;
                Ok(format!(
                    "started run {} of flow {} on \"{}\" again, as run {}",
                    run.trim(),
                    s.flow,
                    s.subject,
                    s.id
                ))
            }
            tools::NOTIFY_TEST => {
                let Reply::Notified(sinks) = self
                    .dispatch(Command::TestNotify)
                    .await
                    .map_err(|e| e.to_string())?
                else {
                    return Err("nothing was sent".into());
                };
                Ok(if sinks.is_empty() {
                    "no notification sinks are configured (conix.yaml notify)".into()
                } else {
                    sinks
                        .iter()
                        .map(|s| {
                            format!(
                                "{}  {}  {}",
                                if s.ok { "ok" } else { "failed" },
                                s.name,
                                s.detail
                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n")
                })
            }
            tools::WORK_CLEANUP => self.work_cleanup(input).await,
            other => Err(format!("unknown tool {other}")),
        }
    }

    async fn work_list(&self, input: &Value) -> Result<String, String> {
        let kind = tools::str_of(input, "kind").unwrap_or_else(|| "all".into());
        let status = tools::str_of(input, "status").unwrap_or_else(|| "all".into());
        let limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(20)
            .min(200) as u32;
        let flow = tools::str_of(input, "flow");
        let job_key = tools::str_of(input, "job");
        let want = |s: &str| status == "all" || status == s;
        let mut lines = vec![];
        if kind != "job" {
            match self.flows() {
                Some(f) => {
                    for r in f.project_runs(limit).await? {
                        let st = run_state(&r.status);
                        if !want(st) || flow.as_ref().is_some_and(|f| *f != r.flow) {
                            continue;
                        }
                        lines.push(format!(
                            "run {}  flow {}  \"{}\"  {}  stations {}/{}{}  started {}{}  {} tokens",
                            r.id,
                            r.flow,
                            r.subject,
                            run_status_text(&r.status),
                            r.stations_done,
                            r.stations_total,
                            r.current.as_ref().map(|c| format!(" (at {c})")).unwrap_or_default(),
                            tool_calls::local_time(r.started_ms),
                            r.ended_ms
                                .map(|e| format!("  ended {}", tool_calls::local_time(e)))
                                .unwrap_or_default(),
                            r.usage.total(),
                        ));
                    }
                }
                None if kind == "flow" => {
                    return Err("flows are not available in this instance".into());
                }
                None => {}
            }
        }
        if kind != "flow"
            && let Ok(sched) = self.scheduler()
        {
            let jobs = sched.list();
            let picked: Vec<&Job> = match &job_key {
                Some(k) => vec![job_by(&jobs, k)?],
                None => jobs.iter().collect(),
            };
            let store = sched.store();
            let s = store.locked();
            let mut firings: Vec<(&Job, JobEntry)> = vec![];
            for j in picked {
                for e in s.history(&j.id.0, limit as usize) {
                    let st = if e.outcome == JobOutcome::Running {
                        "active"
                    } else {
                        "ended"
                    };
                    if want(st) {
                        firings.push((j, e));
                    }
                }
            }
            firings.sort_by_key(|(_, e)| std::cmp::Reverse(e.fired_ms));
            firings.truncate(limit as usize);
            for (j, e) in firings {
                lines.push(format!(
                    "firing of job {} \"{}\"  {}",
                    j.id,
                    j.name,
                    tool_calls::firing_line(&e)
                ));
            }
        }
        Ok(if lines.is_empty() {
            "nothing matches".into()
        } else {
            lines.join("\n")
        })
    }

    async fn work_show(&self, input: &Value) -> Result<String, String> {
        if let Some(run) = tools::str_of(input, "run") {
            let f = self
                .flows()
                .ok_or("flows are not available in this instance")?;
            let v = f.run_view(run.trim()).await?;
            let dir = self.run_process_dir(&v.summary.id.0);
            let saved = std::fs::read_to_string(dir.join("conix-report.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<Value>(&t).ok());
            let now = self.run_report(&v, &dir);
            let out = match saved {
                Some(saved) if run_state(&v.summary.status) == "ended" => saved,
                _ => now,
            };
            return Ok(serde_json::to_string_pretty(&out).unwrap_or_default());
        }
        let sched = self.scheduler().map_err(|e| e.to_string())?;
        let jobs = sched.list();
        let job = tools::str_of(input, "job")
            .and_then(|k| job_by(&jobs, &k).ok())
            .ok_or("give run (a run id), or job (\"#n\" or its id) and fired_ms (from work_list or job_history)")?;
        let entry = {
            let store = sched.store();
            let s = store.locked();
            let history = s.history(&job.id.0, usize::MAX);
            match input.get("fired_ms").and_then(Value::as_u64) {
                Some(f) => history.into_iter().find(|e| e.fired_ms == f),
                None => history.into_iter().next(),
            }
        }
        .ok_or_else(|| format!("no such firing of \"{}\"", job.name))?;
        let dir = self.project_state().job_run_dir(&job.id.0, entry.fired_ms);
        let mut report = std::fs::read_to_string(dir.join("report.json"))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or_else(|| json!({"job": {"id": job.id, "name": job.name}, "dir": dir}));
        // 触发记录是准的（结局可能在报告之后才补上）；还在跑的会话写此刻的样子。
        report["entry"] = json!(entry);
        if let Some(JobTarget::Session { id, .. }) = &entry.target
            && entry.outcome == JobOutcome::Running
        {
            report["session"] = self.session_brief(&id.0);
        }
        if let Some(JobTarget::Run { id, .. }) = &entry.target
            && let Some(f) = self.flows()
            && let Ok(v) = f.run_view(&id.0).await
        {
            report["run"] = json!(v.summary);
        }
        Ok(serde_json::to_string_pretty(&report).unwrap_or_default())
    }

    async fn work_cleanup(&self, input: &Value) -> Result<String, String> {
        let days = input
            .get("older_than_days")
            .and_then(Value::as_u64)
            .unwrap_or(CLEANUP_DAYS);
        let dry = input
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        let before = self.now().saturating_sub(days * DAY_MS);
        let mut lines = vec![];
        if let Ok(sched) = self.scheduler() {
            // `list` 自己要拿存储的锁：先取出来，再锁着读写触发记录。
            let jobs = sched.list();
            let store = sched.store();
            let s = store.locked();
            for j in jobs {
                let old: Vec<u64> = s
                    .history(&j.id.0, usize::MAX)
                    .into_iter()
                    .filter(|e| e.fired_ms < before && e.outcome != JobOutcome::Running)
                    .map(|e| e.fired_ms)
                    .collect();
                if old.is_empty() {
                    continue;
                }
                if !dry {
                    s.prune_before(&j.id.0, before)?;
                    for f in &old {
                        let _ =
                            std::fs::remove_dir_all(self.project_state().job_run_dir(&j.id.0, *f));
                    }
                }
                lines.push(format!(
                    "{} {} firing(s) of job {} \"{}\" that ended before {}",
                    if dry { "would remove" } else { "removed" },
                    old.len(),
                    j.id,
                    j.name,
                    tool_calls::local_time(before)
                ));
            }
        }
        if let Some(f) = self.flows() {
            let ended = f
                .project_runs(u32::MAX)
                .await?
                .into_iter()
                .filter(|r| r.ended_ms.is_some_and(|e| e < before))
                .count();
            if ended > 0 {
                lines.push(format!(
                    "{ended} flow run(s) ended before {}: kept — archiving runs needs the flow library's support, which it does not have yet",
                    tool_calls::local_time(before)
                ));
            }
        }
        if lines.is_empty() {
            return Ok(format!("nothing ended more than {days} days ago"));
        }
        if dry {
            lines.push(
                "nothing was removed (dry_run); call again with dry_run false to remove it".into(),
            );
        }
        Ok(lines.join("\n"))
    }
}

fn job_by<'a>(jobs: &'a [Job], key: &str) -> Result<&'a Job, String> {
    port::order::resolve(&port::order::jobs(jobs), key, |j| &j.id.0)
        .ok_or_else(|| format!("no job {key}"))
}

fn task_brief(t: &port::Task) -> Value {
    json!({
        "id": t.id, "kind": t.kind, "status": t.status, "label": t.label,
        "agent_type": t.agent_type, "model": t.model, "provider": t.provider,
        "session": t.agent_session, "usage": t.usage, "error": t.error,
    })
}

/// 末尾 `REPLY_TAIL` 个字（按字，不按字节：中文不能切在半个字上）。
fn tail(text: &str) -> String {
    let n = text.chars().count();
    text.chars().skip(n.saturating_sub(REPLY_TAIL)).collect()
}

/// 报告写坏了不影响运行与触发本身：记一条日志就算了。
fn write_json(path: &std::path::Path, v: &Value) {
    let r = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|_| {
            let tmp = path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_vec_pretty(v).unwrap_or_default())?;
            std::fs::rename(&tmp, path)
        });
    if let Err(e) = r {
        tracing::warn!(path = %path.display(), "report not written: {e}");
    }
}

/// `work_list` 的状态筛选：没结束的分"在跑"与"等人"，暂停的算在跑。
fn run_state(s: &port::RunStatus) -> &'static str {
    match s {
        port::RunStatus::Waiting { .. } => "waiting",
        port::RunStatus::Finished | port::RunStatus::Cancelled => "ended",
        _ => "active",
    }
}

fn run_status_text(s: &port::RunStatus) -> String {
    match s {
        port::RunStatus::Running => "running".into(),
        port::RunStatus::Waiting { reason } => format!("waiting: {reason}"),
        port::RunStatus::Pausing => "pausing".into(),
        port::RunStatus::Paused => "paused".into(),
        port::RunStatus::Finished => "finished".into(),
        port::RunStatus::Cancelled => "cancelled".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reply_tail_is_cut_on_characters() {
        let long = "长".repeat(REPLY_TAIL + 10);
        assert_eq!(tail(&long).chars().count(), REPLY_TAIL);
        assert_eq!(tail("短"), "短");
    }
}
