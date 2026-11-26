//! 流程包的导出与导入（docs/design/flows.md §7.12）：一个流程包（目录）打成 `<包>-<版本>.zip`，带一份生成的
//! `MANIFEST.yaml`（导出它的 Conix 版本、要求、每个文件的 SHA-256）；导入时先在暂存目录里解开、逐项核对、
//! 用流程库校验，都过了才装进项目的 `.conix/flows/`。命令行（不需要实例）与实例共用这里。
//!
//! 包是别人给的：解包前不信任里面的任何路径与大小，所以不用 `extract`，逐个条目自己读、自己写。

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};

pub use harnix_flow::archive;
use harnix_flow::bundle;
use harnix_flow::{
    Catalog, DEFINITION_FORMAT, DirPacks, FLOW_API_VERSION, MemPacks, Packs, PartKind,
    part_problems, validate,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use port::{FlowExported, FlowImported, FlowPackageInfo};

/// `MANIFEST.yaml` 的格式。
pub const FORMAT: u32 = 1;
pub const MANIFEST: &str = "MANIFEST.yaml";
pub const MAX_FILES: usize = 512;
pub const MAX_TOTAL: u64 = 10 * 1024 * 1024;
pub const MAX_FILE: u64 = 2 * 1024 * 1024;
/// 项目里流程包放的地方（相对项目根）。
pub const PROJECT_FLOWS: &str = ".conix/flows";
/// 覆盖时旧的那份移到这里（相对项目根）。
pub const BACKUPS: &str = ".conix/state/conix/flow-backups";
/// 暂存目录的父目录（相对项目根）：与 `.conix/flows/` 同一个文件系统，最后一步才是 rename。
const STATE: &str = ".conix/state/conix";

/// 正在运行的 Conix 的完整版本（`x.y.z-YYYYMMDDNN`）。
pub const BUILD: &str = env!("CARGO_PKG_VERSION");

/// 界面上显示的 `x.y.z`。
pub fn conix_version() -> &'static str {
    BUILD.split('-').next().unwrap_or(BUILD)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub bundle: ManifestBundle,
    pub conix: ManifestConix,
    pub requires: ManifestRequires,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestBundle {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestConix {
    pub version: String,
    pub build: String,
    pub exported_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestRequires {
    pub conix: String,
    pub flow_definition_format: u32,
    pub flow_api: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: String,
    pub size: u64,
}

fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// 校验流程包时流程库要知道的名字（场景、命令门禁）：与实例里一样；实例自己会换成带插件门禁的那份。
pub fn default_catalog() -> Catalog {
    MemPacks::new()
        .scene(super::SCENE)
        .main_scene(super::SCENE)
        .submit_scene(super::SCENE)
        .command_gates()
        .catalog()
}

// ── 来源 ────────────────────────────────────────────────────────────────────

/// 流程包从哪来：与流程库读的顺序一致（项目 > 插件 > 内置）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Project,
    Plugin(String),
    Builtin,
}

/// 找一个流程包的目录。`plugin_dirs` 是启用着的插件的流程目录（按插件名）；没有实例时给空。
pub fn locate(
    project: &Path,
    plugin_dirs: &[(String, PathBuf)],
    builtin_dir: Option<&Path>,
    name: &str,
) -> Option<(PathBuf, Source)> {
    if let Some(d) = bundle::bundle_in(&project.join(PROJECT_FLOWS), name) {
        return Some((d, Source::Project));
    }
    let mut plugins: Vec<&(String, PathBuf)> = plugin_dirs.iter().collect();
    plugins.sort();
    if let Some((p, d)) = plugins
        .into_iter()
        .find_map(|(p, d)| bundle::bundle_in(d, name).map(|d| (p, d)))
    {
        return Some((d, Source::Plugin(p.clone())));
    }
    builtin_dir
        .and_then(|b| bundle::bundle_in(b, name))
        .map(|d| (d, Source::Builtin))
}

// ── 导出 ────────────────────────────────────────────────────────────────────

/// 包里放什么：除了生成的 MANIFEST、点开头的、`__pycache__`、用户自己的 `skills/*/local.md`。
fn excluded(rel: &[&str]) -> bool {
    rel.iter()
        .any(|c| c.starts_with('.') || *c == "__pycache__")
        || rel == [MANIFEST]
        || (rel.len() == 3 && rel[0] == PartKind::Skill.dir() && rel[2] == bundle::LOCAL_SKILL)
}

fn collect(dir: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
        let entries = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        for e in entries.flatten() {
            let p = e.path();
            let rel = p.strip_prefix(base).map_err(|e| e.to_string())?;
            let parts: Vec<String> = rel
                .components()
                .map(|c| c.as_os_str().to_str().map(String::from))
                .collect::<Option<_>>()
                .ok_or_else(|| format!("{}: the name is not UTF-8", p.display()))?;
            let parts_ref: Vec<&str> = parts.iter().map(String::as_str).collect();
            if excluded(&parts_ref) {
                continue;
            }
            let ft = e.file_type().map_err(|e| format!("{}: {e}", p.display()))?;
            if ft.is_symlink() {
                return Err(format!(
                    "{}: a symlink cannot go in a package",
                    rel.display()
                ));
            }
            if ft.is_dir() {
                walk(base, &p, out)?;
            } else if ft.is_file() {
                out.push((parts.join("/"), p));
            }
        }
        Ok(())
    }
    let mut paths = vec![];
    walk(dir, dir, &mut paths)?;
    paths.sort();
    if paths.len() > MAX_FILES {
        return Err(format!(
            "{} files: a package holds at most {MAX_FILES}",
            paths.len()
        ));
    }
    let mut total = 0u64;
    let mut out = vec![];
    for (rel, p) in paths {
        let bytes = std::fs::read(&p).map_err(|e| format!("{rel}: {e}"))?;
        if bytes.len() as u64 > MAX_FILE {
            return Err(format!(
                "{rel}: {} bytes, a package file is at most {MAX_FILE}",
                bytes.len()
            ));
        }
        total += bytes.len() as u64;
        if total > MAX_TOTAL {
            return Err(format!("a package is at most {MAX_TOTAL} bytes"));
        }
        out.push((rel, bytes));
    }
    Ok(out)
}

/// 包文件的缺省名字。
pub fn default_file_name(dir: &Path) -> Result<String, String> {
    let m = bundle::meta(dir)?;
    Ok(format!("{}-{}.zip", m.name, version_of(&m)))
}

fn version_of(m: &bundle::BundleMeta) -> String {
    if m.version.is_empty() {
        "0.0.0".into()
    } else {
        m.version.clone()
    }
}

/// 把流程包目录导出成 zip。`out` 省略 = 在 `default_dir` 里用缺省名字；是个存在的目录 = 在那里用缺省名字。
pub fn export(dir: &Path, out: Option<&Path>, default_dir: &Path) -> Result<FlowExported, String> {
    let meta = bundle::meta(dir)?;
    let files = collect(dir)?;
    if !files.iter().any(|(p, _)| p == bundle::BUNDLE_FILE) {
        return Err(format!("{}: no {}", dir.display(), bundle::BUNDLE_FILE));
    }
    let requires = meta
        .requires
        .as_ref()
        .and_then(|r| r.conix.clone())
        .unwrap_or_else(|| format!(">={}", conix_version()));
    let manifest = Manifest {
        format: FORMAT,
        bundle: ManifestBundle {
            name: meta.name.clone(),
            version: version_of(&meta),
            title: meta.title.clone(),
            description: meta.description.clone(),
        },
        conix: ManifestConix {
            version: conix_version().into(),
            build: BUILD.into(),
            exported_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        },
        requires: ManifestRequires {
            conix: requires,
            flow_definition_format: DEFINITION_FORMAT,
            flow_api: FLOW_API_VERSION,
        },
        files: files
            .iter()
            .map(|(p, b)| ManifestFile {
                path: p.clone(),
                sha256: sha256(b),
                size: b.len() as u64,
            })
            .collect(),
    };
    let text = serde_yaml::to_string(&manifest).map_err(|e| e.to_string())?;
    let mut all = vec![(MANIFEST.to_string(), text.into_bytes())];
    all.extend(files);
    let bytes = write_zip(&all)?;
    let name = format!("{}-{}.zip", manifest.bundle.name, manifest.bundle.version);
    let target = match out {
        Some(o) if o.is_dir() => o.join(&name),
        Some(o) => o.to_path_buf(),
        None => default_dir.join(&name),
    };
    std::fs::write(&target, bytes).map_err(|e| format!("{}: {e}", target.display()))?;
    Ok(FlowExported {
        file: target.to_string_lossy().into_owned(),
        package: info(&manifest, dir),
    })
}

pub(crate) fn write_zip(entries: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    // 固定时间、固定权限：同样的内容打出同样的字节（MANIFEST 里的导出时刻除外）。
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default())
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        zip.start_file(name.as_str(), opts)
            .map_err(|e| e.to_string())?;
        zip.write_all(bytes).map_err(|e| e.to_string())?;
    }
    Ok(zip.finish().map_err(|e| e.to_string())?.into_inner())
}

