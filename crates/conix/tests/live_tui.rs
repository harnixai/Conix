//! LIVE：界面经真实实例走一遍（testing.md §8）——同一套键位（交互 §7）在真实后端上：^G 切块、
//! ^N 换标签、^T、^O、^R、^H、^X；草稿上第一句话建出会话、`/rename`、`/trust` 是真实例回的。

mod live;

use std::time::Duration;

use app::core::ShellEffect;
use live::tui::Tui;
use live::{Live, QUICK, TURN};
use model::{Focus, Tab};
use port::Instances;

const BASICS: &str = "^G 切块 · ^N 标签 · ^B 切栏 · ^X 退出 · ^H 帮助";

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_keys_walk_the_workbench_of_a_real_instance() {
    let lv = Live::new();
    let mut t = Tui::create(&lv).await;
    assert!(t.core.state.ui.chat.is_none(), "启动是草稿");
    assert_eq!(t.core.state.ui.focus, Focus::Composer);
    assert!(t.last_line().ends_with(BASICS), "{}", t.last_line());

    // ^N 轮换标签，焦点进右上；流程标签里有内置的示例流程（真实例的流程库给的）。
    t.keys("^N").await;
    assert_eq!(
        (t.core.state.ui.tab, t.core.state.ui.focus),
        (Tab::Flows, Focus::SideUpper)
    );
    t.until("示例流程", QUICK, |t| {
        t.core
            .state
            .domain
            .flows
            .iter()
            .any(|f| f.name == "example-review")
    })
    .await;
    t.keys("^N").await;
    assert_eq!(t.core.state.ui.tab, Tab::Jobs);
    t.keys("^N").await;
    assert_eq!(t.core.state.ui.tab, Tab::Sessions);

    // ^G：右上 → 右下 → 输入框 → 右上。
    t.keys("^G").await;
    assert_eq!(t.core.state.ui.focus, Focus::SideLower);
    t.keys("^G").await;
    assert_eq!(t.core.state.ui.focus, Focus::Composer);
    t.keys("^G").await;
    assert_eq!(t.core.state.ui.focus, Focus::SideUpper);

    // ^T：草稿没有面板，提示一句，焦点回输入框。
    t.keys("^T").await;
    assert_eq!(t.core.state.ui.focus, Focus::Composer);
    assert!(t.core.state.ui.notice.is_some());

    // ^H：输入框有字时退格，空着时是帮助。
    t.type_str("ab").await;
    t.keys("^H").await;
    assert_eq!(t.core.state.ui.composer.text, "a");
    t.keys("^U ^H").await;
    assert!(t.core.state.ui.overlay.is_some(), "空输入框 ^H 打开帮助");
    t.keys("Esc").await;

    // 单栏：^G 转到右栏时自动换栏显示，底栏那几项仍在。
    t.size = tui::Size::new(100, 30);
    let s = t.core.resize(t.size);
    t.shell.extend(s);
    t.keys("^G").await;
    assert_eq!(t.core.state.ui.showing, tui::ShowPane::Side);
    assert!(t.last_line().ends_with(BASICS), "{}", t.last_line());
    t.keys("^G ^G").await;
    assert_eq!(t.core.state.ui.showing, tui::ShowPane::Chat);

    // `/trust` 经界面发给实例，实例回了才有提示。
    t.say("/trust").await;
    t.until("信任的回复", QUICK, |t| {
        t.core
            .state
            .ui
            .notice
            .as_ref()
            .is_some_and(|n| !n.error && n.text.contains("已信任"))
    })
    .await;

    // ^X → 后台运行：界面退出（脱离），实例还在跑。
    t.keys("^X Enter").await;
    assert!(
        t.shell.contains(&ShellEffect::Quit { detach: true }),
        "{:?}",
        t.shell
    );
    t.core.detach().await;
    let list = lv.instances.list(false).await.unwrap();
    assert!(
        list.iter()
            .any(|i| i.name.0 == live::NAME && i.state == port::InstanceState::Running),
        "{list:?}"
    );
    // 再接回来，还是同一个实例。
    let t = Tui::attach(&lv).await;
    assert_eq!(
        t.core
            .state
            .domain
            .instance
            .as_ref()
            .map(|i| i.name.0.as_str()),
        Some(live::NAME)
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_first_line_in_the_draft_makes_a_session_and_ctrl_o_reads_it() {
    let lv = Live::new();
    let mut t = Tui::create(&lv).await;
    t.say("Reply with exactly one word: pong").await;
    t.until("草稿建出会话", QUICK, |t| {
        t.core.state.ui.chat.is_some()
    })
    .await;
    let id = t.core.state.ui.chat.clone().unwrap();
    t.until("一轮跑完", TURN, |t| {
        t.idle() && t.transcript().to_lowercase().contains("pong")
    })
    .await;
    assert!(
        t.core.state.domain.sessions.iter().any(|s| s.id == id),
        "会话列表里有它"
    );

    // ^O 看转录（详细），^O 回输入框收回精简。
    t.keys("^O").await;
    assert_eq!(t.core.state.ui.focus, Focus::Transcript);
    assert!(t.core.state.ui.detail);
    t.keys("^O").await;
    assert_eq!(t.core.state.ui.focus, Focus::Composer);
    assert!(!t.core.state.ui.detail);

    // ^G 到右上：选中的就是左边这个会话。
    t.keys("^G").await;
    assert_eq!(t.core.state.selected_session(), Some(id.clone()));

    // `/rename` 经实例改名，列表跟着变。
    t.keys("^G ^G").await;
    t.say("/rename 乒乓").await;
    t.until("改名回来", QUICK, |t| {
        t.core
            .state
            .domain
            .sessions
            .iter()
            .any(|s| s.id == id && s.title == "乒乓")
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn ctrl_r_finds_the_permission_prompt_and_the_answer_reaches_the_instance() {
    let lv = Live::new();
    let mut t = Tui::create(&lv).await;
    t.say(
        // 带算术展开：纯 echo 是只读命令，Agix 不问人。
        "Use the Bash tool to run exactly this command: echo live-tui-$((40+2)) . Then tell me what it printed.",
    )
    .await;
    t.until("等人批准", TURN, |t| t.core.state.ask_visible())
        .await;
    // 输入框空着，提问框直接拿了键盘，底栏是它的键；待处理里有这一处。
    assert!(t.core.state.ui.ask_active);
    assert_eq!(t.core.state.attention().len(), 1);

    // 走开到右栏，再用 ^R 找回来；^G 转回左边就进提问框。
    t.keys("^G").await;
    assert!(t.core.state.ui.focus.in_side());
    t.keys("^R").await;
    assert_eq!(
        t.core.state.ui.tab,
        Tab::Sessions,
        "跳到等人的那个会话所在的标签"
    );
    while t.core.state.ui.focus != Focus::Composer {
        t.keys("^G").await;
    }
    assert!(t.core.state.ui.ask_active, "回到左边就进提问框");
    t.keys("Enter").await;
    t.until("批准之后跑完", TURN, |t| {
        t.idle() && !t.core.state.ask_visible() && t.transcript().contains("live-tui-42")
    })
    .await;
    // 实例那边确实执行了（不是界面自己画的）：工具结果在转录里。
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(t.transcript().contains("Bash"), "{}", t.transcript());
}
