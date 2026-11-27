//! A child run in a worktree of its own (`executor: {kind: subflow, workspace: worktree}`): the
//! kernel's side. The git work is the runner's (`runner/fork.rs`); the kernel says when it is
//! wanted and records what came of it.
//!
//! ```text
//! claim      the task claimed, [`WorkItem::Fork`] handed out (a parent not in a worktree of its
//!            own: the task fails, saying so)
//! fork       the runner commits the parent's worktree and takes an id for the child of every
//!            task that forks this round (the fork commit, once a round: `StationRec::fork`),
//!            makes `<worktrees_root>/<child>` on `<prefix>/<child>` from it under the task's id,
//!            and reports through [`Engine::subflow_forked`]: the child starts there
//! end        the child over, [`WorkItem::Join`] handed out instead of the parent's task being
//!            handed the result; the runner merges the child's branch into the parent's (when it
//!            delivered), removes its worktree, and reports through [`Engine::subflow_joined`]:
//!            the outcome recorded on the child, the parent's task given the result — or failed,
//!            with the files that conflict
//! ```
//!
//! A child in a worktree of its own delivers under its parent's output directory, in a directory
//! of its own: `<parent's output dir>/children/<station>/<task>/` ([`child_output_dir`]), so two
//! children merged back never write the same files. The parent's output directory as the child's
//! worktree holds it — what the parent had when it forked — is `${parent.output_dir}`.
//!
//! The children of a round are forked one at a time and each starts — its main session opened —
//! as soon as it is forked, and a session keeps what it was opened with. So the ids are taken for
//! the whole round at its fork commit, not as each child is made: a child's siblings' worktrees are
//! known when it forks (`ForkRec::siblings`), the ones not made yet included, and the child forked
//! first is kept from reading the one forked after it as that one is from reading it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::def::Executor;
use super::engine::{transact, Engine};
use super::host::{Outcome, WorkItem};
use super::refs::workspace_of;
use super::run::{
    Actor, ForkPoint, ForkRec, RunRecord, RunStatus, StationRec, StationStatus, TaskStatus, Workspace, WorktreeOutcome, WorktreeRec,
};
use super::txn::{Cross, Txn, M};
use super::{FlowError, Result};

/// The directory of a parent's output directory its children in worktrees of their own deliver
/// in, a directory each.
pub const CHILDREN_DIR: &str = "children";

/// What the runner made for a child: the fork commit, the id it allocated for the child
/// (`Engine::alloc_id`) and the worktree it made under it from that commit; the ids of the round's
/// children (`ForkPoint::children`, recorded with the commit by the round's first fork) and the
/// worktrees of the child's siblings (`ForkRec::siblings`).
#[derive(Debug, Clone, PartialEq)]
pub struct Forked {
    pub commit: String,
    pub child: String,
    pub worktree: WorktreeRec,
    pub children: BTreeMap<String, String>,
    pub siblings: Vec<String>,
}

/// Where a child in a worktree of its own works, as the kernel starts it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Own {
    pub id: String,
    pub worktree: WorktreeRec,
    pub output_dir: Option<String>,
    pub data_dir: Option<String>,
}

impl Own {
    pub(crate) fn apply(&self, child: &mut RunRecord) {
        child.workspace = Workspace::Worktree;
        child.base_commit = Some(self.worktree.base.clone());
        child.worktree = Some(self.worktree.clone());
        child.output_dir = self.output_dir.clone();
        child.data_dir = self.data_dir.clone();
    }
}

/// `path` as the worktree at `to` holds it, when it is in the worktree at `from`.
pub fn mapped(path: &str, from: &Path, to: &Path) -> Option<PathBuf> {
    Path::new(path).strip_prefix(from).ok().map(|rel| to.join(rel))
}

/// A child's own output directory in its parent's output directory `parent_out`:
/// `<parent_out>/children/<station>/<task>` (each made safe as a name).
pub fn child_output_dir(parent_out: &Path, station: &str, task: &str) -> PathBuf {
    workspace_of(&workspace_of(&parent_out.join(CHILDREN_DIR), station), task)
}

/// Whether a task's child works in a worktree of its own.
fn forks(rec: &RunRecord, sid: &str, tid: &str) -> Option<(String, Value)> {
    match rec.def.station(sid)?.task_spec(tid)?.executor {
        Executor::Subflow { flow, inputs, workspace: Some(Workspace::Worktree) } => Some((flow, inputs)),
        _ => None,
    }
}

