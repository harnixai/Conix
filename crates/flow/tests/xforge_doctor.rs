//! The xforge bundle's doctor (`scripts/doctor.js`, in Conix; its docs/design/xforge.md §7): the
//! example project has everything xforge needs; a settings file that does not read, a switch of the
//! wrong type, a verify entry the menu's own check refuses, a black-box entry with parameters, a
//! review line on the build line's model where a path reviews are errors; no black-box entry, no principles, no `verify.sh`, no git are warnings; empty baselines
//! are only said. It changes nothing in the project. No model.

use std::path::{Path, PathBuf};

use harnix_flow::*;

const SETTINGS: &str = ".conix/governance/xforge.yaml";

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

/// As Conix offers the bundle: a built-in directory of bundles, command gates on.
fn packs() -> DirPacks {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    DirPacks::new([example().join(".conix/flows")]).with_catalog(catalog)
}

/// As Conix lays the example out: its directory of artifacts is `xforge`, which is the bundle's data.
fn layout() -> ProjectLayout {
    ProjectLayout::new(".conix", "/nowhere", "conix").with_artifacts_dir("xforge")
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A copy of the example as a git repository of its own, committed.
fn project() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), &format!("cp -R {}/. . && git init -q && git add -A && git -c user.email=x@y -c user.name=x commit -qm base", example().display()));
    d
}

async fn check(dir: &Path) -> DoctorReport {
    let r = doctor(&packs(), Some(&layout()), "xforge", dir).await.unwrap();
    assert_eq!(r.script.as_deref(), Some("scripts/doctor.js"));
    r
}

fn edit(dir: &Path, rel: &str, f: impl FnOnce(String) -> String) {
    let p = dir.join(rel);
    std::fs::write(&p, f(std::fs::read_to_string(&p).unwrap())).unwrap();
}

/// The findings of a level, as `message` texts.
fn of(r: &DoctorReport, level: DoctorLevel) -> Vec<String> {
    r.findings.iter().filter(|f| f.level == level).map(|f| f.message.clone()).collect()
}

#[tokio::test]
async fn the_example_project_has_everything_xforge_needs() {
    let p = project();
    let before = sh(p.path(), "git status --porcelain --untracked-files=all");
    let r = check(p.path()).await;
    assert_eq!(r.findings, vec![], "{r:#?}");
    // Read-only: the project is as it was.
    assert_eq!(sh(p.path(), "git status --porcelain --untracked-files=all"), before);
    // The bundle names it, and it holds together with the bundle's flows.
    let packs = packs();
    let (def, _) = packs.flow("xforge/solid").unwrap();
    assert_eq!(validate(&def, &packs), Vec::<Problem>::new());
}

#[tokio::test]
async fn a_settings_file_that_does_not_read_or_a_switch_of_the_wrong_type_is_an_error_on_the_file() {
    let p = project();
    std::fs::write(p.path().join(SETTINGS), "governance: [spec\n").unwrap();
    let r = check(p.path()).await;
    let errors: Vec<_> = r.findings.iter().filter(|f| f.level == DoctorLevel::Error).collect();
    assert_eq!(errors.len(), 1, "{r:#?}");
    assert_eq!(errors[0].path.as_deref(), Some(SETTINGS));
    assert!(errors[0].message.contains("治理文件读不成") && errors[0].message.contains("xforge.yaml"), "{}", errors[0].message);

    edit(p.path(), SETTINGS, |_| "governance: {spec: \"no\", interfaces: true}\nreview: {separate: yes}\nverfiy: {}\n".into());
    let r = check(p.path()).await;
    let errors = of(&r, DoctorLevel::Error);
    assert!(errors.iter().any(|m| m.contains("governance.spec 要是 true 或 false")), "{errors:?}");
    assert!(errors.iter().any(|m| m.contains("review.separate 要是 true 或 false")), "{errors:?}");
    let warns = of(&r, DoctorLevel::Warn);
    assert!(warns.iter().any(|m| m.contains("governance.interfaces xforge 不认识")), "{warns:?}");
    assert!(warns.iter().any(|m| m.contains("verfiy xforge 不认识")), "{warns:?}");
    assert!(r.findings.iter().all(|f| f.path.as_deref() == Some(SETTINGS) || f.level != DoctorLevel::Error));
}

