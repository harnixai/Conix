//! 流程标签（需求 F，交互 §5.3）：上半是流程定义（选中即展开它没结束的运行），末尾是不分流程的
//! 「已结束」；下半是选中运行的整棵树：主 Agent、各站（不缩进，按进展上色）、站的任务，每个有会话的
//! 节点下面再挂它的子 Agent 与任务；选中流程定义时下半是流程设计。
//! 键只管界面：流程与运行的操作都回到用户会话里说（`n` 新建、`m` 说选中的这一项，放话头）；
//! 选中有会话的节点 → 左边切到那个会话，运行里面的事在那里处理。

use keys::Action;
use port::{Command, RunId, RunStatus, RunSummary, RunView, SessionId, StationId, StationStatus};
use tui::{Line, Tone};

use crate::chat::fmt_k;
use crate::marks;
use crate::sessions::{fmt_elapsed, task_glyph};
use crate::state::{Focus, State, Tab};
use crate::update::{Effect, Pending};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowRow {
    Flow(String),
    /// 运行：在流程下面的是没结束的，在「已结束」下面的是结束了的。同一个运行结束时从前者挪到
    /// 后者，行的身份不变，选中跟着它走。
    Run(String, RunId),
    /// 「已结束 (n)」。
    Ended,
    /// 「已结束」末尾的「更多」：再取一页。
    More,
}

/// 完成与取消的运行在「已结束」下面，不在它的流程下面。
pub(crate) fn is_ended(s: &RunStatus) -> bool {
    matches!(s, RunStatus::Finished | RunStatus::Cancelled)
}

/// 「已结束」一次取多少条。
pub(crate) const ENDED_PAGE: u32 = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowLower {
    /// 运行的主 Agent。
    Main,
    /// 站里的会话提的、在等人的建议（`RunView::proposals` 里在等的第 n 条）。
    Proposal(usize),
    Station(StationId),
    /// 站里的一件任务（子 Agent 有会话）。
    StationTask(StationId, usize),
    /// 某个会话派出的任务（子 Agent、Shell……），带它在树里的前缀。
    Task(port::TaskId, String),
    /// 选中流程定义时：流程设计的节点图的一行（只看）。
    Design(usize),
    /// 选中流程定义时：定义里的一站（Enter / `v` 看它的配置）。
    DefStation(usize),
}

impl State {
    pub fn flow_rows(&self) -> Vec<FlowRow> {
        let filter = &self.ui.filter[Tab::Flows.index()];
        let mut out = vec![];
        for f in port::order::flows(&self.domain.flows) {
            if !filter.is_empty()
                && !f.title.contains(filter.as_str())
                && !f.name.contains(filter.as_str())
            {
                continue;
            }
            out.push(FlowRow::Flow(f.name.clone()));
            if self.ui.flow_expanded.contains(&f.name)
                && let Some(runs) = self.domain.runs.get(&f.name)
            {
                out.extend(
                    runs.iter()
                        .filter(|r| !is_ended(&r.status))
                        .map(|r| FlowRow::Run(f.name.clone(), r.id.clone())),
                );
            }
        }
        let ended: Vec<&RunSummary> = self
            .domain
            .ended_runs
            .iter()
            .filter(|r| {
                filter.is_empty()
                    || r.subject.contains(filter.as_str())
                    || r.flow.contains(filter.as_str())
                    || self.flow_title(&r.flow).contains(filter.as_str())
            })
            .collect();
        if !ended.is_empty() {
            out.push(FlowRow::Ended);
            if self.ui.ended_expanded {
                out.extend(
                    ended
                        .iter()
                        .map(|r| FlowRow::Run(r.flow.clone(), r.id.clone())),
                );
                if self.domain.ended_more {
                    out.push(FlowRow::More);
                }
            }
        }
        out
    }

    pub fn selected_flow_row(&self) -> Option<FlowRow> {
        self.flow_rows()
            .get(self.ui.upper[Tab::Flows.index()].selected)
            .cloned()
    }

    pub fn selected_run(&self) -> Option<&RunView> {
        match self.selected_flow_row()? {
            FlowRow::Run(_, id) => self.domain.run_views.get(&id),
            _ => None,
        }
    }

