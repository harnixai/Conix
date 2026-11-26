//! A bundle's commands as an entry of their own (in Conix; docs/design/flows.md §7): declared
//! read-only subcommands, one namespace per bundle, running with and without a run, who may write
//! outside a run, `flow_command` for a session that serves no task, `help`, and the data lock.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::bundle::{BundleMeta, Commands};
use harnix_flow::runner::{
    command_tool_spec, tool_specs, ByKind, CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig, ScriptedHost, TOOL_COMMAND,
};
use harnix_flow::*;
use serde_json::{json, Value};

const T0: u64 = 1_794_787_200_000;

/// A bundle `dev`: one flow `work` with a station `write`, and the command `note`, some of whose
/// subcommands are declared read-only.
fn write_bundle(dir: &Path, bundle_yaml: &str) {
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", bundle_yaml),
        (
            "work.yaml",
            "name: work\nstations:\n  - id: write\n    executor: {kind: session}\n    skill: write\n    output: {type: object}\n    exit: [{kind: output}]\n",
        ),
        ("skills/write/SKILL.md", "---\nname: write\n---\n写。\n"),
        (
            "commands/note.js",
            r#"// note <add|list|get> ...
//
// note add <text>   append a note (writes)
// note list         show them (read-only)
export default function (input, flow) {
  const [sub, ...rest] = input.args;
  flow.print("RAN " + JSON.stringify({ by: input.by, run: input.run, out: input.output_dir, data: input.data_dir }));
  if (sub === "add") { flow.append.jsonl(`${input.data_dir}/notes.jsonl`, { text: rest.join(" "), by: input.by.kind }); return "added"; }
  if (sub === "slow") { flow.append.jsonl(`${input.data_dir}/order.jsonl`, { at: "start", who: rest[0] }); const t = Date.now(); while (Date.now() - t < 300) {} flow.append.jsonl(`${input.data_dir}/order.jsonl`, { at: "end", who: rest[0] }); return "done"; }
  if (sub === "list") { if (rest[0] === "sneak") flow.write.text(`${input.data_dir}/sneak.txt`, "x"); return flow.read.jsonl(`${input.data_dir}/notes.jsonl`) || []; }
  return "?";
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

const DECLARED: &str = "name: dev\ncommands:\n  note: {read_only: [list, get], description: 记笔记}\n";

fn runner(packs_dir: &Path, data: &Path) -> (Arc<Runner>, Arc<Engine>) {
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
    (Runner::new(engine.clone(), ScriptedHost::new(), cfg), engine)
}

fn call(by: CommandBy, args: &[&str], root: &Path) -> CommandCall {
    let mut c = CommandCall::new("dev", "note", args.iter().map(|a| a.to_string()).collect(), by);
    c.project_root = Some(root.to_path_buf());
    c
}

#[test]
fn commands_are_a_list_or_a_map_and_read_only_matches_the_first_argument() {
    let old: BundleMeta = serde_yaml::from_str("name: x\ncommands: [xspec, xledger]\n").unwrap();
    assert_eq!(old.commands.names().collect::<Vec<_>>(), ["xspec", "xledger"]);
    assert!(!harnix_flow::bundle::is_read_only(old.commands.get("xspec"), &["list".into()]), "a list declares nothing read-only");
    let new: BundleMeta = serde_yaml::from_str(
        "name: x\ncommands:\n  xspec: {read_only: [list, get], description: 规格}\n  xledger:\n",
    )
    .unwrap();
    assert_eq!(new.commands.names().collect::<Vec<_>>(), ["xspec", "xledger"], "the file's order");
    let d = new.commands.get("xspec");
    assert_eq!(d.unwrap().description, "规格");
    let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    assert!(harnix_flow::bundle::is_read_only(d, &args(&["list", "--all"])));
    assert!(!harnix_flow::bundle::is_read_only(d, &args(&["set", "list"])), "only the first argument counts");
    assert!(harnix_flow::bundle::is_read_only(d, &args(&[])) && harnix_flow::bundle::is_read_only(None, &args(&["help"])), "help reads");
    assert!(!harnix_flow::bundle::is_read_only(new.commands.get("xledger"), &args(&["list"])));
    // Written again: the list stays a list, the map a map.
    let back: BundleMeta = serde_yaml::from_str(&serde_yaml::to_string(&new).unwrap()).unwrap();
    assert_eq!(back, new);
    assert!(serde_yaml::to_string(&old).unwrap().contains("- xspec"));
    assert_eq!(Commands::default().0.len(), 0);
    assert!(serde_yaml::from_str::<BundleMeta>("name: x\ncommands: {a: {read_only: [b], nope: 1}}\n").is_err());
}

#[test]
fn a_bundles_flows_commands_and_stations_are_one_namespace() {
    let d = tempfile::tempdir().unwrap();
    // A command named like the station `write`, another like the flow `work`, one reserved; a station
    // `write` in a second flow is the same station and may repeat.
    write_bundle(d.path(), "name: dev\ncommands: [write, work, status, note]\n");
    std::fs::write(d.path().join("dev/other.yaml"), "name: other\nstations: [{id: write, exit: [{kind: confirm}]}]\n").unwrap();
    let packs = DirPacks::new([d.path()]);
    let (def, _) = packs.flow("dev/work").unwrap();
    let msgs: Vec<String> = validate(&def, &packs).into_iter().map(|p| p.message).collect();
    let said = |needle: &str| msgs.iter().any(|m| m.contains(needle));
    assert!(said("\"write\" is both a command and a station"), "{msgs:?}");
    assert!(said("\"work\" is both a flow and a command"), "{msgs:?}");
    assert!(said("`status` is reserved"), "{msgs:?}");
    assert!(!said("\"note\""), "{msgs:?}");
    assert!(msgs.iter().all(|m| m.starts_with("bundle dev: ")), "{msgs:?}");
    // A bundle that keeps its names apart is clean, and the repeated station id is no collision.
    write_bundle(d.path(), "name: dev\ncommands: [note]\n");
    let packs = DirPacks::new([d.path()]);
    let (def, _) = packs.flow("dev/work").unwrap();
    assert_eq!(validate(&def, &packs), vec![]);
}

#[tokio::test]
async fn outside_a_run_a_person_may_write_and_a_model_only_read() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), DECLARED);
    let (rn, _) = runner(packs_dir.path(), data.path());
    let root = project.path().canonicalize().unwrap();

    assert!(!rn.is_read_only("dev", "note", &["add".into(), "x".into()], Some(&root)));
    assert!(rn.is_read_only("dev", "note", &["list".into()], Some(&root)));
    assert!(!rn.is_read_only("dev", "nope", &["list".into()], Some(&root)) && !rn.is_read_only("nobundle", "note", &["list".into()], None));

    // A person writes: into the bundle's data directory, with no run and no output directory.
    let r = rn.run_command(call(CommandBy::person("me"), &["add", "一条"], &root)).await.unwrap();
    assert!(r.text.contains("\"kind\":\"person\"") && r.text.contains("\"run\":null") && r.text.contains("\"out\":null") && r.text.contains("\"data\":\"conix/dev\""), "{}", r.text);
    assert!(r.text.ends_with("added") && r.written == ["conix/dev/notes.jsonl"], "{r:?}");
    let notes = std::fs::read_to_string(root.join("conix/dev/notes.jsonl")).unwrap();
    assert_eq!(notes, "{\"text\":\"一条\",\"by\":\"person\"}\n");

    // A model is refused what writes, told what it may do; what reads it may, and sees `by`.
    let refused = rn.run_command(call(CommandBy::model("s1"), &["add", "x"], &root)).await.unwrap_err();
    assert!(refused.contains("inside a flow run") && refused.contains("ask the person") && refused.contains("`list`"), "{refused}");
    assert_eq!(std::fs::read_to_string(root.join("conix/dev/notes.jsonl")).unwrap(), notes, "nothing was written");
    let listed = rn.run_command(call(CommandBy::model("s1"), &["list"], &root)).await.unwrap();
    assert!(listed.text.contains("\"kind\":\"model\"") && listed.text.contains("\"id\":\"s1\"") && listed.text.contains("一条"), "{}", listed.text);

    // Read-only is enforced, not trusted: a `list` that tries to write throws, whoever runs it.
    for by in [CommandBy::model("s1"), CommandBy::person("me")] {
        let e = rn.run_command(call(by, &["list", "sneak"], &root)).await.unwrap_err();
        assert!(e.contains("declared read-only"), "{e}");
    }
    assert!(!root.join("conix/dev/sneak.txt").exists());

    // Another bundle's name, an undeclared command, no project: told why.
    let mut c = call(CommandBy::person("me"), &["list"], &root);
    c.command = "nope".into();
    assert!(rn.run_command(c).await.unwrap_err().contains("no command `nope` in `dev`; it has `note`"));
    let mut c = call(CommandBy::person("me"), &["list"], &root);
    c.bundle = "ghost".into();
    assert!(rn.run_command(c).await.unwrap_err().contains("no bundle `ghost`"));
    let mut c = call(CommandBy::person("me"), &["list"], &root);
    c.project_root = None;
    assert!(rn.run_command(c).await.unwrap_err().contains("no run and no project"));
}

