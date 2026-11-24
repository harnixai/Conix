use port::ConnState;
use tui::layout::{self, LayoutSpec, MIN_COLS, MIN_ROWS};
use tui::{
    Body, ChatPane, Composer, FrameState, Header, Hint, Line, Section, SidePane, TabLabel, Tone,
    Workbench, truncate_to_width,
};

use crate::state::{Confirm, DetachReason, Focus, Overlay, Picker, Screen, State, Tab};

/// 提问框里第一个选项所在的行（鼠标点选项用，交互 §4.6）。
pub(crate) const ASK_OPTIONS_OFFSET: usize = 4;
const SPINNER: [&str; 4] = ["✻", "✽", "✶", "✳"];

pub(crate) fn layout_spec(s: &State) -> LayoutSpec {
    chat_layout(s).0
}

/// 两遍：先按一行输入框算出宽度，再按这个宽度排输入框（或提问框）定高度。
pub(crate) fn chat_layout(s: &State) -> (LayoutSpec, Vec<String>) {
    let labels = tab_labels(s);
    let conv = s.conv();
    let mut spec = LayoutSpec {
        mode: layout::pane_mode_for(
            s.ui.size.cols,
            s.ui.ratio_pct,
            s.ui.side_collapsed,
            s.ui.showing,
        ),
        composer_rows: 1,
        status: conv.is_some_and(|c| c.running || !c.prompts.is_empty()),
        panel_rows: 0,
    };
    if let Some(chat) = layout::compute(s.ui.size, &spec, &labels).chat {
        let w = chat.composer_inner.w as usize;
        spec.panel_rows = s.panel_rows(chat.transcript.w as usize);
        spec.composer_rows = match ask_lines(s, w.saturating_sub(2)) {
            Some((lines, _)) => lines.len() as u16,
            None => {
                s.ui.composer
                    .layout(w, &i18n::t!("view.placeholder"))
                    .0
                    .len() as u16
            }
        };
    }
    (spec, labels)
}

fn tabs(s: &State) -> Vec<TabLabel> {
    Tab::ALL
        .iter()
        .map(|t| TabLabel {
            label: t.label(),
            badge: badge(s, *t),
            active: *t == s.ui.tab,
        })
        .collect()
}

pub(crate) fn tab_labels(s: &State) -> Vec<String> {
    tabs(s).iter().map(TabLabel::display).collect()
}

/// "等你处理"的数量（需求 L-04）：标签据此改提醒色。
fn badge(s: &State, t: Tab) -> u32 {
    s.attention_count(t)
}

pub fn view(s: &State) -> FrameState {
    let size = s.ui.size;
    let footer = footer(s);
    if size.cols < MIN_COLS || size.rows < MIN_ROWS {
        return FrameState {
            theme: s.settings.theme.clone(),
            body: Body::TooSmall {
                need_cols: MIN_COLS,
            },
            footer,
            overlay: None,
        };
    }
    match &s.ui.screen {
        Screen::Instances(p) => {
            return FrameState {
                theme: s.settings.theme.clone(),
                body: Body::Instances(picker(s, p)),
                footer,
                overlay: s.ui.overlay.as_ref().map(|o| overlay(s, o)),
            };
        }
        Screen::Detached(r) => {
            return FrameState {
                theme: s.settings.theme.clone(),
                body: Body::Detached(tui::DetachedScreen {
                    lines: detached(s, r),
                }),
                footer,
                overlay: None,
            };
        }
        Screen::Workbench => {}
    }
    let spec = layout_spec(s);
    let tabs = tabs(s);
    let labels: Vec<String> = tabs.iter().map(TabLabel::display).collect();
    let regions = layout::compute(size, &spec, &labels);
    let chat = regions.chat.as_ref().map(|r| chat_pane(s, r));
    let side = regions.side.as_ref().map(|r| side_pane(s, tabs, r));
    FrameState {
        theme: s.settings.theme.clone(),
        body: Body::Workbench(Box::new(Workbench {
            layout: spec,
            header: header(s),
            chat,
            side,
        })),
        footer,
        overlay: s.ui.overlay.as_ref().map(|o| overlay(s, o)),
    }
}

/// 顶栏：`[CONIX] 实例@主机  项目目录  场景  模型`。实例与项目是全局的；场景、模型取当前
/// 对话区的会话（没有会话时不写）。
fn header(s: &State) -> Header {
    let mut left = Line::toned("[CONIX]", Tone::Accent);
    if let Some(i) = &s.domain.instance {
        left = left
            .push(format!(" {}@{}", i.name, i.host), Tone::Strong)
            .push(
                format!(
                    "  {}",
                    i.project
                        .clone()
                        .unwrap_or_else(|| i18n::t!("common.no_project"))
                ),
                Tone::Normal,
            );
    }
    if let Some(c) = s.conv() {
        let p = &c.summary.provenance;
        left = left.push(format!("  {}  {}", p.scene, p.model), Tone::Dim);
    }
    let right = match s.domain.conn {
        Some(ConnState::Connected) | Some(ConnState::Resynced) => {
            Line::toned(i18n::t!("view.conn.connected"), Tone::Ok)
        }
        Some(ConnState::Reconnecting { attempt }) => Line::toned(
            i18n::t!("view.conn.reconnecting", attempt = attempt),
            Tone::Warn,
        ),
        Some(ConnState::Lost) => Line::toned(i18n::t!("view.conn.lost"), Tone::Error),
        None => Line::toned(i18n::t!("view.conn.connecting"), Tone::Dim),
    };
    Header { left, right }
}