    pub fn flow_lower_rows(&self) -> Vec<FlowLower> {
        match self.selected_flow_row() {
            Some(FlowRow::Run(..)) => {
                let Some(v) = self.selected_run() else {
                    return vec![];
                };
                // 站的任务单列了的会话，在主会话的任务下面不再列一遍。
                let listed: Vec<SessionId> = v
                    .stations
                    .iter()
                    .flat_map(|st| {
                        st.session
                            .iter()
                            .cloned()
                            .chain(st.tasks.iter().filter_map(|t| t.session.clone()))
                    })
                    .collect();
                let tasks = |owner: &SessionId, prefix: &str, skip: &[SessionId]| {
                    let mut t = vec![];
                    self.work_tasks(owner, prefix, skip, &mut t);
                    t.into_iter().map(|(id, p)| FlowLower::Task(id, p))
                };
                let mut out = vec![FlowLower::Main];
                out.extend(
                    open_proposals(v)
                        .into_iter()
                        .map(|(i, _)| FlowLower::Proposal(i)),
                );
                out.extend(tasks(&v.main_session, "  ", &listed));
                for st in &v.stations {
                    out.push(FlowLower::Station(st.id.clone()));
                    if self.station_open(&v.summary.id, &st.id) {
                        if let Some(sid) = &st.session {
                            out.extend(tasks(sid, "    ", &[]));
                        }
                        for (i, t) in st.tasks.iter().enumerate() {
                            out.push(FlowLower::StationTask(st.id.clone(), i));
                            if let Some(sid) = &t.session {
                                out.extend(tasks(sid, "      ", &[]));
                            }
                        }
                    }
                }
                out
            }
            Some(FlowRow::Flow(name)) => self
                .domain
                .flow_details
                .get(&name)
                .map(|d| {
                    let graph = crate::panel::flow_graph(d, self.side_lower_width());
                    (0..graph.len())
                        .map(FlowLower::Design)
                        .chain((0..d.stations.len()).map(FlowLower::DefStation))
                        .collect()
                })
                .unwrap_or_default(),
            Some(FlowRow::Ended | FlowRow::More) | None => vec![],
        }
    }

    /// 一个会话用的模型（流程、作业页的树上写在节点后面）。
    pub(crate) fn session_model(&self, id: &SessionId) -> Option<String> {
        self.domain
            .sessions
            .iter()
            .find(|x| &x.id == id)
            .map(|x| x.provenance.model.clone())
            .filter(|m| !m.is_empty())
    }

    /// 有子 Agent 的站缺省展开，→ / ← 收放。
    pub(crate) fn station_open(&self, run: &RunId, station: &StationId) -> bool {
        !self
            .ui
            .station_collapsed
            .contains(&format!("{run}/{station}"))
    }

    /// 右下一行能放多宽（第一列是选中标记）：流程设计的节点图按它折行。
    pub(crate) fn side_lower_width(&self) -> usize {
        let r = tui::layout::compute(
            self.ui.size,
            &crate::view::layout_spec(self),
            &crate::view::tab_labels(self),
        );
        r.side.map_or(60, |x| x.lower.w as usize).saturating_sub(1)
    }

    /// 流程在规范顺序里的序号（`#n`，从 1 起）。
    pub(crate) fn flow_number(&self, name: &str) -> usize {
        port::order::flows(&self.domain.flows)
            .iter()
            .position(|f| f.name == name)
            .map_or(0, |i| i + 1)
    }

    fn flow_title(&self, name: &str) -> String {
        self.domain
            .flows
            .iter()
            .find(|f| f.name == name)
            .map(|f| f.title.clone())
            .unwrap_or_else(|| name.to_string())
    }

    /// 先看它流程下面的，再看「已结束」的：定义没了的运行只在后者里。
    pub(crate) fn run_summary(&self, flow: &str, id: &RunId) -> Option<&RunSummary> {
        self.domain
            .runs
            .get(flow)
            .and_then(|v| v.iter().find(|r| &r.id == id))
            .or_else(|| self.domain.ended_runs.iter().find(|r| &r.id == id))
    }

    /// 选中的是一个已结束的运行。
    pub(crate) fn selected_ended_run(&self) -> Option<RunId> {
        match self.selected_flow_row()? {
            FlowRow::Run(flow, id) => self
                .run_summary(&flow, &id)
                .filter(|r| is_ended(&r.status))
                .map(|_| id),
            _ => None,
        }
    }
}

/// 在等人的建议：原序号与内容。
fn open_proposals(v: &RunView) -> Vec<(usize, &port::ProposalView)> {
    v.proposals
        .iter()
        .enumerate()
        .filter(|(_, p)| p.status == port::ProposalStatus::Open)
        .collect()
}

/// 运行走到哪了：`站 3/7 实现`；结束了的只说几站；步进的运行末尾标出来。各处（列表、下框标题、
/// 面板）同一个写法。
pub(crate) fn progress(r: &RunSummary) -> String {
    let base = progress_base(r);
    if r.pace == port::Pace::Step && !is_ended(&r.status) {
        format!("{base}{}", i18n::t!("flows.pace_step_tag"))
    } else {
        base
    }
}

