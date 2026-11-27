//! The xforge bundle's dual path (`xforge/dual` and its two children, in Conix's
//! docs/design/xforge.md §5.1): the three flows and every part they name hold together, the risk
//! route stays the three paths; the gates of its own — the black-box run at the join, the spec side
//! a child must not change — judge what they should; and a whole run, driven over a copy of the
//! example by the real runner with its sessions played from here (the files a model would write,
//! the commands it would call, the people's confirmations), goes from the proposal through two
//! children forked from one commit, a black-box failure sent back to the implementation alone, to
//! the archive. No model.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use harnix_flow::author::*;
use harnix_flow::bundle::BundleMeta;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::kernel::route::route;
use harnix_flow::runner::{Binding, CatalogPacks, Runner, RunnerConfig, ScriptedHost, SessionId, TOOL_COMMAND, TOOL_PROPOSE, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

fn packs() -> DirPacks {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    DirPacks::new([example().join(".conix/flows")]).with_catalog(catalog)
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn copy_of_example() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), &format!("cp -R {}/. . && git init -q -b main && git add -A && git -c user.email=x@y -c user.name=x commit -qm base", example().display()));
    d
}

const FLOWS: [&str; 3] = ["xforge/dual", "xforge/dual-impl", "xforge/dual-tests"];

fn stations(p: &DirPacks, flow: &str) -> Vec<String> {
    p.flow(flow).unwrap().0.stations.iter().map(|s| s.id.clone()).collect()
}

fn gates_of(s: &Station) -> Vec<String> {
    s.exit.iter().chain(&s.task_exit).filter_map(|e| match e {
        ExitCond::Gate { gate, .. } => Some(gate.trim_start_matches("xforge/").to_string()),
        _ => None,
    }).collect()
}

#[test]
fn the_dual_path_and_its_children_hold_together_and_stay_out_of_the_risk_route() {
    let p = packs();
    let meta: BundleMeta = serde_yaml::from_str(&p.bundle_file("xforge", "bundle.yaml").unwrap()).unwrap();
    assert_eq!(meta.version, "0.3.0");
    assert_eq!(meta.flows, ["quick", "solid", "major", "dual", "dual-impl", "dual-tests"]);
    assert_eq!(meta.start.route, ["quick", "solid", "major"], "risk picks among the three; dual is started by name");
    assert!(meta.commands.get("xspec").unwrap().read_only.contains(&"seal".to_string()));

    assert_eq!(stations(&p, "xforge/dual"), ["propose", "clarify", "check", "seal", "fork", "join", "verify", "archive"]);
    for child in ["xforge/dual-impl", "xforge/dual-tests"] {
        assert_eq!(stations(&p, child), ["design", "check", "apply"], "{child}");
    }
    let mut gates = BTreeSet::new();
    for f in FLOWS {
        let (def, _) = p.flow(f).unwrap_or_else(|| panic!("no flow {f}"));
        assert_eq!(validate(&def, &p), Vec::<Problem>::new(), "{f}");
        for s in &def.stations {
            gates.extend(gates_of(s));
        }
    }
    for g in &gates {
        let name = format!("xforge/{g}");
        let (text, _) = p.part(PartKind::Gate, &name).unwrap_or_else(|| panic!("no gate {g}"));
        assert_eq!(part_problems(PartKind::Gate, &name, &text, &p), Vec::<String>::new(), "{g}");
    }
    for skill in ["spec-check", "join", "impl-design", "impl-apply", "tests-design", "tests-apply", "dual-check"] {
        assert!(p.skill(&format!("xforge/{skill}")).is_some(), "{skill}");
    }

    let (dual, _) = p.flow("xforge/dual").unwrap();
    assert_eq!(dual.workspace, Some(Workspace::Worktree), "a fork commits on the run's own branch only");
    let fork = dual.station("fork").unwrap();
    let children: Vec<_> = fork.tasks.as_ref().unwrap().iter().map(|t| match &t.executor {
        Executor::Subflow { flow, workspace, inputs } => {
            assert_eq!(workspace, &Some(Workspace::Worktree));
            assert_eq!(inputs["spec_side"], json!("${stations.seal.output.files}"));
            (t.id.as_str(), flow.as_str())
        }
        e => panic!("{e:?}"),
    }).collect();
    assert_eq!(children, [("impl", "xforge/dual-impl"), ("tests", "xforge/dual-tests")]);
    assert!(matches!(&fork.exit[..], [ExitCond::Children { .. }]));
    assert!(matches!(&dual.station("seal").unwrap().executor, Some(Executor::Command { command, args }) if command == "xspec" && args == &["seal"]));
    assert_eq!(gates_of(dual.station("join").unwrap()), ["blackbox", "ledgers-settled"]);
    // Every person's confirmation reads the governance switch, as the three paths' do.
    let confirms: Vec<_> = dual.stations.iter().flat_map(|s| s.exit.iter().map(move |e| (s.id.as_str(), e))).filter_map(|(id, e)| match e {
        ExitCond::Confirm { separate, .. } => Some((id, separate.clone())),
        _ => None,
    }).collect();
    assert_eq!(confirms.iter().map(|(id, _)| *id).collect::<Vec<_>>(), ["clarify", "check", "join", "verify"]);
    for (id, separate) in confirms {
        assert_eq!(separate, Some(OrSetting::Setting("${settings.review.separate}".into())), "{id}");
    }
    assert_eq!(dual.station("join").unwrap().rework_to, ["fork", "propose"]);
    // Every station of a child checks the spec side it holds; what it writes is its own.
    for child in ["xforge/dual-impl", "xforge/dual-tests"] {
        let (def, _) = p.flow(child).unwrap();
        for s in &def.stations {
            assert!(gates_of(s).contains(&"spec-unchanged".to_string()), "{child} {}", s.id);
        }
        assert_eq!(def.station("design").unwrap().writes, ["${output_dir}/**"]);
        assert_eq!(def.station("apply").unwrap().writes, ["${item.paths}"]);
        assert!(def.result.as_ref().is_some_and(|r| r.schema.is_some()), "{child}: what it hands its parent is checked");
        // Started on its own, it is refused at once: the spec side comes only from dual's fork.
        let e = def.apply_inputs(&json!({"title": "t"})).unwrap_err().to_string();
        assert!(e.contains("spec_side"), "{child}: {e}");
    }

    let to = |inputs: Value| match route(&p, "xforge", &inputs).unwrap() {
        Route::Flow { flow, .. } => flow,
        r => panic!("{r:?}"),
    };
    assert_eq!(to(json!({"title": "t", "risk": "high", "impact": ["interface"]})), "xforge/major");
    let Route::Triage { candidates, .. } = route(&p, "xforge", &json!({"title": "t"})).unwrap() else { panic!() };
    assert_eq!(candidates.iter().map(|c| c.flow.as_str()).collect::<Vec<_>>(), ["xforge/quick", "xforge/solid", "xforge/major"]);
}

