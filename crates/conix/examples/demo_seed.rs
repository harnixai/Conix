//! 给演示用的真实实例灌一批数据（由 `scripts/demo.sh` 调用）：用真实模型跑几个会话，让
//! todo、子 Agent、后台任务、goal、待答的提问、作业与触发记录、宿主工具、一次停在待确认的流程运行都有东西可看。
//!
//! 用法：`CONIX_HOME=<demo/home> demo_seed <conix 二进制> <示例项目目录>`，凭据从环境变量来。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client::{Layout, SocketInstances};
use port::{
    Answer, Backend, Command, Event, Instances, Job, JobId, JobKind, NewInstance, PromptKind,
    Reply, SessionId, SubmitMode,
};

/// 每一段：开场白与要不要替它答权限（最后一段留一个提问给人看）。
const TALKS: &[(&str, bool)] = &[
    (
        "先读一遍这个项目的代码，然后用 TodoWrite 列出 5 个改进点作为待办，把第一个标为进行中、第二个标为完成。最后只回复一句话总结。",
        true,
    ),
    (
        "用 Agent 工具并行派两个子 Agent：一个去说明 shop 目录下每个文件是做什么的，另一个去找代码里的 bug。等它们都回来后，把结果汇总成不超过五行。",
        true,
    ),
    (
        "调用 Bash 工具，参数 run_in_background 设为 true，命令是：for i in $(seq 1 3600); do echo tick $i; sleep 2; done 。启动之后直接回复“已在后台运行”，不要读它的输出、不要等它结束、不要停掉它。",
        true,
    ),
    (
        "先用 job_list 看看现在有哪些作业，然后用 job_save 建一个作业：名字“每周清理”，cron 是 0 18 * * 5，提示词是“清理 shop 里的临时文件并汇报”。不用再问我确认。最后只回复一句话。",
        true,
    ),
    (
        "我想给 shop 加一个导出功能。动手之前先用 AskUserQuestion 工具问我一个问题：导出成 CSV 还是 JSON（给这两个选项）。问完就停下等我回答，不要自己决定。",
        false,
    ),
];

fn home() -> PathBuf {
    std::env::var_os("CONIX_HOME")
        .map(PathBuf::from)
        .expect("set CONIX_HOME to the demo home")
}

async fn call(b: &Arc<dyn Backend>, cmd: Command) -> Reply {
    let name = cmd.name();
    b.call(cmd).await.unwrap_or_else(|e| panic!("{name}: {e}"))
}

async fn new_session(b: &Arc<dyn Backend>) -> SessionId {
    let Reply::Session(s) = call(
        b,
        Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: None,
        },
    )
    .await
    else {
        panic!("create_session")
    };
    call(
        b,
        Command::Watch {
            session: s.id.clone(),
        },
    )
    .await;
    s.id
}