/// A review line on the build line's model, or not named, still runs: a warning, not an error, for
/// each path that reviews (solid its designs, major from the spec on), saying what the first station
/// will say. Only a line of the wrong shape is an error.
#[tokio::test]
async fn a_review_line_on_the_build_lines_model_or_unnamed_where_a_path_reviews_is_a_warning() {
    let p = project();
    let lines = |p: &Path, text: &str| edit(p, SETTINGS, |s| {
        let (head, rest) = s.split_once("lines:\n").unwrap();
        let tail = rest.split_once("\n\n").map(|(_, t)| t.to_string()).unwrap_or_default();
        format!("{head}lines:\n{text}\n\n{tail}")
    });
    lines(p.path(), "  build: {model: m1}\n  review: {model: m1}");
    let r = check(p.path()).await;
    assert_eq!(of(&r, DoctorLevel::Error), Vec::<String>::new());
    let warns: Vec<String> = of(&r, DoctorLevel::Warn).into_iter().filter(|m| m.contains("检视线")).collect();
    assert_eq!(warns.len(), 2, "{warns:?}");
    assert!(warns[0].contains("solid 开着检视点 design、tests-design，检视线与开发线是同一个模型 m1") && warns[0].contains("照跑"), "{}", warns[0]);
    assert!(warns[1].contains("major 开着检视点 spec、design、tests-design，检视线与开发线是同一个模型 m1"), "{}", warns[1]);
    // Not named: the instance's model, which is the build line's too when that is not named either.
    lines(p.path(), "  tests: {model: m2}");
    let r = check(p.path()).await;
    assert_eq!(of(&r, DoctorLevel::Error), Vec::<String>::new());
    assert!(of(&r, DoctorLevel::Warn).iter().any(|m| m.contains("检视线没写模型") && m.contains("两条线就是同一个模型")), "{r:#?}");
    // A line of the wrong shape is an error, a name not known a warning.
    lines(p.path(), "  build: {model: m1}\n  tests: [m2]\n  reviw: {model: m3}");
    let r = check(p.path()).await;
    let errors = of(&r, DoctorLevel::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("lines.tests 要写成 {model"), "{errors:?}");
    assert!(of(&r, DoctorLevel::Warn).iter().any(|m| m.contains("lines.reviw xforge 不认识")), "{r:#?}");
    // A review point that is no switch: the station's `when` would quietly not review.
    edit(p.path(), SETTINGS, |s| s.replace("tests-design: true, code: false, tests: false, verify: false}\n  solid", "tests-design: \"yes\", code: false, tests: false, verify: false}\n  solid"));
    let errors = of(&check(p.path()).await, DoctorLevel::Error);
    assert!(errors.iter().any(|m| m.contains("review.major.tests-design 要是 true 或 false")), "{errors:?}");
}

/// solid reviews only its two lines' designs (0.5.0): a review point of the old switch list there
/// does nothing, and the doctor says so rather than let a person think it is on.
#[tokio::test]
async fn a_review_point_solid_no_longer_has_is_a_warning_that_says_where_it_went() {
    let p = project();
    edit(p.path(), SETTINGS, |s| s.replace("  solid: {design: true, tests-design: true}", "  solid: {spec: true, design: true, tests-design: true, code: false}"));
    let r = check(p.path()).await;
    assert_eq!(of(&r, DoctorLevel::Error), Vec::<String>::new());
    let warns = of(&r, DoctorLevel::Warn);
    for k in ["spec", "code"] {
        assert!(warns.iter().any(|m| m.contains(&format!("review.solid.{k} 不起作用")) && m.contains("走 major")), "{k}: {warns:?}");
    }
}

