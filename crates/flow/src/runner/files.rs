//! What a gate the library runs itself (`check`, `script`) may read: files under the run's
//! workspace and nothing else, every one it reads recorded as evidence (docs/design/flows.md §2,
//! in Conix).
//!
//! The evidence is the reader's, not the gate's: whatever a script says it looked at, what goes
//! into the record is what it actually read, with the hash of what it read.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::kernel::parts::{glob, ReadAs};
use crate::kernel::run::Evidence;

/// The largest file a gate reads.
pub(crate) const MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// How many directory entries one glob walks at most.
const MAX_WALK: usize = 20_000;
/// How many files one glob gives at most.
const MAX_MATCHES: usize = 2_000;
/// How deep one glob walks at most.
const MAX_DEPTH: usize = 32;

/// A path that leaves the workspace, or one that cannot be read: not the evidence's fault, so the
/// gate is an `error`, not a `fail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct OutOfBounds(pub String);

impl std::fmt::Display for OutOfBounds {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

pub(crate) struct Reader {
    /// The workspace, canonical.
    root: PathBuf,
    seen: Mutex<BTreeMap<String, Evidence>>,
}

impl Reader {
    pub(crate) fn new(root: &Path) -> Result<Self, OutOfBounds> {
        let root = std::fs::canonicalize(root).map_err(|e| OutOfBounds(format!("{}: {e}", root.display())))?;
        Ok(Self { root, seen: Mutex::new(BTreeMap::new()) })
    }

    /// The files read so far, by path.
    pub(crate) fn evidence(&self) -> Vec<Evidence> {
        self.seen.lock().unwrap().values().cloned().collect()
    }

    /// `rel` as a path under the workspace, written relative to it. An absolute path inside the
    /// workspace is taken as its relative part (`${workspace}/x` reads `x`); `..` and an absolute
    /// path elsewhere are refused.
    pub(crate) fn relative(&self, rel: &str) -> Result<String, OutOfBounds> {
        let p = Path::new(rel.trim());
        let p = if p.is_absolute() {
            match p.strip_prefix(&self.root) {
                Ok(r) => r.to_path_buf(),
                Err(_) => {
                    // The workspace may have been named through a link (`/tmp` on macOS).
                    let canon = std::fs::canonicalize(p).ok();
                    match canon.as_deref().and_then(|c| c.strip_prefix(&self.root).ok()) {
                        Some(r) => r.to_path_buf(),
                        None => return Err(OutOfBounds(format!("{rel} is outside the run's workspace"))),
                    }
                }
            }
        } else {
            p.to_path_buf()
        };
        let mut parts = vec![];
        for c in p.components() {
            match c {
                Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
                Component::CurDir => {}
                _ => return Err(OutOfBounds(format!("{rel}: a path stays inside the run's workspace (no `..`)"))),
            }
        }
        if parts.is_empty() {
            return Err(OutOfBounds(format!("{rel:?} names no file")));
        }
        Ok(parts.join("/"))
    }