#[derive(Default)]
struct Convo {
    project: Mutex<Option<String>>,
}

#[async_trait]
impl AuthorHost for Convo {
    async fn current(&self, _: &SessionId) -> Option<String> {
        None
    }
    async fn project(&self, _: &SessionId) -> Option<String> {
        self.project.lock().unwrap().clone()
    }
}

/// The gates tried on a copy of the example, as `flow_gate_try` tries them.
struct Trials {
    tools: AuthorTools,
    _rn: Arc<Runner>,
    project: tempfile::TempDir,
}

fn trials() -> Trials {
    let cfg = RunnerConfig { command_gates: true, profiles: vec!["code".into()], main_profile: Some("code".into()), submit_profiles: ["code".to_string()].into(), ..Default::default() };
    let project = copy_of_example();
    let layout = ProjectLayout::new(".conix", project.path().join(".wkts"), "conix/flow").with_artifacts_dir("xforge");
    let packs = Arc::new(CatalogPacks::over(&cfg, vec![], Arc::new(packs())));
    packs.set_layout(layout.clone());
    let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), packs.clone()).with_main_scene("code").with_layout(layout));
    let scripted = ScriptedHost::new();
    scripted.trust(project.path(), true);
    let rn = Runner::new(engine, scripted, cfg);
    let sandbox = Arc::new(Sandbox::new(packs, SandboxConfig { guarded: vec![], submit_scenes: vec!["code".into()], main_scene: Some("code".into()) }));
    let host = Arc::new(Convo::default());
    *host.project.lock().unwrap() = Some(project.path().to_string_lossy().into_owned());
    Trials { tools: AuthorTools::new(&rn, sandbox, host), _rn: rn, project }
}

