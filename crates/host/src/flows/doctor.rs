//! `conix flow doctor`（docs/design/flows.md §7.12、§7.13）：全局库与一个项目的流程包体检。通用的检查在这里
//! （目录与记录、校验、版本、lock、信任、`.gitignore`、归档）；包自己要项目备什么由包的 `doctor:` 脚本答
//! （[`harnix_flow::doctor`]）。只读，`--fix` 只做 [`Fix`] 里那几样安全的。

use std::path::{Path, PathBuf};

use harnix_flow::bundle;

use super::library::{BuiltinState, version_key};
use super::manage::{self, Ctx, Where};
use super::package;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub scope: Where,
    pub bundle: Option<String>,
    pub text: String,
    /// 该跑的命令或该做的事。
    pub hint: Option<String>,
}

/// `--fix` 会做的：都不丢东西。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fix {
    /// 删掉暂存的残留。
    Staging(PathBuf),
    /// registry 里目录没了的非内置包。
    RegistryOrphan(String),
    /// lock 里目录没了的包。
    LockOrphan(String),
    /// 内容与全局库里一样、lock 里却没记的：补记。
    Lock(String),
    /// 缺了（没删过）的内置包：重新解开。
    Extract(String),
    /// 进行中的运行用的包项目里没了、全局库里有：装上。
    Install(String),
}

#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
    pub fixes: Vec<Fix>,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.level == Level::Error)
            .count()
    }

    fn add(
        &mut self,
        level: Level,
        scope: Where,
        bundle: Option<&str>,
        text: String,
        hint: Option<String>,
    ) {
        self.findings.push(Finding {
            level,
            scope,
            bundle: bundle.map(String::from),
            text,
            hint,
        });
    }
}

/// 包目录的校验与兼容：每个问题一句。
fn problems_of(dir: &Path) -> Vec<String> {
    let mut out = vec![];
    if let Err(e) = package::compatible(dir) {
        out.push(e);
    }
    if let Err(e) = package::collect(dir) {
        out.push(e);
    }
    out.extend(package::validate_bundle(dir, package::default_catalog()));
    out
}

