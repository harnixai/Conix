//! 命令行：帮助按语言显示（需求 L-11），流程包的管理（design/flows.md §7.12）——起真实的二进制、
//! 临时的全局目录（`CONIX_HOME`）与项目，切到英文。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| {
        matches!(c, '\u{4e00}'..='\u{9fff}' | '\u{3000}'..='\u{303f}' | '\u{ff00}'..='\u{ffef}')
    })
}

fn help_of(args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_conix"))
        .args(args)
        .env("CONIX_LANG", "en")
        .env("CONIX_HOME", tempfile::tempdir().unwrap().path())
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).into_owned()
}

#[test]
fn cli_help_is_english() {
    let text = help_of(&["--help"]);
    assert!(text.contains("agent workbench"), "{text}");
    assert!(!has_cjk(&text), "{text}");
    for sub in ["new", "list", "attach", "start", "stop"] {
        let text = help_of(&[sub, "--help"]);
        assert!(!has_cjk(&text), "{sub}: {text}");
    }
    assert!(help_of(&["attach", "--help"]).contains("Kick out"));
    for sub in [
        "flow", "list", "show", "install", "update", "remove", "reset", "export", "import",
        "inspect", "verify", "backfill", "diff", "clean", "doctor",
    ] {
        let mut a = vec!["flow"];
        if sub != "flow" {
            a.push(sub);
        }
        a.push("--help");
        let text = help_of(&a);
        assert!(!has_cjk(&text), "flow {sub}: {text}");
    }
}

/// 一个全局目录与一个项目；命令都从项目里跑（`-C`）。stdin 不是终端。
struct Env {
    home: tempfile::TempDir,
    project: tempfile::TempDir,
}