fn progress_base(r: &RunSummary) -> String {
    match (&r.status, &r.current) {
        (RunStatus::Finished | RunStatus::Cancelled, _) | (_, None) => {
            i18n::t!(
                "flows.progress_done",
                done = r.stations_done.min(r.stations_total),
                total = r.stations_total
            )
        }
        (_, Some(c)) => i18n::t!(
            "flows.progress",
            done = r.stations_done + 1,
            total = r.stations_total,
            current = c
        ),
    }
}

/// 运行的状态：符号、文字、符号的颜色。
pub(crate) fn run_status(s: &RunStatus) -> (&'static str, String, Tone) {
    let text = match s {
        RunStatus::Running => i18n::t!("flows.run.running"),
        RunStatus::Waiting { reason } => i18n::t!("flows.run.waiting", reason = reason),
        RunStatus::Pausing => i18n::t!("flows.run.pausing"),
        RunStatus::Paused => i18n::t!("flows.run.paused"),
        RunStatus::Finished => i18n::t!("flows.run.finished"),
        RunStatus::Cancelled => i18n::t!("flows.run.cancelled"),
    };
    let m = marks::run(s);
    (m.glyph(), text, m.tone())
}

pub(crate) fn station_text(s: StationStatus) -> String {
    match s {
        StationStatus::Pending => i18n::t!("flows.station.pending"),
        StationStatus::Held => i18n::t!("flows.station.held"),
        StationStatus::Working => i18n::t!("flows.station.working"),
        StationStatus::Unmet => i18n::t!("flows.station.unmet"),
        StationStatus::Passed => i18n::t!("flows.station.passed"),
        StationStatus::Skipped => i18n::t!("flows.station.skipped"),
        StationStatus::Stale => i18n::t!("flows.station.stale"),
    }
}

pub(crate) fn upper_lines(s: &State, width: usize) -> Vec<Line> {
    let now = s.ui.now_ms;
    let order: Vec<&str> = port::order::flows(&s.domain.flows)
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    s.flow_rows()
        .iter()
        .map(|r| match r {
            FlowRow::Flow(name) => {
                let Some(f) = s.domain.flows.iter().find(|f| &f.name == name) else {
                    return Line::default();
                };
                let mark = if s.ui.flow_expanded.contains(name) {
                    "▾"
                } else {
                    "▸"
                };
                let mut origin = crate::marks::origin_label(f.origin);
                if let Some(src) = &f.source {
                    origin = format!("{origin} {src}");
                }
                // 全局流程在名字前标 ◎（各项目共用）；项目级的不标，只留出对齐的位置。
                let scope = if f.origin.is_shared() { "◎ " } else { "  " };
                let running = if f.active_runs > 0 {
                    i18n::t!("flows.running", n = f.active_runs)
                } else {
                    String::new()
                };
                let n = order.iter().position(|x| x == name).map_or(0, |i| i + 1);
                crate::sessions::split_line(
                    Line::toned(format!("{mark} "), Tone::Dim)
                        .push(crate::sessions::number(n, order.len()), Tone::Dim)
                        .push(scope, Tone::Accent)
                        .push(f.title.clone(), Tone::Normal),
                    Line::toned(format!("{}  {origin}  {running}", f.name), Tone::Dim),
                    width,
                )
            }
            FlowRow::Ended => {
                let mark = if s.ui.ended_expanded { "▾" } else { "▸" };
                let n = s.domain.ended_runs.len();
                let label = if s.domain.ended_more {
                    i18n::t!("flows.ended_more", n = n)
                } else {
                    i18n::t!("flows.ended", n = n)
                };
                Line::toned(format!("{mark} "), Tone::Dim).push(label, Tone::Normal)
            }
            FlowRow::More => Line::toned(format!("      {}", i18n::t!("flows.more")), Tone::Dim),
            FlowRow::Run(flow, id) => {
                let Some(r) = s.run_summary(flow, id) else {
                    return Line::default();
                };
                let (g, text, tone) = run_status(&r.status);
                if is_ended(&r.status) {
                    // 不分流程平铺，所以写出它是哪个流程的；状态只靠符号（✓ 完成、✗ 取消）。
                    let end = r.ended_ms.unwrap_or(now);
                    return crate::sessions::split_line(
                        Line::toned("    ", Tone::Normal)
                            .push(format!("{g} "), tone)
                            .push(r.subject.clone(), Tone::Normal)
                            .push(format!("  {}", s.flow_title(&r.flow)), Tone::Dim),
                        Line::toned(
                            format!(
                                "{}  {}  {}",
                                progress(r),
                                fmt_elapsed(end.saturating_sub(r.started_ms)),
                                fmt_k(r.usage.total())
                            ),
                            Tone::Dim,
                        ),
                        width,
                    );
                }
                let end = r.ended_ms.unwrap_or(now);
                let station = progress(r);
                crate::sessions::split_line(
                    Line::toned("    ", Tone::Normal)
                        .push(format!("{g} "), tone)
                        .push(r.subject.clone(), Tone::Normal)
                        .push(format!("  {text}"), marks::run(&r.status).text_tone()),
                    Line::toned(
                        format!(
                            "{station}  {}  {}",
                            fmt_elapsed(end.saturating_sub(r.started_ms)),
                            fmt_k(r.usage.total())
                        ),
                        Tone::Dim,
                    ),
                    width,
                )
            }
        })
        .collect()
}

