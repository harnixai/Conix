//! A child run in a worktree of its own (`executor: {kind: subflow, workspace: worktree}`): the
//! git side (the kernel's is `kernel/fork.rs`).
//!
//! ```text
//! <worktrees_root>/<parent>/   the parent's worktree, on <prefix>/<parent>: the fork commit is
//!                              made here, on its branch, and each child is merged back here
//! <worktrees_root>/<child>/    a child's, on <prefix>/<child> from the fork commit: beside its
//!                              parent's, never in it (a host may only know one level there)
//! ```
//!
//! Children share what their parent did through git, not by reading its worktree: the parent's
//! worktree goes on changing — once one child is merged back it holds that child's work — and a
//! child that read it would see what its sibling did, which two independent paths (an
//! implementation and black-box tests of one spec) must not. The fork commit is the spec side,
//! fixed; each child has its own copy of it. What git leaves out of the parent's output directory
//! (a directory of runs it ignores) is copied into the child's worktree at the fork, and the
//! child's own output directory copied back into its parent's when it is merged.
//!
//! The child's end is the runner's, with no person to decide: delivered, its work is committed on
//! its branch and merged into its parent's branch in its parent's worktree (the parent's changes
//! committed first; a merge never leaves that tree half-merged, `git.rs`), and its worktree and
//! branch removed. A merge that conflicts fails the parent's task with the files, the branch kept.
//! A child that failed or was cancelled, or whose parent's task no longer waits for it: its work
//! committed on its branch, which is kept, its worktree removed. The parent's own end is decided
//! by a person as any run's in a worktree (`worktree.rs`).
//!
//! A child's sessions are kept from reading its parent's worktree and its siblings' ([`deny_read`],
//! `OpenSpec::deny_read`): the ones of its round by the ids the round's fork commit took for them,
//! made yet or not (`ForkPoint::children`), so which of two children forks first makes no
//! difference.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::dispatch::settle;
use super::git::{self, MergeFailed};
use super::Runner;
use crate::kernel::engine::Engine;
use crate::kernel::fork::{child_output_dir, mapped, to_fork, waiting, Forked};
use crate::kernel::layout::PROCESS_DIR;
use crate::kernel::run::{Actor, Finding, RunRecord, RunStatus, TaskStatus, WorktreeEnd, WorktreeOutcome, WorktreeRec};

/// One fork or join at a time: the children of one round fork from the commit the first made, and
/// two children are not merged into one parent at once.
static FORKS: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Fork the child of a claimed subflow task, and start it (`Engine::subflow_forked`); what was made
/// for a child that is then not started is taken back.
pub(crate) async fn run_fork(rn: &Arc<Runner>, run: String, station: String, task: String, token: String) {
    let _one = FORKS.lock().await;
    let engine = rn.engine();
    let Ok(rec) = engine.record(&run).await else { return };
    let Some((flow, _)) = waiting(&rec, &station, &task, &token) else { return };
    let made = fork(engine, &rec, &station, &task, &flow).await;
    let ours = made.as_ref().ok().cloned();
    let r = engine.subflow_forked(&run, &station, &task, &token, made).await;
    let started = r.as_ref().ok().and_then(|o| o.value["child"].as_str().map(String::from));
    if let (Some(f), Some(root)) = (ours, &rec.project_root) {
        if started.as_deref() != Some(f.child.as_str()) {
            super::worktree::unmake(engine, root, &f.worktree).await;
        }
    }
    settle(rn, &run, "subflow_forked", r).await;
}

/// The commit message for a fork: what the parent's worktree held when it forked.
fn fork_message(rec: &RunRecord, station: &str, word: &str) -> String {
    format!("{word} {} ({}): forked at {station}", rec.id, rec.flow)
}

/// The git side of a fork: the fork commit (the round's, if a sibling made it already) and an id
/// for every child of the round, the child's worktree from it under its id, what git does not
/// carry of the parent's output directory copied in, the child's own output directory made.
async fn fork(engine: &Engine, rec: &RunRecord, station: &str, task: &str, flow: &str) -> Result<Forked, String> {
    let w = rec.worktree.clone().ok_or_else(|| format!("run {} has no worktree to fork", rec.id))?;
    let root = rec.project_root.clone().ok_or_else(|| format!("run {} has no project", rec.id))?;
    let layout = engine.layout().ok_or("this host makes no worktrees")?;
    let round = rec.station(station).map_or(0, |s| s.round);
    let had = rec.station(station).and_then(|s| s.fork.clone()).filter(|p| p.round == round);
    let (commit, children) = match had {
        Some(p) => (p.commit, p.children),
        None => {
            let (path, own, message) = (PathBuf::from(&w.path), layout.dir.clone(), fork_message(rec, station, &layout.commit_word));
            let wkts = super::worktree::root_in_repo(Path::new(&root), &w.prefix, &layout.worktrees_root);
            let commit = super::worktree::blocking(move || {
                git::snapshot(&path, Some(&own), wkts.as_deref(), &message)?;
                git::head(&path).ok_or_else(|| format!("{} has no commit checked out", path.display()))
            })
            .await
            .and_then(|r| r)
            .map_err(|e| format!("the fork commit could not be made: {e}"))?;
            // Every child of the round has its id now, before the first starts — the ready ones of a
            // later wave too: each is kept from the worktrees of the others from its first session on,
            // whichever forks first.
            let mut children = BTreeMap::new();
            for (t, f) in to_fork(rec, station) {
                let id = engine.alloc_id(&f, Some(&root)).await.map_err(|e| format!("no id for the child of task {station}/{t}: {e}"))?;
                children.insert(t, id);
            }
            (commit, children)
        }
    };
    // The task's own id, unless a child has it already: an attempt after one that failed this round
    // takes a new one, which the siblings going already are not kept from.
    let mut taken = children.get(task).cloned();
    if let Some(id) = taken.clone() {
        if engine.record(&id).await.is_ok() {
            taken = None;
        }
    }
    let (child, cw) = super::worktree::make(engine, &root, flow, Some(&commit), taken.as_deref()).await.map_err(|e| e.to_string())?;
    let placed = place(rec, &w, &cw, station, task);
    if let Err(e) = placed {
        super::worktree::unmake(engine, &root, &cw).await;
        return Err(format!("the child's worktree could not be made ready: {e}"));
    }
    let siblings = siblings(engine, rec, station, task, &children).await;
    Ok(Forked { commit, child, worktree: cw, children, siblings })
}