fn dir_size(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.file_type() {
                    Ok(t) if t.is_dir() => dir_size(&e.path()),
                    Ok(_) => e.metadata().map(|m| m.len()).unwrap_or(0),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}

/// 全局库：目录与 registry、每个包的校验与要求、内置包的状态、暂存残留、备份。
pub fn global(ctx: &Ctx) -> Report {
    let mut r = Report::default();
    let g = Where::Global;
    let lib = &ctx.library;
    let reg = match lib.registry() {
        Ok(reg) => reg,
        Err(e) => {
            r.add(Level::Error, g, None, e, None);
            return r;
        }
    };
    let names = lib.names();
    for n in &names {
        let dir = lib.path_of(n);
        let problems = problems_of(&dir);
        for p in &problems {
            r.add(
                Level::Error,
                g,
                Some(n),
                i18n::t!("flowlib.dr_invalid", problem = p),
                None,
            );
        }
        match reg.bundles.get(n) {
            None => r.add(
                Level::Warn,
                g,
                Some(n),
                i18n::t!("flowlib.dr_unrecorded"),
                Some(i18n::t!("flowlib.dr_hint_reimport", name = n)),
            ),
            Some(e) if problems.is_empty() => {
                let actual = package::dir_digest(&dir).unwrap_or_default();
                if e.shipped.is_none() && actual != e.digest {
                    r.add(
                        Level::Info,
                        g,
                        Some(n),
                        i18n::t!("flowlib.dr_hand_edited"),
                        None,
                    );
                }
            }
            Some(_) => {}
        }
    }
    for (n, e) in &reg.bundles {
        match lib.builtin_state(n, &reg) {
            Some(BuiltinState::Pristine) => {
                r.add(
                    Level::Info,
                    g,
                    Some(n),
                    i18n::t!("flowlib.bi_pristine"),
                    None,
                );
            }
            Some(BuiltinState::Modified) => {
                let shipped = super::builtin::package(n);
                let newer = shipped
                    .as_ref()
                    .zip(e.shipped.as_ref())
                    .filter(|(b, base)| version_key(&b.version) > version_key(&base.version));
                match newer {
                    Some((b, _)) => r.add(
                        Level::Warn,
                        g,
                        Some(n),
                        i18n::t!("flowlib.dr_newer_builtin", version = b.version),
                        Some(format!("conix flow reset {n}")),
                    ),
                    None => r.add(
                        Level::Info,
                        g,
                        Some(n),
                        i18n::t!("flowlib.bi_modified"),
                        None,
                    ),
                }
            }
            Some(BuiltinState::Removed) => {
                r.add(
                    Level::Info,
                    g,
                    Some(n),
                    i18n::t!("flowlib.bi_removed"),
                    None,
                );
            }
            Some(BuiltinState::Missing) => {
                r.add(
                    Level::Warn,
                    g,
                    Some(n),
                    i18n::t!("flowlib.bi_missing"),
                    Some(format!("conix flow reset {n}")),
                );
                r.fixes.push(Fix::Extract(n.clone()));
            }
            None if lib.dir(n).is_none() => {
                r.add(
                    Level::Warn,
                    g,
                    Some(n),
                    i18n::t!("flowlib.dr_orphan_entry"),
                    Some("conix flow clean --global".into()),
                );
                r.fixes.push(Fix::RegistryOrphan(n.clone()));
            }
            None => {}
        }
    }
    // 这个 Conix 带、registry 里还没有的内置包：起来时就会解开，这里只在没解开时报。
    for b in super::builtin::bundles() {
        if !reg.bundles.contains_key(b) && lib.dir(b).is_none() {
            r.add(
                Level::Warn,
                g,
                Some(b),
                i18n::t!("flowlib.bi_missing"),
                None,
            );
            r.fixes.push(Fix::Extract(b.to_string()));
        }
    }
    for s in entries(&lib.staging()) {
        r.add(
            Level::Warn,
            g,
            None,
            i18n::t!("flowlib.dr_staging", path = s.display()),
            Some("conix flow doctor --global --fix".into()),
        );
        r.fixes.push(Fix::Staging(s));
    }
    let backups = entries(&lib.backups());
    if !backups.is_empty() {
        r.add(
            Level::Info,
            g,
            None,
            i18n::t!(
                "flowlib.dr_backups",
                n = backups.len(),
                kib = dir_size(&lib.backups()) / 1024
            ),
            Some("conix flow clean --global".into()),
        );
    }
    r
}

/// 项目体检要的、命令行那边知道的东西。
#[derive(Debug, Clone, Default)]
pub struct ProjectOpts {
    /// 制品目录名（缺省 `conix`）。
    pub artifacts_dir: String,
    /// 装着的插件的流程目录（按插件名）：查"遮住了插件的同名包"。
    pub plugin_flow_dirs: Vec<(String, PathBuf)>,
    /// 项目受不受信；`None` 是看不出来。
    pub trusted: Option<bool>,
}

/// 项目：每个包的校验、lock、改过、有新版、只在项目、遮住插件、保留名、进行中运行的包、信任、`.gitignore`、
/// 包自己的体检、归档。
pub async fn project(ctx: &Ctx, opts: &ProjectOpts) -> Report {
    let mut r = Report::default();
    let p = Where::Project;
    let rows = match manage::rows(ctx) {
        Ok(rows) => rows,
        Err(e) => {
            r.add(Level::Error, p, None, e, None);
            return r;
        }
    };
    let mut gates = 0;
    for row in &rows {
        let n = row.name.as_str();
        let Some(side) = &row.project else {
            if row.lock.is_some() {
                r.add(
                    Level::Warn,
                    p,
                    Some(n),
                    i18n::t!("flowlib.st_gone"),
                    Some("conix flow clean".into()),
                );
                r.fixes.push(Fix::LockOrphan(n.to_string()));
            }
            continue;
        };
        for prob in problems_of(&side.dir) {
            r.add(
                Level::Error,
                p,
                Some(n),
                i18n::t!("flowlib.dr_invalid", problem = prob),
                None,
            );
        }
        gates += package::command_gates(&side.dir);
        if row.modified() {
            r.add(
                Level::Info,
                p,
                Some(n),
                i18n::t!("flowlib.st_modified"),
                Some(i18n::t!("flowlib.dr_hint_modified", name = n)),
            );
        }
        if let Some(v) = row.update() {
            r.add(
                Level::Info,
                p,
                Some(n),
                i18n::t!("flowlib.st_update", version = v),
                Some(format!("conix flow update {n}")),
            );
        }
        if row.lock.is_none() {
            if row.global.as_ref().is_some_and(|g| g.digest == side.digest) {
                r.add(
                    Level::Info,
                    p,
                    Some(n),
                    i18n::t!("flowlib.dr_same_unlocked"),
                    None,
                );
                r.fixes.push(Fix::Lock(n.to_string()));
            } else {
                r.add(
                    Level::Info,
                    p,
                    Some(n),
                    i18n::t!("flowlib.st_project_only"),
                    Some(format!("conix flow backfill {n}")),
                );
            }
        }
        for (plugin, dir) in &opts.plugin_flow_dirs {
            if bundle::bundle_in(dir, n).is_some() {
                r.add(
                    Level::Warn,
                    p,
                    Some(n),
                    i18n::t!("flowlib.dr_shadows", plugin = plugin),
                    None,
                );
            }
        }
        // 包自己的体检（bundle.yaml 的 `doctor:`）。
        for f in bundle_doctor(ctx, opts, n).await {
            r.findings.push(f);
        }
    }
    for reserved in bundle::RESERVED_BUNDLES {
        if ctx
            .flows_dir()
            .join(reserved)
            .join(bundle::BUNDLE_FILE)
            .is_file()
        {
            r.add(
                Level::Error,
                p,
                Some(reserved),
                i18n::t!("flowlib.dr_reserved", name = reserved),
                None,
            );
        }
    }
    // 进行中的运行用的包不在项目里了：skill 与模板没钉在运行里，跑下去会断。
    let mut gone: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (id, flow) in manage::active_flows(&ctx.project) {
        let b = flow.split('/').next().unwrap_or(&flow).to_string();
        let flat = ctx.flows_dir().join(format!("{b}.yaml")).is_file();
        let plugin = opts.plugin_flow_dirs.iter().any(|(_, d)| {
            bundle::bundle_in(d, &b).is_some() || d.join(format!("{b}.yaml")).is_file()
        });
        if ctx.project_dir(&b).is_none() && !flat && !plugin {
            gone.entry(b).or_default().push(id);
        }
    }
    for (b, runs) in gone {
        let in_lib = ctx.library.dir(&b).is_some();
        r.add(
            Level::Error,
            p,
            Some(&b),
            i18n::t!("flowlib.dr_runs_gone", runs = manage::list_short(&runs)),
            Some(format!("conix flow install {b}")),
        );
        if in_lib {
            r.fixes.push(Fix::Install(b));
        }
    }
    if gates > 0 && opts.trusted == Some(false) {
        r.add(
            Level::Warn,
            p,
            None,
            i18n::t!("flowlib.dr_untrusted", n = gates),
            Some(i18n::t!("flowlib.dr_hint_trust")),
        );
    }
    // 制品目录与 `.wkts/` 第一次用时 Conix 与引擎各自写好忽略规则；还没有的不报。
    for path in [
        format!("{}/runs/", opts.artifacts_dir),
        ".wkts/".to_string(),
    ] {
        let top = path.split('/').next().unwrap_or_default();
        if ctx.project.join(top).is_dir() && ignored(&ctx.project, &path) == Some(false) {
            r.add(
                Level::Warn,
                p,
                None,
                i18n::t!("flowlib.dr_not_ignored", path = path),
                None,
            );
        }
    }
    for s in std::fs::read_dir(ctx.project_state())
        .map(|rd| rd.flatten().map(|e| e.path()).collect::<Vec<_>>())
        .unwrap_or_default()
        .into_iter()
        .filter(|s| {
            s.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("flow-import-"))
        })
    {
        r.add(
            Level::Warn,
            p,
            None,
            i18n::t!("flowlib.dr_staging", path = s.display()),
            Some("conix flow doctor --fix".into()),
        );
        r.fixes.push(Fix::Staging(s));
    }
    let archives = archives(&ctx.project, &opts.artifacts_dir);
    if !archives.is_empty() {
        let runs = client::ProjectState::new(&ctx.project).flow_runs();
        let bad: Vec<String> = archives
            .iter()
            .filter(|a| !package::verify_archive(&runs, a).ok)
            .map(|a| {
                a.strip_prefix(&ctx.project)
                    .unwrap_or(a)
                    .display()
                    .to_string()
            })
            .collect();
        if bad.is_empty() {
            r.add(
                Level::Info,
                p,
                None,
                i18n::t!("flowlib.dr_archives_ok", n = archives.len()),
                None,
            );
        } else {
            r.add(
                Level::Error,
                p,
                None,
                i18n::t!(
                    "flowlib.dr_archives_bad",
                    n = bad.len(),
                    total = archives.len(),
                    list = manage::list_short(&bad)
                ),
                Some("conix flow verify --all".into()),
            );
        }
    }
    r
}

