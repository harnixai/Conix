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

const FLOWS: &[&str] = &["brief", "digest", "digest-note", "publish"];

#[test]
fn every_flow_and_part_of_the_demo_holds_together() {
    let packs = packs();
    for f in FLOWS {
        let (def, _) = packs.flow(f).unwrap_or_else(|| panic!("no flow {f} in the demo"));
        assert_eq!(validate(&def, &packs), vec![], "{f}");
    }
    for g in ["outline-shape", "sections-written", "brief-shape", "sources-cited", "brief-stats", "untouched", "digest-complete", "publish-ready"] {
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
async fn the_brief_dry_runs_to_the_editors_confirmation() {
    let b = bench();
    let sections = json!([{"id": "timeouts", "title": "超时", "sources": ["notes/timeouts.md"]}, {"id": "retries", "title": "重试", "sources": ["notes/retries.md"]}]);
    let dry = b
        .ok("s", TOOL_DRY_RUN, json!({"flow": "brief", "mocks": {"stations": {
            "outline": {"sections": sections},
            "sections": {"file": "x.md"},
            "assemble": {"file": "brief.md", "summary": "s"}}}}))
        .await;
    assert_eq!(dry["stations"]["review"], "working", "{dry}");
    let waits = dry.to_string();
    assert!(waits.contains("confirm"), "{dry}");
}

const OUTLINE: &str = r#"{"title": "服务可靠性", "sections": [
  {"id": "timeouts", "title": "超时", "sources": ["notes/timeouts.md"]},
  {"id": "retries", "title": "重试与缓存", "sources": ["notes/retries.md", "notes/caching.md"]}]}"#;

fn good_brief() -> String {
    let body = "每一次跨进程的调用都要有超时，重试只针对幂等操作并加退避与抖动，缓存要防击穿并能降级。".repeat(6);
    format!(
        "# 服务可靠性\n\n## 摘要\n{body}\n\n## 超时\n{body}\n\n## 重试与缓存\n{body}\n\n## 结论\n- 给每个调用定超时。\n\n## 来源\n- notes/timeouts.md\n- notes/retries.md\n- notes/caching.md\n"
    )
}

#[tokio::test]
async fn each_gate_of_the_demo_passes_a_good_delivery_and_fails_a_bad_one() {
    let b = bench();
    // check: schema
    let (v, _) = b.verdict("outline-shape", json!({"out/outline.json": OUTLINE}), json!({})).await;
    assert_eq!(v, "pass");
    let bad = r#"{"title": "x", "sections": [{"id": "Bad Id", "title": "t", "sources": ["timeouts.md"]}]}"#;
    let (v, r) = b.verdict("outline-shape", json!({"out/outline.json": bad}), json!({})).await;
    assert_eq!(v, "fail", "{r}");
    assert!(r["findings"].as_array().unwrap().len() >= 2, "too few sections, a bad id, a bad source: {r}");

    // check: files
    let (v, _) = b.verdict("sections-written", json!({"out/sections/timeouts.md": "## 超时\n正文"}), json!({})).await;
    assert_eq!(v, "pass");
    let (v, r) = b.verdict("sections-written", json!({"out/sections/timeouts.md": "## 超时\nTODO"}), json!({})).await;
    assert_eq!(v, "fail", "{r}");
    let (v, _) = b.verdict("sections-written", json!({"out/other.md": "x"}), json!({})).await;
    assert_eq!(v, "fail", "no section written at all");

    // check: text
    let (v, _) = b.verdict("brief-shape", json!({"out/brief.md": good_brief()}), json!({})).await;
    assert_eq!(v, "pass");
    let (v, r) = b.verdict("brief-shape", json!({"out/brief.md": "# t\n\n## 摘要\nTODO\n"}), json!({})).await;
    assert_eq!(v, "fail", "{r}");

    // script
    let files = json!({"out/outline.json": OUTLINE, "out/brief.md": good_brief()});
    let (v, r) = b.verdict("sources-cited", files, json!({})).await;
    assert_eq!(v, "pass", "{r}");
    assert_eq!(r["metrics"]["sources"], 3);
    let missing = good_brief().replace("- notes/caching.md\n", "").replace("## 重试与缓存", "## 重试");
    let (v, r) = b.verdict("sources-cited", json!({"out/outline.json": OUTLINE, "out/brief.md": missing}), json!({})).await;
    assert_eq!(v, "fail", "{r}");
    let text = r["findings"].to_string();
    assert!(text.contains("notes/caching.md") && text.contains("重试与缓存"), "{r}");
    let (v, _) = b.verdict("sources-cited", json!({"out/brief.md": good_brief()}), json!({})).await;
    assert_eq!(v, "error", "no outline: nothing to judge");

    // command: in the project (trusted), on a directory of it.
    let out = b.project.path().join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("brief.md"), good_brief()).unwrap();
    let r = b.ok("s", TOOL_GATE_TRY, json!({"gate": "brief-stats", "output_dir": out.to_string_lossy()})).await;
    assert_eq!(r["verdict"], "pass", "{r}");
    assert!(r["metrics"]["chars"].as_u64().unwrap() >= 300, "{r}");
    std::fs::write(out.join("brief.md"), "# 短\n").unwrap();
    let r = b.ok("s", TOOL_GATE_TRY, json!({"gate": "brief-stats", "output_dir": out.to_string_lossy()})).await;
    assert_eq!(r["verdict"], "fail", "{r}");

    // check: output, on the child runs' results.
    let each = json!({"each": [{"path": "notes/a.md", "line": "一句足够长的摘要文字"}, null]});
    let (v, _) = b.verdict("digest-complete", json!({}), each).await;
    assert_eq!(v, "pass");
    let (v, _) = b.verdict("digest-complete", json!({}), json!({"each": [null, null]})).await;
    assert_eq!(v, "fail", "no child delivered");
}
