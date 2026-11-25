//! Flow bundles (in Conix: its docs/design/layout.md): read from a pack directory and a project, a
//! run of one end to end — its own directory under the artifacts, its process directory `_run/` in
//! it, a deliverable's skeleton put in place and judged, a command of the bundle writing only where
//! it may, and the archive with its record once the run finished.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, Turn, TOOL_COMMAND, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

const T0: u64 = 1_794_787_200_000;

/// A bundle `dev` with one flow, `work`: a station that hands in `report.md` from a skeleton.
fn write_bundle(dir: &Path) {
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", "name: dev\ntitle: 开发\ncommands: [note]\n"),
        (
            "work.yaml",
            "name: work\nstations:\n  - id: write\n    executor: {kind: session}\n    skill: write\n    output: {type: object}\n    deliverables:\n      - {path: report.md, template: report.md, description: 报告}\n    exit: [{kind: output}]\n",
        ),
        ("skills/write/SKILL.md", "---\nname: write\n---\n写报告。\n"),
        ("templates/report.md", "# 报告\n\n<!-- conix:template -->\n（在这里写）\n"),
        (
            "commands/note.js",
            r#"export default function (input, flow) {
  const [sub, ...rest] = input.args;
  if (sub === "add") { flow.append.jsonl(`${input.data_dir}/notes.jsonl`, { text: rest.join(" ") }); flow.print("added"); return { count: (flow.read.jsonl(`${input.data_dir}/notes.jsonl`) || []).length }; }
  if (sub === "escape") { flow.write.text("outside.txt", "x"); }
  if (sub === "process") { flow.write.text(`${input.output_dir}/_run/x.txt`, "x"); }
  return "usage: note add <text>";
}
"#,
        ),
    ];
    for (p, t) in files {
        let f = dir.join("dev").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
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
async fn a_bundles_run_works_in_its_own_directory_and_is_archived_with_its_record() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path());
    let host = ScriptedHost::new();
    host.main(vec![Turn::Runs]);
    let (rn, engine) = runner(host.clone(), packs_dir.path(), data.path());
    rn.start().await;
    assert!(engine.packs().flows().contains(&"dev/work".to_string()), "{:?}", engine.packs().flows());

    let root = project.path().canonicalize().unwrap();
    let o = rn
        .start_run(CreateIn { flow: "dev/work".into(), subject: "s".into(), project_root: Some(root.to_string_lossy().into()), ..Default::default() }, Actor::device("d"))
        .await
        .unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    rn.apply(o).await;
    assert!(run.starts_with("dev.work-"), "an id names files: {run}");

    // Where it works: its own directory under the artifacts; its bundle's data beside the runs.
    let rec = engine.record(&run).await.unwrap();
    // The id made a name (as a worktree's is).
    let run_dir = root.join("conix/runs").join(run.replace('.', "-"));
    assert_eq!(rec.output_dir.as_deref().map(Path::new), Some(run_dir.as_path()));
    assert_eq!(rec.data_dir.as_deref().map(Path::new), Some(root.join("conix/dev").as_path()));
    assert_eq!(std::fs::read_to_string(root.join("conix/.gitignore")).unwrap(), "runs/\n");

    // The task is sent: the skeleton is in place first, and the protocol names it and the command.
    let main = eventually("the main session opened", async || host.opened().into_iter().find(|s| s.main)).await;
    let sid = eventually("the session got its key", async || host.session_of(&main.key)).await;
    let sent = eventually("the task was sent", async || host.sent_to(&sid).into_iter().next()).await;
    assert!(std::fs::read_to_string(run_dir.join("report.md")).unwrap().contains("conix:template"));
    for w in ["report.md", "报告", "skeleton", "`note`", "flow_command", "conix/dev"] {
        assert!(sent.contains(w), "{w}: {sent}");
    }
    assert!(run_dir.join("_run/task.json").is_file(), "the process directory is in the run's own");

    // The bundle's command: writes the bundle's data; not out of the run's places, not `_run/`.
    let call = |args: Value| rn.tools().call(&sid, TOOL_COMMAND, json!({"command": "note", "args": args}));
    let r = call(json!(["add", "第一条"])).await;
    assert!(!r.is_error && r.text.contains("added") && r.text.contains("\"count\": 1") && r.text.contains("notes.jsonl"), "{}", r.text);
    assert_eq!(std::fs::read_to_string(root.join("conix/dev/notes.jsonl")).unwrap(), "{\"text\":\"第一条\"}\n");
    let r = call(json!(["escape"])).await;
    assert!(r.is_error && r.text.contains("writes only"), "{}", r.text);
    assert!(!root.join("outside.txt").exists());
    let r = call(json!(["process"])).await;
    assert!(r.is_error && r.text.contains("process directory"), "{}", r.text);
    let r = rn.tools().call(&sid, TOOL_COMMAND, json!({"command": "nope"})).await;
    assert!(r.is_error && r.text.contains("`note`"), "{}", r.text);

    // Handed in with the skeleton left as it was: the deliverables gate holds the station.
    let r = rn.tools().call(&sid, TOOL_SUBMIT, json!({"output": {}})).await;
    assert!(!r.is_error, "{}", r.text);
    let unmet = eventually("the deliverables were judged", async || {
        let rec = engine.record(&run).await.unwrap();
        let c = rec.stations["write"].checks.iter().find(|c| c.label == "deliverables")?.clone();
        (c.status != CheckStatus::Wait).then_some(c)
    })
    .await;
    assert_eq!(unmet.status, CheckStatus::Fail, "{unmet:?}");
    assert!(unmet.explain().contains("conix:template"), "{}", unmet.explain());

    // Written, and the gates run again (as `flow_retry` does): it passes, the run finishes and is
    // archived.
    std::fs::write(run_dir.join("report.md"), "# 报告\n\n结论：可以发布。\n").unwrap();
    let o = engine.rerun_gates(&run, "write", Actor::device("d")).await.unwrap();
    rn.apply(o).await;
    let rec = eventually("the run finished", async || {
        let rec = engine.record(&run).await.unwrap();
        (rec.status == RunStatus::Finished).then_some(rec)
    })
    .await;
    let archive = eventually("the run was archived", async || {
        let base = root.join("conix/dev/archive");
        std::fs::read_dir(&base).ok()?.flatten().map(|e| e.path()).find(|p| p.join("record.json").is_file())
    })
    .await;
    let name = archive.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with("01-work"), "{name}");
    assert_eq!(std::fs::read_to_string(archive.join("report.md")).unwrap(), "# 报告\n\n结论：可以发布。\n");
    let record: Value = serde_json::from_str(&std::fs::read_to_string(archive.join("record.json")).unwrap()).unwrap();
    assert_eq!((record["run"].as_str(), record["status"].as_str(), record["bundle"].as_str()), (Some(run.as_str()), Some("finished"), Some("dev")));
    assert_eq!(record["kept"], json!(["report.md"]));
    let text = record.to_string();
    assert!(!text.contains("\"lease\"") && !text.contains("\"token\"") && !text.contains(&run_dir.to_string_lossy().into_owned()), "no claims, paths beside the record: {text}");
    assert!(run_dir.join("_run/archived.json").is_file());
    assert_eq!(rec.stations["write"].status, StationStatus::Passed);
}

