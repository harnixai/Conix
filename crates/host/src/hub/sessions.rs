//! 会话：从引擎认领、订阅、建、Watch 的快照、改名；流程派工器经它开的会话（architecture §8.4）。

use super::*;

impl Hub {
    /// 这个会话此刻在不在跑一轮、在哪个项目里；不认识就是 `None`。
    pub(crate) fn origin_of(&self, id: &str) -> Option<Origin> {
        let st = self.0.st.locked();
        st.sessions.get(id).map(|s| s.origin.clone())
    }

    pub(crate) fn session_state(&self, id: &str) -> Option<(bool, Option<String>)> {
        let st = self.0.st.locked();
        st.sessions.get(id).map(|s| (s.running, s.project.clone()))
    }

    /// 会话在不在：内存里没有就问引擎，在盘上就带回来、订阅上。
    /// 在 Conix 里关掉的不算在：引擎的存储里还有它，但它不该被谁再带回来。
    pub(crate) async fn bring_back(&self, id: &str) -> Result<bool, RpcError> {
        if self.0.st.locked().sessions.contains_key(id) {
            return Ok(true);
        }
        if self.0.reg.locked().get(id).is_some_and(|r| r.closed) {
            return Ok(false);
        }
        match self.call("session.get", json!({"session_id": id})).await {
            Ok(_) => {}
            Err(RpcError::Remote { code, .. }) if code == SESSION_NOT_FOUND => return Ok(false),
            Err(e) => return Err(e),
        }
        self.adopt(id).await;
        let _ = self.subscribe(id, true).await;
        Ok(self.0.st.locked().sessions.contains_key(id))
    }

    /// 流程派工器开的会话：`session_id` 由键算出，同一个键总是同一个会话（跨重启也是）。
    /// 回 `(会话, 原来就有)`。
    pub(crate) async fn open_flow_session(
        &self,
        session_id: &str,
        scene: &str,
        project: Option<&str>,
        options: Value,
        origin: Origin,
        title: Option<String>,
    ) -> Result<(String, bool), RpcError> {
        let mut params = json!({"session_id": session_id, "scene": scene, "options": options});
        if let Some(p) = project {
            params["project_root"] = json!(p);
        }
        let r = self.call("session.create", params).await?;
        let id = str_field(&r, "session_id");
        for (field, requested, applied) in session_opts::clamped(&r) {
            tracing::warn!(session = %id, %field, %requested, %applied, "flow session option clamped by settings");
        }
        let existed = r["existing"].as_bool().unwrap_or(false);
        let now = self.now();
        let known = self.0.st.locked().sessions.contains_key(&id);
        if !existed || !known {
            self.0.reg.locked().update(&id, |rec| {
                rec.origin = origin.clone();
                if rec.title.is_none() {
                    rec.title = title.clone();
                }
                if rec.created_ms == 0 {
                    rec.created_ms = now;
                }
                rec.updated_ms = now;
                rec.closed = false;
                if let Some(m) = options["model"].as_str() {
                    rec.model = Some(m.to_string());
                }
            });
        }
        if !known {
            let created = parse_rfc3339_ms(r["created_at"].as_str().unwrap_or("")).unwrap_or(now);
            let title = self
                .0
                .reg
                .locked()
                .get(&id)
                .and_then(|r| r.title.clone())
                .or(title);
            let sess = Sess {
                id: id.clone(),
                title: title.unwrap_or_else(|| state::short(&id)),
                project: project.map(str::to_string),
                scene: r["scene"].as_str().unwrap_or(scene).to_string(),
                // 站可以指定模型（流程的 `executor.model`）：记它，不记实例的缺省，界面与报告才写得对。
                model: options["model"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| self.0.default_model.locked().clone()),
                origin,
                live: true,
                updated_ms: now,
                created_ms: created,
                ..Sess::default()
            };
            self.0.st.locked().sessions.insert(id.clone(), sess);
        }
        let _ = self.subscribe(&id, existed).await;
        self.upsert(&id);
        Ok((id, existed))
    }

    pub(crate) async fn interrupt(&self, id: &str) -> Result<(), RpcError> {
        self.call("session.interrupt", json!({"session_id": id}))
            .await
            .map(|_| ())
    }

    /// 派工器关一个会话：看着它的流结束。
    pub(crate) async fn close_session(&self, id: &str) -> Result<(), PortError> {
        self.dispatch(Command::Close {
            session: SessionId(id.to_string()),
        })
        .await?;
        self.0.taps.locked().remove(id);
        Ok(())
    }