fn chat_pane(s: &State, r: &tui::layout::ChatRegions) -> ChatPane {
    let width = r.transcript.w as usize;
    let focused = matches!(s.ui.focus, Focus::Composer | Focus::Transcript);
    let conv = s.conv();
    let mut trail: Vec<String> = s.ui.ctx_parents.iter().map(|(_, t)| t.clone()).collect();
    let task_label =
        s.ui.task_view
            .as_ref()
            .and_then(|t| conv.and_then(|c| c.tasks.iter().find(|x| &x.id == t)))
            .map(|t| t.label.clone());
    let (crumbs, badge) = match conv {
        Some(c) => (
            {
                if task_label.is_none() || s.ui.ctx_parents.is_empty() {
                    trail.push(c.summary.title.clone());
                }
                if let Some(t) = &task_label {
                    trail.push(t.clone());
                }
                trail.dedup();
                format!("{} › {}", crumb_root(s, c), trail.join(" › "))
            },
            badge_text(&c.summary),
        ),
        None => (i18n::t!("view.crumb.new_session"), String::new()),
    };
    let h = r.transcript.h as usize;
    let mut below = None;
    let transcript = match (conv, &s.ui.chat) {
        (Some(_), _) if s.ui.task_view.is_some() => {
            let task = s.ui.task_view.as_ref().unwrap();
            let text = s.domain.task_output.get(task).cloned().unwrap_or_default();
            let mut lines: Vec<Line> = text
                .lines()
                .flat_map(|l| {
                    crate::text::wrap_text(
                        l,
                        Tone::Normal,
                        width,
                        &Line::plain("  "),
                        &Line::plain("  "),
                    )
                })
                .collect();
            if lines.is_empty() {
                lines.push(Line::toned(i18n::t!("view.task.empty"), Tone::Dim));
            }
            let skip = lines.len().saturating_sub(h);
            lines.into_iter().skip(skip).collect()
        }
        (Some(c), _) => {
            let top = c.viewport_top(h);
            let sel = (s.ui.focus == Focus::Transcript)
                .then_some(c.selected)
                .flatten()
                .and_then(|id| c.first_line_of(id));
            if c.unseen && !c.follow {
                below = Some(Line::toned(i18n::t!("view.new_below"), Tone::Attention));
            }
            c.blocks
                .iter()
                .flat_map(|b| b.cache.iter().flat_map(|x| x.lines.iter()))
                .enumerate()
                .skip(top)
                .take(h)
                .map(|(i, l)| {
                    if Some(i) == sel {
                        l.clone().with_tone(Tone::Selected)
                    } else {
                        l.clone()
                    }
                })
                .collect()
        }
        (None, Some(_)) => vec![Line::toned(i18n::t!("view.loading"), Tone::Dim)],
        (None, None) => crate::welcome::idle(s.domain.instance.as_ref(), width),
    };
    let composer_w = r.composer_inner.w as usize;
    let placeholder = match (&s.ui.answering, conv) {
        _ if s.ui.task_view.is_some() => i18n::t!("view.ph.task"),
        _ if s.ui.rename.is_some() => i18n::t!("view.ph.rename"),
        _ if s.ui.via_task.is_some() => i18n::t!("view.ph.via_task"),
        (Some(_), Some(c)) => c
            .prompts
            .first()
            .map(|p| match &p.kind {
                port::PromptKind::Question { question, .. } => {
                    i18n::t!("view.ph.answer", question = question)
                }
                _ => i18n::t!("view.ph.write_answer"),
            })
            .unwrap_or_default(),
        _ => i18n::t!("view.placeholder"),
    };
    let (mut lines, cursor) = s.ui.composer.layout(composer_w, &placeholder);
    // 输入框最多占对话区 1/3：再多就开窗，缺省跟着光标走，Shift+↑↓ / PageUp / PageDown 可以翻。
    let rows = r.composer_inner.h as usize;
    let mut cursor = Some(cursor);
    if rows > 0 && lines.len() > rows {
        let off = s.composer_offset(lines.len(), rows);
        lines = lines.into_iter().skip(off).take(rows).collect();
        cursor = cursor
            .filter(|(_, r)| (*r as usize) >= off && (*r as usize) < off + rows)
            .map(|(c, r)| (c, r - off as u16));
    }
    let picker = s.picker_candidates();
    let ask = ask_lines(s, composer_w.saturating_sub(2));
    let running = conv.is_some_and(|c| c.running);
    let hint_left = if s.ui.esc_armed_ms.is_some() && !s.ui.composer.is_empty() {
        Line::toned(i18n::t!("view.hint.esc_clear"), Tone::Warn)
    } else if running {
        Line::toned(i18n::t!("view.hint.interrupt"), Tone::Dim)
    } else {
        Line::toned(i18n::t!("view.hint.shortcuts"), Tone::Dim)
    };
    let hint_right = match s.permission_shown() {
        port::PermissionMode::Default => Line::default(),
        m @ (port::PermissionMode::BypassPermissions | port::PermissionMode::DontAsk) => {
            Line::toned(permission_label(m), Tone::Error)
        }
        m => Line::toned(permission_label(m), Tone::Accent),
    };
    ChatPane {
        focused,
        crumbs: Line::plain(crumbs),
        badge: Line::plain(badge),
        transcript,
        transcript_focused: s.ui.focus == Focus::Transcript,
        below,
        status: conv.and_then(|c| status_line(s, c, width)),
        panel: r
            .panel
            .map(|p| s.panel_lines(width, p.h as usize))
            .unwrap_or_default(),
        composer: Composer {
            focused: s.ui.focus == Focus::Composer && s.ui.overlay.is_none() && ask.is_none(),
            lines,
            cursor,
        },
        picker: picker
            .iter()
            .map(|c| {
                if c.source == crate::chat_update::MENTION {
                    return Line::toned(format!("  @{}", c.name), Tone::Accent);
                }
                Line::toned(format!("  /{:<14}", c.name), Tone::Accent)
                    .push(
                        truncate_to_width(&c.description, width.saturating_sub(30)),
                        Tone::Normal,
                    )
                    .push(format!("  {}", c.source), Tone::Dim)
            })
            .collect(),
        picker_selected: (!picker.is_empty()).then(|| s.ui.composer.picker.min(picker.len() - 1)),
        ask_selected: ask.as_ref().and_then(|(_, sel)| *sel),
        ask: ask.map(|(l, _)| l),
        hint_left,
        hint_right,
    }
}