pub(crate) fn lower_lines(s: &State, width: usize) -> Vec<Line> {
    let rows = s.flow_lower_rows();
    if let Some(FlowRow::Flow(name)) = s.selected_flow_row() {
        let Some(d) = s.domain.flow_details.get(&name) else {
            return vec![];
        };
        let depth = crate::panel::layers(&d.stations);
        return crate::panel::flow_graph(d, width)
            .into_iter()
            .map(|l| crate::sessions::split_line(l, Line::default(), width))
            .chain(
                depth
                    .iter()
                    .enumerate()
                    .map(|(i, l)| crate::panel::design_station(d, i, *l, width)),
            )
            .collect();
    }
    let Some(v) = s.selected_run() else {
        return vec![];
    };
    rows.iter()
        .map(|r| match r {
            FlowLower::Main => {
                let sum = s.domain.sessions.iter().find(|x| x.id == v.main_session);
                let (g, tone) = sum
                    .map(|x| crate::sessions::state_glyph(x.state))
                    .unwrap_or(("○", Tone::Dim));
                let end = v.summary.ended_ms.unwrap_or(s.ui.now_ms);
                let mut left = Line::toned(format!("  {g} "), tone)
                    .push(i18n::t!("flows.main_agent"), Tone::Strong);
                if let Some(m) = s.session_model(&v.main_session) {
                    left = left.push(format!("  {m}"), Tone::Dim);
                }
                crate::sessions::split_line(
                    left,
                    Line::toned(
                        crate::marks::cost(
                            end.saturating_sub(v.summary.started_ms),
                            sum.map(|x| &x.usage),
                        ),
                        Tone::Dim,
                    ),
                    width,
                )
            }
            FlowLower::Proposal(i) => match v.proposals.get(*i) {
                Some(p) => Line::toned("  ⚑ ", Tone::Warn)
                    .push(crate::flow_addr::proposal_text(p, v), Tone::Warn),
                None => Line::default(),
            },
            FlowLower::Station(id) => {
                let Some(st) = v.stations.iter().find(|x| &x.id == id) else {
                    return Line::default();
                };
                let m = marks::station(st.status);
                // 不缩进：进展靠颜色读（做完的正常、正在做的主色、没轮到的淡色），
                // 汇合靠"← 等 …"读。
                let own_tasks = st
                    .session
                    .as_ref()
                    .is_some_and(|sid| s.domain.tasks.values().any(|t| &t.owner == sid));
                let fold = match (
                    st.tasks.is_empty() && !own_tasks,
                    s.station_open(&v.summary.id, id),
                ) {
                    (true, _) => "  ",
                    (false, true) => "▾ ",
                    (false, false) => "▸ ",
                };
                let mut left = Line::toned(fold, Tone::Dim)
                    .push(format!("{} ", m.glyph()), m.tone())
                    .push(st.title.clone(), m.name_tone())
                    .push(format!("  {}", st.work), Tone::Dim);
                if let Some(sid) = &st.session {
                    left = left.push(i18n::t!("flows.own_session"), Tone::Dim);
                    if let Some(m) = s.session_model(sid) {
                        left = left.push(format!(" · {m}"), Tone::Dim);
                    }
                }
                let note = join_note(&v.stations, st);
                if !note.is_empty() {
                    left = left.push(format!("  {note}"), Tone::Dim);
                }
                // 多人确认的票数：够了才过。
                if let Some(votes) = st.votes {
                    let mut t = i18n::t!("flows.votes", n = votes.approvals, min = votes.min);
                    if votes.rejections > 0 {
                        t.push_str(&i18n::t!("flows.votes_rejected", n = votes.rejections));
                    }
                    let tone = if votes.rejections > 0 {
                        Tone::Error
                    } else if st.status == StationStatus::Passed {
                        Tone::Dim
                    } else {
                        Tone::Warn
                    };
                    left = left.push(format!("  {t}"), tone);
                }
                // 状态不写字：左边的符号与颜色已经说了。未达标的原因例外，那是信息。
                let mut right = Line::default();
                if let Some(u) = &st.unmet {
                    right = Line::toned(format!("{u}  "), Tone::Error);
                }
                if st.elapsed_ms > 0 {
                    right = right.push(
                        crate::marks::cost(st.elapsed_ms, Some(&st.usage)),
                        Tone::Dim,
                    );
                }
                crate::sessions::split_line(left, right, width)
            }
            FlowLower::StationTask(id, k) => {
                let Some(st) = v.stations.iter().find(|x| &x.id == id) else {
                    return Line::default();
                };
                let t = &st.tasks[*k];
                let (g, tone) = task_glyph(t.status);
                let last = *k + 1 == st.tasks.len();
                let (icon, itone) = if t.session.is_some() {
                    crate::marks::kind_icon(&port::TaskKind::Agent)
                } else {
                    crate::marks::kind_icon(&port::TaskKind::Unknown(String::new()))
                };
                let mut left = Line::toned(if last { "    └ " } else { "    ├ " }, Tone::Dim)
                    .push(format!("{g} "), tone)
                    .push(format!("{icon} "), itone)
                    .push(t.label.clone(), Tone::Normal);
                if let Some(m) = t.session.as_ref().and_then(|sid| s.session_model(sid)) {
                    left = left.push(format!("  {m}"), Tone::Dim);
                }
                crate::sessions::split_line(
                    left,
                    Line::toned(
                        crate::marks::cost(
                            0,
                            t.session.as_ref().and_then(|id| {
                                s.domain
                                    .sessions
                                    .iter()
                                    .find(|x| &x.id == id)
                                    .map(|x| &x.usage)
                            }),
                        ),
                        Tone::Dim,
                    ),
                    width,
                )
            }
            FlowLower::Task(id, prefix) => match s.domain.tasks.get(id) {
                Some(t) => crate::sessions::task_line(s, t, prefix, width),
                None => Line::default(),
            },
            FlowLower::Design(_) | FlowLower::DefStation(_) => Line::default(),
        })
        .collect()
}

