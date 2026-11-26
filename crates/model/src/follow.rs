//! 右栏与左边对话区的对应（交互 §5）：右栏的选中按对象记；选中带会话的东西，左边跟着切过去；
//! 左边从别处换了会话，右栏的选中挪到它（看得见的话）。另有按需加载、全局待处理与 Ctrl+R
//! （需求 L-04、L-05）。每次 update 之后跑一遍。

use port::{Command, JobOutcome, RunStatus, SessionId, SessionState, StationStatus};

use crate::flows::{FlowLower, FlowRow};
use crate::jobs::JobLower;
use crate::sessions::{SessRow, TreeRow};
use crate::state::{Focus, Screen, State, Tab};
use crate::update::{Effect, Pending};

/// 让对话区显示某个会话；`parents` 是面包屑里它上面的那些，`via` 是它对应的子 Agent 任务。
pub(crate) fn show(
    s: &mut State,
    id: SessionId,
    parents: Vec<(SessionId, String)>,
    via: Option<port::TaskId>,
) -> Vec<Effect> {
    if s.ui.chat.as_ref() == Some(&id) && s.ui.ctx_parents == parents && s.ui.task_view.is_none() {
        return vec![];
    }
    s.ui.ctx_parents = parents;
    s.ui.task_view = None;
    let e = crate::chat_update::open_session(s, id);
    s.ui.via_task = via;
    e
}

/// 左边回到草稿：不建会话，第一句话提交时才建（交互 §4.1）。
pub(crate) fn go_draft(s: &mut State) {
    s.ui.chat = None;
    s.ui.ctx_parents.clear();
    s.ui.via_task = None;
    s.ui.task_view = None;
    s.ui.ask_active = false;
    s.ui.answering = None;
    s.ui.rename = None;
}

/// 左边回到一个用户会话：已经在某个用户会话里（或它的子 Agent、任务输出里）就回到那个会话，
/// 否则是最近活跃的那个，一个都没有就是草稿。左边本来是草稿就留在草稿：话头发出去就是一个
/// 新会话，不往不相干的旧会话里塞。流程、作业标签的 `n` `e` `d` 从这里开口。
pub(crate) fn back_to_user_session(s: &mut State) -> Vec<Effect> {
    if s.ui.chat.is_none() {
        go_draft(s);
        return vec![];
    }
    let root =
        s.ui.ctx_parents
            .first()
            .map(|(id, _)| id.clone())
            .or_else(|| s.ui.chat.clone())
            .filter(|id| is_user(s, id));
    match root.or_else(|| s.recent_user_session()) {
        Some(id) => show(s, id, vec![], None),
        None => {
            go_draft(s);
            vec![]
        }
    }
}

fn is_user(s: &State, id: &SessionId) -> bool {
    s.domain
        .sessions
        .iter()
        .any(|x| &x.id == id && port::order::is_user_session(x))
}

fn title_of(s: &State, id: &SessionId) -> String {
    s.domain
        .sessions
        .iter()
        .find(|x| &x.id == id)
        .map(|x| x.title.clone())
        .unwrap_or_default()
}

/// 只发一次的加载。
/// 作业下半一次取多少条触发记录。
const JOB_HISTORY_PAGE: u32 = 50;

/// 没取过任务树的会话各取一次 `SessionTasks`。
fn load_tasks(s: &mut State, sessions: Vec<port::SessionId>) -> Vec<Effect> {
    let mut out = vec![];
    for sid in sessions {
        if !s.domain.tasks_loaded.contains(&sid) {
            out.extend(load(
                s,
                format!("tasks:{sid}"),
                Pending::Tasks(sid.clone()),
                Command::SessionTasks { session: sid },
            ));
        }
    }
    out
}

fn load(s: &mut State, key: String, what: Pending, cmd: Command) -> Vec<Effect> {
    if !s.ui.loading.insert(key) {
        return vec![];
    }
    vec![s.call(what, cmd)]
}

