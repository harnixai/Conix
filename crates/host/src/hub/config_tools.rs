//! 在会话里配置自己（architecture §8.11）：换 provider 分"加、测、切、退"四步，改 `conix.yaml` 先校验再写。
//!
//! 为什么拆成四步而不是一次"改配置"：改了就生效的话，新的 provider 不通，会话当场没有模型可用，也就没法
//! 再把它改回来。所以加的时候不动路由（`model.order`），在旁边测通了才切，切完立刻再测一次，不通自动退回。

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::*;

/// 测一次给的时限：连得上的 provider 一句话几秒就回来。
const PROBE_TIMEOUT: Duration = Duration::from_secs(90);
/// 测一次至多花的 token：只为看通不通，不为干活。
const PROBE_BUDGET: u64 = 20_000;
const PROBE_PROMPT: &str = "Reply with exactly the word OK.";

/// 实例对 `conix.yaml` 的读写（由 `conix` crate 给：解析与启动时同一个，`app::config`）。
#[async_trait]
pub trait ConixConfigStore: Send + Sync {
    /// 文件现在的文字。
    fn read(&self) -> Result<String, String>;
    /// 把点分路径上的值换成 `value`（YAML 的值）后，按启动时的解析校验；不写。回新的全文。
    fn set(&self, path: &str, value: &Value) -> Result<String, String>;
    /// 写下新的全文（先备份），并把当场能生效的部分（`jobs`、`notify`）交给实例。回备份在哪、哪些要重启才生效。
    async fn apply(&self, hub: &Hub, text: &str) -> Result<(PathBuf, Vec<String>), String>;
}

/// 一次测通的结果。
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Probe {
    pub ok: bool,
    pub elapsed_ms: u64,
    pub reply: String,
    /// 实际回答的 provider 不是要测的那个（`model.order` 回退到了别的）。
    pub switched_to: Option<String>,
    pub error: Option<String>,
}

impl Probe {
    fn text(&self) -> String {
        if self.ok {
            format!(
                "ok in {} ms (reply: {:?})",
                self.elapsed_ms,
                self.reply.trim()
            )
        } else {
            format!(
                "FAILED after {} ms: {}",
                self.elapsed_ms,
                self.error.as_deref().unwrap_or("no reply")
            )
        }
    }
}

impl Hub {
    pub fn set_conix_config(&self, store: Arc<dyn ConixConfigStore>) {
        let _ = self.0.conix_config.set(store);
    }

    pub fn set_config_home(&self, home: PathBuf) {
        *self.0.config_home.locked() = Some(home);
    }

    /// 开一个不进列表的临时会话，指定 provider / 模型打一句话，看它答没答；测完删掉会话。
    pub(crate) async fn probe(&self, provider: Option<&str>, model: Option<&str>) -> Probe {
        let started = Instant::now();
        let key = format!("probe:{}:{}", self.now(), uuid::Uuid::new_v4());
        let id = crate::flows::session_host::session_id_of(&key);
        self.0.probes.locked().insert(id.clone());
        let mut options = json!({
            "prompt_types": [],
            "attended": false,
            "auto_name": false,
            "settings": {
                "turn": {"token_budget": PROBE_BUDGET},
                // 只答一句话：工具一个都不用，免得测的时候真去动项目。
                "permissions": {"deny": ["Bash", "Write", "Edit", "NotebookEdit", "WebFetch", "WebSearch", "Agent"]}
            }
        });
        if let Some(p) = provider {
            options["provider"] = json!(p);
        }
        if let Some(m) = model {
            options["model"] = json!(m);
        }
        let mut out = Probe::default();
        let created = self
            .call(
                "session.create",
                json!({"session_id": id, "project_root": self.0.project, "options": options}),
            )
            .await;
        if let Err(e) = created {
            out.error = Some(format!("could not open a test session: {e}"));
            self.0.probes.locked().remove(&id);
            return out;
        }
        let mut rx = self.tap(&id);
        let _ = self
            .call(
                "session.subscribe",
                json!({"session_id": id, "resume": false}),
            )
            .await;
        let turn = self.call(
            "session.run_turn",
            json!({"session_id": id, "message": PROBE_PROMPT, "turn_id": uuid::Uuid::new_v4().to_string()}),
        );
        let collect = async {
            let r = turn.await;
            // run_turn 回来时这一轮已经结束；帧在它之前到，收干净。
            while let Ok(t) = rx.try_recv() {
                match t {
                    Tapped::Event { event, .. } => match SessionEvent::parse(&event) {
                        SessionEvent::TextDelta { text } => out.reply.push_str(&text),
                        SessionEvent::ProviderSwitched { to, .. } => out.switched_to = Some(to),
                        SessionEvent::TurnComplete {
                            stop_reason,
                            message,
                            ..
                        } if stop_reason != "end_turn" && stop_reason != "stop_sequence" => {
                            out.error = Some(format!(
                                "the turn ended with {stop_reason}{}",
                                message.map(|m| format!(": {m}")).unwrap_or_default()
                            ));
                        }
                        SessionEvent::Error { message } => out.error = Some(message),
                        _ => {}
                    },
                    Tapped::Failed { kind, message, .. } => {
                        out.error = Some(format!("{kind}: {message}"))
                    }
                }
            }
            if let Err(e) = r {
                out.error.get_or_insert(e.to_string());
            }
        };
        if tokio::time::timeout(PROBE_TIMEOUT, collect).await.is_err() {
            out.error = Some(format!("no answer within {}s", PROBE_TIMEOUT.as_secs()));
            let _ = self
                .call("session.interrupt", json!({"session_id": id}))
                .await;
        }
        if let (Some(want), Some(got)) = (provider, &out.switched_to)
            && want != got
        {
            out.error.get_or_insert(format!(
                "`{want}` did not answer: the call fell back to `{got}` (model.order)"
            ));
        }
        out.ok = out.error.is_none() && !out.reply.trim().is_empty();
        if !out.ok && out.error.is_none() {
            out.error = Some("the model answered nothing".into());
        }
        out.elapsed_ms = started.elapsed().as_millis() as u64;
        let _ = self.call("session.delete", json!({"session_id": id})).await;
        self.0.probes.locked().remove(&id);
        out
    }

