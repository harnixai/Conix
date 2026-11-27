//! A `command` gate's plan (docs/design/flows.md §2.2, in Conix): a bundle script picks what to
//! run, the library runs it.
//!
//! Why not let the script hand back a command line, or build one from the work package: what
//! the script reads was written by a model, and a string handed to `sh -c` is a program. Here
//! the script is policy — read-only and deterministic like a gate's script, picking from what
//! people declared — and every run it plans is started by its `argv`, program and arguments as
//! given, with no shell between: `;`, `$(…)` or `|` in an argument stay characters in that
//! argument. Where a run works is kept inside the run's workspace, and the processes get what a
//! command gate's do (the environment, the trust, the switch, the time): the plan is a command
//! gate whose command lines nobody types.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use super::files::Reader;
use super::exec::Audit;
use super::gate::{gate_request, output_tail, ForTask, GateSite};
use super::scriptenv::{Env, Modules};
use super::Runner;
use crate::kernel::host::{ExecError, GateResult};
use crate::kernel::parts::DEFAULT_SCRIPT_SECONDS;
use crate::kernel::run::{Finding, RunRecord};

/// The most runs one plan may give.
pub(crate) const MAX_RUNS: usize = 32;

/// What a plan gives when it is not a gate result: the runs, in order.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    runs: Vec<Run>,
    #[serde(default)]
    summary: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Run {
    name: String,
    argv: Vec<String>,
    /// Relative to the run's workspace; the workspace when absent.
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    timeout_seconds: Option<u64>,
}

/// A run of the plan, checked: what to start, where, for how long at most.
struct Checked {
    name: String,
    program: PathBuf,
    args: Vec<String>,
    cwd: PathBuf,
    seconds: Option<u64>,
}

/// Judge a `command` gate with a `plan` in a run: as a command gate, the commands being the ones
/// its pinned script plans.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn plan_gate(
    rn: &Runner,
    rec: &RunRecord,
    station: &str,
    round: u32,
    g: &crate::GateDef,
    task: Option<&ForTask<'_>>,
) -> GateResult {
    let fail = GateResult::errored;
    if !rn.config().command_gates {
        return fail(format!("command gates are not turned on here: {} cannot run", g.name));
    }
    let engine = rn.engine();
    let (Some(ws), Some(station_dir)) = (engine.workspace_for(rec), super::project::station_dir(engine, rec, station)) else {
        return fail(format!("gate {}: the run has no workspace", g.name));
    };
    let name = g.plan.clone().unwrap_or_default();
    let Some(source) = rec.parts.scripts.get(&name).cloned() else {
        return fail(format!("script {name} was not in the packs when the run started"));
    };
    let reader = match Reader::new(&ws) {
        Ok(r) => Arc::new(r),
        Err(e) => return fail(format!("gate {}: {e}", g.name)),
    };
    let ctx = super::gate::gate_context(rec, task, ws.to_str());
    let input = super::gate::script_input(rec, station, round, g, &ctx);
    let scope = rn.run_exec_scope(rec, ws.clone(), crate::ExecProfile::Read, Some(station), task.map(|t| t.task), format!("flow.exec in gate {}", g.name));
    let env = Env::for_run(engine, rec, ws.clone(), Modules::script_path(&name), &source).await.with_exec(scope);
    let site = GateSite { run: &rec.id, station, ws: &ws, station_dir: &station_dir, ctx: &ctx, network: super::gate::exec_network(rec) };
    let audit = Audit::run(engine.clone(), &rec.id, Some(station), task.map(|t| t.task), format!("gate {}", g.name));
    run_plan(rn, g, &site, source, input, reader, env, &audit).await
}

