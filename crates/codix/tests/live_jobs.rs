//! LIVE：作业（architecture §8.5）——增删改与启停、立即运行触发出的会话与结局、触发记录落盘、
//! 停实例时跑到一半的那次记为失败、崩溃后没有结局的那次在启动时记为中断、错过的触发补跑一次。宿主工具在 live_tools.rs。

mod live;

use live::{Live, QUICK, TURN};
use port::{Command, Event, Job, JobId, JobKind, JobOutcome, JobTarget, Origin, Reply};

fn job(name: &str, cron: &str, prompt: &str, catch_up: bool) -> Job {
    Job {
        id: JobId(String::new()),
        name: name.into(),
        cron: cron.into(),
        kind: JobKind::Simple {
            prompt: prompt.into(),
            skills: vec![],
            project: None,
            scene: None,
            model: None,
        },
        enabled: true,
        catch_up,
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

async fn save(c: &live::Client, j: Job) -> Job {
    match c.ok(Command::SaveJob { job: j }).await {
        Reply::Job(j) => j,
        r => panic!("save_job 回的是 {r:?}"),
    }
}

async fn list(c: &live::Client) -> Vec<Job> {
    match c.ok(Command::ListJobs).await {
        Reply::Jobs(v) => v,
        r => panic!("list_jobs 回的是 {r:?}"),
    }
}

/// 等这个作业的一条触发记录满足 `want`。
async fn fired(
    c: &mut live::Client,
    id: &JobId,
    what: &str,
    want: impl Fn(&port::JobEntry) -> bool,
) -> port::JobEntry {
    let id = id.clone();
    c.wait_event(what, TURN, |e| match e {
        Event::JobFired { job, entry } if *job == id && want(entry) => Some(entry.clone()),
        _ => None,
    })
    .await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn jobs_are_saved_listed_toggled_and_deleted() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    let j = save(&c, job("nightly", "0 3 * * *", "say hi", false)).await;
    assert!(!j.id.0.is_empty(), "新作业分到 id");
    assert!(j.next_fire_ms.is_some_and(|t| t > live::now_ms()));
    let id = j.id.clone();
    c.wait_event("作业出现", QUICK, |e| {
        matches!(e, Event::JobUpserted(x) if x.id == id).then_some(())
    })
    .await;
    assert_eq!(list(&c).await, vec![j.clone()]);
    let stored = lv.read_jobs();
    assert_eq!(stored[0]["id"], j.id.0.as_str());
    assert_eq!(stored[0]["cron"], "0 3 * * *");
    assert!(
        stored[0].get("enabled").is_none(),
        "启停不进随项目提交的定义"
    );
    assert_eq!(lv.read_state("jobs-state.json")[&j.id.0]["enabled"], true);

    let bad = c
        .call(Command::SaveJob {
            job: job("bad", "not a cron", "x", false),
        })
        .await;
    assert!(bad.is_err(), "坏的 cron 不收：{bad:?}");

    c.ok(Command::SetJobEnabled {
        job: j.id.clone(),
        enabled: false,
    })
    .await;
    let id = j.id.clone();
    let off = c
        .wait_event("停用", QUICK, |e| match e {
            Event::JobUpserted(x) if x.id == id && !x.enabled => Some(x.clone()),
            _ => None,
        })
        .await;
    assert_eq!(off.next_fire_ms, None, "停用的作业没有下次触发");
    assert_eq!(lv.read_state("jobs-state.json")[&j.id.0]["enabled"], false);

    c.ok(Command::DeleteJob { job: j.id.clone() }).await;
    let id = j.id.clone();
    c.wait_event("删掉", QUICK, |e| {
        matches!(e, Event::JobRemoved { job } if *job == id).then_some(())
    })
    .await;
    assert!(list(&c).await.is_empty());
    assert_eq!(lv.read_jobs(), serde_json::json!([]));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn run_now_fires_a_job_session_and_records_the_outcome() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    // 停用的作业也能立即运行一次。
    let mut j = job("probe", "0 3 * * *", "Reply with exactly: JOB-OK", false);
    j.enabled = false;
    let j = save(&c, j).await;
    c.skip_past();
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;

    let running = fired(&mut c, &j.id, "触发（进行中）", |e| {
        e.outcome == JobOutcome::Running
    })
    .await;
    let Some(JobTarget::Session { id: sid, title }) = running.target.clone() else {
        panic!("{running:?}")
    };
    assert_eq!(title, "probe");
    let sum = c
        .sessions()
        .await
        .into_iter()
        .find(|s| s.id == sid)
        .expect("触发出的会话在列表里");
    assert!(
        matches!(&sum.provenance.origin, Origin::Job { job, name, fired_ms }
            if *job == j.id && name == "probe" && *fired_ms == running.fired_ms),
        "{:?}",
        sum.provenance.origin
    );

    let done = fired(&mut c, &j.id, "触发（结局）", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    assert_eq!(done.outcome, JobOutcome::Done);
    assert_eq!(done.fired_ms, running.fired_ms, "同一次触发补上结局");
    assert!(done.elapsed_ms > 0 && done.usage.output > 0, "{done:?}");
    let id = j.id.clone();
    c.wait_event("作业的上次结局", QUICK, |e| {
        matches!(e, Event::JobUpserted(x) if x.id == id && x.last == Some(JobOutcome::Done))
            .then_some(())
    })
    .await;

    let on_disk = lv.job_history_file(&j.id.0);
    assert_eq!(on_disk.len(), 1);
    assert_eq!(on_disk[0].outcome, JobOutcome::Done);
    let Reply::JobHistory(h) = c
        .ok(Command::JobHistory {
            job: j.id.clone(),
            before_ms: None,
            limit: None,
            outcome: None,
        })
        .await
    else {
        panic!("job_history")
    };
    assert_eq!(h, on_disk);
    // 触发出的会话里就是那句提示词与回答。
    let w = c.watch(&sid).await;
    assert!(
        w.history
            .iter()
            .any(|h| matches!(h, port::HistoryItem::Assistant { text } if text.contains("JOB-OK"))),
        "{:?}",
        w.history
    );
}

/// 一次触发跑到一半：起一个长的作业，等它的 `Running` 记录出来。
async fn fire_long(lv: &Live) -> (live::Client, Job, port::JobEntry) {
    let mut c = lv.start_trusted().await;
    let j = save(
        &c,
        job(
            "long",
            "0 3 * * *",
            "Count from 1 to 400, writing each number on its own line, with no other text.",
            false,
        ),
    )
    .await;
    c.skip_past();
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    let running = fired(&mut c, &j.id, "触发（进行中）", |e| {
        e.outcome == JobOutcome::Running
    })
    .await;
    (c, j, running)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_firing_cut_short_by_a_stop_is_recorded_as_stopped_by_the_instance() {
    let lv = Live::new();
    let (c, j, running) = fire_long(&lv).await;
    drop(c);
    lv.stop().await;
    let on_disk = lv.job_history_file(&j.id.0);
    let e = on_disk
        .iter()
        .find(|e| e.fired_ms == running.fired_ms)
        .expect("那次触发");
    assert_eq!(
        e.outcome,
        JobOutcome::Stopped {
            reason: port::StopCause::InstanceStopped
        },
        "正常停实例时引擎收掉那一轮，记录随之收尾，不算作业失败"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_firing_left_running_by_a_crash_is_marked_interrupted_on_start() {
    let lv = Live::new();
    let (c, j, running) = fire_long(&lv).await;
    drop(c);
    let pid = lv.read("instance.json")["pid"].as_u64().unwrap();
    lv.kill_host().await;
    // 看护进程把实例重新拉起来；起来时把崩溃留下的那次收尾。
    let c = lv.wait_restarted(pid).await;
    let Reply::JobHistory(h) = c
        .ok(Command::JobHistory {
            job: j.id.clone(),
            before_ms: None,
            limit: None,
            outcome: None,
        })
        .await
    else {
        panic!("job_history")
    };
    let e = h
        .iter()
        .find(|e| e.fired_ms == running.fired_ms)
        .expect("那次触发");
    assert_eq!(
        e.outcome,
        JobOutcome::Stopped {
            reason: port::StopCause::InstanceStopped
        },
        "崩溃留下的那次在下次启动时收尾，不算作业失败"
    );
    assert!(
        lv.job_history_file(&j.id.0)
            .iter()
            .all(|e| e.outcome != JobOutcome::Running),
        "落盘的记录也改了"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_missed_firing_is_caught_up_once_only_when_asked() {
    let lv = Live::new();
    let c = lv.start_trusted().await;
    let a = save(
        &c,
        job("catch", "0 * * * *", "Reply with exactly: CAUGHT-UP", true),
    )
    .await;
    let b = save(
        &c,
        job("skip", "0 * * * *", "Reply with exactly: SKIPPED", false),
    )
    .await;
    drop(c);
    lv.stop().await;

    // 装作实例停了两个小时：把两个作业的基线挪到两小时前。
    let path = lv.state().jobs_state();
    let mut stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let past = live::now_ms() - 2 * 3_600_000;
    for s in stored.as_object_mut().unwrap().values_mut() {
        s["since_ms"] = past.into();
        s["last_fired_ms"] = past.into();
    }
    std::fs::write(&path, serde_json::to_string(&stored).unwrap()).unwrap();

    let mut c = lv.start_again().await;
    let done = fired(&mut c, &a.id, "补跑的结局", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    assert_eq!(done.outcome, JobOutcome::Done);
    assert_eq!(lv.job_history_file(&a.id.0).len(), 1, "错过两次也只补一次");
    assert!(
        lv.job_history_file(&b.id.0).is_empty(),
        "不补跑的作业不触发"
    );
    let stored: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for s in stored.as_object().unwrap().values() {
        assert!(
            s["last_fired_ms"].as_u64().unwrap() > past,
            "两个作业的基线都前移了：{s}"
        );
    }
    let nb = list(&c).await.into_iter().find(|x| x.id == b.id).unwrap();
    assert!(
        nb.next_fire_ms.unwrap() > live::now_ms(),
        "不补跑的排到下一个整点"
    );
}

/// 停下正在跑的那一次：打断它的会话，结局记成手动停下（architecture §8.5）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_running_firing_is_stopped_and_recorded_as_stopped() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    let j = save(
        &c,
        job(
            "slow",
            "0 3 * * *",
            "Use the Bash tool to run `sleep 60`, then reply with exactly: SLEPT",
            false,
        ),
    )
    .await;
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    fired(&mut c, &j.id, "开始", |e| {
        e.outcome == JobOutcome::Running
    })
    .await;
    c.ok(Command::StopFiring {
        job: j.id.clone(),
        fired_ms: None,
    })
    .await;
    let done = fired(&mut c, &j.id, "停下", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    assert_eq!(
        done.outcome,
        JobOutcome::Stopped {
            reason: port::StopCause::Manual
        }
    );
}

/// 护栏在真实实例上：`codix.yaml` 给的超时到了，那一次被打断、记成超时；作业的护栏显示的是这台机器的值。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_firing_past_the_configured_timeout_is_stopped() {
    let lv = Live::with(live::Opts {
        codix_yaml: Some("jobs:\n  defaults:\n    timeout: 2s\n".into()),
        ..Default::default()
    });
    let mut c = lv.start_trusted().await;
    let j = save(
        &c,
        job(
            "slow",
            "0 3 * * *",
            "Write a 3000-word essay on the history of timekeeping. Do not use any tools.",
            false,
        ),
    )
    .await;
    assert_eq!(
        j.effective_guard.as_ref().and_then(|g| g.timeout_ms),
        Some(2_000),
        "生效的超时取自 codix.yaml"
    );
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    let done = fired(&mut c, &j.id, "超时停下", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    assert_eq!(
        done.outcome,
        JobOutcome::Stopped {
            reason: port::StopCause::Timeout
        },
        "{done:?}"
    );
}

/// 通知出口在真实实例上：作业的一次触发失败了，配好的命令收到整条通知（JSON 进 stdin）；
/// TestNotify 回每个出口的结果（architecture §8.10）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_failed_firing_reaches_the_configured_notification_command() {
    let out = tempfile::tempdir().unwrap();
    let file = out.path().join("alert.json");
    let yaml = format!(
        "notify:\n  when: always\n  min_level: error\n  sinks:\n    - name: file\n      command: [\"sh\", \"-c\", \"cat > {}\"]\n      stdin: json\n",
        file.display()
    );
    let lv = Live::with(live::Opts {
        codix_yaml: Some(yaml),
        env: vec![("CODIX_TEST_NOTIFY_WINDOW_MS".into(), "300".into())],
    });
    let c = lv.start_trusted().await;
    let Reply::Notified(results) = c.ok(Command::TestNotify).await else {
        panic!("test_notify")
    };
    assert_eq!(results.len(), 1);
    assert!(results[0].ok, "{results:?}");
    std::fs::remove_file(&file).unwrap();

    let mut j = job("broken", "0 3 * * *", "say hi", false);
    if let port::JobKind::Simple { model, .. } = &mut j.kind {
        *model = Some("no-such-model-anywhere".into());
    }
    let j = save(&c, j).await;
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !file.exists() && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&file).expect("出口收到了通知")).unwrap();
    assert_eq!(v["alerts"][0]["kind"], "job_failed", "{v}");
    assert!(
        v["attach"].as_str().unwrap().contains("codix attach"),
        "{v}"
    );
}

/// 跑完一个简单作业，回它的结局与触发出的会话。
async fn run_to_end(c: &mut live::Client, j: Job) -> (port::JobEntry, port::SessionId) {
    let j = save(c, j).await;
    c.skip_past();
    c.ok(Command::RunJobNow {
        job: j.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    let done = fired(c, &j.id, "触发（结局）", |e| {
        e.outcome != JobOutcome::Running
    })
    .await;
    let Some(JobTarget::Session { id, .. }) = done.target.clone() else {
        panic!("{done:?}")
    };
    (done, id)
}

/// Agix 0.4 起无人值守的会话里，没有 allow 规则的命令要网络受限才执行。作业缺省 deny_all：
/// 本地命令照样能跑（升级前的缺省网络下这条会被拒）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_job_with_no_network_still_runs_local_commands() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    let j = job(
        "local-cmd",
        "0 3 * * *",
        "Use the Bash tool to run exactly: echo LIVE-$((40+2)) . Then reply with exactly the command's output.",
        false,
    );
    let (done, sid) = run_to_end(&mut c, j).await;
    assert_eq!(done.outcome, JobOutcome::Done, "{done:?}");
    let audit = done.audit.clone().unwrap_or_default();
    assert!(audit.tools.contains_key("Bash"), "{audit:?}");
    assert_eq!(audit.errors, 0, "命令没被拒：{audit:?}");
    let w = c.watch(&sid).await;
    assert!(
        w.history.iter().any(
            |h| matches!(h, port::HistoryItem::Assistant { text } if text.contains("LIVE-42"))
        ),
        "{:?}",
        w.history
    );
}

/// 作业的 max_tokens 下发为会话预算：引擎在调用前就拦住，那一轮以 budget_exceeded 收尾，记成被预算停下。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_job_over_its_token_budget_is_stopped_by_the_engine() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    let mut j = job(
        "tiny-budget",
        "0 3 * * *",
        "Write a 3000-word essay about the history of compilers.",
        false,
    );
    // 比一次调用的输入（系统提示词与工具定义）还小：第一次调用前就付不起。
    j.guard.max_tokens = Some(1_000);
    let (done, _) = run_to_end(&mut c, j).await;
    assert_eq!(
        done.outcome,
        JobOutcome::Stopped {
            reason: port::StopCause::Budget
        },
        "{done:?}"
    );
}

/// 作业会话里模型不能自建定时任务：CronCreate 被拒，作业照常完成。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_job_session_cannot_create_scheduled_tasks() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    let j = job(
        "no-cron",
        "0 3 * * *",
        "Use the CronCreate tool to schedule the prompt \"say hi\" every minute. If the tool is refused or unavailable, reply with exactly: CRON-REFUSED.",
        false,
    );
    let (done, sid) = run_to_end(&mut c, j).await;
    assert_eq!(done.outcome, JobOutcome::Done, "{done:?}");
    let audit = done.audit.clone().unwrap_or_default();
    if audit.tools.contains_key("CronCreate") {
        assert!(audit.errors > 0, "调了就该被拒：{audit:?}");
    }
    let w = c.watch(&sid).await;
    assert!(
        w.history.iter().any(
            |h| matches!(h, port::HistoryItem::Assistant { text } if text.contains("CRON-REFUSED"))
        ),
        "{:?}",
        w.history
    );
}