    /// 问 provider 的端点有哪些模型：通了回 `Ok(个数)`；连都连不上（拒绝连接、解析不了主机）回 `Err(原因, true)`——
    /// 这时不必再真打一次（引擎会一直重试到时限）；别的失败（端点不支持列模型）回 `Err(原因, false)`，照样去打。
    async fn provider_models(&self, id: &str) -> Result<usize, (String, bool)> {
        match self
            .call(
                "config.listModels",
                json!({"provider": id, "scene": "code", "project_root": self.0.project}),
            )
            .await
        {
            Ok(v) => Ok(v["models"].as_array().map_or(0, Vec::len)),
            Err(e) => {
                let t = e.to_string();
                let l = t.to_lowercase();
                // Agix 把连不上报成 `transport: … error sending request`。
                let dead = [
                    "transport:",
                    "error sending request",
                    "connection refused",
                    "dns",
                    "unreachable",
                ]
                .iter()
                .any(|k| l.contains(k));
                Err((t, dead))
            }
        }
    }

    /// 测一个 provider：端点连不上就直接回不通，连得上再真打一次。
    async fn test_provider(&self, id: &str, model: Option<&str>) -> (String, Probe) {
        match self.provider_models(id).await {
            Err((why, true)) => (
                format!("endpoint unreachable: {why}"),
                Probe {
                    error: Some(format!("the endpoint cannot be reached: {why}")),
                    ..Probe::default()
                },
            ),
            listed => {
                let line = match listed {
                    Ok(n) => format!("endpoint answers: {n} model(s)"),
                    Err((why, _)) => {
                        format!("listing models failed (some endpoints do not list): {why}")
                    }
                };
                (line, self.probe(Some(id), model).await)
            }
        }
    }

    fn config_home(&self) -> Result<PathBuf, String> {
        self.0
            .config_home
            .locked()
            .clone()
            .ok_or_else(|| "this instance does not know its home directory".to_string())
    }

    async fn providers(&self) -> Result<Value, String> {
        self.call(
            "config.getProvider",
            json!({"scene": "code", "project_root": self.0.project}),
        )
        .await
        .map_err(|e| e.to_string())
    }

    /// 全局层现在写的 `model.order` 与 revision（切换写在全局层：provider 一般定义在那里）。
    async fn global_order(&self) -> Result<(Value, String), String> {
        let g = self
            .call(
                "config.get",
                json!({"scene": "code", "project_root": self.0.project, "tier": "global"}),
            )
            .await
            .map_err(|e| e.to_string())?;
        Ok((
            g["settings"]["model"]["order"].clone(),
            g["revision"].as_str().unwrap_or_default().to_string(),
        ))
    }

    async fn set_order(&self, order: &Value, revision: Option<&str>) -> Result<(), String> {
        let mut p = json!({"tier": "global", "scene": "code", "project_root": self.0.project,
                           "changes": {"model.order": order}});
        if let Some(r) = revision.filter(|r| !r.is_empty()) {
            p["if_revision"] = json!(r);
        }
        self.call("config.set", p)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    }