/// 面包屑从哪个标签起：流程、作业开出来的会话（包括它们的编辑会话）不属于"会话"。
fn crumb_root(s: &State, c: &crate::chat::Conversation) -> String {
    let root =
        s.ui.ctx_parents
            .first()
            .and_then(|(id, _)| s.domain.sessions.iter().find(|x| &x.id == id))
            .unwrap_or(&c.summary);
    match &root.provenance.origin {
        port::Origin::FlowStation { .. } => i18n::t!("view.root.flow"),
        port::Origin::Job { .. } => i18n::t!("view.root.job"),
        _ => i18n::t!("view.root.session"),
    }
}

/// 对话框标题右边：会话从哪来（人新建的不写）、被谁占着、现在的状态。全局的实例、项目、
/// 场景、模型在顶栏。
fn badge_text(sum: &port::SessionSummary) -> String {
    let origin = match &sum.provenance.origin {
        port::Origin::User => None,
        port::Origin::Job { name, .. } => Some(i18n::t!("view.badge.job", name = name)),
        // 运行的主会话没有站（实例记作 `@main`）。
        port::Origin::FlowStation {
            flow_title,
            subject,
            station,
            ..
        } if station.0 == "@main" => Some(i18n::t!(
            "view.badge.flow_main",
            flow_title = flow_title,
            subject = subject
        )),
        port::Origin::FlowStation {
            flow_title,
            subject,
            station_title,
            ..
        } => Some(i18n::t!(
            "view.badge.flow_station",
            flow_title = flow_title,
            subject = subject,
            station_title = station_title
        )),
        port::Origin::SubAgent { parent_title, .. } => Some(i18n::t!(
            "view.badge.sub_agent",
            parent_title = parent_title
        )),
        port::Origin::Fork { from_title, .. } => {
            Some(i18n::t!("view.badge.fork", from_title = from_title))
        }
    };
    let held = sum
        .held_by
        .as_ref()
        .map(|h| i18n::t!("view.badge.held", holder = h));
    let state = match sum.state {
        port::SessionState::Running => i18n::t!("view.badge.running"),
        port::SessionState::Waiting => i18n::t!("view.badge.waiting"),
        port::SessionState::Idle => i18n::t!("view.badge.idle"),
    };
    origin
        .into_iter()
        .chain(held)
        .chain(std::iter::once(state))
        .collect::<Vec<_>>()
        .join(" · ")
}

fn status_line(s: &State, c: &crate::chat::Conversation, width: usize) -> Option<Line> {
    if let Some(p) = c.prompts.first() {
        let what = match &p.kind {
            port::PromptKind::Permission { tool, summary, .. } => {
                i18n::t!("view.status.need_approval", tool = tool, summary = summary)
            }
            port::PromptKind::Question { question, .. } => {
                i18n::t!("view.status.need_answer", question = question)
            }
        };
        return Some(Line::toned(
            truncate_to_width(&what, width),
            Tone::Attention,
        ));
    }
    if !c.running {
        return None;
    }
    let frame = SPINNER[(s.ui.now_ms / crate::update::SPINNER_MS) as usize % SPINNER.len()];
    let verb = match &c.phase {
        crate::chat::Phase::Thinking => i18n::t!("view.phase.thinking"),
        crate::chat::Phase::Answering => i18n::t!("view.phase.answering"),
        crate::chat::Phase::Tool(name) => i18n::t!("view.phase.tool", name = name),
        crate::chat::Phase::Compacting => i18n::t!("view.phase.compacting"),
    };
    let secs = s.ui.now_ms.saturating_sub(c.turn_started_ms) / 1000;
    let mut tail = format!("{secs}s");
    if c.out_chars > 0 {
        tail.push_str(&format!(
            " · ↓ {} tokens",
            crate::chat::fmt_k((c.out_chars / 3).max(1) as u64)
        ));
    }
    Some(
        Line::toned(format!("{frame} "), Tone::Accent)
            .push(verb, Tone::Accent)
            .push(format!(" ({tail})"), Tone::Dim),
    )
}

