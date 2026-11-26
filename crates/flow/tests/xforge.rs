//! The xforge bundle Conix ships (`examples/xforge/.conix/flows/xforge`, in Conix; its
//! docs/design/xforge.md §4–§5): its three paths and every part they name hold together; a start
//! naming the bundle goes by risk, and to triage without one; each path dry-runs through the people
//! it waits for to its end; the gates on what models write — the proposal's and the design's
//! sections, the impact, the verification record — pass a good one and fail a bad one. The
//! commands and their gates have their own files (`xforge_spec.rs`, `xforge_ledger.rs`, …). No model.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use harnix_flow::author::*;
use harnix_flow::bundle::BundleMeta;
use harnix_flow::dryrun::{Sandbox, SandboxConfig, StartIn};
use harnix_flow::kernel::route::route;
use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, SessionId};
use harnix_flow::*;
use serde_json::{json, Value};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

/// The packs as Conix offers the bundle: a built-in directory of bundles.
fn packs() -> DirPacks {
    packs_in(&example().join(".conix/flows"))
}

/// The same over another directory of bundles.
fn packs_in(dir: &Path) -> DirPacks {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    DirPacks::new([dir]).with_catalog(catalog)
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

const PATHS: [&str; 3] = ["xforge/quick", "xforge/solid", "xforge/major"];

fn stations(p: &DirPacks, flow: &str) -> Vec<String> {
    p.flow(flow).unwrap().0.stations.iter().map(|s| s.id.clone()).collect()
}

#[test]
fn the_three_paths_and_every_part_they_name_hold_together() {
    let p = packs();
    let meta: BundleMeta = serde_yaml::from_str(&p.bundle_file("xforge", "bundle.yaml").unwrap()).unwrap();
    assert_eq!(meta.flows, ["quick", "solid", "major"]);
    assert_eq!(meta.start.route, ["quick", "solid", "major"]);
    assert_eq!(meta.start.triage.as_deref(), Some("triage"));
    assert_eq!(meta.pace, Some(Pace::Auto));
    assert_eq!(meta.commands.names().collect::<Vec<_>>(), ["xspec", "xarch", "xledger", "xpkg"]);
    assert!(meta.commands.get("xspec").unwrap().read_only.contains(&"pending".to_string()));
    assert_eq!(meta.commands.get("xledger").unwrap().read_only, ["list", "help"]);
    for c in meta.commands.names() {
        let src = p.bundle_file("xforge", &format!("commands/{c}.js")).unwrap_or_else(|| panic!("no command {c}"));
        assert!(src.contains("export default function"), "{c}");
    }
    assert!(p.skill("xforge/triage").is_some() && p.skill("xforge/clarify").is_some());
    assert!(p.skill("xforge/archive").is_none(), "archiving is the commands' work, no model's");

    assert_eq!(stations(&p, "xforge/quick"), ["propose", "apply", "verify", "archive"]);
    assert_eq!(stations(&p, "xforge/solid"), ["propose", "design", "check", "apply", "verify", "archive"]);
    assert_eq!(stations(&p, "xforge/major"), ["propose", "clarify", "design", "check", "apply", "verify", "archive"]);
    let mut gates = BTreeSet::new();
    for f in PATHS {
        let (def, _) = p.flow(f).unwrap_or_else(|| panic!("no flow {f}"));
        assert_eq!(validate(&def, &p), Vec::<Problem>::new(), "{f}");
        assert_eq!(def.bundle.as_deref(), Some("xforge"));
        // The three take the same inputs; only what risk each accepts differs.
        assert_eq!(def.inputs.keys().collect::<Vec<_>>(), ["impact", "risk", "title"], "{f}");
        assert!(def.inputs["risk"].default.is_none(), "{f}: no risk given is what triage is for");
        assert_eq!(def.inputs["impact"].default, Some(json!([])), "{f}");
        for s in &def.stations {
            for e in s.exit.iter().chain(&s.task_exit) {
                if let ExitCond::Gate { gate, .. } = e {
                    gates.insert(gate.trim_start_matches("xforge/").to_string());
                }
            }
        }
        let apply = def.station("apply").unwrap();
        assert_eq!(apply.writes, ["${item.paths}"], "{f}: a package writes its own paths");
        let task_exit: Vec<_> = apply.task_exit.iter().map(|e| match e {
            ExitCond::Gate { gate, .. } => gate.as_str(),
            _ => "",
        }).collect();
        assert_eq!(task_exit, ["xforge/package-check", "xforge/package-tests"], "{f}: each package's bookkeeping, then its own verification");
        let archive = def.station("archive").unwrap();
        let steps: Vec<_> = archive.tasks.as_ref().unwrap().iter().map(|t| &t.executor).collect();
        assert!(steps.iter().all(|e| matches!(e, Executor::Command { .. })), "{f}: {steps:?}");
    }
    for g in &gates {
        let name = format!("xforge/{g}");
        let (text, _) = p.part(PartKind::Gate, &name).unwrap_or_else(|| panic!("no gate {g}"));
        assert_eq!(part_problems(PartKind::Gate, &name, &text, &p), Vec::<String>::new(), "{g}");
    }
    for g in ["impact", "design-sections", "interface-delta", "interface-compat", "constitution-reply", "interfaces-archived"] {
        assert!(gates.contains(g), "{g} is named by some path");
    }
}

#[test]
fn a_start_of_the_bundle_goes_by_risk_and_to_triage_without_it() {
    let p = packs();
    let to = |inputs: Value| match route(&p, "xforge", &inputs).unwrap() {
        Route::Flow { flow, .. } => flow,
        r => panic!("{r:?}"),
    };
    assert_eq!(to(json!({"title": "t", "risk": "low"})), "xforge/quick");
    assert_eq!(to(json!({"title": "t", "risk": "medium", "impact": ["security"]})), "xforge/solid");
    assert_eq!(to(json!({"title": "t", "risk": "high", "impact": ["interface", "data-migration"]})), "xforge/major");
    // Impact does not choose the path; it is judged in the proposal.
    assert_eq!(to(json!({"title": "t", "risk": "low", "impact": ["interface"]})), "xforge/quick");

    let Route::Triage { skill, candidates, .. } = route(&p, "xforge", &json!({"title": "t"})).unwrap() else { panic!() };
    assert_eq!(skill, "xforge/triage");
    assert_eq!(candidates.iter().map(|c| c.flow.as_str()).collect::<Vec<_>>(), PATHS);
    assert_eq!(candidates[2].inputs["properties"]["risk"]["enum"], json!(["low", "medium", "high"]));
    let e = route(&p, "xforge", &json!({"title": "t", "risk": "low", "impact": ["ui"]})).unwrap_err().to_string();
    assert!(e.contains("no flow of bundle xforge takes these inputs"), "{e}");
    // Named, a path takes only the risks it accepts.
    assert!(packs().flow("xforge/solid").unwrap().0.apply_inputs(&json!({"title": "t", "risk": "high"})).is_err());
}

fn sandbox() -> Sandbox {
    sandbox_over(packs())
}

fn sandbox_over(packs: DirPacks) -> Sandbox {
    Sandbox::new(Arc::new(packs), SandboxConfig { guarded: vec![], submit_scenes: vec!["code".into()], main_scene: Some("code".into()) })
}

fn person() -> Actor {
    Actor::new("person", "owner")
}

/// A dry run of `flow` with `mocks`, confirmed at each station of `people` in turn: after each
/// run of work it must be waiting at that station. Then it must be finished. The run's record.
async fn dry_through(flow: &str, risk: &str, mocks: Value, people: &[&str]) -> RunRecord {
    let sb = sandbox();
    let start = StartIn {
        flow: Some(flow.into()),
        inputs: json!({"title": "truncate", "risk": risk}),
        mocks: serde_json::from_value(mocks).unwrap(),
        ..Default::default()
    };
    let run = sb.start(start, person()).await.unwrap();
    sb.run(&run).await.unwrap();
    let eng = sb.engine_for(&run).await.unwrap();
    for at in people {
        let rec = eng.record(&run).await.unwrap();
        assert_eq!(rec.stations[*at].status, StationStatus::Working, "{flow}: waits at {at}: {:?}", rec.stations);
        let waiting: Vec<_> = rec.stations.iter().filter(|(_, s)| s.status.is_open()).map(|(k, _)| k.as_str()).collect();
        assert_eq!(waiting, [*at], "{flow}");
        let o = eng.confirm(&run, at, true, None, person()).await.unwrap();
        sb.absorb(&run, o).await.unwrap();
        sb.run(&run).await.unwrap();
    }
    let rec = eng.record(&run).await.unwrap();
    assert_eq!(rec.status, RunStatus::Finished, "{flow}: {:?}", rec.stations);
    rec
}

fn packages() -> Value {
    json!([
        {"id": "core", "title": "加 truncate", "paths": ["src/textkit/**"]},
        {"id": "tests", "title": "测 truncate", "paths": ["tests/**"], "after": ["core"]},
    ])
}

fn ends() -> Value {
    json!({
        "apply": {"package": "core", "status": "done", "summary": "s"},
        "verify": {"summary": "s"},
        "archive": {"specs": {"merged": 1}, "interfaces": {"merged": 0}},
    })
}

#[tokio::test]
async fn each_path_dry_runs_through_the_people_it_waits_for_to_its_end() {
    let mut m = ends();
    m["propose"] = json!({"summary": "s", "packages": packages()});
    let rec = dry_through("xforge/quick", "low", json!({"stations": m}), &["verify"]).await;
    // A dry run reads no project: the bundle's defaults, under which whoever started it confirmed.
    let settings = rec.parts.settings.as_ref().unwrap();
    assert_eq!((&settings["governance"], &settings["review"]), (&json!({"spec": true, "interface": false}), &json!({"separate": false})));
    let apply = &rec.stations["apply"];
    assert_eq!(apply.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["core", "tests"], "a task per package");

    let mut m = ends();
    m["propose"] = json!({"summary": "s"});
    m["design"] = json!({"summary": "s", "packages": packages()});
    m["check"] = json!({"summary": "s", "ready": true});
    let rec = dry_through("xforge/solid", "medium", json!({"stations": m.clone()}), &["check", "verify"]).await;
    assert_eq!(rec.stations["archive"].tasks.len(), 2, "xspec archive and xarch archive");

    m["clarify"] = json!({"summary": "s", "questions": 1});
    dry_through("xforge/major", "high", json!({"stations": m}), &["clarify", "check", "verify"]).await;
}

#[tokio::test]
async fn with_review_separate_on_whoever_authored_the_run_does_not_confirm_it() {
    // Every confirmation of the three paths reads the switch.
    let p = packs();
    for f in PATHS {
        for s in &p.flow(f).unwrap().0.stations {
            for e in &s.exit {
                if let ExitCond::Confirm { separate, .. } = e {
                    assert_eq!(separate, &Some(OrSetting::Setting("${settings.review.separate}".into())), "{f} {}", s.id);
                }
            }
        }
    }
    // A dry run pins the bundle's defaults: a copy whose defaults turn it on stands for a project
    // whose governance file does.
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), &format!("cp -R {}/. .", example().join(".conix/flows").display()));
    let meta = d.path().join("xforge/bundle.yaml");
    let text = std::fs::read_to_string(&meta).unwrap();
    assert!(text.contains("review: {separate: false}"), "{text}");
    std::fs::write(&meta, text.replace("review: {separate: false}", "review: {separate: true}")).unwrap();
    let sb = sandbox_over(packs_in(d.path()));
    let mut m = ends();
    m["propose"] = json!({"summary": "s", "packages": packages()});
    let start = StartIn {
        flow: Some("xforge/quick".into()),
        inputs: json!({"title": "truncate", "risk": "low"}),
        mocks: serde_json::from_value(json!({"stations": m})).unwrap(),
        ..Default::default()
    };
    let run = sb.start(start, person()).await.unwrap();
    sb.run(&run).await.unwrap();
    let eng = sb.engine_for(&run).await.unwrap();
    assert_eq!(eng.record(&run).await.unwrap().stations["verify"].status, StationStatus::Working);
    let e = eng.confirm(&run, "verify", true, None, person()).await.unwrap_err();
    assert!(matches!(&e, FlowError::Denied(why) if why.contains("started the run")), "{e:?}");
    let o = eng.confirm(&run, "verify", true, None, Actor::new("person", "rev")).await.unwrap();
    sb.absorb(&run, o).await.unwrap();
    sb.run(&run).await.unwrap();
    assert_eq!(eng.record(&run).await.unwrap().status, RunStatus::Finished);
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
    /// The gate's verdict on a sample `files`, the run's directory `out`, the bundle's data `data`.
    async fn verdict(&self, gate: &str, files: Value, inputs: Value) -> Value {
        let r = self
            .ok(TOOL_GATE_TRY, json!({"gate": format!("xforge/{gate}"), "files": files, "output_dir": "out", "data_dir": "data", "inputs": inputs}))
            .await;
        assert_eq!(r["tried"], true, "{gate}: {r}");
        r
    }
}

