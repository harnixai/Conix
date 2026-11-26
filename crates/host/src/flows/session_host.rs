//! Conix 的 `SessionHost`：流程派工器的会话，经 Hub 开在嵌入的 Agix 上（Hub 仍是引擎唯一的
//! 客户端）。会话 id 由派工器的键算出（Agix 的 `session.create` 认 `session_id`），所以同一个键
//! 开两次——实例重启之后也一样——是同一个会话。

use crate::sync::Locked;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use harnix_flow::runner::{
    AgentTask, AgentTaskStatus, Decision, Frame, HostError, Note, OpenSpec, Opened, SessionHost,
    SessionId, SessionInfo, Watch,
};
use harnix_flow::{Engine, ask_rules};
use port::{Origin, RunId, StationId, TaskKind, TaskStatus as PortTaskStatus};
use serde_json::{Value, json};

use super::frames::Translator;
use super::policy::{RunPolicies, RunPolicy};
use crate::engine::RpcError;
use crate::hub::{SessionSettings, WeakHub, rule_strings};

/// 任务会话的权限模式：最宽、但仍按会话的规则走的那一档。`bypassPermissions` 不看会话的规则，
/// 派工器对问句的回答就落空了。
pub(crate) const TASK_PERMISSION_MODE: &str = "acceptEdits";

/// Agix 的错误码（协议 §9）里会让派工器换个说法的几个。
const SESSION_NOT_FOUND: i64 = -32000;
const SCENE_NOT_FOUND: i64 = -32004;
const INVALID_PARAMS: i64 = -32602;

pub(crate) struct ConixSessionHost {
    hub: WeakHub,
    /// 运行的记录（会话的归属要写流程名、主题、站名）；派工器建好后填上。
    pub(crate) engine: OnceLock<Arc<Engine>>,
    /// 这个进程里每个键开时的 spec：同一个键换了 spec 再开要拒；跨重启由引擎比 `options`。
    opened: Mutex<HashMap<String, OpenSpec>>,
    /// 作业起的运行的会话限制（网络、禁 `CronCreate`）。
    policies: Arc<RunPolicies>,
}

impl ConixSessionHost {
    pub(crate) fn new(hub: WeakHub, policies: Arc<RunPolicies>) -> Arc<Self> {
        Arc::new(Self {
            hub,
            engine: OnceLock::new(),
            opened: Mutex::default(),
            policies,
        })
    }

    /// 这次运行的会话限制：记过就用记下的；没记过而运行是作业起的，就按作业此刻的护栏定下并记住
    /// （第一次开站会话时取，免得与运行启动抢先后）。
    async fn policy(&self, run: &str) -> Option<RunPolicy> {
        if let Some(p) = self.policies.get(run) {
            return Some(p);
        }
        let rec = self.engine.get()?.record(run).await.ok()?;
        let harnix_flow::Trigger::Schedule { schedule, .. } = &rec.trigger else {
            return None;
        };
        let hub = self.hub().ok()?;
        let p = RunPolicy {
            job: schedule.clone(),
            network: hub.job_network(schedule).unwrap_or_default(),
        };
        self.policies.put(run, p.clone());
        self.policies.get(run)
    }

    fn hub(&self) -> Result<crate::hub::Hub, HostError> {
        self.hub
            .upgrade()
            .ok_or_else(|| HostError::Unavailable("the instance is stopping".into()))
    }

    /// 会话归属：流程标题、主题、站。主会话的站记作 `@main`。
    async fn origin(&self, spec: &OpenSpec) -> (Origin, Option<String>) {
        let (station, _) = station_of(&spec.key);
        let rec = match self.engine.get() {
            Some(e) => e.record(&spec.run).await.ok(),
            None => None,
        };
        let flow_title = rec
            .as_ref()
            .map(|r| {
                if r.def.title.is_empty() {
                    r.flow.clone()
                } else {
                    r.def.title.clone()
                }
            })
            .unwrap_or_default();
        let subject = rec.as_ref().map(|r| r.subject.clone()).unwrap_or_default();
        let station_title = rec
            .as_ref()
            .and_then(|r| r.def.stations.iter().find(|s| s.id == station))
            .map(|s| s.title.clone())
            .unwrap_or_default();
        let title = match (&spec.title, spec.main) {
            (Some(t), _) => Some(t.clone()),
            (None, true) => Some(subject.clone()).filter(|s| !s.is_empty()),
            (None, false) => Some(format!("{subject} · {station_title}")),
        };
        (
            Origin::FlowStation {
                run: RunId(spec.run.clone()),
                flow_title,
                subject,
                station: StationId(station),
                station_title,
            },
            title,
        )
    }
}

