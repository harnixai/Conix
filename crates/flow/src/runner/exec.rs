//! Starting a process for a project or a bundle (in Conix; docs/design/flows.md §7.14): the one
//! way the library does it. Command gates, the runs of a plan and `flow.exec` build an
//! [`ExecRequest`] — the profile fixed by where the call comes from — and hand it to the host's
//! [`Exec`] through [`run_audited`], which records what ran: a line of the run's history in a run,
//! a line of the log outside one. Never what it printed: output is the caller's to keep or not.
//!
//! [`PlainExec`] is the library's own backend, a plain child process: nothing of the profile is
//! enforced, and every outcome says so (`Enforcement::none`). A host that can confine a process
//! (a sandbox profile, a namespace) gives its own `Exec`, and may still start the process itself
//! with [`run_plain`] once it has wrapped the request.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::kernel::engine::Engine;
use crate::kernel::host::{Enforcement, Exec, ExecError, ExecOutcome, ExecProfile, ExecRequest, OutputKeep};
use crate::kernel::run::RunRecord;

/// How long the output of a process killed for its time is still read: what it printed up to
/// then, unless something it left behind holds the pipes open.
const DRAIN_AFTER_KILL: Duration = Duration::from_secs(2);

/// The library's backend: a plain child process, nothing confined.
pub struct PlainExec;

#[async_trait]
impl Exec for PlainExec {
    async fn run(&self, req: ExecRequest) -> Result<ExecOutcome, ExecError> {
        run_plain(req, Enforcement::none("plain")).await
    }
}

/// Start `req` as a child process and wait for it: its own process group (killed whole when it
/// runs out of time), its environment only what the request gives unless it inherits, stdin what
/// it gives or nothing, each output stream kept as `keep` says. `enforced` is what the caller made
/// of the profile before (a backend that wrapped `argv` in a confinement says what it gave).
pub async fn run_plain(req: ExecRequest, enforced: Enforcement) -> Result<ExecOutcome, ExecError> {
    use std::process::Stdio;
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
    let Some((program, args)) = req.argv.split_first() else {
        return Err(ExecError::NotStarted("argv is empty: it is the program and its arguments".into()));
    };
    let mut cmd = tokio::process::Command::new(program);
    cmd.args(args).current_dir(&req.cwd);
    if !req.inherit_env {
        cmd.env_clear();
    }
    cmd.envs(req.env.iter().map(|(k, v)| (k, v)))
        .stdin(if req.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| ExecError::NotStarted(format!("{program}: {e}")))?;
    let pid = child.id();
    let feed = {
        let (text, pipe) = (req.stdin, child.stdin.take());
        // Written beside the reading: a process that answers as it reads would fill its pipe and
        // wait for us while we wait for it.
        async move {
            if let (Some(t), Some(mut p)) = (text, pipe) {
                let _ = p.write_all(&t).await;
                let _ = p.shutdown().await;
            }
        }
    };
    let keep = req.keep;
    let read = |r: Option<Box<dyn AsyncRead + Unpin + Send>>| async move {
        let mut kept = Kept::new(keep);
        let Some(mut r) = r else { return kept };
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf).await {
            if n == 0 {
                break;
            }
            kept.push(&buf[..n]);
        }
        kept
    };
    let out = read(child.stdout.take().map(|o| Box::new(o) as _));
    let err = read(child.stderr.take().map(|o| Box::new(o) as _));
    let all = async {
        let ((), o, e, status) = tokio::join!(feed, out, err, child.wait());
        (o, e, status)
    };
    tokio::pin!(all);
    let (o, e, status) = tokio::select! {
        r = &mut all => {
            let (o, e, status) = r;
            (o, e, Some(status))
        }
        _ = tokio::time::sleep(req.timeout) => {
            // The whole group: a test runner's children too.
            kill_group(pid);
            match tokio::time::timeout(DRAIN_AFTER_KILL, &mut all).await {
                Ok((o, e, _)) => (o, e, None),
                Err(_) => (Kept::new(keep), Kept::new(keep), None),
            }
        }
    };
    let ms = started.elapsed().as_millis() as u64;
    let (code, signal, timed_out) = match status {
        None => (None, None, true),
        Some(Ok(s)) => (s.code(), signal_of(&s), false),
        Some(Err(e)) => return Err(ExecError::Lost(e.to_string())),
    };
    let truncated = o.cut || e.cut;
    Ok(ExecOutcome { code, signal, stdout: o.finish(), stderr: e.finish(), timed_out, truncated, ms, enforced })
}

