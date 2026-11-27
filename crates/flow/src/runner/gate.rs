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
    let result = judge_gate(rn, &rec, &station, round, &gate, None).await;
    let r = offer_result(engine, rn.cancelled(), &run, &station, round, &gate, result).await;
    settle(rn, &run, "gate_result", r).await;
}

/// The task a `task_exit` gate judges the hand-in of: what its references and its context see as
/// `${task}` and `${item}`.
pub(crate) struct ForTask<'a> {
    pub task: &'a str,
    pub item: Option<&'a Value>,
}

/// Judge a gate of the run, for its station or (`task`) for one task's hand-in.
pub(crate) async fn judge_gate(rn: &Arc<Runner>, rec: &RunRecord, station: &str, round: u32, gate: &str, task: Option<&ForTask<'_>>) -> GateResult {
    let engine = rn.engine();
    match rec.parts.gates.get(gate) {
        // What a confirmation approved, recorded or checked again: the library's own reading.
        None if crate::kernel::confirm::implicit(gate).is_some() => super::checks::implicit(engine, rec, station, gate),
        // A command runs on this machine: a project's run gets to run one only while the project
        // is trusted, as its hooks and its own servers do.
        Some(g) if g.kind == GateKind::Command && rec.project_root.as_deref().is_some_and(|r| !rn.cfg.trust.trusted(r)) => {
            GateResult::errored(format!("the project is not trusted: its command gate {} does not run", g.name))
        }
        // A plan picks what to run from what people declared; the runs are commands all the same.
        #[cfg(feature = "script")]
        Some(g) if g.kind == GateKind::Command && g.plan.is_some() => {
            super::plan::plan_gate(rn, rec, station, round, g, task).await
        }
        #[cfg(not(feature = "script"))]
        Some(g) if g.kind == GateKind::Command && g.plan.is_some() => GateResult::errored(format!("gate {}: this build runs no scripts", g.name)),
        Some(g) if g.kind == GateKind::Command => {
            command_gate(rn, rec, station, g, task).await
        }
        Some(g) if g.kind == GateKind::Check => own_gate(rn, rec, station, round, g, task).await,
        Some(g) if g.kind == GateKind::Script && g.script.is_some() => own_gate(rn, rec, station, round, g, task).await,
        Some(g) => match (&rn.cfg.gates, g.host_function(), task) {
            (Some(gs), Some(f), None) => gs.run(rec, station, f).await,
            (Some(gs), Some(f), Some(t)) => gs.run_for_task(rec, station, t.task, t.item, f).await,
            (None, _, _) => GateResult::errored(format!("the host runs no checks: {gate} cannot run")),
            (_, None, _) => GateResult::errored(format!("gate {gate} is not one the host runs")),
        },
        None => GateResult::errored(format!("gate {gate} was not in the packs when the run started")),
    }
}

/// The reference context a gate is judged against: the run's, with the task's when it judges one.
pub(crate) fn gate_context(rec: &RunRecord, task: Option<&ForTask<'_>>, ws: Option<&str>) -> Value {
    crate::kernel::refs::context_in(rec, task.and_then(|t| t.item), task.map(|t| t.task), ws)
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

/// The file a gate's log is kept in, beside its station's context: `gate-<name>.log` as one file
/// name — a bundle's `xforge/unit-tests` is `gate-xforge-unit-tests.log`; the name as it is would
/// make its bundle a directory that is not there, and the log would not be written.
pub(crate) fn log_name(gate: &str) -> String {
    format!("gate-{}.log", super::project::file_name(gate))
}

/// Run a `command` gate where the run works: `sh -c` in its `cwd` (references resolved; a relative
/// one under the run's workspace), else the run's workspace; passes on exit status 0. Refused
/// unless the host turned command gates on.
///
/// The command sees none of this process's environment but the basics (`PATH`, `HOME`, the
/// locale, `TMPDIR`…): no provider key, no hook secret. The run reaches it there, never in the
/// command line — `FLOW_RUN`, `FLOW_STATION`, `FLOW_WORKSPACE` (the project, or the run's own
/// directory), `FLOW_CONTEXT_FILE` (the reference context as JSON, `context.json` of the station in
/// the run's process directory) and, when it is small, `FLOW_CONTEXT` itself. It is started through
/// the host's [`crate::Exec`] (profile `read`, recorded in the run's history) in a process group of
/// its own, killed whole when it runs out of time; the end of its output is the result, and what
/// was read of it is kept beside the context as `gate-<gate>.log`.
pub(crate) async fn command_gate(rn: &Runner, rec: &RunRecord, station: &str, g: &crate::GateDef, task: Option<&ForTask<'_>>) -> GateResult {
    // Everything up to the command's own conclusion is the gate not getting to one.
    let fail = GateResult::errored;
    if !rn.cfg.command_gates {
        return fail(format!("command gates are not turned on here: {} cannot run", g.name));
    }
    let engine = rn.engine();
    let (Some(ws), Some(station_dir)) = (engine.workspace_for(rec), super::project::station_dir(engine, rec, station)) else {
        return fail(format!("gate {}: the run has no workspace", g.name));
    };
    let ctx = gate_context(rec, task, ws.to_str());
    let audit = super::exec::Audit::run(engine.clone(), &rec.id, Some(station), task.map(|t| t.task), format!("gate {}", g.name));
    let site = GateSite { run: &rec.id, station, ws: &ws, station_dir: &station_dir, ctx: &ctx, network: exec_network(rec) };
    command_at(rn, g, &site, &audit).await
}

/// Whether a run's processes may reach the network: what its bundle's `exec` declared when the
/// run started; no bundle, no declaration, no network.
pub(crate) fn exec_network(rec: &RunRecord) -> bool {
    rec.parts.exec.as_ref().is_some_and(|e| e.network)
}

/// Where a gate runs: the run and station it judges, the directory it judges, where its context
/// and log go, the references it resolves against. A trial (`flow_gate_try`) makes one up.
pub(crate) struct GateSite<'a> {
    pub run: &'a str,
    pub station: &'a str,
    pub ws: &'a std::path::Path,
    pub station_dir: &'a std::path::Path,
    pub ctx: &'a Value,
    /// Whether its processes may reach the network (the bundle's `exec.network`).
    pub network: bool,
}

