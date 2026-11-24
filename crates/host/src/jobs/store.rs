//! 作业的存取（architecture §8.5）：定义在 `.conix/jobs.json`，随项目提交，项目受信才读、才写；
//! 启用与否、调度基线在 `state/conix/jobs-state.json`，只属于这台机器；触发记录是
//! `state/conix/jobs/<id>.jsonl`。

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::Write;
use std::path::PathBuf;

use client::ProjectState;
use port::{AutoDisabled, Job, JobEntry, JobGuard, JobId, JobKind, JobOutcome, OutcomeKind, Usage};
use serde::{Deserialize, Serialize};

use super::cron;

/// 随项目提交的那一份：团队共用，不带这台机器的启停与基线。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct JobDef {
    id: JobId,
    name: String,
    cron: String,
    kind: JobKind,
    #[serde(default)]
    catch_up: bool,
    #[serde(default)]
    created_ms: u64,
    #[serde(default, skip_serializing_if = "JobGuard::is_empty")]
    guard: JobGuard,
}

/// 这台机器上的状态。没有记录的作业是停用的：克隆下来的仓库带着作业，不能拉下代码就跑起来。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
struct Local {
    enabled: bool,
    #[serde(default)]
    last_fired_ms: Option<u64>,
    /// 没触发过时的基线：在这台机器上新建、改了 cron、恢复启用的时刻。
    #[serde(default)]
    since_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    auto_disabled: Option<AutoDisabled>,
}

/// 内存里合起来的一条。`Job` 的 `next_fire_ms` 与 `last` 是派生值，这里不用。
#[derive(Debug, Clone, PartialEq)]
pub struct StoredJob {
    pub job: Job,
    pub last_fired_ms: Option<u64>,
    pub since_ms: u64,
}

impl StoredJob {
    /// 调度基线：上次触发，没触发过就是 `since_ms`。
    pub fn base_ms(&self) -> u64 {
        self.last_fired_ms.unwrap_or(self.since_ms)
    }

    /// 按基线算的下一次触发；已经到期（待触发或待补跑）时照实返回过去的时刻。
    pub fn due_ms(&self) -> Option<u64> {
        if !self.job.enabled {
            return None;
        }
        cron::next_after(&self.job.cron, self.base_ms())
    }

    fn def(&self) -> JobDef {
        JobDef {
            id: self.job.id.clone(),
            name: self.job.name.clone(),
            cron: self.job.cron.clone(),
            kind: self.job.kind.clone(),
            catch_up: self.job.catch_up,
            created_ms: self.job.created_ms,
            guard: self.job.guard.clone(),
        }
    }

    fn local(&self) -> Local {
        Local {
            enabled: self.job.enabled,
            last_fired_ms: self.last_fired_ms,
            since_ms: self.since_ms,
            auto_disabled: self.job.auto_disabled.clone(),
        }
    }
}

const UNTRUSTED: &str =
    "the project is not trusted: its job definitions are not read or saved until it is (/trust)";

pub struct JobStore {
    guard: super::guard::GuardConfig,
    ledger: super::ledger::Ledger,
    defs_path: PathBuf,
    state_path: PathBuf,
    history_dir: PathBuf,
    trusted: bool,
    jobs: Vec<StoredJob>,
}

impl JobStore {
    /// 起初当项目不受信、什么都不读：信任要问过引擎才知道（[`JobStore::set_trusted`]）。
    pub fn open(
        defs_path: impl Into<PathBuf>,
        state_path: impl Into<PathBuf>,
        history_dir: impl Into<PathBuf>,
    ) -> JobStore {
        let state_path: PathBuf = state_path.into();
        // 用量账与本机状态放在一起：`state/conix/usage/`。
        let usage = state_path
            .parent()
            .map(|p| p.join("usage"))
            .unwrap_or_else(|| PathBuf::from("usage"));
        JobStore {
            guard: Default::default(),
            ledger: super::ledger::Ledger::new(usage),
            defs_path: defs_path.into(),
            state_path,
            history_dir: history_dir.into(),
            trusted: false,
            jobs: vec![],
        }
    }

