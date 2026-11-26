//! Checking an archive later (docs/design/layout.md §3): `record.json` lists every other file of the
//! archive directory with its digest, and the run's ledger (`RunRecord::archive`, which the model
//! cannot write) holds the digest of `record.json` itself. Nothing is chained: an edit shows
//! because the listing no longer matches the files, the evidence and confirmation digests no
//! longer match what the gates read and a person approved, or `record.json` no longer matches the
//! ledger — and the committed archive's git history shows the rest. [`verify`] is that check; it
//! reads, never repairs.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::kernel::engine::Engine;
use crate::kernel::run::RunRecord;

/// The archive's record, beside the files.
pub const RECORD: &str = "record.json";
/// The format `record.json` is written in; the first archives have none, and are read as before.
pub const RECORD_FORMAT: u32 = 1;

/// One file of an archive as `record.json` lists it: the same `{path, sha256, size}` as a package's
/// MANIFEST (flows.md §7.12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    /// Relative to the archive directory, `/` separated.
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

/// A digest in `record.json` (a gate's evidence, a confirmation's binding) that no longer matches
/// the file beside the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DigestMismatch {
    pub path: String,
    /// What recorded it: `check <station>/<label>` or `confirm <station>`.
    pub by: String,
    /// `changed` or `missing`.
    pub problem: String,
}

/// How `record.json` stands against the ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordState {
    /// The ledger's digest is the file's.
    Matches,
    /// The record was written after the archive was made.
    Differs,
    /// No ledger entry to compare with: none was given, or the run's ledger does not record this
    /// archive (an older one).
    NoLedger,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ArchiveReport {
    /// Nothing is missing, extra, changed or mismatched, the record is readable and does not
    /// differ from the ledger. `unlisted` and `no_ledger` are not failures: they say what could not
    /// be checked.
    pub ok: bool,
    /// `record.json` has no `files` (an archive from before it was written): the listing was not
    /// checked, the evidence digests were.
    pub unlisted: bool,
    /// Listed, not there.
    pub missing: Vec<String>,
    /// There, not listed (empty when `unlisted`).
    pub extra: Vec<String>,
    /// Listed and there, with another digest or size.
    pub changed: Vec<String>,
    pub evidence: Vec<DigestMismatch>,
    pub record: RecordState,
    /// What could not be read or understood, each a line; any makes `ok` false.
    pub problems: Vec<String>,
    /// How many files were hashed.
    pub checked: usize,
}

/// `file`'s sha256 and size, streamed.
pub fn hash_file(file: &Path) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(file)?;
    let (mut h, mut n, mut buf) = (Sha256::new(), 0u64, vec![0u8; 64 * 1024]);
    loop {
        let r = f.read(&mut buf)?;
        if r == 0 {
            break;
        }
        h.update(&buf[..r]);
        n += r as u64;
    }
    Ok((hex(&h.finalize()), n))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Every file under `dir` but `record.json` in its root, sorted by path. A link to a directory is
