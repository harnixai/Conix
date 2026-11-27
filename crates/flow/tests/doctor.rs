//! A bundle's doctor (in Conix; docs/design/flows.md §7.13): `bundle.yaml`'s `doctor` names a
//! script a host runs for a project outside any run. Without one the report is empty; what the
//! script finds is passed through; a script that throws, answers in another shape or tries to
//! write is one `error` naming it; it reads the project and its settings now, imports the bundle's
//! own files, and a `doctor` naming no script is a problem of the bundle's flows.

use std::path::Path;

use harnix_flow::*;
use serde_json::json;

const FLOW: &str = "name: a\nstations: [{id: s, exit: [{kind: confirm}]}]\n";

/// A directory of bundles holding `dev` (`bundle.yaml` with `extra` after its name, one flow,
/// `files`), and a project beside it.
fn bench(extra: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, DirPacks, std::path::PathBuf) {
    let d = tempfile::tempdir().unwrap();
    let b = d.path().join("packs/dev");
    let mut all = vec![("bundle.yaml".to_string(), format!("name: dev\n{extra}")), ("a.yaml".to_string(), FLOW.to_string())];
    all.extend(files.iter().map(|(p, t)| (p.to_string(), t.to_string())));
    for (p, t) in all {
        let f = b.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
    let project = d.path().join("project");
    std::fs::create_dir_all(&project).unwrap();
    let packs = DirPacks::new([d.path().join("packs")]);
    (d, packs, project)
}

fn layout() -> ProjectLayout {
    ProjectLayout::new(".conix", "/nowhere", "flow").with_artifacts_dir("out")
}

async fn run(packs: &DirPacks, project: &Path) -> DoctorReport {
    doctor(packs, Some(&layout()), "dev", project).await.unwrap()
}

#[tokio::test]
async fn a_bundle_without_a_doctor_has_nothing_to_say() {
    let (_d, packs, project) = bench("", &[]);
    let r = run(&packs, &project).await;
    assert_eq!(r, DoctorReport { bundle: "dev".into(), script: None, findings: vec![], log: String::new() });
    assert!(doctor(&packs, None, "nope", &project).await.unwrap_err().contains("no bundle `nope`"));
}

#[tokio::test]
async fn what_the_script_finds_is_passed_through_in_its_order() {
    let src = r#"
import { missing } from "../lib/check.js";
export default function (input, flow) {
  flow.log("checking", input.bundle);
  return { findings: [
    { level: "warn", message: `data in ${input.data_dir}`, path: "x.yaml", fix: "write x.yaml" },
    ...missing(flow, ["ready.txt", "absent.txt"]),
    { level: "info", message: JSON.stringify(input.params) + " " + (input.project.length > 0) },
  ] };
}"#;
    let lib = r#"export function missing(flow, paths) {
  return paths.filter((p) => flow.read.text(p) == null).map((p) => ({ level: "error", message: `${p} is missing`, path: p }));
}"#;
    let (_d, packs, project) = bench("doctor: doctor\n", &[("scripts/doctor.js", src), ("lib/check.js", lib)]);
    std::fs::write(project.join("ready.txt"), "yes").unwrap();
    let r = run(&packs, &project).await;
    assert_eq!(r.script.as_deref(), Some("scripts/doctor.js"));
    let got: Vec<_> = r.findings.iter().map(|f| (f.level, f.message.as_str(), f.path.as_deref(), f.fix.as_deref())).collect();
    assert_eq!(
        got,
        [
            (DoctorLevel::Warn, "data in out/dev", Some("x.yaml"), Some("write x.yaml")),
            (DoctorLevel::Error, "absent.txt is missing", Some("absent.txt"), None),
            (DoctorLevel::Info, "{} true", None, None),
        ]
    );
    assert_eq!((r.count(DoctorLevel::Error), r.worst()), (1, Some(DoctorLevel::Error)));
    assert_eq!(r.log, "checking dev\n");
    // Without a layout there is no data directory to name.
    let r = doctor(&packs, None, "dev", &project).await.unwrap();
    assert_eq!(r.findings[0].message, "data in null");
}

