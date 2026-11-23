//! 宿主工具的执行：会话与作业的那几个，作用域由发起调用的会话决定（architecture §8.5）。

use super::*;

impl Hub {
    /// 宿主工具（tools.rs）：先按发起调用的会话定作用域，再经和端口命令同一条路执行，
    /// 事件照常推给 TUI。
    /// 宿主工具的同一个入口，给不经引擎帧的调用方用（测试；以后的界面）。
    pub async fn run_tool(
        &self,
        session: &str,
        name: &str,
        input: &Value,
    ) -> Result<String, String> {
        self.host_tool(session, name, input).await
    }

    pub(super) async fn host_tool(
        &self,
        rpc: &str,
        name: &str,
        input: &Value,
    ) -> Result<String, String> {
        let scope = {
            let st = self.0.st.locked();
            tools::scope_of(st.sessions.get(rpc).map(|s| &s.origin))
        };
        if scope == tools::Scope::None {
            return Err("this session is not one of this Codix instance's sessions".into());
        }
        if let Some(f) = self.flows()
            && let Some(r) = f.tool(rpc, &scope, name, input).await
        {
            return r;
        }
        let port_err = |e: PortError| e.to_string();
        if tools::CONFIG_TOOLS.contains(&name) {
            // 改的是这台机器上整个 Codix 的配置：作业与流程里的会话不给。
            if scope != tools::Scope::Full {
                return Err("configuring Codix is only for a person's own sessions".into());
            }
            return self.config_tool(name, input).await;
        }
        match name {
            tools::SESSION_LIST => {
                let all = input.get("all").and_then(Value::as_bool).unwrap_or(false);
                let sums = self.top_sessions();
                let mut lines: Vec<String> = port::order::sessions(&sums)
                    .iter()
                    .enumerate()
                    .map(|(i, s)| session_line(Some(i + 1), s))
                    .collect();
                if all {
                    lines.extend(
                        sums.iter()
                            .filter(|s| !port::order::is_user_session(s))
                            .map(|s| session_line(None, s)),
                    );
                    lines.extend(
                        self.closed_sessions()
                            .await
                            .iter()
                            .map(|s| format!("{}  closed", session_line(None, s))),
                    );
                }
                Ok(if lines.is_empty() {
                    "no sessions".into()
                } else {
                    lines.join("\n")
                })
            }
            tools::SESSION_SHOW => {
                let id = self.session_target(input)?;
                let st = self.0.st.locked();
                let (Some(sum), Some(s)) = (st.summary(&id), st.sessions.get(&id)) else {
                    return Err(format!("no session {id}"));
                };
                Ok(serde_json::to_string_pretty(&json!({
                    "id": sum.id, "title": sum.title, "state": sum.state, "origin": sum.provenance.origin,
                    "scene": sum.provenance.scene, "model": sum.provenance.model, "usage": sum.usage,
                    "goal": s.goal, "todos": s.todos, "tasks": s.tasks.values().collect::<Vec<_>>(),
                }))
                .unwrap_or_default())
            }
            tools::SESSION_RENAME => {
                let title = tools::str_of(input, "title")
                    .filter(|t| !t.trim().is_empty())
                    .ok_or("title must not be empty")?;
                let id = match input.get("target") {
                    Some(_) => self.session_target(input)?,
                    None => rpc.to_string(),
                };
                self.may_touch_session(&scope, rpc, &id)?;
                self.dispatch(Command::Rename {
                    session: SessionId(id.clone()),
                    title: title.clone(),
                })
                .await
                .map_err(port_err)?;
                Ok(format!("renamed {id} to \"{title}\""))
            }
            tools::SESSION_CLOSE => {
                let id = self.session_target(input)?;
                self.may_touch_session(&scope, rpc, &id)?;
                let title = self.summary(&id).map(|s| s.title).unwrap_or_default();
                self.dispatch(Command::Close {
                    session: SessionId(id.clone()),
                })
                .await
                .map_err(port_err)?;
                Ok(format!("closed \"{title}\" ({id})"))
            }
            tools::SESSION_DELETE => {
                if scope != tools::Scope::Full {
                    return Err(out_of_scope(&scope));
                }
                let key = tools::str_of(input, "target").ok_or("target is required")?;
                let closed = self.closed_sessions().await;
                let Some(s) = closed.iter().find(|s| s.id.0 == key.trim()) else {
                    return Err(format!(
                        "{key} is not a closed session: close it first (session_close), then delete it"
                    ));
                };
                self.dispatch(Command::DeleteSession {
                    session: s.id.clone(),
                })
                .await
                .map_err(port_err)?;
                Ok(format!("deleted \"{}\" ({})", s.title, s.id))
            }
            tools::JOB_LIST => {
                let sched = self.scheduler().map_err(port_err)?;
                let jobs = sched.list();
                let lines: Vec<String> = port::order::jobs(&jobs)
                    .iter()
                    .enumerate()
                    .map(|(i, j)| job_line(i + 1, j))
                    .collect();
                Ok(if lines.is_empty() {
                    "no jobs".into()
                } else {
                    lines.join("\n")
                })
            }
            tools::JOB_SHOW => {
                let sched = self.scheduler().map_err(port_err)?;
                let job = job_target(&sched.list(), input)?;
                let (guard, capped) = self.guard_config().effective(&job.guard);
                Ok(serde_json::to_string_pretty(&json!({
                    "job": job,
                    "guardrails_in_effect": guard,
                    "capped_by_this_machine": capped,
                    "recent_firings": sched.history(&job.id.0, 10),
                }))
                .unwrap_or_default())
            }
            tools::JOB_VALIDATE => {
                let sched = self.scheduler().map_err(port_err)?;
                let existing = match input.get("target") {
                    Some(_) => Some(job_target(&sched.list(), input)?),
                    None => None,
                };
                let job = tools::job_from_input(input, existing)?;
                let check = self.check_job(&job).await;
                let next: Vec<String> = check.next_fire_ms.iter().map(|t| local_time(*t)).collect();
                Ok(serde_json::to_string_pretty(&json!({
                    "ok": check.problems.is_empty(),
                    "problems": check.problems,
                    "next_firings": next,
                    "guardrails_in_effect": check.guard,
                    "capped_by_this_machine": check.capped,
                }))
                .unwrap_or_default())
            }
            tools::JOB_HISTORY => {
                let sched = self.scheduler().map_err(port_err)?;
                let job = job_target(&sched.list(), input)?;
                let outcome = match tools::str_of(input, "outcome") {
                    Some(o) => Some(
                        serde_json::from_value::<port::OutcomeKind>(json!(o))
                            .map_err(|_| format!("unknown outcome {o}"))?,
                    ),
                    None => None,
                };
                let Reply::JobHistory(page) = self
                    .dispatch(Command::JobHistory {
                        job: job.id.clone(),
                        before_ms: input.get("before_ms").and_then(Value::as_u64),
                        limit: input
                            .get("limit")
                            .and_then(Value::as_u64)
                            .map(|n| n.min(200) as u32),
                        outcome,
                    })
                    .await
                    .map_err(port_err)?
                else {
                    return Err("no history".into());
                };
                let lines: Vec<String> = page.iter().map(firing_line).collect();
                Ok(if lines.is_empty() {
                    format!("no firings of \"{}\"", job.name)
                } else {
                    lines.join("\n")
                })
            }
            tools::JOB_STOP => {
                let sched = self.scheduler().map_err(port_err)?;
                let job = job_target(&sched.list(), input)?;
                match &scope {
                    tools::Scope::Full => {}
                    tools::Scope::Job(mine) if mine == &job.id.0 => {}
                    _ => return Err(out_of_scope(&scope)),
                }
                self.stop_firing(
                    &job.id.0,
                    input.get("fired_ms").and_then(Value::as_u64),
                    port::StopCause::Manual,
                )
                .await?;
                Ok(format!(
                    "stopped the running firing of \"{}\" ({})",
                    job.name, job.id
                ))
            }
            tools::JOB_SAVE => {
                let sched = self.scheduler().map_err(port_err)?;
                let existing = match input.get("target") {
                    Some(_) => Some(job_target(&sched.list(), input)?),
                    None => None,
                };
                match (&scope, &existing) {
                    (tools::Scope::Full, _) => {}
                    (tools::Scope::Job(mine), Some(j)) if &j.id.0 == mine => {}
                    _ => return Err(out_of_scope(&scope)),
                }
                let job = tools::job_from_input(input, existing)?;
                let Reply::Job(saved) = self
                    .dispatch(Command::SaveJob { job })
                    .await
                    .map_err(port_err)?
                else {
                    return Err("the job was not saved".into());
                };
                Ok(format!(
                    "saved \"{}\" ({})\n{}",
                    saved.name,
                    saved.id,
                    serde_json::to_string_pretty(&saved).unwrap_or_default()
                ))
            }
            tools::JOB_DELETE | tools::JOB_ENABLE | tools::JOB_RUN => {
                let sched = self.scheduler().map_err(port_err)?;
                let job = job_target(&sched.list(), input)?;
                match &scope {
                    tools::Scope::Full => {}
                    tools::Scope::Job(mine) if mine == &job.id.0 => {}
                    _ => return Err(out_of_scope(&scope)),
                }
                let (cmd, done) = match name {
                    tools::JOB_DELETE => (
                        Command::DeleteJob {
                            job: job.id.clone(),
                        },
                        "deleted",
                    ),
                    tools::JOB_ENABLE => {
                        let enabled = input
                            .get("enabled")
                            .and_then(Value::as_bool)
                            .ok_or("enabled is required")?;
                        (
                            Command::SetJobEnabled {
                                job: job.id.clone(),
                                enabled,
                            },
                            if enabled { "enabled" } else { "disabled" },
                        )
                    }
                    _ => (
                        Command::RunJobNow {
                            job: job.id.clone(),
                            subject: tools::str_of(input, "subject"),
                            prompt_append: tools::str_of(input, "prompt_append"),
                        },
                        "fired",
                    ),
                };
                self.dispatch(cmd).await.map_err(port_err)?;
                Ok(format!("{done} \"{}\" ({})", job.name, job.id))
            }
            other => Err(format!("unknown tool {other}")),
        }
    }