/// 汇合的站标出它在等谁：`← 等 代码评审、文档评审`。
fn join_note(all: &[port::StationView], st: &port::StationView) -> String {
    if st.needs.len() < 2 {
        return String::new();
    }
    let names: Vec<&str> = st
        .needs
        .iter()
        .filter_map(|n| all.iter().find(|x| &x.id == n).map(|x| x.title.as_str()))
        .collect();
    i18n::t!(
        "flows.waits_for",
        names = names.join(&i18n::t!("flows.sep"))
    )
}

pub(crate) fn lower_title(s: &State) -> (Option<String>, Option<String>) {
    match s.selected_flow_row() {
        Some(FlowRow::Run(flow, id)) => {
            let Some(r) = s.run_summary(&flow, &id) else {
                return (None, None);
            };
            if !is_ended(&r.status) {
                return (Some(r.subject.clone()), Some(progress(r)));
            }
            // 已结束的运行写出它是哪个流程的、跑的哪份定义、那份定义现在怎样了。站数在列表那一行
            // 里有了；框右边的标题至多占一半宽，所以流程写在左边。
            let title = i18n::t!(
                "flows.ended_title",
                subject = r.subject,
                flow = s.flow_title(&flow)
            );
            let def = s.domain.run_views.get(&id).map(|v| {
                let digest: String = v.def_digest.chars().take(7).collect();
                let state = match v.def_state {
                    port::DefState::Current => String::new(),
                    port::DefState::Changed => i18n::t!("flows.def_changed"),
                    port::DefState::Deleted => i18n::t!("flows.def_deleted"),
                };
                format!("{}{state}", i18n::t!("flows.attribution", digest = digest))
            });
            (Some(title), def)
        }
        Some(FlowRow::Ended | FlowRow::More) => (None, None),
        Some(FlowRow::Flow(name)) => {
            let f = s.domain.flows.iter().find(|f| f.name == name);
            (
                f.map(|f| i18n::t!("flows.design_of", title = f.title)),
                // 有没结束的运行时改不了这个流程（实例拒绝），右边先说明白。
                f.filter(|f| f.active_runs > 0)
                    .map(|f| i18n::t!("flows.locked", n = f.active_runs)),
            )
        }
        None => (None, None),
    }
}

pub(crate) fn action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    match s.ui.focus {
        Focus::SideUpper => upper_action(s, a),
        Focus::SideLower => lower_action(s, a),
        _ => None,
    }
}

/// 运行的话头：主题、流程、ID（ID 让工具对上是哪一次，主题让人看得懂）。
fn run_ref(s: &State, flow: &str, id: &RunId) -> String {
    let subject = s
        .run_summary(flow, id)
        .map(|r| r.subject.clone())
        .or_else(|| {
            s.domain
                .run_views
                .get(id)
                .map(|v| v.summary.subject.clone())
        })
        .unwrap_or_default();
    i18n::t!(
        "flows.prefill_run_ref",
        subject = subject,
        flow = s.flow_title(flow),
        id = id
    )
}