/// 给人看的概要：流程与命令按解开后的目录读，不信 MANIFEST 说的。
fn info(m: &Manifest, dir: &Path) -> FlowPackageInfo {
    let meta = bundle::meta(dir).ok();
    let flows = bundle::flow_names(&m.bundle.name, dir)
        .into_iter()
        .collect();
    let command_gates = std::fs::read_dir(dir.join(PartKind::Gate.dir()))
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                .filter_map(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
                .filter(|v| v["kind"].as_str() == Some("command"))
                .count() as u32
        })
        .unwrap_or(0);
    FlowPackageInfo {
        name: m.bundle.name.clone(),
        version: m.bundle.version.clone(),
        title: m.bundle.title.clone(),
        description: m.bundle.description.clone(),
        conix_version: m.conix.version.clone(),
        conix_build: m.conix.build.clone(),
        exported_at: m.conix.exported_at.clone(),
        requires_conix: m.requires.conix.clone(),
        files: m.files.len() as u32,
        flows,
        commands: meta
            .map(|m| m.commands.names().map(String::from).collect())
            .unwrap_or_default(),
        command_gates,
    }
}

// ── 读与核对 ─────────────────────────────────────────────────────────────────

/// 包里一个相对路径能不能要：不是绝对路径、没有 `..`、没有空的或 `.` 的一段、不含反斜杠与盘符、不是隐藏文件。
fn safe_rel(path: &str) -> Result<(), String> {
    let bad = path.is_empty()
        || path.starts_with('/')
        || path.contains(['\\', '\0'])
        || path.as_bytes().get(1) == Some(&b':')
        || path
            .split('/')
            .any(|c| c.is_empty() || c == "." || c == "..");
    if bad {
        return Err(format!(
            "path {path:?} is not a relative path inside the package"
        ));
    }
    let parts: Vec<&str> = path.split('/').collect();
    if path == MANIFEST {
        return Err(format!(
            "{MANIFEST} is made by the export, not one of its files"
        ));
    }
    if excluded(&parts) {
        return Err(format!(
            "{path}: a package does not hold hidden files, __pycache__ or a skill's local.md"
        ));
    }
    Ok(())
}

