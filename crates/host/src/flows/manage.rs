//! 流程包的管理（docs/design/flows.md §7.12）：全局库与项目之间的装、更新、回填、删、导入、清理。
//! 只有命令行（`conix flow …`）调，实例不经这里写。每个改东西的操作先出一份 [`Plan`]（要做什么、
//! 版本从哪到哪、增删改的文件、备份到哪），给人确认后再 [`apply`]；`apply` 拿锁后再看一眼目标，
//! 确认期间被别人改了就不做。
//!
//! 项目里装的包记在 `.conix/flows.lock.yaml`（进 git）：从全局库装的哪一版、摘要。拿它判"项目里改过没有"。
//! 摘要不算 `skills/*/local.md`：那是项目自己的补充，永远留在项目里，更新时带过去。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use harnix_flow::bundle;
use serde::{Deserialize, Serialize};

use super::library::{self, BuiltinState, Entry, Library, Source, version_key};
use super::package::{self, Files, ManifestRequires};

/// 项目里记装了什么的文件（相对项目根），进 git。不放进 `.conix/flows/`：那里的每个 yaml 都当流程读。
pub const LOCK_FILE: &str = ".conix/flows.lock.yaml";
/// 命令行写项目的流程包时拿的 flock（相对项目根）。
const PROJECT_LOCK: &str = ".conix/state/conix/flows-cli.lock";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockEntry {
    pub from: String,
    pub version: String,
    pub digest: String,
    #[serde(default)]
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectLock {
    #[serde(default)]
    pub bundles: BTreeMap<String, LockEntry>,
}

pub fn read_lock(project: &Path) -> Result<ProjectLock, String> {
    let p = project.join(LOCK_FILE);
    match std::fs::read_to_string(&p) {
        Ok(t) => serde_yaml::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(ProjectLock::default()),
        Err(e) => Err(format!("{}: {e}", p.display())),
    }
}

pub fn write_lock(project: &Path, lock: &ProjectLock) -> Result<(), String> {
    let text = serde_yaml::to_string(lock).map_err(|e| e.to_string())?;
    library::write_atomic(
        &project.join(LOCK_FILE),
        format!("# {}\n{text}", i18n::t!("flowlib.lock_header")).as_bytes(),
    )
}

/// 管理在哪个项目、哪个全局库上做。
#[derive(Debug, Clone)]
pub struct Ctx {
    pub project: PathBuf,
    pub library: Library,
}

impl Ctx {
    pub fn new(project: &Path, home: &Path) -> Ctx {
        Ctx {
            project: project.to_path_buf(),
            library: Library::new(home),
        }
    }

    pub fn flows_dir(&self) -> PathBuf {
        self.project.join(package::PROJECT_FLOWS)
    }

    /// 项目里这个包的目录（有 `bundle.yaml` 才算）。
    pub fn project_dir(&self, name: &str) -> Option<PathBuf> {
        bundle::bundle_in(&self.flows_dir(), name)
    }

    pub fn project_backups(&self) -> PathBuf {
        self.project.join(package::BACKUPS)
    }

    pub fn project_state(&self) -> PathBuf {
        self.project.join(package::STATE)
    }

    /// 项目里的包（带 `bundle.yaml` 的子目录）。
    pub fn project_names(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(self.flows_dir())
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| self.project_dir(n).is_some())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }
}

/// 一处的一个包：目录、`bundle.yaml` 的版本、摘要（读不出来是空的）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Side {
    pub dir: PathBuf,
    pub version: String,
    pub digest: String,
}

fn side(dir: Option<PathBuf>) -> Option<Side> {
    let dir = dir?;
    let version = bundle::meta(&dir)
        .map(|m| package::version_of(&m))
        .unwrap_or_default();
    let digest = package::dir_digest(&dir).unwrap_or_default();
    Some(Side {
        dir,
        version,
        digest,
    })
}

/// 一个包在项目、lock、全局库里各是什么样。
#[derive(Debug, Clone)]
pub struct Row {
    pub name: String,
    pub project: Option<Side>,
    pub global: Option<Side>,
    pub lock: Option<LockEntry>,
    pub entry: Option<Entry>,
    pub builtin: Option<BuiltinState>,
}