#[tokio::test]
async fn help_answers_from_the_files_comment_without_running_it() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), DECLARED);
    let (rn, _) = runner(packs_dir.path(), data.path());
    let root = project.path().canonicalize().unwrap();
    for args in [&["help"][..], &[][..]] {
        let r = rn.run_command(call(CommandBy::model("s"), args, &root)).await.unwrap();
        assert!(!r.text.contains("RAN"), "the command ran: {}", r.text);
        for w in ["`note` — 记笔记", "note <add|list|get> ...", "note add <text>   append a note (writes)", "Read-only: `list`, `get`"] {
            assert!(r.text.contains(w), "{w}: {}", r.text);
        }
        assert!(r.written.is_empty());
    }
    assert_eq!(harnix_flow::bundle::command_help("\n// a\n//   b\n\nexport default 1;\n// c").as_deref(), Some("a\n  b"));
    assert_eq!(harnix_flow::bundle::command_help("export default 1;\n// c"), None);
    // The tool and the protocol say so.
    assert!(command_tool_spec().description.contains("help"));
    assert_eq!(command_tool_spec().name, TOOL_COMMAND);
    assert_eq!(tool_specs().iter().filter(|t| t.name == TOOL_COMMAND).count(), 1);
}

#[tokio::test]
async fn flow_command_serves_the_main_conversation_by_bundle_and_command() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), DECLARED);
    let (rn, _) = runner(packs_dir.path(), data.path());
    let root = project.path().canonicalize().unwrap();
    let s = "chat-1".to_string();
    let ask = |input: Value| rn.tools().call_in(&s, TOOL_COMMAND, input, Some(&root));

    let r = ask(json!({"command": "dev:note", "args": ["list"]})).await;
    assert!(!r.is_error && r.text.contains("\"kind\":\"model\""), "{}", r.text);
    let r = ask(json!({"command": "dev:note", "args": ["add", "x"]})).await;
    assert!(r.is_error && r.text.contains("ask the person"), "{}", r.text);
    let r = ask(json!({"command": "note", "args": ["list"]})).await;
    assert!(r.is_error && r.text.contains("`bundle:command`"), "{}", r.text);
    let r = ask(json!({"command": "dev:note", "args": ["list"], "run": "no-such-run-1"})).await;
    assert!(r.is_error, "{}", r.text);
    // The other flow tools are a task's own: this session has none.
    let r = rn.tools().call_in(&s, "flow_state", json!({}), Some(&root)).await;
    assert!(r.is_error && r.text.contains("not working on a flow task"));
}

