//! Running a gate the kernel handed out: `check`, `command` and `script` gates here, a host's
//! function (`script` with `function`, `host`) by the host's checks.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::dispatch::settle;
use super::Runner;
use crate::kernel::engine::Engine;
use crate::kernel::host::{GateResult, Outcome};
use crate::kernel::parts::GateKind;
use crate::kernel::run::RunRecord;
use crate::kernel::FlowError;

/// A gate's work item: run it, and hand its result to the kernel.
pub(crate) async fn run_gate(rn: &Arc<Runner>, run: String, station: String, round: u32, gate: String) {
    let engine = rn.engine();
    let rec = match engine.record(&run).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(run, error = %e, "gate not run");
            return;
        }
    };
    let result = match rec.parts.gates.get(&gate) {
        // A command runs on this machine: a project's run gets to run one only while the project
        // is trusted, as its hooks and its own servers do.
        Some(g) if g.kind == GateKind::Command && rec.project_root.as_deref().is_some_and(|r| !rn.cfg.trust.trusted(r)) => {
            GateResult::errored(format!("the project is not trusted: its command gate {} does not run", g.name))
        }
        Some(g) if g.kind == GateKind::Command => {
            command_gate(rn.cfg.command_gates, &rn.cfg.env_deny, engine, &rec, &station, g).await
        }
        Some(g) if g.kind == GateKind::Check => own_gate(engine, &rec, &station, round, g).await,
        Some(g) if g.kind == GateKind::Script && g.script.is_some() => own_gate(engine, &rec, &station, round, g).await,
        Some(g) => match (&rn.cfg.gates, g.host_function()) {
            (Some(gs), Some(f)) => gs.run(&rec, &station, f).await,
            (None, _) => GateResult::errored(format!("the host runs no checks: {gate} cannot run")),
            (_, None) => GateResult::errored(format!("gate {gate} is not one the host runs")),
        },
        None => GateResult::errored(format!("gate {gate} was not in the packs when the run started")),
    };
    let r = offer_result(engine, rn.cancelled(), &run, &station, round, &gate, result).await;
    settle(rn, &run, "gate_result", r).await;
}

/// The first wait before a gate's result is offered again after a conflict; it doubles each time.
const OFFER_AGAIN_AFTER: Duration = Duration::from_millis(50);
/// The longest wait between two offers.
const OFFER_AGAIN_AT_MOST: Duration = Duration::from_secs(5);

/// Hand a gate's result to the kernel until it is taken. Each offer is a whole operation — the
/// run loaded afresh, the result applied, the commit retried on a conflict as every operation
/// is — so a run busy enough to outlast those retries only delays the result: it is offered
/// again after a wait that grows, for as long as the runner runs. A command may take minutes and
/// change things outside the run; running it again because one commit lost a race would be
/// wrong, and dropping its result would leave the station waiting for good.
///
/// Stopping is the one way out without an answer, and it loses nothing either: the gate is still
/// waiting in the run, and the next start runs it again (`recover`).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn offer_result(
    engine: &Engine,
    stopping: &CancellationToken,
    run: &str,
    station: &str,
    round: u32,
    gate: &str,
    result: GateResult,
) -> crate::Result<Outcome> {
    let mut wait = OFFER_AGAIN_AFTER;
    loop {
        match engine.gate_result(run, station, round, gate, result.clone()).await {
            Err(FlowError::Conflict(why)) => {
                tracing::warn!(run, station, gate, why, "a gate's result met conflicts; offered again");
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    _ = stopping.cancelled() => {
                        tracing::warn!(run, station, gate, "a gate's result not recorded before stopping; the gate runs again at the next start");
                        return Err(FlowError::Conflict(why));
                    }
                }
                wait = (wait * 2).min(OFFER_AGAIN_AT_MOST);
            }
            r => return r,
        }
    }
}