/// 提问框的行与选中行（交互 §4.6，照 Claude Code）。固定结构：标题、摘要、来源、问句，
/// 然后从第 `ASK_OPTIONS_OFFSET` 行起是选项，最后一行是键位提示。
fn ask_lines(s: &State, width: usize) -> Option<(Vec<Line>, Option<usize>)> {
    let c = s.conv()?;
    let p = c.prompts.first()?;
    // 看任务输出时不显示提问框：那时 Esc 是"返回"，不能变成替你拒绝一次审批。
    if s.ui.answering.is_some() || s.ui.task_view.is_some() {
        return None;
    }
    let count = if c.prompts.len() > 1 {
        format!("({}/{})", 1, c.prompts.len())
    } else {
        String::new()
    };
    let (title, summary, question) = match &p.kind {
        port::PromptKind::Permission { tool, summary, .. } => (
            i18n::t!("view.ask.command", tool = tool),
            summary.clone(),
            i18n::t!("view.ask.proceed"),
        ),
        port::PromptKind::Question {
            question, header, ..
        } => (
            header
                .clone()
                .unwrap_or_else(|| i18n::t!("view.ask.model_question")),
            String::new(),
            question.clone(),
        ),
    };
    let fit = |t: String| truncate_to_width(&t, width);
    let pad = width.saturating_sub(tui::display_width(&title) + tui::display_width(&count));
    let mut lines = vec![
        Line::toned(fit(title), Tone::Strong)
            .push(format!("{}{count}", " ".repeat(pad)), Tone::Dim),
        Line::toned(fit(format!("  {summary}")), Tone::Normal),
        match &p.from {
            Some(f) => Line::toned(fit(i18n::t!("view.ask.from", label = f.label)), Tone::Dim),
            None => Line::default(),
        },
        Line::toned(fit(question), Tone::Normal),
    ];
    let options = crate::chat_update::ask_options(&p.kind);
    let sel = s.ui.ask_selected.min(options.len() - 1);
    for (i, o) in options.iter().enumerate() {
        let mark = if i == sel && s.ui.ask_active {
            "❯ "
        } else {
            "  "
        };
        lines.push(Line::plain(fit(format!("{mark}{}. {o}", i + 1))));
    }
    let keys = if s.ui.ask_active {
        i18n::t!("view.ask.keys_active")
    } else {
        i18n::t!("view.ask.keys_idle")
    };
    lines.push(Line::toned(fit(keys), Tone::Dim));
    let selected = s.ui.ask_active.then_some(ASK_OPTIONS_OFFSET + sel);
    Some((lines, selected))
}

fn side_pane(s: &State, tabs: Vec<TabLabel>, r: &tui::layout::SideRegions) -> SidePane {
    let w = r.upper.w as usize;
    // 第一列是选中标记，内容从第二列起。
    let all = match s.ui.tab {
        Tab::Sessions => crate::sessions::upper_lines(s, w.saturating_sub(1)),
        Tab::Flows => crate::flows::upper_lines(s, w.saturating_sub(1)),
        Tab::Jobs => crate::jobs::upper_lines(s, w.saturating_sub(1)),
    };
    let all: Vec<Line> = if matches!(
        s.domain.conn,
        Some(port::ConnState::Lost | port::ConnState::Reconnecting { .. })
    ) {
        all.into_iter()
            .map(|l| Line::toned(l.text(), Tone::Dim))
            .collect()
    } else {
        all
    };
    let up = s.ui.upper();
    let visible: Vec<Line> = all
        .iter()
        .skip(up.scroll)
        .take(r.upper.h as usize)
        .cloned()
        .collect();
    let selected = (!all.is_empty() && up.selected >= up.scroll)
        .then(|| up.selected - up.scroll)
        .filter(|i| *i < visible.len());
    let lower_title = upper_title(s);
    SidePane {
        tabs,
        upper: Section {
            focused: s.ui.focus == Focus::SideUpper && s.ui.overlay.is_none(),
            title: None,
            title_right: None,
            rows: match (&s.domain.load_errors[s.ui.tab.index()], all.is_empty()) {
                (Some(e), _) => vec![Line::toned(
                    truncate_to_width(&i18n::t!("view.load_failed", error = e), w),
                    Tone::Error,
                )],
                (None, true) => vec![Line::toned(empty_hint(s), Tone::Dim)],
                // 会话列表总有"新建会话"那一行；没有别的会话时在它下面给提示。
                (None, false)
                    if s.ui.tab == Tab::Sessions
                        && s.ui.filter[0].is_empty()
                        && s.user_sessions().is_empty() =>
                {
                    let mut v = visible;
                    v.push(Line::toned(empty_hint(s), Tone::Dim));
                    v
                }
                (None, false) => visible,
            },
            selected,
        },
        lower: lower_section(s, r, lower_title),
    }
}

