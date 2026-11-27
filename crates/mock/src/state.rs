use std::collections::BTreeMap;

use port::{
    Event, FlowSummary, HistoryItem, InstanceInfo, Job, JobEntry, JobId, RunId, RunView, SessionId,
    SessionState, SessionSummary, SlashCommand, Task, TaskId, Watched,
};

use crate::seed::{FlowSeed, Seed, SessionSeed};

/// mock 后端眼里的"实例状态"。每个推出去的事件先在这里生效，所以 `Watch` 之类的快照
/// 总与已推出的增量一致——这正是真实后端要守的追赶不变量。
#[derive(Debug, Clone)]
pub struct MockState {
    pub instance: InstanceInfo,
    pub commands: Vec<SlashCommand>,
    pub sessions: BTreeMap<SessionId, SessionSeed>,
    pub tasks: BTreeMap<TaskId, Task>,
    pub task_output: BTreeMap<TaskId, String>,
    pub flows: Vec<FlowSeed>,
    pub bundles: Vec<port::BundleInfo>,
    pub library: Vec<String>,
    pub runs: BTreeMap<RunId, RunView>,
    pub jobs: BTreeMap<JobId, (Job, Vec<JobEntry>)>,
    /// 已关闭的会话，关掉的新的在前。
    pub closed: Vec<SessionSeed>,
    pub plugin_packages: Vec<port::PluginPackage>,
    pub plugins: Vec<port::PluginInfo>,
}

impl MockState {
    pub fn new(instance: InstanceInfo, seed: Seed) -> Self {
        MockState {
            instance,
            commands: seed.commands,
            sessions: seed
                .sessions
                .iter()
                .filter(|s| !s.closed)
                .cloned()
                .map(|s| (s.summary.id.clone(), s))
                .collect(),
            closed: seed.sessions.into_iter().filter(|s| s.closed).collect(),
            tasks: seed.tasks.into_iter().map(|t| (t.id.clone(), t)).collect(),
            task_output: seed.task_output,
            flows: seed.flows,
            bundles: seed.bundles,
            library: seed.library,
            plugin_packages: seed.plugin_packages,
            plugins: vec![],
            runs: seed
                .runs
                .into_iter()
                .map(|r| (r.summary.id.clone(), r))
                .collect(),
            jobs: seed
                .jobs
                .into_iter()
                .map(|j| (j.job.id.clone(), (j.job, j.history)))
                .collect(),
        }
    }

    pub fn summaries(&self) -> Vec<SessionSummary> {
        let mut v: Vec<_> = self.sessions.values().map(|s| s.summary.clone()).collect();
        v.sort_by_key(|s| std::cmp::Reverse(s.updated_ms));
        v
    }

    pub fn watched(&self, id: &SessionId) -> Option<Watched> {
        let s = self
            .sessions
            .get(id)
            .or_else(|| self.closed.iter().find(|s| &s.summary.id == id))?;
        Some(Watched {
            session: s.summary.clone(),
            history: s.history.clone(),
            running: s.running,
            goal: s.goal.clone(),
            todos: s.todos.clone(),
            tasks: self.tasks_under(id),
            prompts: s.prompts.clone(),
        })
    }

    /// 以 `root` 为根的整棵任务树：它自己的任务，加上子 Agent 侧链会话下的，递归。
    pub fn tasks_under(&self, root: &SessionId) -> Vec<Task> {
        let mut out = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(owner) = stack.pop() {
            for t in self.tasks.values().filter(|t| t.owner == owner) {
                if let Some(child) = &t.agent_session {
                    stack.push(child.clone());
                }
                out.push(t.clone());
            }
        }
        out
    }

    pub fn flow_summaries(&self) -> Vec<FlowSummary> {
        self.flows
            .iter()
            .map(|f| {
                let mut s = f.summary.clone();
                s.active_runs = self
                    .runs
                    .values()
                    .filter(|r| r.summary.flow == s.name && !run_finished(r))
                    .count() as u32;
                s
            })
            .collect()
    }