/// Run a `command` gate where the run works: `sh -c` in its `cwd` (references resolved; a relative
/// one under the run's workspace), else the run's workspace; passes on exit status 0. Refused
/// unless the host turned command gates on (`allowed`).
///
/// The command sees none of this process's environment but the basics (`PATH`, `HOME`, the
/// locale, `TMPDIR`…): no provider key, no hook secret. The run reaches it there, never in the
/// command line — `FLOW_RUN`, `FLOW_STATION`, `FLOW_WORKSPACE` (the project, or the run's own
/// directory), `FLOW_CONTEXT_FILE` (the reference context as JSON, `context.json` of the station in
/// the run's process directory) and, when it is small, `FLOW_CONTEXT` itself. It runs in a process
/// group of its own, killed whole when it runs out of time; the end of its output is the result,
/// and what was read of it is kept beside the context as `gate-<gate>.log`.
pub(crate) async fn command_gate(
    allowed: bool,
    env_deny: &[String],
    engine: &Engine,
    rec: &RunRecord,
    station: &str,
    g: &crate::GateDef,
) -> GateResult {
    use tokio::io::AsyncReadExt;
    // Everything up to the command's own conclusion is the gate not getting to one.
    let fail = GateResult::errored;
    if !allowed {
        return fail(format!("command gates are not turned on here: {} cannot run", g.name));
    }
    let Some(command) = g.command.clone().filter(|c| !c.trim().is_empty()) else {
        return fail(format!("gate {} has no command", g.name));
    };
    let (Some(ws), Some(station_dir)) = (engine.workspace_for(rec), super::project::station_dir(engine, rec, station)) else {
        return fail(format!("gate {}: the run has no workspace", g.name));
    };
    let ctx = crate::kernel::refs::context_in(rec, None, None, ws.to_str());
    let cwd = match &g.cwd {
        Some(c) => match crate::kernel::refs::resolve_str(c, &ctx) {
            Value::String(s) if !s.is_empty() => {
                let p = std::path::PathBuf::from(&s);
                if p.is_absolute() {
                    p
                } else {
                    ws.join(p)
                }
            }
            _ => return fail(format!("gate {}: cwd {c} resolves to nothing", g.name)),
        },
        None => ws.clone(),
    };
    let context = ctx.to_string();
    let context_file = station_dir.join("context.json");
    let written = std::fs::create_dir_all(&cwd)
        .and_then(|_| std::fs::create_dir_all(&station_dir))
        .and_then(|_| std::fs::write(&context_file, &context));
    if let Err(e) = written {
        return fail(format!("gate {}: {}: {e}", g.name, cwd.display()));
    }
    let mut cmd = tokio::process::Command::new("sh");
    cmd.arg("-c")
        .arg(&command)
        .current_dir(&cwd)
        .env_clear()
        .env("FLOW_RUN", &rec.id)
        .env("FLOW_STATION", station)
        .env("FLOW_WORKSPACE", &ws)
        .env("FLOW_CONTEXT_FILE", &context_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    for k in GATE_ENV {
        if let Some(v) = std::env::var_os(k) {
            cmd.env(k, v);
        }
    }
    for (k, v) in passed_env(&g.env, env_deny, std::env::vars_os()) {
        cmd.env(k, v);
    }
    if context.len() <= GATE_CONTEXT_INLINE {
        cmd.env("FLOW_CONTEXT", &context);
    }
    #[cfg(unix)]
    cmd.process_group(0);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return fail(format!("`{command}` did not start: {e}")),
    };
    let pid = child.id();
    let tail = |r: Option<Box<dyn tokio::io::AsyncRead + Unpin + Send>>| async move {
        let mut kept: Vec<u8> = vec![];
        let Some(mut r) = r else { return kept };
        let mut buf = [0u8; 8192];
        while let Ok(n) = r.read(&mut buf).await {
            if n == 0 {
                break;
            }
            kept.extend_from_slice(&buf[..n]);
            if kept.len() > 2 * GATE_OUTPUT_READ {
                kept.drain(..kept.len() - GATE_OUTPUT_READ);
            }
        }
        kept
    };
    let out = tail(child.stdout.take().map(|o| Box::new(o) as _));
    let err = tail(child.stderr.take().map(|o| Box::new(o) as _));
    let waited = tokio::time::timeout(Duration::from_secs(g.timeout_seconds()), async {
        let (o, e, status) = tokio::join!(out, err, child.wait());
        (o, e, status)
    })
    .await;
    let (o, e, status) = match waited {
        Ok(v) => v,
        Err(_) => {
            // The whole group: a test runner's children too.
            #[cfg(unix)]
            if let Some(pid) = pid {
                let _ = std::process::Command::new("kill").arg("-KILL").arg(format!("-{pid}")).status();
            }
            let _ = child.kill().await;
            return fail(format!("`{command}` ran longer than {}s", g.timeout_seconds()));
        }
    };
    let status = match status {
        Ok(s) => s,
        Err(e) => return fail(format!("`{command}` was lost: {e}")),
    };
    let mut text = String::from_utf8_lossy(&o).into_owned();
    text.push_str(&String::from_utf8_lossy(&e));
    let log = station_dir.join(format!("gate-{}.log", g.name));
    if let Err(e) = std::fs::write(&log, &text) {
        tracing::warn!(run = rec.id, gate = g.name, error = %e, "a gate's log not written");
    }
    let tail: String = {
        let chars: Vec<char> = text.trim().chars().collect();
        chars[chars.len().saturating_sub(GATE_OUTPUT_KEPT)..].iter().collect()
    };
    if g.result == Some(crate::kernel::parts::CommandResult::Json) {
        let stdout = String::from_utf8_lossy(&o);
        return match printed_result(&stdout) {
            Some(r) => r,
            None => fail(format!(
                "`{command}` printed no gate result (exit {}){}",
                status.code().map_or("by a signal".to_string(), |c| c.to_string()),
                if tail.is_empty() { String::new() } else { format!(": {tail}") }
            )),
        };
    }
    match status.code() {
        Some(0) => GateResult::passed(tail),
        Some(code) => GateResult::failed(format!(
            "`{command}` exited with {code}{}",
            if tail.is_empty() { String::new() } else { format!(": {tail}") }
        )),
        None => fail(format!(
            "`{command}` was stopped by a signal{}",
            if tail.is_empty() { String::new() } else { format!(": {tail}") }
        )),
    }
}