#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid.and_then(|p| i32::try_from(p).ok()) {
        // SAFETY: a signal sent to a process group this process made; no memory is touched.
        unsafe {
            libc::kill(-pid, libc::SIGKILL);
        }
    }
}

#[cfg(not(unix))]
fn kill_group(_: Option<u32>) {}

#[cfg(unix)]
fn signal_of(s: &std::process::ExitStatus) -> Option<i32> {
    std::os::unix::process::ExitStatusExt::signal(s)
}

#[cfg(not(unix))]
fn signal_of(_: &std::process::ExitStatus) -> Option<i32> {
    None
}

/// One output stream as it is read: its head or its tail up to the limit, and whether more came.
struct Kept {
    keep: OutputKeep,
    bytes: Vec<u8>,
    cut: bool,
}

impl Kept {
    fn new(keep: OutputKeep) -> Self {
        Self { keep, bytes: vec![], cut: false }
    }

    fn push(&mut self, data: &[u8]) {
        match self.keep {
            OutputKeep::Head(n) => {
                let room = n.saturating_sub(self.bytes.len());
                self.bytes.extend_from_slice(&data[..room.min(data.len())]);
                self.cut |= data.len() > room;
            }
            OutputKeep::Tail(n) => {
                self.bytes.extend_from_slice(data);
                // Drained in steps, not on every read: a long output is not copied byte by byte.
                if self.bytes.len() > 2 * n {
                    self.bytes.drain(..self.bytes.len() - n);
                    self.cut = true;
                }
            }
        }
    }

    fn finish(mut self) -> Vec<u8> {
        if let OutputKeep::Tail(n) = self.keep {
            if self.bytes.len() > n {
                self.bytes.drain(..self.bytes.len() - n);
            }
        }
        self.bytes
    }
}

/// Where the record of a process goes, and what it was for (`gate semgrep`, `flow.exec in
/// commands/xreport.js`).
#[derive(Clone)]
pub(crate) struct Audit {
    /// In a run: its history. `None`: the log.
    pub run: Option<(Arc<Engine>, String)>,
    pub station: Option<String>,
    pub task: Option<String>,
    pub what: String,
}

impl Audit {
    /// Into the log: a call outside any run (a trial, a doctor, a command run for a person).
    pub fn log(what: impl Into<String>) -> Self {
        Self { run: None, station: None, task: None, what: what.into() }
    }

    /// Into the history of `run`.
    pub fn run(engine: Arc<Engine>, run: impl Into<String>, station: Option<&str>, task: Option<&str>, what: impl Into<String>) -> Self {
        Self { run: Some((engine, run.into())), station: station.map(String::from), task: task.map(String::from), what: what.into() }
    }

    /// The same place, for something else.
    pub fn for_(&self, what: impl Into<String>) -> Self {
        Self { what: what.into(), ..self.clone() }
    }
}

/// What the record of a process says of a request and how it ended (`what`: what it was for): the
/// program and its arguments, where, under what, how it ended, what the backend enforced. Not what
/// it printed. A host that starts processes of its own records them in the same shape.
pub fn exec_record(what: &str, req: &ExecRequest, got: &Result<ExecOutcome, ExecError>) -> Value {
    let mut line = json!({
        "for": what,
        "argv": req.argv,
        "cwd": req.cwd.to_string_lossy(),
        "profile": req.profile,
        "network": req.network,
    });
    match got {
        Ok(o) => {
            line["code"] = json!(o.code);
            if o.signal.is_some() {
                line["signal"] = json!(o.signal);
            }
            if o.timed_out {
                line["timed_out"] = json!(true);
            }
            if o.truncated {
                line["truncated"] = json!(true);
            }
            line["ms"] = json!(o.ms);
            line["enforced"] = json!(o.enforced);
        }
        Err(e) => line["error"] = json!(e.to_string()),
    }
    line
}

