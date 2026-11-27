//! What a script or command may reach besides the files it reads: the bundle's other modules
//! (`import`), the run's git workspace (`flow.git`) and the runs of its project (`flow.runs`).
//!
//! All of it is a snapshot or a table made before the script starts: the script's runtime is
//! synchronous, and nothing in it may wait for the store. What git says is asked of git by Rust
//! when the script asks it, in the run's workspace and nowhere else. The one thing a script waits
//! for is a process it starts with `flow.exec` ([`ExecScope`]): through the host's `Exec`, under a
//! profile the caller fixed, waited for on the runtime while the script's thread blocks.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use rquickjs::loader::{ImportAttributes, Loader, Resolver};
use rquickjs::module::Declared;
use rquickjs::{Ctx, Module};
use serde_json::{json, Value};

use crate::bundle;
use crate::kernel::engine::Engine;
use crate::kernel::host::Packs;
use crate::kernel::layout::ProjectLayout;
use crate::kernel::run::{RunQuery, RunRecord, RunStatus};

/// The newest runs of a project `flow.runs` can see.
const MAX_RUNS: usize = 500;

/// The modules a script may import, by path in its bundle, and the name it runs under: a module's
/// name is its bundle-relative path, so that a relative `import` is resolved against it.
#[derive(Clone, Default)]
pub(crate) struct Modules {
    /// The importing file's own name (`scripts/spec-delta.js`); `gate.js` outside a bundle.
    pub entry: String,
    /// Every file an import may reach. Only what a run pinned (or, in a trial, what the packs hold).
    pub sources: Arc<BTreeMap<String, String>>,
    /// Whether the script is a bundle's: outside one an import says so rather than "not found".
    pub in_bundle: bool,
}

impl Modules {
    /// A script that is no bundle's: no import resolves.
    pub fn none() -> Self {
        Self { entry: "gate.js".into(), sources: Default::default(), in_bundle: false }
    }

    /// The bundle path of a gate script named `xforge/spec-delta`.
    pub fn script_path(name: &str) -> Option<String> {
        bundle::split(name).map(|(_, local)| format!("scripts/{local}.js"))
    }

    /// What a run pinned: its scripts, commands and the files they import, to run `entry` (a path in
    /// the bundle; `None` for a script of no bundle).
    pub fn pinned(rec: &RunRecord, entry: Option<String>) -> Self {
        let (Some(_), Some(entry)) = (&rec.def.bundle, entry) else { return Self::none() };
        let mut sources: BTreeMap<String, String> = rec.parts.libs.clone();
        for (name, src) in &rec.parts.scripts {
            if let Some(p) = Self::script_path(name) {
                sources.insert(p, src.clone());
            }
        }
        for (name, src) in &rec.parts.commands {
            sources.insert(format!("commands/{name}.js"), src.clone());
        }
        Self { entry, sources: Arc::new(sources), in_bundle: true }
    }

    /// What the packs hold now, for a script run outside a run (a gate trial): `entry` with the text
    /// given for it, and what it imports, followed to the end.
    pub fn from_packs(packs: &dyn Packs, bundle: &str, entry: String, text: &str) -> Self {
        let mut sources = bundle::import_closure(&[(entry.as_str(), text)], &|p| packs.bundle_file(bundle, p));
        sources.insert(entry.clone(), text.to_string());
        Self { entry, sources: Arc::new(sources), in_bundle: true }
    }

    /// Whether any module's text mentions `word`: a cheap look before a snapshot is made for it.
    fn mentions(&self, text: &str, word: &str) -> bool {
        text.contains(word) || self.sources.values().any(|s| s.contains(word))
    }
}

/// Resolves an import against the importing module's name; refuses all but the bundle's own files.
pub(crate) struct BundleResolver {
    in_bundle: bool,
}

impl Resolver for BundleResolver {
    fn resolve<'js>(&mut self, ctx: &Ctx<'js>, base: &str, name: &str, _: Option<ImportAttributes<'js>>) -> rquickjs::Result<String> {
        if !self.in_bundle {
            return Err(rquickjs::Exception::throw_message(
                ctx,
                &format!("cannot import {name:?}: a script imports only from its own bundle, and this flow is in none"),
            ));
        }
        bundle::resolve_import(base, name).map_err(|e| rquickjs::Exception::throw_message(ctx, &e))
    }
}

/// Gives a module's pinned text, nothing from the disk.
pub(crate) struct PinnedLoader {
    sources: Arc<BTreeMap<String, String>>,
}

