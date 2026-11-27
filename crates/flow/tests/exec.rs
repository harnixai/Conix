//! One way processes start (in Conix; docs/design/flows.md §7.14): `flow.exec` in a bundle's JS,
//! through the host's `Exec`, its profile fixed by where the script runs — a gate reads, a command
//! step that writes writes where its run may — never by the script; argv only, `cwd` held to the
//! workspace, output capped, time held to the script's, the bundle's `exec` declaring the network
//! and the environment, the host's switch and the project's trust deciding whether anything runs;
//! every process recorded.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use harnix_flow::runner::{run_plain, CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig, ScriptedHost};
use harnix_flow::*;
use serde_json::{json, Value};

const T0: u64 = 1_794_787_200_000;

/// Runs each request as a plain process, and keeps them.
#[derive(Default)]
struct Seen(Mutex<Vec<ExecRequest>>);

impl Seen {
    fn all(&self) -> Vec<ExecRequest> {
        self.0.lock().unwrap().clone()
    }
}

#[async_trait]
impl Exec for Seen {
    async fn run(&self, req: ExecRequest) -> std::result::Result<ExecOutcome, ExecError> {
        self.0.lock().unwrap().push(req.clone());
        run_plain(req, Enforcement::none("seen")).await
    }
}

const REPORT: &str = r#"// report <case>
export default function (input, flow) {
  const [sub] = input.args;
  const ex = (o) => { try { return flow.exec(o); } catch (e) { return { threw: String(e.message), code: e.code }; } };
  if (sub === "show" || sub === "write") {
    return ex({ argv: ["sh", "-c", 'printf "%s|%s|%s" "$RX_TOKEN" "$RX_API_KEY" "$1"', "sh", "a;b $(touch pwned)"] });
  }
  if (sub === "escape") return ex({ argv: ["true"], cwd: "../" });
  if (sub === "big") { const r = ex({ argv: ["sh", "-c", "head -c 5000000 /dev/zero | tr '\\0' a"] }); return { len: r.stdout.length, truncated: r.truncated }; }
  if (sub === "slow") return ex({ argv: ["sleep", "5"], timeout_seconds: 0.5 });
  if (sub === "long") return ex({ argv: ["true"], timeout_seconds: 100000 });
  if (sub === "shell") return ex({ argv: "echo hi" });
  return "?";
}
"#;

const PROBE: &str = r#"export default function (input, flow) {
  const r = flow.exec({ argv: ["printf", "%s", "a;b $(x)"] });
  return { verdict: r.stdout === "a;b $(x)" && r.code === 0 ? "pass" : "fail", summary: JSON.stringify(r) };
}
"#;

/// A bundle `rx`: a flow whose first station's script gate starts a process, whose second is a
/// command step that writes (and may write `src/gen/`).
fn write_bundle(dir: &Path, exec: &str) {
    let bundle = format!("name: rx\ncommands:\n  report: {{read_only: [show, escape, big, slow, long, shell]}}\n{exec}");
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", &bundle),
        (
            "work.yaml",
            r#"name: work
stations:
  - id: look
    exit: [{kind: gate, gate: probe}]
  - id: make
    executor: {kind: command, command: report, args: [write]}
    writes: ["src/gen/**/*.rs", "notes.md"]
    output: {type: object}
    exit: [{kind: output}]
"#,
        ),
        ("gates/probe.yaml", "name: probe\nkind: script\nscript: probe\n"),
        ("scripts/probe.js", PROBE),
        ("commands/report.js", REPORT),
    ];
    for (p, t) in files {
        let f = dir.join("rx").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
}

struct Setup {
    rn: Arc<Runner>,
    engine: Arc<Engine>,
    seen: Arc<Seen>,
    root: PathBuf,
    _dirs: [tempfile::TempDir; 3],
}

fn setup(exec: &str, commands: bool, trusted: bool) -> Setup {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), exec);
    let root = project.path().canonicalize().unwrap();
    let seen = Arc::new(Seen::default());
    let cfg = RunnerConfig {
        profiles: vec!["flow-harnix".into()],
        submit_profiles: ["flow-harnix".to_string()].into(),
        main_profile: Some("flow-harnix".into()),
        lease_ms: 60_000,
        tick_ms: 1000,
        command_gates: commands,
        env_deny: vec!["*_API_KEY".into()],
        exec: Some(seen.clone()),
        ..Default::default()
    };
    cfg.trust.set(&root.to_string_lossy(), trusted);
    let packs = CatalogPacks::over(&cfg, vec![], Arc::new(DirPacks::new([packs_dir.path()])));
    let layout = ProjectLayout::new(".conix", data.path().join("worktrees"), "conix/flow").with_artifacts_dir("conix");
    packs.set_layout(layout.clone());
    let engine = Arc::new(
        Engine::new(Arc::new(MemStore::new()), Arc::new(packs))
            .with_clock(Arc::new(ManualClock::new(T0)))
            .with_lease_ms(60_000)
            .with_submit_scenes(cfg.profiles.clone())
            .with_main_scene("flow-harnix")
            .with_workspaces(data.path().join("workspaces"))
            .with_layout(layout),
    );
    let host = ScriptedHost::new();
    host.trust(&root, trusted);
    let rn = Runner::new(engine.clone(), host, cfg);
    Setup { rn, engine, seen, root, _dirs: [packs_dir, data, project] }
}

