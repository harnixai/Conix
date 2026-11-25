//! The demo project of the repository (`examples/demo`, in Conix): every flow and part of it holds
//! together, its flows dry-run to the person's confirmation, and each of its gates passes a good
//! delivery and fails a bad one — tried with `flow_gate_try`, the code a run uses.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use harnix_flow::author::*;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, SessionId};
use harnix_flow::*;
use serde_json::{json, Value};

fn demo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/demo")
}

fn layout() -> ProjectLayout {
    ProjectLayout::new(".conix", std::env::temp_dir().join("demo-worktrees"), "conix/flow")
}

/// The demo's own flows over what Conix offers: the `code` scene and command gates.
fn packs() -> ProjectPacks {
    let base = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates();
    ProjectPacks::new(Arc::new(base), demo(), &layout(), true)
}

/// A copy of the demo, as a person would make one to run it.
fn copy_of_demo() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let st = std::process::Command::new("cp").arg("-R").arg(format!("{}/.", demo().display())).arg(d.path()).status().unwrap();
    assert!(st.success());
    d
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
    /// A trusted copy of the demo: the conversation works in it, and command gates run there.
    project: tempfile::TempDir,
}

fn bench() -> Bench {
    let cfg = RunnerConfig {
        command_gates: true,
        profiles: vec!["code".into()],
        main_profile: Some("code".into()),
        submit_profiles: ["code".to_string()].into(),
        guarded: vec![],
        ..Default::default()
    };
    let packs = Arc::new(CatalogPacks::over(&cfg, vec![], Arc::new(MemPacks::new().writable())));
    packs.set_layout(layout());
    let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), packs.clone()).with_main_scene("code").with_layout(layout()));
    let scripted = ScriptedHost::new();
    let project = copy_of_demo();
    let root = project.path().to_string_lossy().into_owned();
    scripted.trust(project.path(), true);
    let rn = Runner::new(engine, scripted, cfg);
    let sandbox = Arc::new(Sandbox::new(
        packs,
        SandboxConfig { guarded: vec![], submit_scenes: vec!["code".into()], main_scene: Some("code".into()) },
    ));
    let host = Arc::new(Convo::default());
    host.projects.lock().unwrap().insert("s".into(), root);
    let tools = AuthorTools::new(&rn, sandbox, host);
    Bench { tools, _rn: rn, project }
}

impl Bench {
    async fn ok(&self, session: &str, tool: &str, input: Value) -> Value {
        let r = self.tools.call(&session.to_string(), tool, input, &Actor::new(CONVERSATION_KIND, "s")).await;
        assert!(!r.is_error, "{tool}: {}", r.text);
        serde_json::from_str(&r.text).unwrap_or(Value::String(r.text))
    }

    /// The gate's verdict on `files`, the run's output directory being `out`.
    async fn verdict(&self, gate: &str, files: Value, outputs: Value) -> (String, Value) {
        let r = self
            .ok("s", TOOL_GATE_TRY, json!({"gate": gate, "files": files, "output_dir": "out", "outputs": outputs}))
            .await;
        assert_eq!(r["tried"], true, "{gate}: {r}");
        (r["verdict"].as_str().unwrap_or_default().to_string(), r)
    }
}

const FLOWS: &[&str] = &["brief", "digest", "digest/note", "publish"];
/// Each bundle's gates, by full name: the demo's three bundles are each self-contained.
const GATES: &[&str] = &[
    "brief/outline-shape", "brief/sections-written", "brief/brief-shape", "brief/brief-stats", "brief/facts-shape",
    "brief/policy-shape", "brief/reviews-verified", "brief/governance", "brief/untouched", "digest/digest-complete",
    "publish/publish-ready",
];

#[test]
fn every_flow_and_part_of_the_demo_holds_together() {
    let packs = packs();
    for f in FLOWS {
        let (def, _) = packs.flow(f).unwrap_or_else(|| panic!("no flow {f} in the demo"));
        assert_eq!(validate(&def, &packs), vec![], "{f}");
    }
    for g in GATES {
        let (text, _) = packs.part(PartKind::Gate, g).unwrap_or_else(|| panic!("no gate {g}"));
        assert_eq!(part_problems(PartKind::Gate, g, &text, &packs), Vec::<String>::new(), "{g}");
    }
    // The jobs name a flow the demo has.
    let jobs: Value = serde_json::from_str(&std::fs::read_to_string(demo().join(".conix/jobs.json")).unwrap()).unwrap();
    for j in jobs.as_array().unwrap() {
        if let Some(flow) = j["kind"]["flow"]["flow"].as_str() {
            assert!(FLOWS.contains(&flow), "job {} names flow {flow}", j["id"]);
        }
    }
}