/// 条目名的检查：目录条目只查名字。
fn entry_name_ok(name: &str) -> Result<(), String> {
    let n = name.strip_suffix('/').unwrap_or(name);
    if n.is_empty()
        || n.starts_with('/')
        || n.contains(['\\', '\0'])
        || n.as_bytes().get(1) == Some(&b':')
        || n.split('/').any(|c| c.is_empty() || c == "." || c == "..")
    {
        return Err(format!(
            "entry {name:?} is not a relative path inside the package"
        ));
    }
    Ok(())
}

pub struct Package {
    pub manifest: Manifest,
    pub files: BTreeMap<String, Vec<u8>>,
}

/// `MANIFEST.yaml` 的原文（能读出来就给，核对不核对都行）。
pub fn manifest_text(file: &Path) -> Result<String, String> {
    let f = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a zip file: {e}"))?;
    let mut e = z
        .by_name(MANIFEST)
        .map_err(|_| format!("no {MANIFEST} in the package"))?;
    let mut buf = vec![];
    e.by_ref()
        .take(MAX_FILE + 1)
        .read_to_end(&mut buf)
        .map_err(|e| e.to_string())?;
    if buf.len() as u64 > MAX_FILE {
        return Err(format!("{MANIFEST} is too big"));
    }
    String::from_utf8(buf).map_err(|_| format!("{MANIFEST} is not UTF-8"))
}

/// 读出包并核对：格式、路径、数量与大小、符号链接、文件与 MANIFEST 一一对应、SHA-256。
pub fn read_package(file: &Path) -> Result<Package, String> {
    let len = std::fs::metadata(file)
        .map_err(|e| format!("{}: {e}", file.display()))?
        .len();
    if len > MAX_TOTAL + 1024 * 1024 {
        return Err(format!("the package file is {len} bytes, too big"));
    }
    let f = std::fs::File::open(file).map_err(|e| format!("{}: {e}", file.display()))?;
    let mut z = zip::ZipArchive::new(f).map_err(|e| format!("not a zip file: {e}"))?;
    if z.len() > MAX_FILES + 1 {
        return Err(format!(
            "{} entries: a package holds at most {MAX_FILES} files",
            z.len()
        ));
    }
    let mut got: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..z.len() {
        let mut e = z.by_index(i).map_err(|e| format!("entry {i}: {e}"))?;
        let name = e.name().to_string();
        entry_name_ok(&name)?;
        if e.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(format!("{name}: a symlink in a package"));
        }
        if e.is_dir() {
            continue;
        }
        if e.size() > MAX_FILE {
            return Err(format!(
                "{name}: {} bytes, a package file is at most {MAX_FILE}",
                e.size()
            ));
        }
        // 头里写的大小不可信：读的时候也限。
        let mut buf = vec![];
        e.by_ref()
            .take(MAX_FILE + 1)
            .read_to_end(&mut buf)
            .map_err(|e| format!("{name}: {e}"))?;
        if buf.len() as u64 > MAX_FILE {
            return Err(format!(
                "{name}: bigger than a package file may be ({MAX_FILE} bytes)"
            ));
        }
        total += buf.len() as u64;
        if total > MAX_TOTAL {
            return Err(format!(
                "the package is bigger than {MAX_TOTAL} bytes unpacked"
            ));
        }
        if got.insert(name.clone(), buf).is_some() {
            return Err(format!("{name}: the package has it twice"));
        }
    }
    let text = got
        .remove(MANIFEST)
        .ok_or_else(|| format!("no {MANIFEST} in the package"))?;
    let text = String::from_utf8(text).map_err(|_| format!("{MANIFEST} is not UTF-8"))?;
    let probe: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    match probe["format"].as_u64() {
        Some(f) if f == u64::from(FORMAT) => {}
        other => {
            return Err(format!(
                "{MANIFEST}: format {} is not one this Conix reads (it reads {FORMAT})",
                other.map_or("missing".to_string(), |f| f.to_string())
            ));
        }
    }
    let manifest: Manifest = serde_yaml::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let mut listed = BTreeSet::new();
    for f in &manifest.files {
        safe_rel(&f.path)?;
        if !listed.insert(f.path.as_str()) {
            return Err(format!("{MANIFEST}: {} is listed twice", f.path));
        }
    }
    for p in &listed {
        if !got.contains_key(*p) {
            return Err(format!("{p} is in {MANIFEST} but not in the package"));
        }
    }
    for p in got.keys() {
        if !listed.contains(p.as_str()) {
            return Err(format!("{p} is in the package but not in {MANIFEST}"));
        }
    }
    for f in &manifest.files {
        let bytes = &got[&f.path];
        if bytes.len() as u64 != f.size || sha256(bytes) != f.sha256.to_lowercase() {
            return Err(format!(
                "{}: does not match its sha256 in {MANIFEST} (changed after the export)",
                f.path
            ));
        }
    }
    Ok(Package {
        manifest,
        files: got,
    })
}