    /// 派工器答一个权限问句。
    pub(crate) async fn answer_permission(
        &self,
        id: &str,
        prompt: &str,
        allow: bool,
        reason: Option<String>,
    ) -> Result<(), PortError> {
        self.dispatch(Command::Answer {
            session: SessionId(id.to_string()),
            prompt: port::PromptId(prompt.to_string()),
            answer: if allow {
                port::Answer::Permit
            } else {
                port::Answer::Deny { reason }
            },
        })
        .await
        .map(|_| ())
    }

    pub(crate) async fn append_entry(
        &self,
        id: &str,
        event: &str,
        payload: Value,
    ) -> Result<(), RpcError> {
        self.call(
            "session.appendEntry",
            json!({"session_id": id, "ns": "conix.flow", "event": event, "payload": payload}),
        )
        .await
        .map(|_| ())
    }

    pub(crate) async fn project_trusted(&self, root: &str) -> Result<bool, RpcError> {
        let v = self
            .call("project.status", json!({"project_root": root}))
            .await?;
        Ok(v["trust"].as_str() == Some("trusted"))
    }

    pub(super) fn sess_from_info(&self, info: &SessionInfo) -> Sess {
        let reg = self.0.reg.locked();
        let rec = reg.get(&info.session_id).cloned().unwrap_or_default();
        let model = rec
            .model
            .clone()
            .unwrap_or_else(|| self.0.default_model.locked().clone());
        Sess {
            id: info.session_id.clone(),
            title: rec
                .title
                .clone()
                .or_else(|| info.name.clone())
                .unwrap_or_else(|| state::short(&info.session_id)),
            project: info.project_root.clone(),
            scene: info.scene.clone().unwrap_or_else(|| "code".into()),
            model,
            origin: rec.origin.clone(),
            parent: info.parent_session_id.clone(),
            usage: info
                .usage
                .as_ref()
                .map(state::map_usage)
                .unwrap_or_default(),
            live: info.status == "active",
            running: info.turn_state.as_deref() == Some("running"),
            updated_ms: parse_rfc3339_ms(&info.last_active)
                .or_else(|| parse_rfc3339_ms(&info.created_at))
                .unwrap_or(rec.updated_ms),
            created_ms: Some(rec.created_ms)
                .filter(|ms| *ms > 0)
                .or_else(|| parse_rfc3339_ms(&info.created_at))
                .unwrap_or(rec.updated_ms),
            ..Sess::default()
        }
    }

    pub(super) async fn load_sessions(&self) -> Result<(), RpcError> {
        let v = self.call("session.list", json!({})).await?;
        let infos: Vec<SessionInfo> =
            serde_json::from_value(v["sessions"].clone()).unwrap_or_default();
        let mut live = vec![];
        for info in infos
            .iter()
            .filter(|i| i.project_root.as_deref() == Some(self.0.project.as_str()))
        {
            if self
                .0
                .reg
                .locked()
                .get(&info.session_id)
                .is_some_and(|r| r.closed)
            {
                continue;
            }
            let s = self.sess_from_info(info);
            if s.live {
                live.push(s.id.clone());
            }
            self.0.st.locked().sessions.insert(s.id.clone(), s);
        }
        for id in live {
            let _ = self.subscribe(&id, false).await;
        }
        Ok(())
    }

    /// 不是这个实例建的会话出现在内存里（比如别的连接、或恢复）时，补一份摘要。
    pub(super) async fn adopt(&self, id: &str) {
        let Ok(v) = self.call("session.get", json!({"session_id": id})).await else {
            return;
        };
        if let Ok(info) = serde_json::from_value::<SessionInfo>(v) {
            let s = self.sess_from_info(&info);
            // 和建会话赛跑时别把那边已有的状态（悬着的提问等）盖掉。
            self.0
                .st
                .locked()
                .sessions
                .entry(id.to_string())
                .or_insert(s);
            self.upsert(id);
        }
    }

    /// 订阅一个会话；订阅时还悬着的提问照原样补进来。
    pub(super) async fn subscribe(&self, id: &str, resume: bool) -> Result<(), RpcError> {
        let v = self
            .call(
                "session.subscribe",
                json!({"session_id": id, "resume": resume}),
            )
            .await?;
        let now = self.now();
        self.apply(|st, out| {
            if let Some(s) = st.sessions.get_mut(id) {
                s.live = true;
            }
            for p in v["pending_prompts"].as_array().into_iter().flatten() {
                st.on_event(id, id, SessionEvent::parse(p), now, out);
            }
        });
        self.refresh_permission(id).await;
        Ok(())
    }

