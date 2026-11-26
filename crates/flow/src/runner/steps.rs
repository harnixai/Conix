//! The work of a `foreach` station's items and of a task's hand-in that needs no model: a bundle's
//! command run as a station's task (`executor: {kind: command}`), a `foreach: {script}` worked out,
//! a `task_exit` gate run on one task's hand-in — and, for whoever handed in, the wait for those
//! gates to answer.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::dispatch::{apply, dispatcher, fail, renew, renew_every, settle};
use super::gate::{judge_gate, ForTask};
use super::state::TaskKey;
use super::Runner;
use crate::kernel::run::{RunRecord, TaskStatus};
use crate::kernel::FlowError;

/// How long a command step may run: as long as a command a session runs.
const STEP_TIMEOUT: Duration = Duration::from_secs(30);
/// How long a `foreach` script may run.
const ITEMS_TIMEOUT: Duration = Duration::from_secs(30);

/// What a command step is given: what `flow_command` gives a session's command, the task's own
/// besides (its item, the stations' outputs), and `by` the station.
fn step_input(rec: &RunRecord, ws: &std::path::Path, station: &str, task: &str, command: &str, args: &[String], item: Option<&Value>) -> Value {
    let rel = |p: &Option<String>| -> Value {
        p.as_deref().map_or(Value::Null, |p| {
            Value::String(std::path::Path::new(p).strip_prefix(ws).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string()))
        })
    };
    let outputs: serde_json::Map<String, Value> =
        rec.def.stations.iter().map(|s| (s.id.clone(), crate::kernel::refs::station_output(&rec.def, rec, &s.id))).collect();
    json!({
        "command": command, "args": args, "by": {"kind": "station", "id": station},
        "run": rec.id, "flow": rec.def.name, "station": station, "task": task, "subject": rec.subject,
        "inputs": rec.inputs, "outputs": outputs, "item": item,
        "output_dir": rel(&rec.output_dir), "data_dir": rel(&rec.data_dir),
    })
}

/// Run one of the run's bundle's commands (pinned when it started) as a station's task: what it
/// returns, or why it failed. It writes where a session's command may, no more.
#[cfg(feature = "script")]
async fn run_step(rn: &Runner, rec: &RunRecord, station: &str, task: &str, command: &str, args: &[String], item: Option<&Value>) -> Result<Value, String> {
    let engine = rn.engine();
    let source = rec.parts.commands.get(command).ok_or_else(|| format!("the run's bundle has no command {command}"))?;
    let ws = engine.workspace_for(rec).ok_or("the run has no place it works in")?;
    let reader = super::files::Reader::new(&ws).map_err(|e| e.0)?;
    let roots: Vec<std::path::PathBuf> = [&rec.output_dir, &rec.data_dir].into_iter().flatten().map(std::path::PathBuf::from).collect();
    let denied: Vec<std::path::PathBuf> = super::project::process_dir(engine, rec).into_iter().collect();
    let writer = super::script::Writer { workspace: ws.clone(), roots, denied, written: Default::default() };
    let input = step_input(rec, &ws, station, task, command, args, item);
    let out = super::script::command(source.clone(), input, Arc::new(reader), Arc::new(writer), STEP_TIMEOUT).await;
    if out.ok {
        Ok(out.value)
    } else {
        Err(if out.text.is_empty() { format!("command {command} failed") } else { out.text })
    }
}

#[cfg(not(feature = "script"))]
async fn run_step(_: &Runner, _: &RunRecord, _: &str, _: &str, command: &str, _: &[String], _: Option<&Value>) -> Result<Value, String> {
    Err(format!("this build runs no scripts: command {command} cannot run"))
}

/// A command task: the command run under the task's claim (kept alive meanwhile), what it returns
/// handed in — an object as it is, anything else as `{result}` — or its failure reported.
pub(crate) async fn command_task(rn: &Arc<Runner>, key: &TaskKey, token: &str, command: &str, args: &[String], item: Option<&Value>) {
    let engine = rn.engine();
    let TaskKey { run, station, task, .. } = key;
    let rec = match engine.record(run).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(run, error = %e, "command not run");
            return;
        }
    };
    let result = {
        let running = run_step(rn, &rec, station, task, command, args, item);
        tokio::pin!(running);
        let every = renew_every(rn);
        loop {
            tokio::select! {
                r = &mut running => break r,
                _ = tokio::time::sleep(every) => {
                    if !renew(rn, key, token).await {
                        return;
                    }
                }
                _ = rn.cancelled().cancelled() => return,
            }
        }
    };
    let output = match result {
        Ok(v @ Value::Object(_)) => v,
        Ok(v) => json!({"result": v}),
        Err(e) => return fail(rn, key, token, &e).await,
    };
    match engine.submit(run, station, task, token, output, dispatcher()).await {
        Ok(o) => apply(rn, Some(run), o).await,
        Err(FlowError::Contract(m)) => fail(rn, key, token, &m).await,
        Err(e) => tracing::debug!(run, station, task, error = %e, "command output not taken"),
    }
}