impl Loader for PinnedLoader {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str, _: Option<ImportAttributes<'js>>) -> rquickjs::Result<Module<'js, Declared>> {
        match self.sources.get(name) {
            Some(src) => Module::declare(ctx.clone(), name, src.as_str()),
            None => Err(rquickjs::Exception::throw_message(
                ctx,
                &format!("module {name} is not in the bundle (a run reads only the files its start pinned)"),
            )),
        }
    }
}

/// For the syntax check on save: every import resolves to an empty module.
pub(crate) struct AnyImport;

impl Resolver for AnyImport {
    fn resolve<'js>(&mut self, _: &Ctx<'js>, _: &str, name: &str, _: Option<ImportAttributes<'js>>) -> rquickjs::Result<String> {
        Ok(name.to_string())
    }
}

impl Loader for AnyImport {
    fn load<'js>(&mut self, ctx: &Ctx<'js>, name: &str, _: Option<ImportAttributes<'js>>) -> rquickjs::Result<Module<'js, Declared>> {
        Module::declare(ctx.clone(), name, "export default 0;")
    }
}

impl Modules {
    /// Hand the loader to a runtime.
    pub fn attach(&self, rt: &rquickjs::Runtime) {
        rt.set_loader(BundleResolver { in_bundle: self.in_bundle }, PinnedLoader { sources: self.sources.clone() });
    }
}

/// Where `flow.git` looks: the run's workspace, and the commit it is measured from.
#[derive(Clone)]
pub(crate) struct GitView {
    pub dir: PathBuf,
    pub base: Option<String>,
    /// Names relative to `dir` that are the host's and the run's own, not the work's: left out of
    /// what changed.
    pub exclude: Vec<String>,
}

impl GitView {
    pub fn new(dir: PathBuf, base: Option<String>, engine: &Engine) -> Self {
        Self::in_layout(dir, base, engine.layout())
    }

    /// The same, for a host that has the project's layout but no engine (a bundle's doctor).
    pub fn in_layout(dir: PathBuf, base: Option<String>, layout: Option<&ProjectLayout>) -> Self {
        let mut exclude = vec![];
        match layout {
            Some(l) => {
                exclude.push(l.dir.clone());
                exclude.extend(l.artifacts.clone());
            }
            None => exclude.push(".conix".to_string()),
        }
        Self { dir, base, exclude }
    }

    pub fn head(&self) -> Option<String> {
        super::git::head(&self.dir)
    }

    pub fn changed(&self, since: Option<&str>) -> Result<Vec<String>, String> {
        let Some(since) = since.filter(|s| !s.is_empty()).or(self.base.as_deref()) else {
            return Err("flow.git.changed: the run has no base commit (its workspace is no git repository): say `since`".into());
        };
        super::git::changed_since(&self.dir, since, &self.exclude)
    }
}

/// Everything beyond `flow`'s reading that a script is given.
#[derive(Clone)]
pub(crate) struct Env {
    pub modules: Modules,
    pub git: Option<GitView>,
    /// The project's runs, as `flow.runs` gives them; empty when the script says nothing of runs.
    pub runs: Arc<Vec<Value>>,
    /// `flow.settings`: what the run pinned; outside a run, the project's file now over the
    /// defaults. An `Err` is a file that does not read, thrown when the script asks for it.
    pub settings: Result<Value, String>,
    /// What `flow.exec` starts processes with; `None`: it throws, saying it is not offered here.
    pub exec: Option<ExecScope>,
}

/// What `flow.exec` starts its processes with (docs/design/flows.md §7.14): all of it fixed by
/// where the script runs — a gate's, a doctor's, a read-only call's script reads; a command that
/// writes writes where its run may — and nothing of it by the script.
#[derive(Clone)]
pub(crate) struct ExecScope {
    pub service: Arc<dyn crate::kernel::host::Exec>,
    /// The runtime the process is waited for on: the script itself runs on a thread of its own.
    pub handle: tokio::runtime::Handle,
    /// Why no process may start here (the host's switch is off, the project is not trusted):
    /// thrown at the call, so that the same code says why wherever it runs.
    pub refused: Option<String>,
    /// The directory a `cwd` is relative to and must stay in: the run's workspace, or the project.
    pub root: PathBuf,
    pub profile: crate::kernel::host::ExecProfile,
    pub network: bool,
    /// The process's whole environment: the basics and what the bundle declared, less what the
    /// host denies.
    pub env: Vec<(std::ffi::OsString, std::ffi::OsString)>,
    pub audit: super::exec::Audit,
}

