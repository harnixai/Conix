//! The xforge bundle Conix ships (`examples/xforge/.conix/flows/xforge`, in Conix): both its flows and
//! every part hold together and dry-run to a person's confirmation; each gate passes a good delivery
//! and fails a bad one; its commands (`xspec`, `xledger`, `xpkg`) keep the change's structured data in
//! a run of it, and `xspec archive` merges the delta into the specs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use harnix_flow::author::*;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, SessionId, Turn, TOOL_COMMAND};
use harnix_flow::*;
use serde_json::{json, Value};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

/// The packs as Conix offers the bundle: a built-in directory of bundles.
fn packs() -> DirPacks {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    DirPacks::new([example().join(".conix/flows")]).with_catalog(catalog)
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// A copy of the example as a git repository, committed: where a person would start a change.
fn copy_of_example() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), &format!("cp -R {}/. . && git init -q && git add -A && git -c user.email=x@y -c user.name=x commit -qm base", example().display()));
    d
}

const GATES: &[&str] = &[
    "proposal-sections", "spec-delta-shape", "spec-delta", "design-sections", "constitution", "packages-planned",
    "packages-planned-quick", "ledgers-settled", "packages-done", "unclaimed", "unit-tests", "verification", "archived",
];

#[test]
fn both_paths_and_every_part_of_the_bundle_hold_together() {
    let p = packs();
    assert_eq!(
        p.flows().into_iter().filter(|f| f.starts_with("xforge/")).collect::<Vec<_>>(),
        ["xforge/quick", "xforge/solid"]
    );
    for f in ["xforge/solid", "xforge/quick"] {
        let (def, _) = p.flow(f).unwrap_or_else(|| panic!("no flow {f}"));
        assert_eq!(validate(&def, &p), Vec::<Problem>::new(), "{f}");
        assert_eq!(def.bundle.as_deref(), Some("xforge"));
    }
    for g in GATES {
        let name = format!("xforge/{g}");
        let (text, _) = p.part(PartKind::Gate, &name).unwrap_or_else(|| panic!("no gate {g}"));
        assert_eq!(part_problems(PartKind::Gate, &name, &text, &p), Vec::<String>::new(), "{g}");
    }
    let meta: harnix_flow::bundle::BundleMeta = serde_yaml::from_str(&p.bundle_file("xforge", "bundle.yaml").unwrap()).unwrap();
    for c in meta.commands.names() {
        let src = p.bundle_file("xforge", &format!("commands/{c}.js")).unwrap_or_else(|| panic!("no command {c}"));
        assert!(src.contains("export default function"), "{c}");
    }
}

#[derive(Default)]
struct Convo {
    projects: Mutex<HashMap<String, String>>,
}

#[async_trait]
impl AuthorHost for Convo {
    async fn current(&self, _: &SessionId) -> Option<String> {
        None
    }
    async fn project(&self, session: &SessionId) -> Option<String> {
        self.projects.lock().unwrap().get(session).cloned()
    }
}

struct Bench {
    tools: AuthorTools,
    _rn: Arc<Runner>,
    project: tempfile::TempDir,
}

fn bench() -> Bench {
    let cfg = RunnerConfig {
        command_gates: true,
        profiles: vec!["code".into()],
        main_profile: Some("code".into()),
        submit_profiles: ["code".to_string()].into(),
        ..Default::default()
    };
    let layout = ProjectLayout::new(".conix", std::env::temp_dir().join("xforge-worktrees"), "conix/flow").with_artifacts_dir("xforge");
    let packs = Arc::new(CatalogPacks::over(&cfg, vec![], Arc::new(packs())));
    packs.set_layout(layout.clone());
    let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), packs.clone()).with_main_scene("code").with_layout(layout));
    let scripted = ScriptedHost::new();
    let project = copy_of_example();
    scripted.trust(project.path(), true);
    let rn = Runner::new(engine, scripted, cfg);
    let sandbox = Arc::new(Sandbox::new(packs, SandboxConfig { guarded: vec![], submit_scenes: vec!["code".into()], main_scene: Some("code".into()) }));
    let host = Arc::new(Convo::default());
    host.projects.lock().unwrap().insert("s".into(), project.path().to_string_lossy().into_owned());
    Bench { tools: AuthorTools::new(&rn, sandbox, host), _rn: rn, project }
}

impl Bench {
    async fn ok(&self, tool: &str, input: Value) -> Value {
        let r = self.tools.call(&"s".to_string(), tool, input, &Actor::new(CONVERSATION_KIND, "s")).await;
        assert!(!r.is_error, "{tool}: {}", r.text);
        serde_json::from_str(&r.text).unwrap_or(Value::String(r.text))
    }
    /// The gate's verdict on a sample, the run's directory `out` and the bundle's data `data`.
    async fn verdict(&self, gate: &str, files: Value, outputs: Value) -> Value {
        let r = self
            .ok(TOOL_GATE_TRY, json!({"gate": format!("xforge/{gate}"), "files": files, "output_dir": "out", "data_dir": "data", "outputs": outputs}))
            .await;
        assert_eq!(r["tried"], true, "{gate}: {r}");
        r
    }
}

