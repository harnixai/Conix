//! A bundle's JavaScript (in Conix; docs/design/flows.md §2): scripts and commands import the bundle's
//! own `lib/` files, pinned with the run; `flow.git` and `flow.runs` answer from the run's workspace
//! and its project; the run records the commit it started from.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, Turn, TOOL_COMMAND, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

fn write_bundle(dir: &Path) {
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", "name: jsb\ncommands: [calc, gitq, bad, esc, runsq, utils]\n"),
        (
            "work.yaml",
            "name: work\nstations:\n  - id: write\n    executor: {kind: session}\n    skill: write\n    output: {type: object}\n    exit: [{kind: output}, {kind: gate, gate: shape}]\n",
        ),
        ("gates/shape.yaml", "name: shape\nkind: script\nscript: shape\n"),
        ("skills/write/SKILL.md", "---\nname: write\n---\nwrite.\n"),
        (
            "scripts/shape.js",
            "import { answer } from \"../lib/util.js\";\nexport default function (input, flow) {\n  return { verdict: answer() === 1 && flow.git.base() ? \"pass\" : \"fail\", summary: `answer=${answer()} changed=${flow.git.changed().join(\",\")}` };\n}\n",
        ),
        ("lib/util.js", "import { base } from \"./inner.js\";\nexport function answer() { return 1; }\nexport function twice() { return base() * 2; }\n"),
        ("lib/inner.js", "export function base() { return 21; }\n"),
        ("commands/calc.js", "import { answer, twice } from \"../lib/util.js\";\nexport default function () { return { answer: answer(), twice: twice() }; }\n"),
        (
            "commands/gitq.js",
            "export default function (input, flow) {\n  const head = flow.git.head();\n  return { base: flow.git.base(), head, changed: flow.git.changed(), since_head: flow.git.changed({ since: head }) };\n}\n",
        ),
        ("commands/bad.js", "import fs from \"fs\";\nexport default function () { return \"x\"; }\n"),
        ("commands/esc.js", "import x from \"../../outside.js\";\nexport default function () { return \"x\"; }\n"),
        (
            "commands/runsq.js",
            "export default function (input, flow) {\n  return { active: flow.runs.list({ bundle: \"jsb\", active: true }).map((r) => r.id), none: flow.runs.list({ bundle: \"other\" }).length, stations: flow.runs.get(input.run).stations, missing: flow.runs.get(\"nope\") };\n}\n",
        ),
        (
            "commands/utils.js",
            "export default function (input, flow) {\n  return {\n    sha: flow.sha256(\"abc\"), same: flow.hash.sha256(\"abc\") === flow.sha256(\"abc\"),\n    join: flow.path.join(\"a\", \"../b\", \"c.txt\"), dir: flow.path.dirname(\"a/b/c.txt\"), base: flow.path.basename(\"a/b/c.txt\", \".txt\"), ext: flow.path.extname(\"x.tar.gz\"), rel: flow.path.relative(\"a/b\", \"a/c/d\"),\n    yaml: flow.yaml.parse(\"a: [1, 2]\"), text: flow.yaml.stringify({ k: [1] }), fm: flow.yaml.parseFrontmatter(\"---\\nt: 1\\n---\\nbody\\n\"),\n    glob: [flow.glob.test(\"src/**/*.md\", \"src/a/b.md\"), flow.glob.test(\"*.md\", \"a/b.md\")], filter: flow.glob.filter([\"*.js\", \"*.md\"], [\"a.js\", \"b.txt\", \"c.md\"]),\n  };\n}\n",
        ),
    ];
    for (p, t) in files {
        let f = dir.join("jsb").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
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
async fn a_bundles_scripts_and_commands_import_its_lib_pinned_and_reach_git_and_runs() {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path());
    let root = project.path().canonicalize().unwrap();
    sh(&root, "git init -q && git config user.email t@e && git config user.name t && echo one > a.txt && git add -A && git commit -qm base");
    let head = sh(&root, "git rev-parse HEAD");

    let host = ScriptedHost::new();
    host.main(vec![Turn::Runs]);
    let cfg = RunnerConfig {
        profiles: vec!["flow-harnix".into()],
        submit_profiles: ["flow-harnix".to_string()].into(),
        main_profile: Some("flow-harnix".into()),
        lease_ms: 60_000,
        tick_ms: 1000,
        ..Default::default()
    };
    let packs = CatalogPacks::over(&cfg, vec![], Arc::new(DirPacks::new([packs_dir.path()])));
    let layout = ProjectLayout::new(".conix", data.path().join("worktrees"), "conix/flow").with_artifacts_dir("conix");
    packs.set_layout(layout.clone());
    let engine = Arc::new(
        Engine::new(Arc::new(MemStore::new()), Arc::new(packs))
            .with_clock(Arc::new(ManualClock::new(1_794_787_200_000)))
            .with_lease_ms(60_000)
            .with_submit_scenes(cfg.profiles.clone())
            .with_main_scene("flow-harnix")
            .with_workspaces(data.path().join("workspaces"))
            .with_layout(layout),
    );
    let rn = Runner::new(engine.clone(), host.clone(), cfg);
    rn.start().await;
    let o = rn
        .start_run(CreateIn { flow: "jsb/work".into(), subject: "s".into(), project_root: Some(root.to_string_lossy().into()), ..Default::default() }, Actor::device("d"))
        .await
        .unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    rn.apply(o).await;

    // What the run started from, and what it pinned: the files its scripts and commands import, to the end.
    let rec = engine.record(&run).await.unwrap();
    assert_eq!(rec.base_commit.as_deref(), Some(head.as_str()));
    let libs: Vec<&str> = rec.parts.libs.keys().map(String::as_str).collect();
    assert_eq!(libs, ["lib/inner.js", "lib/util.js"]);
    let main = eventually("the main session opened", async || host.opened().into_iter().find(|s| s.main)).await;
    let sid = eventually("the session got its key", async || host.session_of(&main.key)).await;
    eventually("the task was sent", async || host.sent_to(&sid).into_iter().next()).await;
    let call = |cmd: &str| rn.tools().call(&sid, TOOL_COMMAND, json!({"command": cmd}));
    let ok = async |cmd: &str| -> Value {
        let r = call(cmd).await;
        assert!(!r.is_error, "{cmd}: {}", r.text);
        serde_json::from_str(&r.text).unwrap_or_else(|e| panic!("{cmd}: {e}: {}", r.text))
    };

    // A command imports through two levels, and the library edited now changes nothing in this run.
    assert_eq!(ok("calc").await, json!({"answer": 1, "twice": 42}));
    let bundle = packs_dir.path().join("jsb");
    std::fs::write(bundle.join("lib/util.js"), "export function answer() { return 2; }\nexport function twice() { return 0; }\n").unwrap();
    std::fs::write(bundle.join("lib/inner.js"), "export function base() { return 1; }\n").unwrap();
    assert_eq!(ok("calc").await, json!({"answer": 1, "twice": 42}), "pinned: an edit mid-run changes nothing");

    // An import that is not the bundle's own is said so.
    let r = call("bad").await;
    assert!(r.is_error && r.text.contains("only the bundle's own files"), "{}", r.text);
    let r = call("esc").await;
    assert!(r.is_error && r.text.contains("leaves the bundle"), "{}", r.text);

    // The pure functions, as `agix.*` has them.
    let u = ok("utils").await;
    assert_eq!(u["sha"], "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(u["same"], true);
    assert_eq!((u["join"].as_str(), u["dir"].as_str(), u["base"].as_str(), u["ext"].as_str(), u["rel"].as_str()), (Some("b/c.txt"), Some("a/b"), Some("c"), Some(".gz"), Some("../c/d")));
    assert_eq!(u["yaml"], json!({"a": [1, 2]}));
    assert_eq!(u["text"], "k:\n- 1\n");
    assert_eq!(u["fm"], json!({"data": {"t": 1}, "body": "body\n"}));
    assert_eq!(u["glob"], json!([true, false]));
    assert_eq!(u["filter"], json!(["a.js", "c.md"]));

    // `flow.git`: from the run's base; what was committed since, staged, changed and new; not the
    // artifacts directory and not `.conix/`.
    let g = ok("gitq").await;
    assert_eq!((g["base"].as_str(), g["head"].as_str()), (Some(head.as_str()), Some(head.as_str())));
    assert_eq!(g["changed"], json!([]), "the run's own directory is not the work: {g}");
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::create_dir_all(root.join(".conix/state")).unwrap();
    std::fs::write(root.join("src/new.txt"), "n").unwrap();
    std::fs::write(root.join(".conix/state/x"), "n").unwrap();
    std::fs::write(root.join("a.txt"), "two\n").unwrap();
    sh(&root, "git add a.txt && git commit -qm second && echo three > b.txt && git add b.txt && echo four >> a.txt");
    let g = ok("gitq").await;
    assert_eq!(g["changed"], json!(["a.txt", "b.txt", "src/new.txt"]), "{g}");
    assert_eq!(g["since_head"], json!(["a.txt", "b.txt", "src/new.txt"]), "{g}");
    assert_eq!(g["head"].as_str(), Some(sh(&root, "git rev-parse HEAD").as_str()));
    assert_eq!(g["base"].as_str(), Some(head.as_str()), "the base does not move with the branch");

    // `flow.runs`: the project's runs, active ones by bundle.
    let r = ok("runsq").await;
    assert_eq!(r["active"], json!([run]));
    assert_eq!(r["none"], 0);
    assert_eq!(r["stations"], json!({"write": "working"}));
    assert_eq!(r["missing"], Value::Null);

    // The gate imports the same pinned library (the file says 2 now), and reads git.
    let r = rn.tools().call(&sid, TOOL_SUBMIT, json!({"output": {}})).await;
    assert!(!r.is_error, "{}", r.text);
    let c = eventually("the gate judged", async || {
        let rec = engine.record(&run).await.unwrap();
        let c = rec.stations["write"].checks.iter().find(|c| c.kind == "gate")?.clone();
        (c.status != CheckStatus::Wait).then_some(c)
    })
    .await;
    assert_eq!(c.status, CheckStatus::Pass, "{c:?}");
    assert!(c.detail.starts_with("answer=1 changed=a.txt,b.txt,src/new.txt"), "{}", c.detail);
}

#[test]
fn an_import_in_a_flow_that_is_in_no_bundle_is_refused_and_a_script_with_one_is_saved() {
    let text = "import { x } from \"../lib/x.js\";\nexport default function () { return { verdict: \"pass\" }; }\n";
    let p = MemPacks::new();
    assert_eq!(part_problems(PartKind::Script, "plain", text, &p), Vec::<String>::new(), "syntax only: the loader says the rest when it runs");
}