/// 这个 Conix 与流程库能不能跑它。
fn check_compatible(m: &Manifest, meta: &bundle::BundleMeta) -> Result<(), String> {
    let running = bundle::parse_version(BUILD).ok_or("this build has no version")?;
    let declared = meta.requires.as_ref().and_then(|r| r.conix.as_deref());
    for req in std::iter::once(m.requires.conix.as_str()).chain(declared) {
        let need = bundle::parse_requirement(req).map_err(|e| format!("requires.conix: {e}"))?;
        if running < need {
            return Err(format!(
                "the package needs Conix {}.{}.{} or newer; this is {}",
                need.0,
                need.1,
                need.2,
                conix_version()
            ));
        }
    }
    if m.requires.flow_definition_format > DEFINITION_FORMAT {
        return Err(format!(
            "the package's flow definitions are format {}, this Conix reads up to {DEFINITION_FORMAT}",
            m.requires.flow_definition_format
        ));
    }
    if m.requires.flow_api > FLOW_API_VERSION {
        return Err(format!(
            "the package needs flow API {}, this Conix has {FLOW_API_VERSION}",
            m.requires.flow_api
        ));
    }
    Ok(())
}

/// 用流程库校验解开的流程包目录（`<父>/<包名>`）：每个流程、每个件、`commands` 里列的命令文件。
pub fn validate_bundle(dir: &Path, catalog: Catalog) -> Vec<String> {
    let Some(name) = dir.file_name().and_then(|n| n.to_str()) else {
        return vec!["no bundle name".into()];
    };
    let Some(parent) = dir.parent() else {
        return vec!["no bundle directory".into()];
    };
    let packs = DirPacks::new([parent]).with_catalog(catalog);
    let mut out = vec![];
    for flow in bundle::flow_names(name, dir) {
        match packs.flow(&flow) {
            None => out.push(format!(
                "flow {flow}: cannot be read (it does not parse, or says another name than its file's)"
            )),
            Some((def, _)) => {
                out.extend(validate(&def, &packs).into_iter().map(|p| format!("flow {flow}: {p}")))
            }
        }
    }
    for kind in PartKind::ALL {
        for part in bundle::part_names(name, dir, kind) {
            match packs.part(kind, &part) {
                None => out.push(format!("{} {part}: cannot be read", kind.as_str())),
                Some((text, _)) => out.extend(
                    part_problems(kind, &part, &text, &packs)
                        .into_iter()
                        .map(|p| format!("{} {part}: {p}", kind.as_str())),
                ),
            }
        }
    }
    if let Ok(meta) = bundle::meta(dir) {
        for c in meta.commands.names() {
            if c.is_empty()
                || c.contains(['/', '\\'])
                || c.starts_with('.')
                || !dir.join("commands").join(format!("{c}.js")).is_file()
            {
                out.push(format!("command {c}: no commands/{c}.js"));
            }
        }
    }
    out
}

/// 解开并核对过的包：暂存目录在，`dir` 是 `<暂存>/<包名>`。
pub struct Verified {
    _tmp: tempfile::TempDir,
    pub dir: PathBuf,
    pub manifest: Manifest,
    pub meta: bundle::BundleMeta,
}