fn proposal(impact: &str) -> String {
    format!(
        "# 加 truncate\n\n## 背景\n标题太长，列表里放不下。\n\n## 目标\n能把文本截到给定长度。\n\n## 非目标\n不按词边界截。\n\n\
         ## 改什么\n新增 TK-R3。\n\n## 影响面\n{impact}\n\n## 为什么选这条流程\n只加一个函数，好回退：low。\n"
    )
}

#[tokio::test]
async fn the_proposal_is_judged_by_its_sections_and_its_impact_by_what_the_start_declared() {
    let b = bench();
    let at = |text: String| json!({"out/proposal.md": text});
    assert_eq!(b.verdict("proposal-sections", at(proposal("- 无")), json!({})).await["verdict"], "pass");
    let r = b.verdict("proposal-sections", at(proposal("- 无").replace("## 非目标\n不按词边界截。\n\n", "")), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("非目标"), "{r}");
    assert_eq!(b.verdict("proposal-sections", at(proposal("TODO")), json!({})).await["verdict"], "fail");

    let none = json!({"impact": []});
    assert_eq!(b.verdict("impact", at(proposal("无")), none.clone()).await["verdict"], "pass");
    let r = b.verdict("impact", at(proposal("- 接口：加 truncate")), none.clone()).await;
    assert_eq!(r["verdict"], "fail", "nothing declared, the proposal must say none: {r}");
    let iface = json!({"impact": ["interface"]});
    let r = b.verdict("impact", at(proposal("- 接口：textkit 多一个公开函数 truncate，已有的不变")), iface.clone()).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    let r = b.verdict("impact", at(proposal("- interface")), iface.clone()).await;
    assert_eq!(r["verdict"], "fail", "a name alone says nothing: {r}");
    let r = b.verdict("impact", at(proposal("无")), json!({"impact": ["interface", "security"]})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("security"), "{r}");
    let r = b.verdict("impact", at(proposal("- 无；infra 照旧")), none).await;
    assert_eq!(r["verdict"], "pass", "only a reminder: {r}");
    assert!(r["findings"].to_string().contains("warn"), "{r}");
    let r = b.verdict("impact", at(proposal("无").replace("## 影响面\n无\n\n", "")), iface).await;
    assert_eq!(r["verdict"], "fail", "{r}");
}