/// A gate result printed by a command: all of what it printed, or else its last line that is
/// not blank.
fn printed_result(stdout: &str) -> Option<GateResult> {
    let whole = stdout.trim();
    serde_json::from_str::<GateResult>(whole).ok().or_else(|| {
        let last = whole.lines().rev().find(|l| !l.trim().is_empty())?;
        serde_json::from_str::<GateResult>(last.trim()).ok()
    })
}

/// The variables of `from` a gate's `env` names, less those the application denies.
fn passed_env(
    wanted: &[String],
    deny: &[String],
    from: impl Iterator<Item = (std::ffi::OsString, std::ffi::OsString)>,
) -> Vec<(std::ffi::OsString, std::ffi::OsString)> {
    let named = |list: &[String], k: &str| {
        list.iter().any(|w| match w.strip_suffix('*') {
            Some(prefix) => k.starts_with(prefix),
            None => w == k,
        })
    };
    from.filter(|(k, _)| {
        let k = k.to_string_lossy();
        named(wanted, &k) && !named(deny, &k)
    })
    .collect()
}

/// Run a gate the library judges itself, `check` or `script`, against the run's workspace.
async fn own_gate(engine: &Engine, rec: &RunRecord, station: &str, round: u32, g: &crate::GateDef) -> GateResult {
    let Some(ws) = engine.workspace_for(rec) else {
        return GateResult::errored(format!("gate {}: the run has no workspace", g.name));
    };
    let reader = match super::files::Reader::new(&ws) {
        Ok(r) => std::sync::Arc::new(r),
        Err(e) => return GateResult::errored(format!("gate {}: {e}", g.name)),
    };
    let ctx = crate::kernel::refs::context_in(rec, None, None, ws.to_str());
    if g.kind == GateKind::Check {
        return super::checks::run(g, &ctx, rec, &reader);
    }
    script_gate(engine, rec, station, round, g, &ctx, reader).await
}