fn upper_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let row = s.selected_flow_row();
    let ended = s.selected_ended_run().is_some();
    Some(match (a, row) {
        (Action::Expand, Some(FlowRow::Flow(name))) => {
            s.ui.flow_expanded.insert(name);
            vec![]
        }
        (Action::Expand | Action::Open, Some(FlowRow::Ended)) if !s.ui.ended_expanded => {
            s.ui.ended_expanded = true;
            vec![]
        }
        (Action::Collapse | Action::Open, Some(FlowRow::Ended)) => {
            s.ui.ended_expanded = false;
            vec![]
        }
        (Action::Collapse, Some(FlowRow::Run(..) | FlowRow::More)) if ended || row_is_more(s) => {
            s.ui.ended_expanded = false;
            s.ui.upper_mut().selected = s
                .flow_rows()
                .iter()
                .position(|r| r == &FlowRow::Ended)
                .unwrap_or(0);
            vec![]
        }
        (Action::Open, Some(FlowRow::More)) => {
            let before = s.domain.ended_runs.last().and_then(|r| r.ended_ms);
            vec![s.call(
                Pending::EndedRuns { append: true },
                Command::ListEndedRuns {
                    limit: ENDED_PAGE,
                    before_ms: before,
                },
            )]
        }
        (Action::Collapse, Some(FlowRow::Flow(name) | FlowRow::Run(name, _))) => {
            s.ui.flow_expanded.remove(&name);
            s.ui.upper_mut().selected = s
                .flow_rows()
                .iter()
                .position(|r| r == &FlowRow::Flow(name.clone()))
                .unwrap_or(0);
            vec![]
        }
        (Action::Open, Some(FlowRow::Run(..))) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        (Action::New, _) => crate::jobs::prefill(s, &i18n::t!("flows.prefill_new"), NOTICE),
        (Action::Mention, Some(FlowRow::Flow(name))) => {
            let (n, title) = (s.flow_number(&name), s.flow_title(&name));
            let text = i18n::t!("flows.prefill_flow", n = n, title = title);
            crate::jobs::prefill(s, &text, NOTICE)
        }
        (Action::Mention, Some(FlowRow::Run(flow, id))) => {
            let text = i18n::t!("flows.prefill_run", run = run_ref(s, &flow, &id));
            crate::jobs::prefill(s, &text, NOTICE)
        }
        (Action::View, Some(FlowRow::Flow(name))) => {
            let yaml = s
                .domain
                .flow_details
                .get(&name)
                .map(|d| d.yaml.clone())
                .unwrap_or_default();
            s.ui.overlay = Some(crate::state::Overlay::Text {
                title: i18n::t!("flows.definition_of", name = name),
                lines: yaml.lines().map(str::to_string).collect(),
                scroll: 0,
            });
            vec![]
        }
        (Action::Retry, _) if s.domain.load_errors[Tab::Flows.index()].is_some() => {
            s.domain.load_errors[Tab::Flows.index()] = None;
            vec![s.call(Pending::Flows, Command::ListFlows)]
        }
        _ => return None,
    })
}

const NOTICE: &str = "flows.notice_tool";

fn row_is_more(s: &State) -> bool {
    s.selected_flow_row() == Some(FlowRow::More)
}