    /// 让事件生效。返回会话 id 表示这个会话的摘要变了，调用方随后补推一个 `SessionUpserted`。
    pub fn apply(&mut self, ev: &Event) -> Option<SessionId> {
        use Event::*;
        match ev {
            SessionUpserted(s) => {
                self.sessions
                    .entry(s.id.clone())
                    .and_modify(|e| e.summary = s.clone())
                    .or_insert_with(|| SessionSeed {
                        summary: s.clone(),
                        history: vec![],
                        goal: None,
                        todos: vec![],
                        prompts: vec![],
                        running: false,
                        closed: false,
                    });
                None
            }
            SessionRemoved { session } => {
                self.sessions.remove(session);
                None
            }
            TurnStarted { session } => self.with(session, |s| {
                s.running = true;
                s.summary.state = SessionState::Running;
            }),
            TextDelta { session, text } => self.append(session, text, false),
            ThinkingDelta { session, text } => self.append(session, text, true),
            ToolStarted { session, id, name } => {
                self.push(
                    session,
                    HistoryItem::Tool {
                        id: id.clone(),
                        name: name.clone(),
                        input: String::new(),
                        content: String::new(),
                        is_error: false,
                        file_changes: vec![],
                    },
                );
                None
            }
            ToolInput {
                session,
                id,
                summary,
            } => {
                self.tool(session, id, |input, _, _, _| *input = summary.clone());
                None
            }
            ToolFinished {
                session,
                id,
                content,
                is_error,
                file_changes,
            } => {
                self.tool(session, id, |_, c, e, f| {
                    *c = content.clone();
                    *e = *is_error;
                    *f = file_changes.clone();
                });
                None
            }
            TurnEnded { session, usage, .. } => self.with(session, |s| {
                s.running = false;
                s.summary.usage.add(usage);
                s.summary.state = settled(s);
            }),
            TurnFailed { session, message } => {
                self.push(
                    session,
                    HistoryItem::Notice {
                        text: format!("✗ {message}"),
                    },
                );
                self.with(session, |s| {
                    s.running = false;
                    s.summary.state = settled(s);
                })
            }
            PromptPending { session, prompt } => self.with(session, |s| {
                s.prompts.retain(|p| p.id != prompt.id);
                s.prompts.push(prompt.clone());
                s.summary.waiting = s.prompts.len() as u32;
                s.summary.state = SessionState::Waiting;
            }),
            PromptResolved { session, id, .. } => self.with(session, |s| {
                s.prompts.retain(|p| &p.id != id);
                s.summary.waiting = s.prompts.len() as u32;
                s.summary.state = settled(s);
            }),
            GoalChanged { session, goal } => self.with(session, |s| {
                s.goal = goal.clone();
                s.summary.state = settled(s);
            }),
            TodosChanged { session, todos } => {
                self.with(session, |s| s.todos = todos.clone());
                None
            }
            TaskUpserted(t) => {
                self.tasks.insert(t.id.clone(), t.clone());
                None
            }
            Event::TaskOutput { task, text, .. } => {
                self.task_output
                    .entry(task.clone())
                    .or_default()
                    .push_str(text);
                None
            }
            FlowDetail(d) => {
                match self
                    .flows
                    .iter_mut()
                    .find(|x| x.summary.name == d.summary.name)
                {
                    Some(x) => {
                        x.summary = d.summary.clone();
                        x.stations = d.stations.clone();
                        x.yaml = d.yaml.clone();
                    }
                    None => self.flows.push(FlowSeed {
                        summary: d.summary.clone(),
                        stations: d.stations.clone(),
                        yaml: d.yaml.clone(),
                    }),
                }
                None
            }
            FlowUpserted(f) => {
                match self.flows.iter_mut().find(|x| x.summary.name == f.name) {
                    Some(x) => x.summary = f.clone(),
                    None => self.flows.push(FlowSeed {
                        summary: f.clone(),
                        stations: vec![],
                        yaml: String::new(),
                    }),
                }
                None
            }
            FlowRemoved { name } => {
                self.flows.retain(|x| &x.summary.name != name);
                None
            }
            RunUpserted(r) => {
                if let Some(v) = self.runs.get_mut(&r.id) {
                    v.summary = r.clone();
                }
                None
            }
            RunDetail(v) => {
                self.runs.insert(v.summary.id.clone(), (**v).clone());
                None
            }
            JobUpserted(j) => {
                self.jobs
                    .entry(j.id.clone())
                    .and_modify(|e| e.0 = j.clone())
                    .or_insert_with(|| (j.clone(), vec![]));
                None
            }
            JobRemoved { job } => {
                self.jobs.remove(job);
                None
            }
            JobFired { job, entry } => {
                if let Some((j, hist)) = self.jobs.get_mut(job) {
                    j.last = Some(entry.outcome.clone());
                    hist.insert(0, entry.clone());
                }
                None
            }
            _ => None,
        }
    }

    fn with(&mut self, id: &SessionId, f: impl FnOnce(&mut SessionSeed)) -> Option<SessionId> {
        let s = self.sessions.get_mut(id)?;
        let before = s.summary.clone();
        f(s);
        (s.summary != before).then(|| id.clone())
    }

    fn push(&mut self, id: &SessionId, item: HistoryItem) {
        if let Some(s) = self.sessions.get_mut(id) {
            s.history.push(item);
        }
    }

    fn append(&mut self, id: &SessionId, text: &str, thinking: bool) -> Option<SessionId> {
        let s = self.sessions.get_mut(id)?;
        match (s.history.last_mut(), thinking) {
            (Some(HistoryItem::Assistant { text: t }), false)
            | (Some(HistoryItem::Thinking { text: t }), true) => t.push_str(text),
            (_, false) => s.history.push(HistoryItem::Assistant {
                text: text.to_string(),
            }),
            (_, true) => s.history.push(HistoryItem::Thinking {
                text: text.to_string(),
            }),
        }
        None
    }

    fn tool(
        &mut self,
        session: &SessionId,
        id: &port::ToolId,
        f: impl FnOnce(&mut String, &mut String, &mut bool, &mut Vec<port::FileChange>),
    ) {
        let Some(s) = self.sessions.get_mut(session) else {
            return;
        };
        for item in s.history.iter_mut().rev() {
            if let HistoryItem::Tool {
                id: tid,
                input,
                content,
                is_error,
                file_changes,
                ..
            } = item
                && tid == id
            {
                f(input, content, is_error, file_changes);
                return;
            }
        }
    }
}

fn settled(s: &SessionSeed) -> SessionState {
    let blocked = s
        .goal
        .as_ref()
        .is_some_and(|g| g.phase == port::GoalPhase::Blocked);
    if !s.prompts.is_empty() || blocked {
        SessionState::Waiting
    } else if s.running {
        SessionState::Running
    } else {
        SessionState::Idle
    }
}

pub(crate) fn run_finished(r: &RunView) -> bool {
    matches!(
        r.summary.status,
        port::RunStatus::Finished | port::RunStatus::Cancelled
    )
}