impl Trials {
    async fn tried(&self, gate: &str) -> Value {
        let r = self.tools.call(&"s".to_string(), TOOL_GATE_TRY, json!({"gate": format!("xforge/{gate}")}), &Actor::new(CONVERSATION_KIND, "s")).await;
        assert!(!r.is_error, "{gate}: {}", r.text);
        serde_json::from_str(&r.text).unwrap()
    }
    fn governance(&self, f: impl FnOnce(&str) -> String) {
        let p = self.project.path().join(".conix/governance/xforge.yaml");
        let text = std::fs::read_to_string(&p).unwrap();
        std::fs::write(&p, f(&text)).unwrap();
    }
}

#[tokio::test]
async fn the_black_box_run_takes_the_menus_blackbox_entry_and_errors_without_it() {
    let t = trials();
    let r = t.tried("blackbox").await;
    assert_eq!(r["verdict"], "pass", "the example's own black-box tests: {r}");
    std::fs::write(t.project.path().join("tests_blackbox/test_broken.py"), "import unittest\nclass T(unittest.TestCase):\n    def test_x(self):  # TK-R1\n        self.fail('boom')\n").unwrap();
    let r = t.tried("blackbox").await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("blackbox"), "{r}");

    // Named with a parameter: nobody fills it in at the join.
    t.governance(|s| s.replace("argv: [python3, -m, unittest, discover, -s, tests_blackbox, -q]", "argv: [python3, -m, unittest, discover, -s, \"{dir}\", -q]\n    params: {dir: \"tests_[a-z]+\"}"));
    let r = t.tried("blackbox").await;
    assert_eq!(r["verdict"], "error", "{r}");
    assert!(r["summary"].as_str().unwrap().contains("不能带参数"), "{r}");
    // Not on the menu at all: dual has nothing to judge the implementation with.
    t.governance(|s| s.split("\n  blackbox:").next().unwrap().to_string() + "\n");
    let r = t.tried("blackbox").await;
    assert_eq!(r["verdict"], "error", "{r}");
    assert!(r["summary"].as_str().unwrap().contains("verify 菜单要有 blackbox"), "{r}");
}

#[tokio::test]
async fn the_spec_side_gate_outside_a_child_says_this_flow_is_only_a_child_of_dual() {
    let t = trials();
    let r = t.tried("spec-unchanged").await;
    assert_eq!(r["verdict"], "error", "{r}");
    assert!(r["summary"].as_str().unwrap().contains("只做 xforge/dual 的子运行"), "{r}");
}

// ── a whole run ────────────────────────────────────────────────────────────────────────────────

const T0: u64 = 1_794_787_200_000;

fn person() -> Actor {
    Actor::new("person", "owner")
}

/// The runner over a copy of the example, its sessions played by the test (`Drive::act`).
struct Drive {
    rn: Arc<Runner>,
    engine: Arc<Engine>,
    host: Arc<ScriptedHost>,
    /// The project, as its real path: a temporary directory may be named through a link (`/var`
    /// on macOS), and a script's glob of an absolute path is judged against the real one.
    root: PathBuf,
    _project: tempfile::TempDir,
    _wkts: tempfile::TempDir,
    /// The claims already acted on: run, station, task, token.
    seen: Mutex<HashSet<(String, String, String, String)>>,
    /// The confirmations given: run, station, round.
    confirmed: Mutex<HashSet<(String, String, u32)>>,
    /// What the driving saw on the way, for the checks after.
    log: Mutex<Vec<String>>,
}

async fn drive_setup() -> Drive {
    let project = copy_of_example();
    let root = std::fs::canonicalize(project.path()).unwrap();
    let wkts = tempfile::tempdir().unwrap();
    let wkts_root = std::fs::canonicalize(wkts.path()).unwrap();
    let cfg = RunnerConfig {
        command_gates: true,
        profiles: vec!["code".into()],
        main_profile: Some("code".into()),
        submit_profiles: ["code".to_string()].into(),
        // A claim is renewed (and a task over noticed) every third of it; the manual clock lets none expire.
        lease_ms: 1500,
        tick_ms: 1000,
        ..Default::default()
    };
    let layout = ProjectLayout::new(".conix", &wkts_root, "conix/flow").with_artifacts_dir("xforge");
    let packs = Arc::new(CatalogPacks::over(&cfg, vec![], Arc::new(packs())));
    packs.set_layout(layout.clone());
    let engine = Arc::new(
        Engine::new(Arc::new(MemStore::new()), packs)
            .with_clock(Arc::new(ManualClock::new(T0)))
            .with_lease_ms(600_000)
            .with_submit_scenes(["code"])
            .with_main_scene("code")
            .with_layout(layout),
    );
    let host = ScriptedHost::new();
    host.trust(&root, true);
    let rn = Runner::new(engine.clone(), host.clone(), cfg);
    rn.set_trust(&root, true);
    rn.start().await;
    Drive {
        rn,
        engine,
        host,
        root,
        _project: project,
        _wkts: wkts,
        seen: Default::default(),
        confirmed: Default::default(),
        log: Default::default(),
    }
}

