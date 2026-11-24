//! 键位（交互 §3、§7）：命令键只用 Ctrl+字母——^G 切块、^N 换标签、^T 面板、^O 看转录、
//! ^H 帮助、^R 待处理（m14 在 crosscut），底栏最右边永远是最基本的那几项。

mod common;

use common::Harness;
use model::{Focus, Tab};
use port::{Event, Prompt, PromptKind};
use tui::{ShowPane, Size};

const WIDE: Size = Size::new(160, 45);
const BASICS: &str = "^G 切块 · ^N 标签 · ^B 切栏 · ^X 退出 · ^H 帮助";

#[tokio::test(start_paused = true)]
async fn the_basics_are_always_at_the_right_of_the_footer() {
    for cols in [60u16, 80, 120, 160] {
        let mut h = Harness::new("default", Size::new(cols, 30)).await;
        let script = [
            "", "^O", "^N", "^N", "^N", "^G", "Tab", "F1", "Esc", "^H", "Esc", "^T", "^C",
        ];
        for step in script {
            if !step.is_empty() {
                h.keys(step).await;
            }
            let last = h.last_line();
            assert!(
                last.ends_with(BASICS),
                "{cols} 列、{step:?} 之后底栏是：{last:?}"
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn ctrl_g_walks_composer_upper_lower_and_skips_transcript_and_panel() {
    let mut h = Harness::new("default", WIDE).await;
    h.open_session("重构认证模块").await;
    h.keys("Esc").await;
    assert!(h.core.state.panel_open(), "这个会话有面板");
    let mut seen = vec![h.core.state.ui.focus];
    for _ in 0..3 {
        h.keys("^G").await;
        seen.push(h.core.state.ui.focus);
    }
    assert_eq!(
        seen,
        [
            Focus::Composer,
            Focus::SideUpper,
            Focus::SideLower,
            Focus::Composer
        ],
        "一圈三格，面板与转录都不进"
    );
    let tab = h.core.state.ui.tab;
    h.keys("^G").await;
    assert_eq!(h.core.state.ui.tab, tab, "^G 不换标签");
    // 从转录出发也是去右上。
    h.keys("^G ^G ^O ^G").await;
    assert_eq!(h.core.state.ui.focus, Focus::SideUpper);
}

#[tokio::test(start_paused = true)]
async fn ctrl_g_switches_the_pane_when_only_one_fits() {
    let mut h = Harness::new("default", Size::new(100, 30)).await;
    assert_eq!(h.core.state.ui.showing, ShowPane::Chat);
    h.keys("^G").await;
    assert_eq!(h.core.state.ui.showing, ShowPane::Side);
    assert_eq!(h.core.state.ui.focus, Focus::SideUpper);
    h.keys("^G").await;
    assert_eq!(h.core.state.ui.showing, ShowPane::Side, "右下还在右栏");
    h.keys("^G").await;
    assert_eq!(h.core.state.ui.showing, ShowPane::Chat);
    assert_eq!(h.core.state.ui.focus, Focus::Composer);
}

#[tokio::test(start_paused = true)]
async fn ctrl_g_back_to_the_left_enters_the_waiting_question() {
    let mut h = Harness::new("default", WIDE).await;
    h.open_session("昨天的调研").await;
    h.type_str("正在打").await;
    let session = h.core.state.ui.chat.clone().unwrap();
    h.core
        .handle(model::Msg::Event(Box::new(Event::PromptPending {
            session,
            prompt: Prompt {
                id: "px".into(),
                kind: PromptKind::Permission {
                    tool: "Bash".into(),
                    summary: "ls".into(),
                    paths: vec![],
                },
                from: None,
                asked_ms: 0,
                permit_always: true,
                via: None,
            },
        })));
    h.settle().await;
    assert!(h.core.state.ask_visible() && !h.core.state.ui.ask_active);
    h.keys("^G ^G ^G").await;
    assert_eq!(h.core.state.ui.focus, Focus::Composer);
    assert!(h.core.state.ui.ask_active, "转回左边就进提问框");
    assert_eq!(h.core.state.ui.composer.text, "正在打", "打的字留着");
}

#[tokio::test(start_paused = true)]
async fn ctrl_n_turns_the_tabs_then_ctrl_g_goes_to_that_tabs_lower_block() {
    let mut h = Harness::new("default", WIDE).await;
    assert_eq!(h.core.state.ui.tab, Tab::Sessions);
    let mut seen = vec![];
    for _ in 0..3 {
        h.keys("^N").await;
        assert_eq!(h.core.state.ui.focus, Focus::SideUpper, "焦点进右上");
        seen.push(h.core.state.ui.tab);
    }
    assert_eq!(seen, [Tab::Flows, Tab::Jobs, Tab::Sessions]);
    h.keys("^N ^G").await;
    assert_eq!(
        (h.core.state.ui.tab, h.core.state.ui.focus),
        (Tab::Flows, Focus::SideLower)
    );
    // 鼠标点标签头一样能换。
    let (col, row) = h.find("作业").expect("作业标签");
    h.click(col + 1, row);
    assert_eq!(h.core.state.ui.tab, Tab::Jobs);
    h.keys("Esc").await;
    assert_eq!(h.core.state.ui.focus, Focus::Composer);
}

#[tokio::test(start_paused = true)]
async fn ctrl_t_toggles_the_panel_and_hands_the_keyboard_to_the_composer() {
    let mut h = Harness::new("default", WIDE).await;
    h.open_session("重构认证模块").await;
    assert!(h.core.state.panel_open());
    h.keys("^G").await;
    assert!(h.core.state.ui.focus.in_side());
    h.keys("^T").await;
    assert!(!h.core.state.panel_open());
    assert_eq!(h.core.state.ui.focus, Focus::Composer);
    assert!(h.text().contains("^T 展开"), "收起后标题线上标着怎么打开");
    h.keys("^T").await;
    assert!(h.core.state.panel_open());
}

#[tokio::test(start_paused = true)]
async fn ctrl_o_reads_the_transcript_and_any_way_out_brings_brief_back() {
    let mut h = Harness::new("m03-tools", WIDE).await;
    h.open_session("昨天的调研").await;
    h.type_str("改一下校验").await;
    h.keys("Enter").await;
    h.advance(5_000).await;
    for out in ["^O", "Esc", "^G", "^R"] {
        h.keys("^O").await;
        assert_eq!(h.core.state.ui.focus, Focus::Transcript);
        assert!(h.core.state.ui.detail, "看转录就是详细");
        h.keys(out).await;
        assert_ne!(h.core.state.ui.focus, Focus::Transcript, "{out} 离开转录");
        assert!(!h.core.state.ui.detail, "{out} 离开后回到精简");
        if h.core.state.ui.focus.in_side() {
            h.keys("Esc").await;
        }
    }
}

#[tokio::test(start_paused = true)]
async fn in_the_transcript_o_toggles_brief_and_brackets_pick_blocks() {
    let mut h = Harness::new("m03-tools", WIDE).await;
    h.open_session("昨天的调研").await;
    h.type_str("改一下校验").await;
    h.keys("Enter").await;
    h.advance(5_000).await;
    h.keys("^O o").await;
    assert_eq!(h.core.state.ui.focus, Focus::Transcript);
    assert!(!h.core.state.ui.detail, "o 收回精简，人还在转录里");
    let brief = h.core.state.conv().unwrap().total_lines();
    h.keys("[ [ Enter").await;
    assert!(
        h.core.state.conv().unwrap().total_lines() > brief,
        "[ 选块，Enter 展开它"
    );
    h.keys("] Enter").await;
    h.keys("[ Enter").await;
    assert_eq!(h.core.state.conv().unwrap().total_lines(), brief);
    h.keys("o").await;
    assert!(h.core.state.ui.detail);
}

#[tokio::test(start_paused = true)]
async fn ctrl_h_erases_while_typing_and_is_help_otherwise() {
    let mut h = Harness::new("default", WIDE).await;
    h.type_str("abc").await;
    h.keys("^H").await;
    assert_eq!(
        h.core.state.ui.composer.text, "ab",
        "有的终端退格发的就是 ^H"
    );
    assert!(h.core.state.ui.overlay.is_none());
    h.keys("F1").await;
    assert!(h.core.state.ui.overlay.is_some(), "F1 有字时也是帮助");
    h.keys("Esc ^U ^H").await;
    assert!(h.core.state.ui.overlay.is_some(), "空输入框 ^H 是帮助");
    h.keys("Esc ^N ^H").await;
    assert!(h.core.state.ui.overlay.is_some(), "右栏里 ^H 是帮助");
}

#[tokio::test(start_paused = true)]
async fn the_help_lists_the_focused_context_with_shared_keys_on_one_line() {
    let mut h = Harness::new("default", WIDE).await;
    h.tab(Tab::Sessions).await;
    h.keys("F1").await;
    let text = h.text();
    assert!(text.contains("帮助"));
    assert!(text.contains("会话"), "列出会话上下文的键位");
    assert!(text.contains("重命名"));
    assert!(text.contains("^H / F1"), "同一动作的几个键排一行\n{text}");
    assert!(!text.contains("Alt+"), "兼容键不列\n{text}");
    assert!(
        !text.contains("F2") && !text.contains("F6"),
        "F 键只留 F1\n{text}"
    );
}

#[tokio::test(start_paused = true)]
async fn the_terminals_own_editing_keys_still_work() {
    let mut h = Harness::new("default", WIDE).await;
    h.type_str("one two").await;
    h.keys("Alt+B").await;
    assert_eq!(
        h.core.state.ui.composer.cursor, 4,
        "Esc b（macOS 的 Option+←）按词左移"
    );
    h.keys("Alt+F").await;
    assert_eq!(h.core.state.ui.composer.cursor, 7);
    h.keys("Alt+Enter").await;
    h.type_str("x").await;
    assert_eq!(h.core.state.ui.composer.text, "one two\nx");
}

#[tokio::test(start_paused = true)]
async fn the_rare_ones_are_slash_commands() {
    let mut h = Harness::new("default", WIDE).await;
    h.type_str("/mouse").await;
    h.keys("Enter").await;
    assert!(
        h.shell
            .contains(&app::core::ShellEffect::SetMouseCapture(false))
    );
    h.type_str("/mouse").await;
    h.keys("Enter").await;
    assert!(
        h.shell
            .contains(&app::core::ShellEffect::SetMouseCapture(true))
    );
    h.type_str("/events").await;
    h.keys("Enter").await;
    assert!(h.text().contains("事件日志"));
}