impl Env {
    fn new() -> Env {
        Env {
            home: tempfile::tempdir().unwrap(),
            project: tempfile::tempdir().unwrap(),
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let p = self.project.path().to_str().unwrap().to_string();
        let mut all: Vec<&str> = vec!["flow", "-C", &p];
        all.extend(args);
        Command::new(env!("CARGO_BIN_EXE_conix"))
            .args(&all)
            .env("CONIX_LANG", "en")
            .env("CONIX_HOME", self.home.path())
            .stdin(std::process::Stdio::null())
            .output()
            .unwrap()
    }

    /// 应当成功：回 stdout 与 stderr 拼起来的文字。
    fn ok(&self, args: &[&str]) -> String {
        let o = self.run(args);
        let text = text(&o);
        assert!(o.status.success(), "{args:?}: {text}");
        text
    }

    /// 应当失败：回文字。
    fn fails(&self, args: &[&str]) -> String {
        let o = self.run(args);
        let text = text(&o);
        assert!(!o.status.success(), "{args:?} should fail: {text}");
        text
    }

    fn global(&self) -> PathBuf {
        self.home.path().join("flows")
    }

    fn installed(&self, name: &str) -> PathBuf {
        self.project.path().join(".conix/flows").join(name)
    }

    fn lock(&self) -> serde_yaml::Value {
        serde_yaml::from_str(
            &std::fs::read_to_string(self.project.path().join(".conix/flows.lock.yaml"))
                .unwrap_or_default(),
        )
        .unwrap_or(serde_yaml::Value::Null)
    }

    fn registry(&self) -> serde_yaml::Value {
        serde_yaml::from_str(
            &std::fs::read_to_string(self.global().join(".registry.yaml")).unwrap(),
        )
        .unwrap()
    }
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn set_version(bundle_yaml: &Path, v: &str) {
    let t = std::fs::read_to_string(bundle_yaml).unwrap();
    let t: Vec<String> = t
        .lines()
        .map(|l| {
            if l.starts_with("version:") {
                format!("version: {v}")
            } else {
                l.to_string()
            }
        })
        .collect();
    std::fs::write(bundle_yaml, t.join("\n") + "\n").unwrap();
}

/// 一个自己写的小包：一条一站的流程。
fn small_bundle(dir: &Path, name: &str, version: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(
        dir.join("bundle.yaml"),
        format!("name: {name}\nversion: {version}\nflows: [main]\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("main.yaml"),
        "name: main\nstations:\n  - {id: a, exit: [{kind: confirm}]}\n",
    )
    .unwrap();
}

#[test]
fn the_builtin_bundle_is_bootstrapped_into_the_global_library_and_reset_restores_it() {
    let e = Env::new();
    let out = e.ok(&["list"]);
    assert!(
        out.contains("built-in xforge") && out.contains("global library"),
        "{out}"
    );
    assert!(out.contains("No flow bundles in the project"), "{out}");
    assert!(e.global().join("xforge/bundle.yaml").is_file());
    let reg = e.registry();
    let x = &reg["bundles"]["xforge"];
    assert_eq!(x["source"].as_str(), Some("builtin"));
    assert_eq!(x["digest"], x["shipped"]["digest"]);
    assert!(
        !e.project.path().join(".conix/flows").exists(),
        "the global library does not reach the project"
    );
    let g = e.ok(&["list", "--global"]);
    assert!(
        g.contains("xforge") && g.contains("built-in, unmodified"),
        "{g}"
    );
    // 第二次起来什么都不做。
    assert!(!e.ok(&["list"]).contains("Flow library:"));

    // 改了全局库里的，reset 先备份再恢复。
    std::fs::write(e.global().join("xforge/mine.md"), "mine").unwrap();
    assert!(e.ok(&["list", "--global"]).contains("built-in, modified"));
    assert!(e.fails(&["reset"]).contains("--all"));
    e.ok(&["reset", "xforge", "--yes"]);
    assert!(!e.global().join("xforge/mine.md").exists());
    let backups: Vec<_> = std::fs::read_dir(e.global().join(".backups"))
        .unwrap()
        .flatten()
        .collect();
    assert_eq!(backups.len(), 1);
    assert!(backups[0].path().join("mine.md").is_file());
    assert!(
        e.ok(&["reset", "--all", "--yes"])
            .contains("already the one")
    );

    // remove --global 删得掉内置的，起来时不会自己回来；reset 找回。
    let out = e.ok(&["remove", "xforge", "--global", "--yes"]);
    assert!(out.contains("reset xforge brings it back"), "{out}");
    assert!(!e.global().join("xforge").exists());
    let out = e.ok(&["list", "--global"]);
    assert!(out.contains("removed"), "{out}");
    assert!(!e.global().join("xforge").exists());
    e.ok(&["reset", "xforge", "--yes"]);
    assert!(e.global().join("xforge/bundle.yaml").is_file());
}

#[test]
fn a_pristine_older_builtin_is_upgraded_at_start_and_a_modified_one_is_kept() {
    let e = Env::new();
    e.ok(&["list"]);
    let dir = e.global().join("xforge");
    let shipped = std::fs::read_to_string(dir.join("bundle.yaml")).unwrap();
    // 装成"上一版内置的、没改过"。
    let make_old = |modified: bool| {
        set_version(&dir.join("bundle.yaml"), "0.0.1");
        let digest = host::flows::package::dir_digest(&dir).unwrap();
        let mut reg = e.registry();
        let x = &mut reg["bundles"]["xforge"];
        x["version"] = "0.0.1".into();
        x["digest"] = digest.clone().into();
        x["shipped"]["version"] = "0.0.1".into();
        x["shipped"]["digest"] = if modified {
            "older".into()
        } else {
            digest.into()
        };
        std::fs::write(
            e.global().join(".registry.yaml"),
            serde_yaml::to_string(&reg).unwrap(),
        )
        .unwrap();
    };
    make_old(false);
    let out = e.ok(&["list", "--global"]);
    assert!(
        out.contains("upgraded from 0.0.1"),
        "a pristine copy follows the shipped one: {out}"
    );
    assert_eq!(
        std::fs::read_to_string(dir.join("bundle.yaml")).unwrap(),
        shipped
    );

    make_old(true);
    let out = e.ok(&["list", "--global"]);
    assert!(
        !out.contains("upgraded"),
        "a modified copy is left alone: {out}"
    );
    assert!(
        out.contains("built-in, modified") && out.contains("this Conix ships"),
        "{out}"
    );
    assert!(
        std::fs::read_to_string(dir.join("bundle.yaml"))
            .unwrap()
            .contains("0.0.1")
    );
}

#[test]
fn install_update_and_remove_follow_the_rules_and_keep_local_md() {
    let e = Env::new();
    // 不在终端、没给 --yes：拒绝，什么都不动；--dry-run 只列出。
    let out = e.fails(&["install", "xforge"]);
    assert!(
        out.contains("--yes") && out.contains("install xforge"),
        "{out}"
    );
    assert!(!e.installed("xforge").exists());
    let out = e.ok(&["install", "xforge", "--dry-run"]);
    assert!(out.contains("added") && out.contains("--dry-run"), "{out}");
    assert!(!e.installed("xforge").exists());

    let out = e.ok(&["install", "xforge", "--yes"]);
    assert!(out.contains("command gates"), "{out}");
    assert!(e.installed("xforge").join("bundle.yaml").is_file());
    let lock = e.lock();
    let l = &lock["bundles"]["xforge"];
    assert_eq!(l["from"].as_str(), Some("global"));
    let digest = host::flows::package::dir_digest(&e.installed("xforge")).unwrap();
    assert_eq!(l["digest"].as_str(), Some(digest.as_str()));
    assert_eq!(l["digest"], e.registry()["bundles"]["xforge"]["digest"]);
    assert!(e.ok(&["list"]).contains("installed"));
    assert!(
        e.fails(&["install", "xforge", "--yes"])
            .contains("update xforge")
    );

    // local.md 不算改动、不进 lock 的摘要。
    let skill = e.installed("xforge").join("skills/propose");
    std::fs::write(skill.join("local.md"), "ours\n").unwrap();
    let out = e.ok(&["list"]);
    assert!(
        out.contains("installed") && !out.contains("modified"),
        "{out}"
    );
    assert!(
        e.ok(&["update", "xforge", "--yes"])
            .contains("already the same")
    );

    // 全局库出了新版、项目没改：更新，local.md 带过去。
    let gy = e.global().join("xforge/bundle.yaml");
    set_version(&gy, "9.0.0");
    assert!(e.ok(&["list"]).contains("global has 9.0.0"));
    let out = e.ok(&["update", "--all", "--yes"]);
    assert!(out.contains("→ 9.0.0"), "{out}");
    assert_eq!(
        std::fs::read_to_string(skill.join("local.md")).unwrap(),
        "ours\n"
    );
    assert_eq!(
        e.lock()["bundles"]["xforge"]["version"].as_str(),
        Some("9.0.0")
    );

    // 项目里改过：拒绝并列出；--force 覆盖、先备份。
    set_version(&gy, "9.1.0");
    std::fs::write(e.installed("xforge").join("mine.md"), "x").unwrap();
    assert!(e.ok(&["list"]).contains("modified"));
    let out = e.fails(&["update", "xforge", "--yes"]);
    assert!(
        out.contains("changed in the project") && out.contains("mine.md"),
        "{out}"
    );
    let out = e.ok(&["update", "xforge", "--force", "--yes"]);
    assert!(out.contains("backed up"), "{out}");
    assert!(!e.installed("xforge").join("mine.md").exists());
    assert!(
        e.project
            .path()
            .join(".conix/state/conix/flow-backups")
            .is_dir()
    );
    assert_eq!(
        std::fs::read_to_string(skill.join("local.md")).unwrap(),
        "ours\n"
    );

    // 同版本：无事；全局更旧：拒绝，--force 才降级。
    assert!(
        e.ok(&["update", "xforge", "--yes"])
            .contains("already the same")
    );
    set_version(&gy, "1.0.0");
    assert!(e.fails(&["update", "xforge", "--yes"]).contains("older"));
    e.ok(&["update", "xforge", "--force", "--yes"]);
    assert_eq!(
        e.lock()["bundles"]["xforge"]["version"].as_str(),
        Some("1.0.0")
    );

    // remove：先备份，lock 里去掉。
    e.ok(&["remove", "xforge", "--yes"]);
    assert!(!e.installed("xforge").exists());
    assert!(e.lock()["bundles"]["xforge"].is_null());
    assert!(
        e.fails(&["remove", "xforge", "--yes"])
            .contains("No flow bundle")
    );
    assert!(
        e.fails(&["update", "nope", "--yes"])
            .contains("not installed")
    );
    assert!(
        e.fails(&["install", "nope", "--yes"])
            .contains("global flow library")
    );
}

/// 在项目的运行账本里起一条停在确认上的运行，流程名 `flow`（`<包>` 或 `<包>/<流程>` 的包那一段）。
async fn run_in_flight(project: &Path, flow: &str) -> String {
    use std::sync::Arc;
    let runs = project.join(".conix/state/conix/flow-runs");
    let packs = Arc::new(harnix_flow::MemPacks::new().flow(&format!(
        "name: {flow}\nstations:\n  - {{id: a, exit: [{{kind: confirm}}]}}\n"
    )));
    let eng = harnix_flow::Engine::new(Arc::new(harnix_flow::FileStore::new(runs)), packs);
    let o = eng
        .create(
            harnix_flow::CreateIn {
                flow: flow.into(),
                subject: "s".into(),
                ..Default::default()
            },
            harnix_flow::Actor::new("user", "me"),
        )
        .await
        .unwrap();
    o.value["run"].as_str().unwrap().to_string()
}

#[tokio::test]
async fn remove_refuses_while_runs_in_flight_use_the_bundle() {
    let e = Env::new();
    e.ok(&["install", "xforge", "--yes"]);
    let run = run_in_flight(e.project.path(), "xforge").await;
    let out = e.fails(&["remove", "xforge", "--yes"]);
    assert!(out.contains(&run) && out.contains("--force"), "{out}");
    assert!(e.installed("xforge").exists());
    e.ok(&["remove", "xforge", "--force", "--yes"]);
    assert!(!e.installed("xforge").exists());
}

#[test]
fn export_and_import_go_between_the_project_the_global_library_and_files() {
    let e = Env::new();
    let work = tempfile::tempdir().unwrap();
    let zip = work.path().join("xforge.zip");
    let zip_s = zip.to_str().unwrap();
    // 缺省从项目导出：项目里没有就拒绝；--global 从全局库。
    assert!(
        e.fails(&["export", "xforge", "-o", zip_s])
            .contains("No flow bundle")
    );
    e.ok(&["export", "xforge", "--global", "-o", zip_s]);
    let out = e.ok(&["inspect", zip_s]);
    assert!(
        out.contains("format: 1") && out.contains("Verified"),
        "{out}"
    );

    // 导入缺省进全局库：已有就拒绝，--force 覆盖。
    assert!(e.fails(&["import", zip_s, "--yes"]).contains("--force"));
    e.ok(&["import", zip_s, "--force", "--yes"]);
    assert_eq!(
        e.registry()["bundles"]["xforge"]["source"].as_str(),
        Some("import")
    );
    // --project：直接进项目，不记 lock（只在项目）。
    e.ok(&["import", zip_s, "--project", "--yes"]);
    assert!(e.installed("xforge").join("bundle.yaml").is_file());
    assert!(e.lock()["bundles"]["xforge"].is_null());
    assert!(e.ok(&["list"]).contains("project only"));
    assert!(
        e.fails(&["import", zip_s, "--project", "--yes"])
            .contains("--force")
    );
    let out = e.ok(&["export", "xforge", "-o", work.path().to_str().unwrap()]);
    assert!(out.contains("Exported xforge"), "{out}");

    // 自己写的包导出、导进另一个全局库。
    let mine = e.project.path().join(".conix/flows/mine");
    small_bundle(&mine, "mine", "1.0.0");
    e.ok(&["export", "mine", "-o", work.path().to_str().unwrap()]);
    let other = Env::new();
    other.ok(&[
        "import",
        work.path().join("mine-1.0.0.zip").to_str().unwrap(),
        "--yes",
    ]);
    assert!(other.global().join("mine/main.yaml").is_file());
    assert!(other.ok(&["list", "--global"]).contains("mine"));
}

#[test]
fn backfill_puts_the_project_copy_into_the_global_library_by_the_rules() {
    let e = Env::new();
    // 只在项目的包：库里没有就加，source: backfill，lock 跟上。
    let mine = e.installed("mine");
    small_bundle(&mine, "mine", "1.0.0");
    assert!(e.ok(&["list"]).contains("project only"));
    let out = e.ok(&["backfill", "mine", "--yes"]);
    assert!(out.contains("backfill mine"), "{out}");
    assert!(e.global().join("mine/main.yaml").is_file());
    assert_eq!(
        e.registry()["bundles"]["mine"]["source"].as_str(),
        Some("backfill")
    );
    assert_eq!(
        e.lock()["bundles"]["mine"]["version"].as_str(),
        Some("1.0.0")
    );
    assert!(e.ok(&["list"]).contains("installed"));
    assert!(
        e.ok(&["backfill", "mine", "--yes"])
            .contains("already the same")
    );

    // local.md 不回填。
    std::fs::create_dir_all(mine.join("skills/s")).unwrap();
    std::fs::write(
        mine.join("skills/s/SKILL.md"),
        "---\nname: s\ndescription: d\n---\nbody\n",
    )
    .unwrap();
    std::fs::write(mine.join("skills/s/local.md"), "ours").unwrap();
    // 同版本、内容不同：拒绝；改版本就替换（先备份）。
    assert!(
        e.fails(&["backfill", "mine", "--yes"])
            .contains("bump version")
    );
    let d = e.ok(&["diff", "mine"]);
    assert!(
        d.contains("+ skills/s/SKILL.md") && !d.contains("local.md"),
        "{d}"
    );
    set_version(&mine.join("bundle.yaml"), "1.1.0");
    let out = e.ok(&["backfill", "mine", "--yes"]);
    assert!(
        out.contains("1.0.0 → 1.1.0") && out.contains("backed up"),
        "{out}"
    );
    assert!(e.global().join("mine/skills/s/SKILL.md").is_file());
    assert!(!e.global().join("mine/skills/s/local.md").exists());
    assert_eq!(
        e.lock()["bundles"]["mine"]["version"].as_str(),
        Some("1.1.0")
    );
    assert!(e.ok(&["diff", "mine"]).contains("same content"));

    // 库里更新：拒绝，--force 才盖。
    set_version(&e.global().join("mine/bundle.yaml"), "2.0.0");
    std::fs::write(mine.join("extra.md"), "x").unwrap();
    assert!(e.fails(&["backfill", "mine", "--yes"]).contains("newer"));
    e.ok(&["backfill", "mine", "--force", "--yes"]);
    assert!(e.global().join("mine/extra.md").is_file());

    // 回填内置包：库里那份成改过的内置，reset 回去。
    e.ok(&["install", "xforge", "--yes"]);
    std::fs::write(e.installed("xforge").join("ours.md"), "x").unwrap();
    set_version(&e.installed("xforge").join("bundle.yaml"), "9.9.9");
    let out = e.ok(&["backfill", "xforge", "--yes"]);
    assert!(out.contains("modified built-in"), "{out}");
    assert!(e.ok(&["list", "--global"]).contains("built-in, modified"));
    e.ok(&["reset", "xforge", "--yes"]);
    assert!(e.ok(&["list", "--global"]).contains("built-in, unmodified"));
    assert!(
        e.fails(&["backfill", "nope", "--yes"])
            .contains("No flow bundle")
    );
}

#[test]
fn clean_removes_backups_staging_and_orphan_entries() {
    let e = Env::new();
    e.ok(&["install", "xforge", "--yes"]);
    e.ok(&["install", "xforge", "--force", "--yes"]);
    let backups = e.project.path().join(".conix/state/conix/flow-backups");
    assert!(std::fs::read_dir(&backups).unwrap().count() > 0);
    std::fs::create_dir_all(e.project.path().join(".conix/state/conix/flow-import-left")).unwrap();
    std::fs::create_dir_all(
        e.project
            .path()
            .join(".conix/state/conix/flows-builtin/xforge"),
    )
    .unwrap();
    // lock 里有、目录没了。
    std::fs::remove_dir_all(e.installed("xforge")).unwrap();
    assert!(e.ok(&["list"]).contains("directory gone"));
    let out = e.ok(&["clean", "--dry-run"]);
    assert!(
        out.contains("flow-import-left") && out.contains("orphan"),
        "{out}"
    );
    assert!(
        backups.read_dir().unwrap().count() > 0,
        "dry run did nothing"
    );
    e.ok(&["clean", "--yes"]);
    assert!(!backups.exists() || backups.read_dir().unwrap().count() == 0);
    assert!(
        !e.project
            .path()
            .join(".conix/state/conix/flow-import-left")
            .exists()
    );
    assert!(
        !e.project
            .path()
            .join(".conix/state/conix/flows-builtin")
            .exists()
    );
    assert!(e.lock()["bundles"]["xforge"].is_null());
    assert!(e.ok(&["clean", "--yes"]).contains("Nothing to clean"));

    // 全局：备份、暂存、registry 里目录没了的非内置包。
    let mine = e.installed("mine");
    small_bundle(&mine, "mine", "1.0.0");
    e.ok(&["backfill", "mine", "--yes"]);
    std::fs::remove_dir_all(e.global().join("mine")).unwrap();
    std::fs::create_dir_all(e.global().join(".staging/flow-import-x")).unwrap();
    e.ok(&["reset", "xforge", "--yes"]);
    let out = e.ok(&["clean", "--global", "--yes"]);
    assert!(out.contains("mine"), "{out}");
    assert!(e.registry()["bundles"]["mine"].is_null());
    assert!(
        !e.registry()["bundles"]["xforge"].is_null(),
        "built-ins stay"
    );
    assert_eq!(
        std::fs::read_dir(e.global().join(".staging"))
            .unwrap()
            .count(),
        0
    );
}

fn sha256_hex(b: &[u8]) -> String {
    use sha2::Digest as _;
    sha2::Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// 一份归档：一个文件与列着它的 record.json；`honest` 为假时清单里的摘要不对。
fn archive(project: &Path, bundle: &str, name: &str, honest: bool) {
    let dir = project
        .join("conix")
        .join(bundle)
        .join("archive")
        .join(name);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("a.md"), "x").unwrap();
    let sha = if honest {
        sha256_hex(b"x")
    } else {
        "00".repeat(32)
    };
    std::fs::write(
        dir.join("record.json"),
        serde_json::json!({"format": 1, "files": [{"path": "a.md", "sha256": sha, "size": 1}]})
            .to_string(),
    )
    .unwrap();
}

#[test]
fn verify_all_scans_every_archive_of_the_project() {
    let e = Env::new();
    assert!(e.ok(&["verify", "--all"]).contains("No archives"));
    archive(e.project.path(), "xforge", "2026112601-solid", true);
    archive(e.project.path(), "brief", "2026112602-brief", true);
    let out = e.ok(&["verify", "--all"]);
    assert!(
        out.contains("2026112601-solid") && out.contains("2026112602-brief"),
        "{out}"
    );
    archive(e.project.path(), "brief", "2026112603-brief", false);
    let out = e.fails(&["verify", "--all"]);
    assert!(out.contains("1 of 3"), "{out}");
    e.fails(&["verify"]);
}

#[tokio::test]
async fn doctor_reports_what_is_wrong_and_fix_does_only_the_safe_things() {
    let e = Env::new();
    e.ok(&["list"]);
    // 全局：registry 里目录没了的非内置包、暂存残留、内置包的目录没了。
    let mine = e.installed("mine");
    small_bundle(&mine, "mine", "1.0.0");
    e.ok(&["backfill", "mine", "--yes"]);
    std::fs::remove_dir_all(e.global().join("mine")).unwrap();
    std::fs::create_dir_all(e.global().join(".staging/flow-import-x")).unwrap();
    std::fs::remove_dir_all(e.global().join("xforge")).unwrap();
    let out = e.ok(&["doctor", "--global"]);
    assert!(
        out.contains("directory gone")
            && out.contains("staging left behind")
            && out.contains("can be fixed"),
        "{out}"
    );
    e.ok(&["doctor", "--global", "--fix"]);
    assert!(
        e.global().join("xforge/bundle.yaml").is_file(),
        "the built-in is back"
    );
    assert!(e.registry()["bundles"]["mine"].is_null());
    assert_eq!(
        std::fs::read_dir(e.global().join(".staging"))
            .unwrap()
            .count(),
        0
    );
    let out = e.ok(&["doctor", "--global"]);
    assert!(!out.contains("can be fixed"), "{out}");

    // 项目：与库里一样却没记 lock 的、lock 里目录没了的、保留名、进行中的运行的包不在了、校验不过的。
    let src = e.global().join("xforge");
    let dst = e.installed("xforge");
    copy(&src, &dst);
    std::fs::remove_dir_all(&mine).unwrap();
    small_bundle(&e.installed("export"), "export", "1.0.0");
    let broken = e.installed("broken");
    small_bundle(&broken, "broken", "1.0.0");
    std::fs::write(
        broken.join("main.yaml"),
        "name: main\nstations:\n  - {id: a, executor: {kind: session}, skill: nope, exit: [{kind: output}]}\n",
    )
    .unwrap();
    run_in_flight(e.project.path(), "gone").await;
    let out = e.fails(&["doctor"]);
    for want in [
        "not recorded in the lock",
        "directory gone",
        "reserved name",
        "runs in flight use it",
        "does not validate",
        "nope",
        "errors",
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    // --fix：补 lock、去孤儿；校验不过的、保留名只报。
    let out = e.fails(&["doctor", "--fix"]);
    assert!(out.contains("Fixed"), "{out}");
    let lock = e.lock();
    assert_eq!(lock["bundles"]["xforge"]["from"].as_str(), Some("global"));
    assert!(lock["bundles"]["mine"].is_null());
    assert!(
        broken.join("main.yaml").is_file(),
        "nothing else is touched"
    );

    // 进行中的运行用的包库里有、项目里没有：--fix 装上。
    std::fs::remove_dir_all(&broken).unwrap();
    std::fs::remove_dir_all(e.installed("export")).unwrap();
    let e2 = Env::new();
    e2.ok(&["list"]);
    run_in_flight(e2.project.path(), "xforge").await;
    let out = e2.fails(&["doctor"]);
    assert!(out.contains("conix flow install xforge"), "{out}");
    // 装上之后 xforge 自己的自检还提醒一条：新项目没写检视线的模型（照跑，第一站记告警；不是 --fix 能替人定的）。
    let out = e2.ok(&["doctor", "--fix"]);
    assert!(out.contains("Fixed: installed xforge"), "{out}");
    let out = e2.ok(&["doctor"]);
    assert!(
        out.contains("◐ xforge:") && out.contains("检视线没写模型"),
        "{out}"
    );
    assert!(e2.installed("xforge").join("bundle.yaml").is_file());
}

fn copy(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let (s, d) = (e.path(), to.join(e.file_name()));
        if s.is_dir() {
            copy(&s, &d);
        } else {
            std::fs::copy(&s, &d).unwrap();
        }
    }
}

/// 包建议的作业：装的时候列出来，`--yes` 不写 `--jobs` 不加，`--jobs yes` 才加、加进来是停用的；项目里已有的 ID
/// 不动；更新只提新版新带来的；删包不删作业、只提醒。
#[test]
fn a_bundles_suggested_jobs_are_offered_at_install_and_added_disabled_only_when_asked() {
    let e = Env::new();
    let jobs_json = e.project.path().join(".conix/jobs.json");
    let state_json = e.project.path().join(".conix/state/conix/jobs-state.json");
    let read = |p: &Path| -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
    };
    let with_jobs = |dir: &Path, version: &str, extra: &str| {
        small_bundle(dir, "mine", version);
        let y = dir.join("bundle.yaml");
        let mut t = std::fs::read_to_string(&y).unwrap();
        t.push_str(concat!(
            "jobs:\n",
            "  - {id: daily, name: Daily check, cron: \"0 9 * * *\", kind: {simple: {prompt: Look at yesterday's errors and report in three lines}}}\n",
            "  - {id: weekly, name: Weekly run, cron: \"0 10 * * 1\", kind: {flow: {flow: mine/main}}}\n",
        ));
        t.push_str(extra);
        std::fs::write(&y, t).unwrap();
    };
    // 坏的 cron：流程库只看形状，宿主校验包时判出来，进不了库。
    let mine = e.installed("mine");
    with_jobs(
        &mine,
        "1.0.0",
        "  - {id: bad, name: Bad, cron: \"99 * * * *\", kind: {simple: {prompt: x}}}\n",
    );
    let out = e.fails(&["backfill", "mine", "--yes"]);
    assert!(
        out.contains("does not validate") && out.contains("cron"),
        "{out}"
    );
    std::fs::remove_dir_all(&mine).unwrap();
    with_jobs(&mine, "1.0.0", "");
    e.ok(&["backfill", "mine", "--yes"]);
    e.ok(&["remove", "mine", "--yes"]);

    // --dry-run 列出来；--yes 不写 --jobs：装了包、作业不加，说一声。
    let out = e.ok(&["install", "mine", "--dry-run"]);
    assert!(
        out.contains("Jobs the bundle suggests (2 to add")
            && out.contains("+ daily  Daily check  0 9 * * *")
            && out.contains("starts the flow mine/main"),
        "{out}"
    );
    let out = e.ok(&["install", "mine", "--yes"]);
    assert!(out.contains("--jobs yes"), "{out}");
    assert!(mine.join("bundle.yaml").is_file() && !jobs_json.exists());
    e.ok(&["remove", "mine", "--yes"]);

    // 项目里已有 daily：不覆盖；weekly 加进来、停用。
    std::fs::create_dir_all(jobs_json.parent().unwrap()).unwrap();
    std::fs::write(
        &jobs_json,
        r#"[{"id":"daily","name":"Ours","cron":"0 8 * * *","kind":{"simple":{"prompt":"ours"}}}]"#,
    )
    .unwrap();
    let out = e.ok(&["install", "mine", "--yes", "--jobs", "yes"]);
    assert!(
        out.contains("daily  Ours: the project already has this id")
            && out.contains("Added the job weekly (disabled)"),
        "{out}"
    );
    let defs = read(&jobs_json);
    assert_eq!(defs.as_array().unwrap().len(), 2, "{defs}");
    assert_eq!(defs[0]["name"], "Ours");
    assert_eq!(defs[1]["id"], "weekly");
    assert_eq!(defs[1]["kind"]["flow"]["flow"], "mine/main");
    assert_eq!(defs[1]["kind"]["flow"]["subject"], "Weekly run");
    assert_eq!(read(&state_json)["weekly"]["enabled"], false);

    // 新版多带一个：只提它，旧版提过的不再提。
    let g = e.global().join("mine");
    std::fs::remove_dir_all(&g).unwrap();
    with_jobs(
        &g,
        "1.1.0",
        "  - {id: monthly, name: Monthly, cron: \"0 0 1 * *\", kind: {simple: {prompt: tidy up}}}\n",
    );
    let out = e.ok(&["update", "mine", "--dry-run"]);
    assert!(
        out.contains("(1 to add") && out.contains("+ monthly") && !out.contains("+ daily"),
        "{out}"
    );
    let out = e.ok(&["update", "mine", "--force", "--yes", "--jobs", "no"]);
    assert!(!out.contains("Added the job"), "{out}");
    assert_eq!(read(&jobs_json).as_array().unwrap().len(), 2);

    // 删包：作业留着，提醒哪几个起的是它的流程。
    let out = e.ok(&["remove", "mine", "--yes"]);
    assert!(
        out.contains("not removed with it") && out.contains("Weekly run (weekly)"),
        "{out}"
    );
    assert_eq!(read(&jobs_json).as_array().unwrap().len(), 2);
}

/// 内置的包装进一个新项目后，`conix flow doctor` 跑包自己的自检（flows.md §7.13，xforge.md §7，xreport.md §5）：
/// 缺的治理文件只是 warn（退出码 0），坏的是 error（退出码非 0）；xreport 的 git 经执行服务起，项目没受信、
/// 这台机器关了命令（全局 conix.yaml 的 `flows.command_gates: off`）都说清为什么、该做什么。
#[test]
fn doctor_runs_each_installed_bundles_own_check_on_a_fresh_project() {
    let e = Env::new();
    let project = e.project.path().canonicalize().unwrap();
    let git = |args: &[&str]| {
        let o = Command::new("git")
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@x",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(&project)
            .output()
            .unwrap();
        assert!(o.status.success(), "git {args:?}: {}", text(&o));
    };
    std::fs::write(project.join("README.md"), "# p\n").unwrap();
    git(&["init", "-q"]);
    git(&["add", "-A"]);
    git(&["commit", "-qm", "init"]);

    // xforge：没有治理文件、原则、verify.sh——都是 warn，路径照跑；solid、major 缺省开着检视点、检视线却没写模型
    // （与开发线一样是实例的缺省模型）也只是 warn：照跑，第一站记告警。
    e.ok(&["install", "xforge", "--yes"]);
    let out = e.ok(&["doctor"]);
    for want in [
        "◐ xforge: solid 开着检视点 design、tests-design，检视线没写模型",
        "◐ xforge: major 开着检视点 spec、design、tests-design，检视线没写模型",
        "◐ xforge: xforge/solid、xforge/major 需要 verify.blackbox",
        "◐ xforge: 没有原则文件 .conix/governance/constitution.yaml",
        "◐ xforge: 没有 .conix/governance/verify.sh",
        "· xforge: 治理文件里没有 verify 菜单",
        "the project is not trusted",
    ] {
        assert!(out.contains(want), "{want}: {out}");
    }
    assert_eq!(out.matches("✗").count(), 0, "{out}");
    // 照演示项目备齐治理文件：不再有 warn，只剩"基线还是空的"（第一次运行会建）。
    let example =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge/.conix/governance");
    copy(&example, &project.join(".conix/governance"));
    let out = e.ok(&["doctor"]);
    assert!(
        !out.contains("◐ xforge")
            && !out.contains("✗")
            && out.contains("· xforge: 规格基线还是空的"),
        "{out}"
    );
    // 原则是空的：门禁 constitution 必然 error，自检也是 error，退出码非 0。
    std::fs::write(
        project.join(".conix/governance/constitution.yaml"),
        "principles: []\n",
    )
    .unwrap();
    let out = e.fails(&["doctor"]);
    assert!(
        out.contains("✗ xforge:") && out.contains("没有原则"),
        "{out}"
    );
    copy(&example, &project.join(".conix/governance"));

    // xreport：git 经执行服务起。项目没受信是 error，说 /trust。
    e.ok(&["install", "xreport", "--yes"]);
    let out = e.fails(&["doctor"]);
    assert!(
        out.contains("✗ xreport:") && out.contains("没受信") && out.contains("/trust"),
        "{out}"
    );
    // 受信了：git 起得来，项目有提交，只剩"没有设置文件，用缺省"。
    std::fs::write(
        e.home.path().join("settings.json"),
        serde_json::json!({"app": {"trusted_projects": [project]}}).to_string(),
    )
    .unwrap();
    let out = e.ok(&["doctor"]);
    assert!(!out.contains("✗") && !out.contains("not trusted"), "{out}");
    assert!(
        out.contains("· xreport: 没有 .conix/governance/xreport.yaml"),
        "{out}"
    );
    // 这台机器关了命令：起不了 git，说是哪个开关。
    std::fs::write(
        e.home.path().join("conix.yaml"),
        "flows:\n  command_gates: off\n",
    )
    .unwrap();
    let out = e.fails(&["doctor"]);
    assert!(
        out.contains("✗ xreport:")
            && out.contains("flows.command_gates 是 off")
            && !out.contains("git 起不来"),
        "{out}"
    );
}