impl Setup {
    /// `report <sub>` for a person, outside any run: what it returned.
    async fn report(&self, sub: &str) -> Value {
        let mut c = CommandCall::new("rx", "report", vec![sub.to_string()], CommandBy::person("me"));
        c.project_root = Some(self.root.clone());
        let r = self.rn.run_command(c).await.unwrap();
        serde_json::from_str(&r.text).unwrap_or(Value::String(r.text))
    }
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

#[test]
fn a_bundles_exec_is_validated() {
    let ok: bundle::BundleMeta = serde_yaml::from_str("name: x\nexec: {network: true, env: [CI_TOKEN, CI_*]}\n").unwrap();
    assert_eq!(ok.exec.as_ref().unwrap().problems(), Vec::<String>::new());
    let bad: bundle::BundleMeta = serde_yaml::from_str("name: x\nexec: {env: ['', '1A', 'A-B', '*']}\n").unwrap();
    assert_eq!(bad.exec.unwrap().problems().len(), 4);
    assert!(serde_yaml::from_str::<bundle::BundleMeta>("name: x\nexec: {shell: true}\n").is_err(), "an unknown field is refused");
}

/// In a run: the gate's script starts its process reading, the command step writing where the run
/// may; both through the host's backend, the network and the variables as the bundle declared,
/// the denied one never; both recorded in the run, and no shell between.
#[tokio::test]
async fn in_a_run_the_profile_is_the_callers_and_each_process_is_recorded() {
    std::env::set_var("RX_TOKEN", "tok");
    std::env::set_var("RX_API_KEY", "secret");
    let s = setup("exec: {network: true, env: [RX_*]}\n", true, true);
    let o = s
        .rn
        .start_run(CreateIn { flow: "rx/work".into(), subject: "s".into(), project_root: Some(s.root.to_string_lossy().into()), ..Default::default() }, Actor::device("d"))
        .await
        .unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    s.rn.apply(o).await;
    let rec = eventually("the run finished", async || {
        let rec = s.engine.record(&run).await.unwrap();
        (rec.status == RunStatus::Finished).then_some(rec)
    })
    .await;
    let out = rec.stations["make"].tasks[0].output.clone().unwrap();
    assert_eq!(out["stdout"], "tok||a;b $(touch pwned)", "{out}");
    assert!(!s.root.join("pwned").exists(), "an argument is never a command");
    assert_eq!((out["code"].as_i64(), out["enforced"]["backend"].as_str()), (Some(0), Some("seen")));

    let seen = s.seen.all();
    assert_eq!(seen.len(), 2, "{seen:?}");
    assert_eq!((&seen[0].profile, seen[0].network), (&ExecProfile::Read, true), "a gate's script reads");
    assert_eq!(seen[0].argv, ["printf", "%s", "a;b $(x)"]);
    let ExecProfile::Workspace { writable } = &seen[1].profile else { panic!("a step that writes: {:?}", seen[1].profile) };
    let canon = |p: &str| std::fs::canonicalize(p).unwrap_or_else(|_| PathBuf::from(p));
    assert!(writable.contains(&canon(rec.output_dir.as_deref().unwrap())), "{writable:?}");
    assert!(writable.contains(&s.root.join("conix/rx")), "the bundle's data: {writable:?}");
    assert!(writable.contains(&s.root.join("src/gen")) && writable.contains(&s.root.join("notes.md")), "the station's writes: {writable:?}");
    assert!(!writable.contains(&s.root), "not the whole project: {writable:?}");
    let names: Vec<String> = seen[1].env.iter().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
    assert!(names.contains(&"RX_TOKEN".to_string()) && !names.contains(&"RX_API_KEY".to_string()), "{names:?}");
    assert!(names.iter().all(|n| !n.starts_with("FLOW_")), "a script's process is not a gate's: {names:?}");

    let history = s.engine.history(&run).await.unwrap();
    let lines: Vec<&History> = history.iter().filter(|h| h.op == "exec").collect();
    assert_eq!(lines.len(), 2);
    assert_eq!((lines[0].station.as_deref(), lines[0].detail["for"].as_str()), (Some("look"), Some("flow.exec in gate rx/probe")));
    assert_eq!(lines[0].detail["profile"], json!({"kind": "read"}));
    assert_eq!(lines[1].detail["profile"]["kind"], "workspace");
    assert_eq!((lines[1].station.as_deref(), lines[1].task.as_deref()), (Some("make"), Some("make")));
    for l in &lines {
        assert_eq!((l.detail["network"].as_bool(), l.detail["code"].as_i64()), (Some(true), Some(0)));
        assert_eq!(l.detail["enforced"], json!({"backend": "seen", "filesystem": false, "network": false}));
        assert!(l.detail.get("stdout").is_none() && !l.detail.to_string().contains("tok"), "no output in the history: {}", l.detail);
    }
}

/// Outside a run: read-only whoever asks, recorded in the log, not in a run; `cwd` held inside the
/// project, the output cut at 4 MiB, the time held to the script's, argv only.
#[tokio::test]
async fn outside_a_run_it_reads_and_every_call_is_checked() {
    let s = setup("", true, true);
    let r = s.report("show").await;
    assert_eq!((r["code"].as_i64(), r["stdout"].as_str()), (Some(0), Some("||a;b $(touch pwned)")), "{r}");
    let seen = s.seen.all();
    assert_eq!((&seen[0].profile, seen[0].network), (&ExecProfile::Read, false), "no network undeclared");
    assert_eq!(std::fs::canonicalize(&seen[0].cwd).unwrap(), s.root);

    let r = s.report("escape").await;
    assert!(r["threw"].as_str().unwrap().contains("leaves"), "{r}");
    assert_eq!(r["code"], "exec");
    let r = s.report("big").await;
    assert_eq!((r["len"].as_u64(), r["truncated"].as_bool()), (Some(4 * 1024 * 1024), Some(true)), "{r}");
    let r = s.report("slow").await;
    assert_eq!((r["timed_out"].as_bool(), r["code"].as_i64()), (Some(true), None), "{r}");
    let before = s.seen.all().len();
    s.report("long").await;
    let long = &s.seen.all()[before];
    assert!(long.timeout <= Duration::from_secs(30), "no longer than the command has left: {:?}", long.timeout);
    let r = s.report("shell").await;
    assert!(r["threw"].as_str().unwrap().contains("argv"), "a command line is not taken: {r}");
    assert_eq!(s.seen.all().len(), before + 1, "nothing started for a call that was wrong");
}

/// The host's switch off, or the project not trusted: nothing starts, and the script is told why.
#[tokio::test]
async fn nothing_starts_where_the_switch_is_off_or_the_project_untrusted() {
    for (commands, trusted, why) in [(false, true, "not turned on"), (true, false, "not trusted")] {
        let s = setup("", commands, trusted);
        let r = s.report("show").await;
        assert!(r["threw"].as_str().unwrap().contains(why), "{why}: {r}");
        assert!(s.seen.all().is_empty());
    }
}

/// A doctor's script may start a process the host offers, reading, in the project.
#[tokio::test]
async fn a_doctor_reads_through_the_exec_its_host_gives() {
    let d = tempfile::tempdir().unwrap();
    let b = d.path().join("rx");
    std::fs::create_dir_all(b.join("scripts")).unwrap();
    std::fs::write(b.join("bundle.yaml"), "name: rx\ndoctor: doctor\n").unwrap();
    std::fs::write(
        b.join("scripts/doctor.js"),
        "export default function (input, flow) { const r = flow.exec({argv: ['pwd', '-P']}); return {findings: [{level: 'info', message: r.stdout.trim()}]}; }",
    )
    .unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let packs = DirPacks::new([d.path()]);
    let seen = Arc::new(Seen::default());
    let exec = DoctorExec { exec: seen.clone(), refused: None, env_deny: vec![] };
    let r = doctor_with(&packs, None, "rx", &root, Some(&exec)).await.unwrap();
    assert_eq!(r.findings[0].message, root.to_string_lossy(), "{r:?}");
    assert_eq!(seen.all()[0].profile, ExecProfile::Read);
    // Without one, or refused: the script is told, as a finding.
    let r = doctor(&packs, None, "rx", &root).await.unwrap();
    assert!(r.findings[0].message.contains("flow.exec: not offered"), "{r:?}");
    let refused = DoctorExec { refused: Some("the project is not trusted".into()), ..exec };
    let r = doctor_with(&packs, None, "rx", &root, Some(&refused)).await.unwrap();
    assert!(r.findings[0].message.contains("not trusted"), "{r:?}");
}
