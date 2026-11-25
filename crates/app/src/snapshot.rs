//! 快照：把一帧画进 TestBackend，导出成纯文本（只记字符，不记样式；testing.md §2）。

use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use tui::{FrameState, Size};

pub fn render_text(frame: &FrameState, size: Size) -> String {
    let mut t = Terminal::new(TestBackend::new(size.cols, size.rows)).unwrap();
    t.draw(|f| tui::render(f, frame)).unwrap();
    let buf = t.backend().buffer();
    let mut out = String::new();
    for y in 0..size.rows {
        let mut line = String::new();
        let mut skip = 0;
        for x in 0..size.cols {
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let sym = buf[(x, y)].symbol();
            line.push_str(sym);
            skip = tui::display_width(sym).saturating_sub(1);
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out
}

/// 与 `tests/snapshots/<name>.txt` 比较；`CONIX_UPDATE_SNAPSHOTS=1` 时改写它。
pub fn assert_snapshot(name: &str, actual: &str) {
    // 欢迎框里的版本每次发布都变：换成定长的占位，快照不用跟着重写。占位不能跟版本一样宽：版本从 0.1.9 到
    // 0.1.10 就变宽了，所以占位后面补空白占住多出来的宽度，框的右边线不动（版本至少 x.y.z 五个字符）。
    let v = model::short_version();
    let placeholder = "x.x.x";
    let pad = " ".repeat(v.len().saturating_sub(placeholder.len()));
    let actual = actual.replace(&format!("Conix {v}"), &format!("Conix {placeholder}{pad}"));
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"));
    if std::env::var_os("CONIX_UPDATE_SNAPSHOTS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| {
        panic!(
            "no snapshot at {}; check the screen, then generate it with CONIX_UPDATE_SNAPSHOTS=1\n{actual}",
            path.display()
        )
    });
    if expected != actual {
        panic!("snapshot {name} differs\n--- expected\n{expected}\n--- actual\n{actual}");
    }
}
