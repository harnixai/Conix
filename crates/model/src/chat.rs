//! 一个被 Watch 的会话在界面里的样子（需求 §4.4，交互 §4）：转录块、这一轮的进度、
//! 提问队列、todo、goal、任务。块按宽度缓存排好的行，流式增量只让所在的块重排。

use std::collections::HashSet;

use port::{
    Event, FileChange, Goal, HistoryItem, MessageId, Prompt, Resolution, SessionSummary,
    SlashCommand, StopReason, Task, Todo, TodoStatus, ToolId, Watched,
};
use tui::{Line, Tone};

use crate::text::wrap_text;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    Thinking,
    Answering,
    Tool(String),
    Compacting,
}

#[derive(Debug, Clone)]
pub enum BlockKind {
    Welcome,
    User {
        text: String,
        /// 运行中插话、还没送达时是它的消息 id（交互 §4.4 `⧗ 待送达`）。
        pending: Option<MessageId>,
        steer: bool,
    },
    Assistant {
        text: String,
    },
    Thinking {
        text: String,
        started_ms: u64,
        secs: Option<u64>,
    },
    Tool {
        id: ToolId,
        name: String,
        input: String,
        progress: Option<String>,
        content: Option<String>,
        is_error: bool,
        changes: Vec<FileChange>,
        todos: Vec<Todo>,
    },
    Notice {
        text: String,
        tone: Tone,
    },
}

#[derive(Debug, Clone)]
pub(crate) struct Cache {
    pub width: usize,
    pub detail: bool,
    pub expanded: bool,
    pub lines: Vec<Line>,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub id: u64,
    pub kind: BlockKind,
    pub(crate) cache: Option<Cache>,
}

#[derive(Debug, Clone)]
pub struct Conversation {
    pub summary: SessionSummary,
    pub blocks: Vec<Block>,
    pub running: bool,
    pub turn_started_ms: u64,
    /// 这一轮已输出的字数，状态行按它估 token。
    pub out_chars: usize,
    pub phase: Phase,
    pub goal: Option<Goal>,
    pub todos: Vec<Todo>,
    pub prompts: Vec<Prompt>,
    pub tasks: Vec<Task>,
    pub commands: Vec<SlashCommand>,
    /// 这一步开始时的块数：同一步被重发时，它之后的正文与思考作废（C-03）。
    step_mark: usize,
    /// 贴底跟随；不跟随时 `top` 是视口第一行在全部行里的位置。
    pub follow: bool,
    pub top: usize,
    pub unseen: bool,
    pub selected: Option<u64>,
    pub expanded: HashSet<u64>,
    next_id: u64,
}

impl Conversation {
    /// `now`：正在跑的会话从载入这一刻起计时（快照里没有这一轮的开始时间）。
    pub fn from_watched(w: Watched, now: u64) -> Conversation {
        let mut c = Conversation {
            summary: w.session,
            blocks: vec![],
            running: w.running,
            turn_started_ms: now,
            out_chars: 0,
            phase: Phase::Thinking,
            goal: w.goal,
            todos: w.todos,
            prompts: w.prompts,
            tasks: w.tasks,
            commands: vec![],
            step_mark: 0,
            follow: true,
            top: 0,
            unseen: false,
            selected: None,
            expanded: HashSet::new(),
            next_id: 1,
        };
        c.push(BlockKind::Welcome);
        let todos = c.todos.clone();
        for item in w.history {
            let kind = match item {
                HistoryItem::User { text } => BlockKind::User {
                    text,
                    pending: None,
                    steer: false,
                },
                HistoryItem::Assistant { text } => BlockKind::Assistant { text },
                HistoryItem::Thinking { text } => BlockKind::Thinking {
                    text,
                    started_ms: 0,
                    secs: Some(0),
                },
                HistoryItem::Tool {
                    id,
                    name,
                    input,
                    content,
                    is_error,
                    file_changes,
                } => BlockKind::Tool {
                    todos: if name == "TodoWrite" {
                        todos.clone()
                    } else {
                        vec![]
                    },
                    id,
                    name,
                    input,
                    progress: None,
                    content: Some(content),
                    is_error,
                    changes: file_changes,
                },
                HistoryItem::Notice { text } => BlockKind::Notice {
                    text,
                    tone: Tone::Dim,
                },
            };
            c.push(kind);
        }
        c.step_mark = c.blocks.len();
        c
    }

