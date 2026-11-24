//! 场景 m17（布局）与框架级的选中、鼠标、退出。键位（含 m24 底栏保底）在 keys.rs，m23 的尺寸协商在 term.rs。

mod common;

use common::Harness;
use model::{Focus, Tab};
use tui::Size;

const WIDE: Size = Size::new(160, 45);

#[tokio::test(start_paused = true)]
async fn m17_wide_shows_both_panes() {
    let mut h = Harness::new("default", WIDE).await;
    assert!(h.core.state.domain.instance.is_some(), "Hello 已回来");
    assert!(!h.core.state.domain.sessions.is_empty());
    h.snapshot("m17-wide-160x45");
}

#[tokio::test(start_paused = true)]
async fn m17_collapse_and_ratio() {
    let mut h = Harness::new("default", WIDE).await;
    h.keys("Ctrl+B").await;
    assert!(h.core.state.ui.side_collapsed);
    assert!(h.find("会话 ②").is_none(), "右栏收起后标签头不在");
    h.keys("Ctrl+B").await;
    h.tab(Tab::Sessions).await;
    h.keys("> > > > > > >").await;
    assert_eq!(h.core.state.ui.ratio_pct, 75, "上限 75%");
    h.keys("< < < < < < < < < <").await;
    assert_eq!(h.core.state.ui.ratio_pct, 40, "下限 40%");
}

#[tokio::test(start_paused = true)]
async fn m17_drag_divider() {
    let mut h = Harness::new("default", WIDE).await;
    let (col, row) = h.find("╮╭").expect("两栏相接的边框");
    h.drag((col, row + 3), (80, row + 3));
    assert_eq!(h.core.state.ui.ratio_pct, 50);
    assert!(h.core.state.ui.drag.is_none(), "松开后不再拖动");
}

#[tokio::test(start_paused = true)]
async fn m17_single_pane_switches_back_and_forth() {
    let mut h = Harness::new("default", Size::new(100, 30)).await;
    h.snapshot("m17-single-chat-100x30");
    h.keys("Ctrl+B").await;
    assert_eq!(h.core.state.ui.focus, Focus::SideUpper);
    h.snapshot("m17-single-side-100x30");
    h.keys("Ctrl+B").await;
    assert_eq!(h.core.state.ui.focus, Focus::Composer);
    assert!(
        h.find(&format!("Conix {}", model::short_version()))
            .is_some(),
        "欢迎块"
    );
}

#[tokio::test(start_paused = true)]
async fn m17_too_small_still_has_footer() {
    let mut h = Harness::new("default", Size::new(59, 20)).await;
    let text = h.text();
    assert!(text.contains("终端太小"), "{text}");
    assert!(
        h.last_line()
            .ends_with("^G 切块 · ^N 标签 · ^B 切栏 · ^X 退出 · ^H 帮助")
    );
}

#[tokio::test(start_paused = true)]
async fn side_selection_by_keys_click_and_wheel() {
    let mut h = Harness::new("default", WIDE).await;
    h.tab(model::Tab::Sessions).await;
    h.keys("Down Down").await;
    assert_eq!(h.core.state.ui.upper().selected, 2);
    let n = h.core.state.session_rows().len();
    h.keys("PageDown").await;
    assert_eq!(h.core.state.ui.upper().selected, n - 1, "停在最后一项");
    let second = h.core.state.user_sessions()[1].title.clone();
    let (col, row) = h.find(&second).unwrap();
    h.click(col, row);
    assert_eq!(h.core.state.ui.upper().selected, 2, "第 0 行是新建会话");
    h.mouse(model::MouseKind::ScrollDown, col, row);
    assert_eq!(h.core.state.ui.upper().selected, 5);
}

#[tokio::test(start_paused = true)]
async fn ctrl_c_twice_detaches_and_mouse_command_releases_the_mouse() {
    let mut h = Harness::new("default", WIDE).await;
    h.type_str("/mouse").await;
    h.keys("Enter").await;
    assert!(
        h.shell
            .contains(&app::core::ShellEffect::SetMouseCapture(false))
    );
    h.keys("Ctrl+C").await;
    assert!(h.last_line().contains("再按一次"));
    h.keys("Ctrl+C").await;
    assert!(
        matches!(h.core.state.ui.overlay, Some(model::Overlay::Exit { .. })),
        "连按两次 Ctrl+C 弹出退出框"
    );
    h.keys("1").await;
    assert!(
        h.shell
            .contains(&app::core::ShellEffect::Quit { detach: true }),
        "1 = 后台运行"
    );
}

#[tokio::test(start_paused = true)]
async fn ctrl_x_exit_dialog_background_stop_or_back() {
    let mut h = Harness::new("default", WIDE).await;
    h.keys("Ctrl+X").await;
    let text = h.text();
    assert!(text.contains("后台运行") && text.contains("直接退出") && text.contains("返回"));
    h.snapshot("exit-dialog");
    h.keys("Esc").await;
    assert!(h.core.state.ui.overlay.is_none(), "Esc = 返回");
    h.keys("Ctrl+X Down Enter").await;
    h.settle().await;
    assert!(
        h.shell
            .contains(&app::core::ShellEffect::Quit { detach: false }),
        "直接退出：停掉实例再退"
    );
    let listed = port::Instances::list(h.instances.as_ref(), true)
        .await
        .unwrap();
    let dev = listed.iter().find(|i| i.name.as_str() == "dev").unwrap();
    assert_eq!(dev.state, port::InstanceState::Stopped);
}

#[tokio::test(start_paused = true)]
async fn quit_arm_expires() {
    let mut h = Harness::new("default", WIDE).await;
    h.keys("Ctrl+C").await;
    h.advance(3_000).await;
    h.keys("Ctrl+C").await;
    assert!(
        !h.shell
            .iter()
            .any(|e| matches!(e, app::core::ShellEffect::Quit { .. }))
    );
}