/// 一个包自己的体检：流程库按项目的包（`DirPacks` 读 `.conix/flows/`）与布局跑 `bundle.yaml` 的 `doctor:` 脚本。
async fn bundle_doctor(ctx: &Ctx, opts: &ProjectOpts, name: &str) -> Vec<Finding> {
    let packs =
        harnix_flow::DirPacks::new([ctx.flows_dir()]).with_catalog(package::default_catalog());
    let layout = super::project_layout(&ctx.project, Some(&opts.artifacts_dir));
    let report = match harnix_flow::doctor(&packs, Some(&layout), name, &ctx.project).await {
        Ok(r) => r,
        Err(e) => {
            return vec![Finding {
                level: Level::Error,
                scope: Where::Project,
                bundle: Some(name.to_string()),
                text: e,
                hint: None,
            }];
        }
    };
    report
        .findings
        .into_iter()
        .map(|f| Finding {
            level: match f.level {
                harnix_flow::DoctorLevel::Info => Level::Info,
                harnix_flow::DoctorLevel::Warn => Level::Warn,
                harnix_flow::DoctorLevel::Error => Level::Error,
            },
            scope: Where::Project,
            bundle: Some(name.to_string()),
            text: match f.path {
                Some(p) => format!("{} ({p})", f.message),
                None => f.message,
            },
            hint: f.fix,
        })
        .collect()
}

