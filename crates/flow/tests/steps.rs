//! A bundle's run through the runner: a station whose items a script works out, done in their
//! order by a command of the bundle with no model, each hand-in judged by a gate on that item
//! before it is taken; then a session's hand-in sent back by its task's gate, fixed, and taken.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, Turn, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

const T0: u64 = 1_794_787_200_000;

fn write_bundle(dir: &Path) {
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", "name: pk\ncommands: [pkg]\n"),
        (
            "build.yaml",
            r#"name: build
stations:
  - id: plan
    foreach: {script: pending}
    max_parallel: 2
    executor: {kind: command, command: pkg, args: [make, "${item.id}"]}
    retry: {max: 1}
    output: {type: object}
    task_exit: [{kind: gate, gate: written}]
    exit: [{kind: output}]
  - id: write
    executor: {kind: session}
    skill: write
    output: {type: object}
    task_exit: [{kind: gate, gate: done}]
    exit: [{kind: output}]
"#,
        ),
        ("skills/write/SKILL.md", "---\nname: write\n---\n写完写 done.txt。\n"),
        (
            "scripts/pending.js",
            r#"export default function (input, flow) {
  return [{ id: "a" }, { id: "b", after: ["a"] }, { id: "c" }];
}
"#,
        ),
        (
            "commands/pkg.js",
            r#"export default function (input, flow) {
  const [sub, id] = input.args;
  const log = `${input.data_dir}/log.jsonl`;
  flow.append.jsonl(log, { id, by: input.by.kind, item: input.item.id, sub });
  const tries = (flow.read.jsonl(log) || []).filter((r) => r.id === id).length;
  // c leaves its file out the first time: its task's gate sends it back, and it is run again.
  if (id === "c" && tries === 1) return { id, skipped: true };
  flow.write.text(`${input.output_dir}/${id}.txt`, id);
  return id === "a" ? "made a" : { id };
}
"#,
        ),
        ("gates/written.yaml", "name: written\nkind: check\ncheck: files\npaths: [\"${output_dir}/${item.id}.txt\"]\n"),
        ("gates/done.yaml", "name: done\ntitle: done.txt 写了\nkind: check\ncheck: files\npaths: [\"${output_dir}/done.txt\"]\n"),
    ];
    for (p, t) in files {
        let f = dir.join("pk").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
}

#[test]
fn a_command_step_is_one_its_bundle_declares() {
    let d = tempfile::tempdir().unwrap();
    write_bundle(d.path());
    std::fs::write(
        d.path().join("pk/bad.yaml"),
        "name: bad\nstations:\n  - {id: a, executor: {kind: command, command: nope, args: ['${inputs.x}']}, output: {type: object}, exit: [{kind: output}]}\n",
    )
    .unwrap();
    let dir = DirPacks::new([d.path()]);
    let (bad, _) = dir.flow("pk/bad").unwrap();
    let text = format!("{:?}", validate(&bad, &dir));
    assert!(text.contains("declares no command nope") && text.contains("no input x"), "{text}");
}

fn runner(host: Arc<ScriptedHost>, packs_dir: &Path, data: &Path) -> (Arc<Runner>, Arc<Engine>) {
    let cfg = RunnerConfig {
        profiles: vec!["flow-harnix".into()],
        submit_profiles: ["flow-harnix".to_string()].into(),
        main_profile: Some("flow-harnix".into()),
        lease_ms: 60_000,
        tick_ms: 1000,
        ..Default::default()
    };
    let packs = CatalogPacks::over(&cfg, vec![], Arc::new(DirPacks::new([packs_dir])));
    let layout = ProjectLayout::new(".conix", data.join("worktrees"), "conix/flow").with_artifacts_dir("conix");
    packs.set_layout(layout.clone());
    let engine = Arc::new(
        Engine::new(Arc::new(MemStore::new()), Arc::new(packs))
            .with_clock(Arc::new(ManualClock::new(T0)))
            .with_lease_ms(60_000)
            .with_submit_scenes(cfg.profiles.clone())
            .with_main_scene("flow-harnix")
            .with_workspaces(data.join("workspaces"))
            .with_layout(layout),
    );
    (Runner::new(engine.clone(), host, cfg), engine)
}

