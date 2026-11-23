//! `/plugin`（plugins.md §3）：列表、安装后的两份披露、启停、卸载，对着 mock。

mod common;

use common::Harness;
use model::Overlay;
use port::Command;
use tui::Size;

const SIZE: Size = Size::new(160, 45);

async fn open(h: &mut Harness) {
    h.type_str("/plugin").await;
    h.keys("Enter").await;
    h.settle().await;
}

#[tokio::test(start_paused = true)]
async fn install_review_enable_then_disable_and_uninstall() {
    let mut h = Harness::new("default", SIZE).await;
    open(&mut h).await;
    assert!(matches!(
        h.core.state.ui.overlay,
        Some(Overlay::Plugins { .. })
    ));
    let t = h.text();
    assert!(
        t.contains("+ xforge 0.1.0") && t.contains("xforge-0.1.0.zip"),
        "{t}"
    );

    // Enter 装：停用着装下，摆出披露。
    h.keys("Enter").await;
    h.settle().await;
    assert!(matches!(
        h.core.state.ui.overlay,
        Some(Overlay::PluginDisclosure {
            installing: true,
            ..
        })
    ));
    let t = h.text();
    for want in [
        "Agix 的披露",
        "Codix 的披露",
        "xforge.check",
        "流程 xforge-quick",
        "y / Enter 启用",
    ] {
        assert!(t.contains(want), "缺 {want}\n{t}");
    }
    h.snapshot("plugin-disclosure");

    // y：启用，流程进了流程标签，带插件标记。
    h.keys("y").await;
    h.settle().await;
    assert!(h.core.state.ui.overlay.is_none());
    assert!(h.text().contains("已启用插件 xforge"), "{}", h.text());
    h.tab(model::Tab::Flows).await;
    h.settle().await;
    let t = h.text();
    assert!(
        t.contains("xforge-quick") && t.contains("全局 · 插件 xforge"),
        "{t}"
    );

    // 再开 /plugin：已装、已启用（焦点从右栏回到输入框）。
    h.core.state.ui.focus = model::Focus::Composer;
    open(&mut h).await;
    let t = h.text();
    assert!(t.contains("✓ xforge") && t.contains("2 个流程"), "{t}");
    h.snapshot("plugin-list");

    // 空格停用：浮层不关，列表重列，流程撤掉。
    h.keys("Space").await;
    h.settle().await;
    assert!(h.text().contains("○ xforge"), "{}", h.text());
    assert!(
        h.core
            .state
            .domain
            .flows
            .iter()
            .all(|f| !f.name.starts_with("xforge"))
    );

    // d 两次卸载：第一次只提示。
    h.keys("d").await;
    assert!(h.text().contains("再按一次 d 卸载 xforge"));
    assert!(
        !h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::UninstallPlugin { .. }))
    );
    h.keys("d").await;
    h.settle().await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::UninstallPlugin { .. }))
    );
    assert!(
        h.text().contains("+ xforge 0.1.0"),
        "卸掉后回到可装：{}",
        h.text()
    );
}

#[tokio::test(start_paused = true)]
async fn declining_the_disclosure_uninstalls() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/plugin install xforge").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(matches!(
        h.core.state.ui.overlay,
        Some(Overlay::PluginDisclosure { .. })
    ));
    h.keys("Esc").await;
    h.settle().await;
    assert!(h.core.state.ui.overlay.is_none());
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::UninstallPlugin { name } if name == "xforge"))
    );
    assert!(h.text().contains("已卸载插件 xforge"));
}

#[tokio::test(start_paused = true)]
async fn info_without_install_is_an_error() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/plugin info xforge").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.core.state.ui.overlay.is_none());
    assert!(h.text().contains("没有装插件 xforge"), "{}", h.text());
}