    pub fn open_in(state: &ProjectState) -> JobStore {
        JobStore::open(state.jobs(), state.jobs_state(), state.job_history_dir())
    }

    pub fn set_guard_config(&mut self, g: super::guard::GuardConfig) {
        self.guard = g;
    }

    pub fn guard_config(&self) -> &super::guard::GuardConfig {
        &self.guard
    }

    pub fn ledger(&self) -> &super::ledger::Ledger {
        &self.ledger
    }

    pub fn trusted(&self) -> bool {
        self.trusted
    }

    /// 信任变了：受信就从盘上重读定义，不受信就一个都不留。
    pub fn set_trusted(&mut self, trusted: bool) {
        self.trusted = trusted;
        self.jobs.clear();
        if !trusted {
            return;
        }
        let defs: Vec<JobDef> = fs::read(&self.defs_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        let mut local: BTreeMap<String, Local> = fs::read(&self.state_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        for d in defs.into_iter().filter(|d| valid_id(d.id.as_str())) {
            let l = local.remove(d.id.as_str()).unwrap_or_default();
            self.jobs.push(StoredJob {
                job: Job {
                    id: d.id,
                    name: d.name,
                    cron: d.cron,
                    kind: d.kind,
                    enabled: l.enabled,
                    catch_up: d.catch_up,
                    created_ms: d.created_ms,
                    guard: d.guard,
                    auto_disabled: l.auto_disabled,
                    effective_guard: None,
                    next_fire_ms: None,
                    last: None,
                    running_ms: None,
                    today_tokens: 0,
                },
                last_fired_ms: l.last_fired_ms,
                since_ms: l.since_ms,
            });
        }
    }

    fn history_path(&self, id: &str) -> PathBuf {
        self.history_dir.join(format!("{id}.jsonl"))
    }

    fn save_defs(&self) -> Result<(), String> {
        let defs: Vec<JobDef> = self.jobs.iter().map(StoredJob::def).collect();
        write_atomic(&self.defs_path, &defs)
    }

    fn save_local(&self) -> Result<(), String> {
        let local: BTreeMap<&str, Local> = self
            .jobs
            .iter()
            .map(|s| (s.job.id.as_str(), s.local()))
            .collect();
        write_atomic(&self.state_path, &local)
    }

    fn decorate(&self, s: &StoredJob, now_ms: u64) -> Job {
        let mut job = s.job.clone();
        job.next_fire_ms = if job.enabled {
            cron::next_after(&job.cron, now_ms)
        } else {
            None
        };
        job.effective_guard = Some(self.guard.effective(&job.guard).0);
        job.today_tokens = self
            .ledger
            .of(&super::ledger::day_of(now_ms), job.id.as_str());
        let recent = self.history(job.id.as_str(), 1);
        job.last = recent.first().map(|e| e.outcome.clone());
        job.running_ms = recent
            .first()
            .filter(|e| e.outcome == JobOutcome::Running)
            .map(|e| e.fired_ms);
        job
    }

    pub fn list(&self, now_ms: u64) -> Vec<Job> {
        self.jobs.iter().map(|s| self.decorate(s, now_ms)).collect()
    }

    pub fn get(&self, id: &str, now_ms: u64) -> Option<Job> {
        self.jobs
            .iter()
            .find(|s| s.job.id.as_str() == id)
            .map(|s| self.decorate(s, now_ms))
    }

    /// 调度器用：不带派生字段的原始记录。
    pub fn stored(&self) -> &[StoredJob] {
        &self.jobs
    }

    /// 有触发记录的作业：定义读不到（项目不受信、作业被别人删了）时也要能收拾上次没跑完的触发。
    pub fn history_ids(&self) -> Vec<String> {
        fs::read_dir(&self.history_dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                name.strip_suffix(".jsonl").map(str::to_owned)
            })
            .filter(|id| valid_id(id))
            .collect()
    }

    /// 新建或覆盖。`now_ms` 由调用方给：新建时记作创建时刻；新建、改了 cron 或恢复启用时用作补跑基线，
    /// 存储层自己读时钟的话测试没法控制时间。
    pub fn upsert(&mut self, mut job: Job, now_ms: u64) -> Result<Job, String> {
        if !self.trusted {
            return Err(UNTRUSTED.into());
        }
        cron::validate(&job.cron)?;
        if job.id.as_str().is_empty() {
            job.id = new_id();
        } else if !valid_id(job.id.as_str()) {
            return Err(format!("invalid job id: {}", job.id));
        }
        job.next_fire_ms = None;
        job.last = None;
        job.running_ms = None;
        match self.jobs.iter_mut().find(|s| s.job.id == job.id) {
            Some(s) => {
                // cron 变了，旧基线下"错过"的触发已经没有意义。
                // 从停用恢复同理：停用期间的触发不算错过。
                if s.job.cron != job.cron || (!s.job.enabled && job.enabled) {
                    s.last_fired_ms = None;
                    s.since_ms = now_ms;
                }
                job.created_ms = s.job.created_ms;
                // 自动停用是这台机器上的事：改定义不清它，启用才清。
                job.auto_disabled = if job.enabled && !s.job.enabled {
                    None
                } else {
                    s.job.auto_disabled.clone()
                };
                s.job = job.clone();
            }
            None => {
                job.created_ms = now_ms;
                self.jobs.push(StoredJob {
                    job: job.clone(),
                    last_fired_ms: None,
                    since_ms: now_ms,
                })
            }
        }
        self.save_defs()?;
        self.save_local()?;
        let s = self.jobs.iter().find(|s| s.job.id == job.id).unwrap();
        Ok(self.decorate(s, now_ms))
    }

    pub fn delete(&mut self, id: &str) -> Result<bool, String> {
        if !self.trusted {
            return Err(UNTRUSTED.into());
        }
        let before = self.jobs.len();
        self.jobs.retain(|s| s.job.id.as_str() != id);
        let removed = self.jobs.len() != before;
        if removed {
            self.save_defs()?;
            self.save_local()?;
            let _ = fs::remove_file(self.history_path(id));
        }
        Ok(removed)
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool, now_ms: u64) -> Result<bool, String> {
        if !self.trusted {
            return Err(UNTRUSTED.into());
        }
        let Some(s) = self.jobs.iter_mut().find(|s| s.job.id.as_str() == id) else {
            return Ok(false);
        };
        if enabled && !s.job.enabled {
            s.last_fired_ms = None;
            s.since_ms = now_ms;
        }
        if enabled {
            s.job.auto_disabled = None;
        }
        s.job.enabled = enabled;
        self.save_local()?;
        Ok(true)
    }

    /// 连续失败到了上限：在这台机器上停用，记下来（启用时清掉）。
    pub fn auto_disable(&mut self, id: &str, after: u32, now_ms: u64) -> Result<bool, String> {
        let Some(s) = self.jobs.iter_mut().find(|s| s.job.id.as_str() == id) else {
            return Ok(false);
        };
        s.job.enabled = false;
        s.job.auto_disabled = Some(AutoDisabled {
            at_ms: now_ms,
            after,
        });
        self.save_local()?;
        Ok(true)
    }

    /// 最近连着几次没成（`Failed` / `Stopped`）；跳过的不算，也不打断。
    pub fn failure_streak(&self, id: &str) -> u32 {
        let mut n = 0;
        for e in self.history(id, 100) {
            match &e.outcome {
                o if o.is_failure() => n += 1,
                JobOutcome::Skipped { .. } | JobOutcome::Running => {}
                _ => break,
            }
        }
        n
    }

    pub fn mark_fired(&mut self, id: &str, fired_ms: u64) -> Result<(), String> {
        if let Some(s) = self.jobs.iter_mut().find(|s| s.job.id.as_str() == id) {
            s.last_fired_ms = Some(fired_ms);
            self.save_local()?;
        }
        Ok(())
    }

    pub fn append(&self, id: &str, entry: &JobEntry) -> Result<(), String> {
        self.append_line(id, entry)?;
        self.compact_if_big(id)
    }

    /// 行数到了 `HISTORY_KEEP` 的两倍就只留最近 `HISTORY_KEEP` 次（同一次的多行先合成一行），整份
    /// 重写再改名。追加之后才看：触发记录只追加，崩溃时才安全；压缩是另起一份文件换上去，同样不会写坏。
    /// 按行数而不是字节数：一次的记录可长可短，按字节会在一份正常大小的历史上反复压缩。
    fn compact_if_big(&self, id: &str) -> Result<(), String> {
        let path = self.history_path(id);
        let lines = fs::read(&path)
            .map(|b| b.iter().filter(|c| **c == b'\n').count())
            .unwrap_or(0);
        if lines <= 2 * HISTORY_KEEP {
            return Ok(());
        }
        let mut keep = self.history(id, HISTORY_KEEP);
        keep.reverse();
        let mut text = String::new();
        for e in &keep {
            text.push_str(&serde_json::to_string(e).map_err(|e| e.to_string())?);
            text.push('\n');
        }
        let tmp = path.with_extension("jsonl.tmp");
        fs::write(&tmp, text).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    fn append_line(&self, id: &str, entry: &JobEntry) -> Result<(), String> {
        fs::create_dir_all(&self.history_dir).map_err(|e| e.to_string())?;
        let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.history_path(id))
            .map_err(|e| e.to_string())?;
        f.write_all(line.as_bytes()).map_err(|e| e.to_string())
    }

    /// 结束一次触发：追加同 `fired_ms` 的新行，读的时候后者覆盖前者。
    /// 不改写旧行，追加在崩溃时才安全；`target` 沿用 `Running` 行里记的。
    pub fn finish(
        &self,
        id: &str,
        fired_ms: u64,
        outcome: JobOutcome,
        elapsed_ms: u64,
        usage: Usage,
        audit: Option<port::FiringAudit>,
    ) -> Result<(), String> {
        let target = self
            .history(id, usize::MAX)
            .into_iter()
            .find(|e| e.fired_ms == fired_ms)
            .and_then(|e| e.target);
        self.append(
            id,
            &JobEntry {
                fired_ms,
                outcome,
                elapsed_ms,
                usage,
                target,
                audit,
            },
        )
    }

    /// 一页触发记录：新的在前，比 `before_ms` 早的，按结局筛。
    pub fn history_page(
        &self,
        id: &str,
        before_ms: Option<u64>,
        limit: usize,
        outcome: Option<OutcomeKind>,
    ) -> Vec<JobEntry> {
        self.history(id, usize::MAX)
            .into_iter()
            .filter(|e| before_ms.is_none_or(|b| e.fired_ms < b))
            .filter(|e| outcome.is_none_or(|k| e.outcome.kind() == k))
            .take(limit)
            .collect()
    }

    /// 新的在前；同一个 `fired_ms` 取最后一行。坏行跳过。
    pub fn history(&self, id: &str, limit: usize) -> Vec<JobEntry> {
        let Ok(text) = fs::read_to_string(self.history_path(id)) else {
            return Vec::new();
        };
        let mut by_fired: HashMap<u64, JobEntry> = HashMap::new();
        for line in text.lines() {
            if let Ok(e) = serde_json::from_str::<JobEntry>(line) {
                by_fired.insert(e.fired_ms, e);
            }
        }
        let mut out: Vec<JobEntry> = by_fired.into_values().collect();
        out.sort_by_key(|e| std::cmp::Reverse(e.fired_ms));
        out.truncate(limit);
        out
    }
}

/// 每个作业留多少次触发记录。
const HISTORY_KEEP: usize = 500;

/// 随机的短 ID：定义进 git，几个人各自新建，按序号编的会撞。
fn new_id() -> JobId {
    let hex = uuid::Uuid::new_v4().simple().to_string();
    JobId(format!("j{}", &hex[..8]))
}

/// ID 拼进触发记录的文件名：定义来自仓库，不能带路径。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 先写临时文件再改名：写到一半崩了也不会留下截断的文件。
fn write_atomic(path: &std::path::Path, value: &impl Serialize) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    fs::write(&tmp, bytes).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use port::JobKind;
    use std::path::Path;

