use std::time::Duration;

use mock::{MockInstances, MockOptions, MockState, Scenario, ScriptedBackend, Seed};
use port::{
    Attach, Backend, Command, Event, Instances, PortError, Reply, SessionId, StopReason, SubmitMode,
};

const SEED: &str = r#"
sessions:
  - summary:
      id: s1
      title: 测试
      state: idle
      provenance: {instance: dev, host: h, scene: code, model: m}

plugin_packages:
  - {name: xforge, version: 0.1.0, path: /opt/p/xforge-0.1.0.zip}
"#;

fn backend(scenario: &str) -> std::sync::Arc<ScriptedBackend> {
    let seed = Seed::parse("t", SEED).unwrap();
    let instance = port::InstanceInfo {
        name: "dev".into(),
        host: "h".into(),
        pid: None,
        project: None,
        state: port::InstanceState::Running,
        started_ms: None,
        stopped_ms: None,
        version: String::new(),
        sessions: 1,
        waiting: 0,
        jobs: 0,
        holder: None,
        restarts: 0,
    };
    let sc = Scenario::parse("t", scenario).unwrap();
    ScriptedBackend::new(MockState::new(instance, seed), sc, MockOptions::default())
}

async fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<Event>, ms: u64) -> Vec<Event> {
    tokio::time::sleep(Duration::from_millis(ms)).await;
    let mut out = vec![];
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

fn s1() -> SessionId {
    "s1".into()
}

#[tokio::test(start_paused = true)]
async fn default_answer_streams_and_updates_the_summary() {
    let b = backend("seed: x");
    let mut rx = b.events();
    let reply = b
        .call(Command::Submit {
            session: s1(),
            text: "你好".into(),
            mode: SubmitMode::Turn,
        })
        .await
        .unwrap();
    assert!(matches!(reply, Reply::Submitted { .. }));
    let evs = drain(&mut rx, 5_000).await;
    assert!(matches!(evs.first(), Some(Event::TurnStarted { .. })));
    let text: String = evs
        .iter()
        .filter_map(|e| match e {
            Event::TextDelta { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "（mock）收到：你好");
    assert!(evs.iter().any(|e| matches!(
        e,
        Event::TurnEnded {
            stop: StopReason::EndTurn,
            ..
        }
    )));
    assert!(evs.iter().any(|e| matches!(e, Event::SessionUpserted(_))));
    let Reply::Watched(w) = b.call(Command::Watch { session: s1() }).await.unwrap() else {
        panic!()
    };
    assert!(!w.running);
    assert_eq!(
        w.history.len(),
        2,
        "快照与已推出的增量一致：用户的话 + 回答"
    );
}

#[tokio::test(start_paused = true)]
async fn interrupt_cancels_what_has_not_happened_yet() {
    let b = backend("seed: x");
    let mut rx = b.events();
    b.call(Command::Submit {
        session: s1(),
        text: "一段很长很长很长很长的话".into(),
        mode: SubmitMode::Turn,
    })
    .await
    .unwrap();
    let early = drain(&mut rx, 50).await;
    b.call(Command::Interrupt { session: s1() }).await.unwrap();
    let late = drain(&mut rx, 5_000).await;
    assert!(early.iter().any(|e| matches!(e, Event::TurnStarted { .. })));
    assert!(!late.iter().any(|e| matches!(e, Event::TextDelta { .. })));
    assert!(late.iter().any(|e| matches!(
        e,
        Event::TurnEnded {
            stop: StopReason::Cancelled,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn timeline_rules_and_session_substitution() {
    let b = backend(
        r#"
timeline:
  - {at: 100, play: hi}
rules:
  - on: {submit: {text: ping}}
    once: true
    then: [{after: 10, play: hi}]
scripts:
  hi:
    - {after: 0, turn_started: {session: $session}}
    - {after: 0, text_delta: {session: s1, text: pong}}
"#,
    );
    let mut rx = b.events();
    assert!(drain(&mut rx, 50).await.is_empty());
    let first = drain(&mut rx, 100).await;
    assert!(
        first
            .iter()
            .any(|e| matches!(e, Event::TextDelta { text, .. } if text == "pong"))
    );
    b.call(Command::Submit {
        session: s1(),
        text: "ping".into(),
        mode: SubmitMode::Turn,
    })
    .await
    .unwrap();
    let ruled = drain(&mut rx, 100).await;
    assert!(
        ruled
            .iter()
            .any(|e| matches!(e, Event::TurnStarted { session } if session.as_str() == "s1"))
    );
}

#[tokio::test(start_paused = true)]
async fn occupied_instances_refuse_unless_forced() {
    let insts = MockInstances::new(Scenario::seed_only("empty"), MockOptions::default());
    let listed = insts.list(true).await.unwrap();
    assert!(listed.iter().any(|i| i.holder.is_some()));
    let err = insts.attach("ops", Attach::default()).await.err().unwrap();
    assert!(matches!(err, PortError::Occupied { .. }));
    insts.attach("ops", Attach { force: true }).await.unwrap();
    assert!(
        insts
            .list(false)
            .await
            .unwrap()
            .iter()
            .all(|i| i.holder.is_none())
    );
    assert!(insts.attach("research", Attach::default()).await.is_err());
    insts.start("research").await.unwrap();
}

#[test]
fn builtin_seeds_and_scenarios_parse() {
    for name in mock::seed_names() {
        mock::load_seed(name).unwrap_or_else(|e| panic!("{e}"));
    }
    for name in mock::scenario_names() {
        mock::load_scenario(name).unwrap_or_else(|e| panic!("{e}"));
    }
}

#[tokio::test(start_paused = true)]
async fn plugin_install_enable_uninstall_moves_flows() {
    let b = backend("seed: x");
    let mut rx = b.events();
    let Reply::Plugins(p) = b.call(Command::ListPlugins).await.unwrap() else {
        panic!()
    };
    assert_eq!(p.available.len(), 1);
    assert!(!p.available[0].installed && p.installed.is_empty());

    assert!(
        b.call(Command::InstallPlugin {
            source: "nope".into()
        })
        .await
        .is_err()
    );
    let Reply::Plugin { plugin, disclosure } = b
        .call(Command::InstallPlugin {
            source: "/somewhere/xforge-0.1.0.zip".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!plugin.enabled && !disclosure.capabilities.is_empty());
    assert!(
        drain(&mut rx, 10)
            .await
            .iter()
            .any(|e| matches!(e, Event::PluginsChanged))
    );

    b.call(Command::SetPluginEnabled {
        name: "xforge".into(),
        enabled: true,
    })
    .await
    .unwrap();
    let evs = drain(&mut rx, 10).await;
    assert_eq!(
        evs.iter()
            .filter(
                |e| matches!(e, Event::FlowUpserted(f) if f.source.as_deref() == Some("xforge"))
            )
            .count(),
        2
    );
    let Reply::Flows(f) = b.call(Command::ListFlows).await.unwrap() else {
        panic!()
    };
    assert!(f.iter().any(|f| f.name == "xforge-quick"));

    b.call(Command::UninstallPlugin {
        name: "xforge".into(),
    })
    .await
    .unwrap();
    let evs = drain(&mut rx, 10).await;
    assert_eq!(
        evs.iter()
            .filter(|e| matches!(e, Event::FlowRemoved { .. }))
            .count(),
        2
    );
    assert!(
        b.call(Command::PluginDisclosure {
            name: "xforge".into()
        })
        .await
        .is_err()
    );
}