pub(crate) fn after_update(s: &mut State) -> Vec<Effect> {
    if s.ui.screen != Screen::Workbench {
        return vec![];
    }
    // 详细是"看转录"的一部分：焦点不管怎么离开转录（Esc、Ctrl+G、点别处、跳去待处理），都收回精简。
    if s.ui.focus != Focus::Transcript {
        s.ui.detail = false;
    }
    // 选中流程即展开它的运行；人收起过的同一个流程不再自动展开。先于取数据，好让运行列表
    // 在同一轮里就去取。
    if (s.ui.tab, s.ui.focus) == (Tab::Flows, Focus::SideUpper)
        && let Some(FlowRow::Flow(name)) = s.selected_flow_row()
        && s.ui.flow_auto.as_deref() != Some(name.as_str())
    {
        s.ui.flow_expanded.insert(name.clone());
        s.ui.flow_auto = Some(name);
    }
    reanchor(s);
    land(s);
    let mut out = data(s);
    out.extend(follow(s));
    if s.ui.focus.in_side() {
        // 人在右栏里选：左边跟着右边，不反过来挪选中。
        s.ui.revealed_for = Some(s.ui.chat.clone());
    } else if s.ui.revealed_for.as_ref() != Some(&s.ui.chat) && reveal(s) {
        s.ui.revealed_for = Some(s.ui.chat.clone());
        reanchor(s);
        mark_seen(s);
    }
    out
}

// ---- 选中按对象记 ----

/// 一行的身份：同一个对象在列表怎么变都是同一个键。
fn row_keys(s: &State, tab: Tab, upper: bool) -> Vec<String> {
    match (tab, upper) {
        (Tab::Sessions, true) => s
            .session_rows()
            .iter()
            .map(|r| match r {
                SessRow::New => "new".into(),
                SessRow::Session(id) => format!("s:{id}"),
                SessRow::Goal(id) => format!("g:{id}"),
                SessRow::Todo(id) => format!("t:{id}"),
                SessRow::TodoItem(id, i) => format!("ti:{id}:{i}"),
                SessRow::Closed => "closed".into(),
                SessRow::ClosedSession(id) => format!("cs:{id}"),
            })
            .collect(),
        (Tab::Sessions, false) => s
            .tree_rows()
            .iter()
            .map(|r| match r {
                TreeRow::Root(id) => format!("root:{id}"),
                TreeRow::Task { id, .. } => format!("task:{id}"),
                TreeRow::Finished { owner, .. } => format!("fin:{owner}"),
            })
            .collect(),
        (Tab::Flows, true) => s
            .flow_rows()
            .iter()
            .map(|r| match r {
                FlowRow::Flow(n) => format!("f:{n}"),
                FlowRow::Run(_, id) => format!("r:{id}"),
                FlowRow::Ended => "ended".into(),
                FlowRow::More => "more".into(),
            })
            .collect(),
        (Tab::Flows, false) => s
            .flow_lower_rows()
            .iter()
            .map(|r| match r {
                FlowLower::Main => "main".into(),
                FlowLower::Proposal(i) => format!("prop:{i}"),
                FlowLower::Station(id) => format!("st:{id}"),
                FlowLower::StationTask(id, k) => format!("stt:{id}:{k}"),
                FlowLower::Task(id, _) => format!("task:{id}"),
                FlowLower::Design(i) => format!("design:{i}"),
                FlowLower::DefStation(i) => format!("def:{i}"),
            })
            .collect(),
        (Tab::Jobs, true) => s.job_rows().iter().map(|id| format!("j:{id}")).collect(),
        (Tab::Jobs, false) => {
            let history = s
                .selected_job()
                .and_then(|id| s.domain.job_history.get(&id).cloned())
                .unwrap_or_default();
            s.job_lower_rows()
                .iter()
                .map(|r| match r {
                    JobLower::Info(i) => format!("info:{i}"),
                    JobLower::Flow(n) => format!("flow:{n}"),
                    JobLower::Stat(i) => format!("stat:{i}"),
                    JobLower::Heading => "heading".into(),
                    JobLower::Entry(i) => history
                        .get(*i)
                        .map(|e| format!("e:{}", e.fired_ms))
                        .unwrap_or_default(),
                    JobLower::EntryTask(_, t, _) => format!("task:{t}"),
                })
                .collect()
        }
    }
}