const PROPOSAL: &str = "# 加 truncate\n\n## 背景\n标题太长，列表里放不下。\n\n## 目标\n能把文本截到给定长度。\n\n## 非目标\n不按词边界截。\n\n\
    ## 改什么\n新增 TK-R3 与 fn:textkit.truncate。\n\n## 影响面\n无\n\n## 为什么选这条流程\n行为说得清，值得要一套独立写的测试。\n";

fn design(what: &str) -> String {
    format!(
        "# 加 truncate：{what}\n\n## 做法\n{what}照 TK-R3 的 WHEN / THEN 落地。\n\n## 集成点\n只经 textkit 的公开函数 truncate。\n\n\
         ## 失败模式\nn 为 0 时返回空串。\n\n## 迁移与回滚\n纯新增，删掉即回滚。\n\n## 被否决的方案\n- 按词截断：中文没有空格，截不准。\n\n\
         ## 原则对照\n- P1：只用标准库的字符串切片。\n- P2：TK-R3 有带编号注释的用例。\n- P3：不改已有的 slugify 与 word_count。\n\
         - P4：一个包，只碰它声明的路径。\n\n## 工作包\n一个包。\n"
    )
}

const BLACKBOX: &str = r#"import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from textkit import truncate  # noqa: E402


class TruncateSpec(unittest.TestCase):
    def test_longer_text_is_cut_to_n(self):  # TK-R3 fn:textkit.truncate
        self.assertEqual(truncate("abcdef", 3), "abc")

    def test_shorter_text_is_kept(self):  # TK-R3
        self.assertEqual(truncate("ab", 3), "ab")
"#;

const UNIT: &str = r#"import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "src"))

from textkit import truncate  # noqa: E402


class TruncateTest(unittest.TestCase):
    def test_short_text_is_kept(self):  # TK-R3
        self.assertEqual(truncate("ab", 5), "ab")
"#;

/// The implementation: the first one cuts one character short (its own unit test does not see it;
/// the black-box test does).
fn implementation(right: bool) -> String {
    let src = std::fs::read_to_string(example().join("src/textkit/__init__.py")).unwrap();
    let cut = if right { "text[:n]" } else { "text[: n - 1]" };
    src.replace("__all__ = [\"slugify\", \"word_count\"]", "__all__ = [\"slugify\", \"word_count\", \"truncate\"]")
        + &format!("\n\ndef truncate(text: str, n: int) -> str:\n    \"\"\"把文本截到 n 个字符。\"\"\"\n    return text if len(text) <= n else {cut}\n")
}

impl Drive {
    fn note(&self, s: impl Into<String>) {
        self.log.lock().unwrap().push(s.into());
    }

    async fn call(&self, id: &SessionId, tool: &str, input: Value) -> String {
        let r = self.rn.tools().call(id, tool, input.clone()).await;
        assert!(!r.is_error, "{tool} {input}: {}", r.text);
        r.text
    }

    async fn cmd(&self, id: &SessionId, command: &str, args: &[&str]) -> String {
        self.call(id, TOOL_COMMAND, json!({"command": command, "args": args})).await
    }

    async fn submit(&self, id: &SessionId, output: Value) {
        let text = self.call(id, TOOL_SUBMIT, json!({"output": output})).await;
        assert!(text.starts_with("Accepted"), "{text}");
    }

    async fn record(&self, run: &str) -> RunRecord {
        self.engine.record(run).await.unwrap()
    }

    async fn principles(&self, id: &SessionId) {
        for p in ["P1", "P2", "P3", "P4"] {
            self.cmd(id, "xledger", &["reply", p, "complies", "查过，守得住，理由写在设计与规格里"]).await;
        }
    }

