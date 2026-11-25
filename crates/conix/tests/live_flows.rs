//! LIVE：流程（architecture §8.6）——内置的示例流程、在用户会话里经 `flow_*` 写流程、一次运行
//! 从头跑到尾、取消、有运行时不许改、运行里的会话动不了别的流程。

mod live;

use std::time::Duration;

use live::{Answering, Live, QUICK};
use port::{Command, Event, FlowOrigin, Reply, RunOp, RunStatus, StationStatus};

/// 一次运行跑完的上限：每站一轮模型。
const RUN: Duration = Duration::from_secs(240);

/// 一站：主会话里做一轮，交一个词。
const ONE_STATION: &str = r#"name: live-echo
title: Live echo
stations:
  - id: say
    title: Say
    executor: {kind: session}
    skill: say-word
    output:
      type: object
      required: [word]
      properties:
        word: {type: string}
    exit: [{kind: output}]
"#;

/// 一站，在自己的 worktree 里干活。
const IN_WORKTREE: &str = r#"name: live-wt
title: Live worktree
workspace: worktree
stations:
  - id: say
    title: Say
    executor: {kind: session}
    skill: say-word
    output:
      type: object
      required: [word]
      properties:
        word: {type: string}
    exit: [{kind: output}]
"#;

/// 两站：第二站等人确认，运行就停在那里不结束。
const WAITS: &str = r#"name: live-wait
title: Live wait
stations:
  - id: say
    title: Say
    executor: {kind: session}
    skill: say-word
    output:
      type: object
      required: [word]
      properties:
        word: {type: string}
    exit: [{kind: output}]
  - id: approve
    title: Approve
    exit: [{kind: confirm}]
"#;

/// 两站：说一个词，然后一站进站就搁置、出站要人确认——运行操作在它上面一样样试。
const OPS: &str = r#"name: live-ops
title: Live ops
stations:
  - id: say
    title: Say
    executor: {kind: session}
    skill: say-word
    output:
      type: object
      required: [word]
      properties:
        word: {type: string}
    exit: [{kind: output}]
  - id: gate
    title: Gate
    hold: true
    rework_to: [say]
    exit: [{kind: confirm}]
"#;

/// 全局流程包里放好这些流程，以及它们用的 skill。
/// 流程只在项目里（architecture §8.1）：放进项目自己的流程包 `<项目>/.conix/flows/`，项目受信才读。
fn install(lv: &Live, flows: &[(&str, &str)]) {
    let root = std::path::Path::new(&lv.project_path()).join(".conix/flows");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(root.join("skills/say-word")).unwrap();
    std::fs::write(
        root.join("skills/say-word/SKILL.md"),
        "---\nname: say-word\ndescription: Hand in the one word the subject asks for.\n---\n\n\
         Hand in `word`: the single word the run's subject asks you to say. Nothing else.\n",
    )
    .unwrap();
    for (name, yaml) in flows {
        std::fs::write(root.join(format!("{name}.yaml")), yaml).unwrap();
    }
}

async fn flows(c: &live::Client) -> Vec<port::FlowSummary> {
    match c.ok(Command::ListFlows).await {
        Reply::Flows(v) => v,
        r => panic!("list_flows 回的是 {r:?}"),
    }
}

async fn start(c: &live::Client, flow: &str, subject: &str) -> port::RunSummary {
    match c
        .ok(Command::StartRun {
            flow: flow.into(),
            subject: subject.into(),
            inputs: serde_json::Value::Null,
        })
        .await
    {
        Reply::RunStarted(s) => s,
        r => panic!("start_run 回的是 {r:?}"),
    }
}

