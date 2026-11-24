//! 端口命令的分派（architecture §3.2、§8.4）。

use super::*;

impl Hub {
    pub(super) async fn dispatch(&self, cmd: Command) -> Result<Reply, PortError> {
        use Command as C;
        match cmd {
            C::Hello { .. } => {
                let (instance, flows, jobs) = self.hello();
                Ok(Reply::Hello {
                    instance,
                    flows,
                    jobs,
                })
            }
            C::Heartbeat { .. } | C::Detach | C::StopInstance => Ok(Reply::Ok),
            C::ListSessions { filter } => {
                let st = self.0.st.locked();
                let mut v: Vec<_> = st
                    .sessions
                    .values()
                    .filter(|s| s.parent.is_none())
                    .filter(|s| {
                        filter
                            .project
                            .as_ref()
                            .is_none_or(|p| s.project.as_ref() == Some(p))
                    })
                    .filter_map(|s| st.summary(&s.id))
                    .collect();
                v.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
                Ok(Reply::Sessions(v))
            }
            C::CreateSession {
                scene,
                model,
                permission,
                ..
            } => {
                let settings = SessionSettings {
                    mode: permission.map(state::permission_name),
                    ..SessionSettings::default()
                };
                let id = self
                    .create_session(Origin::User, None, scene, model, json!({}), settings)
                    .await?;
                self.summary(&id).map(Reply::Session)
            }
            C::Watch { session } => self.watch(&session.0).await,
            C::Unwatch { session } => {
                let mut link = self.0.link.locked();
                link.watched.remove(&session.0);
                *link.unwatched.entry(session.0).or_default() += 1;
                Ok(Reply::Ok)
            }
            C::Rename { session, title } => {
                // 名字写在引擎的会话事实里（冷会话也能改）：引擎没改成就不改本地的，免得两边各一个名字。
                self.call(
                    "session.rename",
                    json!({"session_id": session.0, "name": title}),
                )
                .await
                .map_err(rpc_err)?;
                self.retitle(&session.0, &title, Titler::User);
                Ok(Reply::Ok)
            }
            C::Close { session } => {
                if self.0.job_runs.locked().contains_key(&session.0) {
                    self.turn_done(
                        &session.0,
                        false,
                        "session closed".into(),
                        Usage::default(),
                        false,
                    );
                }
                // 没订阅过（冷会话）的退订失败无妨；关闭时冷会话答 SESSION_NOT_FOUND——本来就没有运行实体，
                // 照样算关了。别的错误回给人，本地不动。
                let _ = self
                    .call("session.unsubscribe", json!({"session_id": session.0}))
                    .await;
                match self
                    .call("session.close", json!({"session_id": session.0}))
                    .await
                {
                    Err(e) if e.code() != Some(SESSION_NOT_FOUND) => return Err(rpc_err(e)),
                    _ => {}
                }
                let now = self.now();
                self.0.reg.locked().update(&session.0, |r| {
                    r.closed = true;
                    r.closed_ms = now;
                });
                self.0.st.locked().sessions.remove(&session.0);
                self.0.link.locked().watched.remove(&session.0);
                self.emit_events(vec![Event::SessionRemoved { session }]);
                Ok(Reply::Ok)
            }
            C::ListClosedSessions => Ok(Reply::Sessions(self.closed_sessions().await)),
            C::ReopenSession { session } => {
                let closed = self.closed_sessions().await;
                if !closed.iter().any(|s| s.id == session) {
                    return Err(PortError::Failed(format!(
                        "{session} is not a closed session"
                    )));
                }
                self.0.reg.locked().update(&session.0, |r| {
                    r.closed = false;
                    r.closed_ms = 0;
                });
                self.adopt_closed(&session.0).await;
                self.upsert(&session.0);
                Ok(Reply::Ok)
            }
            C::DeleteSession { session } => {
                if self.0.job_runs.locked().contains_key(&session.0) {
                    self.turn_done(
                        &session.0,
                        false,
                        "session deleted".into(),
                        Usage::default(),
                        false,
                    );
                }
                let _ = self
                    .call("session.unsubscribe", json!({"session_id": session.0}))
                    .await;
                // 删了就没了：转录、附属数据、子 Agent 的侧链都删（Agix 的 session.delete）。
                self.call("session.delete", json!({"session_id": session.0}))
                    .await
                    .map_err(rpc_err)?;
                self.0.reg.locked().remove(&session.0);
                self.0.st.locked().sessions.remove(&session.0);
                self.0.link.locked().watched.remove(&session.0);
                self.emit_events(vec![Event::SessionRemoved { session }]);
                Ok(Reply::Ok)
            }
            C::Submit {
                session,
                text,
                mode,
            } => {
                self.ensure_known(&session.0).await?;
                // 往哪个会话说话就是在看哪个会话：新建会话后 model 先放一个空转录、不等快照就提交。
                self.0.link.locked().watched.insert(session.0.clone());
                let first: String = text.lines().next().unwrap_or("").chars().take(40).collect();
                if !first.trim().is_empty() {
                    self.retitle(&session.0, first.trim(), Titler::FirstText);
                }
                let on_busy = match mode {
                    SubmitMode::Turn => "reject",
                    SubmitMode::Steer => "steer",
                    SubmitMode::Queue => "queue",
                };
                let turn = self.run_turn(&session.0, &text, on_busy);
                Ok(Reply::Submitted {
                    message: port::MessageId(turn),
                })
            }
            C::Interrupt { session } => {
                self.call("session.interrupt", json!({"session_id": session.0}))
                    .await
                    .map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::Answer { prompt, answer, .. } => {
                let meta = self.0.st.locked().prompts.get(&prompt.0).cloned();
                // 不认识的提问多半已经被答掉或撤了：和引擎一样静默成功。
                if let Some(meta) = meta {
                    self.call(
                        "session.respondToPrompt",
                        state::answer_params(&meta, &prompt.0, &answer),
                    )
                    .await
                    .map_err(rpc_err)?;
                }
                Ok(Reply::Ok)
            }
            C::TrustProject { trusted } => {
                self.call(
                    "project.trust",
                    json!({"project_root": self.0.project, "trusted": trusted}),
                )
                .await
                .map_err(rpc_err)?;
                let choice = client::ProjectState::new(&self.0.project).trust_choice();
                let written = choice
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|_| {
                        std::fs::write(
                            &choice,
                            json!({"trusted": trusted, "at_ms": self.now()}).to_string(),
                        )
                    });
                if let Err(e) = written {
                    tracing::warn!(path = %choice.display(), "trust choice not recorded: {e}");
                }
                if let Some(f) = self.0.flows.get() {
                    f.trust_changed().await;
                }
                if let Ok(sched) = self.scheduler() {
                    let gone = sched.set_trusted(trusted);
                    let mut events: Vec<Event> = gone
                        .into_iter()
                        .map(|job| Event::JobRemoved { job })
                        .collect();
                    events.extend(sched.list().into_iter().map(Event::JobUpserted));
                    self.emit_events(events);
                }
                Ok(Reply::Ok)
            }
            C::SetPermissionMode { session, mode } => {
                self.ensure_known(&session.0).await?;
                let r = self
                    .call(
                        "session.setPermissionMode",
                        json!({"session_id": session.0, "mode": state::permission_name(mode)}),
                    )
                    .await
                    .map_err(rpc_err)?;
                let got = r["mode"]
                    .as_str()
                    .and_then(state::permission_mode)
                    .unwrap_or(mode);
                let changed = {
                    let mut st = self.0.st.locked();
                    match st.sessions.get_mut(&session.0) {
                        Some(s) if s.permission != got => {
                            s.permission = got;
                            true
                        }
                        _ => false,
                    }
                };
                if changed {
                    self.upsert(&session.0);
                }
                Ok(Reply::PermissionMode {
                    mode: got,
                    clamped: r["clamped"].as_bool().unwrap_or(false),
                })
            }
            C::ListModels => Ok(Reply::Models(self.list_models().await)),
            C::ProjectFiles => Ok(Reply::Files(self.project_files().await)),
            C::Checkpoints { session } => {
                let v = self
                    .call("session.checkpoints", json!({"session_id": session.0}))
                    .await
                    .map_err(rpc_err)?;
                let turns = v["turns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|t| port::Checkpoint {
                        turn_id: str_field(t, "turn_id"),
                        prompt: str_field(t, "prompt"),
                        files: t["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|f| f["path"].as_str().map(str::to_string))
                            .collect(),
                        restorable: t["restorable"].as_bool().unwrap_or(false),
                    })
                    .collect();
                Ok(Reply::Checkpoints(turns))
            }
            C::Rewind {
                session,
                to_turn,
                scope,
            } => {
                let mut p = json!({
                    "session_id": session.0,
                    "scope": match scope {
                        port::RewindScope::Both => "both",
                        port::RewindScope::Conversation => "conversation",
                        port::RewindScope::Files => "files",
                    },
                });
                match to_turn {
                    Some(t) => p["to_turn"] = json!(t),
                    None => p["to_start"] = json!(true),
                }
                // 回不去的文件（被会话之外改过、副本没了）整个拒绝，问题原样回给人：
                // 不替人决定覆盖或跳过。
                let v = self.call("session.rewind", p).await.map_err(|e| match e {
                    RpcError::Remote { message, data, .. } => {
                        PortError::Failed(match data["issues"].as_array() {
                            Some(issues) if !issues.is_empty() => format!(
                                "{message}: {}",
                                issues
                                    .iter()
                                    .map(|i| format!(
                                        "{} ({})",
                                        i["path"].as_str().unwrap_or(""),
                                        i["issue"].as_str().unwrap_or("")
                                    ))
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ),
                            _ => message,
                        })
                    }
                    e => rpc_err(e),
                })?;
                Ok(Reply::Rewound {
                    removed_turns: v["conversation"]["removed_turns"]
                        .as_array()
                        .map_or(0, |a| a.len() as u32),
                    restored_files: v["files"]["restored"]
                        .as_array()
                        .map_or(0, |a| a.len() as u32),
                })
            }
            C::SetModel {
                session,
                model,
                provider,
            } => {
                let mut p = json!({"session_id": session.0, "model": model});
                if let Some(pv) = provider {
                    p["provider"] = json!(pv);
                }
                self.call("session.setModel", p).await.map_err(rpc_err)?;
                self.0
                    .reg
                    .locked()
                    .update(&session.0, |r| r.model = Some(model.clone()));
                if let Some(s) = self.0.st.locked().sessions.get_mut(&session.0) {
                    s.model = model;
                }
                self.upsert(&session.0);
                Ok(Reply::Ok)
            }
            C::Commands { session } => {
                let v = match self
                    .call("commands.list", json!({"session_id": session.0}))
                    .await
                {
                    Ok(v) => v,
                    Err(_) => self
                        .call("commands.list", json!({}))
                        .await
                        .map_err(rpc_err)?,
                };
                let cmds = v["commands"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| SlashCommand {
                        name: str_field(c, "name"),
                        description: str_field(c, "description"),
                        source: str_field(c, "source"),
                    })
                    .collect();
                Ok(Reply::Commands(cmds))
            }
            C::SideQuestion { session, text } => {
                let r = self
                    .call("session.fork", json!({"session_id": session.0}))
                    .await
                    .map_err(rpc_err)?;
                let id = str_field(&r, "session_id");
                let (from_title, base) = {
                    let st = self.0.st.locked();
                    let s = st.sessions.get(&session.0).cloned().unwrap_or_default();
                    (s.title.clone(), s)
                };
                let title: String = text.chars().take(40).collect();
                let origin = Origin::Fork {
                    from: session.clone(),
                    from_title,
                };
                let now = self.now();
                self.0.reg.locked().update(&id, |r| {
                    r.origin = origin.clone();
                    r.title = Some(title.clone());
                    r.updated_ms = now;
                    r.created_ms = now;
                });
                self.0.st.locked().sessions.insert(
                    id.clone(),
                    Sess {
                        id: id.clone(),
                        title,
                        project: base.project,
                        scene: base.scene,
                        model: base.model,
                        origin,
                        updated_ms: now,
                        created_ms: now,
                        ..Sess::default()
                    },
                );
                self.upsert(&id);
                self.0.side.locked().insert(id.clone());
                self.run_turn(&id, &text, "queue");
                Ok(Reply::Ok)
            }
            C::SetGoal {
                session,
                objective,
                max_rounds,
            } => {
                // goal 的每一轮都是引擎自己开的，没有"第一句话"：还没名字就用目标当名字。
                let first: String = objective.chars().take(40).collect();
                self.retitle(&session.0, first.trim(), Titler::FirstText);
                let mut p = json!({"session_id": session.0, "objective": objective});
                if let Some(m) = max_rounds {
                    p["max_rounds"] = json!(m);
                }
                self.call("session.setGoal", p).await.map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::UpdateGoal { session, action } => {
                let p = match action {
                    GoalAction::Pause => json!({"session_id": session.0, "action": "pause"}),
                    GoalAction::Resume => json!({"session_id": session.0, "action": "resume"}),
                    GoalAction::Clear => json!({"session_id": session.0, "action": "clear"}),
                    GoalAction::Edit {
                        objective,
                        max_rounds,
                    } => {
                        let mut p = json!({"session_id": session.0, "action": "edit",
                                           "objective": objective});
                        if let Some(m) = max_rounds {
                            p["max_rounds"] = json!(m);
                        }
                        p
                    }
                };
                self.call("session.updateGoal", p).await.map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::StopTask { task } => {
                let owner = self.task_owner(&task)?;
                self.call(
                    "session.stopTask",
                    json!({"session_id": owner, "task_id": task.0}),
                )
                .await
                .map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::MessageTask { task, text } => {
                let owner = self.task_owner(&task)?;
                self.call(
                    "session.messageTask",
                    json!({"session_id": owner, "task_id": task.0, "message": text}),
                )
                .await
                .map_err(rpc_err)?;
                Ok(Reply::Ok)
            }
            C::SessionTasks { session } => {
                let st = self.0.st.locked();
                let mut out = vec![];
                let mut owners = vec![session.0.clone()];
                // 子 Agent 的任务挂在它的侧链会话上：顺着 agent_session 往下收，环与过深的不跟。
                while let Some(o) = owners.pop() {
                    if out.len() > 2000 {
                        break;
                    }
                    let Some(s) = st.sessions.get(&o) else {
                        continue;
                    };
                    for t in s.tasks.values() {
                        if let Some(child) = &t.agent_session
                            && child.0 != o
                        {
                            owners.push(child.0.clone());
                        }
                        out.push(t.clone());
                    }
                }
                Ok(Reply::Tasks(out))
            }
            C::TaskOutput { task, from_byte } => {
                let owner = self.task_owner(&task)?;
                let v = self
                    .call(
                        "session.taskOutput",
                        json!({"session_id": owner, "task_id": task.0, "from_byte": from_byte}),
                    )
                    .await
                    .map_err(rpc_err)?;
                Ok(Reply::TaskOutput {
                    text: str_field(&v, "output"),
                    next_byte: v["next_byte"].as_u64().unwrap_or(from_byte),
                    finished: v["final"].as_bool().unwrap_or(false),
                })
            }
            cmd @ (C::ListFlows
            | C::ListRuns { .. }
            | C::ListEndedRuns { .. }
            | C::GetFlow { .. }
            | C::GetRun { .. }
            | C::StartRun { .. }
            | C::RunOp { .. }) => match self.flows() {
                Some(f) => f.command(cmd).await,
                None => match cmd {
                    C::ListFlows => Ok(Reply::Flows(vec![])),
                    C::ListRuns { .. } | C::ListEndedRuns { .. } => Ok(Reply::Runs(vec![])),
                    // 列表回空、由 Hello 的 flows 能力位让界面说明；具体原因在接入时提示过、也在 host.log。
                    _ => Err(PortError::Unsupported(
                        "flows are not available in this instance (see host.log)".into(),
                    )),
                },
            },
            C::ListJobs => Ok(Reply::Jobs(self.scheduler()?.list())),
            C::SaveJob { job } => {
                let job = self.normalize_job(job).await.map_err(PortError::Failed)?;
                let saved = self.scheduler()?.upsert(job).map_err(PortError::Failed)?;
                self.emit_events(vec![Event::JobUpserted(saved.clone())]);
                Ok(Reply::Job(saved))
            }
            C::DeleteJob { job } => {
                self.scheduler()?
                    .delete(&job.0)
                    .map_err(PortError::Failed)?;
                self.emit_events(vec![Event::JobRemoved { job }]);
                Ok(Reply::Ok)
            }
            C::SetJobEnabled { job, enabled } => {
                let sched = self.scheduler()?;
                sched
                    .set_enabled(&job.0, enabled)
                    .map_err(PortError::Failed)?;
                if let Some(j) = sched.get(&job.0) {
                    self.emit_events(vec![Event::JobUpserted(j)]);
                }
                Ok(Reply::Ok)
            }
            C::RunJobNow {
                job,
                subject,
                prompt_append,
            } => {
                self.scheduler()?
                    .run_now_with(&job.0, |j| match &mut j.kind {
                        JobKind::Simple { prompt, .. } => {
                            if let Some(more) = prompt_append.filter(|s| !s.trim().is_empty()) {
                                prompt.push_str("\n\n");
                                prompt.push_str(&more);
                            }
                        }
                        JobKind::Flow { subject: s, .. } => {
                            if let Some(x) = subject.filter(|s| !s.trim().is_empty()) {
                                *s = x;
                            }
                        }
                    })
                    .map_err(PortError::Failed)?;
                Ok(Reply::Ok)
            }
            C::JobHistory {
                job,
                before_ms,
                limit,
                outcome,
            } => {
                let sched = self.scheduler()?;
                let store = sched.store();
                let page = store.locked().history_page(
                    &job.0,
                    before_ms,
                    limit.unwrap_or(20) as usize,
                    outcome,
                );
                Ok(Reply::JobHistory(page))
            }
            C::TestNotify => Ok(Reply::Notified(self.test_notify().await)),
            C::Doctor => Ok(Reply::Doctor(self.doctor().await)),
            cmd @ (C::ListPlugins
            | C::InstallPlugin { .. }
            | C::SetPluginEnabled { .. }
            | C::UninstallPlugin { .. }
            | C::PluginDisclosure { .. }) => self.plugin_command(cmd).await,
            C::ValidateJob { job } => Ok(Reply::JobCheck(self.check_job(&job).await)),
            C::StopFiring { job, fired_ms } => {
                self.stop_firing(&job.0, fired_ms, port::StopCause::Manual)
                    .await
                    .map_err(PortError::Failed)?;
                Ok(Reply::Ok)
            }
        }
    }

    pub(super) fn task_owner(&self, task: &TaskId) -> Result<String, PortError> {
        self.0
            .st
            .locked()
            .task_owner
            .get(&task.0)
            .cloned()
            .ok_or_else(|| PortError::Failed(format!("no task {task}")))
    }
}
