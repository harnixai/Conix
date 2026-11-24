//! 一帧的全部内容。纯数据：可以比较、可以快照，不含任何终端状态。

use crate::layout::LayoutSpec;
use crate::text::display_width;

/// 语义色。颜色只加强、不承载信息（交互 §8）：去掉颜色靠符号也读得懂。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Tone {
    #[default]
    Normal,
    Dim,
    Strong,
    Accent,
    Ok,
    Warn,
    Error,
    Add,
    Del,
    Code,
    /// 有焦点那一块的选中行：橙调的底。
    Selected,
    /// 没焦点那一块的选中行：暗一档的底，好看出右栏选中的就是左边在看的。
    SelectedInactive,
    /// 需要人处理的高亮。
    Attention,
    /// 转录里用户输入那一块的淡底色（与 Claude Code 一致）。
    UserMsg,
    Italic,
    Link,
    /// 语法高亮给出的颜色。
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub tone: Tone,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Line {
    pub spans: Vec<Span>,
    /// 整行的底色语气，如选中行。
    pub tone: Tone,
}

impl Line {
    pub fn plain(text: impl Into<String>) -> Line {
        Line::toned(text, Tone::Normal)
    }

    pub fn toned(text: impl Into<String>, tone: Tone) -> Line {
        Line {
            spans: vec![Span {
                text: text.into(),
                tone,
            }],
            tone: Tone::Normal,
        }
    }

    pub fn push(mut self, text: impl Into<String>, tone: Tone) -> Line {
        self.spans.push(Span {
            text: text.into(),
            tone,
        });
        self
    }

    pub fn with_tone(mut self, tone: Tone) -> Line {
        self.tone = tone;
        self
    }

    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn width(&self) -> usize {
        self.spans.iter().map(|s| display_width(&s.text)).sum()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FrameState {
    pub theme: crate::Theme,
    pub body: Body,
    pub footer: Footer,
    pub overlay: Option<Overlay>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Workbench(Box<Workbench>),
    Instances(InstancesScreen),
    /// 被踢、实例停了之后的"已脱离"界面。
    Detached(DetachedScreen),
    /// 终端太小（交互 §2.5）。
    TooSmall {
        need_cols: u16,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workbench {
    pub layout: LayoutSpec,
    pub header: Header,
    /// 单栏模式下不显示的那一栏为 `None`。
    pub chat: Option<ChatPane>,
    pub side: Option<SidePane>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    pub left: Line,
    pub right: Line,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatPane {
    pub focused: bool,
    /// 框上沿左边的标题：面包屑。
    pub crumbs: Line,
    /// 框上沿右边：这个会话的状态。
    pub badge: Line,
    /// 视口内已排好的行，自上而下，条数不超过转录区高度。
    pub transcript: Vec<Line>,
    pub transcript_focused: bool,
    /// 转录区底部的浮动提示，如 `↓ 有新内容 (End)`。
    pub below: Option<Line>,
    pub status: Option<Line>,
    /// 归属行下面的面板（goal / todo、流程设计、运行进度、作业），已按面板高度排好，
    /// 最后一行是它的下沿。空表示收起。
    pub panel: Vec<Line>,
    pub composer: Composer,
    /// 补全列表：浮在输入框正上方，自下而上贴着它（交互 §4.5）。
    pub picker: Vec<Line>,
    pub picker_selected: Option<usize>,
    /// 提问框：有它时画在输入框的位置上（交互 §4.6）。
    pub ask: Option<Vec<Line>>,
    pub ask_selected: Option<usize>,
    /// 输入框下面那一行：左边 `? 快捷键`，右边权限模式（交互 §4.4）。
    pub hint_left: Line,
    pub hint_right: Line,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Composer {
    pub focused: bool,
    pub lines: Vec<Line>,
    /// 相对输入框内容区左上角的光标位置（列、行）。
    pub cursor: Option<(u16, u16)>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SidePane {
    pub tabs: Vec<TabLabel>,
    pub upper: Section,
    pub lower: Section,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TabLabel {
    pub label: String,
    pub badge: u32,
    pub active: bool,
}

impl TabLabel {
    /// 标签头上显示的文字：` 会话 `，有待处理时后面跟一个 `◐`（不带数字）。
    /// 只有当前标签上主色：提醒色和主色太近，整个标签改提醒色会被看成"选中了"。
    /// layout 用它算宽度做命中测试。
    pub fn display(&self) -> String {
        if self.badge > 0 {
            format!(" {} ◐ ", self.label)
        } else {
            format!(" {} ", self.label)
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Section {
    pub focused: bool,
    /// 框上沿的标题（下框用：上框选中项的名字）；上框的上沿是标签头。
    pub title: Option<String>,
    /// 框上沿右边的摘要，如"2 个在跑"。
    pub title_right: Option<String>,
    /// 视口内的行。
    pub rows: Vec<Line>,
    /// 选中行在 `rows` 里的下标。
    pub selected: Option<usize>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footer {
    pub hints: Vec<Hint>,
    /// 永远在最右边的几项：切块、换标签、切栏、退出、帮助（需求 L-07）。
    pub essential: Vec<Hint>,
    /// 通知、错误或二次确认；有它时它占据提示区，`essential` 照样在。
    pub message: Option<Line>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Hint {
    pub key: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    pub title: String,
    /// 视口内的行。
    pub lines: Vec<Line>,
    /// 浮层宽高（列、行），由 model 按内容与屏幕决定。
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstancesScreen {
    pub title: Line,
    /// 视口内的行（最多 6 行，交互 §2.2）；上下还有更多时由 `more_above` / `more_below` 标出。
    pub rows: Vec<Line>,
    pub more_above: bool,
    pub more_below: bool,
    pub selected: Option<usize>,
    pub message: Option<Line>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DetachedScreen {
    pub lines: Vec<Line>,
}