/// 在一个新的用户会话里让模型调一个工具；回这一轮。
async fn ask(c: &mut live::Client, text: &str) -> live::TurnOutcome {
    let s = c.new_session().await;
    c.watch(&s).await;
    c.turn(&s, text).await
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_example_flow_is_offered() {
    let lv = Live::new();
    let c = lv.start().await;
    let all = flows(&c).await;
    let ex = all
        .iter()
        .find(|f| f.name == "example-review")
        .unwrap_or_else(|| panic!("{all:?}"));
    assert_eq!(ex.origin, FlowOrigin::Builtin);
    let Reply::Flow(d) = c
        .ok(Command::GetFlow {
            name: "example-review".into(),
        })
        .await
    else {
        panic!("get_flow")
    };
    let ids: Vec<&str> = d.stations.iter().map(|s| s.id.0.as_str()).collect();
    assert_eq!(ids, ["review", "approve"]);
    assert_eq!(d.stations[1].needs[0].0, "review");
    assert!(d.yaml.contains("example-review"));
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_user_session_validates_and_creates_a_flow_once_the_project_is_trusted() {
    let lv = Live::new();
    install(&lv, &[]);
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    // 没信任的项目：流程存不进去，工具照实说。
    let t = ask(
        &mut c,
        &format!(
            "Call flow_create once with exactly this YAML:\n{ONE_STATION}\nDo not ask me to confirm. \
             Report the tool's answer in one line."
        ),
    )
    .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not trusted")),
        "{:?}",
        t.results
    );
    assert!(!flows(&c).await.iter().any(|f| f.name == "live-echo"));

    // /trust 之后：校验报错的不存，对的存进项目自己的流程包。
    c.ok(Command::TrustProject { trusted: true }).await;
    let bad = "name: broken\nstations:\n  - id: a\n    exit: [{kind: nope}]\n";
    let t = ask(
        &mut c,
        &format!(
            "Call flow_validate once with exactly this YAML:\n{bad}\nThen call flow_create once with \
             exactly this YAML:\n{ONE_STATION}\nDo not ask me to confirm. Reply with only the word DONE."
        ),
    )
    .await;
    t.ended();
    assert!(
        t.tools.iter().any(|n| n == "flow_validate"),
        "{:?}",
        t.tools
    );
    assert!(
        t.results
            .iter()
            .any(|(r, _)| r.contains("problems") || r.contains("unknown variant")),
        "校验报出了问题：{:?}",
        t.results
    );
    let made = flows(&c)
        .await
        .into_iter()
        .find(|f| f.name == "live-echo")
        .expect("建出来了");
    assert_eq!(made.origin, FlowOrigin::Project, "存进项目自己的流程包");
    assert!(
        std::path::Path::new(&lv.project_path())
            .join(".conix/flows/live-echo.yaml")
            .exists(),
        "项目的 .conix/flows/ 里"
    );
    assert!(
        c.events()
            .iter()
            .any(|e| matches!(e, Event::FlowUpserted(f) if f.name == "live-echo")),
        "流程推出来了"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_run_goes_from_start_to_finish() {
    let lv = Live::new();
    install(&lv, &[("live-echo", ONE_STATION)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let run = start(&c, "live-echo", "say the word OK").await;
    let id = run.id.clone();
    let done = c
        .wait_event("运行结束", RUN, |e| match e {
            Event::RunDetail(v)
                if v.summary.id == id && v.summary.status == RunStatus::Finished =>
            {
                Some((**v).clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(done.stations[0].status, StationStatus::Passed);
    // 主会话是这次运行的会话，归属是流程。运行一结束它就关了，结束后再查列表可能已经没有，
    // 所以看运行期间推过来的会话。
    let main = done.main_session.clone();
    let seen: Vec<_> = c
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::SessionUpserted(s) => Some(s),
            _ => None,
        })
        .collect();
    let sum = seen.iter().rev().find(|s| s.id == main).unwrap_or_else(|| {
        panic!(
            "主会话 {main:?} 没推过：{:?}",
            seen.iter()
                .map(|s| (&s.id, &s.provenance.origin))
                .collect::<Vec<_>>()
        )
    });
    assert!(
        matches!(&sum.provenance.origin, port::Origin::FlowStation { run, .. } if *run == id),
        "{:?}",
        sum.provenance.origin
    );
    let Reply::Runs(runs) = c
        .ok(Command::ListRuns {
            flow: "live-echo".into(),
        })
        .await
    else {
        panic!("list_runs")
    };
    let listed = runs.iter().find(|r| r.id == id).expect("运行在列表里");
    assert_eq!(
        (listed.stations_done, listed.stations_total),
        (1, 1),
        "列表里的站数从运行记录算"
    );
    let Reply::Runs(ended) = c
        .ok(Command::ListEndedRuns {
            limit: 50,
            before_ms: None,
        })
        .await
    else {
        panic!("list_ended_runs")
    };
    let e = ended
        .iter()
        .find(|r| r.id == id)
        .expect("结束的运行在「已结束」里");
    assert!(e.ended_ms.is_some(), "带结束时刻");
    let Reply::Run(v) = c.ok(Command::GetRun { run: id.clone() }).await else {
        panic!("get_run")
    };
    assert!(!v.def_digest.is_empty(), "带钉住的定义的摘要");
    assert_eq!(v.def_state, port::DefState::Current, "定义没改过");
}

/// 运行的 worktree 在仓库根的 `.wkts/<运行>/`，主仓库看不到它，运行记录在项目状态里（architecture §8.1）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_worktree_run_works_under_the_repositorys_wkts() {
    let lv = Live::new();
    install(&lv, &[("live-wt", IN_WORKTREE)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let run = start(&c, "live-wt", "say the word OK").await;
    let id = run.id.clone();
    c.wait_event("运行结束", RUN, |e| match e {
        Event::RunDetail(v) if v.summary.id == id && v.summary.status == RunStatus::Finished => {
            Some(())
        }
        _ => None,
    })
    .await;
    let project = std::path::PathBuf::from(lv.project_path());
    let wkts = project.join(".wkts");
    let safe: String =
        id.0.chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '-'
                }
            })
            .collect();
    assert!(wkts.join(&safe).is_dir(), "worktree 在 .wkts/{safe}");
    assert_eq!(
        std::fs::read_to_string(wkts.join(".gitignore")).unwrap(),
        "*\n"
    );
    let status = std::process::Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(&project)
        .output()
        .unwrap();
    let status = String::from_utf8_lossy(&status.stdout);
    assert!(
        !status.contains(".wkts") && !status.contains(".conix/state"),
        "主仓库看不到 worktree 和项目状态：{status}"
    );
    assert!(
        std::fs::read_dir(lv.state().flow_runs())
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&safe)),
        "运行记录在项目状态里"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_flow_with_runs_not_finished_cannot_be_changed_until_they_are_cancelled() {
    let lv = Live::new();
    install(&lv, &[("live-wait", WAITS), ("live-echo", ONE_STATION)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    c.ok(Command::TrustProject { trusted: true }).await;
    let run = start(&c, "live-wait", "say the word OK").await;
    let id = run.id.clone();
    let waiting = c
        .wait_event("运行停在确认上", RUN, |e| match e {
            Event::RunDetail(v)
                if v.summary.id == id && matches!(v.summary.status, RunStatus::Waiting { .. }) =>
            {
                Some((**v).clone())
            }
            _ => None,
        })
        .await;

    // 有没结束的运行：改不了。
    let t = ask(
        &mut c,
        &format!(
            "Call flow_update once with flow \"live-wait\" and exactly this YAML:\n{}\nDo not ask me \
             to confirm. Report the tool's answer in one line.",
            WAITS.replace("Live wait", "Live wait 2")
        ),
    )
    .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not finished") && r.contains(&id.0)),
        "拒绝并列出运行：{:?}",
        t.results
    );

    // 运行里的会话：动不了别的流程。
    let main = waiting.main_session.clone();
    c.watch(&main).await;
    let t = c
        .turn(
            &main,
            "Call flow_delete once with flow \"live-echo\". Do not ask me to confirm. Report the \
             tool's answer in one line.",
        )
        .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not allowed")),
        "{:?}",
        t.results
    );
    assert!(flows(&c).await.iter().any(|f| f.name == "live-echo"));

    // 取消之后就能改了。
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::Cancel,
    })
    .await;
    c.wait_event("运行取消了", QUICK, |e| {
        matches!(e, Event::RunUpserted(s) if s.id == id && s.status == RunStatus::Cancelled)
            .then_some(())
    })
    .await;
    let t = ask(
        &mut c,
        &format!(
            "Call flow_update once with flow \"live-wait\" and exactly this YAML:\n{}\nDo not ask me \
             to confirm. Reply with only the word DONE.",
            WAITS.replace("Live wait", "Live wait 2")
        ),
    )
    .await;
    t.ended();
    assert!(
        flows(&c)
            .await
            .iter()
            .any(|f| f.name == "live-wait" && f.title == "Live wait 2"),
        "{:?}",
        t.results
    );
}

/// 库的一致性套件对着我们的 `SessionHost`：一个键只开一次、一次 send 恰起一轮并且结束、问句
/// 送来能答、关掉的会话不在了……套件要在进程里拿到宿主，引擎就得嵌在测试进程里，凭据得在
/// 进程的环境里——写环境要 `unsafe`，所以带着凭据把自己再起一遍，只跑里面那个用例。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn our_session_host_passes_the_conformance_suite() {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--ignored",
            "--exact",
            "conformance_inside",
            "--nocapture",
            "--test-threads=1",
        ])
        .envs(live::credentials())
        .env("CONIX_CONFORMANCE", "1")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .status()
        .unwrap();
    assert!(status.success(), "一致性套件没过（输出在上面）");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn conformance_inside() {
    if std::env::var_os("CONIX_CONFORMANCE").is_none() {
        return;
    }
    use harnix_flow::conformance::{Subject, run_all};
    let lv = Live::new();
    let project = std::path::PathBuf::from(lv.project_path());
    let (embedded, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: lv.home.path().to_path_buf(),
        project: project.clone(),
        groups_file: lv.home.path().join("groups.json"),
        settings: host::hub::cli_settings(),
        model: Some(live::model()),
        host_lib: Default::default(),
    })
    .await
    .expect("嵌入引擎");
    let hub = host::Hub::start(
        std::sync::Arc::new(client),
        notes,
        host::HubConfig {
            instance: "conformance".into(),
            host: "local".into(),
            project: project.clone(),
            version: "0".into(),
            started_ms: live::now_ms(),
            registry: host::hub::Registry::in_memory(),
            jobs: None,
            clock: std::sync::Arc::new(live::now_ms),
            engine_model: embedded.model().to_string(),
        },
    )
    .await
    .expect("Hub");
    let subject = Subject::new(host::flows::session_host(&hub), "code")
        .project(project)
        .idle_turn("Reply with only the word OK and stop.")
        .asking_turn(
            "Use the Write tool to create a file named probe.txt containing hi. Do nothing else.",
            "Write",
        )
        .turn_timeout(Duration::from_secs(120));
    let failures = run_all(&subject).await;
    hub.shutdown();
    embedded.shutdown().await;
    assert!(failures.is_empty(), "{failures:#?}");
}