fn section(s: &mut State, tab: Tab, upper: bool) -> &mut crate::state::SectionUi {
    if upper {
        &mut s.ui.upper[tab.index()]
    } else {
        &mut s.ui.lower[tab.index()]
    }
}

/// 把每个标签两半的选中挪回它上次指着的那个对象。上次之后选中被显式改过（行号对不上，或
/// 记号被清掉）就照新行号认；对象没了就落在原来的位置上（相邻的那一行）。
pub(crate) fn reanchor(s: &mut State) {
    for tab in Tab::ALL {
        for upper in [true, false] {
            let part = usize::from(!upper);
            let keys = row_keys(s, tab, upper);
            let anchor = s.ui.anchors[tab.index()][part].clone();
            let sec = section(s, tab, upper);
            if let Some((at, key)) = anchor
                && at == sec.selected
                && keys.get(at) != Some(&key)
            {
                sec.selected = match keys.iter().position(|k| k == &key) {
                    Some(i) => i,
                    None => at.min(keys.len().saturating_sub(1)),
                };
            }
            let at = sec.selected;
            s.ui.anchors[tab.index()][part] = keys.get(at).map(|k| (at, k.clone()));
        }
    }
}

/// 显式改选中（按键、点击、跳转）：照新行号认，不往回挪。
pub(crate) fn select(s: &mut State, tab: Tab, upper: bool, at: usize) {
    section(s, tab, upper).selected = at;
    s.ui.anchors[tab.index()][usize::from(!upper)] = None;
    s.ui.revealed_for = Some(s.ui.chat.clone());
}

fn follow_key(s: &State) -> String {
    let at = |upper: bool| {
        let part = usize::from(!upper);
        s.ui.anchors[s.ui.tab.index()][part]
            .as_ref()
            .map(|(_, k)| k.clone())
            .unwrap_or_default()
    };
    format!("{:?}:{}:{}:{:?}", s.ui.tab, at(true), at(false), s.ui.focus)
}

/// 当前的选中算"已经跟过了"：切标签、焦点进右栏时左边不动，等人真的换了选中再切。
pub(crate) fn mark_seen(s: &mut State) {
    reanchor(s);
    s.ui.last_follow = Some(follow_key(s));
}

/// 左边从别处换了会话（草稿、面包屑退回、Ctrl+R、转录里进子 Agent……）：它在当前标签里
/// 看得见就把右栏的选中挪到它。返回 true 表示已对上（或这个标签里本来就不该有它）。
fn reveal(s: &mut State) -> bool {
    let tab = s.ui.tab;
    let Some(chat) = s.ui.chat.clone() else {
        if tab == Tab::Sessions
            && let Some(i) = s.session_rows().iter().position(|r| r == &SessRow::New)
        {
            set_quiet(s, tab, true, i);
        }
        return true;
    };
    for upper in [true, false] {
        if target_at(s, tab, upper).is_some_and(|t| t.session() == Some(&chat)) {
            return true;
        }
    }
    let upper_hit = match tab {
        Tab::Sessions => s
            .session_rows()
            .iter()
            .position(|r| r == &SessRow::Session(chat.clone())),
        Tab::Flows => s.flow_rows().iter().position(|r| match r {
            FlowRow::Run(_, id) => s
                .domain
                .run_views
                .get(id)
                .is_some_and(|v| v.main_session == chat),
            _ => false,
        }),
        Tab::Jobs => None,
    };
    if let Some(i) = upper_hit {
        set_quiet(s, tab, true, i);
        return true;
    }
    let n = match tab {
        Tab::Sessions => s.tree_rows().len(),
        Tab::Flows => s.flow_lower_rows().len(),
        Tab::Jobs => s.job_lower_rows().len(),
    };
    let saved = s.ui.lower[tab.index()].selected;
    for i in 0..n {
        s.ui.lower[tab.index()].selected = i;
        if target_at(s, tab, false).is_some_and(|t| t.session() == Some(&chat)) {
            set_quiet(s, tab, false, i);
            return true;
        }
    }
    s.ui.lower[tab.index()].selected = saved;
    false
}