fn job(name: &str, cron: &str, prompt: &str, enabled: bool) -> Job {
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
        enabled,
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

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let exe = PathBuf::from(args.next().expect("conix binary"));
    let project = PathBuf::from(args.next().expect("project dir"))
        .canonicalize()
        .expect("project dir exists");
    let instances = SocketInstances::new(Layout::new(home()), exe);

    let all = instances.list(true).await.unwrap();
    let b: Arc<dyn Backend> = match client::for_project(&all, &project) {
        Some(i) if i.state == port::InstanceState::Running => instances
            .attach(&i.name.0, port::Attach { force: true })
            .await
            .unwrap(),
        Some(i) => instances.start(&i.name.0).await.unwrap(),
        None => {
            instances
                .create(NewInstance {
                    name: None,
                    project: Some(project.to_string_lossy().into_owned()),
                })
                .await
                .unwrap()
                .1
        }
    };
    let mut events = b.events();

    // 自动作答的会话：权限一律允许、问题选第一个选项。没列进来的会话的提问留着给人看。
    let auto: Arc<Mutex<HashSet<SessionId>>> = Arc::default();
    let pending: Arc<Mutex<HashSet<SessionId>>> = Arc::default();
    let done = Arc::new(tokio::sync::Notify::new());
    {
        let (b, auto, pending, done) = (b.clone(), auto.clone(), pending.clone(), done.clone());
        tokio::spawn(async move {
            while let Some(ev) = events.recv().await {
                match ev {
                    Event::PromptPending { session, prompt } => {
                        if !auto.lock().unwrap().contains(&session) {
                            println!("  ◐ 留了一个提问在 {session} 等你回答");
                            continue;
                        }
                        let answer = match prompt.kind {
                            PromptKind::Permission { .. } => Answer::Permit,
                            PromptKind::Question { .. } => Answer::Choice { index: 0 },
                        };
                        let _ = b
                            .call(Command::Answer {
                                session,
                                prompt: prompt.id,
                                answer,
                            })
                            .await;
                    }
                    Event::TurnEnded { session, .. } | Event::TurnFailed { session, .. }
                        if pending.lock().unwrap().remove(&session) =>
                    {
                        println!("  ✓ {session} 这一轮结束");
                        done.notify_one();
                    }
                    _ => {}
                }
            }
        });
    }

    println!("会话：");
    let mut waits = 0;
    for (text, answered) in TALKS {
        let id = new_session(&b).await;
        if *answered {
            auto.lock().unwrap().insert(id.clone());
            pending.lock().unwrap().insert(id.clone());
            waits += 1;
        }
        call(
            &b,
            Command::Submit {
                session: id.clone(),
                text: text.to_string(),
                mode: SubmitMode::Turn,
            },
        )
        .await;
        println!("  • {id}: {}", text.chars().take(30).collect::<String>());
    }

    println!("goal：");
    let goal = new_session(&b).await;
    auto.lock().unwrap().insert(goal.clone());
    call(
        &b,
        Command::SetGoal {
            session: goal.clone(),
            objective:
                "给 shop 写一份 README.md：说明它是什么、怎么运行、有哪些命令。写完就标记完成。"
                    .into(),
            max_rounds: Some(4),
        },
    )
    .await;
    println!("  • {goal}");

    println!("作业：");
    let Reply::Job(daily) = call(
        &b,
        Command::SaveJob {
            job: job(
                "每日依赖检查",
                "0 9 * * *",
                "检查 shop 用到的依赖，列出可能需要更新或有问题的，三行以内。",
                true,
            ),
        },
    )
    .await
    else {
        panic!("save_job")
    };
    call(
        &b,
        Command::SaveJob {
            job: job(
                "半小时巡检（已停用）",
                "*/30 * * * *",
                "跑一遍 shop 的自测，报告结果。",
                false,
            ),
        },
    )
    .await;
    call(
        &b,
        Command::RunJobNow {
            job: daily.id.clone(),
            subject: None,
            prompt_append: None,
        },
    )
    .await;
    println!("  • 每日依赖检查（已立即触发一次） · 半小时巡检（停用）");

    println!("流程：");
    // 项目只跑装进项目的流程（flows.md §7.12）：把流程库自带的示例流程放进项目，项目受信才读。
    let flows = project.join(".conix/flows");
    for (rel, text) in [
        (
            "example-review.yaml",
            include_str!("../../flow/packs/examples/flows/example-review.yaml"),
        ),
        (
            "skills/review/SKILL.md",
            include_str!("../../flow/packs/examples/skills/review/SKILL.md"),
        ),
        (
            "rules/review-honest.md",
            include_str!("../../flow/packs/examples/rules/review-honest.md"),
        ),
        (
            "policies/read-only.yaml",
            include_str!("../../flow/packs/examples/policies/read-only.yaml"),
        ),
    ] {
        let p = flows.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    call(&b, Command::TrustProject { trusted: true }).await;
    call(&b, Command::ReloadFlows).await;
    match b
        .call(Command::StartRun {
            flow: "example-review".into(),
            subject: "审查 shop 的代码".into(),
            inputs: serde_json::json!({}),
            step: false,
            session: None,
        })
        .await
    {
        Ok(_) => println!("  • 示例流程 example-review：审查完停在“待确认”那一站"),
        Err(e) => println!("  流程没起来：{e}"),
    }

    println!("等前几段跑完（至多 5 分钟）……");
    let deadline = tokio::time::Instant::now() + Duration::from_secs(300);
    for _ in 0..waits {
        if tokio::time::timeout_at(deadline, done.notified())
            .await
            .is_err()
        {
            println!("  有的还没跑完，不等了：它们会在实例里接着跑");
            break;
        }
    }
    let _ = b.call(Command::Detach).await;
    println!("好了。实例在后台接着跑（goal 会自己往下推进、后台任务一直在 tick）。");
}