/// The tasks of a station whose children fork this round and have none yet, with their flows: the
/// ones claimed and waiting for the runner, and the ones still ready — a later wave of a
/// `max_parallel` station, which forks from the same commit and must be in the list of the ones
/// going before it.
pub(crate) fn to_fork(rec: &RunRecord, sid: &str) -> Vec<(String, String)> {
    let Some(st) = rec.station(sid).filter(|s| s.status == StationStatus::Working) else { return vec![] };
    st.tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Ready || (t.status == TaskStatus::Claimed && t.child.is_none()))
        .filter_map(|t| forks(rec, sid, &t.id).map(|(flow, _)| (t.id.clone(), flow)))
        .collect()
}

/// What the runner was handed for a station's children in worktrees of their own and has not
/// reported: after a restart, handed out again. A join for a child still going is nothing to the
/// runner.
pub(crate) fn pending_work(rec: &RunRecord, sid: &str, st: &StationRec) -> Vec<WorkItem> {
    if st.status != StationStatus::Working {
        return vec![];
    }
    st.tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Claimed && forks(rec, sid, &t.id).is_some())
        .filter_map(|t| match (&t.child, &t.lease) {
            (Some(child), _) => Some(WorkItem::Join { run: rec.id.clone(), station: sid.into(), task: t.id.clone(), child: child.clone() }),
            (None, Some(l)) => Some(WorkItem::Fork { run: rec.id.clone(), station: sid.into(), round: st.round, task: t.id.clone(), token: l.token.clone() }),
            (None, None) => None,
        })
        .collect()
}

/// The child a task waits for the runner to fork, if it still does: its flow and inputs.
pub(crate) fn waiting(rec: &RunRecord, sid: &str, tid: &str, token: &str) -> Option<(String, Value)> {
    let st = rec.station(sid).filter(|s| s.status == StationStatus::Working)?;
    let t = st.task(tid)?;
    let ours = t.status == TaskStatus::Claimed && t.child.is_none() && t.lease.as_ref().is_some_and(|l| l.token == token);
    (rec.is_active() && ours).then(|| forks(rec, sid, tid)).flatten()
}

impl M<'_> {
    /// A subflow task claimed whose child works in a worktree of its own: the runner asked to
    /// fork, or the task failed when its run has no worktree of its own to fork. A run in the
    /// project's own tree is never committed in.
    pub(crate) fn fork_or_fail(&mut self, sid: &str, tid: &str, token: String) {
        let own = self.rec.workspace == Workspace::Worktree && self.rec.worktree.as_ref().is_some_and(|w| w.outcome.is_none());
        if !own {
            let why = format!(
                "the child works in a worktree of its own, made from a fork commit on its parent's branch: run {} works in {}, not in a worktree of its own",
                self.rec.id,
                self.rec.workspace.as_str()
            );
            self.task_failed(sid, tid, why);
            return;
        }
        let round = self.rec.station(sid).map_or(0, |s| s.round);
        self.fx.work.push(WorkItem::Fork { run: self.rec.id.clone(), station: sid.into(), round, task: tid.into(), token });
    }

    /// A child in a worktree of its own that is over reaches its parent through the runner
    /// ([`WorkItem::Join`]): what it did must be merged back before its parent's task has it.
    pub(crate) fn joins_later(&mut self) -> bool {
        let (Some(p), true) = (self.rec.parent.clone(), self.rec.worktree.as_ref().is_some_and(|w| w.fork.is_some())) else {
            return false;
        };
        self.fx.work.push(WorkItem::Join { run: p.run, station: p.station, task: p.task, child: self.rec.id.clone() });
        true
    }
}

