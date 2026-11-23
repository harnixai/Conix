//! `Hub` 对着脚本化的假引擎：端口命令变成哪些 Agix 调用，Agix 的帧变成哪些领域事件、推给谁。

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use host::hub::Registry;
use host::{Engine, Hub, HubConfig, Notification, RpcError, Service};
use port::link::ServerFrame;
use port::{Answer, Command, Event, PromptId, Reply, SessionId, SubmitMode};
use serde_json::{Value, json};
use tokio::sync::mpsc;

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

fn default_handler(method: &str, p: &Value) -> Result<Value, RpcError> {
    Ok(match method {
        "daemon.ping" => json!({"version": "0.4.0", "protocol_version": 5,
            "capabilities": ["session_settings", "config_explain"]}),
        "session.list" => json!({"sessions": [
            {"session_id":"s1","name":"修登录","status":"active","session_kind":"primary",
             "scene":"code","project_root":PROJECT,"last_active":"2026-11-16T10:00:00Z","created_at":""},
            {"session_id":"s2","name":null,"status":"inactive","session_kind":"primary",
             "scene":"code","project_root":PROJECT,"last_active":"","created_at":""},
            {"session_id":"x9","name":"别的项目","status":"active","session_kind":"primary",
             "scene":"code","project_root":"/elsewhere","last_active":"","created_at":""}]}),
        "session.subscribe" => json!({"session_id": p["session_id"], "last_seq": 0,
            "pending_prompts": [], "pending_host_calls": [], "resumed": false}),
        "session.create" => json!({"session_id":"new1","scene":"code"}),
        "session.history" => json!({"messages": [
            {"role":"user","origin":"user","content":[{"type":"text","text":"你好"}]},
            {"role":"assistant","content":[{"type":"text","text":"在"}]}], "total": 2}),
        "session.get" => json!({"session_id": p["session_id"], "turn_state":"idle"}),
        "project.status" => json!({"trust": "trusted"}),
        "session.goal" => json!({"goal": null}),
        "session.tasks" => json!({"tasks": []}),
        "session.run_turn" => {
            json!({"session_id": p["session_id"], "turn_id": p["turn_id"], "delivery":"turn","name":null})
        }
        _ => json!({}),
    })
}

struct Rig {
    hub: Hub,
    fake: Arc<Fake>,
    notes: mpsc::UnboundedSender<Notification>,
    out: mpsc::UnboundedReceiver<ServerFrame>,
    _dir: tempfile::TempDir,
}

async fn rig_with(handler: Handler) -> Rig {
    rig_full(handler, Arc::new(|| 1_000_000)).await
}

/// 时钟可拨的：护栏的超时、到点触发要它。
async fn rig_clocked(handler: Handler) -> (Rig, Arc<std::sync::atomic::AtomicU64>) {
    let now = Arc::new(std::sync::atomic::AtomicU64::new(1_763_400_000_000));
    let n = now.clone();
    let r = rig_full(
        handler,
        Arc::new(move || n.load(std::sync::atomic::Ordering::SeqCst)),
    )
    .await;
    (r, now)
}

async fn rig_full(handler: Handler, clock: host::hub::Clock) -> Rig {
    let fake = Arc::new(Fake {
        calls: Mutex::default(),
        handler,
    });
    let (notes, rx) = mpsc::unbounded_channel();
    let dir = tempfile::tempdir().unwrap();
    let store = host::jobs::JobStore::open(
        dir.path().join("jobs.json"),
        dir.path().join("jobs-state.json"),
        dir.path().join("jobs"),
    );
    let hub = Hub::start(
        fake.clone(),
        rx,
        HubConfig {
            instance: "app".into(),
            host: "box".into(),
            project: PROJECT.into(),
            version: "0".into(),
            started_ms: 1,
            registry: Registry::open(dir.path().join("sessions.json")),
            jobs: Some(store),
            clock,
        },
    )
    .await
    .unwrap();
    let (tx, out) = mpsc::unbounded_channel();
    hub.attach(Some(tx));
    Rig {
        hub,
        fake,
        notes,
        out,
        _dir: dir,
    }
}

async fn rig() -> Rig {
    rig_with(Box::new(default_handler)).await
}

impl Rig {
    fn frame(&self, sid: &str, ev: Value) {
        self.notes
            .send(Notification::Session {
                session_id: sid.into(),
                turn_id: None,
                event: ev,
            })
            .unwrap();
    }

    /// 等通知任务处理完，把这期间推出的事件都取出来。
    async fn events(&mut self) -> Vec<Event> {
        for _ in 0..20 {
            tokio::task::yield_now().await;
        }
        let mut v = vec![];
        while let Ok(f) = self.out.try_recv() {
            if let ServerFrame::Event { event } = f {
                v.push(event);
            }
        }
        v
    }

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
}

#[tokio::test]
async fn lists_only_this_projects_sessions_and_subscribes_the_live_ones() {
    let r = rig().await;
    let Reply::Sessions(v) = r
        .hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let ids: Vec<_> = v.iter().map(|s| s.id.0.as_str()).collect();
    assert_eq!(ids, ["s1", "s2"], "按最近活动排序，别的项目的不列");
    assert_eq!(v[0].title, "修登录");
    let subs = r.calls("session.subscribe");
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0]["session_id"], "s1");
    let (info, flows, jobs) = r.hub.hello();
    assert_eq!((info.sessions, flows, jobs), (2, false, true));
}

