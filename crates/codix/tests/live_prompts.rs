//! LIVE：权限审批的几种答法与引擎撤回、改文件的 diff（需求 C-02、C-04、C-05）。允许一次与模型提问的作答在
//! `live_turns.rs`；这里是拒绝、本会话总是允许、提问超时被引擎撤回，以及文件变更从真实引擎翻成带 diff 的事件。

mod live;

use live::{Answering, Live, TURN};
use port::{Answer, Command, Event, FileChangeKind, Resolution, SubmitMode};

/// 把实例的 `settings.json` 换成带额外键的（原有的提问超时放长保留）。
fn settings(lv: &Live, extra: serde_json::Value) {
    let mut v = serde_json::json!({"permissions": {"prompt_timeout_secs": 600}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    std::fs::write(
        lv.home.path().join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_denied_permission_stops_the_tool_and_the_turn_goes_on() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Deny);
    let s = c.new_session().await;
    let t = c
        .turn(
            &s,
            "Use the Bash tool to run exactly: touch denied.txt . If it is refused, do not retry or work \
             around it; reply with only the word REFUSED.",
        )
        .await;
    assert!(t.end.is_some(), "{t:?}");
    assert_eq!(t.prompts.len(), 1, "问了一次：{:?}", t.prompts);
    assert!(
        t.results.iter().any(|(_, err)| *err),
        "被拒的工具结果是出错：{:?}",
        t.results
    );
    assert!(
        !lv.project.path().join("denied.txt").exists(),
        "拒绝了就不该执行"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn allow_for_the_session_is_asked_once() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Manual);
    let s = c.new_session().await;
    c.skip_past();
    c.submit(
        &s,
        "Use the Bash tool to run exactly: touch first.txt . Then reply with only the word ONE.",
        SubmitMode::Turn,
    )
    .await;
    let p = c.wait_prompt(&s).await;
    c.ok(Command::Answer {
        session: s.clone(),
        prompt: p.id,
        answer: Answer::PermitSession,
    })
    .await;
    c.finish_turn(&s).await.ended();

    // 同样的命令再来一次不再问（总是允许记的是这条命令的规则，不是整个工具）：把自动作答改成拒绝，要是还问，工具就会被拒。
    c.answering(Answering::Deny);
    let t = c
        .turn(
            &s,
            "Use the Bash tool to run exactly: touch first.txt . Then reply with only the word TWO.",
        )
        .await;
    t.ended();
    assert!(
        t.prompts.is_empty(),
        "本会话总是允许之后不再问：{:?}",
        t.prompts
    );
    assert!(t.results.iter().all(|(_, err)| !err), "{:?}", t.results);
}

/// 引擎撤回提问（这里是超时）：实例推 `PromptResolved { Withdrawn }`，会话不再标成在等人，这一轮照样有结局。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn an_unanswered_prompt_is_withdrawn_by_the_engine_when_it_times_out() {
    let lv = Live::new();
    settings(
        &lv,
        serde_json::json!({"permissions": {"prompt_timeout_secs": 3}}),
    );
    let mut c = lv.start().await;
    c.answering(Answering::Manual);
    let s = c.new_session().await;
    c.skip_past();
    c.submit(
        &s,
        "Use the Bash tool to run exactly: touch late.txt . If it is refused, reply with only the word REFUSED.",
        SubmitMode::Turn,
    )
    .await;
    let p = c.wait_prompt(&s).await;
    let (s2, id) = (s.clone(), p.id.clone());
    let res = c
        .wait_event("提问被撤回", TURN, |e| match e {
            Event::PromptResolved {
                session,
                id: pid,
                resolution,
            } if *session == s2 && *pid == id => Some(resolution.clone()),
            _ => None,
        })
        .await;
    let Resolution::Withdrawn { reason } = res else {
        panic!("该是撤回：{res:?}")
    };
    assert!(reason.contains("time"), "原因是超时：{reason}");
    c.wait_upsert("会话不再在等人", |x| x.id == s && x.waiting == 0)
        .await;
    let t = c.finish_turn(&s).await;
    assert!(t.end.is_some(), "{t:?}");
    assert!(!lv.project.path().join("late.txt").exists());
}

/// 改文件的工具结果带着 `file_changes`：路径、种类、unified diff；文件真的改了。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_file_edit_arrives_as_a_change_with_a_diff() {
    let lv = Live::new();
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    let t = c
        .turn(
            &s,
            "Use the Edit tool to replace the text \"A tiny sample project\" with \"A small demo project\" in \
             README.md, then create a new file notes.txt containing the single line hello using the Write \
             tool. Do not ask me to confirm. Then reply with only the word DONE.",
        )
        .await;
    t.ended();
    let changes: Vec<port::FileChange> = c
        .events()
        .into_iter()
        .filter_map(|e| match e {
            Event::ToolFinished { file_changes, .. } => Some(file_changes),
            _ => None,
        })
        .flatten()
        .collect();
    let readme = changes
        .iter()
        .find(|f| f.path.ends_with("README.md"))
        .unwrap_or_else(|| panic!("{changes:?}"));
    assert_eq!(readme.kind, FileChangeKind::Modified);
    let diff = readme.diff.as_deref().unwrap_or_default();
    assert!(
        diff.contains("-A tiny sample project") || diff.contains("A tiny sample project"),
        "{diff}"
    );
    assert!(diff.contains("+A small demo project"), "{diff}");
    let notes = changes
        .iter()
        .find(|f| f.path.ends_with("notes.txt"))
        .unwrap_or_else(|| panic!("{changes:?}"));
    assert_eq!(notes.kind, FileChangeKind::Created);
    let on_disk = std::fs::read_to_string(lv.project.path().join("README.md")).unwrap();
    assert!(on_disk.contains("A small demo project"), "{on_disk}");
}