/// Run `req` through `exec` and record it where `audit` says. A record that cannot be written is
/// a warning, never the process's failure: it has run by then.
pub(crate) async fn run_audited(exec: &dyn Exec, req: ExecRequest, audit: &Audit) -> Result<ExecOutcome, ExecError> {
    let got = exec.run(req.clone()).await;
    let line = exec_record(&audit.what, &req, &got);
    match &audit.run {
        Some((engine, run)) => {
            if let Err(e) = engine.note_exec(run, audit.station.as_deref(), audit.task.as_deref(), line.clone()).await {
                tracing::warn!(run, error = %e, exec = %line, "a process's record not written to its run");
            }
        }
        None => tracing::info!(target: "flow::exec", exec = %line, "process run"),
    }
    got
}

/// The environment a process of a project or a bundle gets: none of this process's but the basics,
/// then what `wanted` names (a name may end in `*`) less what the application denies.
pub(crate) fn passed_env(wanted: &[String], deny: &[String]) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let mut out: Vec<(std::ffi::OsString, std::ffi::OsString)> =
        BASE_ENV.iter().filter_map(|k| std::env::var_os(k).map(|v| ((*k).into(), v))).collect();
    out.extend(named_env(wanted, deny, std::env::vars_os()));
    out
}

/// The variables of `from` that `wanted` names, less those `deny` names. A name ending in `*` is a
/// prefix, one starting with `*` a suffix: a host denies `*_API_KEY` whoever's key it is.
pub(crate) fn named_env(
    wanted: &[String],
    deny: &[String],
    from: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let named = |list: &[String], k: &str| {
        list.iter().any(|w| match (w.strip_suffix('*'), w.strip_prefix('*')) {
            (Some(prefix), _) => k.starts_with(prefix),
            (None, Some(suffix)) => k.ends_with(suffix),
            (None, None) => w == k,
        })
    };
    from.filter(|(k, _)| {
        let k = k.to_string_lossy();
        named(wanted, &k) && !named(deny, &k)
    })
    .collect()
}