#[tokio::test]
async fn turn_details_reach_only_the_watched_session() {
    let mut r = rig().await;
    r.frame("s2", json!({"kind":"turn_start"}));
    r.frame("s2", json!({"kind":"text_delta","text":"不该推"}));
    let ev = r.events().await;
    assert!(
        ev.iter().all(|e| matches!(e, Event::SessionUpserted(_))),
        "没 Watch 的会话只推摘要：{ev:?}"
    );

    let Reply::Watched(w) = r
        .hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(w.history.len(), 2);
    r.frame("s1", json!({"kind":"turn_start"}));
    r.frame("s1", json!({"kind":"step_start","step":0}));
    r.frame("s1", json!({"kind":"text_delta","text":"先看"}));
    r.frame("s1", json!({"kind":"step_start","step":0}));
    r.frame(
        "s1",
        json!({"kind":"tool_use","id":"t1","name":"TodoWrite",
                   "input":{"todos":[{"content":"读代码","status":"in_progress"}]}}),
    );
    r.frame("s1", json!({"kind":"brand_new_kind"}));
    r.frame(
        "s1",
        json!({"kind":"turn_complete","stop_reason":"end_turn",
                   "usage":{"input_tokens":10,"output_tokens":5}}),
    );
    let ev = r.events().await;
    let kinds: Vec<_> = ev
        .iter()
        .filter(|e| !matches!(e, Event::SessionUpserted(_)))
        .map(|e| {
            format!("{e:?}")
                .split([' ', '{', '('])
                .next()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "TurnStarted",
            "TextDelta",
            "StepRestarted",
            "ToolStarted",
            "ToolInput",
            "TodosChanged",
            "TurnEnded"
        ]
    );
    let last = ev
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::SessionUpserted(s) => Some(s.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(last.usage.input, 10);
    assert_eq!(last.state, port::SessionState::Idle);
}

#[tokio::test]
async fn question_answers_use_option_keys_and_the_parent_session() {
    let mut r = rig().await;
    r.hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap();
    r.frame(
        "s1",
        json!({"kind":"subagent_progress","agent_label":"探索","agent_session_id":"c1",
            "agent_type":"explore","parent_turn":1,"task_id":"tk1",
            "event":{"kind":"prompt","prompt_type":"question","prompt_id":"p1","tool_use_id":"x",
                     "question":"哪个分支？","header":null,
                     "options":[{"key":"a","label":"main"},{"key":"b","label":"dev"}]}}),
    );
    let ev = r.events().await;
    let pending = ev
        .iter()
        .find_map(|e| match e {
            Event::PromptPending { session, prompt } => Some((session.clone(), prompt.clone())),
            _ => None,
        })
        .expect("提问推出来了");
    assert_eq!(pending.0.0, "s1", "子 Agent 的提问挂在父会话上");
    assert_eq!(pending.1.from.unwrap().label, "探索");
    r.hub
        .handle(Command::Answer {
            session: SessionId("s1".into()),
            prompt: PromptId("p1".into()),
            answer: Answer::Choice { index: 1 },
        })
        .await
        .unwrap();
    let a = &r.calls("session.respondToPrompt")[0];
    assert_eq!(a["session_id"], "s1");
    assert_eq!(a["answer"], json!({"type":"answer","text":"b"}));
}

#[tokio::test]
async fn a_failed_run_turn_is_a_failed_turn() {
    let mut r = rig_with(Box::new(|m, p| {
        if m == "session.run_turn" {
            Err(RpcError::Remote {
                code: -32002,
                message: "no_credentials: no API key".into(),
                data: Value::Null,
            })
        } else {
            default_handler(m, p)
        }
    }))
    .await;
    r.hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap();
    let reply = r
        .hub
        .handle(Command::Submit {
            session: SessionId("s1".into()),
            text: "hi".into(),
            mode: SubmitMode::Turn,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Submitted { .. }));
    let ev = r.events().await;
    assert!(
        ev.iter().any(
            |e| matches!(e, Event::TurnFailed { message, .. } if message.contains("no_credentials"))
        ),
        "{ev:?}"
    );
    assert_eq!(r.calls("session.run_turn")[0]["on_busy"], "reject");
}

fn tool_call(sid: &str, call_id: &str, name: &str, input: Value) -> Value {
    json!({"kind":"host_tool_call","call_id":call_id,"tool_use_id":"t","name":name,
        "input":input,"timeout_ms":1000,"task_id":null,"agent_session_id":null,"cwd":PROJECT,"depth":0,
        "session_id":sid})
}

impl Rig {
    /// 工具的回答：`(是否出错, 文本)`。
    fn answer(&self, call_id: &str) -> (bool, String) {
        let r = self
            .calls("session.respondToolCall")
            .into_iter()
            .find(|c| c["call_id"] == call_id)
            .unwrap_or_else(|| panic!("{call_id} 没有回答"));
        (
            r["result"]["is_error"].as_bool().unwrap(),
            r["result"]["content"].as_str().unwrap().to_string(),
        )
    }
}

#[tokio::test]
async fn host_tools_hang_on_the_connection_and_a_user_session_saves_jobs() {
    let mut r = rig().await;
    let set = &r.calls("daemon.setHostTools")[0];
    let names: Vec<&str> = set["host_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"job_save") && names.contains(&"session_rename"));
    r.hub
        .handle(Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: None,
        })
        .await
        .unwrap();
    let create = &r.calls("session.create")[0];
    assert!(
        create["options"].get("permission_rules").is_none()
            && create["options"]["settings"]["permissions"]["allow"].is_null(),
        "查看类工具不用每次问，写在 cli 层（host::hub::cli_settings），不随会话下发"
    );
    assert!(
        create["options"]["host_tools"].is_null(),
        "不再按会话挂工具"
    );

    r.frame(
        "s1",
        tool_call(
            "s1",
            "hc1",
            "job_save",
            json!({"name":"nightly","cron":"0 2 * * *","prompt":"跑测试"}),
        ),
    );
    let ev = r.events().await;
    let job = ev
        .iter()
        .find_map(|e| match e {
            Event::JobUpserted(j) => Some(j.clone()),
            _ => None,
        })
        .expect("保存的作业推出来了");
    let (err, text) = r.answer("hc1");
    assert!(!err, "{text}");
    assert!(text.contains(&job.id.0), "回显名字与 ID：{text}");

    r.hub
        .handle(Command::RunJobNow {
            job: job.id.clone(),
            subject: None,
            prompt_append: None,
        })
        .await
        .unwrap();
    let ev = r.events().await;
    assert!(ev.iter().any(|e| matches!(e, Event::JobFired { entry, .. }
        if entry.outcome == port::JobOutcome::Running)));
    let fired = r.calls("session.create").last().cloned().unwrap();
    assert_eq!(fired["options"]["attended"], false, "作业无人值守");
    let turn = r.calls("session.run_turn").last().cloned().unwrap();
    assert_eq!(turn["message"], "跑测试");

    r.frame("new1", json!({"kind":"turn_start"}));
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"end_turn","usage":{"output_tokens":7}}),
    );
    let ev = r.events().await;
    let done = ev
        .iter()
        .find_map(|e| match e {
            Event::JobFired { entry, .. } => Some(entry.clone()),
            _ => None,
        })
        .expect("结局补上了");
    assert_eq!(done.outcome, port::JobOutcome::Done);
    assert_eq!(done.usage.output, 7);
    let Reply::JobHistory(h) = r
        .hub
        .handle(Command::JobHistory {
            job: job.id,
            before_ms: None,
            limit: None,
            outcome: None,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(h.len(), 1, "开始与结局是同一条记录");
}

#[tokio::test]
async fn a_busy_session_returns_the_message_instead_of_failing_the_turn() {
    let mut r = rig_with(Box::new(|m, p| {
        if m == "session.run_turn" {
            Err(RpcError::Remote {
                code: -32015,
                message: "session is busy".into(),
                data: Value::Null,
            })
        } else {
            default_handler(m, p)
        }
    }))
    .await;
    let Reply::Submitted { message } = r
        .hub
        .handle(Command::Submit {
            session: SessionId("s1".into()),
            text: "再来".into(),
            mode: SubmitMode::Turn,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let ev = r.events().await;
    assert!(
        ev.iter().any(
            |e| matches!(e, Event::SteerReturned { messages, .. } if messages == std::slice::from_ref(&message))
        ),
        "{ev:?}"
    );
    assert!(!ev.iter().any(|e| matches!(e, Event::TurnFailed { .. })));
}

#[tokio::test]
async fn a_mid_turn_snapshot_adds_only_the_unpersisted_step() {
    let mut r = rig().await;
    r.frame("s1", json!({"kind":"turn_start"}));
    r.frame("s1", json!({"kind":"step_start","step":0}));
    r.frame("s1", json!({"kind":"text_delta","text":"先看看"}));
    r.frame(
        "s1",
        json!({"kind":"step_end","step":0,"stop_reason":"tool_use","tool_calls":1,"seq":3,"usage":{}}),
    );
    r.frame("s1", json!({"kind":"step_start","step":1}));
    r.frame("s1", json!({"kind":"text_delta","text":"找到了"}));
    r.events().await;
    let Reply::Watched(w) = r
        .hub
        .handle(Command::Watch {
            session: SessionId("s1".into()),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(w.running);
    assert_eq!(
        w.history.last(),
        Some(&port::HistoryItem::Assistant {
            text: "找到了".into()
        }),
        "落了盘的那一步在历史里，不再补一遍"
    );
}

#[tokio::test]
async fn permission_modes_are_set_on_the_session_and_its_frames_keep_them() {
    let mut r = rig_with(Box::new(|m, p| match m {
        "session.setPermissionMode" => {
            // 设置不许 bypass：夹到 default。
            let (mode, clamped) = match p["mode"].as_str() {
                Some("bypassPermissions") => ("default", true),
                Some(x) => (x, false),
                None => ("default", false),
            };
            Ok(json!({"session_id": p["session_id"], "mode": mode,
                      "requested": p["mode"], "clamped": clamped}))
        }
        _ => default_handler(m, p),
    }))
    .await;
    let reply = r
        .hub
        .handle(Command::SetPermissionMode {
            session: SessionId("s1".into()),
            mode: port::PermissionMode::Plan,
        })
        .await
        .unwrap();
    assert_eq!(
        reply,
        Reply::PermissionMode {
            mode: port::PermissionMode::Plan,
            clamped: false
        }
    );
    assert_eq!(r.calls("session.setPermissionMode")[0]["mode"], "plan");
    let ev = r.events().await;
    assert!(
        ev.iter().any(|e| matches!(e, Event::SessionUpserted(s)
            if s.id.0 == "s1" && s.permission == port::PermissionMode::Plan)),
        "{ev:?}"
    );

    // 模型自己 ExitPlanMode / EnterPlanMode 也推帧：摘要跟着变。
    r.frame(
        "s1",
        json!({"kind":"permission_mode_changed","mode":"acceptEdits","source":"model"}),
    );
    let ev = r.events().await;
    assert!(
        ev.iter().any(|e| matches!(e, Event::SessionUpserted(s)
            if s.id.0 == "s1" && s.permission == port::PermissionMode::AcceptEdits)),
        "{ev:?}"
    );

    let Reply::PermissionMode { mode, clamped } = r
        .hub
        .handle(Command::SetPermissionMode {
            session: SessionId("s1".into()),
            mode: port::PermissionMode::BypassPermissions,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        (mode, clamped),
        (port::PermissionMode::Default, true),
        "以实际生效的为准"
    );

    // 草稿里选的模式随建会话带上；Submit 不再带模式。
    r.hub
        .handle(Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: Some(port::PermissionMode::AcceptEdits),
        })
        .await
        .unwrap();
    let create = r.calls("session.create").last().cloned().unwrap();
    assert_eq!(
        create["options"]["settings"]["permissions"]["mode"],
        "acceptEdits"
    );
}

#[tokio::test]
async fn late_frames_do_not_resurrect_a_closed_session() {
    let mut r = rig().await;
    r.hub
        .handle(Command::Close {
            session: SessionId("s2".into()),
        })
        .await
        .unwrap();
    r.frame(
        "s2",
        json!({"kind":"task_finished","task":{"task_id":"t1","kind":"shell","status":"stopped",
            "started_ms":0,"detail":{"command":"sleep 9"}}}),
    );
    r.events().await;
    let Reply::Sessions(v) = r
        .hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(v.iter().all(|s| s.id.0 != "s2"), "{v:?}");
}

#[tokio::test]
async fn a_job_fired_session_may_change_only_its_own_job() {
    let mut r = rig().await;
    let save = |name: &str| port::Job {
        id: port::JobId(String::new()),
        name: name.into(),
        cron: "0 2 * * *".into(),
        kind: port::JobKind::Simple {
            prompt: "跑测试".into(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up: false,
        created_ms: 0,
        guard: Default::default(),
        auto_disabled: None,
        effective_guard: None,
        next_fire_ms: None,
        last: None,
        running_ms: None,
        today_tokens: 0,
    };
    let Reply::Job(mine) = r
        .hub
        .handle(Command::SaveJob { job: save("mine") })
        .await
        .unwrap()
    else {
        panic!()
    };
    let Reply::Job(other) = r
        .hub
        .handle(Command::SaveJob { job: save("other") })
        .await
        .unwrap()
    else {
        panic!()
    };
    r.hub
        .handle(Command::RunJobNow {
            job: mine.id.clone(),
            subject: None,
            prompt_append: None,
        })
        .await
        .unwrap();
    r.events().await;
    // 触发出来的会话是 new1（假引擎建的会话都叫这个）。
    r.frame(
        "new1",
        tool_call(
            "new1",
            "a",
            "job_enable",
            json!({"target": other.id.0, "enabled": false}),
        ),
    );
    r.frame(
        "new1",
        tool_call(
            "new1",
            "b",
            "job_enable",
            json!({"target": mine.id.0, "enabled": false}),
        ),
    );
    r.frame(
        "new1",
        tool_call(
            "new1",
            "c",
            "job_save",
            json!({"name":"x","cron":"* * * * *","prompt":"p"}),
        ),
    );
    r.frame(
        "new1",
        tool_call(
            "new1",
            "d",
            "session_rename",
            json!({"target": "s1", "title": "改别人"}),
        ),
    );
    r.frame(
        "new1",
        tool_call("new1", "e", "session_rename", json!({"title": "改自己"})),
    );
    // 子 Agent 的调用经根会话来，作用域也是根会话的。
    r.frame(
        "new1",
        json!({"kind":"subagent_progress","agent_label":"子","agent_session_id":"c9","task_id":"t9",
            "event": tool_call("new1", "f", "job_delete", json!({"target": other.id.0}))}),
    );
    r.frame("zz", tool_call("zz", "g", "job_list", json!({})));
    r.frame("s1", tool_call("s1", "h", "job_list", json!({})));
    r.events().await;
    for (id, ok) in [
        ("a", false),
        ("b", true),
        ("c", false),
        ("d", false),
        ("e", true),
        ("f", false),
        ("g", false),
        ("h", true),
    ] {
        let (err, text) = r.answer(id);
        assert_eq!(!err, ok, "{id}: {text}");
    }
    let (_, list) = r.answer("h");
    assert!(
        list.starts_with("#1  ") && list.contains("#2  "),
        "按 #n 列：{list}"
    );
    assert!(list.contains("mine") && list.contains("other"));
}

/// 作业定义随项目提交，项目受信才读、才写；`/trust` 之后出现，取消信任就收起来（architecture §8.5）。
#[tokio::test]
async fn jobs_follow_the_projects_trust() {
    let trusted = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let t = trusted.clone();
    let mut r = rig_with(Box::new(move |m, p| match m {
        "project.status" => Ok(json!({"trust":
            if t.load(std::sync::atomic::Ordering::SeqCst) { "trusted" } else { "untrusted" }})),
        "project.trust" => {
            t.store(
                p["trusted"].as_bool().unwrap_or(false),
                std::sync::atomic::Ordering::SeqCst,
            );
            Ok(json!({}))
        }
        _ => default_handler(m, p),
    }))
    .await;
    let job = port::Job {
        id: port::JobId(String::new()),
        name: "nightly".into(),
        cron: "0 2 * * *".into(),
        kind: port::JobKind::Simple {
            prompt: "跑测试".into(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up: false,
        created_ms: 0,
        guard: Default::default(),
        auto_disabled: None,
        effective_guard: None,
        next_fire_ms: None,
        last: None,
        running_ms: None,
        today_tokens: 0,
    };
    let refused = r.hub.handle(Command::SaveJob { job: job.clone() }).await;
    assert!(refused.is_err(), "不受信的项目存不进作业：{refused:?}");

    r.hub
        .handle(Command::TrustProject { trusted: true })
        .await
        .unwrap();
    let Reply::Job(saved) = r.hub.handle(Command::SaveJob { job }).await.unwrap() else {
        panic!("SaveJob 回的不是作业");
    };
    assert_eq!(saved.created_ms, 1_000_000, "新建的作业记下创建时刻");
    r.events().await;

    r.hub
        .handle(Command::TrustProject { trusted: false })
        .await
        .unwrap();
    let ev = r.events().await;
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::JobRemoved { job } if *job == saved.id)),
        "{ev:?}"
    );
    let Reply::Jobs(left) = r.hub.handle(Command::ListJobs).await.unwrap() else {
        panic!("ListJobs 回的不是作业列表");
    };
    assert!(left.is_empty());
}

/// 名字写在引擎里：引擎没改成就回错、本地不改；冷会话关闭时引擎答"不存在"，照样算关了，别的错误回给人。
#[tokio::test]
async fn rename_and_close_follow_what_the_engine_did() {
    let r = rig_with(Box::new(|m, p| match m {
        "session.rename" | "session.close" if p["session_id"] == "s1" => Err(RpcError::Remote {
            code: -32603,
            message: "disk full".into(),
            data: Value::Null,
        }),
        "session.close" if p["session_id"] == "s2" => Err(RpcError::Remote {
            code: -32000,
            message: "not in memory".into(),
            data: Value::Null,
        }),
        _ => default_handler(m, p),
    }))
    .await;
    let titles = |v: &[port::SessionSummary]| -> Vec<(String, String)> {
        v.iter()
            .map(|s| (s.id.0.clone(), s.title.clone()))
            .collect()
    };
    let list = |r: &Rig| {
        let hub = r.hub.clone();
        async move {
            let Reply::Sessions(v) = hub
                .handle(Command::ListSessions {
                    filter: Default::default(),
                })
                .await
                .unwrap()
            else {
                panic!()
            };
            v
        }
    };
    let before = titles(&list(&r).await);
    let renamed = r
        .hub
        .handle(Command::Rename {
            session: SessionId("s1".into()),
            title: "新名字".into(),
        })
        .await;
    assert!(renamed.is_err(), "{renamed:?}");
    assert_eq!(titles(&list(&r).await), before, "引擎没改成，本地也不改");

    let closed = r
        .hub
        .handle(Command::Close {
            session: SessionId("s1".into()),
        })
        .await;
    assert!(closed.is_err(), "{closed:?}");
    assert!(
        list(&r).await.iter().any(|s| s.id.0 == "s1"),
        "没关成就还在"
    );

    r.hub
        .handle(Command::Close {
            session: SessionId("s2".into()),
        })
        .await
        .expect("冷会话照样关");
    assert!(list(&r).await.iter().all(|s| s.id.0 != "s2"));
}

fn simple_job(name: &str, prompt: &str) -> port::Job {
    port::Job {
        id: port::JobId(String::new()),
        name: name.into(),
        cron: "0 2 * * *".into(),
        kind: port::JobKind::Simple {
            prompt: prompt.into(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up: false,
        created_ms: 0,
        guard: Default::default(),
        auto_disabled: None,
        effective_guard: None,
        next_fire_ms: None,
        last: None,
        running_ms: None,
        today_tokens: 0,
    }
}

/// 校验不落盘：回下几次触发、护栏的生效值（作业写的超过天花板就压下来）、存不进去的原因。
#[tokio::test]
async fn validating_a_job_saves_nothing_and_says_what_would_happen() {
    let r = rig().await;
    let mut j = simple_job("nightly", "跑测试");
    j.guard.timeout_ms = Some(48 * 3_600_000);
    let Reply::JobCheck(c) = r
        .hub
        .handle(Command::ValidateJob { job: j.clone() })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(c.problems.is_empty(), "{:?}", c.problems);
    assert_eq!(c.next_fire_ms.len(), 5);
    assert_eq!(c.guard.timeout_ms, Some(4 * 3_600_000), "压到天花板");
    assert_eq!(c.capped, ["timeout"]);
    assert_eq!(c.guard.max_tokens, Some(2_000_000), "没写的用缺省");

    j.cron = "every day".into();
    j.kind = port::JobKind::Flow {
        flow: "#1".into(),
        subject: String::new(),
        inputs: Value::Null,
    };
    let Reply::JobCheck(c) = r.hub.handle(Command::ValidateJob { job: j }).await.unwrap() else {
        panic!()
    };
    assert!(
        c.problems.len() >= 3,
        "cron、主题、流程都不行：{:?}",
        c.problems
    );
    let Reply::Jobs(v) = r.hub.handle(Command::ListJobs).await.unwrap() else {
        panic!()
    };
    assert!(v.is_empty(), "校验不存");
}

/// 停下正在跑的那一次：打断它的会话，那一轮结束时记成手动停下，不算失败的原因写错。
#[tokio::test]
async fn stopping_a_running_firing_interrupts_its_session_and_records_why() {
    let mut r = rig().await;
    let Reply::Job(job) = r
        .hub
        .handle(Command::SaveJob {
            job: simple_job("nightly", "跑测试"),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    r.hub
        .handle(Command::RunJobNow {
            job: job.id.clone(),
            subject: None,
            prompt_append: Some("只看昨天的".into()),
        })
        .await
        .unwrap();
    r.events().await;
    let turn = r.calls("session.run_turn").last().cloned().unwrap();
    assert_eq!(turn["message"], "跑测试\n\n只看昨天的", "这一次补的一句");
    r.frame("new1", json!({"kind":"turn_start"}));
    r.events().await;
    let Reply::Jobs(v) = r.hub.handle(Command::ListJobs).await.unwrap() else {
        panic!()
    };
    assert!(v[0].running_ms.is_some(), "列表看得出正在跑");

    r.hub
        .handle(Command::StopFiring {
            job: job.id.clone(),
            fired_ms: None,
        })
        .await
        .unwrap();
    assert_eq!(r.calls("session.interrupt").len(), 1);
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"cancelled","usage":{"output_tokens":3}}),
    );
    let ev = r.events().await;
    let entry = ev
        .iter()
        .find_map(|e| match e {
            Event::JobFired { entry, .. } => Some(entry.clone()),
            _ => None,
        })
        .expect("结局补上了");
    assert_eq!(
        entry.outcome,
        port::JobOutcome::Stopped {
            reason: port::StopCause::Manual
        }
    );
    let again = r
        .hub
        .handle(Command::StopFiring {
            job: job.id,
            fired_ms: None,
        })
        .await;
    assert!(again.is_err(), "没有在跑的就说清楚：{again:?}");
}

fn guarded(
    cfg: impl FnOnce(&mut host::jobs::guard::GuardConfig),
) -> host::jobs::guard::GuardConfig {
    let mut g = host::jobs::guard::GuardConfig::default();
    cfg(&mut g);
    g
}

async fn save_job(r: &Rig, j: port::Job) -> port::Job {
    let Reply::Job(saved) = r.hub.handle(Command::SaveJob { job: j }).await.unwrap() else {
        panic!()
    };
    saved
}

/// 立即跑一次，等触发真的起来（它在自己的任务里建会话、发第一轮）。
async fn run_now(r: &Rig, job: &port::JobId) {
    r.hub
        .handle(Command::RunJobNow {
            job: job.clone(),
            subject: None,
            prompt_append: None,
        })
        .await
        .unwrap();
    for _ in 0..50 {
        tokio::task::yield_now().await;
    }
}

fn outcomes(ev: &[Event]) -> Vec<port::JobOutcome> {
    ev.iter()
        .filter_map(|e| match e {
            Event::JobFired { entry, .. } => Some(entry.outcome.clone()),
            _ => None,
        })
        .collect()
}

/// 单次 token 上限：按每次调用的用量记账，超了就打断，结局记成超额；用量进了今天的账。
#[tokio::test]
async fn a_firing_over_its_token_limit_is_stopped() {
    let mut r = rig().await;
    let mut j = simple_job("nightly", "跑测试");
    j.guard.max_tokens = Some(1_000);
    let job = save_job(&r, j).await;
    run_now(&r, &job.id).await;
    r.frame("new1", json!({"kind":"turn_start"}));
    r.frame(
        "new1",
        json!({"kind":"step_end","usage":{"input_tokens":500,"output_tokens":100}}),
    );
    r.events().await;
    assert!(r.calls("session.interrupt").is_empty(), "还没超");
    r.frame(
        "new1",
        json!({"kind":"step_end","usage":{"input_tokens":500,"output_tokens":100}}),
    );
    r.events().await;
    assert_eq!(r.calls("session.interrupt").len(), 1, "超了就打断");
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"cancelled"}),
    );
    let ev = r.events().await;
    assert!(
        outcomes(&ev).contains(&port::JobOutcome::Stopped {
            reason: port::StopCause::Budget
        }),
        "{ev:?}"
    );
    let Reply::Jobs(v) = r.hub.handle(Command::ListJobs).await.unwrap() else {
        panic!()
    };
    assert_eq!(v[0].today_tokens, 1_200, "进了今天的账");
}

/// 超时：定时检查发现跑过了作业的超时，打断它，记成超时。
#[tokio::test(start_paused = true)]
async fn a_firing_past_its_timeout_is_stopped() {
    let (mut r, now) = rig_clocked(Box::new(default_handler)).await;
    let mut j = simple_job("nightly", "跑测试");
    j.guard.timeout_ms = Some(60_000);
    let job = save_job(&r, j).await;
    run_now(&r, &job.id).await;
    r.frame("new1", json!({"kind":"turn_start"}));
    r.events().await;
    now.fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_secs(11)).await;
    r.events().await;
    assert_eq!(r.calls("session.interrupt").len(), 1, "超时打断");
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"cancelled"}),
    );
    let ev = r.events().await;
    assert!(outcomes(&ev).contains(&port::JobOutcome::Stopped {
        reason: port::StopCause::Timeout
    }));
}