    /// 会话标签与工具看的那些：顶层的（子 Agent 的侧链不算）。
    pub(super) fn top_sessions(&self) -> Vec<port::SessionSummary> {
        let st = self.0.st.locked();
        st.sessions
            .values()
            .filter(|s| s.parent.is_none())
            .filter_map(|s| st.summary(&s.id))
            .collect()
    }

    /// `target` 指的会话：`#n` 按会话列表的顺序，否则按 id（作业、流程开出来的会话也行）。
    pub(super) fn session_target(&self, input: &Value) -> Result<String, String> {
        let key = tools::str_of(input, "target").ok_or("target is required")?;
        let sums = self.top_sessions();
        if let Some(s) = port::order::resolve(&port::order::sessions(&sums), &key, |s| &s.id.0) {
            return Ok(s.id.0.clone());
        }
        let k = key.trim();
        if self.0.st.locked().sessions.contains_key(k) {
            return Ok(k.to_string());
        }
        Err(format!("no session {key}"))
    }

    pub(super) fn may_touch_session(
        &self,
        scope: &tools::Scope,
        rpc: &str,
        id: &str,
    ) -> Result<(), String> {
        if *scope == tools::Scope::Full || id == rpc {
            Ok(())
        } else {
            Err(out_of_scope(scope))
        }
    }
}

