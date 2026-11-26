//! What a script or command may reach besides the files it reads: the bundle's other modules
//! (`import`), the run's git workspace (`flow.git`) and the runs of its project (`flow.runs`).
//!
//! All of it is a snapshot or a table made before the script starts: the script's runtime is
//! synchronous, and nothing in it may wait for the store or start a process of its own. What git
//! says is asked of git by Rust when the script asks it, in the run's workspace and nowhere else.

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
        let mut exclude = vec![];
        match engine.layout() {
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
}

impl Env {
    /// A script that gets nothing extra.
    pub fn bare() -> Self {
        Self { modules: Modules::none(), git: None, runs: Default::default() }
    }

    /// For a script of a run: its pinned modules (`entry`: its path in the bundle), its workspace in
    /// git, and — only when something in it says `runs` — the project's runs.
    pub async fn for_run(engine: &Engine, rec: &RunRecord, ws: PathBuf, entry: Option<String>, text: &str) -> Self {
        let modules = Modules::pinned(rec, entry);
        let runs = if modules.mentions(text, "runs") { snapshot(engine, rec.project_root.as_deref()).await } else { vec![] };
        let git = Some(GitView::new(ws, rec.base_commit.clone(), engine));
        Self { modules, git, runs: Arc::new(runs) }
    }
}

impl Env {
    /// For a gate tried outside any run: the bundle's files as the packs hold them (a trial pins
    /// nothing), git in the directory judged with no base commit, and no runs.
    pub fn for_trial(engine: &Engine, packs: &dyn Packs, script: Option<&str>, text: Option<&str>, ws: PathBuf) -> Self {
        let modules = match (script.and_then(bundle::split), script.and_then(Modules::script_path), text) {
            (Some((b, _)), Some(entry), Some(text)) => Modules::from_packs(packs, b, entry, text),
            _ => Modules::none(),
        };
        Self { modules, git: Some(GitView::new(ws, None, engine)), runs: Default::default() }
    }
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
