//! 派工器的会话宿主对着假引擎：键幂等、帧原样分过来并翻对、`run_turn` 的错误也结束那一轮。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use harnix_flow::Delivery;
use harnix_flow::runner::{Frame, OpenSpec, SessionHost, TurnEnd};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::session_host::{CodixSessionHost, session_id_of};
use crate::engine::{Engine, Notification, RpcError};
use crate::hub::{Hub, HubConfig, Registry};

type Handler = Box<dyn Fn(&str, &Value) -> Result<Value, RpcError> + Send + Sync>;

struct Fake {
    calls: Mutex<Vec<(String, Value)>>,
    handler: Handler,
}

#[async_trait]
impl Engine for Fake {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        self.calls
            .lock()
            .unwrap()
            .push((method.to_string(), params.clone()));
        (self.handler)(method, &params)
    }
}

const PROJECT: &str = "/work/app";

fn handler(created: Arc<Mutex<Vec<String>>>) -> Handler {
    Box::new(move |method, p| {
        Ok(match method {
            "daemon.ping" => json!({"version": "0.4.0", "protocol_version": 5,
                "capabilities": ["session_settings", "config_explain"]}),
            "session.list" => json!({"sessions": []}),
            "session.create" => {
                let id = p["session_id"].as_str().unwrap().to_string();
                let mut c = created.lock().unwrap();
                let existing = c.contains(&id);
                c.push(id.clone());
                json!({"session_id": id, "scene": "code", "existing": existing,
                       "created_at": "2026-11-17T10:00:00Z"})
            }
            "session.subscribe" => json!({"session_id": p["session_id"], "last_seq": 0,
                "pending_prompts": [], "pending_host_calls": [], "resumed": false}),
            "session.run_turn" if p["message"] == "fail" => {
                return Err(RpcError::Remote {
                    code: -32000,
                    message: "no_credentials: none".into(),
                    data: json!({"kind": "no_credentials", "retryable": false}),
                });
            }
            "session.run_turn" => json!({"session_id": p["session_id"], "turn_id": p["turn_id"]}),
            _ => json!({}),
        })
    })
}

struct Rig {
    hub: Hub,
    fake: Arc<Fake>,
    notes: mpsc::UnboundedSender<Notification>,
}

async fn rig() -> Rig {
    let fake = Arc::new(Fake {
        calls: Mutex::default(),
        handler: handler(Arc::default()),
    });
    let (notes, rx) = mpsc::unbounded_channel();
    let hub = Hub::start(
        fake.clone(),
        rx,
        HubConfig {
            instance: "app".into(),
            host: "box".into(),
            project: PROJECT.into(),
            version: "0".into(),
            started_ms: 1,
            registry: Registry::in_memory(),
            jobs: None,
            clock: Arc::new(|| 1_000_000),
            engine_model: "test-model".into(),
        },
    )
    .await
    .unwrap();
    Rig { hub, fake, notes }
}

impl Rig {
    fn calls(&self, method: &str) -> Vec<Value> {
        self.fake
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _)| m == method)
            .map(|(_, p)| p.clone())
            .collect()
    }

    fn frame(&self, sid: &str, turn: &str, ev: Value) {
        self.notes
            .send(Notification::Session {
                session_id: sid.into(),
                turn_id: Some(turn.into()),
                event: ev,
            })
            .unwrap();
    }
}

fn spec(key: &str) -> OpenSpec {
    let mut s = OpenSpec::new(key, "r1", "code", Delivery::FlowSubmit);
    s.main = key.ends_with("@main");
    s.project_root = Some(PROJECT.into());
    s
}

#[tokio::test]
async fn a_key_opens_the_same_session_and_another_spec_for_it_is_refused() {
    let r = rig().await;
    let host = CodixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let a = host.open(spec("flow:r1@main")).await.unwrap();
    assert_eq!(a.id, session_id_of("flow:r1@main"));
    assert!(!a.existed);
    let again = host.open(spec("flow:r1@main")).await.unwrap();
    assert_eq!(again.id, a.id);
    assert!(again.existed, "引擎答的是那个会话");
    let create = &r.calls("session.create")[0];
    assert_eq!(create["session_id"], a.id);
    assert_eq!(create["project_root"], PROJECT);
    assert_eq!(
        create["options"]["settings"]["permissions"]["mode"],
        "acceptEdits"
    );
    let mut other = spec("flow:r1@main");
    other.profile = "plan".into();
    assert!(host.open(other).await.is_err(), "同一个键换了 spec");
    // 主会话的归属是流程运行。
    let (running, project) = r.hub.session_state(&a.id).unwrap();
    assert!(!running);
    assert_eq!(project.as_deref(), Some(PROJECT));
}