/// 到点时上一次还在跑：缺省跳过，记一条"上一次还在跑"。人立即运行的不受这个管。
#[tokio::test(start_paused = true)]
async fn a_due_firing_is_skipped_while_the_previous_one_runs() {
    let (mut r, now) = rig_clocked(Box::new(default_handler)).await;
    let mut j = simple_job("every-minute", "跑测试");
    j.cron = "* * * * *".into();
    let job = save_job(&r, j).await;
    run_now(&r, &job.id).await;
    r.frame("new1", json!({"kind":"turn_start"}));
    r.events().await;
    now.fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_secs(61)).await;
    let ev = r.events().await;
    assert!(
        outcomes(&ev).contains(&port::JobOutcome::Skipped {
            reason: port::SkipCause::Overlap
        }),
        "{ev:?}"
    );
    assert_eq!(r.calls("session.create").len(), 1, "没有再开一个会话");
}

/// 每日额度用完：在跑的停下、记成额度用完，说一次；之后到点的都跳过。
#[tokio::test(start_paused = true)]
async fn the_daily_budget_stops_running_work_and_skips_the_rest() {
    let (mut r, now) = rig_clocked(Box::new(default_handler)).await;
    r.hub.set_guard_config(guarded(|g| g.daily_tokens = 1_000));
    let mut j = simple_job("every-minute", "跑测试");
    j.cron = "* * * * *".into();
    let job = save_job(&r, j).await;
    run_now(&r, &job.id).await;
    r.frame("new1", json!({"kind":"turn_start"}));
    r.frame(
        "new1",
        json!({"kind":"step_end","usage":{"input_tokens":900,"output_tokens":200}}),
    );
    let ev = r.events().await;
    assert!(
        ev.iter()
            .any(|e| matches!(e, Event::Notice { text, .. } if text.contains("budget"))),
        "{ev:?}"
    );
    assert_eq!(r.calls("session.interrupt").len(), 1);
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"cancelled"}),
    );
    let ev = r.events().await;
    assert!(outcomes(&ev).contains(&port::JobOutcome::Stopped {
        reason: port::StopCause::DailyBudget
    }));
    now.fetch_add(61_000, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(std::time::Duration::from_secs(61)).await;
    let ev = r.events().await;
    assert!(
        outcomes(&ev).contains(&port::JobOutcome::Skipped {
            reason: port::SkipCause::DailyBudget
        }),
        "{ev:?}"
    );
}