/// 项目这一边的状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// 从全局装的，与 lock 一样。
    Installed,
    /// 从全局装的，之后在项目里改过。
    Modified,
    /// 全局库里有更新的版本。
    Update(String),
    /// 项目自己的（lock 里没有）。
    ProjectOnly,
    /// 全局有、项目没装。
    NotInstalled,
}

impl Row {
    /// 对 lock 改过。
    pub fn modified(&self) -> bool {
        matches!((&self.project, &self.lock), (Some(p), Some(l)) if p.digest != l.digest)
    }

    /// 全局库里比装的那版新：回新的版本。
    pub fn update(&self) -> Option<String> {
        let (g, l) = (self.global.as_ref()?, self.lock.as_ref()?);
        self.project.as_ref()?;
        (version_key(&g.version) > version_key(&l.version)).then(|| g.version.clone())
    }

    pub fn statuses(&self) -> Vec<Status> {
        match (&self.project, &self.lock) {
            (None, _) if self.global.is_some() => vec![Status::NotInstalled],
            (None, _) => vec![],
            (Some(_), None) => vec![Status::ProjectOnly],
            (Some(_), Some(_)) => {
                let mut out = vec![if self.modified() {
                    Status::Modified
                } else {
                    Status::Installed
                }];
                if let Some(v) = self.update() {
                    out.push(Status::Update(v));
                }
                out
            }
        }
    }
}

/// 项目、lock、全局库里出现过的每个包一行，按名字。
pub fn rows(ctx: &Ctx) -> Result<Vec<Row>, String> {
    let lock = read_lock(&ctx.project)?;
    let reg = ctx.library.registry()?;
    let mut names: BTreeSet<String> = ctx.project_names().into_iter().collect();
    names.extend(ctx.library.names());
    names.extend(lock.bundles.keys().cloned());
    names.extend(reg.bundles.keys().cloned());
    Ok(names
        .into_iter()
        .map(|name| Row {
            project: side(ctx.project_dir(&name)),
            global: side(ctx.library.dir(&name)),
            lock: lock.bundles.get(&name).cloned(),
            entry: reg.bundles.get(&name).cloned(),
            builtin: ctx.library.builtin_state(&name, &reg),
            name,
        })
        .collect())
}

pub fn row(ctx: &Ctx, name: &str) -> Result<Row, String> {
    let lock = read_lock(&ctx.project)?;
    let reg = ctx.library.registry()?;
    Ok(Row {
        name: name.to_string(),
        project: side(ctx.project_dir(name)),
        global: side(ctx.library.dir(name)),
        lock: lock.bundles.get(name).cloned(),
        entry: reg.bundles.get(name).cloned(),
        builtin: ctx.library.builtin_state(name, &reg),
    })
}

// ── 增删改 ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Changes {
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
}

impl Changes {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.changed.is_empty() && self.removed.is_empty()
    }

    pub fn all(&self) -> Vec<String> {
        let mut v: Vec<String> = self
            .added
            .iter()
            .chain(&self.changed)
            .chain(&self.removed)
            .cloned()
            .collect();
        v.sort();
        v
    }
}

/// 从 `old` 变成 `new`：多出来的、内容变了的、没了的文件。
pub fn diff(old: Option<&Files>, new: &Files) -> Changes {
    let old: BTreeMap<&str, &[u8]> = old
        .into_iter()
        .flatten()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    let new: BTreeMap<&str, &[u8]> = new
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_slice()))
        .collect();
    let mut c = Changes::default();
    for (p, b) in &new {
        match old.get(p) {
            None => c.added.push(p.to_string()),
            Some(o) if o != b => c.changed.push(p.to_string()),
            _ => {}
        }
    }
    c.removed = old
        .keys()
        .filter(|p| !new.contains_key(*p))
        .map(|p| p.to_string())
        .collect();
    c
}

fn files_of(dir: Option<&Path>) -> Result<Option<Files>, String> {
    dir.map(package::collect).transpose()
}

// ── 计划 ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Install,
    Update,
    Remove,
    Backfill,
    Reset,
    Import,
    Clean,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Update => "update",
            Action::Remove => "remove",
            Action::Backfill => "backfill",
            Action::Reset => "reset",
            Action::Import => "import",
            Action::Clean => "clean",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Where {
    Project,
    Global,
}

