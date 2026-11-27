//! A bundle's self-check for a project (in Conix; docs/design/flows.md §7.13): `bundle.yaml`'s
//! `doctor` names a script of the bundle, and a host runs it outside any run to tell a person what
//! the project has not prepared for the bundle — a settings file that does not read, a menu the
//! bundle's commands would refuse, a file its gates look for.
//!
//! What every bundle is checked for (that its flows validate, its version, the host's lock files)
//! is the host's; only the bundle knows what it needs of a project, so that part is its own script.
//! The script runs as a script gate does: read-only, deterministic, the same sandbox and time limit;
//! `flow.read.*` over the project, `flow.settings` the project's file now over the defaults,
//! `flow.git.*` in the project with no base commit, no `flow.runs`. It says what is wrong and how
//! a person would fix it, and never fixes anything itself: a check that wrote would make running it
//! a change to review.
//!
//! ```js
//! // scripts/doctor.js; input: {bundle, project, data_dir, params: {}}
//! export default function (input, flow) {
//!   return { findings: [{ level: "warn", message: "…", path: ".conix/…", fix: "…" }] };
//! }
//! ```

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::bundle;
use crate::kernel::host::Packs;
use crate::kernel::layout::ProjectLayout;
use crate::kernel::parts::PartKind;

/// How long a doctor may run: a script gate's default.
pub const DOCTOR_TIMEOUT: Duration = Duration::from_secs(30);

/// How much a finding matters, least first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DoctorLevel {
    /// Worth knowing, nothing to do yet (a directory the first run makes).
    Info,
    /// Something a flow of the bundle will miss or refuse later; the rest works.
    Warn,
    /// What the bundle cannot run with as the project stands.
    Error,
}

/// One thing a doctor found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DoctorFinding {
    pub level: DoctorLevel,
    pub message: String,
    /// The project's file it is about, relative to the project's root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// What a person would do about it, in words: shown, never run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl DoctorFinding {
    fn error(message: String) -> Self {
        Self { level: DoctorLevel::Error, message, path: None, fix: None }
    }
}

/// What a bundle's doctor said of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoctorReport {
    pub bundle: String,
    /// The script that was run (`scripts/doctor.js`); `None` when the bundle declares no doctor,
    /// and the findings are then empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub script: Option<String>,
    /// In the order the script gave them. A script that failed, or answered in another shape, is
    /// one `error` naming the script.
    pub findings: Vec<DoctorFinding>,
    /// What the script logged (`flow.log`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub log: String,
}

impl DoctorReport {
    /// How many findings are of a level.
    pub fn count(&self, level: DoctorLevel) -> usize {
        self.findings.iter().filter(|f| f.level == level).count()
    }

    /// The highest level found; `None` when nothing was.
    pub fn worst(&self) -> Option<DoctorLevel> {
        self.findings.iter().map(|f| f.level).max()
    }
}

/// What a doctor's script must return.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    findings: Vec<DoctorFinding>,
}

/// Run the doctor of `bundle`, as `packs` offer it, for the project at `project`. `layout` is the
/// host's (where the bundle's data directory is, what `flow.git.changed` leaves out); without one
/// the script's `data_dir` is `null`. `Err` only when the bundle is not there or its `bundle.yaml`
/// does not read; whatever goes wrong with the script itself is a finding.
pub async fn doctor(packs: &dyn Packs, layout: Option<&ProjectLayout>, bundle: &str, project: &Path) -> Result<DoctorReport, String> {
    let text = packs.bundle_file(bundle, bundle::BUNDLE_FILE).ok_or_else(|| format!("no bundle `{bundle}` here"))?;
    let meta = bundle::BundleMeta::parse(&text).map_err(|e| format!("bundle {bundle}: {e}"))?;
    let mut report = DoctorReport { bundle: bundle.to_string(), script: None, findings: vec![], log: String::new() };
    let Some(name) = meta.doctor else { return Ok(report) };
    let entry = format!("scripts/{name}.js");
    report.script = Some(entry.clone());
    let failed = |why: String| DoctorFinding::error(format!("the doctor of bundle {bundle} ({entry}): {why}"));
    let source = match packs.part(PartKind::Script, &bundle::in_bundle(bundle, &name)) {
        Some((src, _)) if bundle::segment(&name) => src,
        _ => {
            report.findings.push(failed("not in the bundle".into()));
            return Ok(report);
        }
    };
    let reader = match super::files::Reader::new(project) {
        Ok(r) => Arc::new(r),
        Err(e) => {
            report.findings.push(failed(format!("cannot read the project: {e}")));
            return Ok(report);
        }
    };
    let data_dir = layout.and_then(|l| l.bundle_data_dir(project, bundle)).map(|d| {
        d.strip_prefix(project).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| d.to_string_lossy().into_owned())
    });
    let input = json!({"bundle": bundle, "project": project.to_string_lossy(), "data_dir": data_dir, "params": {}});
    let env = super::scriptenv::Env::for_doctor(packs, layout, bundle, entry.clone(), &source, project.to_path_buf());
    let (got, log) = super::script::value_logged(source, input, reader, env, DOCTOR_TIMEOUT).await;
    report.log = log;
    match got.and_then(answer) {
        Ok(findings) => report.findings = findings,
        Err(e) => report.findings.push(failed(e)),
    }
    Ok(report)
}

/// The findings in what a doctor returned, or what is wrong with its shape.
fn answer(v: Value) -> Result<Vec<DoctorFinding>, String> {
    let a: Answer = serde_json::from_value(v).map_err(|e| {
        format!("answered in another shape than {{findings: [{{level: error|warn|info, message, path?, fix?}}]}}: {e}")
    })?;
    if let Some(i) = a.findings.iter().position(|f| f.message.trim().is_empty()) {
        return Err(format!("answered a finding with no message (findings[{i}])"));
    }
    Ok(a.findings)
}