    /// 用快照里的状态更新自己，保留本地的转录。
    pub fn merge_state(&mut self, from: Conversation) {
        self.summary = from.summary;
        self.goal = from.goal;
        self.todos = from.todos;
        self.tasks = from.tasks;
        if self.prompts.is_empty() {
            self.prompts = from.prompts;
        }
    }

    pub fn push(&mut self, kind: BlockKind) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.blocks.push(Block {
            id,
            kind,
            cache: None,
        });
        if !self.follow {
            self.unseen = true;
        }
        id
    }

    pub fn block_mut(&mut self, id: u64) -> Option<&mut Block> {
        self.blocks.iter_mut().find(|b| b.id == id)
    }

    fn tool_mut(&mut self, id: &ToolId) -> Option<&mut Block> {
        self.blocks
            .iter_mut()
            .rev()
            .find(|b| matches!(&b.kind, BlockKind::Tool { id: t, .. } if t == id))
    }

    /// 思考块在下一件事来时停表。
    fn close_thinking(&mut self, now: u64) {
        if let Some(Block {
            kind: BlockKind::Thinking {
                started_ms, secs, ..
            },
            cache,
            ..
        }) = self.blocks.last_mut()
            && secs.is_none()
        {
            *secs = Some(now.saturating_sub(*started_ms) / 1000);
            *cache = None;
        }
    }

    /// 让一个事件生效。返回给界面的提示（如"已撤回：超时"）。
    pub fn apply(&mut self, ev: Event, now: u64) -> Option<String> {
        if !matches!(ev, Event::ThinkingDelta { .. }) {
            self.close_thinking(now);
        }
        match ev {
            Event::SessionUpserted(s) => self.summary = s,
            Event::TurnStarted { .. } => {
                self.running = true;
                self.turn_started_ms = now;
                self.out_chars = 0;
                self.phase = Phase::Thinking;
                self.step_mark = self.blocks.len();
            }
            Event::StepRestarted { .. } => {
                let mark = self.step_mark;
                let mut i = 0;
                self.blocks.retain(|b| {
                    i += 1;
                    i <= mark
                        || !matches!(
                            b.kind,
                            BlockKind::Assistant { .. } | BlockKind::Thinking { .. }
                        )
                });
            }
            Event::ModelRetry {
                attempt, reason, ..
            } => {
                self.push(BlockKind::Notice {
                    text: i18n::t!("chat.model_retry", attempt = attempt, reason = reason),
                    tone: Tone::Warn,
                });
            }
            Event::TextDelta { text, .. } => {
                self.phase = Phase::Answering;
                self.out_chars += text.chars().count();
                let after_mark = self.blocks.len() > self.step_mark;
                match self.blocks.last_mut() {
                    Some(Block {
                        kind: BlockKind::Assistant { text: t },
                        cache,
                        ..
                    }) if after_mark => {
                        t.push_str(&text);
                        *cache = None;
                    }
                    _ => {
                        self.push(BlockKind::Assistant { text });
                    }
                }
                if !self.follow {
                    self.unseen = true;
                }
            }
            Event::ThinkingDelta { text, .. } => {
                self.phase = Phase::Thinking;
                self.out_chars += text.chars().count();
                match self.blocks.last_mut() {
                    Some(Block {
                        kind: BlockKind::Thinking { text: t, secs, .. },
                        cache,
                        ..
                    }) if secs.is_none() => {
                        t.push_str(&text);
                        *cache = None;
                    }
                    _ => {
                        self.push(BlockKind::Thinking {
                            text,
                            started_ms: now,
                            secs: None,
                        });
                    }
                }
            }
            Event::ToolStarted { id, name, .. } => {
                self.phase = Phase::Tool(name.clone());
                self.push(BlockKind::Tool {
                    id,
                    name,
                    input: String::new(),
                    progress: None,
                    content: None,
                    is_error: false,
                    changes: vec![],
                    todos: vec![],
                });
            }
            Event::ToolInput { id, summary, .. } => {
                if let Some(b) = self.tool_mut(&id) {
                    if let BlockKind::Tool { input, .. } = &mut b.kind {
                        *input = summary;
                    }
                    b.cache = None;
                }
            }
            Event::ToolProgress { id, message, .. } => {
                if let Some(b) = self.tool_mut(&id) {
                    if let BlockKind::Tool { progress, .. } = &mut b.kind {
                        *progress = Some(message);
                    }
                    b.cache = None;
                }
            }
            Event::ToolFinished {
                id,
                content,
                is_error,
                file_changes,
                ..
            } => {
                if let Some(b) = self.tool_mut(&id) {
                    if let BlockKind::Tool {
                        content: c,
                        is_error: e,
                        changes,
                        progress,
                        ..
                    } = &mut b.kind
                    {
                        *c = Some(content);
                        *e = is_error;
                        *changes = file_changes;
                        *progress = None;
                    }
                    b.cache = None;
                }
                self.step_mark = self.blocks.len();
                self.phase = Phase::Thinking;
            }
            Event::TurnEnded { stop, .. } => {
                self.running = false;
                let text = match stop {
                    StopReason::Cancelled => Some(i18n::t!("chat.interrupted")),
                    StopReason::Budget(message) => {
                        Some(i18n::t!("chat.budget_exceeded", message = message))
                    }
                    StopReason::MaxTurns => Some(i18n::t!("chat.max_turns")),
                    _ => None,
                };
                if let Some(text) = text {
                    self.push(BlockKind::Notice {
                        text,
                        tone: Tone::Warn,
                    });
                }
            }
            Event::TurnFailed { message, .. } => {
                self.running = false;
                self.push(BlockKind::Notice {
                    text: format!("Error: {message}"),
                    tone: Tone::Error,
                });
            }
            Event::SteerDelivered { message, .. } => {
                for b in &mut self.blocks {
                    if let BlockKind::User { pending, .. } = &mut b.kind
                        && pending.as_ref() == Some(&message)
                    {
                        *pending = None;
                        b.cache = None;
                    }
                }
            }
            Event::PromptPending { prompt, .. } => {
                self.prompts.retain(|p| p.id != prompt.id);
                self.prompts.push(prompt);
            }
            Event::PromptResolved { id, resolution, .. } => {
                let had = self.prompts.iter().any(|p| p.id == id);
                self.prompts.retain(|p| p.id != id);
                if let (true, Resolution::Withdrawn { reason }) = (had, resolution) {
                    let why = match reason.as_str() {
                        "timed_out" => i18n::t!("chat.withdrawn_timed_out"),
                        "cancelled" => i18n::t!("chat.withdrawn_cancelled"),
                        "policy" => i18n::t!("chat.withdrawn_policy"),
                        other => other.to_string(),
                    };
                    return Some(i18n::t!("chat.withdrawn", why = why));
                }
            }
            Event::GoalChanged { goal, .. } => self.goal = goal,
            Event::TodosChanged { todos, .. } => {
                if let Some(Block {
                    kind: BlockKind::Tool { name, todos: t, .. },
                    cache,
                    ..
                }) = self.blocks.last_mut()
                    && name == "TodoWrite"
                {
                    *t = todos.clone();
                    *cache = None;
                }
                self.todos = todos;
            }
            Event::Compacting { .. } => self.phase = Phase::Compacting,
            Event::Compacted {
                tokens_before,
                tokens_after,
                ..
            } => {
                self.phase = Phase::Thinking;
                self.push(BlockKind::Notice {
                    text: i18n::t!(
                        "chat.compacted",
                        before = fmt_k(tokens_before),
                        after = fmt_k(tokens_after)
                    ),
                    tone: Tone::Dim,
                });
            }
            Event::TaskUpserted(t) => match self.tasks.iter_mut().find(|x| x.id == t.id) {
                Some(x) => *x = t,
                None => self.tasks.push(t),
            },
            _ => {}
        }
        None
    }

    /// 被退回的插话（`SteerReturned`）：从转录里拿掉，返回原文放回输入框。
    pub fn take_returned(&mut self, ids: &[MessageId]) -> Vec<String> {
        let mut out = vec![];
        self.blocks.retain(|b| match &b.kind {
            BlockKind::User {
                text,
                pending: Some(m),
                ..
            } if ids.contains(m) => {
                out.push(text.clone());
                false
            }
            _ => true,
        });
        out
    }

    pub fn dirty_all(&mut self) {
        for b in &mut self.blocks {
            b.cache = None;
        }
    }

    /// 重排缓存与宽度、详略不符的块；返回重排了几块（testing.md §1 用计数验性能）。
    pub fn relayout(&mut self, width: usize, detail: bool, opts: &RenderOpts) -> u64 {
        let mut n = 0;
        let summary = self.summary.clone();
        for b in &mut self.blocks {
            let expanded = self.expanded.contains(&b.id);
            let ok = b
                .cache
                .as_ref()
                .is_some_and(|c| c.width == width && c.detail == detail && c.expanded == expanded);
            if !ok {
                let mut lines = render(&b.kind, width, detail || expanded, &summary, opts);
                lines.push(Line::default());
                b.cache = Some(Cache {
                    width,
                    detail,
                    expanded,
                    lines,
                });
                n += 1;
            }
        }
        n
    }

    /// 全部已排好的行（测试与守卫用）。
    pub fn lines(&self) -> Vec<Line> {
        self.blocks
            .iter()
            .flat_map(|b| b.cache.iter().flat_map(|c| c.lines.iter().cloned()))
            .collect()
    }

    pub fn total_lines(&self) -> usize {
        self.blocks
            .iter()
            .map(|b| b.cache.as_ref().map_or(0, |c| c.lines.len()))
            .sum()
    }

    /// 某一行属于哪个块。
    pub fn block_at(&self, line: usize) -> Option<u64> {
        let mut acc = 0;
        for b in &self.blocks {
            let n = b.cache.as_ref().map_or(0, |c| c.lines.len());
            if line < acc + n {
                return Some(b.id);
            }
            acc += n;
        }
        None
    }

    pub fn first_line_of(&self, id: u64) -> Option<usize> {
        let mut acc = 0;
        for b in &self.blocks {
            if b.id == id {
                return Some(acc);
            }
            acc += b.cache.as_ref().map_or(0, |c| c.lines.len());
        }
        None
    }

    /// 视口第一行的位置。
    pub fn viewport_top(&self, height: usize) -> usize {
        let total = self.total_lines();
        let max_top = total.saturating_sub(height);
        if self.follow {
            max_top
        } else {
            self.top.min(max_top)
        }
    }

    pub fn foldable(&self) -> Vec<u64> {
        self.blocks
            .iter()
            .filter(|b| matches!(b.kind, BlockKind::Tool { .. } | BlockKind::Thinking { .. }))
            .map(|b| b.id)
            .collect()
    }
}