/// 一个改东西的操作要做什么：给人看、确认后照它做。
#[derive(Debug, Clone)]
pub struct Plan {
    pub action: Action,
    pub target: Where,
    pub bundle: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub changes: Changes,
    /// 旧的备份进哪个目录（有旧的才有）。
    pub backup: Option<PathBuf>,
    /// 没什么要做：为什么。
    pub noop: Option<String>,
    /// 要留意的：命令门禁、遮住了谁、回填的是内置包……
    pub notes: Vec<String>,
    /// `clean` 要删的目录与文件、要去掉的孤儿条目。
    pub paths: Vec<PathBuf>,
    pub orphans: Vec<String>,
    /// 只把这个包记进 lock（内容已经一样）。
    pub lock_only: bool,
    files: Option<Files>,
    requires: Option<ManifestRequires>,
    /// 计划时目标的摘要（没有是 `None`）：`apply` 拿锁后再对一次。
    expect: Option<String>,
}

impl Plan {
    fn new(action: Action, target: Where, bundle: &str) -> Plan {
        Plan {
            action,
            target,
            bundle: bundle.to_string(),
            from: None,
            to: None,
            changes: Changes::default(),
            backup: None,
            noop: None,
            notes: vec![],
            paths: vec![],
            orphans: vec![],
            lock_only: false,
            files: None,
            requires: None,
            expect: None,
        }
    }

    pub fn is_noop(&self) -> bool {
        self.noop.is_some()
    }
}

fn gate_note(dir: &Path) -> Option<String> {
    let n = package::command_gates(dir);
    (n > 0).then(|| i18n::t!("flowlib.note_gates", n = n))
}

fn digest_opt(dir: Option<&Path>) -> Option<String> {
    dir.map(|d| package::dir_digest(d).unwrap_or_default())
}

/// `install <包> [--force]`：全局 → 项目。项目里已有同名的拒绝（用 update），`--force` 换掉（先备份）。
pub fn plan_install(ctx: &Ctx, name: &str, force: bool) -> Result<Plan, String> {
    let g = ctx
        .library
        .dir(name)
        .ok_or_else(|| i18n::t!("flowlib.no_global", name = name))?;
    let files = package::collect(&g)?;
    let existing = ctx.project_dir(name);
    if existing.is_some() && !force {
        return Err(i18n::t!("flowlib.install_exists", name = name));
    }
    let mut p = Plan::new(Action::Install, Where::Project, name);
    let old = files_of(existing.as_deref())?;
    p.changes = diff(old.as_ref(), &files);
    p.from = existing.as_deref().and_then(version_in);
    p.to = version_in(&g);
    p.backup = existing.as_ref().map(|_| ctx.project_backups());
    p.expect = digest_opt(existing.as_deref());
    p.notes.extend(gate_note(&g));
    p.files = Some(files);
    Ok(p)
}

fn version_in(dir: &Path) -> Option<String> {
    bundle::meta(dir).ok().map(|m| package::version_of(&m))
}

