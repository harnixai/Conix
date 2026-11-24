//! 界面可调的部分（`~/.conix/conix.yaml` 的 `ui`、`instance`，由 app 读出来交给 model）。

use crate::state::DEFAULT_IDLE_TIMEOUT_MS;

#[derive(Debug, Clone)]
pub struct Settings {
    pub theme: tui::Theme,
    /// syntect 的主题名，用于代码块高亮。
    pub syntax_theme: String,
    /// 精简模式下工具结果显示几行（与 Claude Code 一致缺省 3 行）。
    pub compact_tool_lines: usize,
    /// 宽屏时左栏占的百分比。
    pub ratio_pct: u16,
    /// 0 表示不超时。
    pub idle_timeout_ms: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: tui::Theme::default(),
            syntax_theme: crate::highlight::DEFAULT_THEME.into(),
            compact_tool_lines: 3,
            ratio_pct: 60,
            idle_timeout_ms: DEFAULT_IDLE_TIMEOUT_MS,
        }
    }
}