/// The worktrees the child of `task` is kept from reading besides its parent's, as it forks: those
/// of the other children of the round, by the ids taken for them (`children`), made yet or not;
/// and those of its station's other children still going — after a rework of some of its tasks,
/// the others' of an earlier round. Fixed for the child from now on (`ForkRec::siblings`): a
/// session opened again must be opened as it was the first time, so the list is never worked out
/// again, though a sibling's worktree goes when it is merged.
async fn siblings(engine: &Engine, rec: &RunRecord, station: &str, task: &str, children: &BTreeMap<String, String>) -> Vec<String> {
    let mut out: Vec<String> = children
        .iter()
        .filter(|(t, _)| *t != task)
        .filter_map(|(_, id)| engine.worktree_dir(id))
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    for c in rec.station(station).into_iter().flat_map(|s| &s.tasks).filter(|t| t.id != task).filter_map(|t| t.child.as_ref()) {
        if let Some(w) = engine.record(c).await.ok().and_then(|s| s.worktree).filter(|w| w.fork.is_some() && w.outcome.is_none()) {
            out.push(w.path);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// In a child's new worktree `cw`: the parent's output directory as of now, where the fork commit
/// did not carry it (git ignores it: a directory of runs) — never its process directory — and the
/// child's own output directory in it, empty.
fn place(rec: &RunRecord, w: &WorktreeRec, cw: &WorktreeRec, station: &str, task: &str) -> std::io::Result<()> {
    let Some(out) = &rec.output_dir else { return Ok(()) };
    let Some(there) = mapped(out, Path::new(&w.path), Path::new(&cw.path)) else { return Ok(()) };
    if Path::new(out).is_dir() {
        copy_tree(Path::new(out), &there, &[PROCESS_DIR], false)?;
    }
    // After a rework the task's child of an earlier round left its directory in the parent's (merged
    // or copied back), and so in this copy: a new child that started with it would take what it
    // planned and recorded for its own (a work package done that this child never did). Its work is
    // in the tree all the same, and its directory stays in the parent's until this child's replaces it.
    let own = child_output_dir(&there, station, task);
    if own.is_dir() {
        std::fs::remove_dir_all(&own)?;
    }
    std::fs::create_dir_all(own)
}

/// The files under `from` copied to the same places under `to`: links left out, and the names in
/// `skip` at the top; a file already there kept unless `over`.
fn copy_tree(from: &Path, to: &Path, skip: &[&str], over: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip.iter().any(|s| name == *s) {
            continue;
        }
        let kind = entry.file_type()?;
        let dest = to.join(&name);
        if kind.is_dir() {
            copy_tree(&entry.path(), &dest, &[], over)?;
        } else if kind.is_file() && (over || !dest.exists()) {
            std::fs::copy(entry.path(), &dest)?;
        }
    }
    Ok(())
}

/// The end of a child in a worktree of its own: merged back into its parent's branch when it
/// delivered to a task still waiting for it, otherwise its branch kept; reported through
/// `Engine::subflow_joined`.
pub(crate) async fn run_join(rn: &Arc<Runner>, run: String, station: String, task: String, child: String) {
    let _one = FORKS.lock().await;
    let engine = rn.engine();
    let Ok(c) = engine.record(&child).await else { return };
    let Some(w) = c.worktree.clone().filter(|w| w.outcome.is_none() && !c.is_active()) else { return };
    let (Some(fork), Some(root), Some(layout)) = (w.fork.clone(), c.project_root.clone(), engine.layout()) else { return };
    let waits = engine.record(&run).await.ok().is_some_and(|p| {
        p.station(&station).and_then(|s| s.task(&task)).is_some_and(|t| t.status == TaskStatus::Claimed && t.child.as_deref() == Some(child.as_str()))
    });
    let delivered = c.status == RunStatus::Finished && waits;
    let job = Join {
        parent: PathBuf::from(&fork.parent_path),
        parent_branch: fork.parent_branch.clone(),
        path: PathBuf::from(&w.path),
        branch: w.branch.clone(),
        own: layout.dir.clone(),
        ours: layout.worktrees_root.clone(),
        wkts: super::worktree::root_in_repo(Path::new(&root), &w.prefix, &layout.worktrees_root),
        message: super::worktree::work_message(&c, &layout.commit_word),
        parent_message: format!("{} {run}: before merging {}", layout.commit_word, w.branch),
        merge_message: format!("Merge {} ({} {child})", w.branch, layout.commit_word),
        outs: c.output_dir.as_deref().and_then(|o| Some((PathBuf::from(o), mapped(o, Path::new(&w.path), Path::new(&fork.parent_path))?))),
        delivered,
    };
    let (kind, into, commit, how, failed) = match super::worktree::blocking(move || job.carry_out()).await {
        Ok(d) => d,
        Err(e) => (WorktreeEnd::Kept, None, None, None, Some(e)),
    };
    let outcome = WorktreeOutcome { kind, at: engine.clock().now_ms(), by: Actor::flow("dispatch"), into, commit, how: how.map(String::from) };
    let r = engine.subflow_joined(&child, outcome, failed).await;
    settle(rn, &child, "subflow_joined", r).await;
}

/// What a join works with.
struct Join {
    parent: PathBuf,
    parent_branch: String,
    path: PathBuf,
    branch: String,
    own: String,
    ours: PathBuf,
    wkts: Option<String>,
    message: String,
    parent_message: String,
    merge_message: String,
    /// The child's output directory, and where it is in the parent's worktree.
    outs: Option<(PathBuf, PathBuf)>,
    delivered: bool,
}

type Joined = (WorktreeEnd, Option<String>, Option<String>, Option<&'static str>, Option<String>);

impl Join {
    /// What became of the worktree, and why the child's work is not in the parent's branch when it
    /// should be.
    fn carry_out(self) -> Joined {
        let kept = |j: &Join, failed: Option<String>| -> Joined {
            if let Err(e) = git::remove_worktree(&j.parent, &j.path, Some(&j.ours)) {
                tracing::warn!(worktree = %j.path.display(), error = %e, "a child's worktree not removed");
            }
            (WorktreeEnd::Kept, None, git::tip(&j.parent, &j.branch).ok(), None, failed)
        };
        // Its work on its branch before anything: kept or merged, nothing it wrote is lost. A
        // worktree that cannot be committed stays where it is, for a person to look at.
        if let Err(e) = git::snapshot(&self.path, Some(&self.own), self.wkts.as_deref(), &self.message) {
            return (WorktreeEnd::Kept, None, None, None, Some(format!("the child's work could not be committed on {}: {e}", self.branch)));
        }
        if !self.delivered {
            return kept(&self, None);
        }
        if let Err(e) = git::snapshot(&self.parent, Some(&self.own), self.wkts.as_deref(), &self.parent_message) {
            return kept(&self, Some(format!("the parent's worktree could not be committed before merging {}: {e}", self.branch)));
        }
        match git::merge(&self.parent, &self.branch, &git::scratch_for(&self.path), Some(&self.ours), &self.merge_message) {
            Ok(m) => {
                // What git does not carry of the child's output directory (it ignores it): copied,
                // over what an earlier round left there.
                if let Some((from, to)) = &self.outs {
                    if let Err(e) = carry_outputs(&self.parent, from, to) {
                        tracing::warn!(from = %from.display(), to = %to.display(), error = %e, "a child's deliverables not copied into its parent's output directory");
                    }
                }
                if let Err(e) = git::remove_worktree(&self.parent, &self.path, Some(&self.ours)).and_then(|()| git::delete_branch(&self.parent, &self.branch, false)) {
                    tracing::warn!(branch = self.branch, error = %e, "a merged child's worktree or branch not removed");
                }
                (WorktreeEnd::Merged, Some(m.into), Some(m.commit), Some(m.how), None)
            }
            Err(MergeFailed::Conflict(files)) => {
                let found: Vec<String> = files.iter().map(|f| Finding::new("merge-conflict", "changed on both sides").at(f.clone()).line_text()).collect();
                let why = format!(
                    "merging {} into {} conflicts — nothing in the parent's worktree changed; the child's work is kept on {}: {}",
                    self.branch,
                    self.parent_branch,
                    self.branch,
                    found.join("; ")
                );
                kept(&self, Some(why))
            }
            Err(MergeFailed::Other(m)) => kept(&self, Some(format!("merging {} into {} failed: {m}", self.branch, self.parent_branch))),
        }
    }
}

/// The child's output directory `from` into its place `to` in the parent's worktree `parent`, when
/// git ignores it there (a directory of runs): what an earlier round left replaced. What git does
/// not ignore came with the merge.
fn carry_outputs(parent: &Path, from: &Path, to: &Path) -> std::io::Result<()> {
    if !from.is_dir() {
        return Ok(());
    }
    // Relative to the worktree: git says nothing of an absolute path spelled other than it knows.
    let rel = to.strip_prefix(parent).map(|r| format!("{}/", r.display())).unwrap_or_else(|_| format!("{}/", to.display()));
    if git::ignored(parent, &rel) != Some(true) {
        return Ok(());
    }
    if to.is_dir() {
        std::fs::remove_dir_all(to)?;
    }
    copy_tree(from, to, &[], true)
}

/// What the sessions of `rec` must not read: for a child in a worktree of its own, its parent's
/// worktree and its siblings' as they were fixed when it forked (`ForkRec::siblings`) — absolute,
/// sorted. A record from before they were fixed: the other children its parent's station has as
/// the session opens.
pub(crate) async fn deny_read(engine: &Engine, rec: &RunRecord) -> Vec<PathBuf> {
    let (Some(f), Some(p)) = (rec.worktree.as_ref().and_then(|w| w.fork.as_ref()), &rec.parent) else { return vec![] };
    let mut out = vec![PathBuf::from(&f.parent_path)];
    if let Some(s) = &f.siblings {
        out.extend(s.iter().map(PathBuf::from));
    } else if let Some(st) = engine.record(&p.run).await.ok().and_then(|up| up.stations.get(&p.station).cloned()) {
        for c in st.tasks.iter().filter_map(|t| t.child.as_ref()).filter(|c| **c != rec.id) {
            if let Some(w) = engine.record(c).await.ok().and_then(|s| s.worktree).filter(|w| w.fork.is_some()) {
                out.push(PathBuf::from(w.path));
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::kernel::engine::CreateIn;
    use crate::kernel::host::WorkItem;
    use crate::kernel::run::{TaskStatus, Workspace};
    use crate::runner::git::tests::{commit_all, have_git, repo};
    use crate::runner::ScriptedHost;
    use crate::{ManualClock, MemPacks, MemStore, ProjectLayout};

    const CHILD: &str = "{name: c, stations: [{id: a, executor: {kind: external}, output: {type: object}, exit: [{kind: output}]}]}";
    /// The spec side, then two children from it, then a station to join them in.
    const PARENT: &str = "{name: p, workspace: worktree, stations: [\
        {id: spec, executor: {kind: external}, output: {type: object}, exit: [{kind: output}]}, \
        {id: dual, tasks: [{id: impl, executor: {kind: subflow, flow: c, workspace: worktree}}, \
                           {id: tests, executor: {kind: subflow, flow: c, workspace: worktree}}], exit: [{kind: children}]}, \
        {id: join, exit: [{kind: confirm}], rework_to: [dual]}]}";
    /// The same with no word on where it works: a start decides.
    const OPEN: &str = "{name: o, stations: [\
        {id: spec, executor: {kind: external}, output: {type: object}, exit: [{kind: output}]}, \
        {id: dual, executor: {kind: subflow, flow: c, workspace: worktree}, exit: [{kind: children}]}]}";
    /// A child an item, one at a time.
    const WAVES: &str = "{name: w, workspace: worktree, inputs: {items: {schema: {type: array}}}, stations: [\
        {id: spec, executor: {kind: external}, output: {type: object}, exit: [{kind: output}]}, \
        {id: each, foreach: '${inputs.items}', max_parallel: 1, executor: {kind: subflow, flow: c, workspace: worktree}, exit: [{kind: children}]}]}";

    fn by() -> Actor {
        Actor::device("d")
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// As Conix lays it out: the runs' worktrees flat in the repository's `.wkts/` (ignored).
    fn setup(p: &Path) -> (Arc<Engine>, Arc<Runner>) {
        std::fs::write(p.join(".gitignore"), ".wkts/\n").unwrap();
        std::fs::create_dir_all(p.join(".conix")).unwrap();
        std::fs::write(p.join(".conix/.gitignore"), "state/\n").unwrap();
        commit_all(p, "ignore the worktrees");
        let packs = MemPacks::new().flow(CHILD).flow(PARENT).flow(OPEN).flow(WAVES);
        let layout = ProjectLayout::new(".conix", p.join(".wkts"), "conix/flow").with_runs_dir("state/conix/runs");
        let e = Arc::new(
            Engine::new(Arc::new(MemStore::new()), Arc::new(packs))
                .with_clock(Arc::new(ManualClock::new(1_794_787_200_000)))
                .with_layout(layout),
        );
        let rn = Runner::for_tests(e.clone(), ScriptedHost::new());
        (e, rn)
    }

    async fn started(rn: &Arc<Runner>, flow: &str, p: &Path, ws: Option<Workspace>) -> String {
        let input = CreateIn {
            flow: flow.into(),
            subject: "the subject".into(),
            project_root: Some(p.to_string_lossy().into_owned()),
            workspace: ws,
            ..Default::default()
        };
        let o = super::super::project::start(rn, input, by()).await.unwrap();
        o.value["run"].as_str().unwrap().to_string()
    }

    /// An external task claimed and handed in: the work that came of it.
    async fn hand_in(e: &Engine, run: &str, station: &str, task: &str) -> Vec<WorkItem> {
        let token = e.claim(run, station, task, by(), 60_000).await.unwrap().value["token"].as_str().unwrap().to_string();
        e.submit(run, station, task, &token, json!({"done": task}), by()).await.unwrap().work
    }

    /// The spec station done, the spec side written first and left uncommitted; its forks carried
    /// out as the dispatcher would.
    async fn forked(e: &Engine, rn: &Arc<Runner>, run: &str, wt: &Path, out: &Path) -> usize {
        std::fs::write(out.join("spec.md"), "the spec\n").unwrap();
        std::fs::write(wt.join("iface.txt"), "the interface\n").unwrap();
        let work = hand_in(e, run, "spec", "spec").await;
        let mut n = 0;
        for w in work {
            if let WorkItem::Fork { run, station, task, token, .. } = w {
                run_fork(rn, run, station, task, token).await;
                n += 1;
            }
        }
        n
    }

    /// A child's station handed in: its join carried out.
    async fn delivered(e: &Engine, rn: &Arc<Runner>, child: &str) {
        for w in hand_in(e, child, "a", "a").await {
            if let WorkItem::Join { run, station, task, child } = w {
                run_join(rn, run, station, task, child).await;
            }
        }
    }

    fn child_of(rec: &RunRecord, station: &str, task: &str) -> String {
        rec.stations[station].task(task).unwrap().child.clone().unwrap()
    }

    fn task_of(rec: &RunRecord, station: &str, task: &str) -> (TaskStatus, Option<String>) {
        let t = rec.stations[station].task(task).unwrap();
        (t.status, t.error.clone())
    }

    async fn worktree_of(e: &Engine, child: &str) -> PathBuf {
        PathBuf::from(e.record(child).await.unwrap().worktree.unwrap().path)
    }

    /// What a child's sessions are kept from.
    async fn denied(e: &Engine, child: &str) -> Vec<PathBuf> {
        deny_read(e, &e.record(child).await.unwrap()).await
    }

    fn sorted(mut v: Vec<PathBuf>) -> Vec<PathBuf> {
        v.sort();
        v
    }

    #[tokio::test]
    async fn two_children_fork_from_one_commit_work_apart_and_merge_back() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let main = git_out(p, &["rev-parse", "main"]);
        let run = started(&rn, "p", p, None).await;
        let rec = e.record(&run).await.unwrap();
        let w = rec.worktree.clone().unwrap();
        let (wt, out) = (PathBuf::from(&w.path), PathBuf::from(rec.output_dir.clone().unwrap()));
        assert_eq!(forked(&e, &rn, &run, &wt, &out).await, 2);

        // One fork commit on the parent's branch, with everything its worktree held.
        let rec = e.record(&run).await.unwrap();
        let fork = rec.stations["dual"].fork.clone().unwrap();
        assert_eq!((fork.round, fork.commit.as_str()), (1, git_out(&wt, &["rev-parse", "HEAD"]).as_str()));
        assert_eq!(git_out(&wt, &["log", "-1", "--format=%s"]), format!("Harnix task {run} (p): forked at dual"));
        let held = git_out(&wt, &["show", "--name-only", "--format=", &fork.commit]);
        let rel_out = out.strip_prefix(&wt).unwrap().to_string_lossy().into_owned();
        assert!(held.contains("iface.txt") && held.contains(&format!("{rel_out}/spec.md")), "{held}");
        assert_eq!(git_out(p, &["rev-parse", "main"]), main, "the project's own branch untouched");

        let (one, two) = (child_of(&rec, "dual", "impl"), child_of(&rec, "dual", "tests"));
        let mut outs = vec![];
        for (task, child) in [("impl", &one), ("tests", &two)] {
            let c = e.record(child).await.unwrap();
            let cw = c.worktree.clone().unwrap();
            // Flat beside the parent's, on a branch of its own from the fork commit.
            assert_eq!(PathBuf::from(&cw.path), p.join(".wkts").join(child));
            assert_eq!(cw.branch, format!("conix/flow/{child}"));
            assert_eq!((cw.base.as_str(), c.base_commit.as_deref()), (fork.commit.as_str(), Some(fork.commit.as_str())));
            assert_eq!(git_out(Path::new(&cw.path), &["rev-parse", "HEAD"]), fork.commit);
            assert_eq!(c.workspace, Workspace::Worktree);
            assert_eq!(e.workspace_for(&c), Some(PathBuf::from(&cw.path)));
            // The spec side as of the fork, its own copy: `${parent.output_dir}`.
            let f = cw.fork.clone().unwrap();
            assert_eq!((f.parent_path.as_str(), f.parent_branch.as_str()), (w.path.as_str(), w.branch.as_str()));
            let theirs = PathBuf::from(f.parent_output_dir.clone().unwrap());
            assert_eq!(theirs, Path::new(&cw.path).join(&rel_out));
            assert_eq!(std::fs::read_to_string(theirs.join("spec.md")).unwrap(), "the spec\n");
            assert!(Path::new(&cw.path).join("iface.txt").is_file());
            let ctx = crate::kernel::refs::context(&c, None, None);
            assert_eq!(ctx["parent"], json!({"run": run, "output_dir": theirs.to_string_lossy()}));
            // Its own deliverables in a directory of their own under it.
            let own = PathBuf::from(c.output_dir.clone().unwrap());
            assert_eq!(own, theirs.join("children/dual").join(task));
            assert!(own.is_dir());
            outs.push(own);
        }
        assert_ne!(outs[0], outs[1]);

        // Neither reads its parent's worktree nor its sibling's — the first forked as the second.
        let c1 = e.record(&one).await.unwrap();
        let c2 = e.record(&two).await.unwrap();
        let (p1, p2) = (PathBuf::from(&c1.worktree.as_ref().unwrap().path), PathBuf::from(&c2.worktree.as_ref().unwrap().path));
        assert_eq!(deny_read(&e, &c1).await, sorted(vec![wt.clone(), p2.clone()]));
        assert_eq!(deny_read(&e, &c2).await, sorted(vec![wt.clone(), p1.clone()]));
        assert_eq!(fork.children, [("impl".to_string(), one.clone()), ("tests".to_string(), two.clone())].into());
        assert!(deny_read(&e, &rec).await.is_empty(), "the parent is kept from nothing");
        // Its processes too — a test it wrote, run by a gate, would print the other side into the log.
        let seen = crate::runner::exec::Recording::default();
        let audit = crate::runner::exec::Audit::run(e.clone(), &one, Some("work"), None, "gate t");
        let req = crate::ExecRequest::new(vec!["true".into()], &p1, std::time::Duration::from_secs(5));
        crate::runner::exec::run_audited(&seen, req, &audit).await.unwrap();
        assert_eq!(seen.requests()[0].deny_read, sorted(vec![wt.clone(), p2.clone()]));

        // Each works apart: the implementation, the tests.
        let (w1, w2) = (PathBuf::from(&c1.worktree.unwrap().path), PathBuf::from(&c2.worktree.unwrap().path));
        std::fs::create_dir_all(w1.join("src")).unwrap();
        std::fs::write(w1.join("src/lib.rs"), "impl\n").unwrap();
        std::fs::write(outs[0].join("report.md"), "impl done\n").unwrap();
        std::fs::create_dir_all(w2.join("tests")).unwrap();
        std::fs::write(w2.join("tests/black_box.rs"), "tests\n").unwrap();
        std::fs::write(outs[1].join("report.md"), "tests done\n").unwrap();
        assert!(!w2.join("src/lib.rs").exists());

        // The parent goes on meanwhile; the first child is merged into its branch, in its worktree.
        std::fs::write(wt.join("notes.txt"), "the parent's\n").unwrap();
        delivered(&e, &rn, &one).await;
        let c1 = e.record(&one).await.unwrap();
        let o = c1.worktree.as_ref().unwrap().outcome.clone().unwrap();
        assert_eq!((o.kind, o.into.as_deref()), (WorktreeEnd::Merged, Some(w.branch.as_str())));
        assert_eq!(o.commit.as_deref(), Some(git_out(&wt, &["rev-parse", "HEAD"]).as_str()));
        assert_eq!(std::fs::read_to_string(wt.join("src/lib.rs")).unwrap(), "impl\n");
        assert_eq!(std::fs::read_to_string(wt.join(&rel_out).join("children/dual/impl/report.md")).unwrap(), "impl done\n");
        assert!(!w1.exists());
        assert_eq!(git_out(p, &["branch", "--list", &format!("conix/flow/{one}")]), "");
        assert_eq!(git_out(&wt, &["status", "--porcelain"]), "", "the parent's own change committed before the merge");
        let rec = e.record(&run).await.unwrap();
        assert_eq!(task_of(&rec, "dual", "impl").0, TaskStatus::Submitted);
        // The second sees none of it, and merges as cleanly.
        assert!(!w2.join("src/lib.rs").exists() && !w2.join("notes.txt").exists());
        delivered(&e, &rn, &two).await;
        let o = e.record(&two).await.unwrap().worktree.unwrap().outcome.unwrap();
        assert_eq!((o.kind, o.how.as_deref()), (WorktreeEnd::Merged, Some("merge")));
        for f in ["src/lib.rs", "tests/black_box.rs", "notes.txt", "iface.txt"] {
            assert!(wt.join(f).is_file(), "{f}");
        }
        assert!(wt.join(&rel_out).join("children/dual/tests/report.md").is_file());
        assert!(!w2.exists());
        let rec = e.record(&run).await.unwrap();
        assert_eq!(task_of(&rec, "dual", "tests").0, TaskStatus::Submitted);
        assert_eq!(rec.stations["join"].status, crate::kernel::run::StationStatus::Working);
        assert_eq!(git_out(p, &["rev-parse", "main"]), main, "the project's own branch still untouched");
        // A child's worktree is not a person's to decide.
        assert!(super::super::worktree::end(&e, &one, WorktreeEnd::Merged, by()).await.is_err());
    }

    /// Sent back for one of its tasks, the station forks again for that one alone: a new fork commit
    /// with both children's work merged, the new child's own directory empty though the parent's
    /// holds what its predecessor left there, the other task's result kept.
    #[tokio::test]
    async fn a_task_sent_back_forks_again_from_a_new_commit_and_starts_in_an_empty_directory() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let run = started(&rn, "p", p, None).await;
        let rec = e.record(&run).await.unwrap();
        let (wt, out) = (PathBuf::from(&rec.worktree.unwrap().path), PathBuf::from(rec.output_dir.unwrap()));
        forked(&e, &rn, &run, &wt, &out).await;
        let rec = e.record(&run).await.unwrap();
        let first = rec.stations["dual"].fork.clone().unwrap();
        let (one, two) = (child_of(&rec, "dual", "impl"), child_of(&rec, "dual", "tests"));
        for (child, file) in [(&one, "src.txt"), (&two, "tests.txt")] {
            let c = e.record(child).await.unwrap();
            std::fs::write(Path::new(&c.worktree.unwrap().path).join(file), "round one\n").unwrap();
            std::fs::write(Path::new(&c.output_dir.unwrap()).join("plan.yaml"), "done: true\n").unwrap();
            delivered(&e, &rn, child).await;
        }
        assert!(out.join("children/dual/impl/plan.yaml").is_file());

        let work = e.rework(&run, "dual", Some(vec!["impl".into()]), "the implementation is wrong", by()).await.unwrap().work;
        let forks: Vec<_> = work.into_iter().filter(|w| matches!(w, WorkItem::Fork { .. })).collect();
        assert_eq!(forks.len(), 1, "{forks:?}");
        for w in forks {
            if let WorkItem::Fork { run, station, task, token, .. } = w {
                assert_eq!(task, "impl");
                run_fork(&rn, run, station, task, token).await;
            }
        }
        let rec = e.record(&run).await.unwrap();
        let again = rec.stations["dual"].fork.clone().unwrap();
        assert_eq!(again.round, first.round + 1);
        assert_ne!(again.commit, first.commit);
        assert_eq!(task_of(&rec, "dual", "tests").0, TaskStatus::Submitted, "the other's result kept");
        let three = child_of(&rec, "dual", "impl");
        assert_ne!(three, one);
        let c = e.record(&three).await.unwrap();
        let cw = PathBuf::from(&c.worktree.clone().unwrap().path);
        assert_eq!(c.base_commit.as_deref(), Some(again.commit.as_str()));
        // Both of the first round's merged in: the new one sees what it is to redo, and its sibling's.
        assert!(cw.join("src.txt").is_file() && cw.join("tests.txt").is_file());
        assert_eq!(again.children, [("impl".to_string(), three.clone())].into());
        assert_eq!(deny_read(&e, &c).await, vec![wt.clone()], "its sibling merged and gone: its parent's alone");
        let own = PathBuf::from(c.output_dir.unwrap());
        assert!(own.is_dir());
        assert_eq!(std::fs::read_dir(&own).unwrap().count(), 0, "not its predecessor's plan");
        let theirs = PathBuf::from(c.worktree.unwrap().fork.unwrap().parent_output_dir.unwrap());
        assert!(theirs.join("children/dual/tests/plan.yaml").is_file());
        assert!(out.join("children/dual/impl/plan.yaml").is_file(), "the parent's own copy untouched until the new one merges");
    }

    /// The second forked after a restart, the first long started: the first is kept from the
    /// second's worktree all the same, by the id its round took for it before either was made.
    #[tokio::test]
    async fn a_child_forked_first_is_kept_from_its_sibling_forked_after_a_restart() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let run = started(&rn, "p", p, None).await;
        let wt = PathBuf::from(e.record(&run).await.unwrap().worktree.unwrap().path);
        let work = hand_in(&e, &run, "spec", "spec").await;
        // The tests first; the implementation's fork never carried out.
        let Some(WorkItem::Fork { run: r1, station, task, token, .. }) = work.into_iter().find(|w| matches!(w, WorkItem::Fork { task, .. } if task == "tests")) else {
            panic!("no fork of the tests");
        };
        run_fork(&rn, r1, station, task, token).await;
        let rec = e.record(&run).await.unwrap();
        let two = child_of(&rec, "dual", "tests");
        let fork = rec.stations["dual"].fork.clone().unwrap();
        let one = fork.children["impl"].clone();
        assert_eq!(fork.children["tests"], two);
        let p1 = e.worktree_dir(&one).unwrap();
        assert!(!p1.exists(), "not made yet");
        assert_eq!(denied(&e, &two).await, sorted(vec![wt.clone(), p1.clone()]));

        // A restart: what is left of the round is handed out again, under the id taken for it.
        let rec = e.record(&run).await.unwrap();
        let again: Vec<WorkItem> = e.dispatched_work(&rec, e.clock().now_ms()).into_iter().filter(|w| matches!(w, WorkItem::Fork { .. })).collect();
        assert_eq!(again.len(), 1, "{again:?}");
        for w in again {
            if let WorkItem::Fork { run, station, task, token, .. } = w {
                assert_eq!(task, "impl");
                run_fork(&rn, run, station, task, token).await;
            }
        }
        let rec = e.record(&run).await.unwrap();
        assert_eq!(child_of(&rec, "dual", "impl"), one);
        assert_eq!(worktree_of(&e, &one).await, p1);
        assert_eq!(rec.stations["dual"].fork.clone().unwrap(), fork, "one fork commit, one set of ids");
        let p2 = worktree_of(&e, &two).await;
        assert_eq!(denied(&e, &one).await, sorted(vec![wt.clone(), p2.clone()]));
        assert_eq!(denied(&e, &two).await, sorted(vec![wt.clone(), p1.clone()]), "unchanged: fixed when it forked");
        // Merged and gone, the sibling stays in the list: a session opened again is opened as before.
        delivered(&e, &rn, &one).await;
        assert!(!p1.exists());
        assert_eq!(denied(&e, &two).await, sorted(vec![wt, p1]));
    }

    /// One task sent back while the other's child still goes: the new child is kept from its
    /// parent's worktree and from that sibling's, which goes on.
    #[tokio::test]
    async fn a_task_sent_back_while_its_sibling_goes_is_kept_from_the_siblings_worktree() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let run = started(&rn, "p", p, None).await;
        let rec = e.record(&run).await.unwrap();
        let (wt, out) = (PathBuf::from(&rec.worktree.unwrap().path), PathBuf::from(rec.output_dir.unwrap()));
        forked(&e, &rn, &run, &wt, &out).await;
        let rec = e.record(&run).await.unwrap();
        let (one, two) = (child_of(&rec, "dual", "impl"), child_of(&rec, "dual", "tests"));
        let work = e.rework(&run, "dual", Some(vec!["impl".into()]), "start the implementation over", by()).await.unwrap().work;
        // The new fork first, the old child's end after: neither order matters.
        let (forks, joins): (Vec<WorkItem>, Vec<WorkItem>) = work.into_iter().filter(|w| matches!(w, WorkItem::Fork { .. } | WorkItem::Join { .. })).partition(|w| matches!(w, WorkItem::Fork { .. }));
        assert_eq!((forks.len(), joins.len()), (1, 1), "{forks:?} {joins:?}");
        for w in forks.into_iter().chain(joins) {
            match w {
                WorkItem::Fork { run, station, task, token, .. } => run_fork(&rn, run, station, task, token).await,
                WorkItem::Join { run, station, task, child } => run_join(&rn, run, station, task, child).await,
                _ => {}
            }
        }
        let rec = e.record(&run).await.unwrap();
        let three = child_of(&rec, "dual", "impl");
        assert_ne!(three, one);
        let fork = rec.stations["dual"].fork.clone().unwrap();
        assert_eq!(fork.children, [("impl".to_string(), three.clone())].into(), "this round forks the implementation alone");
        let p2 = worktree_of(&e, &two).await;
        assert!(p2.is_dir(), "the tests go on");
        assert_eq!(denied(&e, &three).await, sorted(vec![wt.clone(), p2]));
        assert!(!worktree_of(&e, &one).await.exists(), "the one sent back is gone");
    }

    /// Children forked one at a time (`max_parallel`): the first is kept from the worktree of the
    /// one after it, which forks from the same commit under the id taken for it then.
    #[tokio::test]
    async fn a_later_wave_is_forked_under_the_id_its_round_took_for_it() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let input = CreateIn {
            flow: "w".into(),
            subject: "the subject".into(),
            inputs: json!({"items": ["one", "two"]}),
            project_root: Some(p.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let run = super::super::project::start(&rn, input, by()).await.unwrap().value["run"].as_str().unwrap().to_string();
        let rec = e.record(&run).await.unwrap();
        let (wt, out) = (PathBuf::from(&rec.worktree.unwrap().path), PathBuf::from(rec.output_dir.unwrap()));
        assert_eq!(forked(&e, &rn, &run, &wt, &out).await, 1, "one at a time");
        let rec = e.record(&run).await.unwrap();
        let st = &rec.stations["each"];
        let fork = st.fork.clone().unwrap();
        assert_eq!(fork.children.len(), 2, "{fork:?}");
        let first = st.tasks.iter().find_map(|t| t.child.clone()).unwrap();
        let (t2, later) = fork.children.iter().find(|(_, c)| **c != first).map(|(t, c)| (t.clone(), c.clone())).unwrap();
        assert_eq!(task_of(&rec, "each", &t2).0, TaskStatus::Ready);
        let p2 = e.worktree_dir(&later).unwrap();
        assert_eq!(denied(&e, &first).await, sorted(vec![wt.clone(), p2.clone()]));

        // The first through: the second forks, from the round's commit, where its id says.
        for w in hand_in(&e, &first, "a", "a").await {
            if let WorkItem::Join { run, station, task, child } = w {
                run_join(&rn, run, station, task, child).await;
            }
        }
        let rec = e.record(&run).await.unwrap();
        let next: Vec<WorkItem> = e.dispatched_work(&rec, e.clock().now_ms()).into_iter().filter(|w| matches!(w, WorkItem::Fork { .. })).collect();
        assert_eq!(next.len(), 1, "{next:?}");
        for w in next {
            if let WorkItem::Fork { run, station, task, token, .. } = w {
                run_fork(&rn, run, station, task, token).await;
            }
        }
        let rec = e.record(&run).await.unwrap();
        assert_eq!(child_of(&rec, "each", &t2), later);
        assert_eq!(worktree_of(&e, &later).await, p2);
        assert_eq!(e.record(&later).await.unwrap().base_commit.as_deref(), Some(fork.commit.as_str()));
        assert_eq!(rec.stations["each"].fork.clone().unwrap(), fork);
    }

    #[tokio::test]
    async fn a_child_that_conflicts_fails_its_task_and_keeps_its_branch() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let run = started(&rn, "p", p, None).await;
        let rec = e.record(&run).await.unwrap();
        let (wt, out) = (PathBuf::from(&rec.worktree.unwrap().path), PathBuf::from(rec.output_dir.unwrap()));
        forked(&e, &rn, &run, &wt, &out).await;
        let rec = e.record(&run).await.unwrap();
        let (one, two) = (child_of(&rec, "dual", "impl"), child_of(&rec, "dual", "tests"));
        let path = |c: &RunRecord| PathBuf::from(&c.worktree.as_ref().unwrap().path);
        let (w1, w2) = (path(&e.record(&one).await.unwrap()), path(&e.record(&two).await.unwrap()));
        std::fs::write(w1.join("a.txt"), "one\nimpl\nthree\n").unwrap();
        std::fs::write(w2.join("a.txt"), "one\ntests\nthree\n").unwrap();
        delivered(&e, &rn, &one).await;
        let head = git_out(&wt, &["rev-parse", "HEAD"]);
        delivered(&e, &rn, &two).await;

        let rec = e.record(&run).await.unwrap();
        let (status, error) = task_of(&rec, "dual", "tests");
        let error = error.unwrap();
        assert_eq!(status, TaskStatus::Failed);
        assert!(error.contains("a.txt: changed on both sides") && error.contains(&format!("conix/flow/{two}")), "{error}");
        // The parent's worktree as it was, the child's work on its branch, its worktree gone.
        assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "one\nimpl\nthree\n");
        assert_eq!(git_out(&wt, &["status", "--porcelain"]), "");
        let o = e.record(&two).await.unwrap().worktree.unwrap().outcome.unwrap();
        assert_eq!(o.kind, WorktreeEnd::Kept);
        assert_eq!(o.commit.unwrap(), git_out(p, &["rev-parse", &format!("conix/flow/{two}")]));
        assert_eq!(git_out(p, &["show", &format!("conix/flow/{two}:a.txt")]), "one\ntests\nthree");
        assert!(!w2.exists());
        let history = e.history(&two).await.unwrap();
        assert!(history.iter().any(|h| h.op == "worktree" && h.detail["failed"].as_str().is_some_and(|f| f.contains("a.txt"))));
    }

    #[tokio::test]
    async fn a_cancelled_child_keeps_its_branch_and_fails_its_task() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let run = started(&rn, "p", p, None).await;
        let rec = e.record(&run).await.unwrap();
        let (wt, out) = (PathBuf::from(&rec.worktree.unwrap().path), PathBuf::from(rec.output_dir.unwrap()));
        forked(&e, &rn, &run, &wt, &out).await;
        let one = child_of(&e.record(&run).await.unwrap(), "dual", "impl");
        let w1 = PathBuf::from(e.record(&one).await.unwrap().worktree.unwrap().path);
        std::fs::write(w1.join("half.txt"), "half done\n").unwrap();
        let work = e.cancel(&one, "not needed", by()).await.unwrap().work;
        // Its end waits on the runner: the parent's task has nothing yet.
        assert_eq!(task_of(&e.record(&run).await.unwrap(), "dual", "impl").0, TaskStatus::Claimed);
        for w in work {
            if let WorkItem::Join { run, station, task, child } = w {
                run_join(&rn, run, station, task, child).await;
            }
        }
        let (status, error) = task_of(&e.record(&run).await.unwrap(), "dual", "impl");
        assert_eq!((status, error.as_deref()), (TaskStatus::Failed, Some(format!("child run {one} was cancelled: not needed").as_str())));
        let o = e.record(&one).await.unwrap().worktree.unwrap().outcome.unwrap();
        assert_eq!(o.kind, WorktreeEnd::Kept);
        assert!(!w1.exists());
        assert_eq!(git_out(p, &["show", &format!("conix/flow/{one}:half.txt")]), "half done");
        assert!(!wt.join("half.txt").exists());
    }

    #[tokio::test]
    async fn after_a_restart_what_was_not_carried_out_is_handed_out_again() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        let run = started(&rn, "p", p, None).await;
        hand_in(&e, &run, "spec", "spec").await;
        let rec = e.record(&run).await.unwrap();
        let again = e.dispatched_work(&rec, e.clock().now_ms());
        let forks: Vec<&WorkItem> = again.iter().filter(|w| matches!(w, WorkItem::Fork { .. })).collect();
        assert_eq!(forks.len(), 2, "{again:?}");
        for w in again {
            if let WorkItem::Fork { run, station, task, token, .. } = w {
                run_fork(&rn, run.clone(), station.clone(), task.clone(), token.clone()).await;
                // Twice: nothing more.
                run_fork(&rn, run, station, task, token).await;
            }
        }
        let rec = e.record(&run).await.unwrap();
        let one = child_of(&rec, "dual", "impl");
        assert_eq!(std::fs::read_dir(p.join(".wkts")).unwrap().count(), 3, "the parent's and two children's");
        let joins = e.dispatched_work(&rec, e.clock().now_ms()).into_iter().filter(|w| matches!(w, WorkItem::Join { .. })).count();
        assert_eq!(joins, 2);
        // A join for a child still going does nothing.
        run_join(&rn, run.clone(), "dual".into(), "impl".into(), one.clone()).await;
        assert!(e.record(&one).await.unwrap().worktree.unwrap().outcome.is_none());
    }

    #[tokio::test]
    async fn a_parent_not_in_a_worktree_of_its_own_cannot_fork() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        // Said in the definition: refused when it is read.
        let problems = e
            .validate("{name: q, workspace: project, stations: [{id: d, executor: {kind: subflow, flow: c, workspace: worktree}, exit: [{kind: children}]}]}")
            .await;
        assert!(problems.iter().any(|x| x.message.contains("forks its parent's worktree")), "{problems:?}");
        let problems = e.validate("{name: q, stations: [{id: d, executor: {kind: subflow, flow: c, workspace: project}, exit: [{kind: children}]}]}").await;
        assert!(problems.iter().any(|x| x.message.contains("workspace project")), "{problems:?}");
        // Decided at the start: the task fails when it is dispatched, nothing committed.
        let run = started(&rn, "o", p, Some(Workspace::Project)).await;
        let head = git_out(p, &["rev-parse", "HEAD"]);
        let work = hand_in(&e, &run, "spec", "spec").await;
        assert!(!work.iter().any(|w| matches!(w, WorkItem::Fork { .. })));
        let (status, error) = task_of(&e.record(&run).await.unwrap(), "dual", "dual");
        assert_eq!(status, TaskStatus::Failed);
        assert!(error.unwrap().contains("not in a worktree of its own"));
        assert_eq!(git_out(p, &["rev-parse", "HEAD"]), head);
        assert!(!p.join(".wkts").exists());
    }

    /// A directory of runs git ignores (Conix's `conix/runs/`): what the parent wrote there is
    /// copied into the child's worktree at the fork, what the child wrote copied back.
    #[tokio::test]
    async fn an_output_directory_git_ignores_is_copied_in_and_back() {
        if !have_git() {
            return;
        }
        let r = repo();
        let p = r.path();
        let (e, rn) = setup(p);
        std::fs::write(p.join(".gitignore"), ".wkts/\nflows/\n").unwrap();
        commit_all(p, "ignore the runs");
        // `o` says nothing of where it works: this start says a worktree.
        let run = started(&rn, "o", p, Some(Workspace::Worktree)).await;
        let rec = e.record(&run).await.unwrap();
        let (wt, out) = (PathBuf::from(&rec.worktree.unwrap().path), PathBuf::from(rec.output_dir.unwrap()));
        std::fs::create_dir_all(out.join("_run")).unwrap();
        std::fs::write(out.join("_run/task.json"), "{}\n").unwrap();
        assert_eq!(forked(&e, &rn, &run, &wt, &out).await, 1);
        let rec = e.record(&run).await.unwrap();
        let fork = rec.stations["dual"].fork.clone().unwrap();
        assert!(!git_out(&wt, &["show", "--name-only", "--format=", &fork.commit]).contains("spec.md"), "git does not carry it");
        let child = child_of(&rec, "dual", "dual");
        let c = e.record(&child).await.unwrap();
        let theirs = PathBuf::from(c.worktree.unwrap().fork.unwrap().parent_output_dir.unwrap());
        assert_eq!(std::fs::read_to_string(theirs.join("spec.md")).unwrap(), "the spec\n");
        assert!(!theirs.join("_run").exists(), "never the process directory");
        let own = PathBuf::from(c.output_dir.unwrap());
        std::fs::write(own.join("report.md"), "done\n").unwrap();
        // What an earlier round left there is replaced.
        let back = out.join("children/dual/dual");
        std::fs::create_dir_all(&back).unwrap();
        std::fs::write(back.join("stale.md"), "old\n").unwrap();
        delivered(&e, &rn, &child).await;
        assert_eq!(std::fs::read_to_string(back.join("report.md")).unwrap(), "done\n");
        assert!(!back.join("stale.md").exists());
        assert_eq!(task_of(&e.record(&run).await.unwrap(), "dual", "dual").0, TaskStatus::Submitted);
    }
}
