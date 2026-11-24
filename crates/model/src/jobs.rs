//! 作业标签（需求 J，交互 §5.4）：上半是作业列表，下半是选中作业的定义、执行统计与触发记录；简单作业的
//! 一次触发展开是那个会话的任务树。作业本身没有会话，选中它左边不动。键只管界面：新建、修改、删除、启停、
//! 立即运行、停下都回到用户会话里说（`n` 新建、`m` 说选中的这一项，放话头）。

use keys::Action;
use port::{Command, JobId, JobKind, JobOutcome, JobTarget};
use tui::{Line, Tone};

use crate::panel::{JobDefRow, fmt_clock, job_def, job_stats, outcome};
use crate::state::{Focus, State, Tab};
use crate::update::{Effect, Pending};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobLower {
    Info(usize),
    /// 流程作业的流程：Enter 跳到流程标签里它的位置。
    Flow(String),
    /// 执行统计的一行（只看）。
    Stat(usize),
    Heading,
    Entry(usize),
    /// 展开的一次触发（简单作业）下面，它那个会话派出的任务。
    EntryTask(usize, port::TaskId, String),
}

impl State {
    pub fn job_rows(&self) -> Vec<JobId> {
        let filter = &self.ui.filter[Tab::Jobs.index()];
        port::order::jobs(&self.domain.jobs)
            .into_iter()
            .filter(|j| filter.is_empty() || j.name.contains(filter.as_str()))
            .map(|j| j.id.clone())
            .collect()
    }

    pub fn selected_job(&self) -> Option<JobId> {
        self.job_rows()
            .get(self.ui.upper[Tab::Jobs.index()].selected)
            .cloned()
    }

    pub fn job_lower_rows(&self) -> Vec<JobLower> {
        let Some(id) = self.selected_job() else {
            return vec![];
        };
        let Some(job) = self.domain.jobs.iter().find(|j| j.id == id) else {
            return vec![];
        };
        let mut out: Vec<JobLower> = job_def(job, None)
            .iter()
            .enumerate()
            .map(|(i, r)| match r {
                JobDefRow::Flow(name, _) => JobLower::Flow(name.clone()),
                JobDefRow::Plain(_) => JobLower::Info(i),
            })
            .collect();
        let history = self.domain.job_history.get(&id);
        let stats = job_stats(history.map_or(&[][..], Vec::as_slice)).len();
        out.extend((0..stats).map(JobLower::Stat));
        out.push(JobLower::Heading);
        for (i, e) in history.map_or(&[][..], Vec::as_slice).iter().enumerate() {
            out.push(JobLower::Entry(i));
            if let Some(JobTarget::Session { id: sid, .. }) = &e.target
                && self.ui.entry_expanded.contains(&entry_key(&id, e.fired_ms))
            {
                let mut tasks = vec![];
                self.work_tasks(sid, "  ", &[], &mut tasks);
                out.extend(tasks.into_iter().map(|(t, p)| JobLower::EntryTask(i, t, p)));
            }
        }
        out
    }

    /// 作业在规范顺序里的序号（`#n`，从 1 起）。
    pub(crate) fn job_number(&self, id: &JobId) -> usize {
        port::order::jobs(&self.domain.jobs)
            .iter()
            .position(|j| &j.id == id)
            .map_or(0, |i| i + 1)
    }

    /// 下半选中的那次触发去向哪里。
    pub(crate) fn selected_job_target(&self) -> Option<JobTarget> {
        let id = self.selected_job()?;
        let rows = self.job_lower_rows();
        match rows.get(self.ui.lower[Tab::Jobs.index()].selected)? {
            JobLower::Entry(i) => self.domain.job_history.get(&id)?.get(*i)?.target.clone(),
            _ => None,
        }
    }