/// 挪选中但不算人在选：不清 `revealed_for`。
fn set_quiet(s: &mut State, tab: Tab, upper: bool, at: usize) {
    section(s, tab, upper).selected = at;
    s.ui.anchors[tab.index()][usize::from(!upper)] = None;
}

/// 落到会话标签（Ctrl+N 换到它、点标签头）：左边是草稿或某个用户会话（含它的子 Agent、任务输出）就留着；否则换到最近活跃的用户
/// 会话，一个都没有就是草稿。右栏选中对上左边，之后的"选中即切换"从这里算起。
pub(crate) fn land_on_sessions(s: &mut State) -> Vec<Effect> {
    let keep = match &s.ui.chat {
        None => true,
        Some(id) => {
            is_user(s, id)
                || s.ui
                    .ctx_parents
                    .first()
                    .is_some_and(|(root, _)| is_user(s, root))
        }
    };
    let out = if keep {
        vec![]
    } else {
        match s.recent_user_session() {
            Some(id) => show(s, id, vec![], None),
            None => {
                go_draft(s);
                vec![]
            }
        }
    };
    // 左边是子 Agent 时先把上半对到它的主会话，下半才找得到它。
    if let Some((root, _)) = s.ui.ctx_parents.first().cloned()
        && let Some(i) = s
            .session_rows()
            .iter()
            .position(|r| r == &SessRow::Session(root.clone()))
    {
        set_quiet(s, Tab::Sessions, true, i);
    }
    reveal(s);
    s.ui.revealed_for = Some(s.ui.chat.clone());
    mark_seen(s);
    out
}

// ---- 数据与选中即切换 ----

/// 右栏当前看得见的东西缺数据就去取。
fn data(s: &mut State) -> Vec<Effect> {
    let mut out = vec![];
    match s.ui.tab {
        Tab::Flows => {
            let expanded: Vec<String> = s.ui.flow_expanded.iter().cloned().collect();
            for f in expanded {
                if !s.domain.runs.contains_key(&f) {
                    out.extend(load(
                        s,
                        format!("runs:{f}"),
                        Pending::Runs(f.clone()),
                        Command::ListRuns { flow: f },
                    ));
                }
            }
            if !s.domain.ended_loaded {
                out.extend(load(
                    s,
                    "ended".into(),
                    Pending::EndedRuns { append: false },
                    Command::ListEndedRuns {
                        limit: crate::flows::ENDED_PAGE,
                        before_ms: None,
                    },
                ));
            }
            // 选中一个已结束的运行，左边是它的主会话：在那里说话不影响运行，提示一次。
            if s.ui.focus == Focus::SideUpper
                && let Some(id) = s.selected_ended_run()
                && s.ui.ended_noticed.as_ref() != Some(&id)
            {
                s.ui.ended_noticed = Some(id);
                s.notice(i18n::t!("flows.ended_notice"), false);
            }
            match s.selected_flow_row() {
                Some(FlowRow::Flow(name)) if !s.domain.flow_details.contains_key(&name) => {
                    out.extend(load(
                        s,
                        format!("flow:{name}"),
                        Pending::FlowDetail(name.clone()),
                        Command::GetFlow { name },
                    ));
                }
                Some(FlowRow::Run(_, id)) if s.domain.run_views.contains_key(&id) => {
                    // 运行的树要每个会话的任务：没取过的取一次快照，之后靠 TaskUpserted 跟上。
                    let v = &s.domain.run_views[&id];
                    let sessions: Vec<port::SessionId> = std::iter::once(v.main_session.clone())
                        .chain(v.stations.iter().flat_map(|st| {
                            st.session
                                .iter()
                                .cloned()
                                .chain(st.tasks.iter().filter_map(|t| t.session.clone()))
                        }))
                        .collect();
                    out.extend(load_tasks(s, sessions));
                }
                Some(FlowRow::Run(_, id)) => {
                    out.extend(load(
                        s,
                        format!("run:{id}"),
                        Pending::RunView {
                            id: id.clone(),
                            follow: false,
                        },
                        Command::GetRun { run: id },
                    ));
                }
                _ => {}
            }
        }
        Tab::Jobs => {
            let expanded = s.expanded_firing_sessions();
            out.extend(load_tasks(s, expanded));
            if let Some(id) = s.selected_job()
                && !s.domain.job_history.contains_key(&id)
            {
                out.extend(load(
                    s,
                    format!("history:{id}"),
                    Pending::JobHistory(id.clone()),
                    Command::JobHistory {
                        job: id,
                        before_ms: None,
                        limit: Some(JOB_HISTORY_PAGE),
                        outcome: None,
                    },
                ));
            }
        }
        Tab::Sessions => {
            if !s.domain.closed_loaded {
                out.extend(load(
                    s,
                    "closed".into(),
                    Pending::ClosedSessions,
                    Command::ListClosedSessions,
                ));
            }
            out.extend(crate::sessions::ensure_watch(s));
        }
    }
    out
}