#[cfg(feature = "script")]
async fn script_gate(
    engine: &Engine,
    rec: &RunRecord,
    station: &str,
    round: u32,
    g: &crate::GateDef,
    ctx: &Value,
    reader: std::sync::Arc<super::files::Reader>,
) -> GateResult {
    let name = g.script.clone().unwrap_or_default();
    let Some(source) = rec.parts.scripts.get(&name).cloned() else {
        return GateResult::errored(format!("script {name} was not in the packs when the run started"));
    };
    let outputs: serde_json::Map<String, Value> = rec
        .def
        .stations
        .iter()
        .map(|s| (s.id.clone(), crate::kernel::refs::station_output(&rec.def, rec, &s.id)))
        .collect();
    let input = serde_json::json!({
        "gate": g.name,
        "run": rec.id,
        "flow": rec.flow,
        "station": station,
        "round": round,
        "inputs": rec.inputs,
        "outputs": outputs,
        "output_dir": rec.output_dir,
        "workspace": ctx.get("workspace"),
        "params": crate::kernel::refs::resolve(&g.params, ctx),
    });
    let (result, trace) = super::script::run(source, input, reader, Duration::from_secs(g.script_seconds())).await;
    if !trace.log.is_empty() {
        if let Some(dir) = super::project::station_dir(engine, rec, station) {
            let log = dir.join(format!("gate-{}.log", g.name));
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&log, &trace.log)) {
                tracing::warn!(run = rec.id, gate = g.name, error = %e, "a gate's log not written");
            }
        }
    }
    result
}

#[cfg(not(feature = "script"))]
async fn script_gate(
    _: &Engine,
    _: &RunRecord,
    _: &str,
    _: u32,
    g: &crate::GateDef,
    _: &Value,
    _: std::sync::Arc<super::files::Reader>,
) -> GateResult {
    GateResult::errored(format!("gate {}: this build runs no scripts", g.name))
}

/// What a command gate's process keeps of this one's environment.
const GATE_ENV: &[&str] = &["PATH", "HOME", "USER", "LOGNAME", "LANG", "LC_ALL", "LC_CTYPE", "TMPDIR", "TERM", "SHELL"];
/// The largest context also given inline (`FLOW_CONTEXT`); a larger one only by its file.
const GATE_CONTEXT_INLINE: usize = 32 * 1024;
/// How much of each output stream is read and held at most.
const GATE_OUTPUT_READ: usize = 64 * 1024;