impl Verified {
    pub fn info(&self) -> FlowPackageInfo {
        info(&self.manifest, &self.dir)
    }
}

/// 读、核对、解到暂存目录（`stage_in` 里；省略 = 系统临时目录）、查兼容、校验。
pub fn verify(file: &Path, stage_in: Option<&Path>, catalog: Catalog) -> Result<Verified, String> {
    let pkg = read_package(file)?;
    let name = pkg.manifest.bundle.name.clone();
    if !bundle::is_bundle_name(&name) {
        return Err(format!(
            "the bundle name {name:?} is not one a bundle can have"
        ));
    }
    let tmp = match stage_in {
        Some(p) => {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
            tempfile::Builder::new()
                .prefix("flow-import-")
                .tempdir_in(p)
        }
        None => tempfile::Builder::new().prefix("conix-flow-").tempdir(),
    }
    .map_err(|e| e.to_string())?;
    let dir = tmp.path().join(&name);
    for (rel, bytes) in &pkg.files {
        let to = rel.split('/').fold(dir.clone(), |p, c| p.join(c));
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::write(&to, bytes).map_err(|e| format!("{rel}: {e}"))?;
    }
    let meta = bundle::meta(&dir)?;
    if meta.name != name || version_of(&meta) != pkg.manifest.bundle.version {
        return Err(format!(
            "{MANIFEST} says {name} {}, {} says {} {}",
            pkg.manifest.bundle.version,
            bundle::BUNDLE_FILE,
            meta.name,
            version_of(&meta)
        ));
    }
    check_compatible(&pkg.manifest, &meta)?;
    let problems = validate_bundle(&dir, catalog);
    if !problems.is_empty() {
        return Err(format!(
            "the bundle does not validate:\n{}",
            problems
                .iter()
                .map(|p| format!("  - {p}"))
                .collect::<Vec<_>>()
                .join("\n")
        ));
    }
    Ok(Verified {
        _tmp: tmp,
        dir,
        manifest: pkg.manifest,
        meta,
    })
}

// ── 导入 ────────────────────────────────────────────────────────────────────

pub struct ImportOptions<'a> {
    pub force: bool,
    pub catalog: Catalog,
    /// 启用着的插件的流程目录：只用来提醒"遮住了插件的同名流程包"。
    pub plugin_dirs: &'a [(String, PathBuf)],
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)?.flatten() {
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if e.file_type()?.is_dir() {
            copy_tree(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst)?;
        }
    }
    Ok(())
}

/// 旧的那份里用户自己写的 `skills/*/local.md`，搬进新的（新的有这个 skill 才搬）。
fn carry_local(old: &Path, new: &Path) {
    let Ok(rd) = std::fs::read_dir(old.join(PartKind::Skill.dir())) else {
        return;
    };
    for e in rd.flatten() {
        let name = e.file_name();
        let src = e.path().join(bundle::LOCAL_SKILL);
        let dst_dir = new.join(PartKind::Skill.dir()).join(&name);
        if src.is_file() && dst_dir.is_dir() {
            let _ = std::fs::copy(&src, dst_dir.join(bundle::LOCAL_SKILL));
        }
    }
}

/// 装进项目的 `.conix/flows/<包名>/`。
pub fn import(file: &Path, project: &Path, o: &ImportOptions) -> Result<FlowImported, String> {
    if !project.is_dir() {
        return Err(format!("{}: not a project directory", project.display()));
    }
    let v = verify(file, Some(&project.join(STATE)), o.catalog.clone())?;
    let name = v.manifest.bundle.name.clone();
    let flows = project.join(PROJECT_FLOWS);
    let target = flows.join(&name);
    let exists = std::fs::symlink_metadata(&target).is_ok();
    if exists && !o.force {
        return Err(format!(
            "{} already exists in the project; --force replaces it (the old one is kept in {BACKUPS}/)",
            target.display()
        ));
    }
    std::fs::create_dir_all(&flows).map_err(|e| format!("{}: {e}", flows.display()))?;
    let mut backup = None;
    if exists {
        carry_local(&target, &v.dir);
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let b = project.join(BACKUPS).join(format!("{name}-{stamp}"));
        std::fs::create_dir_all(b.parent().unwrap_or(project)).map_err(|e| e.to_string())?;
        std::fs::rename(&target, &b).map_err(|e| format!("backing up the old {name}: {e}"))?;
        backup = Some(b);
    }
    // 暂存目录与目标同一个文件系统时 rename 一步到位；不是就复制，失败了把旧的放回去。
    let placed = std::fs::rename(&v.dir, &target).or_else(|_| copy_tree(&v.dir, &target));
    if let Err(e) = placed {
        let _ = std::fs::remove_dir_all(&target);
        if let Some(b) = &backup {
            let _ = std::fs::rename(b, &target);
        }
        return Err(format!("installing {name}: {e}"));
    }
    let shadows = o
        .plugin_dirs
        .iter()
        .find(|(_, d)| bundle::bundle_in(d, &name).is_some())
        .map(|(p, _)| format!("plugin {p}"))
        .or_else(|| {
            super::builtin::bundles()
                .contains(&name.as_str())
                .then(|| "builtin".to_string())
        });
    Ok(FlowImported {
        package: info(&v.manifest, &target),
        installed: target.to_string_lossy().into_owned(),
        backup: backup.map(|b| b.to_string_lossy().into_owned()),
        shadows,
    })
}

