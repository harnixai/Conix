//! 场景 m21：实例选择、占用与强制接入、被踢、不做空闲脱离、心跳、停止（需求 I-01 至 I-11）。

mod common;

use app::core::{ShellEffect, Start};
use common::Harness;
use model::{DetachReason, Screen};
use port::Command;
use tui::Size;

const SIZE: Size = Size::new(140, 36);

async fn picker() -> Harness {
    Harness::start("default", SIZE, Start::Picker).await
}

fn on_picker(h: &Harness) -> bool {
    matches!(h.core.state.ui.screen, Screen::Instances(_))
}

#[tokio::test(start_paused = true)]
async fn m21_picker_lists_every_state() {
    let mut h = picker().await;
    let text = h.text();
    for want in [
        "选择实例",
        "dev",
        "ops",
        "research",
        "old",
        "被 me@pts/3 (ssh 10.0.0.7)",
        "已停止",
        "残留",
        "＋ 新建实例",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    h.snapshot("m21-picker");
}

#[tokio::test(start_paused = true)]
async fn m21_enter_attaches_a_free_instance() {
    let mut h = picker().await;
    h.keys("Enter").await;
    assert_eq!(h.core.state.ui.screen, Screen::Workbench);
    assert_eq!(h.core.state.ui.attached.as_deref(), Some("dev"));
    assert!(h.core.state.domain.instance.is_some());
}

#[tokio::test(start_paused = true)]
async fn m21_occupied_needs_confirmed_force() {
    let mut h = picker().await;
    h.keys("Down").await;
    h.keys("Enter").await;
    assert!(h.last_line().contains("强制接入 ops"), "{}", h.last_line());
    h.keys("x").await;
    assert!(on_picker(&h), "其他键放弃");
    h.keys("f y").await;
    assert_eq!(h.core.state.ui.attached.as_deref(), Some("ops"));
    assert_eq!(h.core.state.ui.screen, Screen::Workbench);
}

#[tokio::test(start_paused = true)]
async fn m21_attach_occupied_from_command_line_falls_back_to_picker() {
    let mut h = Harness::start(
        "default",
        SIZE,
        Start::Op(model::InstOp::Attach {
            name: "ops".into(),
            force: false,
        }),
    )
    .await;
    assert!(on_picker(&h));
    assert!(h.text().contains("已被 me@pts/3"), "提示占用者");
}

#[tokio::test(start_paused = true)]
async fn m21_start_stopped_and_create_new() {
    // 运行中的排在前面，其余按名字：dev、ops、old（残留）、research（已停止）。
    let mut h = picker().await;
    h.keys("Down Down Enter").await;
    assert_eq!(
        h.core.state.ui.attached.as_deref(),
        Some("old"),
        "残留的实例重新启动"
    );
    let mut h = picker().await;
    h.keys("Down Down Down Enter").await;
    assert_eq!(h.core.state.ui.attached.as_deref(), Some("research"));
    let mut h = picker().await;
    h.keys("n").await;
    assert_eq!(h.core.state.ui.attached.as_deref(), Some("new-1"));
}

#[tokio::test(start_paused = true)]
async fn m21_stop_needs_confirmation() {
    let mut h = picker().await;
    h.keys("s").await;
    assert!(h.last_line().contains("停止实例 dev"));
    h.keys("y").await;
    let Screen::Instances(p) = &h.core.state.ui.screen else {
        panic!()
    };
    let dev = p.list.iter().find(|i| i.name.as_str() == "dev").unwrap();
    assert_eq!(dev.state, port::InstanceState::Stopped);
}

#[tokio::test(start_paused = true)]
async fn m21_kicked_goes_to_detached_then_back_to_picker() {
    let mut h = Harness::new("m21-kicked", SIZE).await;
    h.advance(5_100).await;
    assert!(matches!(
        h.core.state.ui.screen,
        Screen::Detached(DetachReason::Kicked { .. })
    ));
    assert!(!h.core.attached(), "被踢之后不再收原后端的事件");
    let text = h.text();
    assert!(text.contains("alice@pts/5 (ssh 10.0.0.9)"), "{text}");
    h.keys("Enter").await;
    assert!(on_picker(&h));
}

/// 没有空闲脱离：放着不动几个小时也还接着，心跳照发（被踢、实例停了才脱离；占着不放的由别人强制接入）。
#[tokio::test(start_paused = true)]
async fn m21_a_quiet_tui_stays_attached_and_keeps_beating() {
    let mut h = Harness::new("default", SIZE).await;
    h.advance(3 * 3_600_000).await;
    assert_eq!(h.core.state.ui.screen, Screen::Workbench);
    let calls = h.instances.backend("dev").unwrap().calls();
    let calls = calls.lock().unwrap();
    assert!(!calls.contains(&Command::Detach), "不会自己脱离");
    assert!(
        calls
            .iter()
            .filter(|c| matches!(c, Command::Heartbeat { .. }))
            .count()
            > 100
    );
}

#[tokio::test(start_paused = true)]
async fn m21_heartbeat_every_ten_seconds() {
    let mut h = Harness::new("default", SIZE).await;
    h.advance(35_000).await;
    let calls = h.instances.backend("dev").unwrap().calls();
    let beats = calls
        .lock()
        .unwrap()
        .iter()
        .filter(|c| matches!(c, Command::Heartbeat { .. }))
        .count();
    assert_eq!(beats, 3);
}

#[tokio::test(start_paused = true)]
async fn m21_quit_is_detach() {
    let mut h = picker().await;
    h.keys("q").await;
    assert!(h.shell.contains(&ShellEffect::Quit { detach: true }));
}