    /// 加 provider 之前把现在的路由钉住，加完路由一定不变。
    ///
    /// 不能只是"不写 `model.order`"：设置里一个 provider 都没定义时引擎用自带的 `anthropic`，一旦定义了任何一个，
    /// 自带的就没了、路由改走第一个定义的——加一个 provider 本身就把模型切走了。所以这时先把自带的照原样写成一个
    /// 显式的 `anthropic`（与自带的同一种认证：`x-api-key`，key 取 `ANTHROPIC_AUTH_TOKEN` / `ANTHROPIC_API_KEY`，
    /// 地址取 `ANTHROPIC_BASE_URL`，三档都是现在的模型），并显式写下 `model.order`，立刻用缺省打一句；不通就撤回。
    /// 已经有定义时只需把现在生效的 `model.order` 显式写下（没写时它是"第一个定义的"，加一个可能改了次序）。
    /// 现在的缺省本来就不通（比如还没给 key）时没什么要保住的：不钉，回一句说明，加了就走新的。
    async fn pin_routing(&self, tier: &str) -> Result<Option<String>, String> {
        let cur = self.providers().await?;
        let list = cur["providers"].as_array().cloned().unwrap_or_default();
        let builtin_only = list.iter().all(|p| p["builtin"] == true);
        let order = cur["order"].clone();
        let set_order = |order: Value| {
            json!({"tier": tier, "scene": "code", "project_root": self.0.project,
                   "changes": {"model.order": order}})
        };
        if !builtin_only {
            self.call("config.set", set_order(order))
                .await
                .map_err(|e| format!("pinning model.order: {e}"))?;
            return Ok(None);
        }
        let now = self.probe(None, None).await;
        if !now.ok {
            return Ok(Some(format!(
                "the current default (the built-in provider) does not answer either ({}), so there was nothing to keep: the routing now goes to the providers defined",
                now.text()
            )));
        }
        let model = self.0.default_model.locked().clone();
        let mut config = json!({"vendor": "anthropic", "api": "anthropic",
                                "tiers": {"strong": model, "balanced": model, "fast": model}});
        if let Some(url) = std::env::var("ANTHROPIC_BASE_URL")
            .ok()
            .filter(|u| !u.is_empty())
        {
            config["base_url"] = json!(url);
        }
        if let Some(env) = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]
            .into_iter()
            .find(|v| std::env::var(v).is_ok_and(|x| !x.is_empty()))
        {
            config["api_key_env"] = json!(env);
        }
        self.call(
            "config.setProvider",
            json!({"tier": tier, "scene": "code", "project_root": self.0.project,
                   "provider_id": "anthropic", "config": config}),
        )
        .await
        .map_err(|e| format!("writing the built-in provider out: {e}"))?;
        self.call("config.set", set_order(json!(["anthropic"])))
            .await
            .map_err(|e| format!("pinning model.order: {e}"))?;
        let check = self.probe(None, None).await;
        if !check.ok {
            let _ = self
                .call(
                    "config.setProvider",
                    json!({"tier": tier, "scene": "code", "project_root": self.0.project,
                           "provider_id": "anthropic", "delete": true}),
                )
                .await;
            let _ = self.call("config.set", set_order(Value::Null)).await;
            return Err(format!(
                "nothing added: writing the built-in provider out as an explicit one did not answer, so it was undone — {}",
                check.text()
            ));
        }
        Ok(None)
    }

    pub(super) async fn config_tool(&self, name: &str, input: &Value) -> Result<String, String> {
        match name {
            tools::CONFIG_SHOW => {
                let p = self.providers().await?;
                let mut lines = vec![format!(
                    "default model: {}",
                    self.0.default_model.locked().clone()
                )];
                lines.push(format!("model.order (routing): {}", p["order"]));
                for x in p["providers"].as_array().into_iter().flatten() {
                    lines.push(format!(
                        "provider {}: api {}, base_url {}, key {} ({}), tiers {}{}",
                        x["id"].as_str().unwrap_or("?"),
                        x["api"].as_str().unwrap_or("anthropic"),
                        x["base_url"].as_str().unwrap_or("(default)"),
                        x["api_key_env"].as_str().unwrap_or("(host / ANTHROPIC_*)"),
                        if x["credential_present"] == true {
                            "present"
                        } else {
                            "MISSING"
                        },
                        x["tiers"],
                        x["defined_in"]
                            .as_str()
                            .map(|d| format!(", in {d}"))
                            .unwrap_or_else(|| " (built in)".into()),
                    ));
                }
                if let Some(s) = self.0.conix_config.get() {
                    lines.push(String::new());
                    lines.push("conix.yaml:".into());
                    lines.push(s.read()?);
                }
                Ok(lines.join("\n"))
            }
            tools::CONFIG_PROVIDER_ADD => {
                let id = input["provider_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("provider_id is required")?;
                let config = input.get("config").cloned().ok_or("config is required")?;
                if config.get("order").is_some() {
                    return Err("adding a provider does not change model.order; switch with config_switch after config_provider_test".into());
                }
                let tier = input["tier"].as_str().unwrap_or("global");
                let p = json!({"tier": tier, "scene": "code", "project_root": self.0.project,
                               "provider_id": id, "config": config});
                let mut dry = p.clone();
                dry["dry_run"] = json!(true);
                self.call("config.setProvider", dry).await.map_err(|e| {
                    format!("the provider does not check out (nothing written): {e}")
                })?;
                let note = self.pin_routing(tier).await?;
                let r = self
                    .call("config.setProvider", p)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(match note {
                    None => format!(
                        "provider `{id}` written to {} (routing unchanged: model.order is still {}). Test it with config_provider_test before switching.",
                        r["written_to"].as_str().unwrap_or("?"),
                        r["order"]
                    ),
                    Some(n) => format!(
                        "provider `{id}` written to {}; {n} (model.order is {}). Test it with config_provider_test.",
                        r["written_to"].as_str().unwrap_or("?"),
                        r["order"]
                    ),
                })
            }
            tools::CONFIG_PROVIDER_TEST => {
                let id = input["provider_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("provider_id is required")?;
                let (listed, probe) = self.test_provider(id, input["model"].as_str()).await;
                let verdict = if probe.ok { "PASS" } else { "FAIL" };
                Ok(format!(
                    "{verdict}: provider `{id}`\n{listed}\na real call: {}",
                    probe.text()
                ))
            }
            tools::CONFIG_SWITCH => {
                let id = input["provider_id"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("provider_id is required")?;
                // 先测：不通就不切。
                let (_, before_probe) = self.test_provider(id, None).await;
                if !before_probe.ok {
                    return Err(format!(
                        "not switched: `{id}` does not work — {}",
                        before_probe.text()
                    ));
                }
                let (old, revision) = self.global_order().await?;
                let effective = self.providers().await?["order"].clone();
                let mut order: Vec<Value> = vec![json!(id)];
                order.extend(
                    effective
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter(|v| v.as_str() != Some(id))
                        .cloned(),
                );
                let backup = self.config_home()?.join("config-backups");
                std::fs::create_dir_all(&backup).map_err(|e| e.to_string())?;
                let file = backup.join(format!("{}.json", self.now()));
                std::fs::write(
                    &file,
                    serde_json::to_vec_pretty(&json!({"tier": "global", "model.order": old}))
                        .unwrap_or_default(),
                )
                .map_err(|e| format!("backup {}: {e}", file.display()))?;
                self.set_order(&json!(order), Some(&revision)).await?;
                // 切完立刻用新的缺省（不指定 provider）打一句：不通就写回去。
                let after = self.probe(None, None).await;
                let served_by_new = after.switched_to.as_deref().is_none_or(|p| p == id);
                if after.ok && served_by_new {
                    Ok(format!(
                        "switched: model.order is now {} — {}. Backup: {} (config_rollback restores it).",
                        json!(order),
                        after.text(),
                        file.display()
                    ))
                } else {
                    self.set_order(&old, None).await?;
                    Err(format!(
                        "switching failed and was rolled back (model.order is {old} again): {}",
                        after.text()
                    ))
                }
            }
            tools::CONFIG_ROLLBACK => {
                let dir = self.config_home()?.join("config-backups");
                let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
                    .map_err(|_| "no switch to roll back".to_string())?
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x == "json"))
                    .collect();
                files.sort();
                let last = files.pop().ok_or("no switch to roll back")?;
                let b: Value =
                    serde_json::from_slice(&std::fs::read(&last).map_err(|e| e.to_string())?)
                        .map_err(|e| e.to_string())?;
                self.set_order(&b["model.order"], None).await?;
                let _ = std::fs::rename(&last, last.with_extension("json.restored"));
                let after = self.probe(None, None).await;
                Ok(format!(
                    "rolled back: model.order is {} again — {}",
                    b["model.order"],
                    after.text()
                ))
            }
            tools::CONIX_CONFIG_SET => {
                let store = self
                    .0
                    .conix_config
                    .get()
                    .cloned()
                    .ok_or("conix.yaml is not managed by this instance")?;
                let path = input["path"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .ok_or("path is required, e.g. jobs.defaults.network")?;
                let value = input.get("value").cloned().unwrap_or(Value::Null);
                let text = store.set(path, &value)?;
                if input["dry_run"] == true {
                    return Ok(format!("checks out (nothing written):\n{text}"));
                }
                let (backup, restart) = store.apply(self, &text).await?;
                Ok(format!(
                    "conix.yaml written (backup {}).{}",
                    backup.display(),
                    if restart.is_empty() {
                        " In effect now.".to_string()
                    } else {
                        format!(
                            " Takes effect at the next start of: {}.",
                            restart.join(", ")
                        )
                    }
                ))
            }
            other => Err(format!("unknown tool {other}")),
        }
    }
}