/// How much of a command gate's output its result keeps (the end of it).
const GATE_OUTPUT_KEPT: usize = 2000;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::host::{Gates, RunStore, StoreError, WorkItem};
    use crate::kernel::run::{Actor, CheckStatus, StationStatus};
    use crate::runner::{RunnerConfig, ScriptedHost};
    use crate::{CreateIn, Hooked, ManualClock, MemPacks, MemStore, ProjectLayout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A store whose next `n` commits conflict, `n` being what the counter holds.
    fn flaky() -> (Arc<Hooked<MemStore>>, Arc<AtomicUsize>) {
        let conflicts = Arc::new(AtomicUsize::new(0));
        let left = conflicts.clone();
        let store = Hooked::new(MemStore::new()).check_commits(move |_| {
            match left.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1)) {
                Ok(_) => Err(StoreError::Conflict("injected".into())),
                Err(_) => Ok(()),
            }
        });
        (Arc::new(store), conflicts)
    }

    /// The host's checks: every one passes.
    struct Passes;

    #[async_trait::async_trait]
    impl Gates for Passes {
        async fn run(&self, _: &RunRecord, _: &str, gate: &str) -> GateResult {
            GateResult::passed(format!("{gate} passed"))
        }
    }

    /// A run whose first station waits for gate `gate`, and the runner it runs in.
    struct Gated {
        rn: Arc<Runner>,
        engine: Arc<Engine>,
        run: String,
        work: WorkItem,
        _ws: tempfile::TempDir,
    }

    impl Gated {
        async fn new(gate_def: &str, store: Arc<dyn RunStore>, configure: impl FnOnce(&mut RunnerConfig)) -> Self {
            Self::with(gate_def, store, configure, |p| p, &[]).await
        }

        /// With more in the packs, and files in the run's workspace before it starts.
        async fn with(
            gate_def: &str,
            store: Arc<dyn RunStore>,
            configure: impl FnOnce(&mut RunnerConfig),
            more: impl FnOnce(MemPacks) -> MemPacks,
            files: &[(&str, &str)],
        ) -> Self {
            let name = serde_yaml::from_str::<Value>(gate_def).unwrap()["name"].as_str().unwrap().to_string();
            let packs = more(MemPacks::new()
                .command_gates()
                .check("lint")
                .gate_def(gate_def)
                .flow(&format!(
                    "{{name: g, stations: [{{id: a, exit: [{{kind: gate, gate: {name}}}]}}, {{id: b, exit: [{{kind: confirm}}]}}]}}"
                )));
            let ws = tempfile::tempdir().unwrap();
            let engine = Arc::new(
                Engine::new(store, Arc::new(packs))
                    .with_clock(Arc::new(ManualClock::new(1_000_000)))
                    .with_workspaces(ws.path())
                    .with_layout(ProjectLayout::new(".harnix", ws.path().join("worktrees"), "harnix/flow")),
            );
            let mut cfg = RunnerConfig { lease_ms: 60_000, guarded: vec![], ..Default::default() };
            configure(&mut cfg);
            let rn = Runner::for_tests_with(engine.clone(), ScriptedHost::new(), cfg);
            let o = engine
                .create(
                    CreateIn {
                        flow: "g".into(),
                        subject: "s".into(),
                        ..Default::default()
                    },
                    Actor::device("d"),
                )
                .await
                .unwrap();
            let run = o.value["run"].as_str().unwrap().to_string();
            for (p, t) in files {
                let full = ws.path().join(&run).join(p);
                std::fs::create_dir_all(full.parent().unwrap()).unwrap();
                std::fs::write(full, t).unwrap();
            }
            let work = o
                .work
                .into_iter()
                .find(|w| matches!(w, WorkItem::Gate { .. }))
                .expect("the gate is handed out");
            Gated { rn, engine, run, work, _ws: ws }
        }
        async fn run_it(&self) {
            let WorkItem::Gate { run, station, round, gate } = self.work.clone() else {
                unreachable!()
            };
            run_gate(&self.rn, run, station, round, gate).await;
        }
        /// The station's status and its gate check's.
        async fn judged(&self) -> (StationStatus, CheckStatus, String) {
            let rec = self.engine.record(&self.run).await.unwrap();
            let st = &rec.stations["a"];
            let c = st.checks.iter().find(|c| c.kind == "gate").unwrap();
            (st.status, c.status, c.detail.clone())
        }
    }

    fn command(cfg: &mut RunnerConfig) {
        cfg.command_gates = true;
    }

    /// Where the run works, with the run in its environment and nothing of this process's but
    /// the basics.
    #[tokio::test]
    async fn a_command_gate_passes_on_status_0_where_the_run_works() {
        std::env::set_var("HARNIX_FLOW_GATE_TEST_SECRET", "not for gates");
        let g = Gated::new(
            r#"{name: build, kind: command, command: 'test "$FLOW_STATION" = a && test -z "$HARNIX_FLOW_GATE_TEST_SECRET" && test -f "$FLOW_CONTEXT_FILE" && test "$(pwd -P)" = "$(cd "$FLOW_WORKSPACE" && pwd -P)" && echo built "$FLOW_RUN"'}"#,
            Arc::new(MemStore::new()),
            command,
        )
        .await;
        g.run_it().await;
        let (station, check, detail) = g.judged().await;
        assert_eq!((station, check), (StationStatus::Passed, CheckStatus::Pass), "{detail}");
        assert_eq!(detail, format!("built {}", g.run));
        // The context it read and what it printed, in the station's process directory.
        let dir = g._ws.path().join(&g.run).join(".harnix/runs").join(&g.run).join("stations/a");
        let context: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("context.json")).unwrap()).unwrap();
        assert_eq!(context["run"], g.run.as_str());
        assert_eq!(std::fs::read_to_string(dir.join("gate-build.log")).unwrap().trim(), format!("built {}", g.run));
        assert!(!g._ws.path().join(&g.run).join(".flow").exists());
    }

    #[tokio::test]
    async fn a_command_gate_fails_on_any_other_status_with_the_end_of_its_output() {
        let g = Gated::new(
            "{name: build, kind: command, command: 'echo nope >&2; exit 3'}",
            Arc::new(MemStore::new()),
            command,
        )
        .await;
        g.run_it().await;
        let (station, check, detail) = g.judged().await;
        assert_eq!(check, CheckStatus::Fail);
        assert_ne!(station, StationStatus::Passed);
        assert!(detail.contains("exited with 3: nope"), "{detail}");
    }

    #[tokio::test]
    async fn a_command_gate_runs_only_where_command_gates_are_turned_on() {
        let g = Gated::new("{name: build, kind: command, command: 'true'}", Arc::new(MemStore::new()), |_| {}).await;
        g.run_it().await;
        let (_, check, detail) = g.judged().await;
        assert_eq!(check, CheckStatus::Error);
        assert!(detail.contains("command gates are not turned on here"), "{detail}");
    }

    #[tokio::test]
    async fn a_command_gate_that_runs_too_long_is_killed_and_fails() {
        let g = Gated::new(
            "{name: build, kind: command, command: 'sleep 30', timeout_seconds: 1}",
            Arc::new(MemStore::new()),
            command,
        )
        .await;
        let started = std::time::Instant::now();
        g.run_it().await;
        assert!(started.elapsed() < Duration::from_secs(10));
        let (_, check, detail) = g.judged().await;
        assert_eq!(check, CheckStatus::Error);
        assert!(detail.contains("ran longer than 1s"), "{detail}");
    }

    /// Only the variables the gate names, and never one the application denies.
    #[tokio::test]
    async fn a_command_gate_gets_the_variables_it_names_and_none_the_application_denies() {
        std::env::set_var("FLOW_GATE_ENV_OK", "yes");
        std::env::set_var("FLOW_GATE_ENV_P1", "1");
        std::env::set_var("FLOW_GATE_ENV_PKEY", "secret");
        std::env::set_var("FLOW_GATE_ENV_OTHER", "no");
        let g = Gated::new(
            r#"{name: build, kind: command, env: [FLOW_GATE_ENV_OK, FLOW_GATE_ENV_P*], command: 'test "$FLOW_GATE_ENV_OK" = yes && test "$FLOW_GATE_ENV_P1" = 1 && test -z "$FLOW_GATE_ENV_PKEY" && test -z "$FLOW_GATE_ENV_OTHER"'}"#,
            Arc::new(MemStore::new()),
            |cfg| {
                cfg.command_gates = true;
                cfg.env_deny = vec!["*KEY".into(), "FLOW_GATE_ENV_PKEY".into()];
            },
        )
        .await;
        g.run_it().await;
        let (station, check, detail) = g.judged().await;
        assert_eq!((station, check), (StationStatus::Passed, CheckStatus::Pass), "{detail}");
    }

    /// `result: json`: what it prints is the result, findings and all; nothing readable is an error.
    #[tokio::test]
    async fn a_command_gate_may_print_its_result() {
        let printed = "{name: scan, kind: command, result: json, command: 'echo scanning; cat r.json; exit 1'}";
        let json = r#"{"verdict":"fail","summary":"1 finding","findings":[{"rule":"r1","path":"a.rs","line":3,"message":"unsafe"}],"metrics":{"n":1}}"#;
        let g = Gated::with(printed, Arc::new(MemStore::new()), command, |p| p, &[("r.json", json)]).await;
        g.run_it().await;
        let rec = g.engine.record(&g.run).await.unwrap();
        let c = rec.stations["a"].checks.iter().find(|c| c.kind == "gate").unwrap();
        assert_eq!((c.status, c.detail.as_str()), (CheckStatus::Fail, "1 finding"));
        assert_eq!(c.findings[0].line_text(), "a.rs:3: unsafe");
        assert_eq!(c.metrics["n"], 1);
        let g = Gated::new("{name: scan, kind: command, result: json, command: 'echo not json'}", Arc::new(MemStore::new()), command).await;
        g.run_it().await;
        let (_, check, detail) = g.judged().await;
        assert_eq!(check, CheckStatus::Error, "{detail}");
    }

    /// A `check` gate reads the run's workspace and keeps what it read.
    #[tokio::test]
    async fn a_check_gate_judges_the_files_and_keeps_its_evidence() {
        let gate = "{name: written, kind: check, check: files, paths: [design.md, 'adr/*.md']}";
        let g = Gated::with(gate, Arc::new(MemStore::new()), |_| {}, |p| p, &[("design.md", "# D\n"), ("adr/1.md", "x")]).await;
        g.run_it().await;
        let rec = g.engine.record(&g.run).await.unwrap();
        let c = rec.stations["a"].checks.iter().find(|c| c.kind == "gate").unwrap();
        assert_eq!(c.status, CheckStatus::Pass, "{c:?}");
        assert_eq!(c.evidence.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["adr/1.md", "design.md"]);
        let g = Gated::with(gate, Arc::new(MemStore::new()), |_| {}, |p| p, &[("design.md", "")]).await;
        g.run_it().await;
        let (station, check, _) = g.judged().await;
        assert_eq!((station, check), (StationStatus::Unmet, CheckStatus::Fail));
        let v = g.engine.get(&g.run).await.unwrap();
        let why = v.unmet["a"][0].explain();
        assert!(why.contains("design.md: is empty") && why.contains("adr/*.md: no file matches"), "{why}");
    }

    /// A `script` gate runs the script pinned when the run started, not the one in the packs now.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_script_gate_runs_its_pinned_script() {
        let gate = "{name: ledgers, kind: script, script: ledgers, params: {file: '${output_dir}'}}";
        let src = "export default function (input, flow) { const t = flow.read.text('notes.md'); flow.log('read', t); return { verdict: t && t.includes('done') ? 'pass' : 'fail', summary: input.station }; }";
        let g = Gated::with(gate, Arc::new(MemStore::new()), |_| {}, |p| p.part(crate::PartKind::Script, "ledgers", src), &[("notes.md", "all done")]).await;
        g.run_it().await;
        let rec = g.engine.record(&g.run).await.unwrap();
        assert!(rec.parts.scripts["ledgers"].contains("export default"));
        let c = rec.stations["a"].checks.iter().find(|c| c.kind == "gate").unwrap();
        assert_eq!((c.status, c.detail.as_str()), (CheckStatus::Pass, "a"), "{c:?}");
        assert_eq!(c.evidence[0].path, "notes.md");
        let dir = g._ws.path().join(&g.run).join(".harnix/runs").join(&g.run).join("stations/a");
        assert_eq!(std::fs::read_to_string(dir.join("gate-ledgers.log")).unwrap(), "read all done\n");
    }

    /// A gate that did not conclude leaves the station unmet without the automatic rework.
    #[tokio::test]
    async fn a_gate_error_is_not_reworked_automatically() {
        let packs = |p: MemPacks| {
            p.flow("{name: r, stations: [{id: w, exit: [{kind: confirm}]}, {id: a, exit: [{kind: gate, gate: build}], on_unmet: {rework_to: w, max: 2}}]}")
        };
        let g = Gated::with("{name: build, kind: command, command: 'true'}", Arc::new(MemStore::new()), |_| {}, packs, &[]).await;
        // The helper's own flow; this one is started by hand.
        let o = g.engine.create(CreateIn { flow: "r".into(), subject: "s".into(), ..Default::default() }, Actor::device("d")).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        g.engine.confirm(&run, "w", true, None, Actor::device("d")).await.unwrap();
        let rec = g.engine.record(&run).await.unwrap();
        let round = rec.stations["a"].round;
        run_gate(&g.rn, run.clone(), "a".into(), round, "build".into()).await;
        let rec = g.engine.record(&run).await.unwrap();
        assert_eq!(rec.stations["a"].status, StationStatus::Unmet);
        assert_eq!(rec.stations["w"].round, 1, "not reworked");
        assert_eq!(rec.stations["a"].checks[0].status, CheckStatus::Error);
        // A person runs it again: it is handed out afresh, the station waits on it.
        let o = g.engine.rerun_gates(&run, "a", Actor::device("d")).await.unwrap();
        assert!(o.work.iter().any(|w| matches!(w, WorkItem::Gate { gate, .. } if gate == "build")), "{:?}", o.work);
        let rec = g.engine.record(&run).await.unwrap();
        assert_eq!((rec.stations["a"].status, rec.stations["a"].checks[0].status), (StationStatus::Working, CheckStatus::Wait));
        assert!(g.engine.rerun_gates(&run, "a", Actor::device("d")).await.is_err(), "nothing left to run again");
    }

    /// A gate that fails sends the round back with what it found, and the round is told.
    #[tokio::test]
    async fn a_failed_gate_reworks_and_the_new_round_is_told_why() {
        let packs = |p: MemPacks| {
            p.gate_def("{name: written, kind: check, check: files, paths: [design.md]}")
                .flow("{name: r, stations: [{id: w, exit: [{kind: confirm}]}, {id: a, exit: [{kind: gate, gate: written}], on_unmet: {rework_to: w, max: 2}}]}")
        };
        let g = Gated::with("{name: build, kind: command, command: 'true'}", Arc::new(MemStore::new()), command, packs, &[]).await;
        let o = g.engine.create(CreateIn { flow: "r".into(), subject: "s".into(), ..Default::default() }, Actor::device("d")).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        g.engine.confirm(&run, "w", true, None, Actor::device("d")).await.unwrap();
        std::fs::create_dir_all(g._ws.path().join(&run)).unwrap();
        let round = g.engine.record(&run).await.unwrap().stations["a"].round;
        run_gate(&g.rn, run.clone(), "a".into(), round, "written".into()).await;
        let rec = g.engine.record(&run).await.unwrap();
        assert_eq!(rec.stations["w"].round, 2, "reworked");
        let r = rec.stations["w"].reworked.as_ref().unwrap();
        assert_eq!(r.from.as_deref(), Some("a"));
        assert_eq!(r.unmet[0].explain(), "1 problem in 0 files; design.md: does not exist");
    }

    /// More conflicts than one operation retries: the result is offered again, not dropped.
    #[tokio::test(start_paused = true)]
    async fn a_gates_result_outlasts_more_conflicts_than_one_commit_retries() {
        let (store, conflicts) = flaky();
        let g = Gated::new("{name: lint, kind: host}", store.clone(), |cfg| cfg.gates = Some(Arc::new(Passes))).await;
        conflicts.store(3 * crate::DEFAULT_RETRIES - 1, Ordering::SeqCst);
        g.run_it().await;
        assert_eq!(conflicts.load(Ordering::SeqCst), 0);
        let (station, check, _) = g.judged().await;
        assert_eq!((station, check), (StationStatus::Passed, CheckStatus::Pass));
    }

    /// Stopping while the result still meets conflicts: the gate is still waiting, and the next
    /// start hands it out again.
    #[tokio::test(start_paused = true)]
    async fn a_gates_result_not_recorded_before_stopping_runs_again_at_the_next_start() {
        let (store, conflicts) = flaky();
        let g = Gated::new("{name: lint, kind: host}", store.clone(), |cfg| cfg.gates = Some(Arc::new(Passes))).await;
        conflicts.store(usize::MAX, Ordering::SeqCst);
        let stop = async {
            tokio::time::sleep(Duration::from_secs(60)).await;
            g.rn.cancelled().cancel();
        };
        tokio::join!(g.run_it(), stop);
        conflicts.store(0, Ordering::SeqCst);
        let (_, check, _) = g.judged().await;
        assert_eq!(check, CheckStatus::Wait);
        let rec = g.engine.record(&g.run).await.unwrap();
        let again = g.engine.dispatched_work(&rec, g.engine.clock().now_ms());
        assert_eq!(again, vec![g.work.clone()]);
    }
}