/// 流程作业：触发一次就启动一次运行，触发记录指着那次运行。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_flow_job_starts_a_run_when_it_fires() {
    let lv = Live::new();
    install(&lv, &[("live-echo", ONE_STATION)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let job = port::Job {
        id: port::JobId(String::new()),
        name: "flow-job".into(),
        cron: "0 4 * * *".into(),
        kind: port::JobKind::Flow {
            flow: "live-echo".into(),
            subject: "say the word OK".into(),
            inputs: serde_json::Value::Null,
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
    let Reply::Job(job) = c.ok(Command::SaveJob { job }).await else {
        panic!("save_job")
    };
    c.ok(Command::RunJobNow {
        job: job.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    let jid = job.id.clone();
    let entry = c
        .wait_event("触发记录", QUICK, |e| match e {
            Event::JobFired { job, entry } if *job == jid => Some(entry.clone()),
            _ => None,
        })
        .await;
    let Some(port::JobTarget::Run { id, subject }) = entry.target.clone() else {
        panic!("{entry:?}")
    };
    assert_eq!(subject, "say the word OK");
    assert_eq!(entry.outcome, port::JobOutcome::Done);
    c.wait_event("那次运行跑完", RUN, |e| {
        matches!(e, Event::RunUpserted(s) if s.id == id && s.status == RunStatus::Finished)
            .then_some(())
    })
    .await;
    // 作业起的运行：第一次开站会话时按作业的护栏记下网络（缺省 deny_all），这次运行的站会话都照它开。
    let policies: serde_json::Value = serde_json::from_slice(
        &std::fs::read(lv.state().run_policies()).expect("run-policies.json"),
    )
    .unwrap();
    assert_eq!(policies[&id.0]["network"], "deny_all", "{policies}");
    assert_eq!(policies[&id.0]["job"], job.id.0.as_str());
}

/// 作业工具对齐流程：用户会话在对话里先校验、再建出一个流程作业（流程按名字认、主题照给），
/// 再这一次换个主题立即跑一次，起的运行就是这个主题（architecture §8.5）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_user_session_creates_and_runs_a_flow_job_with_the_job_tools() {
    let lv = Live::new();
    install(&lv, &[("live-echo", ONE_STATION)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let t = ask(
        &mut c,
        "Call job_validate once, then job_save once, both with: name \"weekly-echo\", cron \"0 9 * * 1\", \
         kind \"flow\", flow \"live-echo\", subject \"say the word OK\". Then call job_run once with target \
         \"weekly-echo\"'s id and subject \"say the word YES\". Do not ask me to confirm. Reply with only DONE.",
    )
    .await;
    t.ended();
    for tool in ["job_validate", "job_save", "job_run"] {
        assert!(t.tools.iter().any(|n| n == tool), "{tool}: {:?}", t.tools);
    }
    let Reply::Jobs(jobs) = c.ok(Command::ListJobs).await else {
        panic!("list_jobs")
    };
    let job = jobs
        .iter()
        .find(|j| j.name == "weekly-echo")
        .expect("建出了流程作业");
    assert!(
        matches!(&job.kind, port::JobKind::Flow { flow, subject, .. }
            if flow == "live-echo" && subject == "say the word OK"),
        "{:?}",
        job.kind
    );
    c.wait_event("这一次的运行", RUN, |e| match e {
        Event::RunUpserted(r) if r.flow == "live-echo" && r.subject == "say the word YES" => {
            Some(())
        }
        _ => None,
    })
    .await;
}

/// 等到这次运行的详情满足 `want`。
async fn until_run(
    c: &mut live::Client,
    what: &str,
    id: &port::RunId,
    want: impl Fn(&port::RunView) -> bool,
) -> port::RunView {
    let id = id.clone();
    c.wait_event(what, RUN, |e| match e {
        Event::RunDetail(v) if v.summary.id == id && want(v) => Some((**v).clone()),
        _ => None,
    })
    .await
}

fn station(v: &port::RunView, id: &str) -> StationStatus {
    v.stations
        .iter()
        .find(|s| s.id.as_str() == id)
        .map(|s| s.status)
        .unwrap()
}

/// 运行操作经端口落到真实的派工器：搁置的站放行、暂停与恢复、返工到前一站、强制放行、确认
/// （取消在上面那条用例里）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn run_ops_release_pause_resume_rework_force_and_confirm() {
    let lv = Live::new();
    install(&lv, &[("live-ops", OPS)]);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let gate = port::StationId("gate".into());
    let op = |run: &port::RunId, op: RunOp| Command::RunOp {
        run: run.clone(),
        op,
    };

    let id = start(&c, "live-ops", "say the word OK").await.id;
    until_run(&mut c, "进站搁置", &id, |v| {
        station(v, "gate") == StationStatus::Held
    })
    .await;

    c.ok(op(&id, RunOp::Pause)).await;
    until_run(&mut c, "暂停", &id, |v| {
        matches!(v.summary.status, RunStatus::Paused | RunStatus::Pausing)
    })
    .await;
    c.ok(op(&id, RunOp::Resume)).await;
    until_run(&mut c, "恢复", &id, |v| {
        !matches!(v.summary.status, RunStatus::Paused | RunStatus::Pausing)
    })
    .await;

    c.ok(op(
        &id,
        RunOp::Release {
            station: gate.clone(),
        },
    ))
    .await;
    until_run(&mut c, "放行后等确认", &id, |v| {
        station(v, "gate") == StationStatus::Working
    })
    .await;

    c.ok(op(
        &id,
        RunOp::Rework {
            to: port::StationId("say".into()),
        },
    ))
    .await;
    until_run(&mut c, "返工后重新到搁置", &id, |v| {
        station(v, "say") == StationStatus::Passed && station(v, "gate") == StationStatus::Held
    })
    .await;
    c.ok(op(
        &id,
        RunOp::Release {
            station: gate.clone(),
        },
    ))
    .await;
    c.ok(op(
        &id,
        RunOp::Force {
            station: gate.clone(),
            reason: "live test".into(),
        },
    ))
    .await;
    until_run(&mut c, "强制放行后结束", &id, |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;

    let id = start(&c, "live-ops", "say the word YES").await.id;
    until_run(&mut c, "第二次进站搁置", &id, |v| {
        station(v, "gate") == StationStatus::Held
    })
    .await;
    c.ok(op(
        &id,
        RunOp::Release {
            station: gate.clone(),
        },
    ))
    .await;
    until_run(&mut c, "等确认", &id, |v| {
        station(v, "gate") == StationStatus::Working
    })
    .await;
    c.ok(op(
        &id,
        RunOp::Confirm {
            station: gate,
            reject: false,
            reason: None,
        },
    ))
    .await;
    until_run(&mut c, "确认后结束", &id, |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
}

/// 流程只在项目里：项目的标 Project；内置的标 Builtin、各项目都有；项目里放一份与内置同名的就取代它，
/// 来源是 Project。以前的全局目录 `~/.conix/flows/` 不再读。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn flows_live_in_the_project_and_a_projects_same_named_flow_replaces_a_builtin_one() {
    let lv = Live::new();
    let old = lv.home.path().join("flows/flows");
    std::fs::create_dir_all(&old).unwrap();
    std::fs::write(
        old.join("live-old.yaml"),
        ONE_STATION.replace("name: live-echo", "name: live-old"),
    )
    .unwrap();
    install(&lv, &[("live-echo", ONE_STATION)]);
    let c = lv.start_trusted().await;
    let all = flows(&c).await;
    let find = |all: &[port::FlowSummary], name: &str| all.iter().find(|f| f.name == name).cloned();
    let mine = find(&all, "live-echo").expect("项目的流程列得出");
    assert_eq!(mine.origin, FlowOrigin::Project, "{mine:?}");
    let builtin = find(&all, "example-review").expect("内置的列得出");
    assert!(
        builtin.origin == FlowOrigin::Builtin && builtin.origin.is_shared(),
        "{builtin:?}"
    );
    assert!(find(&all, "live-old").is_none(), "全局目录不再读：{all:?}");

    // 项目里放一份同名的：流程库在实例启动时读，重启之后看到的是项目的那份。
    install(
        &lv,
        &[(
            "example-review",
            &ONE_STATION
                .replace("name: live-echo", "name: example-review")
                .replace("title: Live echo", "title: Project review"),
        )],
    );
    drop(c);
    let c = lv.restart().await;
    let all = flows(&c).await;
    let replaced = find(&all, "example-review").unwrap();
    assert_eq!(
        (replaced.origin, replaced.title.as_str()),
        (FlowOrigin::Project, "Project review"),
        "项目的那份取代内置的：{all:?}"
    );
}
