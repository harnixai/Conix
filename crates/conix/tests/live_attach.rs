//! LIVE：单一接入（architecture §8.3）——握手、被占用、强制接入踢人、脱离放开接入、
//! 心跳超时断开后重连、实例被停时接入者收到通知。

mod live;

use std::time::Duration;

use live::{Beat, Live, NAME, Opts, QUICK};
use port::{Attach, Command, ConnState, Event, Instances, PortError, Reply};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn hello_reports_the_instance_its_capabilities_and_the_holder() {
    let lv = Live::new();
    let c = lv.start().await;
    let Reply::Hello {
        instance,
        flows,
        jobs,
    } = c
        .ok(Command::Hello {
            client: client::this_client(),
            force: false,
        })
        .await
    else {
        panic!("hello")
    };
    assert_eq!(instance.name.0, NAME);
    assert_eq!(
        instance.project.as_deref(),
        Some(lv.project_path().as_str())
    );
    assert!(flows, "流程接上了");
    assert!(jobs);
    assert_eq!(
        instance.holder.map(|h| h.pid),
        Some(std::process::id()),
        "接入者是自己"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_second_client_is_refused_until_it_forces_and_then_the_first_is_kicked() {
    let lv = Live::new();
    let mut a = lv.start().await;

    match lv.attach(Attach::default()).await {
        Err(PortError::Occupied { holder, .. }) => assert_eq!(holder.pid, std::process::id()),
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("同一实例只能接一个客户端"),
    }
    let b = lv.attach(Attach { force: true }).await.expect("强制接入");
    a.wait_event("被踢", QUICK, |e| {
        matches!(e, Event::Kicked { by, .. } if by.pid == std::process::id()).then_some(())
    })
    .await;
    // 被踢的一方不重连，命令得到断开。
    assert!(matches!(
        a.call(Command::ListJobs).await,
        Err(PortError::Disconnected)
    ));
    assert!(b.call(Command::ListJobs).await.is_ok(), "新接入者正常可用");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn detaching_releases_the_instance_for_the_next_client() {
    let lv = Live::new();
    let a = lv.start().await;
    assert!(!lv.read("instance.json")["holder"].is_null());
    a.ok(Command::Detach).await;
    // 实例在连接收尾时才放开接入、重写发现文件。
    let deadline = std::time::Instant::now() + QUICK;
    while !lv.read("instance.json")["holder"].is_null() {
        assert!(
            std::time::Instant::now() < deadline,
            "发现文件里的接入者没清掉"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    lv.attach(Attach::default())
        .await
        .expect("脱离之后不带 force 也能接入");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_silent_client_is_dropped_after_the_heartbeat_timeout_and_reconnects() {
    let lv = Live::with(Opts {
        env: vec![("CONIX_TEST_HEARTBEAT_TIMEOUT_MS".into(), "2000".into())],
        ..Default::default()
    });
    let mut a = lv.start().await;
    a.heartbeat(Beat::None);
    a.wait_event("心跳超时断开", QUICK, |e| {
        matches!(e, Event::Connection(ConnState::Lost)).then_some(())
    })
    .await;
    a.wait_event("重连上", QUICK, |e| {
        matches!(e, Event::Connection(ConnState::Resynced)).then_some(())
    })
    .await;
    a.heartbeat(Beat::Active);
    assert!(a.call(Command::ListJobs).await.is_ok(), "重连后照常可用");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn stopping_the_instance_tells_the_attached_client() {
    let lv = Live::new();
    let mut a = lv.start().await;
    lv.stop().await;
    let reason = a
        .wait_event("实例停了", QUICK, |e| match e {
            Event::InstanceStopped { reason } => Some(reason.clone()),
            _ => None,
        })
        .await;
    assert_eq!(reason, "stopped");
    assert!(lv.instances.list(false).await.unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_stop_instance_command_shuts_the_instance_down() {
    let lv = Live::new();
    let a = lv.start().await;
    a.ok(Command::StopInstance).await;
    let deadline = std::time::Instant::now() + QUICK;
    while !lv.instances.list(false).await.unwrap().is_empty() {
        assert!(std::time::Instant::now() < deadline, "实例没停下");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(lv.read("meta.json")["stopped_ms"].is_u64());
    assert!(!lv.layout().socket(NAME).exists());
}

/// 脱离期间一次作业失败了：下一次接入时实例推一次 `AwaySummary`，里面有这条失败，指向那个作业；再下一次接入就没有了。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn what_happened_while_detached_is_pushed_once_on_the_next_attach() {
    let lv = Live::with(Opts {
        conix_yaml: Some("jobs:\n  defaults:\n    timeout: 2s\n".into()),
        ..Default::default()
    });
    let c = lv.start_trusted().await;
    let Reply::Job(j) = c
        .ok(Command::SaveJob {
            job: port::Job {
                id: port::JobId(String::new()),
                name: "away".into(),
                cron: "0 3 * * *".into(),
                kind: port::JobKind::Simple {
                    prompt: "Write a 3000-word essay on the history of timekeeping. Do not use any tools."
                        .into(),
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
            },
        })
        .await
    else {
        panic!("save_job")
    };
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    // 触发一开始就脱离；两秒的超时到了、记录落盘时，没有人在。
    c.ok(Command::Detach).await;
    drop(c);
    let deadline = std::time::Instant::now() + live::TURN;
    loop {
        let done = lv
            .job_history_file(&j.id.0)
            .iter()
            .any(|e| e.outcome != port::JobOutcome::Running);
        if done {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "那一次一直没有结局");
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    let mut b = loop {
        match lv.attach(Attach::default()).await {
            Ok(b) => break b,
            // 上一条连接的收尾与这次接入可能交错：被占用就稍后再试。
            Err(PortError::Occupied { .. }) => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(e) => panic!("接入：{e}"),
        }
    };
    let alerts = b
        .wait_event("离开期间的摘要", QUICK, |e| match e {
            Event::AwaySummary { alerts, .. } => Some(alerts.clone()),
            _ => None,
        })
        .await;
    assert!(
        alerts.iter().any(|a| a.kind == port::AlertKind::JobFailed
            && a.target == Some(port::AlertTarget::Job(j.id.clone()))),
        "{alerts:?}"
    );

    // 只推一次：脱离再接入，没有新的事就没有摘要。
    b.ok(Command::Detach).await;
    drop(b);
    let b = loop {
        match lv.attach(Attach::default()).await {
            Ok(b) => break b,
            Err(PortError::Occupied { .. }) => tokio::time::sleep(Duration::from_millis(200)).await,
            Err(e) => panic!("接入：{e}"),
        }
    };
    b.expect_none("摘要不再来", Duration::from_secs(2), |e| {
        matches!(e, Event::AwaySummary { .. })
    })
    .await;
}