fn empty_hint(s: &State) -> String {
    let f = &s.ui.filter[s.ui.tab.index()];
    if !f.is_empty() {
        return i18n::t!("view.empty.filter", filter = f);
    }
    match s.ui.tab {
        Tab::Sessions => i18n::t!("view.empty.sessions"),
        // 实例还没接流程（architecture §8.6）：别让人以为是自己没建。
        Tab::Flows if s.domain.instance.is_some() && !s.domain.flows_available => {
            i18n::t!("view.empty.flows_unavailable")
        }
        Tab::Flows => i18n::t!("view.empty.flows"),
        Tab::Jobs => i18n::t!("view.empty.jobs"),
    }
}

fn lower_section(s: &State, r: &tui::layout::SideRegions, title: Option<String>) -> Section {
    let focused = s.ui.focus == Focus::SideLower && s.ui.overlay.is_none();
    let w = r.lower.w as usize;
    match s.ui.tab {
        Tab::Sessions => {
            let all = crate::sessions::lower_lines(s, w.saturating_sub(1));
            let lo = s.ui.lower();
            Section {
                focused,
                title,
                title_right: crate::sessions::lower_summary(s),
                selected: lo
                    .selected
                    .checked_sub(lo.scroll)
                    .filter(|i| *i < r.lower.h as usize && !all.is_empty()),
                rows: all
                    .into_iter()
                    .skip(lo.scroll)
                    .take(r.lower.h as usize)
                    .collect(),
            }
        }
        Tab::Flows | Tab::Jobs => {
            let all = if s.ui.tab == Tab::Flows {
                crate::flows::lower_lines(s, w.saturating_sub(1))
            } else {
                crate::jobs::lower_lines(s, w.saturating_sub(1))
            };
            let (title, right) = if s.ui.tab == Tab::Flows {
                crate::flows::lower_title(s)
            } else {
                (title, None)
            };
            let lo = s.ui.lower();
            Section {
                focused,
                title,
                title_right: right,
                selected: lo
                    .selected
                    .checked_sub(lo.scroll)
                    .filter(|i| *i < r.lower.h as usize && !all.is_empty()),
                rows: all
                    .into_iter()
                    .skip(lo.scroll)
                    .take(r.lower.h as usize)
                    .collect(),
            }
        }
    }
}

fn upper_title(s: &State) -> Option<String> {
    match s.ui.tab {
        Tab::Sessions => s
            .selected_session()
            .and_then(|id| s.domain.sessions.iter().find(|x| x.id == id))
            .map(|x| x.title.clone()),
        Tab::Flows => None,
        Tab::Jobs => s
            .selected_job()
            .and_then(|id| s.domain.jobs.iter().find(|j| j.id == id))
            .map(|j| i18n::t!("view.job_detail", name = j.name)),
    }
}

fn footer(s: &State) -> tui::Footer {
    let ctx = s.key_contexts()[0];
    let hints = s
        .keymap
        .hints(ctx)
        .into_iter()
        .map(|(key, label)| Hint {
            label: if label == keys::Action::NextAttention.help() {
                format!("{label}({})", s.attention().len())
            } else {
                label.to_string()
            },
            key,
        })
        .collect();
    let essential = s
        .keymap
        .essential()
        .into_iter()
        .map(|(key, label)| Hint { key, label })
        .collect();
    let confirm = s.ui.confirm.as_ref().map(|c| {
        let text = match c {
            Confirm::ForceAttach(n) => i18n::t!("view.confirm.force_attach", name = n),
            Confirm::StopInstance(n) => {
                i18n::t!("view.confirm.stop_instance", name = n)
            }
            Confirm::CloseSession(_, t) => i18n::t!("view.confirm.close_session", title = t),
            Confirm::StopTask(_, t) => i18n::t!("view.confirm.stop_task", title = t),
            Confirm::ClearGoal(_) => i18n::t!("view.confirm.clear_goal"),
            Confirm::CancelRun(_, t) => i18n::t!("view.confirm.cancel_run", title = t),
            Confirm::StopFiring(_, _, n) => i18n::t!("view.confirm.stop_firing", name = n),
            Confirm::DeleteSession(_, t) => i18n::t!("view.confirm.delete_session", title = t),
            Confirm::ForceStation(_, _, t) => {
                i18n::t!("view.confirm.force_station", title = t)
            }
            Confirm::Rework(_, _, t) => {
                i18n::t!("view.confirm.rework", title = t)
            }
        };
        Line::toned(text, Tone::Attention)
    });
    let message = confirm.or_else(|| {
        s.ui.notice.as_ref().map(|n| {
            Line::toned(
                n.text.clone(),
                if n.error { Tone::Error } else { Tone::Warn },
            )
        })
    });
    tui::Footer {
        hints,
        essential,
        message,
    }
}