/// What a process of a project or a bundle keeps of this one's environment.
pub(crate) const BASE_ENV: &[&str] = &["PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "TERM", "SHELL"];

/// `dir` as a directory a profile names: canonical where it exists, so a backend compares like
/// with like.
pub(crate) fn writable_dir(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}

/// The profile of a call that may write: these directories, those that are given.
pub(crate) fn workspace_profile(dirs: impl IntoIterator<Item = PathBuf>) -> ExecProfile {
    let mut writable: Vec<PathBuf> = dirs.into_iter().map(|d| writable_dir(&d)).collect();
    writable.sort();
    writable.dedup();
    ExecProfile::Workspace { writable }
}

/// Where a call of a command for run `rec` may write, as a profile: the run's directory, its
/// bundle's data directory, and — for a station's task — the directories its `writes` name (the
/// part of each glob before its first wildcard: a backend grants directories, the glob itself is
/// the writer's and the session's to hold to).
pub(crate) fn command_profile(rec: &RunRecord, ws: &Path, station: Option<&str>, task: Option<&str>) -> ExecProfile {
    let mut dirs: Vec<PathBuf> = [&rec.output_dir, &rec.data_dir].into_iter().flatten().map(PathBuf::from).collect();
    if let Some(st) = station.and_then(|s| rec.def.station(s)).filter(|st| !st.writes.is_empty()) {
        let item = task.and_then(|t| rec.stations.get(&st.id)?.tasks.iter().find(|x| x.id == t)?.item.as_ref());
        let ctx = crate::kernel::refs::context_in(rec, item, task, ws.to_str());
        let scope = crate::kernel::parts::resolve_writes(&st.writes, &ctx, rec.output_dir.as_deref());
        for g in scope.globs {
            let fixed: Vec<&str> = g.split('/').take_while(|c| !c.contains(['*', '?', '[', '{'])).collect();
            let p = PathBuf::from(fixed.join("/"));
            dirs.push(if p.is_absolute() { p } else { ws.join(p) });
        }
    }
    workspace_profile(dirs)
}

/// A backend that runs as [`PlainExec`] does and keeps every request it was given, for tests.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct Recording(pub std::sync::Mutex<Vec<ExecRequest>>);

#[cfg(test)]
impl Recording {
    pub fn requests(&self) -> Vec<ExecRequest> {
        self.0.lock().unwrap().clone()
    }
}

#[cfg(test)]
#[async_trait]
impl Exec for Recording {
    async fn run(&self, req: ExecRequest) -> Result<ExecOutcome, ExecError> {
        self.0.lock().unwrap().push(req.clone());
        run_plain(req, Enforcement::none("recording")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(argv: &[&str], timeout: u64) -> ExecRequest {
        let mut r = ExecRequest::new(argv.iter().map(|s| s.to_string()).collect(), std::env::temp_dir(), Duration::from_secs(timeout));
        r.env = passed_env(&[], &[]);
        r
    }

    #[tokio::test]
    async fn a_plain_process_gets_its_arguments_as_they_are_and_says_nothing_was_enforced() {
        let got = PlainExec.run(req(&["printf", "%s|", "a;b", "$(x)"], 5)).await.unwrap();
        assert_eq!(String::from_utf8_lossy(&got.stdout), "a;b|$(x)|");
        assert_eq!((got.code, got.timed_out, got.truncated), (Some(0), false, false));
        assert_eq!(got.enforced, Enforcement::none("plain"));
    }

    #[tokio::test]
    async fn output_is_cut_at_its_head_or_its_tail_and_says_so() {
        let mut r = req(&["sh", "-c", "printf 0123456789"], 5);
        r.keep = OutputKeep::Head(4);
        let got = PlainExec.run(r.clone()).await.unwrap();
        assert_eq!((got.stdout.as_slice(), got.truncated), (&b"0123"[..], true));
        r.keep = OutputKeep::Tail(4);
        let got = PlainExec.run(r).await.unwrap();
        assert_eq!((got.stdout.as_slice(), got.truncated), (&b"6789"[..], true));
    }

    #[tokio::test]
    async fn stdin_is_given_and_a_process_out_of_time_is_killed_with_its_group() {
        let mut r = req(&["cat"], 5);
        r.stdin = Some(b"fed".to_vec());
        assert_eq!(PlainExec.run(r).await.unwrap().stdout, b"fed");
        let t = Instant::now();
        let got = PlainExec.run(req(&["sh", "-c", "sleep 30 & sleep 30"], 1)).await.unwrap();
        assert!(got.timed_out && got.code.is_none(), "{got:?}");
        assert!(t.elapsed() < Duration::from_secs(10));
        assert!(matches!(PlainExec.run(req(&["no-such-program-here"], 5)).await, Err(ExecError::NotStarted(_))));
    }

    #[test]
    fn the_environment_is_what_is_named_less_what_is_denied() {
        let from = || {
            [("CI_A", "1"), ("CI_KEY", "2"), ("OTHER", "3"), ("X_API_KEY", "4")]
                .into_iter()
                .map(|(k, v)| (std::ffi::OsString::from(k), std::ffi::OsString::from(v)))
        };
        let got = named_env(&["CI_*".into(), "X_API_KEY".into()], &["*_API_KEY".into(), "CI_KEY".into()], from());
        assert_eq!(got, vec![("CI_A".into(), "1".into())]);
    }
}