fn lower_action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    if let Some(FlowRow::Flow(name)) = s.selected_flow_row() {
        return def_action(s, a, &name);
    }
    let run = s.selected_run()?.clone();
    let rows = s.flow_lower_rows();
    let row = rows.get(s.ui.lower[Tab::Flows.index()].selected)?.clone();
    let station = match &row {
        FlowLower::Station(id) | FlowLower::StationTask(id, _) => {
            run.stations.iter().find(|x| &x.id == id).cloned()
        }
        _ => None,
    };
    let id = run.summary.id.clone();
    match (a, &row) {
        (Action::Expand, FlowLower::Station(st)) => {
            s.ui.station_collapsed.remove(&format!("{id}/{st}"));
            return Some(vec![]);
        }
        (Action::Expand | Action::Collapse, FlowLower::Task(t, _))
            if s.domain
                .tasks
                .get(t)
                .is_some_and(|x| x.agent_session.is_some()) =>
        {
            if a == Action::Expand {
                s.ui.tree_collapsed.remove(t);
            } else {
                s.ui.tree_collapsed.insert(t.clone());
            }
            return Some(vec![]);
        }
        (Action::Collapse, FlowLower::Station(st) | FlowLower::StationTask(st, _)) => {
            s.ui.station_collapsed.insert(format!("{id}/{st}"));
            let at = s
                .flow_lower_rows()
                .iter()
                .position(|r| r == &FlowLower::Station(st.clone()))
                .unwrap_or(0);
            s.ui.lower[Tab::Flows.index()].selected = at;
            return Some(vec![]);
        }
        _ => {}
    }
    Some(match (a, station) {
        (Action::Open, _) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        // 站上的确认、放行、强制放行、返工、重试都在对话里说：话头写明是哪次运行的哪一站、它在等什么。
        (Action::Mention, Some(st)) => {
            let waits = st
                .unmet
                .clone()
                .or_else(|| match &run.summary.status {
                    RunStatus::Waiting { reason } => Some(reason.clone()),
                    _ => None,
                })
                .filter(|_| {
                    matches!(
                        st.status,
                        StationStatus::Held | StationStatus::Unmet | StationStatus::Working
                    )
                })
                .map(|w| i18n::t!("flows.prefill_waits", what = w))
                .unwrap_or_default();
            let text = i18n::t!(
                "flows.prefill_station",
                run = run_ref(s, &run.summary.flow, &id),
                station = st.title,
                waits = waits
            );
            crate::jobs::prefill(s, &text, NOTICE)
        }
        (Action::Mention, None) => {
            let text = i18n::t!(
                "flows.prefill_run",
                run = run_ref(s, &run.summary.flow, &id)
            );
            crate::jobs::prefill(s, &text, NOTICE)
        }
        _ => return None,
    })
}

/// 选中流程定义时下半的键：Enter / `v` 用浮层看一站的配置，`n` `m` 同上半。
fn def_action(s: &mut State, a: Action, name: &str) -> Option<Vec<Effect>> {
    let row = s
        .flow_lower_rows()
        .get(s.ui.lower[Tab::Flows.index()].selected)
        .cloned();
    match (a, row) {
        (Action::Open | Action::View, Some(FlowLower::DefStation(i))) => {
            let d = s.domain.flow_details.get(name)?;
            let st = d.stations.get(i)?;
            s.ui.overlay = Some(crate::state::Overlay::Text {
                title: i18n::t!(
                    "flows.station_of",
                    flow = d.summary.title,
                    n = i + 1,
                    title = st.title
                ),
                lines: crate::panel::station_body(d, i)
                    .iter()
                    .map(Line::text)
                    .collect(),
                scroll: 0,
            });
            Some(vec![])
        }
        (Action::View | Action::New | Action::Mention, _) => upper_action(s, a),
        _ => None,
    }
}