    /// 会话此刻的权限模式：订阅之后它一定在内存里，`session.resume` 只报状态（`already_active`）。
    /// 订阅之前切的、设置夹过的、重建后按设置得到的，帧都不报，只能这样读。
    pub(super) async fn refresh_permission(&self, id: &str) {
        let Ok(r) = self.call("session.resume", json!({"session_id": id})).await else {
            return;
        };
        let Some(m) = r["permission"]["mode"]
            .as_str()
            .and_then(state::permission_mode)
        else {
            return;
        };
        let changed = {
            let mut st = self.0.st.locked();
            match st.sessions.get_mut(id) {
                Some(s) if s.permission != m => {
                    s.permission = m;
                    true
                }
                _ => false,
            }
        };
        if changed {
            self.upsert(id);
        }
    }

    pub(super) async fn create_session(
        &self,
        origin: Origin,
        title: Option<String>,
        scene: Option<String>,
        model: Option<String>,
        extra: Value,
        settings: SessionSettings,
    ) -> Result<String, PortError> {
        // 只读工具不问写在 cli 层（`session_opts::cli_settings`）：会话层的 allow 规则 0.4 起要放宽开关才算。
        let mut options = json!({"prompt_types": ["permission", "question"]});
        if let (Some(o), Some(e)) = (options.as_object_mut(), extra.as_object()) {
            o.extend(e.clone());
        }
        settings.apply(&mut options);
        if let Some(m) = &model {
            options["model"] = json!(m);
        }
        let mut params = json!({"project_root": self.0.project, "options": options});
        if let Some(sc) = &scene {
            params["scene"] = json!(sc);
        }
        let r = self.call("session.create", params).await.map_err(rpc_err)?;
        let id = str_field(&r, "session_id");
        // 被设置夹住的只记日志：权限模式订阅后由 `refresh_permission` 按实际值写进摘要，其余的界面不显示。
        for (field, requested, applied) in session_opts::clamped(&r) {
            tracing::warn!(session = %id, %field, %requested, %applied, "session option clamped by settings");
        }
        let now = self.now();
        self.0.reg.locked().update(&id, |rec| {
            rec.origin = origin.clone();
            rec.title = title.clone();
            rec.model = model.clone();
            rec.updated_ms = now;
            rec.created_ms = now;
        });
        let sess = Sess {
            id: id.clone(),
            title: title.unwrap_or_else(|| state::short(&id)),
            project: Some(self.0.project.clone()),
            scene: r["scene"]
                .as_str()
                .map(str::to_string)
                .or(scene)
                .unwrap_or_else(|| "code".into()),
            model: model.unwrap_or_else(|| self.0.default_model.locked().clone()),
            origin,
            live: true,
            updated_ms: now,
            created_ms: now,
            ..Sess::default()
        };
        self.0.st.locked().sessions.insert(id.clone(), sess);
        let _ = self.subscribe(&id, false).await;
        self.upsert(&id);
        Ok(id)
    }

    pub(super) fn summary(&self, id: &str) -> Result<port::SessionSummary, PortError> {
        self.0
            .st
            .locked()
            .summary(id)
            .ok_or_else(|| PortError::Failed(format!("no session {id}")))
    }

    pub(super) async fn ensure_known(&self, id: &str) -> Result<(), PortError> {
        if !self.0.st.locked().sessions.contains_key(id) {
            self.adopt(id).await;
        }
        if self.0.st.locked().sessions.contains_key(id) {
            Ok(())
        } else {
            Err(PortError::Failed(format!("no session {id}")))
        }
    }

    pub(super) async fn history(&self, id: &str) -> Vec<HistoryMessage> {
        let first = self
            .call(
                "session.history",
                json!({"session_id": id, "offset": 0, "limit": HISTORY_TAIL}),
            )
            .await;
        let Ok(first) = first else { return vec![] };
        let total = first["total"].as_u64().unwrap_or(0);
        let page = if total > HISTORY_TAIL {
            self.call(
                "session.history",
                json!({"session_id": id, "offset": total - HISTORY_TAIL, "limit": HISTORY_TAIL}),
            )
            .await
            .unwrap_or(first)
        } else {
            first
        };
        serde_json::from_value(page["messages"].clone()).unwrap_or_default()
    }