/// `update <包> [--force]`：全局 → 项目，按 lock 判项目改过没有（flows.md §7.12 的表）。
pub fn plan_update(ctx: &Ctx, name: &str, force: bool) -> Result<Plan, String> {
    let pd = ctx
        .project_dir(name)
        .ok_or_else(|| i18n::t!("flowlib.not_installed", name = name))?;
    let g = ctx
        .library
        .dir(name)
        .ok_or_else(|| i18n::t!("flowlib.no_global", name = name))?;
    let files = package::collect(&g)?;
    let old = package::collect(&pd)?;
    let lock = read_lock(&ctx.project)?;
    let locked = lock.bundles.get(name);
    let (pdig, gdig) = (package::digest_of(&old), package::digest_of(&files));
    let (pv, gv) = (
        version_in(&pd).unwrap_or_default(),
        version_in(&g).unwrap_or_default(),
    );
    let mut p = Plan::new(Action::Update, Where::Project, name);
    p.from = Some(pv.clone());
    p.to = Some(gv.clone());
    p.expect = Some(pdig.clone());
    if pdig == gdig {
        if locked.is_some_and(|l| l.digest == gdig) {
            p.noop = Some(i18n::t!("flowlib.update_same", name = name, version = gv));
        } else {
            p.lock_only = true;
            p.notes.push(i18n::t!("flowlib.note_lock_only"));
        }
        p.files = Some(files);
        return Ok(p);
    }
    let changes = diff(Some(&old), &files);
    let modified = locked.is_none_or(|l| l.digest != pdig);
    if !force {
        if version_key(&gv) == version_key(&pv) {
            p.noop = Some(i18n::t!(
                "flowlib.update_same_version",
                name = name,
                version = gv
            ));
            return Ok(p);
        }
        if version_key(&gv) < version_key(&pv) {
            return Err(i18n::t!(
                "flowlib.update_older",
                name = name,
                global = gv,
                project = pv
            ));
        }
        if modified {
            return Err(i18n::t!(
                "flowlib.update_modified",
                name = name,
                files = list_short(&changes.all())
            ));
        }
    }
    p.changes = changes;
    p.backup = Some(ctx.project_backups());
    p.notes.extend(gate_note(&g));
    p.files = Some(files);
    Ok(p)
}

/// 列文件给一句话用：至多 8 个。
pub fn list_short(v: &[String]) -> String {
    const N: usize = 8;
    let mut s = v.iter().take(N).cloned().collect::<Vec<_>>().join(", ");
    if v.len() > N {
        s.push_str(&i18n::t!("flowlib.and_more", n = v.len() - N));
    }
    s
}

/// 这个项目里没结束的运行：（运行，流程名）。直接读运行账本，不经实例。
pub fn active_flows(project: &Path) -> Vec<(String, String)> {
    use harnix_flow::RunStore as _;
    let store = harnix_flow::FileStore::new(client::ProjectState::new(project).flow_runs());
    // 没有运行时：FileStore 在没有 Tokio 运行时的时候就地读，这个 future 不会挂起。
    futures::executor::block_on(store.list(&harnix_flow::RunQuery::default()))
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.status == harnix_flow::RunStatus::Active)
        .map(|r| (r.id, r.flow))
        .collect()
}

/// 这个项目里没结束、用着这个包的运行（流程名 `<包>/<流程>`，不在包里的流程包名就是流程名）。
pub fn active_runs(project: &Path, name: &str) -> Vec<String> {
    active_flows(project)
        .into_iter()
        .filter(|(_, f)| f.split('/').next() == Some(name))
        .map(|(id, _)| id)
        .collect()
}

/// `remove <包> [--global] [--force]`：都先备份。项目里有进行中的运行用它就拒绝，`--force` 才删。
pub fn plan_remove(ctx: &Ctx, name: &str, global: bool, force: bool) -> Result<Plan, String> {
    if global {
        let g = ctx
            .library
            .dir(name)
            .ok_or_else(|| i18n::t!("flowlib.no_global", name = name))?;
        let mut p = Plan::new(Action::Remove, Where::Global, name);
        p.changes.removed = package::collect(&g)?.into_iter().map(|(f, _)| f).collect();
        p.from = version_in(&g);
        p.backup = Some(ctx.library.backups());
        p.expect = digest_opt(Some(&g));
        let reg = ctx.library.registry()?;
        if ctx.library.builtin_state(name, &reg).is_some() {
            p.notes
                .push(i18n::t!("flowlib.note_remove_builtin", name = name));
        }
        return Ok(p);
    }
    let d = ctx
        .project_dir(name)
        .ok_or_else(|| i18n::t!("flowlib.not_in_project", name = name))?;
    let runs = active_runs(&ctx.project, name);
    if !runs.is_empty() && !force {
        return Err(i18n::t!(
            "flowlib.remove_runs",
            name = name,
            runs = list_short(&runs)
        ));
    }
    let mut p = Plan::new(Action::Remove, Where::Project, name);
    p.changes.removed = package::collect(&d)?.into_iter().map(|(f, _)| f).collect();
    p.from = version_in(&d);
    p.backup = Some(ctx.project_backups());
    p.expect = digest_opt(Some(&d));
    if !runs.is_empty() {
        p.notes.push(i18n::t!(
            "flowlib.note_remove_runs",
            runs = list_short(&runs)
        ));
    }
    Ok(p)
}

