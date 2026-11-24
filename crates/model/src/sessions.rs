//! 会话标签（需求 S，交互 §5.2）：上半是主 Agent 列表（展开后是 goal 与 todo），下半是选中
//! 主 Agent 的拆分树（子 Agent 递归、任务；已结束的收在一组里）。选中谁就 Watch 谁，goal、
//! todo、任务都来自它的快照与之后的增量。

use port::{
    GoalPhase, Origin, SessionId, SessionState, SessionSummary, Task, TaskId, TaskKind, TaskStatus,
    TodoStatus,
};
use tui::{Line, Tone, display_width, truncate_to_width};

use crate::chat::fmt_k;
use crate::state::State;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessRow {
    /// 第一行：新建会话（也可以在输入框里 /new）。
    New,
    Session(SessionId),
    Goal(SessionId),
    Todo(SessionId),
    TodoItem(SessionId, usize),
    /// 「已关闭 (n)」。
    Closed,
    /// 已关闭的会话：选中只读地看，`o` 重新打开，`d` 删除。
    ClosedSession(SessionId),
}

impl SessRow {
    pub fn session(&self) -> Option<&SessionId> {
        match self {
            SessRow::New | SessRow::Closed => None,
            SessRow::Session(s)
            | SessRow::Goal(s)
            | SessRow::Todo(s)
            | SessRow::TodoItem(s, _)
            | SessRow::ClosedSession(s) => Some(s),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRow {
    /// 主 Agent 自己。
    Root(SessionId),
    Task {
        id: TaskId,
        prefix: String,
    },
    /// "已结束 (n)"：某个会话下已经结束的任务收成一组。
    Finished {
        owner: SessionId,
        count: usize,
        prefix: String,
    },
}

impl State {
    pub fn session_rows(&self) -> Vec<SessRow> {
        let filter = &self.ui.filter[0];
        let mut out = if filter.is_empty() {
            vec![SessRow::New]
        } else {
            vec![]
        };
        for s in self.user_sessions() {
            if !filter.is_empty() && !s.title.contains(filter.as_str()) {
                continue;
            }
            out.push(SessRow::Session(s.id.clone()));
            if !self.ui.sess_expanded.contains(&s.id) {
                continue;
            }
            let Some(c) = self.domain.convs.get(&s.id) else {
                continue;
            };
            if c.goal.is_some() {
                out.push(SessRow::Goal(s.id.clone()));
            }
            if !c.todos.is_empty() {
                out.push(SessRow::Todo(s.id.clone()));
                if self.ui.todo_expanded.contains(&s.id) {
                    out.extend(
                        crate::marks::todo_order(&c.todos)
                            .into_iter()
                            .map(|i| SessRow::TodoItem(s.id.clone(), i)),
                    );
                }
            }
        }
        let closed: Vec<&SessionSummary> = self
            .domain
            .closed_sessions
            .iter()
            .filter(|s| filter.is_empty() || s.title.contains(filter.as_str()))
            .collect();
        if !closed.is_empty() {
            out.push(SessRow::Closed);
            if self.ui.closed_expanded {
                out.extend(closed.iter().map(|s| SessRow::ClosedSession(s.id.clone())));
            }
        }
        out
    }

    /// 对话区里的这个会话已经关了：只读（交互 §5.2）。
    pub(crate) fn chat_is_closed(&self) -> bool {
        self.ui
            .chat
            .as_ref()
            .is_some_and(|id| self.domain.closed_sessions.iter().any(|s| &s.id == id))
    }

    /// 右栏上半选中的主 Agent。
    pub fn selected_session(&self) -> Option<SessionId> {
        self.session_rows()
            .get(self.ui.upper[0].selected)
            .and_then(|r| r.session().cloned())
    }

    pub fn tree_rows(&self) -> Vec<TreeRow> {
        let Some(root) = self.selected_session() else {
            return vec![];
        };
        let Some(c) = self.domain.convs.get(&root) else {
            return vec![TreeRow::Root(root)];
        };
        let mut out = vec![TreeRow::Root(root.clone())];
        self.tree_under(&c.tasks, &root, "", &mut out, 0);
        out
    }

    fn tree_under(
        &self,
        tasks: &[Task],
        owner: &SessionId,
        prefix: &str,
        out: &mut Vec<TreeRow>,
        depth: u8,
    ) {
        if depth > 8 {
            return;
        }
        let mut live: Vec<&Task> = tasks
            .iter()
            .filter(|t| &t.owner == owner && !t.status.finished())
            .collect();
        let done: Vec<&Task> = tasks
            .iter()
            .filter(|t| &t.owner == owner && t.status.finished())
            .collect();
        live.sort_by_key(|t| (t.status != TaskStatus::Waiting, t.started_ms));
        let show_done = self.ui.finished_expanded.contains(owner);
        let mut items: Vec<Option<&Task>> = live.into_iter().map(Some).collect();
        if !done.is_empty() {
            if show_done {
                items.extend(done.iter().map(|t| Some(*t)));
            } else {
                items.push(None);
            }
        }
        let n = items.len();
        for (i, item) in items.into_iter().enumerate() {
            let last = i + 1 == n;
            let branch = if last { "└ " } else { "├ " };
            let next = format!("{prefix}{}", if last { "  " } else { "│ " });
            match item {
                Some(t) => {
                    out.push(TreeRow::Task {
                        id: t.id.clone(),
                        prefix: format!("{prefix}{branch}"),
                    });
                    if let Some(child) = &t.agent_session
                        && !self.ui.tree_collapsed.contains(&t.id)
                    {
                        self.tree_under(tasks, child, &next, out, depth + 1);
                    }
                }
                None => out.push(TreeRow::Finished {
                    owner: owner.clone(),
                    count: done.len(),
                    prefix: format!("{prefix}{branch}"),
                }),
            }
        }
    }

    /// 流程、作业页的树：`owner` 派出的任务，子 Agent 的接着往下（来自 `domain.tasks`，不要求 Watch）。
    /// 在跑的在前；`skip` 里的会话（站的任务已经单列了它们）不重复列。
    pub(crate) fn work_tasks(
        &self,
        owner: &SessionId,
        prefix: &str,
        skip: &[SessionId],
        out: &mut Vec<(TaskId, String)>,
    ) {
        self.work_tasks_at(owner, prefix, skip, out, 0);
    }

    fn work_tasks_at(
        &self,
        owner: &SessionId,
        prefix: &str,
        skip: &[SessionId],
        out: &mut Vec<(TaskId, String)>,
        depth: u8,
    ) {
        if depth > 8 {
            return;
        }
        let mut mine: Vec<&Task> = self
            .domain
            .tasks
            .values()
            .filter(|t| &t.owner == owner)
            .filter(|t| t.agent_session.as_ref().is_none_or(|c| !skip.contains(c)))
            .collect();
        mine.sort_by_key(|t| {
            (
                t.status.finished(),
                t.status != TaskStatus::Waiting,
                t.started_ms,
            )
        });
        let n = mine.len();
        for (i, t) in mine.into_iter().enumerate() {
            let last = i + 1 == n;
            out.push((
                t.id.clone(),
                format!("{prefix}{}", if last { "└ " } else { "├ " }),
            ));
            if let Some(child) = &t.agent_session
                && !self.ui.tree_collapsed.contains(&t.id)
            {
                let next = format!("{prefix}{}", if last { "  " } else { "│ " });
                self.work_tasks_at(child, &next, skip, out, depth + 1);
            }
        }
    }

    /// 选中主 Agent 的整棵任务树（测试与视图用）。
    pub fn conv_tasks(&self) -> Vec<Task> {
        self.selected_session()
            .and_then(|r| self.domain.convs.get(&r))
            .map(|c| c.tasks.clone())
            .unwrap_or_default()
    }

    pub(crate) fn task(&self, id: &TaskId) -> Option<&Task> {
        let root = self.selected_session()?;
        self.domain
            .convs
            .get(&root)?
            .tasks
            .iter()
            .find(|t| &t.id == id)
    }
}

pub(crate) fn state_glyph(s: SessionState) -> (&'static str, Tone) {
    crate::marks::session(s).pair()
}

pub(crate) fn task_glyph(t: TaskStatus) -> (&'static str, Tone) {
    crate::marks::task(t).pair()
}

pub(crate) fn origin_icon(o: &Origin) -> &'static str {
    match o {
        Origin::User | Origin::SubAgent { .. } => "",
        Origin::Job { .. } => "⏰",
        Origin::FlowStation { .. } => "⇶",
        Origin::Fork { .. } => "⑂",
    }
}

pub fn fmt_elapsed(ms: u64) -> String {
    let s = ms / 1000;
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m{:02}s", s / 60, s % 60)
    } else {
        format!("{}h{:02}m", s / 3600, s % 3600 / 60)
    }
}

/// 左边是内容，右边是靠右对齐的摘要；放不下时先截左边。
pub(crate) fn split_line(left: Line, right: Line, width: usize) -> Line {
    let rw = right.width();
    let room = width.saturating_sub(rw + 1);
    let mut l = Line::default();
    let mut used = 0;
    for sp in left.spans {
        let w = display_width(&sp.text);
        if used + w <= room {
            used += w;
            l = l.push(sp.text, sp.tone);
        } else {
            let t = truncate_to_width(&sp.text, room - used);
            used += display_width(&t);
            l = l.push(t, sp.tone);
            break;
        }
    }
    l = l.push(" ".repeat(width.saturating_sub(used + rw)), Tone::Normal);
    for sp in right.spans {
        l = l.push(sp.text, sp.tone);
    }
    l
}

/// `#n `：对象在规范顺序里的位置（`port::order`），淡色，按总数补齐好让名字对齐。
pub(crate) fn number(n: usize, total: usize) -> String {
    let w = total.max(1).to_string().len();
    format!("#{n:<w$} ")
}

pub(crate) fn upper_lines(s: &State, width: usize) -> Vec<Line> {
    let rows = s.session_rows();
    let order: Vec<SessionId> = s.user_sessions().iter().map(|x| x.id.clone()).collect();
    let numbered = |id: &SessionId| {
        let n = order.iter().position(|x| x == id).map_or(0, |i| i + 1);
        number(n, order.len())
    };
    rows.iter()
        .map(|r| match r {
            SessRow::New => Line::toned(i18n::t!("sessions.new"), Tone::Accent)
                .push(i18n::t!("sessions.new_hint"), Tone::Dim),
            SessRow::Session(id) => {
                let Some(x) = s.domain.sessions.iter().find(|x| &x.id == id) else {
                    return Line::default();
                };
                session_line(s, x, numbered(id), width)
            }
            SessRow::Closed => {
                let mark = if s.ui.closed_expanded { "▾" } else { "▸" };
                Line::toned(format!("{mark} "), Tone::Dim).push(
                    i18n::t!("sessions.closed", n = s.domain.closed_sessions.len()),
                    Tone::Normal,
                )
            }
            SessRow::ClosedSession(id) => {
                let Some(x) = s.domain.closed_sessions.iter().find(|x| &x.id == id) else {
                    return Line::default();
                };
                split_line(
                    Line::toned("    ", Tone::Dim).push(x.title.clone(), Tone::Dim),
                    Line::toned(crate::chat::fmt_k(x.usage.total()), Tone::Dim),
                    width,
                )
            }
            SessRow::Goal(id) => {
                let Some(g) = s.domain.convs.get(id).and_then(|c| c.goal.as_ref()) else {
                    return Line::default();
                };
                let (glyph, tone, tail) = match g.phase {
                    GoalPhase::Active => (
                        "●",
                        Tone::Ok,
                        i18n::t!(
                            "sessions.goal_round",
                            n = g.rounds_started,
                            max = g.max_rounds
                        ),
                    ),
                    GoalPhase::Paused => ("⏸", Tone::Dim, i18n::t!("sessions.goal_paused")),
                    GoalPhase::Blocked => (
                        "◐",
                        Tone::Attention,
                        i18n::t!(
                            "sessions.goal_blocked",
                            reason = g.blocked_reason.as_deref().unwrap_or("")
                        ),
                    ),
                    GoalPhase::Complete => ("✓", Tone::Dim, i18n::t!("sessions.goal_complete")),
                };
                let line = Line::toned("     goal ", Tone::Dim)
                    .push(format!("{glyph} "), tone)
                    .push(g.objective.clone(), Tone::Normal)
                    .push(
                        format!(" · {tail}"),
                        if g.phase == GoalPhase::Blocked {
                            Tone::Attention
                        } else {
                            Tone::Dim
                        },
                    );
                trim(line, width)
            }
            SessRow::Todo(id) => {
                let Some(c) = s.domain.convs.get(id) else {
                    return Line::default();
                };
                let done = c
                    .todos
                    .iter()
                    .filter(|t| t.status == TodoStatus::Done)
                    .count();
                let mark = if s.ui.todo_expanded.contains(id) {
                    "▾"
                } else {
                    "▸"
                };
                let mut line = Line::toned(
                    format!("     todo {mark} {done}/{}  ", c.todos.len()),
                    Tone::Dim,
                );
                for t in crate::marks::todo_order(&c.todos)
                    .into_iter()
                    .take(4)
                    .map(|i| &c.todos[i])
                {
                    let (g, tone) = todo_glyph(t.status);
                    line = line
                        .push(format!("{g} "), tone)
                        .push(format!("{}  ", t.text), Tone::Normal);
                }
                trim(line, width)
            }
            SessRow::TodoItem(id, i) => {
                let Some(t) = s.domain.convs.get(id).and_then(|c| c.todos.get(*i)) else {
                    return Line::default();
                };
                let (g, tone) = todo_glyph(t.status);
                let text_tone = if t.status == TodoStatus::Done {
                    Tone::Dim
                } else {
                    Tone::Normal
                };
                trim(
                    Line::toned("         ", Tone::Normal)
                        .push(format!("{g} "), tone)
                        .push(format!("{}. {}", i + 1, t.text), text_tone),
                    width,
                )
            }
        })
        .collect()
}

fn todo_glyph(t: TodoStatus) -> (&'static str, Tone) {
    crate::marks::todo(t)
}

fn trim(line: Line, width: usize) -> Line {
    split_line(line, Line::default(), width)
}

fn session_line(s: &State, x: &SessionSummary, num: String, width: usize) -> Line {
    let (g, tone) = state_glyph(x.state);
    let mark = if s.ui.sess_expanded.contains(&x.id) {
        "▾"
    } else {
        "▸"
    };
    let left = Line::toned(format!("{mark} "), Tone::Dim)
        .push(num, Tone::Dim)
        .push(format!("{g} "), tone)
        .push(x.title.clone(), Tone::Normal);
    let icon = origin_icon(&x.provenance.origin);
    let right = Line::toned(
        // 会话都是这个项目的（实例绑定项目目录），不再逐行写项目。
        format!(
            "{}  {:>5}{}",
            x.provenance.scene,
            fmt_k(x.usage.total()),
            if icon.is_empty() {
                "  ".into()
            } else {
                format!(" {icon}")
            }
        ),
        Tone::Dim,
    );
    split_line(left, right, width)
}

pub(crate) fn lower_lines(s: &State, width: usize) -> Vec<Line> {
    s.tree_rows()
        .iter()
        .map(|r| match r {
            TreeRow::Root(id) => {
                let sum = s.domain.sessions.iter().find(|x| &x.id == id);
                let (g, tone) = sum
                    .map(|x| state_glyph(x.state))
                    .unwrap_or(("○", Tone::Dim));
                split_line(
                    Line::toned(format!("{g} "), tone)
                        .push(i18n::t!("sessions.main_agent"), Tone::Strong),
                    Line::toned(crate::marks::cost(0, sum.map(|x| &x.usage)), Tone::Dim),
                    width,
                )
            }
            TreeRow::Finished {
                count,
                prefix,
                owner,
            } => {
                let mark = if s.ui.finished_expanded.contains(owner) {
                    "▾"
                } else {
                    "▸"
                };
                Line::toned(
                    format!(
                        "{prefix}{mark} {}",
                        i18n::t!("sessions.finished", count = count)
                    ),
                    Tone::Dim,
                )
            }
            TreeRow::Task { id, prefix } => match s.task(id) {
                Some(t) => task_line(s, t, prefix, width),
                None => Line::default(),
            },
        })
        .collect()
}

/// 任务树的一行：会话标签的拆分树与流程、作业页的树共用。
pub(crate) fn task_line(s: &State, t: &Task, prefix: &str, width: usize) -> Line {
    let (g, tone) = task_glyph(t.status);
    let mark = match (&t.agent_session, s.ui.tree_collapsed.contains(&t.id)) {
        (Some(_), false) => "▾ ",
        (Some(_), true) => "▸ ",
        (None, _) => "",
    };
    // 子 Agent 写它的 agent 类型：引擎还不报子 Agent 实际用的模型（Agix#98），类型多半就定了模型。
    let kind = match &t.kind {
        TaskKind::Agent => t
            .agent_type
            .clone()
            .unwrap_or_else(|| i18n::t!("sessions.sub_agent")),
        k => crate::marks::kind_label(k),
    };
    let end = t.ended_ms.unwrap_or(s.ui.now_ms);
    // 子 Agent 与任务用图标分开：◆ 是子 Agent（选中切到它的会话），其余是任务
    // （选中看它的输出）。
    let (icon, itone) = crate::marks::kind_icon(&t.kind);
    let left = Line::toned(prefix.to_string(), Tone::Dim)
        .push(mark, Tone::Dim)
        .push(format!("{g} "), tone)
        .push(format!("{icon} "), itone)
        .push(t.label.clone(), Tone::Normal)
        .push(format!("  {kind}"), Tone::Dim);
    let right = Line::toned(
        crate::marks::cost(end.saturating_sub(t.started_ms), Some(&t.usage)),
        Tone::Dim,
    );
    split_line(left, right, width)
}

/// 右下框标题右边的摘要：几件在跑。
pub(crate) fn lower_summary(s: &State) -> Option<String> {
    let root = s.selected_session()?;
    let c = s.domain.convs.get(&root)?;
    let running = c.tasks.iter().filter(|t| !t.status.finished()).count();
    (running > 0).then(|| i18n::t!("sessions.running", n = running))
}

use keys::Action;
use port::{Command, GoalAction};

use crate::state::{Confirm, Focus};
use crate::update::{Effect, Pending};

/// 会话标签里的按键（交互 §5.2）。返回 `None` 表示没处理，交给通用的列表键位。
pub(crate) fn action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    match s.ui.focus {
        Focus::SideUpper => upper_action(s, a),
        Focus::SideLower => lower_action(s, a),
        _ => None,
    }
}