/// Run the plan's script at a site, then the runs it plans. The evidence is what the script
/// read; the log (`gate-<name>.log` in the site's directory) what it logged and what each run
/// printed.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_plan(
    rn: &Runner,
    g: &crate::GateDef,
    site: &GateSite<'_>,
    source: String,
    input: Value,
    reader: Arc<Reader>,
    env: Env,
    audit: &Audit,
) -> GateResult {
    // The plan only reads and picks: a script's own time, whatever the runs are given.
    let (planned, mut log) =
        super::script::value_logged(source, input, reader.clone(), env, Duration::from_secs(DEFAULT_SCRIPT_SECONDS)).await;
    let mut result = match planned.and_then(read_plan) {
        Err(e) => GateResult::errored(format!("gate {}: the plan: {e}", g.name)),
        Ok(Planned::Result(r)) => r,
        Ok(Planned::Runs(plan)) => run_all(rn, g, site, plan, &mut log, audit).await,
    };
    result.evidence = reader.evidence();
    let dir = site.station_dir;
    if let Err(e) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(dir.join(super::gate::log_name(&g.name)), &log)) {
        tracing::warn!(run = site.run, gate = g.name, error = %e, "a gate's log not written");
    }
    result
}

/// What a plan's script gave back.
enum Planned {
    Runs(Plan),
    /// A result to take as it is: nothing to run decides it.
    Result(GateResult),
}

fn read_plan(v: Value) -> Result<Planned, String> {
    let is_result = v.get("verdict").is_some() || v.get("pass").is_some();
    if is_result {
        return serde_json::from_value::<GateResult>(v).map(Planned::Result).map_err(|e| format!("not a gate result: {e}"));
    }
    serde_json::from_value::<Plan>(v).map(Planned::Runs).map_err(|e| format!("neither a gate result nor {{runs: [{{name, argv, cwd?, timeout_seconds?}}]}}: {e}"))
}

/// Check every run before any starts: a plan that is wrong anywhere runs nothing.
fn check(plan: &Plan, ws: &Path, budget: u64) -> Result<Vec<Checked>, String> {
    if plan.runs.len() > MAX_RUNS {
        return Err(format!("{} runs; at most {MAX_RUNS}", plan.runs.len()));
    }
    let root = std::fs::canonicalize(ws).map_err(|e| format!("{}: {e}", ws.display()))?;
    let mut out = vec![];
    for (i, r) in plan.runs.iter().enumerate() {
        let at = |why: String| format!("runs[{i}] ({}): {why}", r.name);
        if r.name.trim().is_empty() {
            return Err(format!("runs[{i}] has no name"));
        }
        let Some((program, args)) = r.argv.split_first() else {
            return Err(at("argv is empty: it is the program and its arguments".into()));
        };
        if program.trim().is_empty() {
            return Err(at("argv[0], the program, is empty".into()));
        }
        if r.argv.iter().any(|a| a.contains('\0')) {
            return Err(at("an argument holds a NUL".into()));
        }
        let cwd = inside(&root, r.cwd.as_deref().unwrap_or(""), "the run's workspace").map_err(at)?;
        // A program by path is found where the run works, not where this process happens to be:
        // which of the two a relative path means is up to the platform otherwise.
        let program = if program.contains('/') && Path::new(program).is_relative() { cwd.join(program) } else { PathBuf::from(program) };
        if r.timeout_seconds == Some(0) {
            return Err(at("timeout_seconds is 0".into()));
        }
        out.push(Checked { name: r.name.clone(), program, args: args.to_vec(), cwd, seconds: r.timeout_seconds.map(|s| s.min(budget)) });
    }
    Ok(out)
}

/// `rel` as a directory of `root` (canonical; `what` names it in a refusal): relative, no `..`, and
/// still inside once symbolic links are followed.
pub(crate) fn inside(root: &Path, rel: &str, what: &str) -> Result<PathBuf, String> {
    let p = Path::new(rel.trim());
    if p.is_absolute() {
        return Err(format!("cwd {rel} is absolute: it is a directory of {what}, written relative to it"));
    }
    if p.components().any(|c| !matches!(c, Component::Normal(_) | Component::CurDir)) {
        return Err(format!("cwd {rel} leaves {what}"));
    }
    let full = std::fs::canonicalize(root.join(p)).map_err(|e| format!("cwd {rel}: {e}"))?;
    if !full.starts_with(root) {
        return Err(format!("cwd {rel} leaves {what} by a symbolic link"));
    }
    if !full.is_dir() {
        return Err(format!("cwd {rel} is not a directory"));
    }
    Ok(full)
}