/// 选中变了，对话区跟着切（只在焦点在右栏、是人在选的时候）。
fn follow(s: &mut State) -> Vec<Effect> {
    if !s.ui.focus.in_side() {
        return vec![];
    }
    let key = follow_key(s);
    if s.ui.last_follow.as_deref() == Some(key.as_str()) {
        return vec![];
    }
    let upper = s.ui.focus == Focus::SideUpper;
    let target = target_at(s, s.ui.tab, upper);
    // 目标的数据还没到（运行详情）时先不记下，到了再切。没有会话的行（流程定义、作业、
    // 定义里的站）记下就完了：左边不动。
    if target.is_some() || !waits_for_data(s, upper) {
        s.ui.last_follow = Some(key);
    }
    match target {
        Some(Target::Session(id, parents, via)) => show(s, id, parents, via),
        Some(Target::Output(owner, task, parents)) => {
            if s.ui.task_view.as_ref() == Some(&task) {
                return vec![];
            }
            s.ui.ctx_parents = parents;
            crate::chat_update::open_task_output(s, owner, task)
        }
        Some(Target::Draft) => {
            go_draft(s);
            vec![]
        }
        None => vec![],
    }
}

/// 选中的是运行，它的详情还没到。
fn waits_for_data(s: &State, upper: bool) -> bool {
    upper
        && s.ui.tab == Tab::Flows
        && matches!(s.selected_flow_row(), Some(FlowRow::Run(_, id)) if !s.domain.run_views.contains_key(&id))
}

enum Target {
    Session(SessionId, Vec<(SessionId, String)>, Option<port::TaskId>),
    /// 没有会话的任务：看它的输出（只读）。
    Output(SessionId, port::TaskId, Vec<(SessionId, String)>),
    /// "＋ 新建会话"那一行：左边是草稿。
    Draft,
}

impl Target {
    /// 左边会显示哪个会话（任务输出挂在它所属的会话上，不算）。
    fn session(&self) -> Option<&SessionId> {
        match self {
            Target::Session(id, ..) => Some(id),
            Target::Output(..) | Target::Draft => None,
        }
    }
}