async fn eventually<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let r = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(v) = f().await {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    r.unwrap_or_else(|_| panic!("{what} did not happen"))
}

#[tokio::test]
async fn scripted_items_run_in_order_as_commands_each_judged_by_its_gate_and_a_session_fixes_what_its_gate_found() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path());
    let host = ScriptedHost::new();
    host.main(vec![Turn::Runs]);
    let (rn, engine) = runner(host.clone(), packs_dir.path(), data.path());
    rn.start().await;
    let (def, _) = engine.packs().flow("pk/build").expect("the bundle's flow");
    assert_eq!(validate(&def, engine.packs().as_ref()), Vec::<Problem>::new());

    let root = project.path().canonicalize().unwrap();
    let o = rn
        .start_run(CreateIn { flow: "pk/build".into(), subject: "s".into(), project_root: Some(root.to_string_lossy().into()), ..Default::default() }, Actor::device("d"))
        .await
        .unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    rn.apply(o).await;

    // The items worked out, each done by the command and judged on its own file.
    let rec = eventually("the plan passed", async || {
        let rec = engine.record(&run).await.unwrap();
        (rec.stations["plan"].status == StationStatus::Passed).then_some(rec)
    })
    .await;
    let plan = &rec.stations["plan"];
    let ids: Vec<&str> = plan.tasks.iter().map(|t| t.id.as_str()).collect();
    assert_eq!(ids, ["a", "b", "c"]);
    assert_eq!(plan.task("a").unwrap().output, Some(json!({"result": "made a"})), "a value that is no object is wrapped");
    assert_eq!(plan.task("b").unwrap().output, Some(json!({"id": "b"})));
    let c = plan.task("c").unwrap();
    assert_eq!((c.attempt, c.output.clone()), (2, Some(json!({"id": "c"}))), "sent back once by its gate, then taken");
    assert_eq!(c.task_exit.as_ref().unwrap().status, CheckStatus::Pass);
    let log: Vec<Value> = std::fs::read_to_string(root.join("conix/pk/log.jsonl"))
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    let at = |id: &str| log.iter().position(|r| r["id"] == id).unwrap();
    assert!(at("a") < at("b"), "b comes after a: {log:?}");
    assert!(log.iter().all(|r| r["by"] == "station" && r["item"] == r["id"] && r["sub"] == "make"), "{log:?}");
    let hist = engine.history(&run).await.unwrap();
    let claimed_b = hist.iter().position(|h| h.op == "claim" && h.task.as_deref() == Some("b")).unwrap();
    let taken_a = hist.iter().position(|h| h.op == "submit" && h.task.as_deref() == Some("a")).unwrap();
    assert!(taken_a < claimed_b, "b is dispatched only once a is taken");

    // The session's hand-in: sent back by its task's gate in the reply, then fixed and taken.
    let main = eventually("the main session opened", async || host.opened().into_iter().find(|s| s.main)).await;
    let sid = eventually("the session got its key", async || host.session_of(&main.key)).await;
    let sent = eventually("the write task was sent", async || {
        host.sent_to(&sid).into_iter().find(|m| m.contains("checks of your own task"))
    })
    .await;
    assert!(sent.contains("`pk/done`"), "{sent}");
    let r = rn.tools().call(&sid, TOOL_SUBMIT, json!({"output": {}})).await;
    assert!(r.is_error && r.text.contains("checks of your task did not pass") && r.text.contains("done.txt"), "{}", r.text);
    let t = engine.record(&run).await.unwrap().stations["write"].task("write").unwrap().clone();
    assert_eq!((t.status, t.task_exit.as_ref().map(|x| x.status)), (TaskStatus::Claimed, Some(CheckStatus::Fail)));
    let run_dir = engine.record(&run).await.unwrap().output_dir.unwrap();
    std::fs::write(Path::new(&run_dir).join("done.txt"), "ok").unwrap();
    let r = rn.tools().call(&sid, TOOL_SUBMIT, json!({"output": {"ok": true}})).await;
    assert!(!r.is_error && r.text.contains("passed"), "{}", r.text);
    eventually("the run finished", async || {
        (engine.record(&run).await.unwrap().status == RunStatus::Finished).then_some(())
    })
    .await;
}