impl ExecScope {
    /// A scope for a script of a bundle whose `exec` is `decl`, the host's deny list `deny`.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        service: Arc<dyn crate::kernel::host::Exec>,
        refused: Option<String>,
        root: PathBuf,
        profile: crate::kernel::host::ExecProfile,
        decl: Option<&bundle::ExecDecl>,
        deny: &[String],
        audit: super::exec::Audit,
    ) -> Option<Self> {
        // Without a runtime to wait on there is nothing to start a process with: not offered.
        let handle = tokio::runtime::Handle::try_current().ok()?;
        let wanted = decl.map(|d| d.env.clone()).unwrap_or_default();
        Some(Self {
            service,
            handle,
            refused,
            root,
            profile,
            network: decl.is_some_and(|d| d.network),
            env: super::exec::passed_env(&wanted, deny),
            audit,
        })
    }
}

/// What `flow.exec` is called with.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ExecCall {
    /// The program and its arguments: no shell, no command line.
    argv: Vec<String>,
    /// Relative to the scope's root, inside it; the root when absent.
    #[serde(default)]
    cwd: Option<String>,
    /// At most what the script has left, which is also the default.
    #[serde(default)]
    timeout_seconds: Option<f64>,
    /// Text written to the process's stdin; none: stdin is empty.
    #[serde(default)]
    stdin: Option<String>,
}

/// How much of each output stream `flow.exec` hands back (its head; `truncated` says when more came).
pub(crate) const EXEC_OUTPUT: usize = 4 * 1024 * 1024;

impl ExecScope {
    /// `flow.exec(opts)` (`opts` as JSON) for a script that must be done by `deadline`: checked
    /// whole before anything starts, started through the host's `Exec` and recorded, waited for on
    /// the runtime. A process that exits non-zero, is killed by a signal or runs out of time is an
    /// answer (`code`, `signal`, `timed_out`); one that cannot be started, or a call that is wrong,
    /// is an `Err`, thrown to the script.
    pub fn call(&self, opts: &str, deadline: std::time::Instant) -> Result<Value, String> {
        if let Some(why) = &self.refused {
            return Err(why.clone());
        }
        let call: ExecCall = serde_json::from_str(opts).map_err(|e| format!("takes {{argv: [program, ...args], cwd?, timeout_seconds?, stdin?}}: {e}"))?;
        let Some(program) = call.argv.first() else {
            return Err("argv is empty: it is the program and its arguments (no shell: a command line is not split)".into());
        };
        if program.trim().is_empty() {
            return Err("argv[0], the program, is empty".into());
        }
        if call.argv.iter().any(|a| a.contains('\0')) || call.stdin.as_deref().is_some_and(|t| t.contains('\0')) {
            return Err("an argument holds a NUL".into());
        }
        let root = std::fs::canonicalize(&self.root).map_err(|e| format!("{}: {e}", self.root.display()))?;
        let cwd = super::plan::inside(&root, call.cwd.as_deref().unwrap_or(""), "the directory the script works in")?;
        let left = deadline.saturating_duration_since(std::time::Instant::now());
        if left < std::time::Duration::from_millis(10) {
            return Err("the script has no time left to wait for a process".into());
        }
        let timeout = match call.timeout_seconds {
            Some(t) if !t.is_finite() || t <= 0.0 => return Err(format!("timeout_seconds {t} is not a number of seconds above 0")),
            Some(t) => std::time::Duration::from_secs_f64(t).min(left),
            None => left,
        };
        let mut argv = call.argv;
        // A program by path is found where the process works, as a plan's run is.
        if argv[0].contains('/') && std::path::Path::new(&argv[0]).is_relative() {
            argv[0] = cwd.join(&argv[0]).to_string_lossy().into_owned();
        }
        let mut req = crate::kernel::host::ExecRequest::new(argv, cwd, timeout);
        req.profile = self.profile.clone();
        req.network = self.network;
        req.env = self.env.clone();
        req.stdin = call.stdin.map(String::into_bytes);
        req.keep = crate::kernel::host::OutputKeep::Head(EXEC_OUTPUT);
        let (tx, rx) = std::sync::mpsc::channel();
        let (service, audit) = (self.service.clone(), self.audit.clone());
        // On the runtime, not here: this thread is the script's, and the process, its pipes and
        // its timer are the runtime's to drive.
        self.handle.spawn(async move {
            let _ = tx.send(super::exec::run_audited(service.as_ref(), req, &audit).await);
        });
        let got = rx.recv().map_err(|_| "the process was not waited for (the host is stopping)".to_string())?;
        let o = got.map_err(|e| e.to_string())?;
        Ok(json!({
            "code": o.code,
            "signal": o.signal,
            "stdout": String::from_utf8_lossy(&o.stdout),
            "stderr": String::from_utf8_lossy(&o.stderr),
            "timed_out": o.timed_out,
            "truncated": o.truncated,
            "ms": o.ms,
            "enforced": o.enforced,
        }))
    }
}