/// 派工器的键里的站：主会话 `flow:<运行>@main`、一件任务自己的 `flow:<运行>/<站>/<任务>@<token>`、
/// 共享的 `flow:<运行>#<键>`。回 `(站, 是不是主会话)`。
pub(crate) fn station_of(key: &str) -> (String, bool) {
    let rest = key.strip_prefix("flow:").unwrap_or(key);
    if rest.ends_with("@main") {
        return ("@main".into(), true);
    }
    match rest.split_once('/') {
        Some((_, tail)) => (tail.split('/').next().unwrap_or("").to_string(), false),
        None => (
            rest.split_once('#')
                .map(|(_, k)| format!("#{k}"))
                .unwrap_or_default(),
            false,
        ),
    }
}

/// 一个键开出来的会话 id：键的 v5 UUID，照引擎写 id 的样子写成 BASE58。
pub(crate) fn session_id_of(key: &str) -> String {
    let ns = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_DNS, b"flow.conix");
    base58(uuid::Uuid::new_v5(&ns, key.as_bytes()).as_bytes())
}

fn base58(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
    let zeros = bytes.iter().take_while(|b| **b == 0).count();
    let mut digits: Vec<u8> = vec![];
    for &b in &bytes[zeros..] {
        let mut carry = b as u32;
        for d in digits.iter_mut() {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut out = String::with_capacity(zeros + digits.len());
    out.extend(std::iter::repeat_n('1', zeros));
    out.extend(digits.iter().rev().map(|&d| ALPHABET[d as usize] as char));
    out
}

/// 建会话的 `options`：只由 spec 与这次运行的限制定，同一个 spec 再建总是同一份（引擎按它判断
/// "是同一个会话"；运行的限制按运行记下、不再变，见 [`super::policy`]）。
pub(crate) fn options_of(spec: &OpenSpec, policy: Option<&RunPolicy>) -> Value {
    // 只读工具与流程自己的工具（flow_state / flow_submit / flow_propose）不问，写在 cli 层
    // （`hub::cli_settings`）；`acceptEdits` 比项目缺省宽，靠 cli 层开的 `client_may_loosen` 才生效。
    // 流程库的 `ask_rules` 还是 0.3 的规则形状，这里换成规则串。
    let settings = SessionSettings {
        mode: Some(TASK_PERMISSION_MODE.into()),
        ask: rule_strings(&ask_rules(&spec.asks), "ask"),
        effort: spec.placement.effort.clone(),
        // 作业起的运行同简单作业：命令的出网按作业的护栏，模型不能再自建定时任务。
        network: policy
            .and_then(|p| p.network.sandbox_mode())
            .map(Into::into),
        deny: policy.map_or_else(Vec::new, |_| vec!["CronCreate".into()]),
        ..SessionSettings::default()
    };
    let mut o = json!({
        "prompt_types": ["permission", "question"],
        "attended": true,
        "auto_name": false,
    });
    settings.apply(&mut o);
    if let Some(m) = &spec.placement.model {
        o["model"] = json!(m);
    }
    if let Some(p) = &spec.placement.provider {
        o["provider"] = json!(p);
    }
    if let Some(s) = &spec.output_schema {
        o["output_schema"] = s.clone();
    }
    if !spec.tools.is_empty() {
        o["host_tools"] = Value::Array(
            spec.tools
                .iter()
                // `flow_command` 已作为连接上的工具挂在每个会话上（`flows::tools::declarations`），
                // 这里再带一份就是同名两个。
                .filter(|t| t.name != harnix_flow::runner::TOOL_COMMAND)
                .map(|t| {
                    json!({"name": t.name, "description": t.description,
                           "input_schema": t.input_schema, "read_only": t.read_only})
                })
                .collect(),
        );
    }
    o
}

fn host_error(e: RpcError) -> HostError {
    match e {
        RpcError::Remote { code, message, .. } => match code {
            SESSION_NOT_FOUND => HostError::Gone(message),
            SCENE_NOT_FOUND | INVALID_PARAMS => HostError::Refused(message),
            c if c <= -32000 => HostError::Refused(message),
            _ => HostError::Other(message),
        },
        RpcError::Closed => HostError::Unavailable("the engine is gone".into()),
    }
}

#[async_trait::async_trait]
impl SessionHost for ConixSessionHost {
    async fn open(&self, spec: OpenSpec) -> Result<Opened, HostError> {
        if let Some(had) = self.opened.locked().get(&spec.key)
            && *had != spec
        {
            return Err(HostError::Refused(format!(
                "key {} is open with another spec",
                spec.key
            )));
        }
        let p = &spec.placement;
        if p.instance.is_some() || p.template.is_some() || p.space.is_some() {
            return Err(HostError::Refused(
                "Conix runs every session in this instance: instance, template and space are not supported"
                    .into(),
            ));
        }
        let hub = self.hub()?;
        let (origin, title) = self.origin(&spec).await;
        let policy = self.policy(&spec.run).await;
        let project = spec
            .project_root
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());
        let (id, existed) = hub
            .open_flow_session(
                &session_id_of(&spec.key),
                &spec.profile,
                project.as_deref(),
                options_of(&spec, policy.as_ref()),
                origin,
                title,
            )
            .await
            .map_err(host_error)?;
        self.opened.locked().insert(spec.key.clone(), spec);
        Ok(Opened { id, existed })
    }

    async fn lookup(&self, id: &SessionId) -> Result<Option<SessionInfo>, HostError> {
        let hub = self.hub()?;
        if !hub.bring_back(id).await.map_err(host_error)? {
            return Ok(None);
        }
        Ok(hub.session_state(id).map(|(running, project)| SessionInfo {
            project_root: project.map(Into::into),
            turn_running: running,
        }))
    }

    async fn watch(&self, id: &SessionId) -> Result<Watch, HostError> {
        let hub = self.hub()?;
        if !hub.bring_back(id).await.map_err(host_error)? {
            return Err(HostError::Gone(id.clone()));
        }
        // 先接上再读状态：两者之间结束的一轮，结束帧已经在流里了。
        let rx = hub.tap(id);
        let (running, project) = hub.session_state(id).unwrap_or_default();
        let frames = futures::stream::unfold(
            (
                rx,
                Translator::default(),
                std::collections::VecDeque::<Frame>::new(),
            ),
            |(mut rx, mut tr, mut queued)| async move {
                loop {
                    if let Some(f) = queued.pop_front() {
                        return Some((f, (rx, tr, queued)));
                    }
                    queued.extend(tr.translate(rx.recv().await?));
                }
            },
        );
        Ok(Watch {
            frames: Box::pin(frames),
            info: SessionInfo {
                project_root: project.map(Into::into),
                turn_running: running,
            },
        })
    }

    async fn send(&self, id: &SessionId, text: &str) -> Result<(), HostError> {
        let hub = self.hub()?;
        if hub.session_state(id).is_none() && !hub.bring_back(id).await.map_err(host_error)? {
            return Err(HostError::Gone(id.clone()));
        }
        hub.send_queued(id, text);
        Ok(())
    }

    async fn interrupt(&self, id: &SessionId) -> Result<(), HostError> {
        self.hub()?.interrupt(id).await.map_err(host_error)
    }

    async fn close(&self, id: &SessionId) -> Result<(), HostError> {
        self.hub()?
            .close_session(id)
            .await
            .map_err(|e| HostError::Other(e.to_string()))
    }

    async fn answer(&self, id: &SessionId, ask: &str, decision: Decision) -> Result<(), HostError> {
        let (allow, reason) = match decision {
            Decision::Allow => (true, None),
            Decision::Deny { reason } => (false, Some(reason)),
        };
        // 问句已经不悬着（那一轮结束了、别人答了）：派工器不在乎，算答过。
        let _ = self.hub()?.answer_permission(id, ask, allow, reason).await;
        Ok(())
    }

    async fn note(&self, id: &SessionId, note: Note) -> Result<(), HostError> {
        self.hub()?
            .append_entry(id, &note.event, note.payload)
            .await
            .map_err(host_error)
    }

    /// 主会话的任务表里，description 带这个标记的子 Agent。引擎的 `Agent` 工具前台调用时结果里没有任务 id，
    /// 模型给不出来，所以按派活时定下的 description 找，不让模型报。
    async fn agent_tasks(
        &self,
        id: &SessionId,
        tag: &str,
    ) -> Result<Option<Vec<AgentTask>>, HostError> {
        let hub = self.hub()?;
        Ok(Some(
            hub.tasks_of(id)
                .into_iter()
                .filter(|t| t.kind == TaskKind::Agent && t.label.contains(tag))
                .map(|t| AgentTask {
                    id: t.id.0,
                    session: t.agent_session.map(|s| s.0),
                    status: match t.status {
                        PortTaskStatus::Done => AgentTaskStatus::Done,
                        PortTaskStatus::Running | PortTaskStatus::Waiting => {
                            AgentTaskStatus::Running
                        }
                        PortTaskStatus::Failed | PortTaskStatus::Stopped => AgentTaskStatus::Failed,
                    },
                    usage: harnix_flow::Usage {
                        input_tokens: t.usage.input,
                        output_tokens: t.usage.output,
                        cache_read_input_tokens: t.usage.cache_read,
                        cache_creation_input_tokens: t.usage.cache_write,
                        turns: 0,
                    },
                })
                .collect(),
        ))
    }

    async fn project_trusted(&self, root: &Path) -> Result<bool, HostError> {
        self.hub()?
            .project_trusted(&root.to_string_lossy())
            .await
            .map_err(host_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use harnix_flow::Delivery;

    #[test]
    fn a_key_is_always_the_same_engine_style_id() {
        let a = session_id_of("flow:r-1@main");
        assert_eq!(a, session_id_of("flow:r-1@main"));
        assert_ne!(a, session_id_of("flow:r-2@main"));
        assert!(
            (21..=22).contains(&a.len()) && a.chars().all(|c| c.is_ascii_alphanumeric()),
            "{a}"
        );
        assert_eq!(base58(&[0x00, 0xff]), "15Q");
    }

    #[test]
    fn stations_are_read_off_the_runners_keys() {
        assert_eq!(station_of("flow:r1@main"), ("@main".into(), true));
        assert_eq!(
            station_of("flow:r1/review/t1@tok"),
            ("review".into(), false)
        );
        assert_eq!(station_of("flow:r1#shared"), ("#shared".into(), false));
    }

    #[test]
    fn options_depend_only_on_the_spec() {
        let mut spec = OpenSpec::new("flow:r@main", "r", "code", Delivery::FlowSubmit);
        spec.asks = vec!["Write".into(), "Bash(git:*)".into()];
        let o = options_of(&spec, None);
        assert_eq!(o, options_of(&spec.clone(), None));
        let p = &o["settings"]["permissions"];
        assert_eq!(p["mode"], TASK_PERMISSION_MODE);
        assert_eq!(p["ask"], json!(["Write", "Bash(git:*)"]));
        assert!(p.get("allow").is_none(), "放行的写在 cli 层，会话层只收紧");
        for old in ["permission_mode", "permission_rules", "effort"] {
            assert!(o.get(old).is_none(), "0.4 不认 options.{old}");
        }
        assert!(o.get("host_tools").is_none());

        spec.placement.effort = Some("high".into());
        spec.tools = harnix_flow::runner::tool_specs();
        let o = options_of(&spec, None);
        assert_eq!(o["settings"]["model"]["effort"], "high");
        assert_eq!(
            o["host_tools"].as_array().unwrap().len(),
            3,
            "flow_command 在连接上，不在会话上"
        );
        assert!(
            o["settings"].get("sandbox").is_none(),
            "不是作业起的运行：网络照设置"
        );

        let job = RunPolicy {
            job: "nightly".into(),
            network: port::Network::DenyAll,
        };
        let o = options_of(&spec, Some(&job));
        assert_eq!(o["settings"]["sandbox"]["network"]["mode"], "deny_all");
        assert_eq!(o["settings"]["permissions"]["deny"], json!(["CronCreate"]));
        assert_eq!(o, options_of(&spec, Some(&job)), "同一次运行再开一样");
        let inherit = RunPolicy {
            network: port::Network::Inherit,
            ..job
        };
        let o = options_of(&spec, Some(&inherit));
        assert!(o["settings"].get("sandbox").is_none());
    }
}