#[tokio::test]
async fn with_a_run_the_pinned_source_runs_and_the_run_must_be_the_bundles() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), DECLARED);
    std::fs::create_dir_all(packs_dir.path().join("other")).unwrap();
    std::fs::write(packs_dir.path().join("other/bundle.yaml"), "name: other\n").unwrap();
    std::fs::write(packs_dir.path().join("other/flow.yaml"), "name: other\nstations: [{id: a, exit: [{kind: confirm}]}]\n").unwrap();
    let (rn, engine) = runner(packs_dir.path(), data.path());
    let root = project.path().canonicalize().unwrap();
    let start = |flow: &str| {
        let input = CreateIn { flow: flow.into(), subject: "s".into(), project_root: Some(root.to_string_lossy().into()), ..Default::default() };
        let rn = rn.clone();
        async move {
            let o = rn.start_run(input, Actor::device("d")).await.unwrap();
            let run = o.value["run"].as_str().unwrap().to_string();
            rn.apply(o).await;
            run
        }
    };
    let run = start("dev/work").await;
    let other = start("other").await;
    let rec = engine.record(&run).await.unwrap();
    assert!(rec.parts.command_decls["note"].read_only == ["list", "get"], "pinned with the declaration");

    // The source edited after the run started: the run keeps what it pinned.
    std::fs::write(packs_dir.path().join("dev/commands/note.js"), "export default function () { return 'edited'; }").unwrap();
    let mut c = CommandCall::new("dev", "note", vec!["list".into()], CommandBy::person("me"));
    c.run = Some(run.clone());
    let r = rn.run_command(c.clone()).await.unwrap();
    assert!(r.text.contains("RAN") && !r.text.contains("edited") && r.text.contains(&format!("\"run\":\"{run}\"")), "{}", r.text);
    assert!(r.text.contains("\"out\":\"conix/runs/"), "{}", r.text);
    // Without a run it is the current source.
    let mut now = c.clone();
    now.run = None;
    now.project_root = Some(root.clone());
    assert!(rn.run_command(now).await.unwrap().text.contains("edited"));

    // A model may write inside a run (the run records it); the declaration still binds it.
    c.by = CommandBy::model("s");
    c.args = vec!["add".into(), "in a run".into()];
    assert!(rn.run_command(c.clone()).await.unwrap().text.ends_with("added"));
    c.args = vec!["list".into(), "sneak".into()];
    assert!(rn.run_command(c.clone()).await.unwrap_err().contains("declared read-only"));
    // The run is the bundle's, or the call is refused.
    c.run = Some(other);
    assert!(rn.run_command(c.clone()).await.unwrap_err().contains("of the bundle `other`, not `dev`"));
    c.run = Some("nope-1".into());
    assert!(rn.run_command(c).await.is_err());
    assert_eq!(ByKind::Station, CommandBy::station("x").kind);
}

#[tokio::test]
async fn writes_to_one_data_directory_do_not_interleave_across_runs() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), DECLARED);
    let (rn, _) = runner(packs_dir.path(), data.path());
    let root = project.path().canonicalize().unwrap();
    // `slow add`s a start line, spins, then an end line: with no lock a second writer lands between.
    // The command has no `slow` in `read_only`, so it is a writer and holds the lock.
    let slow = |who: &str| call(CommandBy::person("me"), &["slow", who], &root);
    let (a, b) = tokio::time::timeout(Duration::from_secs(20), async { tokio::join!(rn.run_command(slow("a")), rn.run_command(slow("b"))) }).await.unwrap();
    assert!(a.is_ok() && b.is_ok(), "{a:?} {b:?}");
    let lines: Vec<Value> = std::fs::read_to_string(root.join("conix/dev/order.jsonl")).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    let seq: Vec<String> = lines.iter().map(|l| format!("{}{}", l["at"].as_str().unwrap(), l["who"].as_str().unwrap())).collect();
    assert!(seq == ["starta", "enda", "startb", "endb"] || seq == ["startb", "endb", "starta", "enda"], "{seq:?}");
}