    fn is_closed(&self, id: &str) -> bool {
        self.0.reg.locked().get(id).is_some_and(|r| r.closed)
    }

    /// 这个项目里在 Conix 里关掉的会话（Agix 还留着它们的转录），关掉的时刻新的在前。
    pub(super) async fn closed_sessions(&self) -> Vec<port::SessionSummary> {
        let Ok(v) = self.call("session.list", json!({})).await else {
            return vec![];
        };
        let infos: Vec<SessionInfo> =
            serde_json::from_value(v["sessions"].clone()).unwrap_or_default();
        let mut out: Vec<(u64, port::SessionSummary)> = infos
            .iter()
            .filter(|i| i.project_root.as_deref() == Some(self.0.project.as_str()))
            .filter_map(|i| {
                let at = self
                    .0
                    .reg
                    .locked()
                    .get(&i.session_id)
                    .filter(|r| r.closed)?
                    .closed_ms;
                let s = self.sess_from_info(i);
                Some((at, self.0.st.locked().summary_of(&s)))
            })
            .collect();
        out.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        out.into_iter().map(|(_, s)| s).collect()
    }

    /// 重新打开的会话放回会话表（冷的：用到时再恢复）。
    pub(super) async fn adopt_closed(&self, id: &str) {
        let Ok(v) = self.call("session.list", json!({})).await else {
            return;
        };
        let infos: Vec<SessionInfo> =
            serde_json::from_value(v["sessions"].clone()).unwrap_or_default();
        if let Some(i) = infos.iter().find(|i| i.session_id == id) {
            let s = self.sess_from_info(i);
            self.0.st.locked().sessions.insert(s.id.clone(), s);
        }
    }

    /// 已关闭的会话只读地看：读它的转录拼一份快照，不订阅、不放回会话表（交互 §5.2）。
    async fn watch_closed(&self, id: &str) -> Result<Reply, PortError> {
        let session = self
            .closed_sessions()
            .await
            .into_iter()
            .find(|s| s.id.0 == id)
            .ok_or_else(|| PortError::Failed(format!("no session {id}")))?;
        let conv = history::convert(&self.history(id).await);
        Ok(Reply::Watched(Box::new(Watched {
            session,
            history: conv.items,
            running: false,
            goal: None,
            todos: conv.todos.unwrap_or_default(),
            tasks: vec![],
            prompts: vec![],
        })))
    }

    pub(super) async fn watch(&self, id: &str) -> Result<Reply, PortError> {
        if self.is_closed(id) {
            return self.watch_closed(id).await;
        }
        self.ensure_known(id).await?;
        let gone = || PortError::Failed(format!("no session {id}"));
        let generation = self.0.link.locked().generation(id);
        let (parent, was_live) = {
            let st = self.0.st.locked();
            let s = st.sessions.get(id).ok_or_else(gone)?;
            (s.parent.clone(), s.live)
        };
        let mut running_hint = None;
        match &parent {
            None => {
                self.subscribe(id, true).await.map_err(rpc_err)?;
                if let Ok(g) = self.call("session.get", json!({"session_id": id})).await {
                    running_hint = Some(g["turn_state"].as_str() == Some("running"));
                    // 累计用量以引擎为准：重启后内存里的是 0，轮间的调用也只有引擎知道。
                    if let Ok(u) = serde_json::from_value::<wire::frames::Usage>(g["usage"].clone())
                        && let Some(s) = self.0.st.locked().sessions.get_mut(id)
                    {
                        s.usage = state::map_usage(&u);
                    }
                }
                if let Ok(g) = self.call("session.goal", json!({"session_id": id})).await {
                    let goal = serde_json::from_value::<wire::frames::GoalView>(g["goal"].clone())
                        .ok()
                        .map(|g| state::goal_of(&g));
                    if let Some(s) = self.0.st.locked().sessions.get_mut(id) {
                        s.goal = goal;
                    }
                }
                if let Ok(t) = self.call("session.tasks", json!({"session_id": id})).await {
                    let infos: Vec<wire::frames::TaskInfo> =
                        serde_json::from_value(t["tasks"].clone()).unwrap_or_default();
                    let now = self.now();
                    self.apply(|st, out| {
                        for i in infos {
                            st.on_event(id, id, SessionEvent::TaskUpdated { task: i }, now, out);
                        }
                    });
                }
            }
            // 侧链的帧经父会话来：父会话订阅着就够了。
            Some(p) => {
                let _ = self.subscribe(p, false).await;
            }
        }
        let msgs = self.history(id).await;
        let conv = history::convert(&msgs);
        // 收尾在 st 与 link 两把锁里一次做完：之后的帧都作为事件发出，之前的都在快照里。
        let mut st = self.0.st.locked();
        let s = st.sessions.get_mut(id).ok_or_else(gone)?;
        // 实例早就订阅着的会话，它自己跟着帧记的状态比刚才那次 session.get 新。
        if !was_live && let Some(r) = running_hint {
            s.running = r;
        }
        if s.todos.is_empty()
            && let Some(t) = conv.todos
        {
            s.todos = t;
        }
        let mut items = conv.items;
        // 每一步在 step_end 时连同工具结果落盘；正在写的这一步还没进历史，补上已经到了的那段。
        let pending = s.pending_step_text();
        if s.running && !pending.is_empty() {
            items.push(port::HistoryItem::Assistant { text: pending });
        }
        let s = &st.sessions[id];
        let watched = Watched {
            session: st.summary(id).ok_or_else(gone)?,
            history: items,
            running: s.running,
            goal: s.goal.clone(),
            todos: s.todos.clone(),
            tasks: s.tasks.values().cloned().collect(),
            prompts: s.prompts.clone(),
        };
        let mut link = self.0.link.locked();
        if link.generation(id) == generation {
            link.watched.insert(id.to_string());
        }
        Ok(Reply::Watched(Box::new(watched)))
    }