/// 连续失败到了上限：在这台机器上停用，标出来；启用时清掉。
#[tokio::test]
async fn repeated_failures_disable_the_job_on_this_machine() {
    let (mut r, now) = rig_clocked(Box::new(|m, p| match m {
        "session.run_turn" => Err(RpcError::Remote {
            code: -32002,
            message: "no_credentials: no API key".into(),
            data: Value::Null,
        }),
        _ => default_handler(m, p),
    }))
    .await;
    let mut j = simple_job("nightly", "跑测试");
    j.guard.disable_after = Some(2);
    let job = save_job(&r, j).await;
    run_now(&r, &job.id).await;
    r.events().await;
    now.fetch_add(1_000, std::sync::atomic::Ordering::SeqCst);
    run_now(&r, &job.id).await;
    let ev = r.events().await;
    let last = ev
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::JobUpserted(j) => Some(j.clone()),
            _ => None,
        })
        .unwrap();
    assert!(!last.enabled && last.auto_disabled.as_ref().is_some_and(|a| a.after == 2));
    r.hub
        .handle(Command::SetJobEnabled {
            job: job.id.clone(),
            enabled: true,
        })
        .await
        .unwrap();
    let Reply::Jobs(v) = r.hub.handle(Command::ListJobs).await.unwrap() else {
        panic!()
    };
    assert!(v[0].enabled && v[0].auto_disabled.is_none(), "启用时清掉");
}