fn overlay(s: &State, o: &Overlay) -> tui::Overlay {
    let text = |title: &str, lines: Vec<String>, scroll: usize| {
        let height = (lines.len() as u16 + 2)
            .min(s.ui.size.rows.saturating_sub(4))
            .max(5);
        let body = height.saturating_sub(2) as usize;
        let max = lines.len().saturating_sub(body);
        tui::Overlay {
            title: title.to_string(),
            lines: lines
                .into_iter()
                .skip(scroll.min(max))
                .map(Line::plain)
                .collect(),
            width: (s.ui.size.cols.saturating_sub(10)).min(110),
            height,
        }
    };
    match o {
        Overlay::Text {
            title,
            lines,
            scroll,
        } => text(title, lines.clone(), *scroll),
        Overlay::EventLog { scroll } => {
            let mut lines: Vec<String> = s.ui.event_log.iter().rev().cloned().collect();
            if lines.is_empty() {
                lines.push(i18n::t!("view.event_log.empty"));
            }
            text(&i18n::t!("view.event_log.title"), lines, *scroll)
        }
        Overlay::Rewind {
            turns, selected, ..
        } => {
            let mut lines: Vec<Line> = turns
                .iter()
                .enumerate()
                .map(|(i, t)| {
                    let mark = if i == *selected { "› " } else { "  " };
                    let mut l = Line::plain(mark).push(
                        truncate_to_width(&t.prompt.replace('\n', " "), 70),
                        Tone::Normal,
                    );
                    if !t.files.is_empty() {
                        l = l.push(i18n::t!("view.rewind.files", n = t.files.len()), Tone::Dim);
                    }
                    if !t.restorable {
                        l = l.push(i18n::t!("view.rewind.unrestorable"), Tone::Warn);
                    }
                    if i == *selected {
                        l = l.with_tone(Tone::Selected);
                    }
                    l
                })
                .collect();
            let body = s.ui.size.rows.saturating_sub(8) as usize;
            let skip = selected.saturating_sub(body.saturating_sub(1));
            lines = lines.into_iter().skip(skip).take(body).collect();
            lines.push(Line::default());
            lines.push(Line::toned(i18n::t!("view.rewind.keys"), Tone::Dim));
            tui::Overlay {
                title: i18n::t!("view.rewind.title"),
                height: lines.len() as u16 + 2,
                lines,
                width: (s.ui.size.cols.saturating_sub(10)).min(100),
            }
        }
        Overlay::Plugins {
            plugins,
            selected,
            armed,
        } => crate::plugins::list_view(s, plugins, *selected, armed),
        Overlay::PluginDisclosure {
            plugin,
            disclosure,
            scroll,
            installing,
        } => crate::plugins::disclosure_view(s, plugin, disclosure, *scroll, *installing),
        Overlay::Models { models, selected } => {
            let mut lines: Vec<Line> = models
                .iter()
                .enumerate()
                .map(|(i, m)| {
                    let mark = if i == *selected { "› " } else { "  " };
                    let mut l = Line::plain(mark)
                        .push(
                            m.display_name.clone().unwrap_or_else(|| m.id.clone()),
                            Tone::Normal,
                        )
                        .push(format!("  {} · {}", m.provider, m.id), Tone::Dim);
                    if i == *selected {
                        l = l.with_tone(Tone::Selected);
                    }
                    l
                })
                .collect();
            let body = s.ui.size.rows.saturating_sub(8) as usize;
            let skip = selected.saturating_sub(body.saturating_sub(1));
            lines = lines.into_iter().skip(skip).take(body).collect();
            lines.push(Line::default());
            lines.push(Line::toned(i18n::t!("view.models.keys"), Tone::Dim));
            tui::Overlay {
                title: i18n::t!("view.models.title"),
                height: lines.len() as u16 + 2,
                lines,
                width: (s.ui.size.cols.saturating_sub(10)).min(90),
            }
        }
        Overlay::Away {
            since_ms,
            alerts,
            selected,
        } => {
            let away = crate::sessions::fmt_elapsed(s.ui.now_ms.saturating_sub(*since_ms));
            let mut lines = vec![];
            for (i, a) in alerts.iter().enumerate() {
                let (g, tone) = match a.level {
                    port::NoticeLevel::Error => ("✗", Tone::Error),
                    port::NoticeLevel::Warn => ("◐", Tone::Warn),
                    port::NoticeLevel::Info => ("✓", Tone::Ok),
                };
                let mark = if i == *selected { "› " } else { "  " };
                let mut l = Line::plain(mark)
                    .push(format!("{g} "), tone)
                    .push(a.title.clone(), Tone::Normal);
                if !a.text.is_empty() {
                    l = l.push(format!("  {}", a.text), Tone::Dim);
                }
                if i == *selected {
                    l = l.with_tone(Tone::Selected);
                }
                lines.push(l);
            }
            lines.push(Line::default());
            lines.push(Line::toned(i18n::t!("view.away.keys"), Tone::Dim));
            tui::Overlay {
                title: i18n::t!("view.away.title", elapsed = away),
                height: (lines.len() as u16 + 2).min(s.ui.size.rows.saturating_sub(4)),
                lines,
                width: (s.ui.size.cols.saturating_sub(10)).min(100),
            }
        }
        Overlay::Exit { selected } => {
            let name =
                s.ui.attached
                    .clone()
                    .unwrap_or_else(|| i18n::t!("view.exit.current_instance"));
            let options = [
                (
                    i18n::t!("view.exit.background"),
                    i18n::t!("view.exit.background_desc", name = name),
                ),
                (
                    i18n::t!("view.exit.quit"),
                    i18n::t!("view.exit.quit_desc", name = name),
                ),
                (i18n::t!("view.exit.back"), String::new()),
            ];
            let mut lines = vec![Line::plain(i18n::t!("view.exit.ask")), Line::default()];
            for (i, (label, desc)) in options.iter().enumerate() {
                let mark = if i == *selected { "❯ " } else { "  " };
                let mut l = Line::plain(format!("{mark}{}. ", i + 1))
                    .push(format!("{label}  "), Tone::Strong)
                    .push(desc.clone(), Tone::Dim);
                if i == *selected {
                    l = l.with_tone(Tone::Selected);
                }
                lines.push(l);
            }
            lines.push(Line::default());
            lines.push(Line::toned(i18n::t!("view.exit.keys"), Tone::Dim));
            tui::Overlay {
                title: i18n::t!("view.exit.title"),
                height: lines.len() as u16 + 2,
                lines,
                width: 72,
            }
        }
        Overlay::Help { scroll } => {
            let mut ctxs = s.key_contexts_without_overlay();
            ctxs.push(keys::Context::Global);
            let mut lines = vec![];
            for ctx in ctxs {
                lines.push(Line::toned(ctx.label(), Tone::Strong));
                // 同一个动作的几个键排在一行（`^H / F1`）；兼容键不列。
                let mut rows: Vec<(keys::Action, Vec<String>)> = vec![];
                for b in s.keymap.bindings().iter().filter(|b| b.context == ctx) {
                    if b.keys.iter().any(|k| k.is_compat()) {
                        continue;
                    }
                    let key = keys::chord_label(&b.keys);
                    match rows.iter_mut().find(|(a, _)| *a == b.action) {
                        Some((_, ks)) => ks.push(key),
                        None => rows.push((b.action, vec![key])),
                    }
                }
                for (action, ks) in rows {
                    let key = ks.join(" / ");
                    let pad = 14usize.saturating_sub(tui::display_width(&key));
                    lines.push(
                        Line::toned(format!("  {key}{}", " ".repeat(pad)), Tone::Accent)
                            .push(action.help(), Tone::Normal),
                    );
                }
                lines.push(Line::default());
            }
            lines.push(Line::toned(i18n::t!("view.help.mouse"), Tone::Dim));
            let height = (lines.len() as u16 + 2).min(s.ui.size.rows.saturating_sub(4));
            let body = height.saturating_sub(2) as usize;
            let max_scroll = lines.len().saturating_sub(body);
            tui::Overlay {
                title: i18n::t!("view.help.title"),
                lines: lines.into_iter().skip((*scroll).min(max_scroll)).collect(),
                width: 72,
                height,
            }
        }
    }
}