#[test]
fn a_project_bundle_stands_whole_for_one_offered_below_and_a_directory_without_bundle_yaml_only_adds() {
    let (base_dir, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(base_dir.path());
    let layout = ProjectLayout::new(".conix", "/tmp/w", "conix/flow");
    let base: Arc<dyn Packs> = Arc::new(DirPacks::new([base_dir.path()]));
    let own = project.path().join(".conix/flows");

    // Only a local.md: the bundle below, with the project's section after the skill.
    std::fs::create_dir_all(own.join("dev/skills/write")).unwrap();
    std::fs::write(own.join("dev/skills/write/local.md"), "我们这里写中英双语。\n").unwrap();
    let p = ProjectPacks::new(base.clone(), project.path(), &layout, true);
    assert!(p.flows().contains(&"dev/work".to_string()));
    let (skill, _) = p.skill("dev/write").unwrap();
    assert!(skill.contains("写报告。") && skill.contains("本项目的补充") && skill.contains("中英双语"), "{skill}");
    assert_eq!(p.origin("dev/work"), Origin::Builtin);
    assert!(p.save_part(PartKind::Skill, "dev/write", "x").unwrap_err().contains("copy it into"), "a bundle is changed whole");
    let untrusted = ProjectPacks::new(base.clone(), project.path(), &layout, false);
    assert!(!untrusted.skill("dev/write").unwrap().0.contains("中英双语"), "an untrusted project adds nothing");

    // The project's own bundle of that name: it alone is offered, flows and parts.
    std::fs::write(own.join("dev/bundle.yaml"), "name: dev\n").unwrap();
    std::fs::write(own.join("dev/fast.yaml"), "name: fast\nstations: [{id: a, exit: [{kind: confirm}]}]\n").unwrap();
    let p = ProjectPacks::new(base.clone(), project.path(), &layout, true);
    let flows = p.flows();
    assert!(flows.contains(&"dev/fast".to_string()) && !flows.contains(&"dev/work".to_string()), "{flows:?}");
    assert!(p.skill("dev/write").is_none(), "the bundle below is not looked into");
    assert_eq!(p.origin("dev/fast"), Origin::Project);
    p.save_part(PartKind::Skill, "dev/write", "---\nname: write\n---\n新的。\n").unwrap();
    assert!(own.join("dev/skills/write/SKILL.md").is_file());
    // A new bundle is started by saving a flow under its full name; on disk it says its own name.
    p.save_flow("name: lab/try\nstations: [{id: a, exit: [{kind: confirm}]}]\n").unwrap();
    assert!(own.join("lab/bundle.yaml").is_file());
    assert!(std::fs::read_to_string(own.join("lab/try.yaml")).unwrap().starts_with("name: try\n"));
    assert_eq!(p.flow("lab/try").unwrap().0.name, "lab/try");
    let untrusted = ProjectPacks::new(base, project.path(), &layout, false);
    assert!(untrusted.flow("dev/fast").is_none() && untrusted.flow("dev/work").is_none());
    assert!(untrusted.withheld_flows().iter().any(|(n, w)| n == "dev/fast" && *w == Withheld::Untrusted));
}

#[test]
fn a_bundles_flows_validate_only_against_its_own_parts() {
    let d = tempfile::tempdir().unwrap();
    write_bundle(d.path());
    std::fs::write(d.path().join("dev/bad.yaml"), "name: bad\noutput_dir: x\nstations:\n  - {id: a, executor: {kind: session}, skill: elsewhere, exit: [{kind: output}], deliverables: [{path: ../x.md, template: none.md}]}\n").unwrap();
    let packs = MemPacks::new().scene("flow-harnix").main_scene("flow-harnix").submit_scene("flow-harnix");
    let dir = DirPacks::new([d.path()]).with_catalog(packs.catalog());
    let (good, _) = dir.flow("dev/work").unwrap();
    assert_eq!(validate(&good, &dir), Vec::<Problem>::new());
    let (bad, _) = dir.flow("dev/bad").unwrap();
    let text = format!("{:?}", validate(&bad, &dir));
    for w in ["dev/elsewhere", "output_dir", "../x.md", "none.md"] {
        assert!(text.contains(w), "{w}: {text}");
    }
}