/// 只有人能做的站上的决定（design/flows.md §4–§5）：`/confirm`、`/reject`、`/release`、`/force`。会话里的模型
/// 没有这几个工具，标签上也不放操作键（交互 §5），所以由人在输入框里打：打出来的就是人的决定，不经模型。
/// `/rework` 不是只有人能做（模型有 `flow_rework`），放在这里是让人不必绕道对话去叫模型打回。
///
/// `[运行] [站] [理由]`：运行不写，是流程标签里选中的运行、或左边正对着的运行主会话；站不写，是这个运行里唯一
/// 在等这件事的站。驳回与强制放行要写理由。打回的站是目标站，没有"在等打回"的站可推断，所以一定要写。
pub(crate) fn person_op(s: &mut State, name: &str, arg: &str) -> Vec<Effect> {
    let mut words: Vec<&str> = arg.split_whitespace().collect();
    let known = |id: &str| s.domain.run_views.contains_key(&RunId(id.to_string()));
    let run = match words.first() {
        Some(w) if known(w) => Some(RunId(words.remove(0).to_string())),
        _ => s.selected_run().map(|v| v.summary.id.clone()).or_else(|| {
            let chat = s.ui.chat.as_ref()?;
            s.domain
                .run_views
                .values()
                .find(|v| {
                    &v.main_session == chat
                        || v.stations
                            .iter()
                            .any(|st| st.session.as_ref() == Some(chat))
                })
                .map(|v| v.summary.id.clone())
        }),
    };
    let Some(view) = run.and_then(|r| s.domain.run_views.get(&r)).cloned() else {
        s.notice(i18n::t!("flows.person_no_run", cmd = name), true);
        return vec![];
    };
    let waits = |st: &port::StationView| match name {
        "confirm" | "reject" => {
            matches!(st.status, StationStatus::Working | StationStatus::Unmet)
                && st.exit.iter().any(|e| e.starts_with("confirm"))
        }
        "release" => st.status == StationStatus::Held,
        "rework" => false,
        _ => st.status == StationStatus::Unmet,
    };
    let named = words.first().and_then(|w| {
        view.stations
            .iter()
            .find(|st| st.id.0 == *w || st.title == *w)
    });
    let station = match named {
        Some(st) => {
            words.remove(0);
            st.id.clone()
        }
        None => {
            let candidates: Vec<&port::StationView> =
                view.stations.iter().filter(|st| waits(st)).collect();
            match candidates.as_slice() {
                [one] => one.id.clone(),
                [] if name == "rework" => {
                    s.notice(i18n::t!("flows.person_rework_to"), true);
                    return vec![];
                }
                [] => {
                    s.notice(i18n::t!("flows.person_no_station", cmd = name), true);
                    return vec![];
                }
                many => {
                    let ids: Vec<&str> = many.iter().map(|st| st.id.0.as_str()).collect();
                    s.notice(
                        i18n::t!(
                            "flows.person_which_station",
                            cmd = name,
                            stations = ids.join(", ")
                        ),
                        true,
                    );
                    return vec![];
                }
            }
        }
    };
    let reason = words.join(" ");
    let reason = (!reason.is_empty()).then_some(reason);
    if matches!(name, "reject" | "force") && reason.is_none() {
        s.notice(i18n::t!("flows.person_needs_reason", cmd = name), true);
        return vec![];
    }
    let op = match name {
        "confirm" => port::RunOp::Confirm {
            station: station.clone(),
            reject: false,
            reason,
        },
        "reject" => port::RunOp::Confirm {
            station: station.clone(),
            reject: true,
            reason,
        },
        "release" => port::RunOp::Release {
            station: station.clone(),
        },
        "rework" => port::RunOp::Rework {
            to: station.clone(),
            reason,
        },
        _ => port::RunOp::Force {
            station: station.clone(),
            reason: reason.unwrap_or_default(),
        },
    };
    let done = i18n::tr_args(
        &format!("flows.person_done_{name}"),
        &[("station", &station.0), ("run", &view.summary.id.0)],
    );
    vec![s.call(
        Pending::Done(done),
        Command::RunOp {
            run: view.summary.id.clone(),
            op,
        },
    )]
}

/// `/flow:export <包> [文件]`、`/flow:import <文件> [--force]`：流程包与包文件互转（design/flows.md §7.12）。
/// 两个词是流程包的保留名，所以不会与某个包的名字混；发给实例，不经模型。文件相对项目根。
pub(crate) fn package_command(s: &mut State, name: &str, arg: &str) -> Vec<Effect> {
    let mut words: Vec<&str> = arg.split_whitespace().collect();
    if name == "flow:export" {
        if words.is_empty() || words.len() > 2 {
            s.notice(i18n::t!("flows.package_usage_export"), true);
            return vec![];
        }
        let bundle = words.remove(0).to_string();
        let file = words.pop().map(String::from);
        return vec![s.call(Pending::FlowExported, Command::ExportFlow { bundle, file })];
    }
    let force = words.contains(&"--force");
    words.retain(|w| *w != "--force");
    if words.is_empty() {
        s.notice(i18n::t!("flows.package_usage_import"), true);
        return vec![];
    }
    vec![s.call(
        Pending::FlowImported,
        Command::ImportFlow {
            file: words.join(" "),
            force,
        },
    )]
}

/// 导入的报告：装了什么、谁导出的，以及要留意的（覆盖了谁、遮住了谁、命令门禁）。
pub(crate) fn imported(s: &mut State, r: port::FlowImported) {
    let p = &r.package;
    let join = |v: &[String]| {
        if v.is_empty() {
            i18n::t!("flows.package_none")
        } else {
            v.join(", ")
        }
    };
    let mut lines = vec![
        i18n::t!(
            "flows.package_line_bundle",
            name = p.name,
            version = p.version
        ),
        i18n::t!(
            "flows.package_line_by",
            conix = p.conix_version,
            at = p.exported_at
        ),
        i18n::t!("flows.package_line_flows", flows = join(&p.flows)),
        i18n::t!("flows.package_line_commands", commands = join(&p.commands)),
        i18n::t!("flows.package_line_installed", dir = r.installed),
    ];
    if let Some(b) = &r.backup {
        lines.push(i18n::t!("flows.package_warn_backup", dir = b));
    }
    if let Some(by) = &r.shadows {
        lines.push(i18n::t!(
            "flows.package_warn_shadows",
            name = p.name,
            by = by
        ));
    }
    if p.command_gates > 0 {
        lines.push(i18n::t!("flows.package_warn_gates", n = p.command_gates));
    }
    s.ui.overlay = Some(crate::state::Overlay::Text {
        title: i18n::t!("flows.package_imported_title", name = p.name),
        lines,
        scroll: 0,
    });
}