/// 右栏某一半当前选中的那一行带着哪个会话。流程定义、作业本身没有会话。
fn target_at(s: &State, tab: Tab, upper: bool) -> Option<Target> {
    match (tab, upper) {
        (Tab::Sessions, true) => match s.session_rows().get(s.ui.upper[0].selected)? {
            SessRow::New => Some(Target::Draft),
            row => Some(Target::Session(row.session()?.clone(), vec![], None)),
        },
        (Tab::Sessions, false) => {
            let root = s.selected_session()?;
            match s.tree_rows().get(s.ui.lower[0].selected)? {
                TreeRow::Root(id) => Some(Target::Session(id.clone(), vec![], None)),
                TreeRow::Task { id, .. } => {
                    let t = s.task(id)?;
                    let parents = vec![(root.clone(), title_of(s, &root))];
                    Some(match t.agent_session.clone() {
                        Some(child) => Target::Session(child, parents, Some(t.id.clone())),
                        None => Target::Output(root, t.id.clone(), parents),
                    })
                }
                TreeRow::Finished { .. } => None,
            }
        }
        (Tab::Flows, true) => match s.selected_flow_row()? {
            FlowRow::Flow(_) | FlowRow::Ended | FlowRow::More => None,
            FlowRow::Run(_, id) => {
                let v = s.domain.run_views.get(&id)?;
                Some(Target::Session(v.main_session.clone(), vec![], None))
            }
        },
        (Tab::Flows, false) => {
            let v = s.selected_run()?;
            match s
                .flow_lower_rows()
                .get(s.ui.lower[Tab::Flows.index()].selected)?
            {
                FlowLower::Main => Some(Target::Session(v.main_session.clone(), vec![], None)),
                // 建议是哪一站的会话提的，就切到哪里去看。
                FlowLower::Proposal(i) => {
                    let p = v.proposals.get(*i)?;
                    let own = v
                        .stations
                        .iter()
                        .find(|x| x.id == p.station)
                        .and_then(|x| x.session.clone());
                    Some(match own {
                        Some(id) => Target::Session(
                            id,
                            vec![(v.main_session.clone(), v.summary.subject.clone())],
                            None,
                        ),
                        None => Target::Session(v.main_session.clone(), vec![], None),
                    })
                }
                // 有独立会话的站切到它；其余的站都在运行的主会话里做。
                FlowLower::Station(st) => {
                    let st = v.stations.iter().find(|x| &x.id == st)?;
                    Some(match &st.session {
                        Some(id) => Target::Session(
                            id.clone(),
                            vec![(v.main_session.clone(), v.summary.subject.clone())],
                            None,
                        ),
                        None => Target::Session(v.main_session.clone(), vec![], None),
                    })
                }
                FlowLower::StationTask(st, k) => {
                    let t = v.stations.iter().find(|x| &x.id == st)?.tasks.get(*k)?;
                    let child = t.session.clone()?;
                    Some(Target::Session(
                        child,
                        vec![(v.main_session.clone(), v.summary.subject.clone())],
                        Some(t.id.clone()),
                    ))
                }
                FlowLower::Task(id, _) => {
                    let t = s.domain.tasks.get(id)?;
                    let parents = vec![(v.main_session.clone(), v.summary.subject.clone())];
                    Some(match t.agent_session.clone() {
                        Some(child) => Target::Session(child, parents, Some(t.id.clone())),
                        None => Target::Output(t.owner.clone(), t.id.clone(), parents),
                    })
                }
                FlowLower::Design(_) | FlowLower::DefStation(_) => None,
            }
        }
        (Tab::Jobs, true) => None,
        (Tab::Jobs, false)
            if let Some(crate::jobs::JobLower::EntryTask(i, t, _)) = s
                .job_lower_rows()
                .get(s.ui.lower[Tab::Jobs.index()].selected) =>
        {
            let t = s.domain.tasks.get(t)?;
            let e = s.domain.job_history.get(&s.selected_job()?)?.get(*i)?;
            let parents = match &e.target {
                Some(port::JobTarget::Session { id, title }) => vec![(id.clone(), title.clone())],
                _ => vec![],
            };
            Some(match t.agent_session.clone() {
                Some(child) => Target::Session(child, parents, Some(t.id.clone())),
                None => Target::Output(t.owner.clone(), t.id.clone(), parents),
            })
        }
        (Tab::Jobs, false) => match s.selected_job_target()? {
            port::JobTarget::Session { id, .. } => Some(Target::Session(id, vec![], None)),
            port::JobTarget::Run { id, .. } => {
                let v = s.domain.run_views.get(&id)?;
                Some(Target::Session(v.main_session.clone(), vec![], None))
            }
        },
    }
}

/// 等你处理的地方（需求 L-04）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Attention {
    Session(SessionId),
    Run(String, port::RunId),
    Job(port::JobId),
}

impl From<port::AlertTarget> for Attention {
    fn from(t: port::AlertTarget) -> Self {
        match t {
            port::AlertTarget::Session(id) => Attention::Session(id),
            port::AlertTarget::Run { flow, run } => Attention::Run(flow, run),
            port::AlertTarget::Job(id) => Attention::Job(id),
        }
    }
}

