//! 内置键位表（docs/design/interaction.md §5–§7）。

use crate::{Action, Binding, Context, Key};

fn b(context: Context, keys: &str, action: Action) -> Binding {
    Binding {
        context,
        keys: keys
            .split(' ')
            .map(|k| Key::parse(k).unwrap_or_else(|| panic!("builtin key binding is wrong: {k}")))
            .collect(),
        action,
    }
}

pub(crate) fn bindings() -> Vec<Binding> {
    use Action as A;
    use Context as C;
    let mut v = vec![
        // 全局（交互 §7）。命令键只用 Ctrl+字母：F 键在笔记本上要按 Fn，Alt 在 macOS 要终端设成
        // Meta 才收得到，Ctrl+字母在哪个终端、ssh、tmux 里都一样。F1 是例外，人人都知道它是帮助。
        // Ctrl+H 写在 F1 前面：底栏和帮助取第一个绑定。
        b(C::Global, "Ctrl+H", A::Help),
        b(C::Global, "F1", A::Help),
        b(C::Global, "Ctrl+G", A::FocusCycle),
        b(C::Global, "Ctrl+N", A::NextTab),
        b(C::Global, "Ctrl+T", A::TogglePanel),
        b(C::Global, "Ctrl+O", A::ReadTranscript),
        b(C::Global, "Ctrl+R", A::NextAttention),
        b(C::Global, "Ctrl+B", A::PaneToggle),
        // 滚动：哪一块有焦点滚哪一块（交互 §7）。PageUp / PageDown 一页，Shift+↑↓ 一行。
        b(C::Global, "Shift+Up", A::LineUp),
        b(C::Global, "Shift+Down", A::LineDown),
        b(C::Global, "Ctrl+C", A::Interrupt),
        b(C::Global, "Ctrl+D", A::Detach),
        b(C::Global, "Ctrl+X", A::Exit),
        b(C::Global, "Ctrl+L", A::Redraw),
        b(C::Global, "Esc", A::Back),
        // 输入框
        b(C::Composer, "Enter", A::Submit),
        b(C::Composer, "Shift+Enter", A::Newline),
        // Alt+Enter、Alt+←→、Alt+B/F、Ctrl+←→ 只为兼容留着，不进底栏和文档：它们是各家终端按
        // Option+← 之类时自己发出来的编辑键（macOS 终端发的是 Esc b / Esc f），不是 Conix 的快捷键。
        b(C::Composer, "Alt+Enter", A::Newline),
        b(C::Composer, "Ctrl+J", A::Newline),
        b(C::Composer, "Tab", A::Complete),
        b(C::Composer, "BackTab", A::CycleMode),
        b(C::Composer, "Left", A::CursorLeft),
        b(C::Composer, "Right", A::CursorRight),
        b(C::Composer, "Alt+Left", A::WordLeft),
        b(C::Composer, "Alt+Right", A::WordRight),
        b(C::Composer, "Alt+B", A::WordLeft),
        b(C::Composer, "Alt+F", A::WordRight),
        b(C::Composer, "Home", A::LineStart),
        b(C::Composer, "Ctrl+A", A::LineStart),
        b(C::Composer, "End", A::LineEnd),
        b(C::Composer, "Ctrl+E", A::LineEnd),
        b(C::Composer, "Ctrl+U", A::ClearInput),
        b(C::Composer, "Ctrl+W", A::DeleteWord),
        b(C::Composer, "Ctrl+K", A::KillToEnd),
        b(C::Composer, "Backspace", A::Backspace),
        // 有的终端退格键发的就是 Ctrl+H：输入框有字时它得是退格。
        b(C::Composer, "Ctrl+H", A::HelpOrErase),
        b(C::Composer, "Delete", A::DeleteForward),
        b(C::Composer, "Ctrl+Left", A::WordLeft),
        b(C::Composer, "Ctrl+Right", A::WordRight),
        b(C::Composer, "Up", A::Up),
        b(C::Composer, "Down", A::Down),
        b(C::Composer, "PageUp", A::PageUp),
        b(C::Composer, "PageDown", A::PageDown),
        // 转录
        b(C::Transcript, "Up", A::Up),
        b(C::Transcript, "k", A::Up),
        b(C::Transcript, "Down", A::Down),
        b(C::Transcript, "j", A::Down),
        b(C::Transcript, "PageUp", A::PageUp),
        b(C::Transcript, "PageDown", A::PageDown),
        b(C::Transcript, "Home", A::Top),
        b(C::Transcript, "End", A::Bottom),
        b(C::Transcript, "[", A::BlockPrev),
        b(C::Transcript, "]", A::BlockNext),
        b(C::Transcript, "Enter", A::Open),
        b(C::Transcript, "y", A::Copy),
        // 详细时整段都展开，单个块的 Enter 就没用了：o 收回精简再一块一块看。
        b(C::Transcript, "o", A::ToggleDetail),
        // 补全列表
        b(C::Picker, "Up", A::Up),
        b(C::Picker, "Down", A::Down),
        b(C::Picker, "Tab", A::Complete),
        b(C::Picker, "Enter", A::Submit),
        b(C::Picker, "Backspace", A::Backspace),
        // 对话框（提问框、二次确认）
        b(C::Dialog, "Up", A::Up),
        b(C::Dialog, "Down", A::Down),
        b(C::Dialog, "Enter", A::Confirm),
        b(C::Dialog, "y", A::Yes),
        b(C::Dialog, "n", A::No),
        // 实例选择
        b(C::Instances, "Up", A::Up),
        b(C::Instances, "k", A::Up),
        b(C::Instances, "Down", A::Down),
        b(C::Instances, "j", A::Down),
        b(C::Instances, "Enter", A::Open),
        b(C::Instances, "n", A::New),
        b(C::Instances, "s", A::Stop),
        b(C::Instances, "f", A::Force),
        b(C::Instances, "q", A::Quit),
        // 浮层
        b(C::Overlay, "Up", A::Up),
        b(C::Overlay, "Down", A::Down),
        b(C::Overlay, "PageUp", A::PageUp),
        b(C::Overlay, "PageDown", A::PageDown),
        b(C::Overlay, "q", A::Back),
        b(C::Overlay, "Enter", A::Confirm),
        // `/plugin` 浮层：启停、卸载、确认披露。
        b(C::Overlay, "Space", A::ToggleEnabled),
        b(C::Overlay, "d", A::Delete),
        b(C::Overlay, "y", A::Yes),
        b(C::Overlay, "n", A::No),
        b(C::Overlay, "1", A::Choose1),
        b(C::Overlay, "2", A::Choose2),
        b(C::Overlay, "3", A::Choose3),
    ];
    for (i, a) in [
        A::Choose1,
        A::Choose2,
        A::Choose3,
        A::Choose4,
        A::Choose5,
        A::Choose6,
        A::Choose7,
        A::Choose8,
        A::Choose9,
    ]
    .into_iter()
    .enumerate()
    {
        v.push(b(C::Dialog, &(i + 1).to_string(), a));
    }
    // 右栏三个标签的公共键位（交互 §5.1）
    for ctx in [C::Sessions, C::Flows, C::Jobs] {
        v.extend([
            b(ctx, "Up", A::Up),
            b(ctx, "k", A::Up),
            b(ctx, "Down", A::Down),
            b(ctx, "j", A::Down),
            b(ctx, "Left", A::Collapse),
            b(ctx, "h", A::Collapse),
            b(ctx, "Right", A::Expand),
            b(ctx, "l", A::Expand),
            b(ctx, "Enter", A::Open),
            b(ctx, "Tab", A::SwitchSection),
            b(ctx, "/", A::Filter),
            b(ctx, "<", A::RatioDec),
            b(ctx, ">", A::RatioInc),
            b(ctx, "?", A::Help),
            b(ctx, "PageUp", A::PageUp),
            b(ctx, "PageDown", A::PageDown),
            b(ctx, "n", A::New),
        ]);
    }
    v.extend([
        b(C::Sessions, "r", A::Rename),
        b(C::Sessions, "x", A::Close),
        b(C::Sessions, "p", A::PauseResume),
        b(C::Sessions, "m", A::Message),
        b(C::Sessions, "s", A::Stop),
        b(C::Sessions, "o", A::Reopen),
        b(C::Sessions, "d", A::Delete),
        b(C::Flows, "v", A::View),
        b(C::Flows, "p", A::PauseResume),
        b(C::Flows, "x", A::Close),
        b(C::Flows, "c", A::Confirm),
        b(C::Flows, "a", A::Release),
        b(C::Flows, "f", A::Force),
        b(C::Flows, "r", A::Rework),
        b(C::Flows, "t", A::Retry),
        b(C::Flows, "e", A::Edit),
        b(C::Flows, "d", A::Delete),
        b(C::Flows, "R", A::RunNow),
        b(C::Jobs, "v", A::View),
        b(C::Jobs, "e", A::Edit),
        b(C::Jobs, "Space", A::ToggleEnabled),
        b(C::Jobs, "R", A::RunNow),
        b(C::Jobs, "d", A::Delete),
        b(C::Jobs, "r", A::Retry),
        b(C::Jobs, "t", A::Retry),
        b(C::Jobs, "s", A::Stop),
    ]);
    v
}

/// 底栏左侧按上下文提示哪些动作（交互 §2.3 示意图）。切块、换标签、切栏、退出、帮助另由
/// `Keymap::essential` 给出。
pub(crate) fn footer(ctx: Context) -> &'static [Action] {
    use Action as A;
    match ctx {
        Context::Global | Context::Composer => {
            &[A::NextAttention, A::ReadTranscript, A::TogglePanel]
        }
        Context::Transcript => &[A::BlockPrev, A::Open, A::ToggleDetail, A::ReadTranscript],
        Context::Sessions => &[A::Open, A::New, A::Rename, A::Stop],
        Context::Flows => &[A::New, A::Edit, A::RunNow, A::Confirm, A::View],
        Context::Jobs => &[A::New, A::Edit, A::ToggleEnabled, A::RunNow, A::View],
        Context::Dialog => &[A::Confirm, A::Yes, A::No, A::Back],
        Context::Picker => &[A::Complete, A::Submit, A::Back],
        Context::Instances => &[A::Open, A::New, A::Force, A::Stop, A::Quit],
        Context::Overlay => &[A::Back],
    }
}
