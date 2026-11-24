//! LIVE：一轮对话经实例翻译成的领域事件（architecture §8.4 的对应表）——文字与用量、忙时退回、
//! 插话、排队、中断、权限与模型的提问及作答、侧问分叉、goal、后台任务的输出与停止、子 Agent
//! 与给它发消息。断言只看 Conix 的翻译与路由；模型怎么想、工具怎么跑是 Agix 的事。

mod live;

use std::time::Duration;

use live::{Answering, Live, QUICK, TURN};
use port::{
    Answer, Command, Event, GoalAction, GoalPhase, MessageId, Origin, PromptKind, Reply,
    Resolution, SessionId, SessionState, StopReason, SubmitMode, TaskKind, TaskStatus,
};

/// 够长的一轮：给插话、排队、中断留出时间。
const LONG: &str = "Count from 1 to 300, writing each number on its own line, with no other text.";

fn submitted(r: Reply) -> MessageId {
    match r {
        Reply::Submitted { message } => message,
        r => panic!("submit 回的是 {r:?}"),
    }
}

/// 等这个会话的这一轮开始说话（第一段文字到了）。
async fn speaking(c: &mut live::Client, s: &SessionId) {
    let s = s.clone();
    c.wait_event("开始输出", TURN, |e| {
        matches!(e, Event::TextDelta { session, .. } if *session == s).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_turn_streams_text_and_ends_with_usage() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    c.skip_past();
    c.submit(&s, "Reply with exactly: PONG", SubmitMode::Turn)
        .await;
    let s2 = s.clone();
    c.wait_event("这一轮开始", TURN, |e| {
        matches!(e, Event::TurnStarted { session } if *session == s2).then_some(())
    })
    .await;
    let t = c.finish_turn(&s).await;
    t.ended();
    assert!(t.text.contains("PONG"), "{}", t.text);
    assert!(t.usage.output > 0 && t.usage.input > 0, "{:?}", t.usage);
    // 摘要随之更新：一轮结束回到空闲，用量累计进去。
    let sum = c.sessions().await.into_iter().find(|x| x.id == s).unwrap();
    assert_eq!(sum.state, SessionState::Idle);
    assert!(sum.usage.output > 0);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_turn_while_busy_is_returned_and_a_steer_is_delivered() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    c.skip_past();
    c.submit(&s, LONG, SubmitMode::Turn).await;
    speaking(&mut c, &s).await;

    let busy = submitted(
        c.submit(&s, "Reply with exactly: X", SubmitMode::Turn)
            .await,
    );
    let s2 = s.clone();
    let returned = c
        .wait_event("忙时退回", TURN, |e| match e {
            Event::SteerReturned { session, messages } if *session == s2 => Some(messages.clone()),
            _ => None,
        })
        .await;
    assert_eq!(returned, vec![busy], "退回的就是那一句");

    let steer = submitted(
        c.submit(
            &s,
            "Stop counting now and reply with only the word STEERED.",
            SubmitMode::Steer,
        )
        .await,
    );
    let s2 = s.clone();
    let delivered = c
        .wait_event("插话送达", TURN, |e| match e {
            Event::SteerDelivered { session, message } if *session == s2 => Some(message.clone()),
            _ => None,
        })
        .await;
    assert_eq!(delivered, steer, "送达的是 Submitted 回的那个 id");
    let t = c.finish_turn(&s).await;
    t.ended();
    assert!(t.text.contains("STEERED"), "{}", t.text);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_queued_message_runs_as_the_next_turn() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    c.skip_past();
    c.submit(&s, LONG, SubmitMode::Turn).await;
    speaking(&mut c, &s).await;
    c.submit(&s, "Reply with exactly: QUEUED", SubmitMode::Queue)
        .await;
    c.finish_turn(&s).await.ended();
    let s2 = s.clone();
    c.wait_event("排着的那一轮开始", TURN, |e| {
        matches!(e, Event::TurnStarted { session } if *session == s2).then_some(())
    })
    .await;
    let t = c.finish_turn(&s).await;
    t.ended();
    assert!(t.text.contains("QUEUED"), "{}", t.text);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn interrupt_ends_the_turn_as_cancelled() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    c.skip_past();
    c.submit(&s, LONG, SubmitMode::Turn).await;
    speaking(&mut c, &s).await;
    c.ok(Command::Interrupt { session: s.clone() }).await;
    let t = c.finish_turn(&s).await;
    assert_eq!(t.end, Some(Ok(StopReason::Cancelled)), "{}", t.text);
    // 中断之后会话照常可用。
    c.turn(&s, "Reply with exactly: AGAIN").await.ended();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_permission_prompt_waits_for_the_answer() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Manual);
    let s = c.new_session().await;
    c.skip_past();
    c.submit(
        &s,
        "Use the Bash tool to run exactly this command: echo live-$((40+2)) . Then reply with \
         only its output.",
        SubmitMode::Turn,
    )
    .await;
    let p = c.wait_prompt(&s).await;
    let PromptKind::Permission { tool, .. } = &p.kind else {
        panic!("{p:?}")
    };
    assert_eq!(tool, "Bash");
    c.wait_upsert("会话在等人", |x| {
        x.id == s && x.state == SessionState::Waiting && x.waiting == 1
    })
    .await;
    c.ok(Command::Answer {
        session: s.clone(),
        prompt: p.id.clone(),
        answer: Answer::Permit,
    })
    .await;
    let (s2, id) = (s.clone(), p.id.clone());
    let res = c
        .wait_event("提问了结", TURN, |e| match e {
            Event::PromptResolved {
                session,
                id: pid,
                resolution,
            } if *session == s2 && *pid == id => Some(resolution.clone()),
            _ => None,
        })
        .await;
    assert_eq!(res, Resolution::Answered);
    let t = c.finish_turn(&s).await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| r.contains("live-42") && !err),
        "{:?}",
        t.results
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_model_question_is_answered_with_a_choice() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    c.skip_past();
    c.submit(
        &s,
        "Use the AskUserQuestion tool to ask me \"Pick a color\" with exactly two options: \
         \"red\" and \"blue\". After I answer, reply with only the color I picked.",
        SubmitMode::Turn,
    )
    .await;
    let p = c.wait_prompt(&s).await;
    let PromptKind::Question { options, .. } = &p.kind else {
        panic!("{p:?}")
    };
    let blue = options
        .iter()
        .position(|o| o.to_lowercase().contains("blue"))
        .unwrap_or_else(|| panic!("{options:?}"));
    c.ok(Command::Answer {
        session: s.clone(),
        prompt: p.id.clone(),
        answer: Answer::Choice { index: blue },
    })
    .await;
    let t = c.finish_turn(&s).await;
    t.ended();
    assert!(t.text.to_lowercase().contains("blue"), "{}", t.text);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_side_question_forks_a_session_and_reports_back() {
    let lv = Live::new();
    let mut c = lv.start().await;
    // 模型偶尔想把要记的词写进记忆（要权限）：这里不管它怎么做，一律放行。
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    c.turn(
        &s,
        "In this conversation the code word is MANGO. Do not use any tools. Reply with only OK.",
    )
    .await
    .ended();
    c.skip_past();
    c.ok(Command::SideQuestion {
        session: s.clone(),
        text: "What is the code word? Do not use any tools. Reply with only that word.".into(),
    })
    .await;
    let fork = c
        .wait_upsert(
            "分叉出的会话",
            |x| matches!(&x.provenance.origin, Origin::Fork { from, .. } if *from == s),
        )
        .await;
    assert!(
        fork.title.starts_with("What is the code word"),
        "{}",
        fork.title
    );
    let text = c
        .wait_event("侧问的回答", TURN, |e| match e {
            Event::Notice { text, .. } => Some(text.clone()),
            _ => None,
        })
        .await;
    assert!(text.to_uppercase().contains("MANGO"), "{text}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn goal_changes_are_pushed_and_in_the_snapshot() {
    let lv = Live::new();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    c.skip_past();
    let objective = "Write the word GOAL into a file named goal.txt in the project, then stop.";
    c.ok(Command::SetGoal {
        session: s.clone(),
        objective: objective.into(),
        max_rounds: Some(3),
    })
    .await;
    let s2 = s.clone();
    let g = c
        .wait_event("goal 设上了", TURN, |e| match e {
            Event::GoalChanged {
                session,
                goal: Some(g),
            } if *session == s2 => Some(g.clone()),
            _ => None,
        })
        .await;
    assert_eq!(g.objective, objective);
    assert_eq!(g.max_rounds, 3);
    let sum = c.sessions().await.into_iter().find(|x| x.id == s).unwrap();
    assert!(
        objective.starts_with(&sum.title),
        "没名字的 goal 会话用目标当名字：{}",
        sum.title
    );

    c.ok(Command::UpdateGoal {
        session: s.clone(),
        action: GoalAction::Pause,
    })
    .await;
    let s2 = s.clone();
    c.wait_event("goal 暂停", TURN, |e| {
        matches!(e, Event::GoalChanged { session, goal: Some(g) }
            if *session == s2 && g.phase == GoalPhase::Paused)
        .then_some(())
    })
    .await;
    // 正在跑的那一轮跑完，快照里看得到暂停的 goal。
    let w = loop {
        let w = c.watch(&s).await;
        if !w.running {
            break w;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    };
    assert_eq!(w.goal.as_ref().map(|g| g.phase), Some(GoalPhase::Paused));

    c.ok(Command::UpdateGoal {
        session: s.clone(),
        action: GoalAction::Clear,
    })
    .await;
    let s2 = s.clone();
    c.wait_event("goal 清掉", TURN, |e| {
        matches!(e, Event::GoalChanged { session, goal: None } if *session == s2).then_some(())
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_background_task_streams_output_and_can_be_stopped() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    c.skip_past();
    let from = c.mark();
    c.submit(
        &s,
        "Call the Monitor tool once with the command \
         `for i in $(seq 1 40); do echo tick $i; sleep 3; done` (it prints a line every few \
         seconds; that is intended). Do not wait for it. Once it has started, reply with only \
         the word STARTED.",
        SubmitMode::Turn,
    )
    .await;
    let s2 = s.clone();
    let task = c
        .wait_event("后台任务", TURN, |e| match e {
            // Agix 的 Bash 只在前台跑，长跑的后台命令是 Monitor：没有会话的任务。
            Event::TaskUpserted(t)
                if t.owner == s2
                    && t.kind == TaskKind::Monitor
                    && t.status == TaskStatus::Running =>
            {
                Some(t.clone())
            }
            _ => None,
        })
        .await;
    // 任务帧与这一轮的结束先后不定：从同一处再等结束。这一轮怎么收尾（说完、话太长……）
    // 是模型的事，只要它结束了。
    c.rewind_to(from);
    let t = c.finish_turn(&s).await;
    assert!(matches!(t.end, Some(Ok(_))), "{:?}", t.end);

    // 输出：先取一次，再等增量推过来。
    let mut text = String::new();
    let deadline = std::time::Instant::now() + QUICK;
    while !text.contains("tick 2") {
        let Reply::TaskOutput { text: t, .. } = c
            .ok(Command::TaskOutput {
                task: task.id.clone(),
                from_byte: 0,
            })
            .await
        else {
            panic!("task_output")
        };
        text = t;
        assert!(
            std::time::Instant::now() < deadline,
            "任务没有输出：{text:?}"
        );
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    let id = task.id.clone();
    c.wait_event("任务输出的增量", QUICK, |e| {
        matches!(e, Event::TaskOutput { task, text, .. } if *task == id && text.contains("tick"))
            .then_some(())
    })
    .await;
    let w = c.watch(&s).await;
    assert!(
        w.tasks
            .iter()
            .any(|t| t.id == task.id && !t.status.finished()),
        "快照里有这个任务：{:?}",
        w.tasks
    );

    c.ok(Command::StopTask {
        task: task.id.clone(),
    })
    .await;
    let id = task.id.clone();
    let st = c
        .wait_event("任务停了", QUICK, |e| match e {
            Event::TaskUpserted(t) if t.id == id && t.status.finished() => Some(t.status),
            _ => None,
        })
        .await;
    assert_eq!(st, TaskStatus::Stopped);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_sub_agent_shows_up_as_a_task_with_its_own_session() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    c.skip_past();
    c.submit(
        &s,
        "Use the Agent tool once, with run_in_background set to true, description \"probe\" and \
         this prompt: \"Run the Bash command `sleep 20` and then reply with only the word PONG.\" \
         Then reply with only the word LAUNCHED and do not wait for it.",
        SubmitMode::Turn,
    )
    .await;
    let s2 = s.clone();
    let task = c
        .wait_event("子 Agent 任务", TURN, |e| match e {
            Event::TaskUpserted(t)
                if t.owner == s2 && t.kind == TaskKind::Agent && t.agent_session.is_some() =>
            {
                Some(t.clone())
            }
            _ => None,
        })
        .await;
    let child = task.agent_session.clone().unwrap();
    // 侧链会话不进会话列表（ListSessions 只列主会话），但能 Watch：摘要里写着它从哪来。
    assert!(c.sessions().await.iter().all(|x| x.id != child));
    let w = c.watch(&child).await;
    assert!(
        matches!(&w.session.provenance.origin, Origin::SubAgent { parent, task: t, .. }
            if *parent == s && *t == task.id),
        "{:?}",
        w.session.provenance.origin
    );
    // Watch 着侧链会话时，父会话上的 subagent_progress 解包成它自己的事件。
    let ch = child.clone();
    c.wait_event("子 Agent 自己的事件", TURN, |e| {
        matches!(e, Event::ToolStarted { session, .. } | Event::TextDelta { session, .. }
            | Event::PromptPending { session, .. } if *session == ch)
        .then_some(())
    })
    .await;

    let r = c
        .call(Command::MessageTask {
            task: task.id.clone(),
            text: "When you are done, reply with only the word PONG.".into(),
        })
        .await;
    assert!(r.is_ok(), "给运行中的子 Agent 发消息：{r:?}");
    let id = task.id.clone();
    c.wait_event("子 Agent 结束", TURN, |e| {
        matches!(e, Event::TaskUpserted(t) if t.id == id && t.status.finished()).then_some(())
    })
    .await;
}