/// `backfill <包> [--force]`：项目 → 全局（flows.md §7.12 的表）。`local.md` 不回填。
pub fn plan_backfill(ctx: &Ctx, name: &str, force: bool) -> Result<Plan, String> {
    let pd = ctx
        .project_dir(name)
        .ok_or_else(|| i18n::t!("flowlib.not_in_project", name = name))?;
    let files = package::collect(&pd)?;
    let pv = version_in(&pd).unwrap_or_default();
    let g = ctx.library.dir(name);
    let mut p = Plan::new(Action::Backfill, Where::Global, name);
    p.to = Some(pv.clone());
    p.expect = digest_opt(g.as_deref());
    let reg = ctx.library.registry()?;
    let builtin = ctx.library.builtin_state(name, &reg).is_some();
    if let Some(g) = &g {
        let old = package::collect(g)?;
        let gv = version_in(g).unwrap_or_default();
        p.from = Some(gv.clone());
        if package::digest_of(&old) == package::digest_of(&files) {
            let lock = read_lock(&ctx.project)?;
            if lock
                .bundles
                .get(name)
                .is_some_and(|l| l.digest == package::digest_of(&files))
            {
                p.noop = Some(i18n::t!("flowlib.backfill_same", name = name));
            } else {
                p.lock_only = true;
                p.notes.push(i18n::t!("flowlib.note_lock_only"));
            }
            p.files = Some(files);
            return Ok(p);
        }
        if !force {
            if version_key(&pv) == version_key(&gv) {
                return Err(i18n::t!(
                    "flowlib.backfill_same_version",
                    name = name,
                    version = pv
                ));
            }
            if version_key(&pv) < version_key(&gv) {
                return Err(i18n::t!(
                    "flowlib.backfill_older",
                    name = name,
                    global = gv,
                    project = pv
                ));
            }
        }
        p.changes = diff(Some(&old), &files);
        p.backup = Some(ctx.library.backups());
    } else {
        p.changes = diff(None, &files);
    }
    if builtin {
        p.notes
            .push(i18n::t!("flowlib.note_backfill_builtin", name = name));
    }
    p.files = Some(files);
    Ok(p)
}

/// `reset <包>`：全局库里的内置包恢复成这个 Conix 带的那版，当前的先备份。不碰项目。
pub fn plan_reset(ctx: &Ctx, name: &str) -> Result<Plan, String> {
    let b = super::builtin::package(name)
        .ok_or_else(|| i18n::t!("flowlib.not_builtin", name = name))?;
    let g = ctx.library.dir(name);
    let mut p = Plan::new(Action::Reset, Where::Global, name);
    let old = files_of(g.as_deref())?;
    p.from = g.as_deref().and_then(version_in);
    p.to = Some(b.version.clone());
    p.changes = diff(old.as_ref(), &b.files);
    p.backup = g.as_ref().map(|_| ctx.library.backups());
    p.expect = digest_opt(g.as_deref());
    let reg = ctx.library.registry()?;
    if g.is_some()
        && p.changes.is_empty()
        && reg
            .bundles
            .get(name)
            .is_some_and(|e| !e.removed && e.shipped.as_ref().is_some_and(|s| s.digest == b.digest))
    {
        p.noop = Some(i18n::t!("flowlib.reset_same", name = name));
    }
    p.files = Some(b.files);
    Ok(p)
}