impl Env {
    /// A script that gets nothing extra.
    pub fn bare() -> Self {
        Self { modules: Modules::none(), git: None, runs: Default::default(), settings: Ok(json!({})), exec: None }
    }

    /// The same, `flow.exec` starting its processes in `scope`.
    pub fn with_exec(mut self, scope: Option<ExecScope>) -> Self {
        self.exec = scope;
        self
    }

    /// For a script of a run: its pinned modules (`entry`: its path in the bundle), its workspace in
    /// git, its pinned settings, and — only when something in it says `runs` — the project's runs.
    pub async fn for_run(engine: &Engine, rec: &RunRecord, ws: PathBuf, entry: Option<String>, text: &str) -> Self {
        let modules = Modules::pinned(rec, entry);
        let runs = if modules.mentions(text, "runs") { snapshot(engine, rec.project_root.as_deref()).await } else { vec![] };
        let git = Some(GitView::new(ws, rec.base_commit.clone(), engine));
        Self { modules, git, runs: Arc::new(runs), settings: Ok(rec.parts.settings.clone().unwrap_or_else(|| json!({}))), exec: None }
    }
}

impl Env {
    /// For a gate tried outside any run: the bundle's files as the packs hold them (a trial pins
    /// nothing), git in the directory judged with no base commit, no runs, and the settings of
    /// the directory judged as if it were the project.
    pub fn for_trial(engine: &Engine, packs: &dyn Packs, script: Option<&str>, text: Option<&str>, ws: PathBuf) -> Self {
        let modules = match (script.and_then(bundle::split), script.and_then(Modules::script_path), text) {
            (Some((b, _)), Some(entry), Some(text)) => Modules::from_packs(packs, b, entry, text),
            _ => Modules::none(),
        };
        let settings = match script.and_then(bundle::split) {
            Some((b, _)) => live_settings(packs, b, &ws),
            None => Ok(json!({})),
        };
        Self { modules, git: Some(GitView::new(ws, None, engine)), runs: Default::default(), settings, exec: None }
    }
}

impl Env {
    /// For a bundle's command run outside any run (in Conix): the bundle's files as the packs hold
    /// them now, git in the project with no base commit, and the project's runs when it says `runs`.
    pub async fn for_bundle(engine: &Engine, packs: &dyn Packs, bundle: &str, entry: String, text: &str, root: PathBuf) -> Self {
        let modules = Modules::from_packs(packs, bundle, entry, text);
        let runs = if modules.mentions(text, "runs") { snapshot(engine, root.to_str()).await } else { vec![] };
        let settings = live_settings(packs, bundle, &root);
        Self { modules, git: Some(GitView::new(root, None, engine)), runs: Arc::new(runs), settings, exec: None }
    }
}

impl Env {
    /// For a bundle's doctor (in Conix): its files as the packs hold them now, git in the project
    /// with no base commit, the project's settings now. No runs: what a project has prepared does
    /// not depend on what is in flight, and the check must say the same twice in a row.
    pub fn for_doctor(packs: &dyn Packs, layout: Option<&ProjectLayout>, bundle: &str, entry: String, text: &str, root: PathBuf) -> Self {
        let modules = Modules::from_packs(packs, bundle, entry, text);
        let settings = live_settings(packs, bundle, &root);
        Self { modules, git: Some(GitView::in_layout(root, None, layout)), runs: Default::default(), settings, exec: None }
    }
}

/// A bundle's settings in the project at `root` now; `{}` for a bundle that declares none.
fn live_settings(packs: &dyn Packs, bundle: &str, root: &std::path::Path) -> Result<Value, String> {
    bundle::read_settings(packs, bundle, Some(root)).map(|s| s.unwrap_or_else(|| json!({})))
}

/// One run as `flow.runs` shows it.
pub(crate) fn plain(rec: &RunRecord) -> Value {
    json!({
        "id": rec.id,
        "flow": rec.flow,
        "bundle": rec.def.bundle,
        "subject": rec.subject,
        "status": rec.status,
        "inputs": rec.inputs,
        "output_dir": rec.output_dir,
        "data_dir": rec.data_dir,
        "stations": rec.stations.iter().map(|(id, s)| (id.clone(), json!(s.status))).collect::<serde_json::Map<_, _>>(),
    })
}