    /// 起名：用户改的名字总算数；引擎首轮自动起的名字只替掉"第一句话"那种临时名字；
    /// 第一句话只在还没有名字时用（Agix 不报冷会话的名字，没有它列表里只剩 id）。
    pub(super) fn retitle(&self, id: &str, title: &str, by: Titler) {
        {
            let mut reg = self.0.reg.locked();
            let rec = reg.get(id).cloned().unwrap_or_default();
            let keep = match by {
                Titler::User => false,
                Titler::Engine => rec.title.is_some() && !rec.title_from_text,
                Titler::FirstText => rec.title.is_some(),
            };
            if keep {
                return;
            }
            reg.update(id, |r| {
                r.title = Some(title.to_string());
                r.title_from_text = by == Titler::FirstText;
            });
        }
        if let Some(s) = self.0.st.locked().sessions.get_mut(id) {
            s.title = title.to_string();
        }
        self.upsert(id);
    }

    /// 配置了的 provider 各自有哪些模型：经引擎问各自的端点（`config.listModels`），问不到的跳过。
    pub(super) async fn list_models(&self) -> Vec<port::ModelInfo> {
        let loc = json!({"scene": "code", "project_root": self.0.project, "tier": "effective"});
        let Ok(cfg) = self.call("config.get", loc).await else {
            return vec![];
        };
        // 0.4 起 `providers` 是列表；一个都没定义时引擎用自带的 `anthropic`。
        let mut providers: Vec<String> = cfg["settings"]["providers"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|p| p["id"].as_str().map(str::to_string))
            .collect();
        if providers.is_empty() {
            providers.push("anthropic".into());
        }
        let mut out = vec![];
        for p in providers {
            let Ok(v) = self
                .call(
                    "config.listModels",
                    json!({"provider": p, "scene": "code", "project_root": self.0.project}),
                )
                .await
            else {
                continue;
            };
            for m in v["models"].as_array().into_iter().flatten() {
                if let Some(id) = m["id"].as_str() {
                    out.push(port::ModelInfo {
                        provider: p.clone(),
                        id: id.to_string(),
                        display_name: m["display_name"].as_str().map(str::to_string),
                    });
                }
            }
        }
        out
    }

    /// 项目里的文件，相对项目根：仓库里用 `git ls-files`（跟着 `.gitignore`，含没提交的新文件），
    /// 不是仓库就没有（不去整个目录树里乱走）。至多一万个，`@` 补全够用。
    pub(super) async fn project_files(&self) -> Vec<String> {
        let out = tokio::process::Command::new("git")
            .args(["ls-files", "-co", "--exclude-standard"])
            .current_dir(&self.0.project)
            .output()
            .await;
        let Some(o) = out.ok().filter(|o| o.status.success()) else {
            return vec![];
        };
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .take(10_000)
            .map(str::to_string)
            .collect()
    }
}