/// `import <文件> [--project] [--force]`：包文件 → 全局库（缺省）或项目。核对与校验同包文件的规矩。
pub fn plan_import(ctx: &Ctx, file: &Path, project: bool, force: bool) -> Result<Plan, String> {
    let stage_in = if project {
        ctx.project_state()
    } else {
        ctx.library.staging()
    };
    let v = package::verify(file, Some(&stage_in), package::default_catalog())?;
    let name = v.manifest.bundle.name.clone();
    let files = package::collect(&v.staged.dir)?;
    let (target, here) = if project {
        (ctx.project_dir(&name), Where::Project)
    } else {
        (ctx.library.dir(&name), Where::Global)
    };
    if target.is_some() && !force {
        return Err(if project {
            i18n::t!("flowlib.import_exists_project", name = name)
        } else {
            i18n::t!("flowlib.import_exists_global", name = name)
        });
    }
    let mut p = Plan::new(Action::Import, here, &name);
    let old = files_of(target.as_deref())?;
    p.changes = diff(old.as_ref(), &files);
    p.from = target.as_deref().and_then(version_in);
    p.to = Some(v.manifest.bundle.version.clone());
    p.backup = target.as_ref().map(|_| match here {
        Where::Project => ctx.project_backups(),
        Where::Global => ctx.library.backups(),
    });
    p.expect = digest_opt(target.as_deref());
    p.notes.push(i18n::t!(
        "flowlib.note_import_by",
        conix = v.manifest.conix.version,
        at = v.manifest.conix.exported_at
    ));
    p.notes.extend(gate_note(&v.staged.dir));
    p.requires = Some(v.manifest.requires.clone());
    p.files = Some(files);
    Ok(p)
}

fn children(dir: &Path, keep: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| e.file_name().to_str().is_some_and(&keep))
                .map(|e| e.path())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// `clean [--global]`：备份、暂存残留、孤儿条目（项目的 lock / 全局的 registry 里有、目录没了的）。
pub fn plan_clean(ctx: &Ctx, global: bool) -> Result<Plan, String> {
    let mut p;
    if global {
        p = Plan::new(Action::Clean, Where::Global, "");
        p.paths = children(&ctx.library.backups(), |_| true);
        p.paths.extend(children(&ctx.library.staging(), |_| true));
        let reg = ctx.library.registry()?;
        p.orphans = global_orphans(ctx, &reg);
    } else {
        p = Plan::new(Action::Clean, Where::Project, "");
        p.paths = children(&ctx.project_backups(), |_| true);
        // `flows-builtin/` 是以前内置包每次起实例时重写的那份，现在不读了。
        p.paths.extend(children(&ctx.project_state(), |n| {
            n.starts_with("flow-import-") || n == "flows-builtin"
        }));
        p.orphans = lock_orphans(ctx)?;
    }
    if p.paths.is_empty() && p.orphans.is_empty() {
        p.noop = Some(i18n::t!("flowlib.clean_nothing"));
    }
    Ok(p)
}

/// registry 里有、目录没了的非内置包（内置的没了起来时会重新解开，删过的留着那一条）。
pub fn global_orphans(ctx: &Ctx, reg: &library::Registry) -> Vec<String> {
    reg.bundles
        .iter()
        .filter(|(n, e)| {
            e.shipped.is_none()
                && super::builtin::package(n).is_none()
                && ctx.library.dir(n).is_none()
        })
        .map(|(n, _)| n.clone())
        .collect()
}

/// lock 里有、项目里没有这个包了。
pub fn lock_orphans(ctx: &Ctx) -> Result<Vec<String>, String> {
    Ok(read_lock(&ctx.project)?
        .bundles
        .keys()
        .filter(|n| ctx.project_dir(n).is_none())
        .cloned()
        .collect())
}

// ── 照计划做 ────────────────────────────────────────────────────────────────

/// 做完了：旧的备份到了哪。
#[derive(Debug, Clone, Default)]
pub struct Applied {
    pub backup: Option<PathBuf>,
}

fn recheck(plan: &Plan, now: Option<&Path>) -> Result<(), String> {
    if digest_opt(now) != plan.expect {
        return Err(i18n::t!("flowlib.changed_meanwhile", name = plan.bundle));
    }
    Ok(())
}

fn lock_entry(version: &str, digest: &str) -> LockEntry {
    LockEntry {
        from: "global".into(),
        version: version.to_string(),
        digest: digest.to_string(),
        at: library::now(),
    }
}