#[tokio::test]
async fn the_brief_dry_runs_to_the_editors_sign_off() {
    let b = bench();
    let sections = json!([{"id": "timeouts", "title": "超时", "sources": ["notes/timeouts.md"]}, {"id": "retries", "title": "重试", "sources": ["notes/retries.md"]}]);
    let dry = b
        .ok("s", TOOL_DRY_RUN, json!({"flow": "brief", "mocks": {"stations": {
            "outline": {"sections": sections},
            "sections": {"file": "x.md"},
            "assemble": {"file": "brief.md", "summary": "s"},
            "reviews": {"file": "reviews/x.jsonl", "count": 3}}}}))
        .await;
    assert_eq!(dry["stations"]["sign-off"], "working", "{dry}");
    let waits = dry.to_string();
    assert!(waits.contains("confirm"), "{dry}");
}

/// YAML, as the outline station writes it.
const OUTLINE: &str = "title: 服务可靠性\nsections:\n  - {id: timeouts, title: 超时, sources: [notes/timeouts.md]}\n  - id: retries\n    title: 重试与缓存\n    sources: [notes/retries.md, notes/caching.md]\n";

fn good_brief() -> String {
    let body = "每一次跨进程的调用都要有超时，重试只针对幂等操作并加退避与抖动，缓存要防击穿并能降级。".repeat(6);
    format!(
        "# 服务可靠性\n\n## 摘要\n{body}\n\n## 超时\n{body}\n\n## 重试与缓存\n{body}\n\n## 结论\n- 给每个调用定超时。\n\n## 来源\n- notes/timeouts.md\n- notes/retries.md\n- notes/caching.md\n"
    )
}

/// One line per claim, quoting the notes of the demo as they are.
const FACTS: &str = concat!(
    r#"{"claim": "超时由外到内递减", "verdict": "supported", "source": "notes/timeouts.md", "quote": "超时按调用链从外到内递减：网关 10 秒，服务之间 3 秒"}"#, "\n",
    r#"{"claim": "只重试幂等操作", "verdict": "supported", "source": "notes/retries.md", "quote": "只重试幂等的操作，或者带幂等键的写操作。"}"#, "\n",
    r#"{"claim": "缓存要防击穿", "verdict": "supported", "source": "notes/caching.md", "quote": "用单飞（同一个键只放一个请求去回源）"}"#, "\n",
);

const POLICY: &str = concat!(
    r#"{"rule": "no-new-facts", "verdict": "ok", "message": "都出自笔记"}"#, "\n",
    r#"{"rule": "actionable", "verdict": "ok", "message": "结论能照着做"}"#, "\n",
    r#"{"rule": "plain", "verdict": "violated", "message": "摘要有一句堆了形容词"}"#, "\n",
);

/// The files a run would have when the station `govern` judges, with `brief` as its brief: a sample
/// is all a trial reads, so the demo's governance file goes in with it.
fn delivered(brief: &str, facts: &str, policy: &str) -> Value {
    let rules = std::fs::read_to_string(demo().join(".conix/governance/policy.yaml")).unwrap();
    json!({".conix/governance/policy.yaml": rules, "out/outline.yaml": OUTLINE, "out/brief.md": brief,
        "out/reviews/facts.jsonl": facts, "out/reviews/policy.jsonl": policy})
}

// Each gate of the demo passes a good delivery and fails a bad one: one test a kind of gate.

#[tokio::test]
async fn the_demos_check_gates_judge_yaml_jsonl_files_and_text() {
    let b = bench();
    // check: schema, on YAML
    let (v, _) = b.verdict("brief/outline-shape", json!({"out/outline.yaml": OUTLINE}), json!({})).await;
    assert_eq!(v, "pass");
    let bad = "title: x\nsections:\n  - {id: Bad Id, title: t, sources: [timeouts.md]}\n";
    let (v, r) = b.verdict("brief/outline-shape", json!({"out/outline.yaml": bad}), json!({})).await;
    assert_eq!(v, "fail", "{r}");
    assert!(r["findings"].as_array().unwrap().len() >= 2, "too few sections, a bad id, a bad source: {r}");

    // check: files
    let (v, _) = b.verdict("brief/sections-written", json!({"out/sections/timeouts.md": "## 超时\n正文"}), json!({})).await;
    assert_eq!(v, "pass");
    let (v, r) = b.verdict("brief/sections-written", json!({"out/sections/timeouts.md": "## 超时\nTODO"}), json!({})).await;
    assert_eq!(v, "fail", "{r}");
    let (v, _) = b.verdict("brief/sections-written", json!({"out/other.md": "x"}), json!({})).await;
    assert_eq!(v, "fail", "no section written at all");

    // check: text
    let (v, _) = b.verdict("brief/brief-shape", json!({"out/brief.md": good_brief()}), json!({})).await;
    assert_eq!(v, "pass");
    let (v, r) = b.verdict("brief/brief-shape", json!({"out/brief.md": "# t\n\n## 摘要\nTODO\n"}), json!({})).await;
    assert_eq!(v, "fail", "{r}");

    // check: schema, on JSONL
    let (v, r) = b.verdict("brief/facts-shape", json!({"out/reviews/facts.jsonl": FACTS}), json!({})).await;
    assert_eq!(v, "pass", "{r}");
    let (v, r) = b.verdict("brief/facts-shape", json!({"out/reviews/facts.jsonl": format!("[{}]", FACTS.lines().collect::<Vec<_>>().join(","))}), json!({})).await;
    assert_eq!(v, "fail", "an array on one line is one row, not three: {r}");
    let (v, _) = b.verdict("brief/policy-shape", json!({"out/reviews/policy.jsonl": POLICY}), json!({})).await;
    assert_eq!(v, "pass");
    let (v, r) = b.verdict("brief/policy-shape", json!({"out/reviews/policy.jsonl": POLICY.replace("violated", "meh")}), json!({})).await;
    assert_eq!(v, "fail", "{r}");

    // check: output, on the child runs' results.
    let each = json!({"each": [{"path": "notes/a.md", "line": "一句足够长的摘要文字"}, null]});
    let (v, _) = b.verdict("digest/digest-complete", json!({}), each).await;
    assert_eq!(v, "pass");
    let (v, _) = b.verdict("digest/digest-complete", json!({}), json!({"each": [null, null]})).await;
    assert_eq!(v, "fail", "no child delivered");
}

