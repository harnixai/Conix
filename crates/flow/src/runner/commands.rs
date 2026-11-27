//! A bundle's commands as an entry of their own (in Conix; docs/design/flows.md §7): run by a
//! station's session (`flow_command`), by the main conversation, or by a host for a person, with a
//! run or without one.
//!
//! | | source | `output_dir` | writes |
//! |---|---|---|---|
//! | with a run | what the run pinned | the run's | the run's directory (not `_run/`) and the bundle's data |
//! | without | the current packs | none | the bundle's data directory only |
//!
//! A call whose first argument the bundle declares `read_only` gets a writer that throws; that is
//! the enforcement, so a command's code needs no check of its own. Outside a run a **model** may
//! only make such calls: what a command writes there outlives every run and nothing records who
//! asked, so it is a person's (or a run's, where the run's record shows it).
//!
//! Writes to one bundle's data directory are serialized across runs ([`data_lock`]): two runs'
//! commands, or a command and an archive, never interleave in it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::Runner;
use crate::bundle::{self, CommandDecl};

/// How long a bundle's command may run.
pub(crate) const COMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Who asks for a command: a command tells a person's answer from a model's by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ByKind {
    Person,
    Model,
    /// A station's own step (`executor: {kind: command}`), no one asking.
    Station,
}

/// The asker, as the command's `input.by` shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandBy {
    pub kind: ByKind,
    pub id: String,
}

impl CommandBy {
    pub fn person(id: impl Into<String>) -> Self {
        Self { kind: ByKind::Person, id: id.into() }
    }
    pub fn model(id: impl Into<String>) -> Self {
        Self { kind: ByKind::Model, id: id.into() }
    }
    pub fn station(id: impl Into<String>) -> Self {
        Self { kind: ByKind::Station, id: id.into() }
    }
}

/// One call of a bundle's command.
#[derive(Debug, Clone)]
pub struct CommandCall {
    pub bundle: String,
    pub command: String,
    pub args: Vec<String>,
    /// The run it is for: its pinned source, its directory. `None`: the current packs, the
    /// bundle's data directory alone.
    pub run: Option<String>,
    pub by: CommandBy,
    /// The project it is for when there is no run (the run's own says it otherwise).
    pub project_root: Option<PathBuf>,
    /// A station's task, when the call is its own (shown to the command as `station`, `task`).
    pub station: Option<String>,
    pub task: Option<String>,
    /// More for `input` (`outputs`, `item`, … for a command step); never replaces the keys above.
    pub extra: Map<String, Value>,
}

impl CommandCall {
    pub fn new(bundle: impl Into<String>, command: impl Into<String>, args: Vec<String>, by: CommandBy) -> Self {
        Self {
            bundle: bundle.into(),
            command: command.into(),
            args,
            run: None,
            by,
            project_root: None,
            station: None,
            task: None,
            extra: Map::new(),
        }
    }
}

/// What a command answered: its text (what it printed, then what it returned) and the files it wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandReply {
    pub text: String,
    /// Relative to where the command works, in order.
    pub written: Vec<String>,
}

impl CommandReply {
    /// The text with the files written named after it, as a session is told.
    pub fn shown(&self) -> String {
        let mut text = self.text.clone();
        if !self.written.is_empty() {
            let mut w = self.written.clone();
            w.dedup();
            text.push_str(&format!("\n(written: {})", w.join(", ")));
        }
        text
    }
}

/// The lock of a bundle's data directory: held while a command that may write runs, and while the
/// directory is archived into. Per process, which is the instance's: one host owns a project's
/// runs. The registry only grows, one entry per data directory ever written.
pub(crate) fn data_lock(dir: &Path) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    let key = dir.components().collect::<PathBuf>();
    let mut g = super::lock(LOCKS.get_or_init(Default::default));
    g.entry(key).or_default().clone()
}

fn meta_of(packs: &Arc<dyn crate::kernel::host::Packs>, bundle: &str) -> Option<bundle::BundleMeta> {
    serde_yaml::from_str(&packs.bundle_file(bundle, bundle::BUNDLE_FILE)?).ok()
}

/// What a call resolves to before it runs.
struct Place {
    source: String,
    decl: Option<CommandDecl>,
    workspace: PathBuf,
    output_dir: Option<PathBuf>,
    data_dir: Option<PathBuf>,
    denied: Vec<PathBuf>,
    base: Value,
    env: super::scriptenv::Env,
}

impl Runner {
    /// Whether a call of `command` with `args` only reads, by what the bundle declares (an
    /// undeclared command, or a bundle not found, writes). `bundle.yaml` is read from the packs a
    /// project at `project_root` sees (else the application's), as a call outside a run reads it:
    /// for a host that must know before it asks a person or hands the call to a model.
    pub fn is_read_only(&self, bundle: &str, command: &str, args: &[String], project_root: Option<&Path>) -> bool {
        let meta = meta_of(&self.engine().packs_for(project_root.and_then(Path::to_str)), bundle);
        bundle::is_read_only(meta.as_ref().and_then(|m| m.commands.get(command)), args)
    }

