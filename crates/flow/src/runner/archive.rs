//! A run's archive (in Conix: its docs/design/layout.md §3): once a bundle's run is over, what it
//! handed in is copied from the run's own directory (`<artifacts>/runs/<run>/`, not committed) to
//! its bundle's (`<data_dir>/archive/<name>/`, committed), with a `record.json` of how it went.
//!
//! What is kept: the flow's `archive.keep`; else every station's deliverables; else all of the run's
//! directory but the process directory. The record is a projection of the store's, as `_run/` is:
//! subject, inputs, each station's checks (paths in their evidence made relative to the archive),
//! confirmations, deliveries, usage — no leases, tokens, logs or contexts. A run is archived once:
//! the process directory keeps `archived.json` saying where. Failing is logged and changes nothing.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::kernel::def::{ArchiveWhen, DEFAULT_ARCHIVE_NAME};
use crate::kernel::engine::Engine;
use crate::kernel::layout::PROCESS_DIR;
use crate::kernel::output::OutputVars;
use crate::kernel::run::{RunRecord, RunStatus};

use super::files::Reader;
use super::project::{claim, local_date};

/// What the process directory keeps once the run is archived.
const ARCHIVED: &str = "archived.json";
/// The record in an archive.
pub const RECORD: &str = "record.json";

/// Archive `rec` if it is a bundle's (or says `archive:`), is over as its flow wants, and was not
/// archived yet; `process` is its process directory.
pub(crate) fn archive(engine: &Engine, rec: &RunRecord, process: &Path) {
    let when = rec.def.archive.as_ref().and_then(|a| a.when).unwrap_or_default();
    let over = match rec.status {
        RunStatus::Finished => true,
        RunStatus::Cancelled => when == ArchiveWhen::Always,
        RunStatus::Active => false,
    };
    if !over || (rec.def.bundle.is_none() && rec.def.archive.is_none()) || process.join(ARCHIVED).exists() {
        return;
    }
    // A child works in its parent's directory: the parent's archive holds what it wrote.
    if rec.parent.is_some() {
        return;
    }
    let (Some(run_dir), Some(data)) = (rec.output_dir.as_deref(), rec.data_dir.as_deref()) else { return };
    match archive_into(engine, rec, Path::new(run_dir), Path::new(data)) {
        Ok(to) => {
            let note = json!({"to": to.to_string_lossy(), "at": engine.clock().now_ms()});
            if let Err(e) = std::fs::write(process.join(ARCHIVED), pretty(&note)) {
                tracing::warn!(run = rec.id, error = %e, "the archive was made but not noted");
            }
        }
        Err(e) => tracing::warn!(run = rec.id, error = %e, "the run was not archived"),
    }
}

fn archive_into(engine: &Engine, rec: &RunRecord, run_dir: &Path, data: &Path) -> Result<PathBuf, String> {
    let keep = kept(rec, run_dir)?;
    let template = rec.def.archive.as_ref().and_then(|a| a.name.clone()).unwrap_or_else(|| DEFAULT_ARCHIVE_NAME.to_string());
    let date = local_date(rec.finished_at.unwrap_or_else(|| engine.clock().now_ms()));
    let flow = own_name(rec);
    let vars = OutputVars { flow: &flow, date: &date, title: &rec.title, inputs: &rec.inputs };
    let base = data.join("archive");
    std::fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
    let (to, _) = claim(&base, &template, &vars)?;
    for rel in &keep {
        let (from, dest) = (run_dir.join(rel), to.join(rel));
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::copy(&from, &dest).map_err(|e| format!("{}: {e}", from.display()))?;
    }
    std::fs::write(to.join(RECORD), pretty(&record(rec, run_dir, &keep))).map_err(|e| format!("{RECORD}: {e}"))?;
    Ok(to)
}

/// A flow's own name in its bundle: `xforge/solid` → `solid`, `brief` → `brief`.
fn own_name(rec: &RunRecord) -> String {
    rec.def.name.rsplit('/').next().unwrap_or(&rec.def.name).to_string()
}

/// The files kept, relative to the run's directory, in order.
fn kept(rec: &RunRecord, run_dir: &Path) -> Result<Vec<String>, String> {
    let reader = Reader::new(run_dir).map_err(|e| format!("{}: {e:?}", run_dir.display()))?;
    let mut patterns: Vec<String> = rec.def.archive.as_ref().map(|a| a.keep.clone()).unwrap_or_default();
    if patterns.is_empty() {
        patterns = rec.def.stations.iter().flat_map(|s| s.deliverables.iter().map(|d| d.path.clone())).collect();
    }
    if patterns.is_empty() {
        patterns.push("**".to_string());
    }
    let mut out = std::collections::BTreeSet::new();
    for p in &patterns {
        for f in reader.glob(p).map_err(|e| format!("{p}: {e:?}"))? {
            if f != PROCESS_DIR && !f.starts_with(&format!("{PROCESS_DIR}/")) && run_dir.join(&f).is_file() {
                out.insert(f);
            }
        }
    }
    Ok(out.into_iter().collect())
}

/// How the run went, for people reading the archive.
fn record(rec: &RunRecord, run_dir: &Path, kept: &[String]) -> Value {
    let stations: Vec<Value> = rec
        .def
        .stations
        .iter()
        .filter_map(|s| {
            let r = rec.stations.get(&s.id)?;
            let tasks: Vec<Value> = r
                .tasks
                .iter()
                .map(|t| json!({
                    "id": t.id, "status": t.status, "attempt": t.attempt, "item": t.item, "output": t.output,
                    "error": t.error, "delivery": t.delivery, "started_at": t.started_at, "ended_at": t.ended_at,
                    "usage": t.usage,
                }))
                .collect();
            Some(json!({
                "id": s.id, "title": s.title, "status": r.status, "rounds": r.rounds_total,
                "entered_at": r.entered_at, "left_at": r.left_at, "usage": r.usage(),
                "tasks": tasks, "checks": r.checks, "confirms": r.confirms, "approvals": r.approvals,
                "forced": r.forced,
            }))
        })
        .collect();
    let v = json!({
        "run": rec.id, "flow": rec.def.name, "bundle": rec.def.bundle, "title": rec.title,
        "subject": rec.subject, "inputs": rec.inputs, "status": rec.status,
        "created_at": rec.created_at, "created_by": rec.created_by, "finished_at": rec.finished_at,
        "project_root": rec.project_root, "worktree": rec.worktree.as_ref().map(|w| json!({"branch": w.branch, "base": w.base})),
        "kept": kept, "stations": stations,
    });
    // Evidence named the run's directory: in the archive the same files are beside the record.
    let text = v.to_string();
    let abs = format!("{}/", run_dir.to_string_lossy().trim_end_matches('/'));
    let mut text = text.replace(&abs, "");
    if let Some(root) = rec.project_root.as_deref() {
        if let Ok(rel) = run_dir.strip_prefix(root) {
            text = text.replace(&format!("{}/", rel.to_string_lossy()), "");
        }
    }
    serde_json::from_str(&text).unwrap_or(v)
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default() + "\n"
}
