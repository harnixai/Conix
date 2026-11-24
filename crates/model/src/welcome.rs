//! 开场欢迎块（需求 C-18，交互 §4.2）：Conix logo、版本、实例、项目、场景、模型、常用键。

use port::SessionSummary;
use tui::{Line, Tone, display_width, truncate_to_width};

const LOGO: [&str; 2] = ["▄▀▀ ▄▀▄ █▀▄ █ ▀▄▀", "▀▄▄ ▀▄▀ █▄▀ █ ▄▀▄"];
/// 欢迎框里只写 `x.y.z`：Cargo 的版本是 `x.y.z-YYYYMMDDNN`，日期序号是发布的标识，不是给人看的版本。
pub fn short_version() -> &'static str {
    let v = env!("CARGO_PKG_VERSION");
    v.split('-').next().unwrap_or(v)
}
const BOX_MAX: usize = 60;

/// 还没打开会话时：只有实例信息。
pub(crate) fn idle(instance: Option<&port::InstanceInfo>, width: usize) -> Vec<Line> {
    let info = instance.map(|i| {
        vec![i18n::t!(
            "welcome.instance",
            name = i.name,
            host = i.host,
            project = i
                .project
                .clone()
                .unwrap_or_else(|| i18n::t!("welcome.none"))
        )]
    });
    boxed(width, info.unwrap_or_default())
}

/// 会话转录最上面的那一块。
pub(crate) fn block(s: &SessionSummary, width: usize) -> Vec<Line> {
    let p = &s.provenance;
    boxed(
        width,
        vec![
            i18n::t!(
                "welcome.instance",
                name = p.instance,
                host = p.host,
                project = p
                    .project
                    .clone()
                    .unwrap_or_else(|| i18n::t!("welcome.none"))
            ),
            i18n::t!("welcome.scene_model", scene = p.scene, model = p.model),
        ],
    )
}

fn boxed(width: usize, info: Vec<String>) -> Vec<Line> {
    let w = width.min(BOX_MAX);
    if w < 24 {
        return vec![Line::toned(
            format!("Conix {}", short_version()),
            Tone::Accent,
        )];
    }
    let inner = w - 4;
    let mut body: Vec<Line> = vec![
        Line::toned(LOGO[0], Tone::Accent),
        Line::toned(LOGO[1], Tone::Accent)
            .push(format!("   Conix {}", short_version()), Tone::Strong),
        Line::default(),
    ];
    body.extend(info.into_iter().map(|t| Line::toned(t, Tone::Dim)));
    body.extend([
        Line::default(),
        Line::toned(i18n::t!("welcome.keys_help"), Tone::Dim),
        Line::toned(i18n::t!("welcome.keys_tabs"), Tone::Dim),
    ]);
    let mut out = vec![Line::toned(
        format!("╭{}╮", "─".repeat(w - 2)),
        Tone::Accent,
    )];
    for l in body {
        let text_w = l.width();
        let mut row = Line::toned("│ ", Tone::Accent);
        if text_w > inner {
            row = row.push(truncate_to_width(&l.text(), inner), Tone::Dim);
            let pad = inner.saturating_sub(display_width(&truncate_to_width(&l.text(), inner)));
            row = row.push(" ".repeat(pad), Tone::Normal);
        } else {
            for sp in l.spans {
                row = row.push(sp.text, sp.tone);
            }
            row = row.push(" ".repeat(inner - text_w), Tone::Normal);
        }
        out.push(row.push(" │", Tone::Accent));
    }
    out.push(Line::toned(
        format!("╰{}╯", "─".repeat(w - 2)),
        Tone::Accent,
    ));
    out
}