    /// Run a bundle's command (see the module). `Err` is a refusal or a failure, said as the
    /// session or the person should read it.
    pub async fn run_command(&self, call: CommandCall) -> Result<CommandReply, String> {
        if !bundle::segment(&call.command) || !bundle::segment(&call.bundle) {
            return Err(format!("`{}:{}` is not a command's name", call.bundle, call.command));
        }
        let mut place = self.place_of(&call).await?;
        let read_only = bundle::is_read_only(place.decl.as_ref(), &call.args);
        place.env = self.command_exec(&call, place.env, &place.workspace, read_only).await;
        if let Some(text) = help(&call, &place) {
            return Ok(CommandReply { text, written: vec![] });
        }
        if call.run.is_none() && call.by.kind == ByKind::Model && !read_only {
            return Err(format!(
                "`{}` with `{}` writes, and a run is where a model may do that: run it inside a flow run \
                 (give `run`), or ask the person to run it. Outside a run only the read-only ones are for \
                 you{}.",
                call.command,
                call.args.first().map(String::as_str).unwrap_or(""),
                match place.decl.as_ref().map(|d| &d.read_only).filter(|r| !r.is_empty()) {
                    Some(r) => format!(": {}", r.iter().map(|a| format!("`{a}`")).collect::<Vec<_>>().join(", ")),
                    None => String::new(),
                }
            ));
        }
        let reader = super::files::Reader::new(&place.workspace).map_err(|e| e.0)?;
        let roots: Vec<PathBuf> = [place.output_dir.clone(), place.data_dir.clone()].into_iter().flatten().collect();
        let writer = super::script::Writer {
            workspace: place.workspace.clone(),
            roots,
            denied: place.denied.clone(),
            written: Default::default(),
            read_only,
        };
        let mut input = place.base.clone();
        for (k, v) in &call.extra {
            input.as_object_mut().unwrap().entry(k.clone()).or_insert(v.clone());
        }
        // Held for the whole run of a command that may write: what it reads of the data it then
        // changes must not move under it.
        let _held = match (&place.data_dir, read_only) {
            (Some(d), false) => Some(data_lock(d).lock_owned().await),
            _ => None,
        };
        let out = super::script::command(place.source, input, Arc::new(reader), Arc::new(writer), place.env, COMMAND_TIMEOUT).await;
        let reply = CommandReply { text: out.text, written: out.written };
        if out.ok { Ok(reply) } else { Err(reply.shown()) }
    }

    /// What `flow.exec` starts its processes with for this call: in a run, `workspace` where the
    /// call may write (`read` for a read-only one) and recorded in the run; outside one, `read` and
    /// recorded in the log, whoever asks — what outlives every run is written by the command's own
    /// writer, not by a process it starts.
    async fn command_exec(&self, call: &CommandCall, env: super::scriptenv::Env, ws: &Path, read_only: bool) -> super::scriptenv::Env {
        let what = format!("flow.exec in {}:{}", call.bundle, call.command);
        let scope = match &call.run {
            Some(run) => match self.engine().record(run).await {
                Ok(rec) => {
                    let (station, task) = (call.station.as_deref(), call.task.as_deref());
                    let profile = if read_only { crate::ExecProfile::Read } else { super::exec::command_profile(&rec, ws, station, task) };
                    self.run_exec_scope(&rec, ws.to_path_buf(), profile, station, task, what)
                }
                Err(_) => None,
            },
            None => {
                let packs = self.engine().packs_for(ws.to_str());
                self.bundle_exec_scope(packs.as_ref(), Some(&call.bundle), ws.to_path_buf(), None, what)
            }
        };
        env.with_exec(scope)
    }