#[tokio::test]
async fn the_demos_script_gate_concludes_from_the_governance_file() {
    let b = bench();
    let (v, r) = b.verdict("brief/governance", delivered(&good_brief(), FACTS, POLICY), json!({})).await;
    assert_eq!(v, "pass", "{r}");
    assert_eq!((r["metrics"]["sources"].as_u64(), r["metrics"]["violated"].as_u64()), (Some(3), Some(1)), "{r}");
    assert!(r["summary"].as_str().unwrap().contains("可以签发"), "a warn rule broken is a reminder: {r}");
    let broken = good_brief().replace("- notes/caching.md\n", "").replace("## 重试与缓存", "## 重试") + "\n这样做 100% 不会出事。\n";
    let unsupported = FACTS.to_string()
        + "{\"claim\": \"a\", \"verdict\": \"unsupported\", \"source\": \"\", \"quote\": \"\"}\n"
        + "{\"claim\": \"b\", \"verdict\": \"unsupported\", \"source\": \"\", \"quote\": \"\"}\n";
    let policy = POLICY.replace(r#""no-new-facts", "verdict": "ok""#, r#""no-new-facts", "verdict": "violated""#).replace("actionable", "other");
    let (v, r) = b.verdict("brief/governance", delivered(&broken, &unsupported, &policy), json!({})).await;
    assert_eq!(v, "fail", "{r}");
    let text = r["findings"].to_string();
    for what in ["notes/caching.md", "重试与缓存", "100%", "没有依据", "no-new-facts", "actionable 没有判"] {
        assert!(text.contains(what), "{what}: {r}");
    }
    assert!(r["summary"].as_str().unwrap().contains("不可签发"), "{r}");
    let (v, _) = b.verdict("brief/governance", json!({"out/brief.md": good_brief()}), json!({})).await;
    assert_eq!(v, "error", "no outline: nothing to judge");
}

#[tokio::test]
async fn the_demos_command_gates_run_its_own_tool() {
    let b = bench();
    // command: the project's own tool, in the project (trusted), on a directory of it.
    let out = b.project.path().join("out");
    std::fs::create_dir_all(out.join("reviews")).unwrap();
    let at = json!({"output_dir": out.to_string_lossy()});
    let gate = |g: &str| {
        let mut i = at.clone();
        i["gate"] = json!(g);
        i
    };
    std::fs::write(out.join("brief.md"), good_brief()).unwrap();
    let r = b.ok("s", TOOL_GATE_TRY, gate("brief/brief-stats")).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    assert!(r["metrics"]["chars"].as_u64().unwrap() >= 300, "{r}");
    std::fs::write(out.join("brief.md"), "# 短\n").unwrap();
    let r = b.ok("s", TOOL_GATE_TRY, gate("brief/brief-stats")).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    std::fs::write(out.join("reviews/facts.jsonl"), FACTS).unwrap();
    let r = b.ok("s", TOOL_GATE_TRY, gate("brief/reviews-verified")).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    assert_eq!(r["metrics"]["supported"], 3, "{r}");
    let made_up = FACTS.replace("网关 10 秒", "网关 30 秒");
    std::fs::write(out.join("reviews/facts.jsonl"), made_up).unwrap();
    let r = b.ok("s", TOOL_GATE_TRY, gate("brief/reviews-verified")).await;
    assert_eq!(r["verdict"], "fail", "a quote the note does not have: {r}");
    assert!(r["findings"].to_string().contains("notes/timeouts.md"), "{r}");
}
