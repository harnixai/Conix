//! 流程包的管理只在命令行（design/flows.md §7.12）：界面里没有 `/flow:export`、`/flow:import`；地址里的包
//! 项目里没装、全局库里有时，提示 `conix flow install`。对着 mock。

mod common;

use common::Harness;
use port::Command;
use tui::Size;

const SIZE: Size = Size::new(160, 45);

#[tokio::test(start_paused = true)]
async fn a_bundle_in_the_global_library_but_not_installed_says_how_to_install_it() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/flow:review 看一眼").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::ListLibrary)),
        "问实例全局库里有没有"
    );
    let t = h.text();
    assert!(t.contains("conix flow install review"), "{t}");

    h.type_str("/flow:nope").await;
    h.keys("Enter").await;
    h.settle().await;
    let t = h.text();
    assert!(t.contains("没有流程包「nope」"), "{t}");
}

#[tokio::test(start_paused = true)]
async fn export_and_import_are_not_commands_of_the_workbench() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/flow:").await;
    let names: Vec<String> = h
        .core
        .state
        .picker_candidates()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert!(
        !names
            .iter()
            .any(|n| n == "flow:export" || n == "flow:import"),
        "{names:?}"
    );
    h.core.state.ui.composer.clear();
    h.type_str("/flow:export feature-to-ship").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.text().contains("没有流程包「export」"), "{}", h.text());
}
