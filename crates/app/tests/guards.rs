//! 守卫（testing.md §5）：鼠标不另走逻辑；FrameState 的每个可选字段至少被一个场景画出来过。

mod common;

use std::collections::BTreeSet;

use app::core::Start;
use common::Harness;
use tui::{Body, FrameState, Size};

#[test]
fn mouse_only_hit_tests_and_drags() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../model/src");
    let mut offenders = vec![];
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains("MouseKind::")
            && !["mouse.rs", "update.rs", "lib.rs"].contains(&name.as_str())
        {
            offenders.push(name);
        }
    }
    assert!(
        offenders.is_empty(),
        "鼠标只做命中测试与拖动，翻译成键盘动作：{offenders:?}"
    );
}

fn mark(seen: &mut BTreeSet<&'static str>, f: &FrameState) {
    if f.overlay.is_some() {
        seen.insert("overlay");
    }
    if f.footer.message.is_some() {
        seen.insert("footer.message");
    }
    match &f.body {
        Body::TooSmall { .. } => {
            seen.insert("body.too_small");
        }
        Body::Detached(_) => {
            seen.insert("body.detached");
        }
        Body::Instances(i) => {
            seen.insert("body.instances");
            if i.message.is_some() {
                seen.insert("instances.message");
            }
        }
        Body::Workbench(w) => {
            if let Some(c) = &w.chat {
                for (k, on) in [
                    ("chat.below", c.below.is_some()),
                    ("chat.status", c.status.is_some()),
                    ("chat.panel", !c.panel.is_empty()),
                    ("chat.picker", !c.picker.is_empty()),
                    ("chat.ask", c.ask.is_some()),
                    ("chat.badge", !c.badge.spans.is_empty()),
                    ("chat.hint_right", !c.hint_right.spans.is_empty()),
                    ("chat.transcript_focused", c.transcript_focused),
                ] {
                    if on {
                        seen.insert(k);
                    }
                }
            }
            if let Some(s) = &w.side {
                if s.lower.title_right.is_some() {
                    seen.insert("side.lower.title_right");
                }
                if s.tabs.iter().any(|t| t.badge > 0) {
                    seen.insert("side.tabs.badge");
                }
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn every_optional_frame_field_is_produced_somewhere() {
    let size = Size::new(160, 45);
    let mut seen = BTreeSet::new();

    let mut h = Harness::start("default", size, Start::Picker).await;
    mark(&mut seen, &h.core.frame());
    h.keys("Down Enter").await;
    mark(&mut seen, &h.core.frame());

    let mut h = Harness::new("m04-prompts", size).await;
    h.select_session("重构认证模块").await;
    mark(&mut seen, &h.core.frame());
    h.open_session("昨天的调研").await;
    h.type_str("跑测试").await;
    h.keys("Enter").await;
    h.advance(600).await;
    mark(&mut seen, &h.core.frame());
    h.keys("1").await;
    h.keys("Ctrl+T BackTab").await;
    h.type_str("/he").await;
    mark(&mut seen, &h.core.frame());
    h.keys("Ctrl+U Ctrl+O PageUp").await;
    mark(&mut seen, &h.core.frame());
    h.keys("Ctrl+X").await;
    mark(&mut seen, &h.core.frame());

    let mut h = Harness::new("m01-chat-basic", size).await;
    h.open_session("昨天的调研").await;
    h.type_str("你好").await;
    h.keys("Enter").await;
    h.advance(150).await;
    mark(&mut seen, &h.core.frame());
    h.advance(3_000).await;
    h.keys("BackTab").await;
    mark(&mut seen, &h.core.frame());

    let mut h = Harness::new("m03-tools", Size::new(160, 24)).await;
    h.open_session("昨天的调研").await;
    h.type_str("改一下").await;
    h.keys("Enter").await;
    h.advance(200).await;
    h.keys("Ctrl+O PageUp").await;
    h.advance(3_000).await;
    mark(&mut seen, &h.core.frame());

    let mut h = Harness::new("m21-kicked", size).await;
    h.advance(5_100).await;
    mark(&mut seen, &h.core.frame());

    let mut h = Harness::new("default", Size::new(50, 20)).await;
    mark(&mut seen, &h.core.frame());

    let mut h = Harness::new("m07-steer", size).await;
    h.open_session("昨天的调研").await;
    h.type_str("开始").await;
    h.keys("Enter").await;
    h.advance(300).await;
    h.keys("Ctrl+O Up Up Up").await;
    h.advance(400).await;
    mark(&mut seen, &h.core.frame());

    let expected = [
        "overlay",
        "footer.message",
        "body.too_small",
        "body.detached",
        "body.instances",
        "chat.status",
        "chat.panel",
        "chat.picker",
        "chat.ask",
        "chat.badge",
        "chat.hint_right",
        "chat.transcript_focused",
        "chat.below",
        "side.lower.title_right",
        "side.tabs.badge",
    ];
    let missing: Vec<_> = expected.iter().filter(|k| !seen.contains(*k)).collect();
    assert!(
        missing.is_empty(),
        "这些字段没有任何场景画出来：{missing:?}"
    );
}