/// 没 TUI 接着时出的事攒成"离开期间"：重新接入时推一次，之后不再重复。
#[tokio::test]
async fn what_happened_while_detached_is_shown_on_attach() {
    let r = rig_with(Box::new(|m, p| match m {
        "session.run_turn" => Err(RpcError::Remote {
            code: -32002,
            message: "no_credentials: no API key".into(),
            data: Value::Null,
        }),
        _ => default_handler(m, p),
    }))
    .await;
    let job = save_job(&r, simple_job("nightly", "跑测试")).await;
    r.hub.attach(None);
    run_now(&r, &job.id).await;
    let (tx, mut out) = mpsc::unbounded_channel();
    r.hub.attach(Some(tx));
    let mut away = None;
    while let Ok(f) = out.try_recv() {
        if let ServerFrame::Event {
            event: Event::AwaySummary { alerts, .. },
        } = f
        {
            away = Some(alerts);
        }
    }
    let alerts = away.expect("接入时推了离开期间");
    assert!(
        alerts.iter().any(|a| a.kind == port::AlertKind::JobFailed
            && a.target == Some(port::AlertTarget::Job(job.id.clone()))),
        "{alerts:?}"
    );
    let (tx, mut out) = mpsc::unbounded_channel();
    r.hub.attach(Some(tx));
    while let Ok(f) = out.try_recv() {
        assert!(
            !matches!(
                f,
                ServerFrame::Event {
                    event: Event::AwaySummary { .. }
                }
            ),
            "只推一次"
        );
    }
}