/// `conix flow inspect`：MANIFEST 原文（读得出来就给）与核对的结果。
pub struct Inspected {
    pub manifest: Option<String>,
    pub verified: Result<FlowPackageInfo, String>,
}

/// 归档的事后核对（layout.md §3）：`runs_dir` 是项目的运行账本目录。直接读账本、不经实例：实例开着也只是
/// 读一个文件，账本的写入（与它的索引）仍只有实例一家。record.json 里没写运行、账本里没有，都是"没有账本可对"。
pub fn verify_archive(runs_dir: &Path, dir: &Path) -> harnix_flow::archive::ArchiveReport {
    use harnix_flow::RunStore as _;
    let run = std::fs::read(dir.join(harnix_flow::archive::RECORD))
        .ok()
        .and_then(|t| serde_json::from_slice::<serde_json::Value>(&t).ok())
        .and_then(|v| v["run"].as_str().map(str::to_string));
    let ledger = run.and_then(|run| {
        let store = harnix_flow::FileStore::new(runs_dir);
        // 没有运行时：FileStore 在没有 Tokio 运行时的时候就地读，这个 future 不会挂起。
        futures::executor::block_on(store.load(&run)).ok().flatten()
    });
    harnix_flow::archive::verify(dir, ledger.as_ref())
}

pub fn inspect(file: &Path) -> Inspected {
    Inspected {
        manifest: manifest_text(file).ok(),
        verified: verify(file, None, default_catalog()).map(|v| v.info()),
    }
}