    /// What the session bound to `b` does: the files a model would write, the commands it would
    /// call, its hand-in.
    async fn act(&self, id: &SessionId, b: &Binding) {
        let rec = self.record(&b.run).await;
        let ws = self.engine.workspace_for(&rec).unwrap();
        let out = PathBuf::from(rec.output_dir.clone().unwrap());
        let parent_out = rec.worktree.as_ref().and_then(|w| w.fork.as_ref()).and_then(|f| f.parent_output_dir.clone()).map(PathBuf::from);
        self.note(format!("{} {} {}", rec.flow, b.station, b.task));
        match (rec.flow.as_str(), b.station.as_str()) {
            ("xforge/dual", "propose") => {
                std::fs::write(out.join("proposal.md"), PROPOSAL).unwrap();
                self.cmd(id, "xspec", &["add", "textkit", "截断", "- WHEN 文本超过 n 个字符\n- THEN 返回前 n 个字符\n- WHEN 文本不超过 n 个字符\n- THEN 原样返回"]).await;
                self.cmd(id, "xarch", &["add", "textkit", "fn", "truncate", "truncate(text: str, n: int) -> str", "把文本截到 n 个字符"]).await;
                self.submit(id, json!({"summary": "加 truncate"})).await;
            }
            ("xforge/dual", "clarify") => self.submit(id, json!({"summary": "没有会改变范围的问题", "questions": 0})).await,
            ("xforge/dual", "check") => {
                self.principles(id).await;
                self.submit(id, json!({"summary": "规格侧说得清", "ready": true})).await;
            }
            ("xforge/dual", "join") => {
                let round = rec.stations["join"].round;
                if round == 1 {
                    let ran = sh(&ws, "python3 -B -m unittest discover -s tests_blackbox -q 2>&1 || true");
                    assert!(ran.contains("FAILED (failures=1)") && ran.contains("'ab' != 'abc'"), "the first implementation is one short: {ran}");
                    self.cmd(id, "xledger", &["add", "findings", "test_longer_text_is_cut_to_n：期望 abc，实际 ab。建议：实现错，因为 TK-R3 说返回前 n 个字符", "--ref", "TK-R3,tests_blackbox/test_truncate.py"]).await;
                    self.call(id, TOOL_PROPOSE, json!({"kind": "rework", "to": "fork", "reason": "只重做 impl：F1"})).await;
                    self.submit(id, json!({"summary": "1 个用例没过，错在实现", "failures": 1, "rework": {"tasks": ["impl"], "reason": "F1"}})).await;
                } else {
                    self.cmd(id, "xledger", &["set", "findings", "F1", "resolved", "重做的实现截到 n 个，黑盒测试都过了"]).await;
                    self.submit(id, json!({"summary": "黑盒测试都过了", "failures": 0})).await;
                }
            }
            ("xforge/dual", "verify") => {
                let rows = "- TK-R3 通过：tests_blackbox/test_truncate.py#test_longer_text_is_cut_to_n，黑盒测试跑过\n\
                            - fn:textkit.truncate 通过：签名与登记的一致，tests_blackbox/test_truncate.py 经它调用";
                std::fs::write(out.join("verification.md"), format!("# 验证\n\n## 覆盖\n{rows}\n\n## 连贯性\n提案、规格、两路的设计与测试一致。\n\n## 还剩的风险\n无。\n")).unwrap();
                self.submit(id, json!({"summary": "都验证了"})).await;
            }
            (child @ ("xforge/dual-impl" | "xforge/dual-tests"), "design") => {
                let parent_out = parent_out.expect("a child in a worktree of its own");
                // The spec side as of the fork, its own copy.
                let delta = std::fs::read_to_string(parent_out.join("spec-delta.yaml")).unwrap();
                assert!(delta.contains("TK-R3"), "{delta}");
                self.note(format!("{child} sees the spec side at {}", parent_out.display()));
                let impl_side = child == "xforge/dual-impl";
                std::fs::write(out.join("design.md"), design(if impl_side { "实现" } else { "黑盒测试" })).unwrap();
                let (pkg, paths, verify): (&str, &str, &[&str]) =
                    if impl_side { ("core", "src/textkit/**,tests/test_truncate.py", &["--verify", "compile"]) } else { ("bb", "tests_blackbox/test_truncate.py", &[]) };
                let mut args = vec!["plan", pkg, "加 truncate", "--paths", paths, "--criteria", "照 TK-R3 做"];
                args.extend_from_slice(verify);
                self.cmd(id, "xpkg", &args).await;
                // The tests' first design edits the spec side it holds: the gate sends it back.
                let spec = parent_out.join("spec-delta.yaml");
                let round = rec.stations["design"].round;
                if !impl_side && round == 1 {
                    std::fs::write(&spec, format!("{delta}# 测试这一路顺手改了规格\n")).unwrap();
                    self.note("tests edited the spec side");
                } else if !impl_side && round == 2 {
                    std::fs::write(&spec, delta.replace("# 测试这一路顺手改了规格\n", "")).unwrap();
                    let unmet = &rec.stations["design"].reworked.as_ref().unwrap().unmet;
                    let found = unmet.iter().find(|c| c.label.contains("spec-unchanged")).unwrap_or_else(|| panic!("{unmet:?}"));
                    assert_eq!(found.status, CheckStatus::Fail);
                    assert!(found.explain().contains("spec-delta.yaml 改了"), "{}", found.explain());
                    self.note("spec-unchanged failed the tests' edit");
                }
                self.submit(id, json!({"summary": format!("{child} 的设计"), "packages": [{"id": pkg, "title": "加 truncate", "paths": paths.split(',').collect::<Vec<_>>()}]})).await;
            }
            (_, "check") => {
                self.principles(id).await;
                self.submit(id, json!({"summary": "照规格侧做的", "ready": true})).await;
            }
            ("xforge/dual-impl", "apply") => {
                // The implementation is redone only after the black box sent it back.
                let again = ws.join("tests_blackbox/test_truncate.py").is_file();
                std::fs::write(ws.join("src/textkit/__init__.py"), implementation(again)).unwrap();
                let files = if again {
                    self.note("the redone implementation sees the tests merged in the round before");
                    "src/textkit/__init__.py"
                } else {
                    std::fs::write(ws.join("tests/test_truncate.py"), UNIT).unwrap();
                    "src/textkit/__init__.py,tests/test_truncate.py"
                };
                self.cmd(id, "xpkg", &["done", "core", "加了 truncate", "--files", files, "--evidence", "C1=单测过了"]).await;
                self.submit(id, json!({"package": "core", "status": "done", "summary": "加了 truncate"})).await;
            }
            ("xforge/dual-tests", "apply") => {
                assert!(!ws.join("tests/test_truncate.py").exists(), "the tests' worktree has none of the implementation's");
                std::fs::write(ws.join("tests_blackbox/test_truncate.py"), BLACKBOX).unwrap();
                self.cmd(id, "xpkg", &["done", "bb", "写了 TK-R3 的用例", "--files", "tests_blackbox/test_truncate.py", "--evidence", "C1=两个用例，经 truncate 调用"]).await;
                self.submit(id, json!({"package": "bb", "status": "done", "summary": "写了 TK-R3 的黑盒测试"})).await;
            }
            other => panic!("no part for {other:?}"),
        }
    }