    async fn place_of(&self, call: &CommandCall) -> Result<Place, String> {
        let engine = self.engine();
        let (name, args) = (call.command.as_str(), &call.args);
        let by = json!({"kind": call.by.kind, "id": call.by.id});
        if let Some(run) = &call.run {
            let rec = engine.record(run).await.map_err(|e| e.to_string())?;
            if rec.def.bundle.as_deref() != Some(call.bundle.as_str()) {
                return Err(match &rec.def.bundle {
                    Some(b) => format!("run {run} is of the bundle `{b}`, not `{}`", call.bundle),
                    None => format!("run {run}'s flow is in no bundle: it has no commands"),
                });
            }
            let have = || rec.parts.commands.keys().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ");
            let Some(source) = rec.parts.commands.get(name) else {
                return Err(if rec.parts.commands.is_empty() {
                    format!("this flow has no commands (no `{name}`)")
                } else {
                    format!("no command `{name}`; this flow has {}", have())
                });
            };
            let ws = engine.workspace_for(&rec).ok_or("the run has no place it works in")?;
            let rel = |p: &Option<String>| -> Value {
                p.as_deref().map_or(Value::Null, |p| {
                    Value::String(Path::new(p).strip_prefix(&ws).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string()))
                })
            };
            let base = json!({
                "command": name, "args": args, "by": by, "run": rec.id, "flow": rec.def.name,
                "station": call.station, "task": call.task, "subject": rec.subject, "inputs": rec.inputs,
                "output_dir": rel(&rec.output_dir), "data_dir": rel(&rec.data_dir),
            });
            let env = super::scriptenv::Env::for_run(engine, &rec, ws.clone(), Some(format!("commands/{name}.js")), source).await;
            return Ok(Place {
                env,
                source: source.clone(),
                decl: rec.parts.command_decls.get(name).cloned(),
                output_dir: rec.output_dir.as_deref().map(PathBuf::from),
                data_dir: rec.data_dir.as_deref().map(PathBuf::from),
                denied: super::project::process_dir(engine, &rec).into_iter().collect(),
                workspace: ws,
                base,
            });
        }
        let root = call.project_root.clone().ok_or("this call is for no run and no project: say which (`run`)")?;
        let packs = engine.packs_for(root.to_str());
        let meta = meta_of(&packs, &call.bundle).ok_or_else(|| format!("no bundle `{}` here", call.bundle))?;
        let Some(decl) = meta.commands.get(name) else {
            let have: Vec<String> = meta.commands.names().map(|c| format!("`{c}`")).collect();
            return Err(if have.is_empty() {
                format!("bundle `{}` has no commands (no `{name}`)", call.bundle)
            } else {
                format!("no command `{name}` in `{}`; it has {}", call.bundle, have.join(", "))
            });
        };
        let source = packs
            .bundle_file(&call.bundle, &format!("commands/{name}.js"))
            .ok_or_else(|| format!("bundle `{}` declares `{name}` but has no commands/{name}.js", call.bundle))?;
        let data = engine
            .layout()
            .and_then(|l| l.bundle_data_dir(&root, &call.bundle))
            .ok_or("this host keeps no data directory for bundles")?;
        let rel = data.strip_prefix(&root).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| data.to_string_lossy().into_owned());
        let base = json!({
            "command": name, "args": args, "by": by, "run": null, "flow": null, "station": null, "task": null,
            "output_dir": null, "data_dir": rel,
        });
        let env = super::scriptenv::Env::for_bundle(engine, packs.as_ref(), &call.bundle, format!("commands/{name}.js"), &source, root.clone()).await;
        Ok(Place { source, decl: Some(decl.clone()), workspace: root, output_dir: None, data_dir: Some(data), denied: vec![], base, env })
    }
}

/// The usage a call for `help` (or with no arguments) is answered with, without running the command.
fn help(call: &CommandCall, place: &Place) -> Option<String> {
    if !matches!(call.args.first().map(String::as_str), None | Some("help")) {
        return None;
    }
    let mut out = format!("`{}`", call.command);
    if let Some(d) = place.decl.as_ref().filter(|d| !d.description.is_empty()) {
        out.push_str(&format!(" — {}", d.description));
    }
    out.push('\n');
    match bundle::command_help(&place.source) {
        Some(h) => out.push_str(&h),
        None => out.push_str("(its file opens with no `//` usage comment)"),
    }
    if let Some(d) = place.decl.as_ref().filter(|d| !d.read_only.is_empty()) {
        out.push_str(&format!("\nRead-only: {}.", d.read_only.iter().map(|a| format!("`{a}`")).collect::<Vec<_>>().join(", ")));
    }
    Some(out)
}

/// A command named as a session or a person writes it: `xspec`, or `xforge:xspec` in a bundle's
/// name; the bundle first, then the command.
pub fn split_command<'a>(name: &'a str, default_bundle: Option<&'a str>) -> Option<(&'a str, &'a str)> {
    match name.split_once(':') {
        Some((b, c)) if bundle::segment(b) && bundle::segment(c) && !c.contains(':') => Some((b, c)),
        Some(_) => None,
        None => default_bundle.filter(|_| bundle::segment(name)).map(|b| (b, name)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_named_with_its_bundle_or_in_the_runs() {
        assert_eq!(split_command("xforge:xspec", None), Some(("xforge", "xspec")));
        assert_eq!(split_command("xspec", Some("xforge")), Some(("xforge", "xspec")));
        assert_eq!(split_command("xspec", None), None);
        assert_eq!(split_command("a:b:c", Some("x")), None);
        assert_eq!(split_command(":b", Some("x")), None);
    }
}