impl State {
    pub fn attention(&self) -> Vec<Attention> {
        let mut out = vec![];
        for x in self.user_sessions() {
            let blocked = self
                .domain
                .convs
                .get(&x.id)
                .and_then(|c| c.goal.as_ref())
                .is_some_and(|g| g.phase == port::GoalPhase::Blocked);
            if x.waiting > 0 || x.state == SessionState::Waiting || blocked {
                out.push(Attention::Session(x.id.clone()));
            }
        }
        for (flow, runs) in &self.domain.runs {
            for r in runs {
                if matches!(r.status, RunStatus::Waiting { .. }) {
                    out.push(Attention::Run(flow.clone(), r.id.clone()));
                }
            }
        }
        for v in self
            .domain
            .run_views
            .values()
            .filter(|v| !crate::flows::is_ended(&v.summary.status))
        {
            let stuck = v
                .stations
                .iter()
                .any(|st| matches!(st.status, StationStatus::Held | StationStatus::Unmet));
            let item = Attention::Run(v.summary.flow.clone(), v.summary.id.clone());
            if stuck && !out.contains(&item) {
                out.push(item);
            }
        }
        for j in &self.domain.jobs {
            if matches!(j.last, Some(JobOutcome::Failed { .. })) {
                out.push(Attention::Job(j.id.clone()));
            }
        }
        out
    }

    pub fn attention_count(&self, tab: Tab) -> u32 {
        self.attention()
            .iter()
            .filter(|a| {
                matches!(
                    (a, tab),
                    (Attention::Session(_), Tab::Sessions)
                        | (Attention::Run(..), Tab::Flows)
                        | (Attention::Job(_), Tab::Jobs)
                )
            })
            .count() as u32
    }
}

/// Ctrl+R：跳到下一处待处理（需求 L-05）。
pub(crate) fn next_attention(s: &mut State) -> Vec<Effect> {
    let items = s.attention();
    if items.is_empty() {
        s.notice(i18n::t!("follow.none_waiting"), false);
        return vec![];
    }
    let i = s.ui.attention_cursor % items.len();
    s.ui.attention_cursor = i + 1;
    go_to(s, &items[i]);
    s.notice(
        i18n::t!("follow.position", n = i + 1, total = items.len()),
        false,
    );
    vec![]
}

/// 跳到一件等人处理的事：^R 与"离开期间"浮层的 Enter 同一个落点。
pub(crate) fn go_to(s: &mut State, item: &Attention) {
    s.ui.side_collapsed = false;
    s.ui.showing = tui::ShowPane::Side;
    s.ui.focus = Focus::SideUpper;
    s.ui.filter = Default::default();
    // 那一行可能还没取回来（流程的运行列表、作业列表）：记下，数据到了再选中（`land`）。
    s.ui.pending_jump = Some(item.clone());
    s.ui.pending_jump_set = false;
    land(s);
}

/// 待跳转的那一行出现了就选中它；人自己改了标签就作废。
fn land(s: &mut State) {
    let Some(item) = s.ui.pending_jump.clone() else {
        return;
    };
    let want = match item {
        Attention::Session(_) => Tab::Sessions,
        Attention::Run(..) => Tab::Flows,
        Attention::Job(_) => Tab::Jobs,
    };
    if s.ui.tab != want && s.ui.pending_jump_set {
        s.ui.pending_jump = None;
        return;
    }
    s.ui.pending_jump_set = true;
    let item = &item;
    match item {
        Attention::Session(id) => {
            s.ui.tab = Tab::Sessions;
            if let Some(k) = s
                .session_rows()
                .iter()
                .position(|r| r == &SessRow::Session(id.clone()))
            {
                select(s, s.ui.tab, true, k);
                s.ui.pending_jump = None;
            }
        }
        Attention::Run(flow, id) => {
            s.ui.tab = Tab::Flows;
            s.ui.flow_expanded.insert(flow.clone());
            if let Some(k) = s
                .flow_rows()
                .iter()
                .position(|r| r == &FlowRow::Run(flow.clone(), id.clone()))
            {
                select(s, s.ui.tab, true, k);
                s.ui.pending_jump = None;
            }
        }
        Attention::Job(id) => {
            s.ui.tab = Tab::Jobs;
            if let Some(k) = s.job_rows().iter().position(|x| x == id) {
                select(s, s.ui.tab, true, k);
                s.ui.pending_jump = None;
            }
        }
    }
}
