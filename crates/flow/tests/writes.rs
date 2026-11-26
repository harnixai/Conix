//! Write allow-lists (`Station.writes`): what a task is handed, and how its writes are judged.

mod common;

use common::*;
use harnix_flow::*;
use serde_json::json;

const WRITING: &str = r#"
name: writing
guard: auto
inputs: {items: {schema: {type: array}}}
stations:
  - id: pkg
    foreach: ${inputs.items}
    executor: {kind: session, scene: harnix}
    skill: s
    writes: ['${item.paths}', 'docs/*.md']
    permissions: [{effect: deny, tools: [Write, Edit], paths: ['**/secret.rs']}]
    output: {type: object}
    exit: [{kind: output}]
"#;

#[tokio::test]
async fn a_foreach_task_may_write_only_its_own_paths() {
    let mut h = harness(packs().skill("s", "Do it.").flow(WRITING));
    h.eng = h.eng.with_workspaces("/data/ws");
    assert_eq!(validate_yaml(WRITING, h.packs.as_ref()), vec![]);
    let items = json!([{"paths": ["src/a/**", "src/a.rs"]}, {"paths": ["src/b/**"]}]);
    let (_, o) = start_with(&h, "writing", json!({"items": items})).await;
    let scopes: Vec<(WriteScope, String)> = o
        .work
        .iter()
        .filter_map(|w| match w {
            WorkItem::Task { writes: Some(w), protocol, .. } => Some((w.clone(), protocol.clone().unwrap())),
            _ => None,
        })
        .collect();
    assert_eq!(scopes.len(), 2, "{:?}", o.work);
    assert_eq!(scopes[0].0.globs, ["src/a/**", "src/a.rs", "docs/*.md"]);
    assert_eq!(scopes[1].0.globs, ["src/b/**", "docs/*.md"]);
    assert!(scopes[1].1.contains("Write scope") && scopes[1].1.contains("`src/b/**`"), "{}", scopes[1].1);
    assert!(scopes[0].0.error.is_none());
    // Judged the way the runner does: the deny rule wins, the allow-list narrows the rest.
    let denied = [Granted {
        permission: serde_yaml::from_str("{effect: deny, tools: [Write], paths: ['**/secret.rs']}").unwrap(),
        from: "station pkg".into(),
    }];
    let judged = |scope: &WriteScope, file: &str| {
        let input = json!({"file_path": file});
        let call = Call {
            tool: "Write",
            input: Some(&input),
            paths: &[],
            workspace: Some("/data/ws/r"),
            root: Some("/data/ws/r"),
            reserved: &[],
            agent_dirs: &[],
        };
        let v = judge_under(Guard::Auto, &denied, &["Write".to_string()], &call, "pkg");
        narrow_writes(scope, v, &call, "pkg")
    };
    assert_eq!(judged(&scopes[1].0, "/data/ws/r/src/b/x.rs").effect, Effect::Allow);
    let v = judged(&scopes[1].0, "/data/ws/r/src/a/x.rs");
    assert_eq!(v.effect, Effect::Deny);
    assert!(v.reason.contains("may only write: src/b/**, docs/*.md"), "{}", v.reason);
    assert_eq!(judged(&scopes[1].0, "/data/ws/r/src/b/secret.rs").effect, Effect::Deny);
}

#[tokio::test]
async fn a_writes_reference_that_is_not_a_list_is_an_error_for_the_task() {
    let h = harness(packs().skill("s", "Do it.").flow(WRITING));
    let (_, o) = start_with(&h, "writing", json!({"items": [{"paths": 7}]})).await;
    let WorkItem::Task { writes: Some(w), .. } = &o.work[0] else { panic!("{:?}", o.work) };
    let e = w.error.as_deref().unwrap();
    assert!(e.contains("${item.paths}") && e.contains("not an array or a string"), "{e}");
}

#[test]
fn malformed_writes_are_reported() {
    let p = packs();
    let bad = "name: w\nstations: [{id: a, writes: ['', 'src/[ab', 'a**b', '${nope.x}'], exit: [{kind: confirm}]}]";
    let problems = format!("{:?}", validate_yaml(bad, &p));
    for want in ["a glob is empty", "unbalanced brackets", "`**` must be a whole path segment", "unknown root"] {
        assert!(problems.contains(want), "missing {want}: {problems}");
    }
}
