//! `/flow:export`、`/flow:import`（design/flows.md §7.12）：输入框里的写法走到端口命令，结果在界面上摆出来，对着 mock。

mod common;

use common::Harness;
use model::Overlay;
use port::Command;
use tui::Size;

const SIZE: Size = Size::new(160, 45);

fn package_calls(h: &Harness) -> Vec<Command> {
    h.calls("dev")
        .into_iter()
        .filter(|c| matches!(c, Command::ExportFlow { .. } | Command::ImportFlow { .. }))
        .collect()
}

#[tokio::test(start_paused = true)]
async fn export_takes_a_bundle_and_an_optional_file() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/flow:export feature-to-ship out/pack.zip")
        .await;
    h.keys("Enter").await;
    h.settle().await;
    assert_eq!(
        package_calls(&h),
        [Command::ExportFlow {
            bundle: "feature-to-ship".into(),
            file: Some("out/pack.zip".into())
        }]
    );
    let t = h.text();
    assert!(
        t.contains("已导出 feature-to-ship") && t.contains("out/pack.zip"),
        "{t}"
    );

    h.type_str("/flow:export feature-to-ship").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(matches!(
        package_calls(&h).last(),
        Some(Command::ExportFlow { file: None, .. })
    ));

    h.type_str("/flow:export nope").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.text().contains("没有流程包 nope"), "{}", h.text());
}

#[tokio::test(start_paused = true)]
async fn a_usage_mistake_never_reaches_the_instance() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/flow:export").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.text().contains("用法：/flow:export"), "{}", h.text());
    h.type_str("/flow:import --force").await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.text().contains("用法：/flow:import"), "{}", h.text());
    assert!(package_calls(&h).is_empty());
}

#[tokio::test(start_paused = true)]
async fn import_reports_what_it_installed_and_the_flow_tab_gains_it() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/flow:import pack/review-1.2.0.zip --force")
        .await;
    h.keys("Enter").await;
    h.settle().await;
    assert_eq!(
        package_calls(&h),
        [Command::ImportFlow {
            file: "pack/review-1.2.0.zip".into(),
            force: true
        }]
    );
    assert!(matches!(
        h.core.state.ui.overlay,
        Some(Overlay::Text { .. })
    ));
    let t = h.text();
    assert!(
        t.contains("已导入流程包 review") && t.contains("review/main") && t.contains("已装到"),
        "{t}"
    );
    h.keys("Esc").await;
    h.tab(model::Tab::Flows).await;
    let t = h.text();
    assert!(t.contains("review/main"), "{t}");
}

#[tokio::test(start_paused = true)]
async fn the_two_commands_are_offered_by_the_slash_picker() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/flow:").await;
    let names: Vec<String> = h
        .core
        .state
        .picker_candidates()
        .into_iter()
        .map(|c| c.name)
        .collect();
    assert_eq!(names[..2], ["flow:export", "flow:import"]);
}