/// not entered (it could lead out of the archive), a link to a file is read through.
pub fn list_files(dir: &Path) -> std::io::Result<Vec<FileEntry>> {
    let mut out = vec![];
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((d, rel)) = stack.pop() {
        for e in std::fs::read_dir(&d)? {
            let e = e?;
            let name = e.file_name().to_string_lossy().into_owned();
            let path = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            if e.file_type()?.is_dir() {
                stack.push((e.path(), path));
            } else if path != RECORD {
                let (sha256, size) = hash_file(&e.path())?;
                out.push(FileEntry { path, sha256, size });
            }
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// A path as `record.json` may name a file in its own directory: relative, nothing to climb out by.
fn inside(path: &str) -> bool {
    !path.is_empty() && !path.starts_with('/') && !path.split('/').any(|s| s == ".." || s.is_empty())
}

/// Check the archive in `dir`. `ledger` is the run's authoritative record ([`ledger_record`]).
pub fn verify(dir: &Path, ledger: Option<&RunRecord>) -> ArchiveReport {
    let mut rep = ArchiveReport {
        ok: false,
        unlisted: false,
        missing: vec![],
        extra: vec![],
        changed: vec![],
        evidence: vec![],
        record: RecordState::NoLedger,
        problems: vec![],
        checked: 0,
    };
    let bytes = match std::fs::read(dir.join(RECORD)) {
        Ok(b) => b,
        Err(e) => {
            rep.problems.push(format!("{RECORD}: {e}"));
            return rep;
        }
    };
    if let Some(a) = ledger.and_then(|l| l.archive.as_ref()) {
        rep.record = if a.record_sha256 == hex(&Sha256::digest(&bytes)) { RecordState::Matches } else { RecordState::Differs };
    }
    let record: Value = match serde_json::from_slice(&bytes) {
        Ok(v) => v,
        Err(e) => {
            rep.problems.push(format!("{RECORD}: not JSON: {e}"));
            return rep;
        }
    };
    let files = match list_files(dir) {
        Ok(f) => f,
        Err(e) => {
            rep.problems.push(format!("{}: {e}", dir.display()));
            return rep;
        }
    };
    rep.checked = files.len();
    let actual: BTreeMap<&str, &FileEntry> = files.iter().map(|f| (f.path.as_str(), f)).collect();

    match record.get("files").map(|f| serde_json::from_value::<Vec<FileEntry>>(f.clone())) {
        None => rep.unlisted = true,
        Some(Err(e)) => rep.problems.push(format!("{RECORD}: files: {e}")),
        Some(Ok(listed)) => {
            for f in &listed {
                match actual.get(f.path.as_str()) {
                    None => rep.missing.push(f.path.clone()),
                    Some(a) if a.sha256 != f.sha256 || a.size != f.size => rep.changed.push(f.path.clone()),
                    Some(_) => {}
                }
            }
            let names: BTreeSet<&str> = listed.iter().map(|f| f.path.as_str()).collect();
            rep.extra = actual.keys().filter(|p| !names.contains(*p)).map(|p| p.to_string()).collect();
        }
    }

    // What the gates read and what a person approved, as `record.json` quotes it. A path that is
    // not a file of the archive was not kept (or is a missing one, named above): no digest to check
    // here. The digests are sha256 of the bytes, as `files` are.
    let mut digests: Vec<(&str, &str, String)> = vec![];
    for st in record.get("stations").and_then(Value::as_array).into_iter().flatten() {
        let sid = st["id"].as_str().unwrap_or("?");
        for c in st.get("checks").and_then(Value::as_array).into_iter().flatten() {
            let by = format!("check {sid}/{}", c["label"].as_str().unwrap_or("?"));
            for e in c.get("evidence").and_then(Value::as_array).into_iter().flatten() {
                if let (Some(p), Some(h)) = (e["path"].as_str(), e["sha256"].as_str()) {
                    digests.push((p, h, by.clone()));
                }
            }
        }
        for c in st.get("confirms").and_then(Value::as_array).into_iter().flatten() {
            for (p, h) in c.get("binds").and_then(Value::as_object).into_iter().flatten() {
                if let Some(h) = h.as_str() {
                    digests.push((p, h, format!("confirm {sid}")));
                }
            }
        }
    }
    let gone: BTreeSet<&str> = rep.missing.iter().map(String::as_str).collect();
    for (path, want, by) in digests {
        if !inside(path) {
            continue;
        }
        let problem = match actual.get(path) {
            Some(a) if a.sha256 == want => continue,
            Some(_) => "changed",
            None if gone.contains(path) => "missing",
            None => continue,
        };
        let m = DigestMismatch { path: path.to_string(), by, problem: problem.to_string() };
        if !rep.evidence.contains(&m) {
            rep.evidence.push(m);
        }
    }

    rep.ok = rep.problems.is_empty()
        && rep.missing.is_empty()
        && rep.extra.is_empty()
        && rep.changed.is_empty()
        && rep.evidence.is_empty()
        && rep.record != RecordState::Differs;
    rep
}

/// The ledger's record of the run an archive says it is of: `record.json`'s `run`, read from
/// `engine`'s store. `None` when the record or the run is not there; the engine must be the one of
/// the project the archive belongs to (its store is the project's).
pub async fn ledger_record(engine: &Engine, dir: &Path) -> Option<RunRecord> {
    let v: Value = serde_json::from_slice(&std::fs::read(dir.join(RECORD)).ok()?).ok()?;
    engine.record(v["run"].as_str()?).await.ok()
}