    /// 展开着的触发的会话：它们的任务树要取。
    pub(crate) fn expanded_firing_sessions(&self) -> Vec<port::SessionId> {
        let Some(id) = self.selected_job() else {
            return vec![];
        };
        self.domain
            .job_history
            .get(&id)
            .map(|h| {
                h.iter()
                    .filter(|e| self.ui.entry_expanded.contains(&entry_key(&id, e.fired_ms)))
                    .filter_map(|e| match &e.target {
                        Some(JobTarget::Session { id, .. }) => Some(id.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

pub(crate) fn entry_key(job: &JobId, fired_ms: u64) -> String {
    format!("{job}/{fired_ms}")
}

pub(crate) fn upper_lines(s: &State, width: usize) -> Vec<Line> {
    let order: Vec<JobId> = port::order::jobs(&s.domain.jobs)
        .iter()
        .map(|j| j.id.clone())
        .collect();
    s.job_rows()
        .iter()
        .filter_map(|id| s.domain.jobs.iter().find(|j| &j.id == id))
        .map(|j| {
            let m = match (&j.last, j.enabled) {
                (_, false) => crate::marks::Mark::Paused,
                (Some(o @ (JobOutcome::Running | JobOutcome::Failed { .. })), _) => {
                    crate::marks::outcome(o)
                }
                _ => crate::marks::Mark::Idle,
            };
            let (g, tone) = m.pair();

            let next = match (&j.auto_disabled, j.enabled, j.running_ms) {
                (_, _, Some(t)) => i18n::t!(
                    "jobs.running_for",
                    elapsed = crate::sessions::fmt_elapsed(s.ui.now_ms.saturating_sub(t))
                ),
                (Some(a), false, _) => i18n::t!("jobs.auto_disabled", n = a.after),
                (_, true, _) => j
                    .next_fire_ms
                    .map(|t| i18n::t!("jobs.next_fire", time = fmt_clock(t)))
                    .unwrap_or_default(),
                (_, false, _) => i18n::t!("jobs.disabled"),
            };
            let (last, ltone) = match &j.last {
                Some(JobOutcome::Failed { .. }) => (i18n::t!("jobs.last_failed"), Tone::Error),
                Some(JobOutcome::Done) => (i18n::t!("jobs.last_done"), Tone::Ok),
                Some(JobOutcome::Running) => (i18n::t!("jobs.running"), Tone::Accent),
                _ => (String::new(), Tone::Dim),
            };
            let n = order.iter().position(|x| x == &j.id).map_or(0, |i| i + 1);
            crate::sessions::split_line(
                Line::toned(crate::sessions::number(n, order.len()), Tone::Dim)
                    .push(format!("{g} "), tone)
                    .push(j.name.clone(), Tone::Normal)
                    .push(format!("  {}", j.cron), Tone::Dim),
                Line::toned(format!("{next}  "), Tone::Dim).push(last, ltone),
                width,
            )
        })
        .collect()
}

pub(crate) fn lower_lines(s: &State, width: usize) -> Vec<Line> {
    let Some(id) = s.selected_job() else {
        return vec![];
    };
    let Some(job) = s.domain.jobs.iter().find(|j| j.id == id) else {
        return vec![];
    };
    let flow_title = match &job.kind {
        JobKind::Flow { flow, .. } => s
            .domain
            .flows
            .iter()
            .find(|f| &f.name == flow)
            .map(|f| f.title.as_str()),
        _ => None,
    };
    let def = job_def(job, flow_title);
    let line_at = |i: usize| def.get(i).map(|r| r.line().clone()).unwrap_or_default();
    let flow_at = def
        .iter()
        .position(|r| matches!(r, JobDefRow::Flow(..)))
        .unwrap_or(0);
    let history = s.domain.job_history.get(&id).cloned().unwrap_or_default();
    let stats = job_stats(&history);
    s.job_lower_rows()
        .iter()
        .map(|r| match r {
            JobLower::Info(i) => line_at(*i),
            JobLower::Flow(_) => line_at(flow_at),
            JobLower::Stat(i) => stats.get(*i).cloned().unwrap_or_default(),
            JobLower::Heading => Line::toned(
                if history.is_empty() {
                    i18n::t!("jobs.history_empty")
                } else {
                    i18n::t!("jobs.history_open")
                },
                Tone::Dim,
            ),
            JobLower::Entry(i) => {
                let e = &history[*i];
                let fold = match &e.target {
                    Some(JobTarget::Session { .. })
                        if s.ui.entry_expanded.contains(&entry_key(&id, e.fired_ms)) =>
                    {
                        "▾ "
                    }
                    Some(JobTarget::Session { .. }) => "▸ ",
                    _ => "  ",
                };
                entry_line(e, fold, width)
            }
            JobLower::EntryTask(_, t, prefix) => match s.domain.tasks.get(t) {
                Some(t) => crate::sessions::task_line(s, t, &format!("  {prefix}"), width),
                None => Line::default(),
            },
        })
        .collect()
}

/// 触发记录的一行：时刻、结局，右边是耗时、用量与去向。
fn entry_line(e: &port::JobEntry, fold: &str, width: usize) -> Line {
    let (o, tone) = outcome(&e.outcome);
    let target = match &e.target {
        Some(JobTarget::Session { title, .. }) => i18n::t!("jobs.target_session", title = title),
        Some(JobTarget::Run { subject, .. }) => i18n::t!("jobs.target_run", subject = subject),
        None => String::new(),
    };
    let mut left =
        Line::toned(format!("{fold}{}  ", fmt_clock(e.fired_ms)), Tone::Dim).push(o, tone);
    if let Some(a) = e
        .audit
        .as_ref()
        .filter(|a| a.calls() > 0 || !a.files.is_empty())
    {
        left = left.push(format!("  {}", audit_text(a)), Tone::Dim);
    }
    crate::sessions::split_line(
        left,
        Line::toned(
            format!(
                "{}  {}  {target}",
                crate::sessions::fmt_elapsed(e.elapsed_ms),
                crate::chat::fmt_k(e.usage.billed())
            ),
            Tone::Dim,
        ),
        width,
    )
}

pub(crate) fn action(s: &mut State, a: Action) -> Option<Vec<Effect>> {
    let id = s.selected_job();
    let job = id
        .as_ref()
        .and_then(|i| s.domain.jobs.iter().find(|j| &j.id == i))
        .cloned();
    let n = job.as_ref().map_or(0, |j| s.job_number(&j.id));
    let lower = s.ui.focus == Focus::SideLower;
    let row = if lower {
        s.job_lower_rows()
            .get(s.ui.lower[Tab::Jobs.index()].selected)
            .cloned()
    } else {
        None
    };
    Some(match (a, job) {
        // 展开、收起一次触发：简单作业那次的会话的任务树。
        (Action::Expand | Action::Collapse, Some(j)) if matches!(row, Some(JobLower::Entry(_))) => {
            let e = selected_entry(s)?;
            if !matches!(e.target, Some(JobTarget::Session { .. })) {
                return None;
            }
            let k = entry_key(&j.id, e.fired_ms);
            if a == Action::Expand {
                s.ui.entry_expanded.insert(k);
            } else {
                s.ui.entry_expanded.remove(&k);
            }
            vec![]
        }
        (Action::Expand | Action::Collapse, _)
            if matches!(&row, Some(JobLower::EntryTask(_, t, _))
                if s.domain.tasks.get(t).is_some_and(|x| x.agent_session.is_some())) =>
        {
            let Some(JobLower::EntryTask(_, t, _)) = row else {
                return None;
            };
            if a == Action::Expand {
                s.ui.tree_collapsed.remove(&t);
            } else {
                s.ui.tree_collapsed.insert(t);
            }
            vec![]
        }
        (Action::New, _) => prefill(s, &i18n::t!("jobs.prefill_new"), "jobs.notice_tool"),
        // 启停、立即运行、停下、修改、删除都在对话里说：话头写明是哪个作业，选中一次触发就写明是哪一次。
        (Action::Mention, Some(j)) => {
            let text = match selected_entry(s).filter(|_| lower) {
                Some(e) => i18n::t!(
                    "jobs.prefill_entry",
                    n = n,
                    name = j.name,
                    time = fmt_clock(e.fired_ms),
                    fired = e.fired_ms
                ),
                None => i18n::t!("jobs.prefill_job", n = n, name = j.name),
            };
            prefill(s, &text, "jobs.notice_tool")
        }
        (Action::View, Some(j)) => {
            let history = s.domain.job_history.get(&j.id).cloned().unwrap_or_default();
            let mut lines: Vec<String> =
                job_def(&j, None).iter().map(|r| r.line().text()).collect();
            lines.push(String::new());
            lines.extend(job_stats(&history).iter().map(Line::text));
            lines.push(String::new());
            lines.push(i18n::t!("jobs.history_all", n = history.len()));
            for e in &history {
                lines.push(entry_line(e, "", 200).text());
                if let Some(a) = &e.audit {
                    lines.extend(audit_lines(a));
                }
            }
            s.ui.overlay = Some(crate::state::Overlay::Text {
                title: i18n::t!("jobs.view_title", n = n, name = j.name),
                lines,
                scroll: 0,
            });
            vec![]
        }
        (Action::Open, _) if s.ui.focus == Focus::SideLower && selected_flow_link(s).is_some() => {
            let name = selected_flow_link(s)?;
            jump_to_flow(s, &name);
            vec![]
        }
        // 流程作业的一次触发：跳到流程标签里它起的那次运行。
        (Action::Open, _) if lower && selected_run_link(s).is_some() => {
            let (flow, run) = selected_run_link(s)?;
            jump_to_run(s, &flow, &run);
            vec![]
        }
        (Action::Open, _) => {
            s.show_chat_pub();
            s.ui.focus = Focus::Composer;
            vec![]
        }
        (Action::Retry, _) if s.domain.load_errors[Tab::Jobs.index()].is_some() => {
            s.domain.load_errors[Tab::Jobs.index()] = None;
            vec![s.call(Pending::Jobs, Command::ListJobs)]
        }
        _ => return None,
    })
}

/// 无人值守的这一次做了什么：工具 12 次 · 改 3 个文件 · 出错 1。
fn audit_text(a: &port::FiringAudit) -> String {
    let mut t = i18n::t!("jobs.audit_calls", n = a.calls());
    if !a.files.is_empty() {
        t.push_str(&i18n::t!("jobs.audit_files", n = a.files.len()));
    }
    if a.errors > 0 {
        t.push_str(&i18n::t!("jobs.audit_errors", n = a.errors));
    }
    t
}

/// `v` 浮层里一次触发的明细：每个工具几次、改了哪些文件。
fn audit_lines(a: &port::FiringAudit) -> Vec<String> {
    let mut out = vec![];
    if !a.tools.is_empty() {
        let tools: Vec<String> = a.tools.iter().map(|(k, v)| format!("{k} ×{v}")).collect();
        out.push(format!("    {}", tools.join(" · ")));
    }
    for f in &a.files {
        out.push(format!("    ✎ {f}"));
    }
    out
}

fn selected_entry(s: &State) -> Option<port::JobEntry> {
    let id = s.selected_job()?;
    match s
        .job_lower_rows()
        .get(s.ui.lower[Tab::Jobs.index()].selected)?
    {
        JobLower::Entry(i) => s.domain.job_history.get(&id)?.get(*i).cloned(),
        _ => None,
    }
}

/// 选中的是流程作业的一次触发：它起的运行（流程、运行 ID）。
fn selected_run_link(s: &State) -> Option<(String, port::RunId)> {
    let e = selected_entry(s)?;
    let Some(JobTarget::Run { id, .. }) = e.target else {
        return None;
    };
    let flow = s
        .domain
        .run_views
        .get(&id)
        .map(|v| v.summary.flow.clone())
        .or_else(|| {
            let job = s.selected_job()?;
            match &s.domain.jobs.iter().find(|j| j.id == job)?.kind {
                JobKind::Flow { flow, .. } => Some(flow.clone()),
                JobKind::Simple { .. } => None,
            }
        })?;
    Some((flow, id))
}

/// 跳到流程标签里那次运行：没结束的在它的流程下面，结束了的在「已结束」里。
fn jump_to_run(s: &mut State, flow: &str, run: &port::RunId) {
    s.ui.tab = Tab::Flows;
    s.ui.focus = Focus::SideUpper;
    s.ui.filter[Tab::Flows.index()].clear();
    s.ui.flow_expanded.insert(flow.to_string());
    s.ui.ended_expanded = true;
    let row = crate::flows::FlowRow::Run(flow.to_string(), run.clone());
    match s.flow_rows().iter().position(|r| r == &row) {
        Some(i) => crate::follow::select(s, Tab::Flows, true, i),
        None => jump_to_flow(s, flow),
    }
}

fn selected_flow_link(s: &State) -> Option<String> {
    match s
        .job_lower_rows()
        .get(s.ui.lower[Tab::Jobs.index()].selected)?
    {
        JobLower::Flow(name) => Some(name.clone()),
        _ => None,
    }
}

/// 从流程作业跳到流程标签里它用的那个流程（选中即展开它的运行）。
fn jump_to_flow(s: &mut State, name: &str) {
    s.ui.tab = Tab::Flows;
    s.ui.focus = Focus::SideUpper;
    s.ui.filter[Tab::Flows.index()].clear();
    match s
        .flow_rows()
        .iter()
        .position(|r| r == &crate::flows::FlowRow::Flow(name.to_string()))
    {
        Some(i) => crate::follow::select(s, Tab::Flows, true, i),
        None => s.notice(i18n::t!("jobs.no_flow", name = name), true),
    }
}

/// 回到用户会话，把话头放进输入框（交互 §5.4）：作业、流程本身没有会话，改它们在对话里说。
pub(crate) fn prefill(s: &mut State, text: &str, notice: &str) -> Vec<Effect> {
    let e = crate::follow::back_to_user_session(s);
    s.show_chat_pub();
    s.ui.focus = Focus::Composer;
    s.ui.composer.clear();
    s.ui.composer.insert_str(text);
    s.notice(i18n::tr(notice), false);
    e
}