impl Engine {
    /// What the runner made for the child of a subflow task in a worktree of its own (`Err`: why
    /// it could not, which fails the task's attempt). The fork commit is recorded for the station's
    /// round the first time; the child starts in its worktree, its output directory its own under
    /// its parent's. A task that no longer waits for it changes nothing. `value`: `{run, child}`,
    /// `child` null when no child was started — the worktree is then the runner's to take back.
    pub async fn subflow_forked(
        &self,
        run: &str,
        station: &str,
        task: &str,
        token: &str,
        made: std::result::Result<Forked, String>,
    ) -> Result<Outcome> {
        transact!(self, Actor::flow("dispatch"), |t| {
            t.load(run).await?;
            let rec = t.get(run);
            let Some((flow, inputs)) = waiting(rec, station, task, token) else {
                return Ok(json!({"run": run, "child": null}));
            };
            let f = match made.clone() {
                Ok(f) => f,
                Err(why) => {
                    t.m(run).task_failed(station, task, why);
                    t.settle().await?;
                    return Ok(json!({"run": run, "child": null}));
                }
            };
            let round = rec.station(station).map_or(0, |s| s.round);
            let had = rec.station(station).and_then(|s| s.fork.clone()).filter(|p| p.round == round);
            if let Some(p) = had.as_ref().filter(|p| p.commit != f.commit) {
                return Err(FlowError::Invalid(format!("station {station} of run {run} forked at {} this round, not at {}", p.commit, f.commit)));
            }
            let Some(up) = rec.worktree.clone() else {
                return Err(FlowError::Invalid(format!("run {run} has no worktree to fork")));
            };
            let (from, to) = (PathBuf::from(&up.path), PathBuf::from(&f.worktree.path));
            let in_child = |d: &str| mapped(d, &from, &to).unwrap_or_else(|| PathBuf::from(d));
            let parent_out = rec.output_dir.as_deref().map(in_child);
            let own = Own {
                id: f.child.clone(),
                worktree: WorktreeRec {
                    base: f.commit.clone(),
                    outcome: None,
                    fork: Some(ForkRec {
                        parent_path: up.path.clone(),
                        parent_branch: up.branch.clone(),
                        parent_output_dir: parent_out.as_ref().map(|p| p.to_string_lossy().into_owned()),
                        siblings: Some(f.siblings.clone()),
                    }),
                    ..f.worktree.clone()
                },
                output_dir: parent_out.as_ref().map(|p| child_output_dir(p, station, task).to_string_lossy().into_owned()),
                data_dir: rec.data_dir.as_deref().map(|d| in_child(d).to_string_lossy().into_owned()),
            };
            let now = t.fx.now;
            {
                let mut m = t.m(run);
                m.by = Actor::flow("dispatch");
                if had.is_none() {
                    if let Some(s) = m.rec.stations.get_mut(station) {
                        s.fork = Some(ForkPoint { round, commit: f.commit.clone(), at: now, children: f.children.clone() });
                    }
                    m.log("fork", Some(station), None, json!({"round": round, "commit": f.commit, "children": f.children}));
                }
                m.spawn_child(station, task, &flow, &inputs, token.to_string(), Some(Box::new(own)));
            }
            t.settle().await?;
            let started = t.get(run).station(station).and_then(|s| s.task(task)).and_then(|x| x.child.clone());
            Ok(json!({"run": run, "child": started.filter(|c| *c == f.child)}))
        })
    }

    /// What became of the worktree of `child` — a child in a worktree of its own that is over —
    /// once the runner carried it out: recorded on the child; then the parent's task, while it
    /// still waits for the child, handed the child's result, or failed with `failed` (why its
    /// work did not get into the parent's branch: the files that conflict) or because the child
    /// was cancelled. `value`: `{run, worktree}`.
    pub async fn subflow_joined(&self, child: &str, outcome: WorktreeOutcome, failed: Option<String>) -> Result<Outcome> {
        // Why it was cancelled is in its history, not its record.
        let reason = self
            .history(child)
            .await?
            .into_iter()
            .rev()
            .find(|h| h.op == "cancel")
            .and_then(|h| h.detail.get("reason").and_then(Value::as_str).map(String::from));
        transact!(self, Actor::flow("dispatch"), |t| {
            t.load(child).await?;
            let rec = t.get(child);
            if rec.is_active() {
                return Err(FlowError::Invalid(format!("run {child} is still going")));
            }
            let Some(w) = rec.worktree.as_ref().filter(|w| w.fork.is_some()) else {
                return Err(FlowError::Invalid(format!("run {child} has no worktree of its own forked from its parent's")));
            };
            if w.outcome.is_some() {
                return Ok(json!({"run": child, "worktree": w}));
            }
            let Some(parent) = rec.parent.clone() else {
                return Err(FlowError::Invalid(format!("run {child} is no child run")));
            };
            let status = rec.status;
            let result = match (status, failed.clone()) {
                (RunStatus::Finished, None) => t.m(child).child_result(),
                (RunStatus::Finished, Some(e)) => Err(e),
                _ => Err(match &reason {
                    Some(r) => format!("child run {child} was cancelled: {r}"),
                    None => format!("child run {child} was cancelled"),
                }),
            };
            {
                let mut m = t.m(child);
                let mut detail = serde_json::to_value(&outcome).unwrap_or(Value::Null);
                if let (Some(e), Some(o)) = (&failed, detail.as_object_mut()) {
                    o.insert("failed".into(), json!(e));
                }
                if let Some(w) = m.rec.worktree.as_mut() {
                    w.outcome = Some(outcome.clone());
                }
                m.log("worktree", None, None, detail);
            }
            t.fx.cross.push_back(Cross::ChildDone { child: child.to_string(), parent, result });
            t.settle().await?;
            Ok(json!({"run": child, "worktree": t.get(child).worktree}))
        })
    }
}