impl State {
    /// 帮助浮层要列的是打开浮层之前那个焦点的键位。
    fn key_contexts_without_overlay(&self) -> Vec<keys::Context> {
        match self.ui.focus {
            Focus::Composer => vec![keys::Context::Composer],
            Focus::Transcript => vec![keys::Context::Transcript],
            _ => vec![self.ui.tab.context()],
        }
    }
}

/// `3h12m`、`6d`、`41m`、`12s` 这种给人看的时长。
pub(crate) fn fmt_duration(ms: u64) -> String {
    let s = ms / 1000;
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h{}m", s / 3600, s % 3600 / 60),
        _ => format!("{}d", s / 86_400),
    }
}

fn pad(text: &str, width: usize) -> String {
    let t = truncate_to_width(text, width);
    let w = tui::display_width(&t);
    format!("{t}{}", " ".repeat(width.saturating_sub(w)))
}

fn picker(s: &State, p: &Picker) -> tui::InstancesScreen {
    let local = i18n::t!("view.picker.local");
    let host = p.list.first().map(|i| i.host.as_str()).unwrap_or(&local);
    let now = s.ui.now_ms;
    let mut all: Vec<Line> = vec![Line::toned(i18n::t!("view.picker.new"), Tone::Accent)];
    all.extend(p.list.iter().map(|i| {
        let (glyph, tone) = match (i.state, i.holder.is_some()) {
            (port::InstanceState::Running, true) => ("🔒", Tone::Warn),
            (port::InstanceState::Running, false) => ("● ", Tone::Ok),
            (port::InstanceState::Stopped, _) => ("○ ", Tone::Dim),
            (port::InstanceState::Stale, _) => ("✗ ", Tone::Error),
        };
        let project = i.project.as_deref().unwrap_or("");
        let mut line = Line::toned(format!("{glyph} "), tone)
            .push(pad(i.name.as_str(), 10), Tone::Strong)
            .push(pad(project, 14), Tone::Dim);
        match i.state {
            port::InstanceState::Running => {
                let up = i.started_ms.map(|t| fmt_duration(now.saturating_sub(t)));
                line = line
                    .push(
                        pad(
                            &i18n::t!("view.picker.uptime", up = up.unwrap_or_default()),
                            11,
                        ),
                        Tone::Normal,
                    )
                    .push(
                        pad(&i18n::t!("view.picker.sessions", count = i.sessions), 9),
                        Tone::Normal,
                    );
                if i.waiting > 0 {
                    line = line.push(
                        pad(&i18n::t!("view.picker.waiting", count = i.waiting), 10),
                        Tone::Attention,
                    );
                }
                if let Some(h) = &i.holder {
                    line = line.push(
                        i18n::t!(
                            "view.picker.holder",
                            who = h.label(),
                            idle = fmt_duration(now.saturating_sub(h.last_input_ms))
                        ),
                        Tone::Warn,
                    );
                }
            }
            port::InstanceState::Stopped => {
                line = line.push(i18n::t!("view.picker.stopped"), Tone::Dim)
            }
            port::InstanceState::Stale => {
                line = line.push(i18n::t!("view.picker.stale"), Tone::Error)
            }
        }
        line
    }));
    let n = crate::state::PICKER_ROWS;
    let scroll = p.scroll.min(all.len().saturating_sub(n));
    let message = if p.loading && p.list.is_empty() {
        Some(Line::toned(i18n::t!("view.picker.listing"), Tone::Dim))
    } else {
        p.message
            .as_ref()
            .map(|m| Line::toned(m.clone(), Tone::Warn))
    };
    tui::InstancesScreen {
        title: Line::plain(i18n::t!("view.picker.title", host = host)),
        more_above: scroll > 0,
        more_below: scroll + n < all.len(),
        selected: p.selected.checked_sub(scroll).filter(|i| *i < n),
        rows: all.into_iter().skip(scroll).take(n).collect(),
        message,
    }
}