#[tokio::test]
async fn frames_reach_the_runner_with_the_text_that_started_the_turn() {
    let r = rig().await;
    let host = CodixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let id = host.open(spec("flow:r1@main")).await.unwrap().id;
    let mut w = host.watch(&id).await.unwrap();
    host.send(&id, "做这一站").await.unwrap();
    // 等 run_turn 发出去，拿到它的轮 id。
    let mut turn = None;
    for _ in 0..50 {
        tokio::task::yield_now().await;
        if let Some(c) = r.calls("session.run_turn").first() {
            turn = c["turn_id"].as_str().map(str::to_string);
            break;
        }
    }
    let turn = turn.expect("run_turn 发了");
    r.frame(&id, &turn, json!({"kind": "turn_start"}));
    r.frame(
        &id,
        &turn,
        json!({"kind": "tool_use", "id": "c1", "name": "flow_submit", "input": {}}),
    );
    r.frame(
        &id,
        &turn,
        json!({"kind": "turn_complete", "stop_reason": "end_turn", "usage": {"output_tokens": 2}}),
    );
    let f: Vec<Frame> = w.frames.by_ref().take(3).collect().await;
    assert_eq!(
        f[0],
        Frame::TurnStarted {
            turn: turn.clone(),
            text: "做这一站".into()
        }
    );
    assert!(matches!(&f[1], Frame::ToolCall { tool, .. } if tool == "flow_submit"));
    assert!(matches!(
        &f[2],
        Frame::TurnSettled {
            end: TurnEnd::Done,
            ..
        }
    ));

    // run_turn 回错：这一轮照样开始、结束，派工器认得是自己发的那句。
    host.send(&id, "fail").await.unwrap();
    let f: Vec<Frame> = w.frames.by_ref().take(2).collect().await;
    assert!(matches!(&f[0], Frame::TurnStarted { text, .. } if text == "fail"));
    assert!(
        matches!(&f[1], Frame::TurnSettled { end: TurnEnd::Error { retryable: false, kind, .. }, .. } if kind == "no_credentials")
    );
}

#[tokio::test]
async fn a_closed_session_is_gone_and_its_stream_ends() {
    let r = rig().await;
    let host = CodixSessionHost::new(
        r.hub.downgrade(),
        std::sync::Arc::new(super::policy::RunPolicies::in_memory()),
    );
    let id = host.open(spec("flow:r1/s/t@x")).await.unwrap().id;
    let mut w = host.watch(&id).await.unwrap();
    host.close(&id).await.unwrap();
    assert!(w.frames.next().await.is_none(), "看着它的流结束了");
    assert!(host.send(&id, "x").await.is_err());
    assert!(host.lookup(&id).await.unwrap().is_none());
}

/// 流程作业的流程：`#n` 按流程列表认成名字，名字照认，不在的说清楚，inputs 要合流程的 schema。
#[tokio::test]
async fn a_flow_jobs_flow_is_resolved_and_its_inputs_checked() {
    let r = rig().await;
    let dir = tempfile::tempdir().unwrap();
    let flows = super::start(
        &r.hub,
        super::FlowsConfig {
            flows_dir: dir.path().join("flows"),
            runs_dir: dir.path().join("runs"),
            worktrees_dir: dir.path().join(".wkts"),
            workspaces_dir: dir.path().join("ws"),
            policies_file: None,
        },
    )
    .await
    .unwrap();
    let all = flows.summaries().await;
    let first = port::order::flows(&all)[0].name.clone();
    use crate::hub::FlowPort;
    assert_eq!(
        flows.check_job_flow("#1", &Value::Null).await.unwrap(),
        first
    );
    assert_eq!(
        flows.check_job_flow(&first, &json!({})).await.unwrap(),
        first
    );
    assert!(
        flows
            .check_job_flow("no-such-flow", &Value::Null)
            .await
            .is_err()
    );
    assert!(
        flows.check_job_flow(&first, &json!(5)).await.is_err(),
        "inputs 不是对象，不合 schema"
    );
    flows.stop().await;
}