/// A `foreach: {script}` station's items: the script run read-only on the run, its list reported.
pub(crate) async fn run_items(rn: &Arc<Runner>, run: String, station: String, round: u32, script: String) {
    let engine = rn.engine();
    let rec = match engine.record(&run).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(run, error = %e, "items not worked out");
            return;
        }
    };
    let items = items_of(rn, &rec, &station, round, &script).await;
    let r = engine.foreach_items(&run, &station, round, items).await;
    settle(rn, &run, "foreach_items", r).await;
}

#[cfg(feature = "script")]
async fn items_of(rn: &Runner, rec: &RunRecord, station: &str, round: u32, script: &str) -> Result<Value, String> {
    let source = rec.parts.scripts.get(script).ok_or_else(|| format!("script {script} was not in the packs when the run started"))?;
    let ws = rn.engine().workspace_for(rec).ok_or("the run has no place it works in")?;
    let reader = super::files::Reader::new(&ws).map_err(|e| e.0)?;
    let outputs: serde_json::Map<String, Value> =
        rec.def.stations.iter().map(|s| (s.id.clone(), crate::kernel::refs::station_output(&rec.def, rec, &s.id))).collect();
    let input = json!({
        "run": rec.id, "flow": rec.flow, "station": station, "round": round, "subject": rec.subject,
        "inputs": rec.inputs, "outputs": outputs, "output_dir": rec.output_dir, "data_dir": rec.data_dir,
        "workspace": ws.to_string_lossy(),
    });
    super::script::value(source.clone(), input, Arc::new(reader), ITEMS_TIMEOUT).await
}

#[cfg(not(feature = "script"))]
async fn items_of(_: &Runner, _: &RunRecord, _: &str, _: u32, script: &str) -> Result<Value, String> {
    Err(format!("this build runs no scripts: {script} cannot give the items"))
}

/// A `task_exit` gate on one task's hand-in: judged with the task's context, its result offered
/// until it is taken (a conflict is no reason to run a command again).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_task_gate(rn: &Arc<Runner>, run: String, station: String, round: u32, task: String, seq: u32, cond: usize, gate: String) {
    let engine = rn.engine();
    let rec = match engine.record(&run).await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(run, error = %e, "task gate not run");
            return;
        }
    };
    let item = rec.stations.get(&station).and_then(|s| s.task(&task)).and_then(|t| t.item.clone());
    let result = judge_gate(rn, &rec, &station, round, &gate, Some(&ForTask { task: &task, item: item.as_ref() })).await;
    let mut wait = Duration::from_millis(50);
    loop {
        match engine.task_gate_result(&run, &station, round, &task, seq, cond, result.clone()).await {
            Err(FlowError::Conflict(why)) => {
                tracing::warn!(run, station, task, gate, why, "a task gate's result met conflicts; offered again");
                tokio::select! {
                    _ = tokio::time::sleep(wait) => {}
                    // Still waiting in the run: the next start runs it again.
                    _ = rn.cancelled().cancelled() => return,
                }
                wait = (wait * 2).min(Duration::from_secs(5));
            }
            r => {
                settle(rn, &run, "task_gate_result", r).await;
                return;
            }
        }
    }
}

/// What became of a hand-in waited on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Checked {
    /// Taken: the task is handed in.
    Taken,
    /// Sent back, with what the task's gates found; the claim is still the task's.
    SentBack(String),
    /// Still claimed, and nothing was sent back: no hand-in of it is waiting.
    Open,
    /// The claim is no longer this one, or the runner stops.
    Gone,
}

/// Wait until the task's hand-in under `token` is no longer being checked, keeping the claim
/// alive meanwhile (a session's turn may be waiting on it).
pub(crate) async fn wait_checked(rn: &Arc<Runner>, key: &TaskKey, token: &str) -> Checked {
    let mut events = rn.events();
    let every = renew_every(rn);
    loop {
        let Ok(rec) = rn.engine().record(&key.run).await else { return Checked::Gone };
        let Some(t) = rec.stations.get(&key.station).and_then(|s| s.task(&key.task)) else { return Checked::Gone };
        if !t.lease.as_ref().is_some_and(|l| l.token == token) {
            return Checked::Gone;
        }
        if !t.checking() {
            return match (t.status, crate::kernel::handin::sent_back(t)) {
                (TaskStatus::Submitted, _) => Checked::Taken,
                (TaskStatus::Claimed, Some(why)) => Checked::SentBack(why),
                (TaskStatus::Claimed, None) => Checked::Open,
                _ => Checked::Gone,
            };
        }
        tokio::select! {
            e = events.recv() => {
                if matches!(e, Err(tokio::sync::broadcast::error::RecvError::Closed)) {
                    return Checked::Gone;
                }
            }
            _ = tokio::time::sleep(every) => {
                if !renew(rn, key, token).await {
                    return Checked::Gone;
                }
            }
            _ = rn.cancelled().cancelled() => return Checked::Gone,
        }
    }
}