/// 摘要类全推；一轮的细节只推 Watch 着的会话（architecture §8.4）。
pub(super) fn out_of_scope(scope: &tools::Scope) -> String {
    match scope {
        tools::Scope::Job(job) => format!(
            "not allowed: a session started by a scheduled job may change only that job ({job}) and itself"
        ),
        tools::Scope::Run(run) => format!(
            "not allowed: a session inside a flow run may change only that run ({run}) and itself"
        ),
        _ => "not allowed from this session".into(),
    }
}

pub(super) fn local_time(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

pub(super) fn session_line(n: Option<usize>, s: &port::SessionSummary) -> String {
    let no = n.map(|n| format!("#{n}  ")).unwrap_or_default();
    let origin = match &s.provenance.origin {
        Origin::User => String::new(),
        Origin::Fork { from_title, .. } => format!("  forked from \"{from_title}\""),
        Origin::Job { name, .. } => format!("  started by job \"{name}\""),
        Origin::FlowStation {
            flow_title,
            subject,
            ..
        } => {
            format!("  in flow run \"{flow_title}\" / {subject}")
        }
        Origin::SubAgent { parent_title, .. } => format!("  sub-agent of \"{parent_title}\""),
    };
    format!("{no}{}  \"{}\"  {:?}{origin}", s.id, s.title, s.state)
}

pub(super) fn job_line(n: usize, j: &Job) -> String {
    let kind = match &j.kind {
        port::JobKind::Simple { .. } => "simple".to_string(),
        port::JobKind::Flow { flow, .. } => format!("flow {flow}"),
    };
    let state = match (&j.auto_disabled, j.enabled) {
        (Some(a), _) => format!("disabled after {} failures in a row", a.after),
        (None, true) => "enabled".into(),
        (None, false) => "disabled".into(),
    };
    let next = j
        .next_fire_ms
        .filter(|_| j.enabled)
        .map(|ms| format!("  next {}", local_time(ms)))
        .unwrap_or_default();
    let running = j
        .running_ms
        .map(|ms| format!("  running since {}", local_time(ms)))
        .unwrap_or_default();
    let last = j
        .last
        .as_ref()
        .map(|o| format!("  last {}", outcome_text(o)))
        .unwrap_or_default();
    format!(
        "#{n}  {}  \"{}\"  {kind}  {}  {state}{next}{running}{last}",
        j.id, j.name, j.cron
    )
}

fn outcome_text(o: &port::JobOutcome) -> String {
    use port::JobOutcome as O;
    match o {
        O::Running => "running".into(),
        O::Done => "done".into(),
        O::Failed { message } => format!("failed: {message}"),
        O::Stopped { reason } => format!("stopped: {}", json!(reason).as_str().unwrap_or("")),
        O::Skipped { reason } => format!("skipped: {}", json!(reason).as_str().unwrap_or("")),
    }
}

fn firing_line(e: &port::JobEntry) -> String {
    let target = match &e.target {
        Some(port::JobTarget::Session { id, title }) => format!("  → session \"{title}\" ({id})"),
        Some(port::JobTarget::Run { id, subject }) => format!("  → run \"{subject}\" ({id})"),
        None => String::new(),
    };
    let audit = e
        .audit
        .as_ref()
        .map(|a| {
            let tools: Vec<String> = a.tools.iter().map(|(k, v)| format!("{k}×{v}")).collect();
            let mut t = format!("  tools [{}]", tools.join(", "));
            if a.errors > 0 {
                t.push_str(&format!("  {} errors", a.errors));
            }
            if !a.files.is_empty() {
                t.push_str(&format!("  changed [{}]", a.files.join(", ")));
            }
            t
        })
        .unwrap_or_default();
    format!(
        "{} (fired_ms {})  {}  {}s  {} tokens{target}{audit}",
        local_time(e.fired_ms),
        e.fired_ms,
        outcome_text(&e.outcome),
        e.elapsed_ms / 1000,
        e.usage.billed()
    )
}

/// `target` 指的作业：`#n` 按作业列表的顺序，否则按 id。
pub(super) fn job_target(jobs: &[Job], input: &Value) -> Result<Job, String> {
    let key = tools::str_of(input, "target").ok_or("target is required")?;
    port::order::resolve(&port::order::jobs(jobs), &key, |j| &j.id.0)
        .cloned()
        .ok_or_else(|| format!("no job {key}"))
}