fn spec() -> String {
    std::fs::read_to_string(example().join("xforge/specs/textkit.md")).unwrap()
}

const DELTA: &str = "added:\n  - {id: TK-R3, title: 截断, text: \"truncate(text, n) 超过 n 个字符时截到 n-1 个再加 …\"}\nmodified:\n  - {id: TK-R2, text: \"word_count(text) 返回以空白分隔的非空片段个数；None 视为空文本，返回 0。\"}\n";

#[tokio::test]
async fn the_solid_path_dry_runs_to_the_reviewer_and_the_quick_one_to_the_owner() {
    let b = bench();
    let pkgs = json!([{"id": "truncate", "title": "加 truncate"}]);
    let dry = b
        .ok(TOOL_DRY_RUN, json!({"flow": "xforge/solid", "inputs": {"title": "truncate"}, "mocks": {"stations": {
            "propose": {"summary": "s"}, "design": {"summary": "s", "packages": pkgs}, "check": {"summary": "s", "ready": true}}}}))
        .await;
    assert_eq!(dry["stations"]["check"], "working", "{dry}");
    let dry = b
        .ok(TOOL_DRY_RUN, json!({"flow": "xforge/quick", "inputs": {"title": "t"}, "mocks": {"stations": {
            "propose": {"summary": "s", "packages": pkgs}, "apply": {"summary": "s"}, "verify": {"summary": "s"}}}}))
        .await;
    assert_eq!(dry["stations"]["verify"], "working", "{dry}");
}

#[tokio::test]
async fn the_declared_gates_judge_documents_ledgers_and_packages() {
    let b = bench();
    let good = "# 加 truncate\n\n## 为什么\n标题太长。\n\n## 改什么\n新增 truncate。\n\n## 不改什么\nslugify 不变。\n";
    assert_eq!(b.verdict("proposal-sections", json!({"out/proposal.md": good}), json!({})).await["verdict"], "pass");
    assert_eq!(b.verdict("proposal-sections", json!({"out/proposal.md": "## 为什么\nTODO\n"}), json!({})).await["verdict"], "fail");
    assert_eq!(b.verdict("spec-delta-shape", json!({"out/spec-delta.yaml": DELTA}), json!({})).await["verdict"], "pass");
    assert_eq!(b.verdict("spec-delta-shape", json!({"out/spec-delta.yaml": "added: []\n"}), json!({})).await["verdict"], "fail");
    let ledger = "entries:\n  - {id: D1, text: 用省略号截断, conclusion: accepted}\n";
    assert_eq!(b.verdict("ledgers-settled", json!({"out/ledgers/decisions.yaml": ledger}), json!({})).await["verdict"], "pass");
    let open = json!({"out/ledgers/findings.yaml": "entries:\n  - {id: F1, text: 没测空串, conclusion: open}\n"});
    assert_eq!(b.verdict("ledgers-settled", open, json!({})).await["verdict"], "fail");
    let plan = "packages:\n  - {id: truncate, title: t, paths: [src/textkit/**, tests/**], status: done}\n";
    assert_eq!(b.verdict("packages-done", json!({"out/work-packages.yaml": plan}), json!({})).await["verdict"], "pass");
    let r = b.verdict("packages-done", json!({"out/work-packages.yaml": plan.replace("done", "planned")}), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    let handed = json!({"design": {"summary": "s", "packages": [{"id": "truncate", "title": "t"}]}});
    assert_eq!(b.verdict("packages-planned", json!({"out/work-packages.yaml": plan}), handed.clone()).await["verdict"], "pass");
    let other = json!({"design": {"summary": "s", "packages": [{"id": "other", "title": "t"}]}});
    let r = b.verdict("packages-planned", json!({"out/work-packages.yaml": plan}), other).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("other") && r["findings"].to_string().contains("truncate"), "{r}");
}