#[tokio::test]
async fn a_script_that_throws_or_answers_in_another_shape_is_one_error_naming_it() {
    for (src, says) in [
        ("export default function () { throw new Error('boom'); }", "boom"),
        ("export default function () { return [{ level: 'warn', message: 'x' }]; }", "another shape"),
        ("export default function () { return { findings: [{ level: 'fatal', message: 'x' }] }; }", "another shape"),
        ("export default function () { return { findings: [{ level: 'warn', message: 'x', severity: 'warn' }] }; }", "another shape"),
        ("export default function () { return { findings: [{ level: 'warn', message: ' ' }] }; }", "no message"),
        ("export default function () { return { findings: [], summary: 'ok' }; }", "another shape"),
        ("export default function () {}", "returned nothing"),
        ("export default function (i, flow) { flow.read.text('../secret'); return { findings: [] }; }", "out"),
    ] {
        let (_d, packs, project) = bench("doctor: doctor\n", &[("scripts/doctor.js", src)]);
        let r = run(&packs, &project).await;
        assert_eq!(r.findings.len(), 1, "{src}: {r:?}");
        let f = &r.findings[0];
        assert_eq!(f.level, DoctorLevel::Error, "{src}");
        assert!(f.message.contains("the doctor of bundle dev (scripts/doctor.js)") && f.message.contains(says), "{src}: {}", f.message);
    }
}

#[tokio::test]
async fn a_doctor_only_reads() {
    let src = "export default function (i, flow) { flow.write.text('made.txt', 'x'); return { findings: [] }; }";
    let (_d, packs, project) = bench("doctor: doctor\n", &[("scripts/doctor.js", src)]);
    let r = run(&packs, &project).await;
    assert_eq!(r.findings.len(), 1);
    assert_eq!(r.findings[0].level, DoctorLevel::Error);
    assert!(!project.join("made.txt").exists());
    // Nor runs anything: there is no process to start and no runs to look at.
    let src = "export default function (i, flow) { return { findings: [{ level: 'info', message: [typeof require, typeof flow.write, flow.runs.list().length].join(' ') }] }; }";
    let (_d, packs, project) = bench("doctor: doctor\n", &[("scripts/doctor.js", src)]);
    assert_eq!(run(&packs, &project).await.findings[0].message, "undefined undefined 0");
}

#[tokio::test]
async fn the_settings_are_the_projects_file_now_and_one_that_does_not_read_is_the_scripts_to_say() {
    let src = r#"export default function (i, flow) {
  try { return { findings: [{ level: "info", message: JSON.stringify(flow.settings) }] }; }
  catch (e) { return { findings: [{ level: "error", message: e.message, path: "cfg.yaml" }] }; }
}"#;
    let settings = "settings: {file: cfg.yaml, defaults: {a: 1, b: {c: true}}}\n";
    let (_d, packs, project) = bench(&format!("doctor: doctor\n{settings}"), &[("scripts/doctor.js", src)]);
    assert_eq!(run(&packs, &project).await.findings[0].message, json!({"a": 1, "b": {"c": true}}).to_string());
    std::fs::write(project.join("cfg.yaml"), "b: {c: false}\n").unwrap();
    assert_eq!(run(&packs, &project).await.findings[0].message, json!({"a": 1, "b": {"c": false}}).to_string());
    std::fs::write(project.join("cfg.yaml"), "- not a mapping\n").unwrap();
    let f = &run(&packs, &project).await.findings[0];
    assert_eq!(f.level, DoctorLevel::Error);
    assert!(f.message.contains("cfg.yaml"), "{}", f.message);
}

#[test]
fn a_doctor_naming_no_script_of_the_bundle_is_a_problem_of_its_flows() {
    let (_d, packs, _) = bench("doctor: doctor\n", &[]);
    let (def, _) = packs.flow("dev/a").unwrap();
    let problems: Vec<String> = validate(&def, &packs).into_iter().map(|p| p.message).collect();
    assert!(problems.iter().any(|m| m.contains("doctor names the script doctor") && m.contains("scripts/doctor.js")), "{problems:?}");
    let (_d, packs, _) = bench("doctor: ../x\n", &[]);
    let (def, _) = packs.flow("dev/a").unwrap();
    assert!(validate(&def, &packs).iter().any(|p| p.message.contains("is a script's name")));
    let (_d, packs, _) = bench("doctor: doctor\n", &[("scripts/doctor.js", "export default function () { return { findings: [] }; }")]);
    let (def, _) = packs.flow("dev/a").unwrap();
    assert_eq!(validate(&def, &packs), Vec::<Problem>::new());
}