pub(crate) fn fmt_k(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

const COMPACT_DIFF_LINES: usize = 10;

/// 排版时要用到的设置。设置改了要让全部块重排（`dirty_all`）。
#[derive(Debug, Clone)]
pub struct RenderOpts {
    pub compact_tool_lines: usize,
    pub syntax_theme: String,
}

fn more(n: usize) -> Line {
    Line::toned(i18n::t!("chat.more_lines", n = n), Tone::Dim)
}

/// 一个块在某宽度下的全部行（交互 §4.2 的画法，照 Claude Code）。
fn render(
    kind: &BlockKind,
    width: usize,
    full: bool,
    summary: &SessionSummary,
    opts: &RenderOpts,
) -> Vec<Line> {
    let w = width.max(8);
    match kind {
        BlockKind::Welcome => crate::welcome::block(summary, w),
        BlockKind::User {
            text,
            pending,
            steer,
        } => {
            let mut ls = wrap_text(
                text,
                Tone::Normal,
                w,
                &Line::toned("> ", Tone::Dim),
                &Line::plain("  "),
            );
            if pending.is_some() {
                let last = ls.pop().unwrap_or_default();
                ls.push(last.push(i18n::t!("chat.pending"), Tone::Dim));
            } else if *steer {
                let last = ls.pop().unwrap_or_default();
                ls.push(last.push(i18n::t!("chat.steered"), Tone::Dim));
            }
            ls.into_iter().map(|l| l.with_tone(Tone::UserMsg)).collect()
        }
        BlockKind::Assistant { text } => crate::text::markdown_with(
            text,
            w,
            &Line::plain("⏺ "),
            &Line::plain("  "),
            &opts.syntax_theme,
        ),
        BlockKind::Thinking { text, secs, .. } => {
            let head = match secs {
                None => i18n::t!("chat.thinking"),
                Some(s) if full => i18n::t!("chat.thought_for", s = s),
                Some(s) => i18n::t!("chat.thought_for_expand", s = s),
            };
            let mut ls = vec![Line::toned(head, Tone::Dim)];
            if full {
                ls.extend(wrap_text(
                    text,
                    Tone::Dim,
                    w,
                    &Line::plain("  "),
                    &Line::plain("  "),
                ));
            }
            ls
        }
        BlockKind::Tool {
            name,
            input,
            progress,
            content,
            is_error,
            changes,
            todos,
            ..
        } => {
            let dot = match (content, is_error) {
                (None, _) => Tone::Warn,
                (Some(_), true) => Tone::Error,
                (Some(_), false) => Tone::Ok,
            };
            let mut head = Line::toned("⏺ ", dot).push(name.clone(), Tone::Strong);
            if !input.is_empty() {
                head = head.push(format!("({input})"), Tone::Normal);
            }
            let mut ls = crate::text::wrap(&head.spans, w, &Line::default(), &Line::plain("  "));
            let lead = || Line::toned("  ⎿  ", Tone::Dim);
            let cont = || Line::plain("     ");
            if name == "TodoWrite" {
                for (i, t) in todos.iter().enumerate() {
                    let (g, _) = crate::marks::todo(t.status);
                    let tone = match t.status {
                        TodoStatus::Done => Tone::Dim,
                        TodoStatus::InProgress => Tone::Strong,
                        TodoStatus::Pending => Tone::Normal,
                    };
                    let first = if i == 0 { lead() } else { cont() };
                    ls.extend(wrap_text(
                        &format!("{g} {}. {}", i + 1, t.text),
                        tone,
                        w,
                        &first,
                        &cont(),
                    ));
                }
                return ls;
            }
            let Some(content) = content else {
                let p = progress.clone().unwrap_or_else(|| i18n::t!("chat.running"));
                ls.extend(wrap_text(&p, Tone::Dim, w, &lead(), &cont()));
                return ls;
            };
            if !changes.is_empty() {
                for (i, ch) in changes.iter().enumerate() {
                    let diff = ch.diff.as_deref().unwrap_or("");
                    let add = diff
                        .lines()
                        .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
                        .count();
                    let del = diff
                        .lines()
                        .filter(|l| l.starts_with('-') && !l.starts_with("---"))
                        .count();
                    let key = match ch.kind {
                        port::FileChangeKind::Created => "chat.file_created",
                        port::FileChangeKind::Modified => "chat.file_modified",
                        port::FileChangeKind::Deleted => "chat.file_deleted",
                    };
                    let first = if i == 0 { lead() } else { cont() };
                    ls.extend(wrap_text(
                        &i18n::tr_args(key, &[("path", &ch.path), ("add", &add), ("del", &del)]),
                        Tone::Normal,
                        w,
                        &first,
                        &cont(),
                    ));
                    let body: Vec<&str> = diff
                        .lines()
                        .filter(|l| !l.starts_with("+++") && !l.starts_with("---"))
                        .collect();
                    let shown = if full {
                        body.len()
                    } else {
                        body.len().min(COMPACT_DIFF_LINES)
                    };
                    for l in &body[..shown] {
                        let tone = if l.starts_with('+') {
                            Tone::Add
                        } else if l.starts_with('-') {
                            Tone::Del
                        } else {
                            Tone::Dim
                        };
                        ls.extend(wrap_text(l, tone, w, &cont(), &cont()));
                    }
                    if shown < body.len() {
                        ls.push(more(body.len() - shown));
                    }
                }
                return ls;
            }
            if *is_error {
                let text = format!("Error: {}", content.trim());
                ls.extend(wrap_text(&text, Tone::Error, w, &lead(), &cont()));
                return ls;
            }
            let lines: Vec<&str> = content.trim_end().lines().collect();
            if name == "Read" && !full {
                ls.extend(wrap_text(
                    &i18n::t!("chat.read_lines", n = lines.len()),
                    Tone::Dim,
                    w,
                    &lead(),
                    &cont(),
                ));
                return ls;
            }
            if lines.is_empty() {
                ls.push(lead().push(i18n::t!("chat.no_output"), Tone::Dim));
                return ls;
            }
            let shown = if full {
                lines.len()
            } else {
                lines.len().min(opts.compact_tool_lines)
            };
            for (i, l) in lines[..shown].iter().enumerate() {
                let first = if i == 0 { lead() } else { cont() };
                ls.extend(wrap_text(l, Tone::Normal, w, &first, &cont()));
            }
            if shown < lines.len() {
                ls.push(more(lines.len() - shown));
            }
            ls
        }
        BlockKind::Notice { text, tone } => wrap_text(
            text,
            *tone,
            w,
            &Line::toned("  ⎿  ", Tone::Dim),
            &Line::plain("     "),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conversation() -> Conversation {
        let w: Watched = serde_json::from_value(serde_json::json!({
            "session": {
                "id": "s1", "title": "t", "state": "idle",
                "provenance": {
                    "instance": "lv", "host": "h", "scene": "x", "model": "m",
                },
            },
        }))
        .unwrap();
        Conversation::from_watched(w, 0)
    }

    fn last_notice(c: &Conversation) -> Option<(String, Tone)> {
        match c.blocks.last().map(|b| &b.kind) {
            Some(BlockKind::Notice { text, tone }) => Some((text.clone(), *tone)),
            _ => None,
        }
    }

    fn end(c: &mut Conversation, stop: StopReason) {
        c.apply(
            Event::TurnEnded {
                session: port::SessionId("s1".into()),
                stop,
                usage: Default::default(),
            },
            0,
        );
    }

    #[test]
    fn a_budget_stop_says_whose_budget_it_was() {
        let mut c = conversation();
        end(&mut c, StopReason::Budget("x".into()));
        let (text, tone) = last_notice(&c).expect("有提示");
        assert!(text.contains('x'), "{text}");
        assert_eq!(tone, Tone::Warn);
    }

    #[test]
    fn hitting_the_call_limit_is_said_and_a_normal_end_is_not() {
        let mut c = conversation();
        end(&mut c, StopReason::EndTurn);
        assert!(last_notice(&c).is_none());
        end(&mut c, StopReason::MaxTurns);
        assert_eq!(last_notice(&c).expect("有提示").1, Tone::Warn);
    }
}