pub fn apply(ctx: &Ctx, plan: &Plan) -> Result<Applied, String> {
    if plan.is_noop() {
        return Ok(Applied::default());
    }
    let catalog = package::default_catalog;
    let name = plan.bundle.as_str();
    let files = plan.files.clone().unwrap_or_default();
    match (plan.action, plan.target) {
        (Action::Clean, here) => {
            let _l = match here {
                Where::Global => ctx.library.lock()?,
                Where::Project => library::lock_file(&ctx.project.join(PROJECT_LOCK))?,
            };
            for p in &plan.paths {
                let r = if p.is_dir() {
                    std::fs::remove_dir_all(p)
                } else {
                    std::fs::remove_file(p)
                };
                r.map_err(|e| format!("{}: {e}", p.display()))?;
            }
            match here {
                Where::Global => {
                    let mut reg = ctx.library.registry()?;
                    for o in &plan.orphans {
                        reg.bundles.remove(o);
                    }
                    ctx.library.save(&reg)?;
                }
                Where::Project if !plan.orphans.is_empty() => {
                    let mut lock = read_lock(&ctx.project)?;
                    for o in &plan.orphans {
                        lock.bundles.remove(o);
                    }
                    write_lock(&ctx.project, &lock)?;
                }
                Where::Project => {}
            }
            Ok(Applied::default())
        }
        (Action::Install | Action::Update | Action::Import, Where::Project) => {
            let _l = library::lock_file(&ctx.project.join(PROJECT_LOCK))?;
            let target = ctx.flows_dir().join(name);
            recheck(plan, ctx.project_dir(name).as_deref())?;
            let digest = package::digest_of(&files);
            let mut lock = read_lock(&ctx.project)?;
            let mut backup = None;
            let version;
            if plan.lock_only {
                version = plan.to.clone().unwrap_or_default();
            } else {
                let staged = package::stage(
                    name,
                    &files,
                    plan.requires.as_ref(),
                    Some(&ctx.project_state()),
                    catalog(),
                )?;
                version = staged.version();
                backup = package::place(staged, &target, &ctx.project_backups(), true)?;
            }
            if plan.action == Action::Import {
                // 从包文件直接装进项目的，不是从全局库来的：是"只在项目"的包。
                lock.bundles.remove(name);
            } else {
                lock.bundles
                    .insert(name.to_string(), lock_entry(&version, &digest));
            }
            write_lock(&ctx.project, &lock)?;
            Ok(Applied { backup })
        }
        (Action::Remove, Where::Project) => {
            let _l = library::lock_file(&ctx.project.join(PROJECT_LOCK))?;
            recheck(plan, ctx.project_dir(name).as_deref())?;
            let backup =
                package::back_up(&ctx.flows_dir().join(name), &ctx.project_backups(), name)?;
            let mut lock = read_lock(&ctx.project)?;
            if lock.bundles.remove(name).is_some() {
                write_lock(&ctx.project, &lock)?;
            }
            Ok(Applied { backup })
        }
        (action, Where::Global) => {
            let _l = ctx.library.lock()?;
            recheck(plan, ctx.library.dir(name).as_deref())?;
            let mut reg = ctx.library.registry()?;
            let backup = match action {
                Action::Remove => ctx.library.remove(&mut reg, name)?,
                Action::Reset => ctx.library.reset(&mut reg, name)?,
                Action::Import => {
                    let staged = package::stage(
                        name,
                        &files,
                        plan.requires.as_ref(),
                        Some(&ctx.library.staging()),
                        catalog(),
                    )?;
                    drop(staged);
                    ctx.library
                        .write(&mut reg, name, &files, Source::Import, None, catalog())?
                }
                Action::Backfill => {
                    let backup = if plan.lock_only {
                        None
                    } else {
                        ctx.library.write(
                            &mut reg,
                            name,
                            &files,
                            Source::Backfill,
                            None,
                            catalog(),
                        )?
                    };
                    // 回填的就是项目里这一份：lock 跟上，项目对全局不再算改过。
                    let _p = library::lock_file(&ctx.project.join(PROJECT_LOCK))?;
                    let mut lock = read_lock(&ctx.project)?;
                    lock.bundles.insert(
                        name.to_string(),
                        lock_entry(
                            plan.to.as_deref().unwrap_or_default(),
                            &package::digest_of(&files),
                        ),
                    );
                    write_lock(&ctx.project, &lock)?;
                    backup
                }
                _ => {
                    return Err(format!(
                        "{} is not done in the global library",
                        action.as_str()
                    ));
                }
            };
            Ok(Applied { backup })
        }
        (action, Where::Project) => Err(format!("{} is not done in a project", action.as_str())),
    }
}