/// Run the plan's runs one after another. Each that does not exit 0 is a finding and the rest
/// still run; one that cannot start or runs out of time ends the plan with an `error`, as it
/// ends a command gate. The gate's `timeout_seconds` is the time for all of them.
async fn run_all(rn: &Runner, g: &crate::GateDef, site: &GateSite<'_>, plan: Plan, log: &mut String, audit: &Audit) -> GateResult {
    let budget = g.timeout_seconds();
    let runs = match check(&plan, site.ws, budget) {
        Ok(r) => r,
        Err(e) => return GateResult::errored(format!("gate {}: the plan: {e}", g.name)),
    };
    let lead = |s: String| if plan.summary.is_empty() { s } else { format!("{}: {s}", plan.summary) };
    if runs.is_empty() {
        return GateResult::passed(if plan.summary.is_empty() { "nothing to run".to_string() } else { plan.summary.clone() });
    }
    let context = site.ctx.to_string();
    let context_file = site.station_dir.join("context.json");
    if let Err(e) = std::fs::create_dir_all(site.station_dir).and_then(|_| std::fs::write(&context_file, &context)) {
        return GateResult::errored(format!("gate {}: {}: {e}", g.name, site.station_dir.display()));
    }
    let started = Instant::now();
    let (mut passed, mut findings) = (vec![], vec![]);
    for r in &runs {
        let shown = std::iter::once(r.program.to_string_lossy().into_owned()).chain(r.args.iter().cloned()).collect::<Vec<_>>();
        log.push_str(&format!("== {} {:?} in {}\n", r.name, shown, r.cwd.display()));
        let left = budget.saturating_sub(started.elapsed().as_secs());
        let seconds = r.seconds.unwrap_or(budget).min(left);
        let errored = |findings: Vec<Finding>, why: String| {
            let mut out = GateResult::errored(lead(why));
            out.findings = findings;
            out
        };
        if seconds == 0 {
            findings.push(Finding::new(&r.name, format!("{}: not run, the gate's {budget}s are used up", r.name)));
            return errored(findings, format!("{} not run: the runs before it took the gate's {budget}s", r.name));
        }
        let argv: Vec<String> = shown.clone();
        let req = gate_request(rn, argv, r.cwd.clone(), g, site, &context_file, &context, seconds);
        let (o, e, code) = match super::exec::run_audited(rn.exec().as_ref(), req, &audit.for_(format!("gate {} run {}", g.name, r.name))).await {
            Ok(o) if o.timed_out => {
                log.push_str(&format!("ran longer than {seconds}s: killed\n"));
                findings.push(Finding::new(&r.name, format!("{}: ran longer than {seconds}s", r.name)));
                return errored(findings, format!("{} ran longer than {seconds}s", r.name));
            }
            Ok(o) => (o.stdout, o.stderr, o.code),
            Err(ExecError::NotStarted(e) | ExecError::Refused(e)) => {
                log.push_str(&format!("did not start: {e}\n"));
                findings.push(Finding::new(&r.name, format!("{}: {} did not start: {e}", r.name, shown[0])));
                return errored(findings, format!("{} did not start: {e}", r.name));
            }
            Err(ExecError::Lost(e)) => {
                findings.push(Finding::new(&r.name, format!("{}: was lost: {e}", r.name)));
                return errored(findings, format!("{} was lost: {e}", r.name));
            }
        };
        let mut text = String::from_utf8_lossy(&o).into_owned();
        text.push_str(&String::from_utf8_lossy(&e));
        log.push_str(&text);
        if !text.ends_with('\n') && !text.is_empty() {
            log.push('\n');
        }
        let tail = output_tail(&text);
        let said = if tail.is_empty() { String::new() } else { format!(": {tail}") };
        match code {
            Some(0) => passed.push(r.name.clone()),
            Some(code) => findings.push(Finding::new(&r.name, format!("{} exited with {code}{said}", r.name))),
            None => {
                findings.push(Finding::new(&r.name, format!("{} was stopped by a signal{said}", r.name)));
                return errored(findings, format!("{} was stopped by a signal", r.name));
            }
        }
    }
    if findings.is_empty() {
        return GateResult::passed(lead(format!("{} passed: {}", passed.len(), passed.join(", "))));
    }
    let failed: Vec<&str> = findings.iter().map(|f| f.rule.as_str()).collect();
    let mut out = GateResult::failed(lead(format!("{} of {} did not pass: {}", failed.len(), runs.len(), failed.join(", "))));
    out.findings = findings;
    out
}