#[tokio::test]
async fn a_verify_entry_the_menus_own_check_refuses_is_an_error() {
    let p = project();
    edit(p.path(), SETTINGS, |s| s + "  lint:\n    argv: [ruff, \"{target}\"]\n    params: {style: \"[a-z]+\"}\n  Bad:\n    argv: []\n");
    let r = check(p.path()).await;
    let errors = of(&r, DoctorLevel::Error);
    // The same words `xpkg plan --verify` and package-tests refuse the menu with (lib/verify.js).
    for says in ["verify.lint.argv 里的 {target} 在 params 里没有声明", "verify.lint.params.style 在 argv 里没用到", "verify.Bad：名字是小写字母"] {
        assert!(errors.iter().any(|m| m.contains(says)), "{says}: {errors:?}");
    }
    assert!(r.findings.iter().filter(|f| f.level == DoctorLevel::Error).all(|f| f.path.as_deref() == Some(SETTINGS) && f.fix.is_some()));
}

#[tokio::test]
async fn the_black_box_entry_is_wanted_by_solid_and_major_and_takes_no_parameters() {
    let p = project();
    edit(p.path(), SETTINGS, |s| s.split("\n  blackbox:").next().unwrap().to_string() + "\n");
    let r = check(p.path()).await;
    assert_eq!(of(&r, DoctorLevel::Error), Vec::<String>::new(), "{r:#?}");
    let warns = of(&r, DoctorLevel::Warn);
    assert_eq!(warns.len(), 1, "{warns:?}");
    assert!(warns[0].contains("xforge/solid、xforge/major 需要 verify.blackbox"), "{}", warns[0]);

    edit(p.path(), SETTINGS, |s| {
        s + "  blackbox:\n    argv: [python3, -m, unittest, discover, -s, \"{dir}\", -q]\n    params: {dir: \"tests_[a-z]+\"}\n"
    });
    let r = check(p.path()).await;
    let errors = of(&r, DoctorLevel::Error);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("verify.blackbox 不能带参数"), "{}", errors[0]);
}

#[tokio::test]
async fn what_else_a_project_prepares_is_warned_of_and_empty_baselines_are_only_said() {
    let p = project();
    std::fs::remove_file(p.path().join(".conix/governance/constitution.yaml")).unwrap();
    std::fs::remove_file(p.path().join(".conix/governance/verify.sh")).unwrap();
    std::fs::remove_dir_all(p.path().join("xforge/specs")).unwrap();
    std::fs::remove_dir_all(p.path().join("xforge/interfaces")).unwrap();
    std::fs::remove_dir_all(p.path().join(".git")).unwrap();
    let r = check(p.path()).await;
    assert_eq!(of(&r, DoctorLevel::Error), Vec::<String>::new(), "{r:#?}");
    let warns = of(&r, DoctorLevel::Warn);
    assert_eq!(warns.len(), 3, "{warns:?}");
    assert!(warns.iter().any(|m| m.contains("没有原则文件")), "{warns:?}");
    assert!(warns.iter().any(|m| m.contains("verify.sh")), "{warns:?}");
    assert!(warns.iter().any(|m| m.contains("不是 git 仓库")), "{warns:?}");
    let infos = of(&r, DoctorLevel::Info);
    assert!(infos.iter().any(|m| m.contains("规格基线还是空的（xforge/specs/）")), "{infos:?}");
    assert!(infos.iter().any(|m| m.contains("接口基线还是空的（xforge/interfaces/）")), "{infos:?}");

    // Principles the gates cannot answer by are errors.
    std::fs::write(p.path().join(".conix/governance/constitution.yaml"), "principles:\n  - {id: P1, title: a, text: x}\n  - {id: R2, title: b, text: y}\n  - {id: P1, title: c, text: z}\n").unwrap();
    let errors = of(&check(p.path()).await, DoctorLevel::Error);
    assert!(errors.iter().any(|m| m.contains("\"R2\" 不是 P<数字>")), "{errors:?}");
    assert!(errors.iter().any(|m| m.contains("P1 用了两次")), "{errors:?}");
}