/// [`command_gate`] at a site, command gates allowed.
pub(crate) async fn command_at(rn: &Runner, g: &crate::GateDef, site: &GateSite<'_>, audit: &super::exec::Audit) -> GateResult {
    let fail = GateResult::errored;
    let Some(command) = g.command.clone().filter(|c| !c.trim().is_empty()) else {
        return fail(format!("gate {} has no command", g.name));
    };
    let (ws, station_dir, ctx) = (site.ws.to_path_buf(), site.station_dir, site.ctx);
    let cwd = match &g.cwd {
        Some(c) => match crate::kernel::refs::resolve_str(c, ctx) {
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
        .and_then(|_| std::fs::create_dir_all(station_dir))
        .and_then(|_| std::fs::write(&context_file, &context));
    if let Err(e) = written {
        return fail(format!("gate {}: {}: {e}", g.name, cwd.display()));
    }
    let req = gate_request(rn, vec!["sh".into(), "-c".into(), command.clone()], cwd, g, site, &context_file, &context, g.timeout_seconds());
    let (o, e, code) = match super::exec::run_audited(rn.exec().as_ref(), req, audit).await {
        Ok(o) if o.timed_out => return fail(format!("`{command}` ran longer than {}s", g.timeout_seconds())),
        Ok(o) => (o.stdout, o.stderr, o.code),
        Err(crate::ExecError::Lost(e)) => return fail(format!("`{command}` was lost: {e}")),
        Err(crate::ExecError::NotStarted(e)) => return fail(format!("`{command}` did not start: {e}")),
        Err(crate::ExecError::Refused(e)) => return fail(format!("`{command}` was refused: {e}")),
    };
    let mut text = String::from_utf8_lossy(&o).into_owned();
    text.push_str(&String::from_utf8_lossy(&e));
    let log = station_dir.join(log_name(&g.name));
    if let Err(e) = std::fs::write(&log, &text) {
        tracing::warn!(run = site.run, gate = g.name, error = %e, "a gate's log not written");
    }
    let tail = output_tail(&text);
    if g.result == Some(crate::kernel::parts::CommandResult::Json) {
        let stdout = String::from_utf8_lossy(&o);
        return match printed_result(&stdout) {
            Some(r) => r,
            None => fail(format!(
                "`{command}` printed no gate result (exit {}){}",
                code.map_or("by a signal".to_string(), |c| c.to_string()),
                if tail.is_empty() { String::new() } else { format!(": {tail}") }
            )),
        };
    }
    match code {
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

/// The request a command gate's process (or a run of its plan) is started by: none of this
/// process's environment but the basics, the run's own variables, the ones the gate names less the
/// ones the application denies; profile `read` (a gate judges the work, it does not do it); the
/// network as the bundle declared; the end of each output stream kept.
#[allow(clippy::too_many_arguments)]
pub(crate) fn gate_request(
    rn: &Runner,
    argv: Vec<String>,
    cwd: std::path::PathBuf,
    g: &crate::GateDef,
    site: &GateSite<'_>,
    context_file: &std::path::Path,
    context: &str,
    seconds: u64,
) -> crate::ExecRequest {
    let mut env: Vec<(std::ffi::OsString, std::ffi::OsString)> = vec![
        ("FLOW_RUN".into(), site.run.into()),
        ("FLOW_STATION".into(), site.station.into()),
        ("FLOW_WORKSPACE".into(), site.ws.into()),
        ("FLOW_CONTEXT_FILE".into(), context_file.into()),
    ];
    env.extend(super::exec::passed_env(&g.env, &rn.cfg.env_deny));
    if context.len() <= GATE_CONTEXT_INLINE {
        env.push(("FLOW_CONTEXT".into(), context.into()));
    }
    let mut req = crate::ExecRequest::new(argv, cwd, Duration::from_secs(seconds));
    req.env = env;
    req.network = site.network;
    req.keep = crate::OutputKeep::Tail(GATE_OUTPUT_READ);
    req
}

/// The end of a gate's output that its result keeps.
pub(crate) fn output_tail(text: &str) -> String {
    let chars: Vec<char> = text.trim().chars().collect();
    chars[chars.len().saturating_sub(GATE_OUTPUT_KEPT)..].iter().collect()
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

/// Run a gate the library judges itself, `check` or `script`, against the run's workspace.
async fn own_gate(rn: &Runner, rec: &RunRecord, station: &str, round: u32, g: &crate::GateDef, task: Option<&ForTask<'_>>) -> GateResult {
    let engine = rn.engine();
    let Some(ws) = engine.workspace_for(rec) else {
        return GateResult::errored(format!("gate {}: the run has no workspace", g.name));
    };
    let reader = match super::files::Reader::new(&ws) {
        Ok(r) => std::sync::Arc::new(r),
        Err(e) => return GateResult::errored(format!("gate {}: {e}", g.name)),
    };
    let ctx = gate_context(rec, task, ws.to_str());
    if g.kind == GateKind::Check {
        return super::checks::run(g, &ctx, rec, &reader);
    }
    script_gate(rn, rec, station, round, g, task, &ctx, reader).await
}

#[cfg(feature = "script")]
#[allow(clippy::too_many_arguments)]
async fn script_gate(
    rn: &Runner,
    rec: &RunRecord,
    station: &str,
    round: u32,
    g: &crate::GateDef,
    task: Option<&ForTask<'_>>,
    ctx: &Value,
    reader: std::sync::Arc<super::files::Reader>,
) -> GateResult {
    let engine = rn.engine();
    let name = g.script.clone().unwrap_or_default();
    let Some(source) = rec.parts.scripts.get(&name).cloned() else {
        return GateResult::errored(format!("script {name} was not in the packs when the run started"));
    };
    let input = script_input(rec, station, round, g, ctx);
    let env = match engine.workspace_for(rec) {
        // A gate judges: what its script starts reads, whatever it is.
        Some(ws) => super::scriptenv::Env::for_run(engine, rec, ws.clone(), super::scriptenv::Modules::script_path(&name), &source).await.with_exec(
            rn.run_exec_scope(rec, ws, crate::ExecProfile::Read, Some(station), task.map(|t| t.task), format!("flow.exec in gate {}", g.name)),
        ),
        None => super::scriptenv::Env::bare(),
    };
    let (result, trace) = super::script::run(source, input, reader, env, Duration::from_secs(g.script_seconds())).await;
    if !trace.log.is_empty() {
        if let Some(dir) = super::project::station_dir(engine, rec, station) {
            let log = dir.join(log_name(&g.name));
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&log, &trace.log)) {
                tracing::warn!(run = rec.id, gate = g.name, error = %e, "a gate's log not written");
            }
        }
    }
    result
}

/// What a gate's script is given as `input` (docs/design/flows.md §2.3, in Conix): a `script`
/// gate's script, a `command` gate's plan.
pub(crate) fn script_input(rec: &RunRecord, station: &str, round: u32, g: &crate::GateDef, ctx: &Value) -> Value {
    let outputs: serde_json::Map<String, Value> = rec
        .def
        .stations
        .iter()
        .map(|s| (s.id.clone(), crate::kernel::refs::station_output(&rec.def, rec, &s.id)))
        .collect();
    serde_json::json!({
        "gate": g.name,
        "run": rec.id,
        "flow": rec.flow,
        "station": station,
        "round": round,
        "inputs": rec.inputs,
        "outputs": outputs,
        "output_dir": rec.output_dir,
        "data_dir": rec.data_dir,
        "workspace": ctx.get("workspace"),
        "params": crate::kernel::refs::resolve(&g.params, ctx),
        // A `task_exit` gate's task and item; null for a station's own gate.
        "task": ctx.get("task"),
        "item": ctx.get("item"),
    })
}

#[cfg(not(feature = "script"))]
#[allow(clippy::too_many_arguments)]
async fn script_gate(
    _: &Runner,
    _: &RunRecord,
    _: &str,
    _: u32,
    g: &crate::GateDef,
    _: Option<&ForTask<'_>>,
    _: &Value,
    _: std::sync::Arc<super::files::Reader>,
) -> GateResult {
    GateResult::errored(format!("gate {}: this build runs no scripts", g.name))
}

/// A gate judged once outside any run (`flow_gate_try`): the gate, the script it runs when it is
/// not saved yet, the directory it judges, and what the run would have had — the station judged,
/// the inputs, the stations' outputs, the output directory.
pub(crate) struct Trial {
    pub gate: crate::GateDef,
    pub script: Option<String>,
    pub ws: std::path::PathBuf,
    pub station: String,
    pub inputs: Value,
    pub outputs: serde_json::Map<String, Value>,
    pub output_dir: Option<String>,
    /// The bundle's data directory the run would have had (in Conix).
    pub data_dir: Option<String>,
    /// Why a command may not run here; `None`: it may.
    pub no_commands: Option<String>,
    /// Whether its processes may reach the network (its bundle's `exec.network`).
    pub network: bool,
    /// What the script may import and reach: the packs' bundle files (nothing is pinned in a trial).
    #[cfg(feature = "script")]
    pub env: super::scriptenv::Env,
}

/// What a trial gives back besides the result: what the gate wrote to its log.
pub(crate) struct Tried {
    pub result: GateResult,
    pub log: String,
}

impl Runner {
    /// Judge a gate as a run would, on a made-up run: the same code as in a run, nothing recorded.
    pub(crate) async fn try_gate(&self, t: Trial) -> Tried {
        try_gate(self, t).await
    }
}

async fn try_gate(rn: &Runner, t: Trial) -> Tried {
    let g = &t.gate;
    let tried = |result| Tried { result, log: String::new() };
    let stations: serde_json::Map<String, Value> = t
        .outputs
        .iter()
        .map(|(k, v)| (k.clone(), serde_json::json!({"status": "passed", "round": 1, "output": v})))
        .collect();
    let ctx = serde_json::json!({
        "run": "trial", "subject": "trial", "inputs": t.inputs, "stations": stations,
        "item": null, "task": null, "workspace": t.ws.to_string_lossy(), "output_dir": t.output_dir,
        "data_dir": t.data_dir,
    });
    let input = serde_json::json!({
        "gate": g.name, "run": "trial", "flow": "trial", "station": t.station, "round": 1,
        "inputs": t.inputs, "outputs": t.outputs, "output_dir": t.output_dir, "data_dir": t.data_dir,
        "workspace": t.ws.to_string_lossy(), "params": crate::kernel::refs::resolve(&g.params, &ctx),
    });
    match g.kind {
        GateKind::Check if g.check == Some(crate::kernel::parts::CheckKind::Sealed) => tried(GateResult::errored(
            "a `sealed` check compares the files the run's earlier gates read: it is judged only in a run".to_string(),
        )),
        GateKind::Check | GateKind::Script if g.function.is_none() => {
            let reader = match super::files::Reader::new(&t.ws) {
                Ok(r) => Arc::new(r),
                Err(e) => return tried(GateResult::errored(format!("gate {}: {e}", g.name))),
            };
            if g.kind == GateKind::Check {
                let rec = trial_record(&t.station);
                return tried(super::checks::run(g, &ctx, &rec, &reader));
            }
            let Some(source) = t.script.clone() else {
                return tried(GateResult::errored(format!("script {} is not in the packs", g.script.as_deref().unwrap_or("?"))));
            };
            #[cfg(feature = "script")]
            return trial_script(source, input, reader, trial_env(rn, &t), g.script_seconds()).await;
            #[cfg(not(feature = "script"))]
            trial_script(source, input, reader, g.script_seconds()).await
        }
        GateKind::Command => {
            if let Some(why) = &t.no_commands {
                return tried(GateResult::errored(why.clone()));
            }
            if !rn.cfg.command_gates {
                return tried(GateResult::errored(format!("command gates are not turned on here: {} cannot run", g.name)));
            }
            let dir = std::env::temp_dir().join(format!("flow-gate-try-{}-{}", std::process::id(), crate::kernel::host::digest(&ctx.to_string())));
            let site = GateSite { run: "trial", station: &t.station, ws: &t.ws, station_dir: &dir, ctx: &ctx, network: t.network };
            // A trial is no run: what it starts goes to the log.
            let audit = super::exec::Audit::log(format!("gate {} (tried)", g.name));
            #[cfg(feature = "script")]
            let result = match (&g.plan, t.script.clone(), super::files::Reader::new(&t.ws)) {
                (None, _, _) => command_at(rn, g, &site, &audit).await,
                (Some(p), None, _) => GateResult::errored(format!("script {p} is not in the packs")),
                (Some(_), _, Err(e)) => GateResult::errored(format!("gate {}: {e}", g.name)),
                (Some(_), Some(source), Ok(reader)) => {
                    super::plan::run_plan(rn, g, &site, source, input, Arc::new(reader), trial_env(rn, &t), &audit).await
                }
            };
            #[cfg(not(feature = "script"))]
            let result = command_at(rn, g, &site, &audit).await;
            let log = std::fs::read_to_string(dir.join(log_name(&g.name))).unwrap_or_default();
            let _ = std::fs::remove_dir_all(&dir);
            Tried { result, log }
        }
        _ => match (&rn.cfg.gates, g.host_function()) {
            (Some(gs), Some(f)) => tried(gs.run(&trial_record(&t.station), &t.station, f).await),
            (None, _) => tried(GateResult::errored(format!("the host runs no checks: {} cannot run", g.name))),
            (_, None) => tried(GateResult::errored(format!("gate {} is not one the host runs", g.name))),
        },
    }
}

/// What a tried gate's script reaches: the trial's, `flow.exec` reading in the directory judged,
/// recorded in the log, refused where the trial would refuse a command.
#[cfg(feature = "script")]
fn trial_env(rn: &Runner, t: &Trial) -> super::scriptenv::Env {
    let bundle = crate::bundle::split(&t.gate.name).map(|(b, _)| b);
    let scope = rn.bundle_exec_scope(rn.engine().packs().as_ref(), bundle, t.ws.clone(), t.no_commands.clone(), format!("flow.exec in gate {} (tried)", t.gate.name));
    t.env.clone().with_exec(scope)
}

/// The run a trial judges in: one station, nothing done yet.
fn trial_record(station: &str) -> RunRecord {
    let def = crate::FlowDef::parse(&format!("{{name: trial, stations: [{{id: {station:?}, exit: [{{kind: confirm}}]}}]}}"))
        .unwrap_or_else(|_| crate::FlowDef::parse("{name: trial, stations: [{id: trial, exit: [{kind: confirm}]}]}").expect("a trial flow"));
    crate::kernel::txn::new_record(
        "trial".into(),
        def,
        Default::default(),
        "trial".into(),
        "trial".into(),
        "trial".into(),
        Value::Null,
        crate::Actor::flow("trial"),
        0,
        None,
    )
}

#[cfg(feature = "script")]
async fn trial_script(source: String, input: Value, reader: Arc<super::files::Reader>, env: super::scriptenv::Env, seconds: u64) -> Tried {
    let (result, trace) = super::script::run(source, input, reader, env, Duration::from_secs(seconds)).await;
    Tried { result, log: trace.log }
}

#[cfg(not(feature = "script"))]
async fn trial_script(_: String, _: Value, _: Arc<super::files::Reader>, _: u64) -> Tried {
    Tried { result: GateResult::errored("this build runs no scripts".to_string()), log: String::new() }
}

/// The largest context also given inline (`FLOW_CONTEXT`); a larger one only by its file.
const GATE_CONTEXT_INLINE: usize = 32 * 1024;
/// How much of each output stream is kept at most (its end).
pub(crate) const GATE_OUTPUT_READ: usize = 64 * 1024;

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
            // Not `fetch_update`: deprecated on newer toolchains, and its successor is not on older ones.
            let mut n = left.load(Ordering::SeqCst);
            loop {
                if n == 0 {
                    break Ok(());
                }
                match left.compare_exchange(n, n - 1, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => break Err(StoreError::Conflict("injected".into())),
                    Err(now) => n = now,
                }
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
                // A leading `*` is a suffix: denied by it alone.
                cfg.env_deny = vec!["*KEY".into()];
            },
        )
        .await;
        g.run_it().await;
        let (station, check, detail) = g.judged().await;
        assert_eq!((station, check), (StationStatus::Passed, CheckStatus::Pass), "{detail}");
    }

    /// A command gate's process is started by the host's `Exec`: `sh -c` and the command as one
    /// argument, profile `read`, no network; what ran (not what it printed) is in the run's history.
    #[tokio::test]
    async fn a_command_gate_runs_through_the_hosts_exec_and_is_recorded_in_the_run() {
        let backend = Arc::new(super::super::exec::Recording::default());
        let b = backend.clone();
        let g = Gated::new("{name: build, kind: command, command: 'echo printed-not-kept'}", Arc::new(MemStore::new()), move |cfg| {
            cfg.command_gates = true;
            cfg.exec = Some(b);
        })
        .await;
        g.run_it().await;
        assert_eq!(g.judged().await.1, CheckStatus::Pass);
        let reqs = backend.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].argv, ["sh", "-c", "echo printed-not-kept"]);
        assert_eq!((&reqs[0].profile, reqs[0].network), (&crate::ExecProfile::Read, false));
        let history = g.engine.history(&g.run).await.unwrap();
        let line = history.iter().find(|h| h.op == "exec").expect("an exec line");
        assert_eq!(line.station.as_deref(), Some("a"));
        assert_eq!(line.detail["for"], "gate build");
        assert_eq!(line.detail["argv"], serde_json::json!(["sh", "-c", "echo printed-not-kept"]));
        assert_eq!(line.detail["profile"], serde_json::json!({"kind": "read"}));
        assert_eq!((line.detail["code"].as_i64(), line.detail["network"].as_bool()), (Some(0), Some(false)));
        assert_eq!(line.detail["enforced"], serde_json::json!({"backend": "recording", "filesystem": false, "network": false}));
        assert!(line.detail.get("stdout").is_none() && line.detail.get("stderr").is_none(), "no output in the history: {}", line.detail);
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

    /// A command gate whose plan (`scripts/plan.js`) returns `plan`, a JS expression over `input`.
    #[cfg(feature = "script")]
    async fn planned(plan: &str, timeout: u64, configure: impl FnOnce(&mut RunnerConfig), files: &[(&str, &str)]) -> Gated {
        let gate = format!("{{name: tests, kind: command, plan: plan, params: {{p: '${{run}}'}}, timeout_seconds: {timeout}}}");
        let src = format!("export default function (input, flow) {{ flow.log('planning', input.params.p); return {plan}; }}");
        // The run's workspace is there, as it is for a real run.
        let files: Vec<(&str, &str)> = [(".keep", "")].into_iter().chain(files.iter().copied()).collect();
        Gated::with(&gate, Arc::new(MemStore::new()), configure, |p| p.part(crate::PartKind::Script, "plan", &src), &files).await
    }

    #[cfg(feature = "script")]
    fn gate_check(rec: &RunRecord) -> crate::kernel::run::CheckRec {
        rec.stations["a"].checks.iter().find(|c| c.kind == "gate").unwrap().clone()
    }

    /// A bundle's gate is named `<bundle>/<gate>`: its log is still one file in the station's directory.
    #[test]
    fn a_bundles_gate_keeps_its_log_in_one_file() {
        assert_eq!(log_name("xforge/unit-tests"), "gate-xforge-unit-tests.log");
        assert_eq!(log_name("tests"), "gate-tests.log");
    }

    /// Every run exits 0: passes, the runs in order where the run works, their output in the log.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plan_passes_when_every_run_exits_0() {
        let plan = r#"{summary: "two", runs: [
            {name: "first", argv: ["sh", "-c", "echo one >> order"]},
            {name: "second", argv: ["sh", "-c", "test \"$FLOW_STATION\" = a && echo two >> order && pwd -P"], cwd: "sub"}]}"#;
        let g = planned(plan, 10, command, &[("sub/.keep", "")]).await;
        g.run_it().await;
        let rec = g.engine.record(&g.run).await.unwrap();
        let c = gate_check(&rec);
        assert_eq!((rec.stations["a"].status, c.status), (StationStatus::Passed, CheckStatus::Pass), "{c:?}");
        assert_eq!(c.detail, "two: 2 passed: first, second");
        let ws = g._ws.path().join(&g.run);
        assert_eq!(std::fs::read_to_string(ws.join("order")).unwrap(), "one\n");
        assert_eq!(std::fs::read_to_string(ws.join("sub/order")).unwrap(), "two\n");
        let dir = ws.join(".harnix/runs").join(&g.run).join("stations/a");
        let log = std::fs::read_to_string(dir.join("gate-tests.log")).unwrap();
        assert!(log.starts_with(&format!("planning {}\n", g.run)) && log.contains("== second") && log.contains("/sub\n"), "{log}");
    }

    /// What a run is given is never a command line: `;`, `$(…)`, `|` and backquotes stay characters.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plans_arguments_reach_the_program_as_they_are_with_no_shell() {
        let plan = r#"{runs: [{name: "args", argv: ["printf", "[%s]\n", "a; touch semi", "$(touch subst)", "x | touch pipe", "`touch tick`", "&& touch and"]}]}"#;
        let g = planned(plan, 10, command, &[]).await;
        g.run_it().await;
        let rec = g.engine.record(&g.run).await.unwrap();
        assert_eq!(gate_check(&rec).status, CheckStatus::Pass, "{:?}", gate_check(&rec));
        let ws = g._ws.path().join(&g.run);
        for f in ["semi", "subst", "pipe", "tick", "and"] {
            assert!(!ws.join(f).exists(), "{f} was made: a shell ran");
        }
        let log = std::fs::read_to_string(ws.join(".harnix/runs").join(&g.run).join("stations/a/gate-tests.log")).unwrap();
        assert!(log.contains("[a; touch semi]\n[$(touch subst)]\n[x | touch pipe]\n[`touch tick`]\n[&& touch and]\n"), "{log}");
    }

    /// A run that exits otherwise is a finding with its name and the end of its output; the rest still run.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plan_fails_with_a_finding_for_each_run_that_does_not_exit_0() {
        let plan = r#"{runs: [
            {name: "lint", argv: ["sh", "-c", "echo bad style >&2; exit 3"]},
            {name: "unit", argv: ["sh", "-c", "touch ran"]},
            {name: "doc", argv: ["false"]}]}"#;
        let g = planned(plan, 10, command, &[]).await;
        g.run_it().await;
        let rec = g.engine.record(&g.run).await.unwrap();
        let c = gate_check(&rec);
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.detail, "2 of 3 did not pass: lint, doc");
        assert_eq!(c.findings.iter().map(|f| f.line_text()).collect::<Vec<_>>(), ["lint exited with 3: bad style", "doc exited with 1"]);
        assert!(g._ws.path().join(&g.run).join("ran").exists(), "a failure does not stop the runs after it");
    }

    /// Out of time is an `error`, as for a command gate: killed, and the runs after it not started.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plans_run_that_runs_too_long_is_killed_and_the_gate_errs() {
        let plan = r#"{runs: [{name: "slow", argv: ["sleep", "30"], timeout_seconds: 1}, {name: "after", argv: ["touch", "after"]}]}"#;
        let g = planned(plan, 20, command, &[]).await;
        let started = std::time::Instant::now();
        g.run_it().await;
        assert!(started.elapsed() < Duration::from_secs(10));
        let c = gate_check(&g.engine.record(&g.run).await.unwrap());
        assert_eq!(c.status, CheckStatus::Error, "{c:?}");
        assert!(c.detail.contains("slow ran longer than 1s"), "{}", c.detail);
        assert_eq!(c.findings[0].rule, "slow");
        assert!(!g._ws.path().join(&g.run).join("after").exists());
        // A program that is not there does not start: also an error.
        let g = planned(r#"{runs: [{name: "ghost", argv: ["no-such-program-for-a-plan"]}]}"#, 5, command, &[]).await;
        g.run_it().await;
        let c = gate_check(&g.engine.record(&g.run).await.unwrap());
        assert_eq!(c.status, CheckStatus::Error);
        assert!(c.detail.contains("ghost did not start"), "{}", c.detail);
    }

    /// Where a run works stays in the run's workspace; a plan wrong anywhere runs nothing.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plans_cwd_outside_the_workspace_is_refused_and_nothing_runs() {
        let outside = tempfile::tempdir().unwrap();
        let link = outside.path().to_string_lossy().into_owned();
        for (cwd, why) in [("..", "leaves the run's workspace"), ("sub/../..", "leaves the run's workspace"), ("/tmp", "is absolute"), ("out", "by a symbolic link"), ("none", "No such file")] {
            let plan = format!(r#"{{runs: [{{name: "first", argv: ["touch", "first"]}}, {{name: "bad", argv: ["true"], cwd: "{cwd}"}}]}}"#);
            let g = planned(&plan, 5, command, &[("sub/.keep", "")]).await;
            let ws = g._ws.path().join(&g.run);
            std::os::unix::fs::symlink(&link, ws.join("out")).unwrap();
            g.run_it().await;
            let c = gate_check(&g.engine.record(&g.run).await.unwrap());
            assert_eq!(c.status, CheckStatus::Error, "{cwd}: {c:?}");
            assert!(c.detail.contains(why), "{cwd}: {}", c.detail);
            assert!(!ws.join("first").exists(), "{cwd}: nothing runs");
        }
        // Nor an empty program or a NUL.
        for argv in [r#"[]"#, r#"[""]"#, r#"["echo", "a\u0000b"]"#] {
            let g = planned(&format!(r#"{{runs: [{{name: "x", argv: {argv}}}]}}"#), 5, command, &[]).await;
            g.run_it().await;
            assert_eq!(gate_check(&g.engine.record(&g.run).await.unwrap()).status, CheckStatus::Error, "{argv}");
        }
    }

    /// A plan may decide by itself; with nothing to run it passes. Either way the evidence is
    /// what it read.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plan_may_return_its_result_or_nothing_to_run() {
        let plan = r#"{verdict: "fail", summary: flow.read.text("why.txt"), findings: [{rule: "menu", message: "no such check"}]}"#;
        let g = planned(plan, 5, command, &[("why.txt", "unknown check")]).await;
        g.run_it().await;
        let c = gate_check(&g.engine.record(&g.run).await.unwrap());
        assert_eq!((c.status, c.detail.as_str()), (CheckStatus::Fail, "unknown check"));
        assert_eq!(c.findings[0].line_text(), "no such check");
        assert_eq!(c.evidence.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["why.txt"]);
        for (plan, said) in [(r#"{runs: []}"#, "nothing to run"), (r#"{runs: [], summary: "没有要跑的"}"#, "没有要跑的")] {
            let g = planned(plan, 5, command, &[]).await;
            g.run_it().await;
            let c = gate_check(&g.engine.record(&g.run).await.unwrap());
            assert_eq!((c.status, c.detail.as_str()), (CheckStatus::Pass, said));
        }
        // Neither: an error.
        let g = planned(r#"{run: "make test"}"#, 5, command, &[]).await;
        g.run_it().await;
        assert_eq!(gate_check(&g.engine.record(&g.run).await.unwrap()).status, CheckStatus::Error);
    }

    /// A plan's runs are commands: only where command gates are on, and in a project only while it is trusted.
    #[cfg(feature = "script")]
    #[tokio::test]
    async fn a_plan_runs_only_where_a_command_gate_would() {
        let plan = r#"{runs: [{name: "mark", argv: ["touch", "marked"]}]}"#;
        let g = planned(plan, 5, |_| {}, &[]).await;
        g.run_it().await;
        let c = gate_check(&g.engine.record(&g.run).await.unwrap());
        assert_eq!(c.status, CheckStatus::Error);
        assert!(c.detail.contains("command gates are not turned on here"), "{}", c.detail);
        assert!(!g._ws.path().join(&g.run).join("marked").exists());

        let g = planned(plan, 5, command, &[]).await;
        let project = tempfile::tempdir().unwrap();
        let root = project.path().to_string_lossy().into_owned();
        let in_project = async || {
            let o = g.engine.create(CreateIn { flow: "g".into(), subject: "s".into(), project_root: Some(root.clone()), ..Default::default() }, Actor::device("d")).await.unwrap();
            let run = o.value["run"].as_str().unwrap().to_string();
            let round = g.engine.record(&run).await.unwrap().stations["a"].round;
            run_gate(&g.rn, run.clone(), "a".into(), round, "tests".into()).await;
            gate_check(&g.engine.record(&run).await.unwrap())
        };
        let c = in_project().await;
        assert_eq!(c.status, CheckStatus::Error);
        assert!(c.detail.contains("the project is not trusted: its command gate tests does not run"), "{}", c.detail);
        assert!(!project.path().join("marked").exists());
        g.rn.set_trust(project.path(), true);
        assert_eq!(in_project().await.status, CheckStatus::Pass);
        assert!(project.path().join("marked").exists());
    }

    /// A gate runs its command or its plan, and the plan's script is there.
    #[test]
    fn a_command_gate_has_a_command_or_a_plan() {
        let packs = MemPacks::new().command_gates().part(crate::PartKind::Script, "plan", "export default function () { return {runs: []}; }");
        let problems = |g: &str| crate::part_problems(crate::PartKind::Gate, "g", g, &packs);
        assert_eq!(problems("{name: g, kind: command, plan: plan, params: {item: '${item}'}}"), Vec::<String>::new());
        assert!(problems("{name: g, kind: command, plan: plan, command: 'true'}").iter().any(|m| m.contains("not both")));
        assert!(problems("{name: g, kind: command}").iter().any(|m| m.contains("needs its command (or a plan)")));
        assert!(problems("{name: g, kind: command, plan: ghost}").iter().any(|m| m.contains("no script ghost")));
        assert!(problems("{name: g, kind: command, plan: plan, cwd: x}").iter().any(|m| m.contains("own cwd")));
        assert!(problems("{name: g, kind: command, command: 'true', params: {a: 1}}").iter().any(|m| m.contains("go with a script gate")));
        assert!(problems("{name: g, kind: script, script: plan, plan: plan}").iter().any(|m| m.contains("go with a command gate")));
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