fn upper_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    if a == Action::Rename && s.domain.load_errors[0].is_some() {
        s.domain.load_errors[0] = None;
        return Some(vec![s.call(
            Pending::Sessions,
            Command::ListSessions {
                filter: port::SessionFilter::default(),
            },
        )]);
    }
    let rows = s.session_rows();
    let row = rows.get(s.ui.upper[0].selected).cloned();
    let id = row.as_ref().and_then(|r| r.session().cloned());
    Some(match (a, row) {
        (Action::Expand | Action::Open, Some(SessRow::Closed)) if !s.ui.closed_expanded => {
            s.ui.closed_expanded = true;
            vec![]
        }
        (Action::Collapse | Action::Open, Some(SessRow::Closed)) => {
            s.ui.closed_expanded = false;
            vec![]
        }
        (Action::Collapse, Some(SessRow::ClosedSession(_))) => {
            s.ui.closed_expanded = false;
            s.ui.upper[0].selected = s
                .session_rows()
                .iter()
                .position(|x| x == &SessRow::Closed)
                .unwrap_or(0);
            vec![]
        }
        (Action::Reopen, Some(SessRow::ClosedSession(id))) => {
            vec![s.call(Pending::Ignore, Command::ReopenSession { session: id })]
        }
        (Action::Delete, Some(SessRow::ClosedSession(id))) => {
            let title = s
                .domain
                .closed_sessions
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.title.clone())
                .unwrap_or_default();
            s.ui.confirm = Some(Confirm::DeleteSession(id, title));
            vec![]
        }
        // 已关闭的会话只读：不进输入框、不改名。
        (Action::Open | Action::Rename, Some(SessRow::ClosedSession(_))) => {
            s.notice(i18n::t!("sessions.closed_readonly"), false);
            vec![]
        }
        (Action::Open, Some(SessRow::New)) | (Action::New, _) => {
            crate::follow::go_draft(s);
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        (Action::Expand, Some(SessRow::Session(id))) => {
            s.ui.sess_expanded.insert(id);
            vec![]
        }
        (Action::Expand | Action::Open, Some(SessRow::Todo(id))) => {
            if !s.ui.todo_expanded.remove(&id) {
                s.ui.todo_expanded.insert(id);
            }
            vec![]
        }
        (Action::Collapse, Some(r)) if r.session().is_some() => {
            let id = r.session().cloned()?;
            s.ui.todo_expanded.remove(&id);
            s.ui.sess_expanded.remove(&id);
            s.ui.upper[0].selected = s
                .session_rows()
                .iter()
                .position(|x| x == &SessRow::Session(id.clone()))
                .unwrap_or(0);
            vec![]
        }
        (Action::Open, Some(SessRow::Session(id))) => {
            s.ui.ctx_parents.clear();
            s.ui.via_task = None;
            s.ui.task_view = None;
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            crate::chat_update::open_session(s, id)
        }
        (Action::Open, Some(SessRow::Goal(_))) => {
            s.notice(i18n::t!("sessions.notice_goal"), false);
            vec![]
        }
        (Action::PauseResume, Some(SessRow::Goal(id))) => {
            let phase = s
                .domain
                .convs
                .get(&id)
                .and_then(|c| c.goal.as_ref())
                .map(|g| g.phase);
            let action = match phase {
                Some(GoalPhase::Paused) | Some(GoalPhase::Blocked) => GoalAction::Resume,
                _ => GoalAction::Pause,
            };
            vec![s.call(
                Pending::Ignore,
                Command::UpdateGoal {
                    session: id,
                    action,
                },
            )]
        }
        (Action::Close, Some(SessRow::Goal(id))) => {
            s.ui.confirm = Some(Confirm::ClearGoal(id));
            vec![]
        }
        (Action::Close, Some(SessRow::Session(id))) => {
            let title = s
                .domain
                .sessions
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.title.clone())
                .unwrap_or_default();
            s.ui.confirm = Some(Confirm::CloseSession(id, title));
            vec![]
        }
        (Action::Rename, Some(_)) => {
            let id = id?;
            let title = s
                .domain
                .sessions
                .iter()
                .find(|x| x.id == id)
                .map(|x| x.title.clone())
                .unwrap_or_default();
            s.ui.rename = Some(id);
            s.ui.composer.clear();
            s.ui.composer.insert_str(&title);
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            s.notice(i18n::t!("sessions.notice_rename"), false);
            vec![]
        }
        _ => return None,
    })
}

