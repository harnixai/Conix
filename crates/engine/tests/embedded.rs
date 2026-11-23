//! 真实嵌入的 Agix 加上 `Hub`：建会话、跑一轮、收到这一轮的结局。没有凭据时那一轮以
//! `no_credentials` 失败（Agix 0.3.17 起不发请求），所以这条用例离线也跑得动；带凭据的
//! 那条要真的调模型，标了 `ignore`，用 `source .env && cargo test -p engine -- --ignored` 跑。

use std::sync::Arc;
use std::time::Duration;

use host::hub::Registry;
use host::{Hub, HubConfig, Service};
use port::link::ServerFrame;
use port::{Command, Event, Reply, SubmitMode};
use tokio::sync::mpsc;

struct Rig {
    hub: Hub,
    engine: engine::Embedded,
    out: mpsc::UnboundedReceiver<ServerFrame>,
    _dirs: (tempfile::TempDir, tempfile::TempDir),
}

async fn rig() -> Rig {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let (engine, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: home.path().to_path_buf(),
        project: project_path.clone(),
        groups_file: home.path().join("groups.json"),
        settings: host::hub::cli_settings(),
        model: None,
    })
    .await
    .unwrap();
    let hub = Hub::start(
        Arc::new(client),
        notes,
        HubConfig {
            instance: "t".into(),
            host: "h".into(),
            project: project_path,
            version: "0".into(),
            started_ms: 0,
            registry: Registry::in_memory(),
            jobs: None,
            clock: Arc::new(|| 0),
        },
    )
    .await
    .unwrap();
    let (tx, out) = mpsc::unbounded_channel();
    hub.attach(Some(tx));
    Rig {
        hub,
        engine,
        out,
        _dirs: (home, project),
    }
}

/// 建会话、Watch、提交一句话，等这一轮结束（成功或失败）。
async fn one_turn(r: &mut Rig, text: &str) -> Vec<Event> {
    let Reply::Session(s) = r
        .hub
        .handle(Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: None,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    r.hub
        .handle(Command::Watch {
            session: s.id.clone(),
        })
        .await
        .unwrap();
    r.hub
        .handle(Command::Submit {
            session: s.id.clone(),
            text: text.into(),
            mode: SubmitMode::Turn,
        })
        .await
        .unwrap();
    let mut seen = vec![];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
    loop {
        let f = tokio::time::timeout_at(deadline, r.out.recv())
            .await
            .expect("这一轮在两分钟内结束")
            .unwrap();
        if let ServerFrame::Event { event } = f {
            let end = matches!(event, Event::TurnEnded { .. } | Event::TurnFailed { .. });
            seen.push(event);
            if end {
                return seen;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_turn_ends_with_or_without_credentials() {
    let mut r = rig().await;
    let ev = one_turn(&mut r, "Reply with the single word: pong").await;
    match ev.last().unwrap() {
        Event::TurnFailed { message, .. } => {
            assert!(message.contains("credential"), "{message}")
        }
        Event::TurnEnded { .. } => {}
        e => panic!("{e:?}"),
    }
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
    assert_eq!(v.len(), 1);
    r.engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "调真实的模型：先 source .env"]
async fn live_turn_streams_text() {
    let mut r = rig().await;
    let ev = one_turn(&mut r, "Reply with the single word: pong").await;
    let text: String = ev
        .iter()
        .filter_map(|e| match e {
            Event::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        matches!(ev.last(), Some(Event::TurnEnded { .. })),
        "{:?}",
        ev.last()
    );
    assert!(text.to_lowercase().contains("pong"), "{text}");
    r.engine.shutdown().await;
}