    fn job(id: &str, cron: &str) -> Job {
        Job {
            id: JobId(id.into()),
            name: id.into(),
            cron: cron.into(),
            kind: JobKind::Simple {
                prompt: "hi".into(),
                skills: vec![],
                project: None,
                scene: None,
                model: None,
            },
            enabled: true,
            catch_up: false,
            created_ms: 0,
            guard: Default::default(),
            auto_disabled: None,
            effective_guard: None,
            next_fire_ms: None,
            last: None,
            running_ms: None,
            today_tokens: 0,
        }
    }

    fn entry(fired_ms: u64, outcome: JobOutcome) -> JobEntry {
        JobEntry {
            fired_ms,
            outcome,
            elapsed_ms: 0,
            usage: Usage::default(),
            target: None,
            audit: None,
        }
    }

    fn open_untrusted(dir: &Path) -> JobStore {
        JobStore::open(
            dir.join("jobs.json"),
            dir.join("state/jobs-state.json"),
            dir.join("state/jobs"),
        )
    }

    fn open(dir: &Path) -> JobStore {
        let mut s = open_untrusted(dir);
        s.set_trusted(true);
        s
    }

    fn read(path: &Path) -> serde_json::Value {
        serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
    }

    #[test]
    fn round_trip_and_derived_fields() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        let saved = s.upsert(job("a", "* * * * *"), 1_000).unwrap();
        assert_eq!(saved.next_fire_ms, Some(60_000));
        s.mark_fired("a", 60_000).unwrap();
        let s2 = open(d.path());
        assert_eq!(s2.stored()[0].last_fired_ms, Some(60_000));
        assert_eq!(s2.stored()[0].since_ms, 1_000);
        assert_eq!(s2.stored()[0].job.created_ms, 1_000);
        assert_eq!(s2.stored()[0].job.next_fire_ms, None);
        assert_eq!(s2.list(61_000)[0].next_fire_ms, Some(120_000));
        assert!(s2.get("a", 0).is_some());
        assert!(s2.get("zz", 0).is_none());
    }

    #[test]
    fn upsert_validates_and_generates_id() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        assert!(s.upsert(job("a", "bad"), 0).is_err());
        assert!(s.upsert(job("../x", "* * * * *"), 0).is_err());
        let j1 = s.upsert(job("", "* * * * *"), 5).unwrap();
        let j2 = s.upsert(job("", "* * * * *"), 5).unwrap();
        assert!(j1.id.as_str().starts_with('j') && j1.id.as_str().len() == 9);
        assert_ne!(j1.id, j2.id);
        assert_eq!(j1.created_ms, 5);
    }

    #[test]
    fn disabled_has_no_next() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        assert!(s.set_enabled("a", false, 0).unwrap());
        assert_eq!(s.list(0)[0].next_fire_ms, None);
        assert!(!s.set_enabled("nope", true, 0).unwrap());
    }

    #[test]
    fn history_last_line_wins_and_corrupt_skipped() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        s.append("a", &entry(100, JobOutcome::Running)).unwrap();
        s.append("a", &entry(200, JobOutcome::Running)).unwrap();
        s.finish("a", 100, JobOutcome::Done, 7, Usage::default(), None)
            .unwrap();
        let mut f = fs::OpenOptions::new()
            .append(true)
            .open(d.path().join("state/jobs/a.jsonl"))
            .unwrap();
        f.write_all(b"{garbage\n").unwrap();
        let h = s.history("a", 10);
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].fired_ms, 200);
        assert_eq!(h[0].outcome, JobOutcome::Running);
        assert_eq!(h[1].outcome, JobOutcome::Done);
        assert_eq!(h[1].elapsed_ms, 7);
        assert_eq!(s.history("a", 1).len(), 1);
        assert_eq!(s.list(0)[0].last, Some(JobOutcome::Running));
    }

    #[test]
    fn delete_removes_history() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        s.append("a", &entry(1, JobOutcome::Done)).unwrap();
        assert!(d.path().join("state/jobs/a.jsonl").exists());
        assert!(s.delete("a").unwrap());
        assert!(!d.path().join("state/jobs/a.jsonl").exists());
        assert!(!s.delete("a").unwrap());
        assert!(open(d.path()).stored().is_empty());
    }

    /// 定义随项目提交，不带这台机器的启停与基线；那些在本机状态里。
    #[test]
    fn definitions_and_local_state_are_kept_apart() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 1_000).unwrap();
        s.mark_fired("a", 60_000).unwrap();
        let defs = read(&d.path().join("jobs.json"));
        assert_eq!(defs[0]["id"], "a");
        assert_eq!(defs[0]["created_ms"], 1_000);
        assert!(defs[0].get("enabled").is_none() && defs[0].get("last_fired_ms").is_none());
        let local = read(&d.path().join("state/jobs-state.json"));
        assert_eq!(local["a"]["enabled"], true);
        assert_eq!(local["a"]["last_fired_ms"], 60_000);
    }

    /// 克隆下来的仓库带着作业：这台机器没有它们的状态，一律停用。
    #[test]
    fn jobs_without_local_state_are_disabled() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("jobs.json"),
            r#"[{"id":"x","name":"x","cron":"* * * * *","kind":{"simple":{"prompt":"hi"}}}]"#,
        )
        .unwrap();
        let s = open(d.path());
        let j = s.get("x", 0).unwrap();
        assert!(!j.enabled);
        assert_eq!(j.next_fire_ms, None);
        assert!(s.stored()[0].due_ms().is_none());
    }

    #[test]
    fn an_untrusted_project_has_no_jobs_and_saves_none() {
        let d = tempfile::tempdir().unwrap();
        open(d.path()).upsert(job("a", "* * * * *"), 0).unwrap();
        let mut s = open_untrusted(d.path());
        assert!(s.list(0).is_empty());
        assert!(s.upsert(job("b", "* * * * *"), 0).is_err());
        assert!(s.delete("a").is_err());
        assert!(s.set_enabled("a", false, 0).is_err());
        s.set_trusted(true);
        assert_eq!(s.list(0).len(), 1);
        s.set_trusted(false);
        assert!(s.list(0).is_empty());
    }

    /// ID 来自仓库里的文件，拼进触发记录的文件名：带路径的不认。
    #[test]
    fn definitions_with_path_like_ids_are_ignored() {
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("jobs.json"),
            r#"[{"id":"../../x","name":"x","cron":"* * * * *","kind":{"simple":{"prompt":"hi"}}}]"#,
        )
        .unwrap();
        assert!(open(d.path()).list(0).is_empty());
    }

    /// 触发记录不会一直长：文件大了就压回最近 500 次。
    #[test]
    fn history_is_capped_when_the_file_grows() {
        let d = tempfile::tempdir().unwrap();
        let mut s = open(d.path());
        s.upsert(job("a", "* * * * *"), 0).unwrap();
        let mut e = entry(0, JobOutcome::Done);
        e.target = Some(port::JobTarget::Session {
            id: port::SessionId("x".repeat(200)),
            title: "y".repeat(200),
        });
        for i in 0..1_100 {
            e.fired_ms = i;
            s.append("a", &e).unwrap();
        }
        let h = s.history("a", usize::MAX);
        assert!(h.len() <= 2 * HISTORY_KEEP, "压过：{}", h.len());
        assert!(h.len() >= HISTORY_KEEP);
        assert_eq!(h[0].fired_ms, 1_099, "留的是最近的");
    }
}