fn detached(s: &State, r: &DetachReason) -> Vec<Line> {
    let name = s.ui.attached.clone().unwrap_or_default();
    match r {
        DetachReason::Kicked { by, at_ms } => vec![
            Line::toned(i18n::t!("view.detached.title", name = name), Tone::Strong),
            Line::default(),
            Line::toned(
                i18n::t!(
                    "view.detached.kicked",
                    by = by.label(),
                    ago = fmt_duration(s.ui.now_ms.saturating_sub(*at_ms))
                ),
                Tone::Warn,
            ),
            Line::toned(i18n::t!("view.detached.still_running"), Tone::Dim),
            Line::default(),
            Line::toned(i18n::t!("view.detached.kicked_keys"), Tone::Dim),
        ],
        DetachReason::Idle { after_ms } => vec![
            Line::toned(i18n::t!("view.detached.title", name = name), Tone::Strong),
            Line::default(),
            Line::toned(
                i18n::t!("view.detached.idle", idle = fmt_duration(*after_ms)),
                Tone::Warn,
            ),
            Line::default(),
            Line::toned(i18n::t!("view.detached.idle_keys"), Tone::Dim),
        ],
        DetachReason::Stopped { reason } => vec![
            Line::toned(i18n::t!("view.detached.stopped", name = name), Tone::Strong),
            Line::default(),
            Line::toned(reason.clone(), Tone::Warn),
            Line::default(),
            Line::toned(i18n::t!("view.detached.kicked_keys"), Tone::Dim),
        ],
    }
}

impl State {
    /// 输入框的字超出了框：（总行数，框里看得见的行数）。
    pub fn composer_overflow(&self) -> Option<(usize, usize)> {
        let c = self.chat_regions().chat?;
        let rows = c.composer_inner.h as usize;
        let n = self
            .ui
            .composer
            .layout(c.composer_inner.w as usize, &i18n::t!("view.placeholder"))
            .0
            .len();
        (rows > 0 && n > rows).then_some((n, rows))
    }

    fn composer_key(&self) -> (usize, usize) {
        (self.ui.composer.cursor, self.ui.composer.text.len())
    }

    /// 输入框窗口的第一行：手动翻过、之后没再编辑就照手动的，否则让光标所在行看得见。
    pub(crate) fn composer_offset(&self, len: usize, rows: usize) -> usize {
        let max = len.saturating_sub(rows);
        if let Some((key, off)) = self.ui.composer_scroll
            && key == self.composer_key()
        {
            return off.min(max);
        }
        let w = self
            .chat_regions()
            .chat
            .map_or(80, |c| c.composer_inner.w as usize);
        let row = self
            .ui
            .composer
            .layout(w, &i18n::t!("view.placeholder"))
            .1
            .1 as usize;
        (row + 1).saturating_sub(rows).min(max)
    }

    pub(crate) fn scroll_composer(&mut self, lines: isize) {
        let Some((len, rows)) = self.composer_overflow() else {
            return;
        };
        let cur = self.composer_offset(len, rows) as isize;
        let off = (cur + lines).clamp(0, len.saturating_sub(rows) as isize) as usize;
        self.ui.composer_scroll = Some((self.composer_key(), off));
    }
}

/// 输入框下右边那句：权限模式与切换键。缺省模式不写（与 Claude Code 一致），这里给出也只用于提示。
pub(crate) fn permission_label(m: port::PermissionMode) -> String {
    match m {
        port::PermissionMode::Default => i18n::t!("view.mode.default"),
        port::PermissionMode::AcceptEdits => i18n::t!("view.mode.accept_edits"),
        port::PermissionMode::Plan => i18n::t!("view.mode.plan"),
        port::PermissionMode::BypassPermissions => i18n::t!("view.mode.bypass"),
        port::PermissionMode::DontAsk => i18n::t!("view.mode.dont_ask"),
    }
}