const DESIGN: &str = "# 加 truncate：设计\n\n## 做法\n在 textkit 里加 truncate(text, n)。\n\n## 集成点\n无调用方，只是新函数。\n\n\
    ## 失败模式\nn 小于 1 时抛 ValueError。\n\n## 迁移与回滚\n纯新增，删掉函数即回滚。\n\n## 被否决的方案\n- 按词截断：中文没有空格，截不准。\n\n\
    ## 原则对照\n- P1：只用 str 切片。\n\n## 工作包\n一个包 core。\n";

#[tokio::test]
async fn the_design_has_every_section_and_a_rejected_alternative_with_its_reason() {
    let b = bench();
    let at = |text: &str| json!({"out/design.md": text});
    let r = b.verdict("design-sections", at(DESIGN), json!({})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    assert_eq!(r["metrics"]["rejected"], 1);
    let r = b.verdict("design-sections", at(&DESIGN.replace("## 失败模式\nn 小于 1 时抛 ValueError。\n\n", "")), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("失败模式"), "{r}");
    let r = b.verdict("design-sections", at(&DESIGN.replace("纯新增，删掉函数即回滚。", "<!-- 已有数据怎么过渡 -->")), json!({})).await;
    assert_eq!(r["verdict"], "fail", "a template's comment is no text: {r}");
    let r = b.verdict("design-sections", at(&DESIGN.replace("- 按词截断：中文没有空格，截不准。", "考虑过按词截断。")), json!({})).await;
    assert_eq!(r["verdict"], "fail", "an alternative is `- 方案：为什么不选`: {r}");
    let r = b.verdict("design-sections", at(&DESIGN.replace("一个包 core。", "TBD")), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
}

const SPEC_DELTA: &str = "added:\n  - {id: TK-R3, title: 截断, text: \"- WHEN 文本超过 n 个字符\\n- THEN 截到 n-1 个再加 …\"}\n";
const IFACE_DELTA: &str = "added:\n  - {id: fn:textkit.truncate, signature: \"truncate(text: str, n: int) -> str\", description: 截断, stability: experimental}\n";
const TESTS: &str = "def test_truncate(self):  # TK-R3\n    pass\ndef test_signature(self):  # fn:textkit.truncate\n    pass\n";

fn record(coverage: &str) -> String {
    format!("# 验证\n\n## 覆盖\n{coverage}\n\n## 连贯性\n提案、规格、实现一致。\n\n## 还剩的风险\n无。\n")
}

#[tokio::test]
async fn the_verification_record_covers_every_item_of_both_deltas_with_a_conclusion_and_a_test() {
    let b = bench();
    let both = "governance: {spec: true, interface: true}\n";
    let sample = |rec: String, tests: &str, gov: &str| {
        json!({"out/spec-delta.yaml": SPEC_DELTA, "out/interface-delta.yaml": IFACE_DELTA, "out/verification.md": rec,
               "tests/test_textkit.py": tests, ".conix/governance/xforge.yaml": gov})
    };
    let rows = "- TK-R3 通过：tests/test_textkit.py#test_truncate\n- fn:textkit.truncate 通过：签名与登记的一致";
    let r = b.verdict("verification", sample(record(rows), TESTS, both), json!({})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    assert_eq!(r["metrics"]["items"], 2);
    // The interface element is owed only when the interface baseline is governed.
    let r = b.verdict("verification", sample(record("- TK-R3 通过：单测"), TESTS, both), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("fn:textkit.truncate"), "{r}");
    let r = b.verdict("verification", sample(record("- TK-R3 通过：单测"), TESTS, "governance: {interface: false}\n"), json!({})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    // A conclusion outside 覆盖 is no coverage; 未通过 fails, 未验证 is the owner's to judge.
    let outside = "# 验证\n\n- TK-R3 通过\n- fn:textkit.truncate 通过\n\n## 覆盖\n见上\n\n## 连贯性\n一致。\n".to_string();
    assert_eq!(b.verdict("verification", sample(outside, TESTS, both), json!({})).await["verdict"], "fail");
    let failed = "- TK-R3 未通过：多截了一个字\n- fn:textkit.truncate 通过：一致";
    assert_eq!(b.verdict("verification", sample(record(failed), TESTS, both), json!({})).await["verdict"], "fail");
    let unverified = "- TK-R3 未验证：没有环境\n- fn:textkit.truncate 通过：一致";
    let r = b.verdict("verification", sample(record(unverified), TESTS, both), json!({})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    assert!(r["findings"].to_string().contains("warn"), "{r}");
    // Every item has a test tagged with it; TK-R30 is not TK-R3.
    let r = b.verdict("verification", sample(record(rows), "def test_x(self):  # TK-R30\n", both), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert_eq!(r["findings"].as_array().unwrap().iter().filter(|f| f["rule"] == "tested").count(), 2, "{r}");
    // Coherence is a section of its own, not empty.
    let r = b.verdict("verification", sample(record(rows).replace("提案、规格、实现一致。", ""), TESTS, both), json!({})).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r["findings"].to_string().contains("连贯性"), "{r}");
}

#[tokio::test]
async fn the_projects_verify_is_run_and_its_absence_said() {
    let b = bench();
    let root = b.project.path();
    let at = json!({"gate": "xforge/unit-tests"});
    let r = b.ok(TOOL_GATE_TRY, at.clone()).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    std::fs::write(root.join("tests/test_broken.py"), "import unittest\nclass T(unittest.TestCase):\n    def test_x(self):\n        self.fail('boom')\n").unwrap();
    assert_eq!(b.ok(TOOL_GATE_TRY, at.clone()).await["verdict"], "fail");
    // A package's own verification tried outside a run: its plan runs, and says it has no work item to judge.
    let r = b.ok(TOOL_GATE_TRY, json!({"gate": "xforge/package-tests"})).await;
    assert_eq!(r["verdict"], "error", "{r}");
    assert!(r["summary"].as_str().unwrap().contains("params.item"), "{r}");
    std::fs::remove_file(root.join(".conix/governance/verify.sh")).unwrap();
    let r = b.ok(TOOL_GATE_TRY, at).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    assert!(r.to_string().contains("verify.sh"), "{r}");
}