/// `git check-ignore`：项目不在 git 里、git 不在时是 `None`。
fn ignored(project: &Path, rel: &str) -> Option<bool> {
    let probe = format!("{rel}conix-doctor-probe");
    let out = std::process::Command::new("git")
        .args(["check-ignore", "-q", "--no-index", &probe])
        .current_dir(project)
        .output()
        .ok()?;
    match out.status.code() {
        Some(0) => Some(true),
        Some(1) => Some(false),
        _ => None,
    }
}

/// 项目的归档：`<制品目录>/*/archive/*/` 里有 `record.json` 的。
pub fn archives(project: &Path, artifacts_dir: &str) -> Vec<PathBuf> {
    let mut out = vec![];
    for b in entries(&project.join(artifacts_dir)) {
        for a in entries(&b.join("archive")) {
            if a.join(harnix_flow::archive::RECORD).is_file() {
                out.push(a);
            }
        }
    }
    out
}

/// 照 `fixes` 做；回每一件做了什么（或为什么没做成）。
pub fn fix(ctx: &Ctx, fixes: &[Fix]) -> Vec<Result<String, String>> {
    let mut out = vec![];
    for f in fixes {
        let r = match f {
            Fix::Staging(p) => std::fs::remove_dir_all(p)
                .or_else(|_| std::fs::remove_file(p))
                .map(|_| i18n::t!("flowlib.fixed_staging", path = p.display()))
                .map_err(|e| format!("{}: {e}", p.display())),
            Fix::RegistryOrphan(n) => (|| {
                let _l = ctx.library.lock()?;
                let mut reg = ctx.library.registry()?;
                reg.bundles.remove(n);
                ctx.library.save(&reg)?;
                Ok(i18n::t!("flowlib.fixed_orphan", name = n))
            })(),
            Fix::LockOrphan(n) => (|| {
                let mut lock = manage::read_lock(&ctx.project)?;
                lock.bundles.remove(n);
                manage::write_lock(&ctx.project, &lock)?;
                Ok(i18n::t!("flowlib.fixed_orphan", name = n))
            })(),
            Fix::Lock(n) => manage::plan_update(ctx, n, false)
                .and_then(|p| manage::apply(ctx, &p))
                .map(|_| i18n::t!("flowlib.fixed_lock", name = n)),
            Fix::Extract(n) => manage::plan_reset(ctx, n)
                .and_then(|p| manage::apply(ctx, &p))
                .map(|_| i18n::t!("flowlib.fixed_extract", name = n)),
            Fix::Install(n) => manage::plan_install(ctx, n, false)
                .and_then(|p| manage::apply(ctx, &p))
                .map(|_| i18n::t!("flowlib.fixed_install", name = n)),
        };
        out.push(r);
    }
    out
}