fn lower_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let rows = s.tree_rows();
    let row = rows.get(s.ui.lower[0].selected).cloned()?;
    Some(match (a, row) {
        (Action::Expand | Action::Collapse | Action::Open, TreeRow::Finished { owner, .. }) => {
            if !s.ui.finished_expanded.remove(&owner) {
                s.ui.finished_expanded.insert(owner);
            }
            vec![]
        }
        (Action::Collapse, TreeRow::Task { id, .. }) => {
            s.ui.tree_collapsed.insert(id);
            vec![]
        }
        (Action::Expand, TreeRow::Task { id, .. }) => {
            s.ui.tree_collapsed.remove(&id);
            vec![]
        }
        (Action::Open, TreeRow::Root(id)) => {
            s.ui.ctx_parents.clear();
            s.ui.via_task = None;
            s.ui.task_view = None;
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            crate::chat_update::open_session(s, id)
        }
        (Action::Open | Action::Message, TreeRow::Task { id, .. }) => {
            let t = s.task(&id)?.clone();
            let root = s.selected_session()?;
            let root_title = s
                .domain
                .sessions
                .iter()
                .find(|x| x.id == root)
                .map(|x| x.title.clone())
                .unwrap_or_default();
            s.ui.ctx_parents = vec![(root.clone(), root_title)];
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            match &t.agent_session {
                Some(child) => crate::chat_update::open_sidechain(s, child.clone(), t.id.clone()),
                None if a == Action::Message => {
                    s.notice(i18n::t!("sessions.notice_sub_only"), true);
                    vec![]
                }
                None => crate::chat_update::open_task_output(s, root, t.id.clone()),
            }
        }
        (Action::Stop, TreeRow::Task { id, .. }) => {
            let t = s.task(&id)?;
            if t.status.finished() {
                s.notice(i18n::t!("sessions.notice_ended"), false);
                return Some(vec![]);
            }
            s.ui.confirm = Some(Confirm::StopTask(id.clone(), t.label.clone()));
            vec![]
        }
        _ => return None,
    })
}

/// 每次 update 之后：右栏会话标签选中的主 Agent 没 Watch 过就 Watch 它，好有 goal、todo、任务树。
pub(crate) fn ensure_watch(s: &mut State) -> Vec<Effect> {
    if s.ui.tab != crate::state::Tab::Sessions || s.ui.screen != crate::state::Screen::Workbench {
        return vec![];
    }
    let mut ids: Vec<SessionId> = s.selected_session().into_iter().collect();
    ids.extend(s.ui.sess_expanded.iter().cloned());
    let mut out = vec![];
    for id in ids {
        if s.domain.convs.contains_key(&id) || s.ui.watching.contains(&id) {
            continue;
        }
        s.ui.watching.insert(id.clone());
        out.push(s.call(Pending::Watch(id.clone()), Command::Watch { session: id }));
    }
    out
}
