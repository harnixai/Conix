//! 配色（需求：主题可配置，`~/.conix/conix.yaml` 的 `ui.theme`）。渲染端的每一种颜色都从
//! 这里取，不在别处写死。预设 `claude`（橙）与 `aqua`（水蓝），其余颜色两者相同。

use ratatui::style::Color;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Theme {
    /// 主色：有焦点的标题条底色、强调字、选中标记。
    pub accent: Color,
    /// 主色底上的字。
    pub accent_fg: Color,
    /// 顶栏、底栏、没焦点的标题条。
    pub bar_bg: Color,
    pub bar_fg: Color,
    pub border: Color,
    /// 有焦点那一块的框线：亮灰，不抢眼。
    pub border_focus: Color,
    pub dim: Color,
    pub selected_bg: Color,
    /// 没焦点那一块的选中行（交互 §2.3）。
    pub selected_bg_inactive: Color,
    pub selected_fg: Color,
    pub ok: Color,
    pub warn: Color,
    pub error: Color,
    pub add: Color,
    pub del: Color,
    pub code: Color,
    pub link: Color,
    pub user_bg: Color,
}

impl Theme {
    pub fn claude() -> Theme {
        Theme {
            accent: Color::Rgb(0xD9, 0x77, 0x57),
            accent_fg: Color::Rgb(0x1A, 0x1A, 0x1A),
            bar_bg: Color::Indexed(235),
            bar_fg: Color::Indexed(250),
            border: Color::Indexed(238),
            border_focus: Color::Indexed(246),
            dim: Color::Indexed(243),
            // 暗调的橙：行里提醒色（◐）、主色（◎、强调）、绿红状态色落在上面都还看得清。
            selected_bg: Color::Rgb(0x5C, 0x34, 0x26),
            selected_bg_inactive: Color::Rgb(0x3A, 0x27, 0x21),
            selected_fg: Color::Indexed(255),
            ok: Color::Indexed(114),
            warn: Color::Indexed(179),
            error: Color::Indexed(167),
            add: Color::Indexed(114),
            del: Color::Indexed(167),
            code: Color::Indexed(110),
            link: Color::Indexed(75),
            user_bg: Color::Indexed(236),
        }
    }

    pub fn aqua() -> Theme {
        Theme {
            accent: Color::Rgb(0x4F, 0xC3, 0xF7),
            selected_bg: Color::Rgb(0x1E, 0x46, 0x5A),
            selected_bg_inactive: Color::Rgb(0x1B, 0x30, 0x3B),
            ..Theme::claude()
        }
    }

    pub fn preset(name: &str) -> Option<Theme> {
        match name {
            "claude" => Some(Theme::claude()),
            "aqua" => Some(Theme::aqua()),
            _ => None,
        }
    }

    /// 按名字改一种颜色（配置里的覆盖项）；名字或颜色不认识时返回错误说明。
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), String> {
        let c: Color = value
            .parse()
            .map_err(|_| i18n::t!("tui.theme.bad_color", value = format!("{value:?}")))?;
        let slot = match key {
            "accent" => &mut self.accent,
            "accent_fg" => &mut self.accent_fg,
            "bar_bg" => &mut self.bar_bg,
            "bar_fg" => &mut self.bar_fg,
            "border" => &mut self.border,
            "border_focus" => &mut self.border_focus,
            "dim" => &mut self.dim,
            "selected_bg" => &mut self.selected_bg,
            "selected_bg_inactive" => &mut self.selected_bg_inactive,
            "selected_fg" => &mut self.selected_fg,
            "ok" => &mut self.ok,
            "warn" => &mut self.warn,
            "error" => &mut self.error,
            "add" => &mut self.add,
            "del" => &mut self.del,
            "code" => &mut self.code,
            "link" => &mut self.link,
            "user_bg" => &mut self.user_bg,
            _ => return Err(i18n::t!("tui.theme.no_slot", key = key)),
        };
        *slot = c;
        Ok(())
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::claude()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overrides_parse_hex_names_and_indices() {
        let mut t = Theme::aqua();
        t.set("accent", "#112233").unwrap();
        t.set("border", "240").unwrap();
        t.set("ok", "green").unwrap();
        assert_eq!(t.accent, Color::Rgb(0x11, 0x22, 0x33));
        assert_eq!(t.border, Color::Indexed(240));
        assert!(t.set("nope", "red").is_err());
        assert!(t.set("accent", "not-a-color").is_err());
    }
}