/// The newest runs of a project, newest first. Records, not summaries: the stations' statuses are
/// in them. What cannot be loaded is left out.
async fn snapshot(engine: &Engine, project: Option<&str>) -> Vec<Value> {
    let Some(project) = project else { return vec![] };
    let q = RunQuery { project: Some(project.to_string()), limit: Some(MAX_RUNS), ..Default::default() };
    let Ok(list) = engine.list(q).await else { return vec![] };
    let mut out = vec![];
    for s in list {
        if let Ok(rec) = engine.record(&s.id).await {
            out.push(plain(&rec));
        }
    }
    out
}

impl Env {
    /// `flow.runs.list`: by bundle, and only the active ones when `active`.
    pub fn runs_list(&self, bundle: Option<&str>, active: bool) -> Vec<Value> {
        self.runs
            .iter()
            .filter(|r| bundle.is_none_or(|b| r["bundle"].as_str() == Some(b)))
            .filter(|r| !active || r["status"] == json!(RunStatus::Active))
            .cloned()
            .collect()
    }

    pub fn runs_get(&self, id: &str) -> Option<Value> {
        self.runs.iter().find(|r| r["id"].as_str() == Some(id)).cloned()
    }
}

impl super::Runner {
    /// `flow.exec`'s scope for a script of run `rec` working in `ws`, under `profile`: the host's
    /// `Exec`, the rule for every process a project asks for, the bundle's declaration the run
    /// pinned, the record into the run's history (`station`, `task`, `what` it was for).
    pub(crate) fn run_exec_scope(
        &self,
        rec: &RunRecord,
        ws: PathBuf,
        profile: crate::kernel::host::ExecProfile,
        station: Option<&str>,
        task: Option<&str>,
        what: String,
    ) -> Option<ExecScope> {
        let audit = super::exec::Audit::run(self.engine().clone(), &rec.id, station, task, what);
        let refused = self.exec_refused(rec.project_root.as_deref());
        ExecScope::new(self.exec().clone(), refused, ws, profile, rec.parts.exec.as_ref(), &self.config().env_deny, audit)
    }

    /// The same outside any run, for a bundle's script in the project at `root` (a command run for
    /// a person or a model, a gate tried): profile `read`, the bundle's declaration as it is now,
    /// the record into the log. `refused` is a reason the caller has besides the host's rule.
    pub(crate) fn bundle_exec_scope(&self, packs: &dyn Packs, bundle: Option<&str>, root: PathBuf, refused: Option<String>, what: String) -> Option<ExecScope> {
        let decl = bundle.and_then(|b| bundle::exec_decl(packs, b));
        let refused = refused.or_else(|| self.exec_refused(root.to_str()));
        let audit = super::exec::Audit::log(what);
        ExecScope::new(self.exec().clone(), refused, root, crate::kernel::host::ExecProfile::Read, decl.as_ref(), &self.config().env_deny, audit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_import_is_resolved_against_the_importer_inside_the_bundle() {
        assert_eq!(bundle::resolve_import("scripts/a.js", "../lib/x.js").unwrap(), "lib/x.js");
        assert_eq!(bundle::resolve_import("lib/a.js", "./b.js").unwrap(), "lib/b.js");
        assert_eq!(bundle::resolve_import("lib/a/b.js", "../c/./d.js").unwrap(), "lib/c/d.js");
        assert!(bundle::resolve_import("scripts/a.js", "../../x.js").unwrap_err().contains("leaves the bundle"));
        assert!(bundle::resolve_import("scripts/a.js", "fs").unwrap_err().contains("relative path"));
        assert!(bundle::resolve_import("scripts/a.js", "/etc/passwd").is_err());
        assert!(bundle::resolve_import("scripts/a.js", "./.hidden.js").is_err());
    }

    #[test]
    fn the_imports_of_a_text_are_found_in_every_form() {
        let t = "import a from './a.js';\nimport { b,\n c } from \"../lib/b.js\"\nimport './side.js';\nexport * from './re.js';\nexport { d } from './d.js';\nconst s = 'from \"x\"';";
        let mut got = bundle::imports_of(t);
        got.sort();
        assert_eq!(got, ["../lib/b.js", "./a.js", "./d.js", "./re.js", "./side.js"]);
    }
}