    /// The runs of the tree under `run`.
    async fn tree(&self, run: &str) -> Vec<String> {
        let mut out = vec![run.to_string()];
        let mut i = 0;
        while i < out.len() {
            let rec = self.record(&out[i]).await;
            for st in rec.stations.values() {
                out.extend(st.tasks.iter().filter_map(|t| t.child.clone()));
            }
            i += 1;
        }
        out
    }

    /// Act for every session bound to a task not acted on yet, confirm every station that waits
    /// only for a person, until `done` says so of the parent.
    async fn until(&self, parent: &str, what: &str, done: impl Fn(&RunRecord) -> bool) {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        loop {
            let rec = self.record(parent).await;
            if done(&rec) {
                return;
            }
            assert!(rec.is_active(), "{what}: the run ended: {:?}", rec.status);
            assert!(tokio::time::Instant::now() < deadline, "{what}: not reached: {:#?}\n{:#?}", self.engine.get(parent).await.unwrap().unmet, self.log.lock().unwrap());
            // Nothing here should be past judging: say so at once rather than at the deadline.
            for run in self.tree(parent).await {
                let view = self.engine.get(&run).await.unwrap();
                let broken: Vec<_> = view.unmet.values().flatten().filter(|c| c.status == CheckStatus::Error).collect();
                assert!(broken.is_empty(), "{what}: {run}: {broken:#?}\n{:#?}", self.log.lock().unwrap());
                let failed: Vec<_> = view.record.stations.values().flat_map(|s| &s.tasks).filter(|t| t.status == TaskStatus::Failed).collect();
                assert!(failed.is_empty(), "{what}: {run}: {failed:#?}");
            }
            for spec in self.host.opened() {
                let Some(id) = self.host.session_of(&spec.key) else { continue };
                let Some(b) = self.rn.binding(&id) else { continue };
                if self.seen.lock().unwrap().insert((b.run.clone(), b.station.clone(), b.task.clone(), b.token.clone())) {
                    self.act(&id, &b).await;
                }
            }
            for run in self.tree(parent).await {
                let view = self.engine.get(&run).await.unwrap();
                for (sid, unmet) in &view.unmet {
                    let round = view.record.stations[sid].round;
                    let only_people = !unmet.is_empty() && unmet.iter().all(|c| c.kind == "confirm");
                    if only_people && self.confirmed.lock().unwrap().insert((run.clone(), sid.clone(), round)) {
                        self.note(format!("confirmed {run} {sid}"));
                        self.rn.settle(self.engine.confirm(&run, sid, true, None, person()).await).await.unwrap();
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

fn git(dir: &Path, args: &str) -> String {
    sh(dir, &format!("git {args}"))
}

#[tokio::test]
async fn a_dual_run_forks_two_children_from_one_commit_joins_them_and_archives() {
    if std::process::Command::new("python3").arg("--version").output().is_err() {
        return;
    }
    let d = drive_setup().await;
    let root = d.root.as_path();
    let main_head = git(root, "rev-parse main");
    let input = CreateIn {
        flow: "xforge/dual".into(),
        subject: "给 textkit 加 truncate".into(),
        inputs: json!({"title": "truncate", "risk": "high"}),
        project_root: Some(root.to_string_lossy().into_owned()),
        ..Default::default()
    };
    let o = d.rn.start_run(input, person()).await.unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    d.rn.apply(o).await;

    // The spec side written, checked and approved; sealed; two children forked and through.
    d.until(&run, "the first join", |r| r.stations.get("join").is_some_and(|s| s.status.is_open() && s.tasks.iter().all(|t| t.status == TaskStatus::Submitted))).await;
    let rec = d.record(&run).await;
    let wt = PathBuf::from(&rec.worktree.as_ref().unwrap().path);
    let out = PathBuf::from(rec.output_dir.clone().unwrap());
    let sealed = rec.stations["seal"].tasks[0].output.clone().unwrap();
    for f in ["proposal.md", "spec-delta.yaml", "interface-delta.yaml"] {
        assert!(sealed["files"][f].as_str().is_some_and(|s| s.len() == 64), "{f}: {sealed}");
    }
    let first = rec.stations["fork"].fork.clone().unwrap();
    let tasks = &rec.stations["fork"].tasks;
    let (one, two) = (tasks[0].child.clone().unwrap(), tasks[1].child.clone().unwrap());
    for (task, child) in [("impl", &one), ("tests", &two)] {
        let c = d.record(child).await;
        assert_eq!(c.base_commit.as_deref(), Some(first.commit.as_str()), "{task}: both from the one fork commit");
        assert_eq!(c.inputs["spec_side"], sealed["files"], "{task}");
        let o = c.worktree.as_ref().unwrap().outcome.clone().unwrap();
        assert_eq!(o.kind, WorktreeEnd::Merged, "{task}");
        assert_eq!(o.into.as_deref(), Some(rec.worktree.as_ref().unwrap().branch.as_str()), "{task}: into the run's branch");
        sh(Path::new(&rec.worktree.as_ref().unwrap().path), &format!("git merge-base --is-ancestor {} HEAD", o.commit.unwrap()));
        assert_eq!(c.output_dir.as_deref().map(|o| o.ends_with(&format!("children/fork/{task}"))), Some(true));
        // Each one's design in a directory of its own, back in the parent's.
        let designed = std::fs::read_to_string(out.join("children/fork").join(task).join("design.md")).unwrap();
        assert!(designed.contains(if task == "impl" { "：实现" } else { "：黑盒测试" }), "{designed}");
        // Its result, as its flow says.
        let result = tasks.iter().find(|t| t.id == task).unwrap().output.clone().unwrap();
        assert_eq!(result["side"], task);
        assert_eq!(result["done"][0]["status"], "done");
        // The skill told it where the spec side is.
        let main = d.host.session_of(&format!("flow:{child}#@main")).unwrap();
        let sent = d.host.sent_to(&main).join("\n");
        let theirs = c.worktree.as_ref().unwrap().fork.as_ref().unwrap().parent_output_dir.clone().unwrap();
        let skill = sent.split("## Skill `xforge/").nth(1).unwrap_or_default();
        assert!(skill.contains(&format!("{theirs}/ledgers/findings.yaml")) && !skill.contains("${parent.output_dir}"), "{task}: {skill}");
    }
    // Every session of either child — its main one, where its stations run, from its start — is
    // kept from its parent's worktree and its sibling's, whichever forked first.
    let opened = d.host.opened();
    let (w1, w2) = (d.record(&one).await.worktree.unwrap().path, d.record(&two).await.worktree.unwrap().path);
    for (child, other) in [(&one, &w2), (&two, &w1)] {
        let mut want = vec![wt.clone(), PathBuf::from(other)];
        want.sort();
        let theirs: Vec<_> = opened.iter().filter(|s| s.run == *child).collect();
        assert!(theirs.iter().any(|s| s.key == format!("flow:{child}#@main")), "{child}: no main session");
        for s in theirs {
            assert_eq!(s.deny_read, want, "{}", s.key);
        }
    }
    assert!(opened.iter().filter(|s| s.run == run).all(|s| s.deny_read.is_empty()), "the parent is kept from nothing");
    // Both merged into the run's branch, not the project's.
    assert!(wt.join("tests_blackbox/test_truncate.py").is_file() && wt.join("tests/test_truncate.py").is_file());
    assert_eq!(git(root, "rev-parse main"), main_head);
    {
        let log = d.log.lock().unwrap();
        assert!(log.iter().any(|l| l == "spec-unchanged failed the tests' edit"), "{log:#?}");
    }

    // The black box fails the first implementation: the join waits; a person sends the
    // implementation alone back.
    d.until(&run, "the join judged", |r| r.stations["join"].checks.iter().any(|c| c.label.contains("blackbox") && c.status == CheckStatus::Fail)).await;
    let rec = d.record(&run).await;
    assert_eq!(rec.stations["join"].status, StationStatus::Unmet, "{:?}", rec.stations["join"]);
    assert!(d.engine.get(&run).await.unwrap().proposals.iter().any(|p| p.to.as_deref() == Some("fork")));
    d.rn.settle(d.engine.rework(&run, "fork", Some(vec!["impl".into()]), "黑盒测试 F1：实现少截了一个", person()).await).await.unwrap();

    d.until(&run, "the end", |r| r.status == RunStatus::Finished).await;
    let rec = d.record(&run).await;
    let again = rec.stations["fork"].fork.clone().unwrap();
    assert_eq!(again.round, first.round + 1);
    assert_ne!(again.commit, first.commit);
    let tasks = &rec.stations["fork"].tasks;
    let three = tasks[0].child.clone().unwrap();
    assert_ne!(three, one, "the implementation forked again");
    assert_eq!(tasks[1].child.as_deref(), Some(two.as_str()), "the tests kept");
    assert_eq!(d.record(&three).await.base_commit.as_deref(), Some(again.commit.as_str()));
    // Its sibling merged and gone: kept from its parent's worktree alone.
    let theirs: Vec<_> = d.host.opened().into_iter().filter(|s| s.run == three).collect();
    assert!(!theirs.is_empty());
    for s in theirs {
        assert_eq!(s.deny_read, std::slice::from_ref(&wt), "{}", s.key);
    }
    {
        let log = d.log.lock().unwrap();
        assert!(log.iter().any(|l| l == "the redone implementation sees the tests merged in the round before"), "{log:#?}");
    }
    // Merged, verified, archived into the baselines on the run's branch.
    assert!(std::fs::read_to_string(wt.join("src/textkit/__init__.py")).unwrap().contains("text[:n]"));
    assert!(std::fs::read_to_string(wt.join("xforge/specs/textkit.md")).unwrap().contains("TK-R3"));
    assert!(std::fs::read_to_string(wt.join("xforge/interfaces/textkit.yaml")).unwrap().contains("fn:textkit.truncate"));
    let findings = std::fs::read_to_string(out.join("ledgers/findings.yaml")).unwrap();
    assert!(findings.contains("resolved"), "{findings}");
    let archived = PathBuf::from(&wt).join(rec.archive.as_ref().unwrap().dir.clone());
    assert!(archived.join("children/fork/impl/design.md").is_file() && archived.join("children/fork/tests/design.md").is_file(), "{}", archived.display());
    assert_eq!(git(root, "rev-parse main"), main_head, "the project's own branch untouched");
}
