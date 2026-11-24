//! LIVE：宿主工具（architecture §8.5）——每个会话都有、查看不用问、改东西经权限询问、
//! 作用域按发起调用的会话、目标用 #n 或 ID。

mod live;

use live::{Answering, Live, TURN};
use port::{Command, Event, Job, JobId, JobKind, JobOutcome, PromptKind, Reply};

fn job(name: &str, prompt: &str) -> Job {
    Job {
        id: JobId(String::new()),
        name: name.into(),
        cron: "0 4 * * *".into(),
        kind: JobKind::Simple {
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

async fn jobs(c: &live::Client) -> Vec<Job> {
    match c.ok(Command::ListJobs).await {
        Reply::Jobs(v) => v,
        r => panic!("list_jobs 回的是 {r:?}"),
    }
}

fn asked_for(t: &live::TurnOutcome, tool: &str) -> bool {
    t.prompts
        .iter()
        .any(|p| matches!(&p.kind, PromptKind::Permission { tool: x, .. } if x == tool))
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_user_session_manages_jobs_and_names_itself_with_host_tools() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    c.watch(&s).await;

    let t = c
        .turn(
            &s,
            "Call the job_save tool once with name \"live-tool\", cron \"0 4 * * *\" and prompt \
             \"say hi\". Do not ask me to confirm. Then reply with only the word SAVED.",
        )
        .await;
    t.ended();
    assert!(t.tools.iter().any(|n| n == "job_save"), "{:?}", t.tools);
    assert!(asked_for(&t, "job_save"), "改东西的工具经权限询问");
    let made = jobs(&c)
        .await
        .into_iter()
        .find(|j| j.name == "live-tool")
        .expect("作业建出来了");
    assert_eq!(made.cron, "0 4 * * *");

    let t = c
        .turn(
            &s,
            "Call job_list, then disable job #1 with job_enable (enabled false). Do not ask me to \
             confirm. Then reply with only the word DONE.",
        )
        .await;
    t.ended();
    assert!(t.tools.iter().any(|n| n == "job_list"), "{:?}", t.tools);
    assert!(!asked_for(&t, "job_list"), "查看类工具不问");
    assert!(
        t.results.iter().any(|(r, err)| !err && r.contains("#1  ")),
        "列表按 #n：{:?}",
        t.results
    );
    assert!(
        !jobs(&c).await.iter().any(|j| j.enabled),
        "#1 就是那个作业，停用了"
    );

    let t = c
        .turn(
            &s,
            "Rename this session to \"工具改的名\" with session_rename (no target). Do not ask me \
             to confirm. Then reply with only the word OK.",
        )
        .await;
    t.ended();
    let me = s.clone();
    c.wait_event("会话改了名", TURN, |e| {
        matches!(e, Event::SessionUpserted(x) if x.id == me && x.title == "工具改的名")
            .then_some(())
    })
    .await;

    let t = c
        .turn(
            &s,
            &format!(
                "Delete the job with id {} using job_delete. Do not ask me to confirm. Then reply \
                 with only the word DELETED.",
                made.id
            ),
        )
        .await;
    t.ended();
    assert!(jobs(&c).await.is_empty(), "按 ID 删掉了");
}

/// 作业触发出来的会话只能动它自己的作业：让它去删另一个作业，那个作业还在。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_job_session_cannot_touch_another_job() {
    let lv = Live::new();
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let other = match c
        .ok(Command::SaveJob {
            job: job("other", "say hi"),
        })
        .await
    {
        Reply::Job(j) => j,
        r => panic!("{r:?}"),
    };
    let prompt = format!(
        "First call job_list. Then call job_delete with target \"{}\". Report what the tool \
         answered in one line.",
        other.id
    );
    let mine = match c
        .ok(Command::SaveJob {
            job: job("intruder", &prompt),
        })
        .await
    {
        Reply::Job(j) => j,
        r => panic!("{r:?}"),
    };
    c.ok(Command::RunJobNow {
        job: mine.id.clone(),
        subject: None,
        prompt_append: None,
    })
    .await;
    let id = mine.id.clone();
    let done = c
        .wait_event("触发的结局", TURN, |e| match e {
            Event::JobFired { job, entry }
                if *job == id && entry.outcome != JobOutcome::Running =>
            {
                Some(entry.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(done.outcome, JobOutcome::Done, "{done:?}");
    assert!(
        jobs(&c).await.iter().any(|j| j.id == other.id),
        "另一个作业还在"
    );
}