    /// The file on disk, refused when it resolves (through a link) outside the workspace.
    /// `None` when there is nothing there.
    fn locate(&self, rel: &str) -> Result<Option<(String, PathBuf)>, OutOfBounds> {
        let rel = self.relative(rel)?;
        let full = self.root.join(&rel);
        match std::fs::canonicalize(&full) {
            Ok(c) if c.starts_with(&self.root) => Ok(Some((rel, c))),
            Ok(_) => Err(OutOfBounds(format!("{rel} leads outside the run's workspace"))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(OutOfBounds(format!("{rel}: {e}"))),
        }
    }

    /// A file's bytes, recorded as evidence; `None` when it does not exist.
    pub(crate) fn read(&self, rel: &str) -> Result<Option<(String, Vec<u8>)>, OutOfBounds> {
        let Some((rel, path)) = self.locate(rel)? else { return Ok(None) };
        let meta = std::fs::metadata(&path).map_err(|e| OutOfBounds(format!("{rel}: {e}")))?;
        if !meta.is_file() {
            return Err(OutOfBounds(format!("{rel} is not a file")));
        }
        if meta.len() > MAX_FILE_BYTES {
            return Err(OutOfBounds(format!("{rel} is larger than {} MiB", MAX_FILE_BYTES / (1024 * 1024))));
        }
        let bytes = std::fs::read(&path).map_err(|e| OutOfBounds(format!("{rel}: {e}")))?;
        self.record(&rel, &bytes);
        Ok(Some((rel, bytes)))
    }

    /// A file's text (lossy), recorded as evidence.
    pub(crate) fn text(&self, rel: &str) -> Result<Option<(String, String)>, OutOfBounds> {
        Ok(self.read(rel)?.map(|(r, b)| (r, String::from_utf8_lossy(&b).into_owned())))
    }

    /// A file's size and hash, recorded as evidence.
    pub(crate) fn stat(&self, rel: &str) -> Result<Option<Evidence>, OutOfBounds> {
        Ok(self.read(rel)?.map(|(r, b)| evidence(&r, &b)))
    }

    fn record(&self, rel: &str, bytes: &[u8]) {
        self.seen.lock().unwrap().insert(rel.to_string(), evidence(rel, bytes));
    }

    /// The files a pattern names, relative and sorted: a pattern without `*` or `?` is itself
    /// (when it is a file); one with them is matched against the workspace's files (`*` within a
    /// segment, `**` across). `.git` is not walked, nor links to directories.
    pub(crate) fn glob(&self, pattern: &str) -> Result<Vec<String>, OutOfBounds> {
        let pat = pattern.trim();
        if !pat.contains(['*', '?']) {
            return Ok(match self.locate(pat)? {
                Some((rel, p)) if p.is_file() => vec![rel],
                _ => vec![],
            });
        }
        let pat = match Path::new(pat).is_absolute() {
            true => self.relative(pat)?,
            false => pat.trim_start_matches("./").to_string(),
        };
        if pat.split('/').any(|s| s == "..") {
            return Err(OutOfBounds(format!("{pattern}: a pattern stays inside the run's workspace (no `..`)")));
        }
        // Walk from the part before the first wildcard.
        let fixed: Vec<&str> = pat.split('/').take_while(|s| !s.contains(['*', '?'])).collect();
        let base = fixed.join("/");
        let start = if base.is_empty() { self.root.clone() } else { self.root.join(&base) };
        let mut out = vec![];
        let mut walked = 0usize;
        let mut stack = vec![(start, base, 0usize)];
        while let Some((dir, rel, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for e in entries.flatten() {
                walked += 1;
                if walked > MAX_WALK {
                    return Err(OutOfBounds(format!("{pattern}: more than {MAX_WALK} entries to look through")));
                }
                let name = e.file_name().to_string_lossy().into_owned();
                let child = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
                let Ok(ft) = e.file_type() else { continue };
                if ft.is_dir() {
                    if name != ".git" && depth < MAX_DEPTH {
                        stack.push((e.path(), child, depth + 1));
                    }
                } else if (ft.is_file() || ft.is_symlink()) && glob(&pat, &child, true) {
                    // A link is only taken when it resolves to a file inside the workspace.
                    if let Ok(Some((_, p))) = self.locate(&child) {
                        if p.is_file() {
                            out.push(child);
                        }
                    }
                    if out.len() > MAX_MATCHES {
                        return Err(OutOfBounds(format!("{pattern}: more than {MAX_MATCHES} files")));
                    }
                }
            }
        }
        out.sort();
        Ok(out)
    }
}

pub(crate) fn evidence(rel: &str, bytes: &[u8]) -> Evidence {
    Evidence { path: rel.to_string(), sha256: hex(&Sha256::digest(bytes)), size: bytes.len() as u64 }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A file's text read as a value. `Err` says why it is not one.
pub(crate) fn parse(text: &str, read_as: ReadAs) -> Result<Value, String> {
    match read_as {
        ReadAs::Json => serde_json::from_str(text).map_err(|e| format!("not JSON: {e}")),
        ReadAs::Yaml => yaml(text),
        ReadAs::Jsonl => text
            .lines()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| serde_json::from_str(l).map_err(|e| format!("line {}: not JSON: {e}", i + 1)))
            .collect::<Result<Vec<Value>, String>>()
            .map(Value::Array),
        ReadAs::Frontmatter => {
            let body = text.strip_prefix('\u{feff}').unwrap_or(text);
            let Some(rest) = body.strip_prefix("---\n").or_else(|| body.strip_prefix("---\r\n")) else {
                return Err("no front matter (a first line of `---`)".into());
            };
            let end = rest
                .find("\n---\n")
                .or_else(|| rest.find("\n---\r\n"))
                .or_else(|| rest.strip_suffix("\n---").map(|r| r.len()))
                .ok_or("the front matter is not closed with `---`")?;
            match yaml(&rest[..end])? {
                Value::Null => Ok(Value::Object(Default::default())),
                v @ Value::Object(_) => Ok(v),
                _ => Err("the front matter is not a mapping".into()),
            }
        }
    }
}

fn yaml(text: &str) -> Result<Value, String> {
    let v: serde_yaml::Value = serde_yaml::from_str(text).map_err(|e| format!("not YAML: {e}"))?;
    serde_json::to_value(v).map_err(|e| format!("YAML that is not JSON-shaped: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws() -> (tempfile::TempDir, Reader) {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("docs/adr")).unwrap();
        std::fs::write(d.path().join("docs/a.md"), "# A\n").unwrap();
        std::fs::write(d.path().join("docs/adr/1.md"), "one").unwrap();
        std::fs::write(d.path().join("docs/adr/2.txt"), "two").unwrap();
        let r = Reader::new(d.path()).unwrap();
        (d, r)
    }

    #[test]
    fn reads_inside_the_workspace_and_records_what_it_read() {
        let (_d, r) = ws();
        assert_eq!(r.text("docs/a.md").unwrap().unwrap().1, "# A\n");
        assert_eq!(r.text("./docs/missing.md").unwrap(), None);
        let ev = r.evidence();
        assert_eq!(ev.len(), 1);
        assert_eq!((ev[0].path.as_str(), ev[0].size), ("docs/a.md", 4));
        assert_eq!(ev[0].sha256.len(), 64);
    }

    #[test]
    fn a_path_out_of_the_workspace_is_refused() {
        let (d, r) = ws();
        assert!(r.read("../x").is_err());
        assert!(r.read("/etc/hosts").is_err());
        let abs = d.path().join("docs/a.md");
        assert_eq!(r.read(abs.to_str().unwrap()).unwrap().unwrap().0, "docs/a.md");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink("/etc", d.path().join("out")).unwrap();
            assert!(r.read("out/hosts").is_err());
        }
    }

    #[test]
    fn globs_match_within_and_across_segments() {
        let (_d, r) = ws();
        assert_eq!(r.glob("docs/*.md").unwrap(), ["docs/a.md"]);
        assert_eq!(r.glob("docs/**/*.md").unwrap(), ["docs/a.md", "docs/adr/1.md"]);
        assert_eq!(r.glob("docs/adr/2.txt").unwrap(), ["docs/adr/2.txt"]);
        assert!(r.glob("nothing/*.md").unwrap().is_empty());
        assert!(r.glob("../*").is_err());
    }

    #[test]
    fn files_read_as_values() {
        assert_eq!(parse("{\"a\":1}", ReadAs::Json).unwrap(), serde_json::json!({"a": 1}));
        assert_eq!(parse("a: [1, 2]\n", ReadAs::Yaml).unwrap(), serde_json::json!({"a": [1, 2]}));
        assert_eq!(parse("{\"n\":1}\n\n{\"n\":2}\n", ReadAs::Jsonl).unwrap(), serde_json::json!([{"n": 1}, {"n": 2}]));
        assert!(parse("{\"n\":1}\nnope\n", ReadAs::Jsonl).unwrap_err().starts_with("line 2"));
        let md = "---\nstatus: done\nowner: x\n---\n# Body\n";
        assert_eq!(parse(md, ReadAs::Frontmatter).unwrap(), serde_json::json!({"status": "done", "owner": "x"}));
        assert!(parse("# no front matter\n", ReadAs::Frontmatter).is_err());
        assert_eq!(ReadAs::of("a/b.jsonl"), ReadAs::Jsonl);
        assert_eq!(ReadAs::of("README.md"), ReadAs::Frontmatter);
    }
}
