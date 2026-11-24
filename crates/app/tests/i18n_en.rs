//! 英文界面（需求 L-11）。语言是进程级的全局状态，所以单独成一个测试文件（单独的进程），
//! 里面的测试都先切到英文、互不冲突。

mod common;

use common::Harness;
use tui::Size;

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
    })
}

#[test]
fn cli_help_resources_are_english() {
    i18n::init("en", None);
    for key in [
        "app.cli.about",
        "app.cli.mock",
        "app.cli.speed",
        "app.cli.new",
        "app.cli.list",
        "app.cli.attach",
        "app.cli.attach_force",
        "app.cli.start",
        "app.cli.stop",
    ] {
        assert!(!has_cjk(&i18n::tr(key)), "{key}");
    }
}

#[tokio::test(start_paused = true)]
async fn tui_footer_and_header_are_english() {
    i18n::init("en", None);
    let mut h = Harness::new("demo", Size::new(160, 45)).await;
    let text = h.text();
    let footer = text.lines().rev().find(|l| !l.trim().is_empty()).unwrap();
    assert!(footer.contains("Pane"), "{footer}");
    assert!(footer.contains("Exit"), "{footer}");
    assert!(footer.contains("Help"), "{footer}");
    let header = text.lines().next().unwrap();
    assert!(!has_cjk(header), "{header}");
    assert!(!has_cjk(footer), "{footer}");
}