/// 没有实例时导出：项目的与内置的流程包（内置的先写到临时目录）；插件的要在实例里导。
pub fn export_offline(
    project: &Path,
    bundle_name: &str,
    out: Option<&Path>,
    default_dir: &Path,
) -> Result<FlowExported, String> {
    if let Some((dir, _)) = locate(project, &[], None, bundle_name) {
        return export(&dir, out, default_dir);
    }
    if super::builtin::bundles().contains(&bundle_name) {
        let tmp = tempfile::tempdir().map_err(|e| e.to_string())?;
        super::builtin::write(tmp.path()).map_err(|e| e.to_string())?;
        if let Some(dir) = bundle::bundle_in(tmp.path(), bundle_name) {
            return export(&dir, out, default_dir);
        }
    }
    Err(format!(
        "no flow bundle {bundle_name} in {} or built in (a plugin's bundle is exported from a running instance: /flow:export)",
        project.join(PROJECT_FLOWS).display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xforge_zip(dir: &Path) -> PathBuf {
        export_offline(dir, "xforge", Some(&dir.join("x.zip")), dir)
            .unwrap()
            .file
            .into()
    }

    fn project() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn unzip(file: &Path) -> Vec<(String, Vec<u8>)> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(file).unwrap()).unwrap();
        (0..z.len())
            .map(|i| {
                let mut e = z.by_index(i).unwrap();
                let mut b = vec![];
                e.read_to_end(&mut b).unwrap();
                (e.name().to_string(), b)
            })
            .collect()
    }

    fn rewrite(file: &Path, f: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) {
        let mut all = unzip(file);
        f(&mut all);
        std::fs::write(file, write_zip(&all).unwrap()).unwrap();
    }

    fn opts(force: bool) -> ImportOptions<'static> {
        ImportOptions {
            force,
            catalog: default_catalog(),
            plugin_dirs: &[],
        }
    }

    fn tree(dir: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        fn go(base: &Path, d: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    go(base, &p, out)
                } else {
                    out.insert(
                        p.strip_prefix(base).unwrap().to_string_lossy().into_owned(),
                        std::fs::read(&p).unwrap(),
                    );
                }
            }
        }
        go(dir, dir, &mut out);
        out
    }

    #[test]
    fn the_builtin_xforge_exports_and_imports_into_another_project() {
        let a = project();
        let b = project();
        let z = xforge_zip(a.path());
        let manifest: Manifest = serde_yaml::from_str(&manifest_text(&z).unwrap()).unwrap();
        assert_eq!(manifest.format, 1);
        assert_eq!(manifest.bundle.name, "xforge");
        assert_eq!(manifest.conix.version, conix_version());
        assert_eq!(manifest.conix.build, BUILD);
        assert_eq!(manifest.requires.conix, format!(">={}", conix_version()));
        let text = manifest_text(&z).unwrap();
        assert!(!text.contains("/Users") && !text.contains("home"), "{text}");
        let r = import(&z, b.path(), &opts(false)).unwrap();
        assert_eq!(r.package.name, "xforge");
        assert_eq!(r.package.flows, ["xforge/quick", "xforge/solid"]);
        assert!(r.package.commands.contains(&"xspec".to_string()));
        assert!(r.package.command_gates > 0);
        assert_eq!(r.shadows.as_deref(), Some("builtin"));
        assert!(r.backup.is_none());
        let src = tempfile::tempdir().unwrap();
        super::super::builtin::write(src.path()).unwrap();
        let installed = tree(&b.path().join(PROJECT_FLOWS).join("xforge"));
        let mut built = tree(&src.path().join("xforge"));
        built.retain(|p, _| !p.split('/').any(|c| c.starts_with('.')));
        assert_eq!(
            installed.keys().collect::<Vec<_>>(),
            built.keys().collect::<Vec<_>>()
        );
        assert_eq!(installed, built);
        assert!(!installed.contains_key(MANIFEST));
        let again = import(&z, b.path(), &opts(false)).unwrap_err();
        assert!(again.contains("--force"), "{again}");
        let seen = inspect(&z);
        assert_eq!(seen.verified.unwrap().name, "xforge");
        assert!(seen.manifest.unwrap().contains("sha256"));
    }

    #[test]
    fn a_changed_byte_an_extra_or_a_missing_file_is_refused() {
        let a = project();
        let z = xforge_zip(a.path());
        let b = project();
        let tampered = a.path().join("t.zip");
        std::fs::copy(&z, &tampered).unwrap();
        rewrite(&tampered, |all| {
            let f = all.iter_mut().find(|(n, _)| n == "solid.yaml").unwrap();
            f.1.push(b'\n');
        });
        let e = import(&tampered, b.path(), &opts(false)).unwrap_err();
        assert!(e.contains("solid.yaml") && e.contains("sha256"), "{e}");
        std::fs::copy(&z, &tampered).unwrap();
        rewrite(&tampered, |all| {
            all.push(("extra.md".into(), b"x".to_vec()))
        });
        assert!(
            import(&tampered, b.path(), &opts(false))
                .unwrap_err()
                .contains("not in MANIFEST")
        );
        std::fs::copy(&z, &tampered).unwrap();
        rewrite(&tampered, |all| all.retain(|(n, _)| n != "solid.yaml"));
        assert!(
            import(&tampered, b.path(), &opts(false))
                .unwrap_err()
                .contains("not in the package")
        );
        assert!(
            !b.path().join(PROJECT_FLOWS).join("xforge").exists(),
            "nothing installed"
        );
    }

    #[test]
    fn paths_that_escape_and_symlinks_are_refused_before_anything_is_written() {
        let a = project();
        let z = xforge_zip(a.path());
        let b = project();
        let bad = a.path().join("bad.zip");
        for evil in ["../evil.txt", "/etc/evil", "skills/../../evil", "a\\b"] {
            std::fs::copy(&z, &bad).unwrap();
            rewrite(&bad, |all| all.push((evil.into(), b"x".to_vec())));
            let e = import(&bad, b.path(), &opts(false)).unwrap_err();
            assert!(e.contains("relative path"), "{evil}: {e}");
        }
        // MANIFEST 里自己写一个出包的路径，文件也配上。
        std::fs::copy(&z, &bad).unwrap();
        rewrite(&bad, |all| {
            let mut m: Manifest = serde_yaml::from_slice(&all[0].1).unwrap();
            m.files.push(ManifestFile {
                path: "../evil".into(),
                sha256: sha256(b"x"),
                size: 1,
            });
            all[0].1 = serde_yaml::to_string(&m).unwrap().into_bytes();
            all.push(("../evil".into(), b"x".to_vec()));
        });
        assert!(import(&bad, b.path(), &opts(false)).is_err());
        // 符号链接条目。
        let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
        zip.add_symlink(
            "link",
            "/etc/passwd",
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
        std::fs::write(&bad, zip.finish().unwrap().into_inner()).unwrap();
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("symlink")
        );
        assert!(!b.path().join("evil").exists() && !a.path().join("evil.txt").exists());
    }

    #[test]
    fn limits_and_unknown_formats_are_refused() {
        let a = project();
        let z = xforge_zip(a.path());
        let b = project();
        let bad = a.path().join("bad.zip");
        std::fs::copy(&z, &bad).unwrap();
        rewrite(&bad, |all| {
            all[0].1 = String::from_utf8(all[0].1.clone())
                .unwrap()
                .replacen("format: 1", "format: 2", 1)
                .into_bytes();
        });
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("format 2")
        );
        std::fs::copy(&z, &bad).unwrap();
        rewrite(&bad, |all| {
            all.push(("big.bin".into(), vec![0; MAX_FILE as usize + 1]))
        });
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("at most")
        );
        std::fs::copy(&z, &bad).unwrap();
        rewrite(&bad, |all| {
            for i in 0..MAX_FILES {
                all.push((format!("many/{i}.txt"), vec![]));
            }
        });
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("at most")
        );
        std::fs::copy(&z, &bad).unwrap();
        rewrite(&bad, |all| {
            for i in 0..6 {
                all.push((format!("fat/{i}.bin"), vec![1; MAX_FILE as usize]));
            }
        });
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("bigger than")
        );
    }

    #[test]
    fn a_package_that_needs_a_newer_conix_or_library_is_refused() {
        let a = project();
        let z = xforge_zip(a.path());
        let b = project();
        let bad = a.path().join("bad.zip");
        let remake = |from: &str, to: String| {
            std::fs::copy(&z, &bad).unwrap();
            rewrite(&bad, |all| {
                let t = String::from_utf8(all[0].1.clone()).unwrap();
                assert!(t.contains(from), "{t}");
                all[0].1 = t.replacen(from, &to, 1).into_bytes();
            });
        };
        remake(&format!(">={}", conix_version()), ">=99.0.0".into());
        let e = import(&bad, b.path(), &opts(false)).unwrap_err();
        assert!(e.contains("99.0.0") && e.contains("newer"), "{e}");
        remake(
            "flow_definition_format: 1",
            format!("flow_definition_format: {}", DEFINITION_FORMAT + 1),
        );
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("flow definitions")
        );
        remake("flow_api: 1", format!("flow_api: {}", FLOW_API_VERSION + 1));
        assert!(
            import(&bad, b.path(), &opts(false))
                .unwrap_err()
                .contains("flow API")
        );
    }

    #[test]
    fn a_bundle_that_does_not_validate_is_refused() {
        let a = project();
        let src = a.path().join(PROJECT_FLOWS).join("broken");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("bundle.yaml"), "name: broken\n").unwrap();
        std::fs::write(
            src.join("flow.yaml"),
            "name: broken\nstations: [{id: a, executor: {kind: session}, skill: nope, exit: [{kind: confirm}]}]\n",
        )
        .unwrap();
        let z = export_offline(a.path(), "broken", None, a.path()).unwrap();
        assert!(z.file.ends_with("broken-0.0.0.zip"), "{}", z.file);
        let b = project();
        let e = import(Path::new(&z.file), b.path(), &opts(false)).unwrap_err();
        assert!(e.contains("does not validate") && e.contains("nope"), "{e}");
    }

    #[test]
    fn force_backs_the_old_one_up_and_keeps_the_users_local_skill_sections() {
        let a = project();
        let z = xforge_zip(a.path());
        let b = project();
        import(&z, b.path(), &opts(false)).unwrap();
        let installed = b.path().join(PROJECT_FLOWS).join("xforge");
        let skill = std::fs::read_dir(installed.join("skills"))
            .unwrap()
            .flatten()
            .next()
            .unwrap()
            .path();
        std::fs::write(skill.join("local.md"), "我们这里还要写英文摘要。\n").unwrap();
        std::fs::write(installed.join("mine.txt"), "old only").unwrap();
        let r = import(&z, b.path(), &opts(true)).unwrap();
        let backup = PathBuf::from(r.backup.unwrap());
        assert!(backup.starts_with(b.path().join(BACKUPS)) && backup.join("mine.txt").is_file());
        assert!(!installed.join("mine.txt").exists());
        assert_eq!(
            std::fs::read_to_string(skill.join("local.md")).unwrap(),
            "我们这里还要写英文摘要。\n"
        );
        // 导出不带 local.md：用户自己的补充不外传。
        let again = export(&installed, Some(&a.path().join("again.zip")), a.path()).unwrap();
        assert!(
            unzip(Path::new(&again.file))
                .iter()
                .all(|(n, _)| !n.ends_with("local.md"))
        );
    }

    #[test]
    fn requires_in_bundle_yaml_wins_and_reserved_names_are_not_bundles() {
        let a = project();
        let src = a.path().join(PROJECT_FLOWS).join("pinned");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(
            src.join("bundle.yaml"),
            "name: pinned\nversion: 1.2.3\nrequires: {conix: \">=0.1.0\"}\n",
        )
        .unwrap();
        let e = export_offline(a.path(), "pinned", None, a.path()).unwrap();
        assert!(e.file.ends_with("pinned-1.2.3.zip"));
        let m: Manifest =
            serde_yaml::from_str(&manifest_text(Path::new(&e.file)).unwrap()).unwrap();
        assert_eq!(m.requires.conix, ">=0.1.0");
        for n in bundle::RESERVED_BUNDLES {
            assert!(locate(a.path(), &[], None, n).is_none());
        }
    }
}