/// 关掉的会话进「已关闭」：只读地看（读转录，不订阅、不放回会话表），可以重新打开，也可以删掉。
#[tokio::test]
async fn closed_sessions_are_listed_viewed_reopened_and_deleted() {
    let r = rig().await;
    let s2 = SessionId("s2".into());
    r.hub
        .handle(Command::Close {
            session: s2.clone(),
        })
        .await
        .unwrap();
    let Reply::Sessions(closed) = r.hub.handle(Command::ListClosedSessions).await.unwrap() else {
        panic!()
    };
    assert_eq!(
        closed.iter().map(|s| s.id.0.as_str()).collect::<Vec<_>>(),
        ["s2"]
    );

    let subs_before = r.calls("session.subscribe").len();
    let Reply::Watched(w) = r
        .hub
        .handle(Command::Watch {
            session: s2.clone(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!w.history.is_empty(), "读到了转录");
    assert_eq!(
        r.calls("session.subscribe").len(),
        subs_before,
        "只读：不订阅"
    );
    let Reply::Sessions(open) = r
        .hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(open.iter().all(|s| s.id != s2), "看过之后也不回到上面");

    r.hub
        .handle(Command::ReopenSession {
            session: s2.clone(),
        })
        .await
        .unwrap();
    let Reply::Sessions(open) = r
        .hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(open.iter().any(|s| s.id == s2), "重新打开回到上面");

    r.hub
        .handle(Command::Close {
            session: s2.clone(),
        })
        .await
        .unwrap();
    r.hub
        .handle(Command::DeleteSession {
            session: s2.clone(),
        })
        .await
        .unwrap();
    assert_eq!(r.calls("session.delete")[0]["session_id"], "s2");
}

/// 自检：引擎的检查原样带上，再加实例自己的几项；配置里读不懂的报成 degraded。
#[tokio::test]
async fn doctor_reports_the_engines_checks_and_the_instances_own() {
    let r = rig_with(Box::new(|m, p| match m {
        "daemon.doctor" => Ok(json!({"health": {"status": "ok", "checks": [
            {"name": "settings.tiers", "status": "ok", "summary": "every settings.json present parses"}]}})),
        _ => default_handler(m, p),
    }))
    .await;
    r.hub
        .config_problem("jobs.defaults.timeout: \"soon\" is not understood".into());
    let Reply::Doctor(checks) = r.hub.handle(Command::Doctor).await.unwrap() else {
        panic!()
    };
    let status = |n: &str| checks.iter().find(|c| c.name == n).map(|c| c.status);
    assert_eq!(status("settings.tiers"), Some(port::HealthStatus::Ok));
    assert_eq!(status("codix.config"), Some(port::HealthStatus::Degraded));
    assert_eq!(
        status("codix.flows"),
        Some(port::HealthStatus::Failing),
        "这个搭法里没有流程"
    );
    assert!(status("codix.notify").is_some() && status("codix.gitignore").is_some());
}

/// 无人值守的这一次做了什么记进触发记录：调了哪些工具各几次、出错几次、改了哪些文件。
#[tokio::test]
async fn a_firing_records_what_it_did() {
    let mut r = rig().await;
    let job = save_job(&r, simple_job("nightly", "跑测试")).await;
    run_now(&r, &job.id).await;
    r.frame("new1", json!({"kind":"turn_start"}));
    for (id, name) in [("a", "Bash"), ("b", "Bash"), ("c", "Write")] {
        r.frame(
            "new1",
            json!({"kind":"tool_use","id":id,"name":name,"input":{}}),
        );
    }
    r.frame(
        "new1",
        json!({"kind":"tool_result","id":"a","name":"Bash","content":"denied","is_error":true}),
    );
    r.frame(
        "new1",
        json!({"kind":"tool_result","id":"c","name":"Write","content":"ok",
               "file_changes":[{"path":"src/a.rs","op":"modify"}]}),
    );
    r.frame(
        "new1",
        json!({"kind":"turn_complete","stop_reason":"end_turn"}),
    );
    let ev = r.events().await;
    let audit = ev
        .iter()
        .find_map(|e| match e {
            Event::JobFired { entry, .. } if entry.outcome == port::JobOutcome::Done => {
                entry.audit.clone()
            }
            _ => None,
        })
        .expect("收尾的那条带着明细");
    assert_eq!(audit.tools.get("Bash"), Some(&2));
    assert_eq!(audit.tools.get("Write"), Some(&1));
    assert_eq!(audit.errors, 1);
    assert_eq!(audit.files, ["src/a.rs"]);
}