#[tokio::test]
async fn the_script_gates_judge_against_the_specs_the_principles_and_the_tests() {
    let b = bench();
    let with_spec = |k: &str, v: &str| json!({"data/specs/textkit.md": spec(), k: v});
    assert_eq!(b.verdict("spec-delta", with_spec("out/spec-delta.yaml", DELTA), json!({})).await["verdict"], "pass");
    let bad = "added:\n  - {id: TK-R1, title: x, text: 已经有的编号}\nmodified:\n  - {id: TK-R9, text: 不存在的需求}\n";
    let r = b.verdict("spec-delta", with_spec("out/spec-delta.yaml", bad), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("TK-R9"), "{r}");

    let c = std::fs::read_to_string(example().join(".conix/governance/constitution.yaml")).unwrap();
    let design = "## 原则对照\n- P1：只用 str 的切片，不引入依赖。\n- P2：TK-R3 与 TK-R2 各加一条用例。\n- P3：slugify 与 word_count 的签名不变。\n- P4：一个工作包，只碰 src 与 tests。\n";
    let r = b.verdict("constitution", json!({".conix/governance/constitution.yaml": c, "out/design.md": design}), json!({})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    let r = b.verdict("constitution", json!({".conix/governance/constitution.yaml": c, "out/design.md": design.replace("- P3：slugify 与 word_count 的签名不变。\n", "")}), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    let r = b.verdict("constitution", json!({"out/design.md": design}), json!({})).await;
    assert_eq!(r["verdict"], "pass", "a project without principles is reminded, not held: {r}");
    assert!(r["findings"].to_string().contains("warn"), "{r}");

    let tests = "def test_truncate(self):  # TK-R3\n    pass\ndef test_none(self):  # TK-R2\n    pass\n";
    let record = "- TK-R3 通过：跑了单测\n- TK-R2 未验证：没有 None 的调用方\n";
    let r = b.verdict("verification", json!({"out/spec-delta.yaml": DELTA, "out/verification.md": record, "tests/test_textkit.py": tests}), json!({})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    let r = b.verdict("verification", json!({"out/spec-delta.yaml": DELTA, "out/verification.md": "- TK-R3 未通过\n", "tests/test_textkit.py": "x"}), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");

    let merged = spec().replace("`word_count(text)` 返回以空白分隔的非空片段个数；全是空白时为 0。", "word_count(text) 返回以空白分隔的非空片段个数；None 视为空文本，返回 0。")
        + "\n### TK-R3 截断\ntruncate(text, n) 超过 n 个字符时截到 n-1 个再加 …\n";
    assert_eq!(b.verdict("archived", json!({"out/spec-delta.yaml": DELTA, "data/specs/textkit.md": merged}), json!({})).await["verdict"], "pass");
    assert_eq!(b.verdict("archived", with_spec("out/spec-delta.yaml", DELTA), json!({})).await["verdict"], "fail");
}

#[tokio::test]
async fn the_command_gates_check_the_scope_with_sh_and_git_and_run_the_projects_verify() {
    let b = bench();
    let root = b.project.path();
    let out = root.join("xforge/runs/r1");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("packages.paths"), "src/textkit/**\ntests/**\n").unwrap();
    let at = |g: &str| json!({"gate": format!("xforge/{g}"), "output_dir": out.to_string_lossy()});
    std::fs::write(root.join("src/textkit/__init__.py"), std::fs::read_to_string(example().join("src/textkit/__init__.py")).unwrap() + "\n\ndef truncate(text, n):\n    return text if len(text) <= n else text[: n - 1] + \"…\"\n").unwrap();
    let r = b.ok(TOOL_GATE_TRY, at("unclaimed")).await;
    assert_eq!(r["verdict"], "pass", "the run's own directory does not count: {r}");
    assert_eq!(r["metrics"]["changed"], 1, "{r}");
    let r = b.ok(TOOL_GATE_TRY, at("unit-tests")).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    std::fs::write(root.join("README.md"), "changed").unwrap();
    let r = b.ok(TOOL_GATE_TRY, at("unclaimed")).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("README.md"), "{r}");
    std::fs::write(root.join("tests/test_broken.py"), "import unittest\nclass T(unittest.TestCase):\n    def test_x(self):\n        self.fail('boom')\n").unwrap();
    let r = b.ok(TOOL_GATE_TRY, at("unit-tests")).await;
    assert_eq!(r["verdict"], "fail", "{r}");
}

/// A run of `xforge/solid` in a copy of the example, its main session played by hand: the
/// commands the stations' sessions run.
#[tokio::test]
async fn the_commands_keep_the_changes_data_and_archive_merges_the_delta() {
    let project = copy_of_example();
    let root = project.path().canonicalize().unwrap();
    let data = tempfile::tempdir().unwrap();
    let cfg = RunnerConfig { command_gates: true, profiles: vec!["code".into()], submit_profiles: ["code".to_string()].into(), main_profile: Some("code".into()), ..Default::default() };
    let packs = CatalogPacks::over(&cfg, vec![], Arc::new(packs()));
    let layout = ProjectLayout::new(".conix", data.path().join("w"), "conix/flow").with_artifacts_dir("xforge");
    packs.set_layout(layout.clone());
    let engine = Arc::new(
        Engine::new(Arc::new(MemStore::new()), Arc::new(packs))
            .with_submit_scenes(cfg.profiles.clone())
            .with_main_scene("code")
            .with_workspaces(data.path().join("ws"))
            .with_layout(layout),
    );
    let host = ScriptedHost::new();
    host.main(vec![Turn::Runs]);
    // The example carries the bundle in its own `.conix/flows/`: there it is the project's.
    host.trust(&root, true);
    let rn = Runner::new(engine.clone(), host.clone(), cfg);
    rn.start().await;
    let o = rn
        .start_run(
            CreateIn { flow: "xforge/solid".into(), subject: "加 truncate".into(), inputs: json!({"title": "truncate"}), project_root: Some(root.to_string_lossy().into()), ..Default::default() },
            Actor::device("d"),
        )
        .await
        .unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    rn.apply(o).await;
    let rec = engine.record(&run).await.unwrap();
    assert_eq!(rec.data_dir.as_deref().map(Path::new), Some(root.join("xforge").as_path()), "the artifacts are named as the bundle: its data is them");
    let out = PathBuf::from(rec.output_dir.unwrap());
    let sid = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(m) = host.opened().into_iter().find(|s| s.main) {
                if let Some(id) = host.session_of(&m.key) {
                    if !host.sent_to(&id).is_empty() {
                        return id;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the proposal was sent");
    assert!(std::fs::read_to_string(out.join("proposal.md")).unwrap().contains("conix:template"));
    let run_cmd = async |cmd: &str, args: &[&str]| {
        let r = rn.tools().call(&sid, TOOL_COMMAND, json!({"command": cmd, "args": args})).await;
        (r.is_error, r.text)
    };

    // xspec: the delta, numbered after the specs; checked; merged.
    let (e, t) = run_cmd("xspec", &["list"]).await;
    assert!(!e && t.contains("textkit（TK）：2 条"), "{t}");
    let (e, t) = run_cmd("xspec", &["add", "textkit", "截断", "truncate(text, n) 超过 n 个字符时截到 n-1 个再加 …"]).await;
    assert!(!e && t.contains("TK-R3"), "{t}");
    let (e, t) = run_cmd("xspec", &["modify", "TK-R2", "word_count(text) 返回以空白分隔的非空片段个数；None 视为空文本，返回 0。"]).await;
    assert!(!e, "{t}");
    let (e, t) = run_cmd("xspec", &["modify", "TK-R9", "nothing"]).await;
    assert!(e && t.contains("没有需求 TK-R9"), "{t}");
    let (e, t) = run_cmd("xspec", &["check"]).await;
    assert!(!e && t.contains("对得上基线"), "{t}");
    let delta = std::fs::read_to_string(out.join("spec-delta.yaml")).unwrap();
    assert!(delta.contains("TK-R3") && delta.contains("TK-R2"), "{delta}");

    // xledger and xpkg: what the gates read, in the shapes they read.
    let (e, t) = run_cmd("xledger", &["add", "decisions", "用", "U+2026", "作省略号"]).await;
    assert!(!e && t.contains("D1"), "{t}");
    let (e, _) = run_cmd("xledger", &["set", "decisions", "D1", "accepted", "与", "提案一致"]).await;
    assert!(!e);
    let (e, t) = run_cmd("xledger", &["set", "decisions", "D1", "maybe"]).await;
    assert!(e && t.contains("结论是"), "{t}");
    let (e, t) = run_cmd("xpkg", &["plan", "truncate", "加 truncate", "--paths", "src/textkit/**,tests/**", "--verify", "单测"]).await;
    assert!(!e, "{t}");
    assert_eq!(std::fs::read_to_string(out.join("packages.paths")).unwrap(), "src/textkit/**\ntests/**\n");
    let (e, _) = run_cmd("xpkg", &["done", "truncate", "加了函数与三条用例", "--files", "src/textkit/__init__.py"]).await;
    assert!(!e);
    let ledger: Value = serde_yaml::from_str(&std::fs::read_to_string(out.join("ledgers/decisions.yaml")).unwrap()).unwrap();
    assert_eq!(ledger["entries"][0]["conclusion"], "accepted", "{ledger}");
    let plan: Value = serde_yaml::from_str(&std::fs::read_to_string(out.join("work-packages.yaml")).unwrap()).unwrap();
    assert_eq!(plan["packages"][0]["status"], "done", "{plan}");

    // Archive: the delta merged into the specs, in the bundle's data.
    let (e, t) = run_cmd("xspec", &["archive"]).await;
    assert!(!e, "{t}");
    let merged = std::fs::read_to_string(root.join("xforge/specs/textkit.md")).unwrap();
    assert!(merged.contains("### TK-R3 截断\ntruncate(text, n)") && merged.contains("None 视为空文本") && !merged.contains("全是空白时为 0"), "{merged}");
    let (_, t) = run_cmd("xspec", &["list", "textkit"]).await;
    assert!(t.contains("TK-R3 截断"), "{t}");
}
