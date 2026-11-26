//! The spec side of the xforge bundle (docs/design/xforge.md §1, §2.1, §3): `xspec` with the
//! governance switch, the WHEN / THEN shape, the other in-flight runs of the bundle (`pending`, the
//! numbering, conflicts), `archive`, and the gates `spec-delta`, `spec-delta-shape`, `archived`. No model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use harnix_flow::author::*;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig, ScriptedHost, SessionId, Turn};
use harnix_flow::*;
use serde_json::{json, Value};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

fn packs() -> DirPacks {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    DirPacks::new([example().join(".conix/flows")]).with_catalog(catalog)
}

fn sh(dir: &Path, cmd: &str) {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
}

/// A copy of the example as a committed git repository: where a person would start a change.
fn copy_of_example() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    sh(d.path(), &format!("cp -R {}/. . && git init -q && git add -A && git -c user.email=x@y -c user.name=x commit -qm base", example().display()));
    d
}

const WT: &str = "- WHEN 对 None 调用 `word_count(text)`\n- THEN 返回 0";
const SHAPED: &str = "- WHEN 文本超过 n 个字符\n- THEN 截到 n-1 个再加 …";

struct World {
    project: tempfile::TempDir,
    root: PathBuf,
    rn: Arc<Runner>,
    _data: tempfile::TempDir,
}

async fn world() -> World {
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
    host.trust(&root, true);
    let rn = Runner::new(engine, host, cfg);
    rn.start().await;
    World { project, root, rn, _data: data }
}

impl World {
    /// Start a run of the bundle; its directory is where the commands write.
    async fn start(&self, title: &str) -> (String, PathBuf) {
        let o = self
            .rn
            .start_run(
                CreateIn { flow: "xforge/solid".into(), subject: title.into(), inputs: json!({"title": title}), project_root: Some(self.root.to_string_lossy().into()), ..Default::default() },
                Actor::device("d"),
            )
            .await
            .unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        self.rn.apply(o).await;
        let rec = self.rn.engine().record(&run).await.unwrap();
        (run, PathBuf::from(rec.output_dir.unwrap()))
    }

    async fn cmd(&self, run: Option<&str>, by: CommandBy, args: &[&str]) -> std::result::Result<String, String> {
        let mut c = CommandCall::new("xforge", "xspec", args.iter().map(|a| a.to_string()).collect(), by);
        c.run = run.map(String::from);
        c.project_root = Some(self.root.clone());
        self.rn.run_command(c).await.map(|r| r.text)
    }

    async fn ok(&self, run: &str, args: &[&str]) -> String {
        self.cmd(Some(run), CommandBy::model("s"), args).await.unwrap_or_else(|e| panic!("{args:?}: {e}"))
    }

    async fn err(&self, run: &str, args: &[&str]) -> String {
        self.cmd(Some(run), CommandBy::model("s"), args).await.expect_err(&format!("{args:?} should fail"))
    }

    fn specs(&self) -> String {
        std::fs::read_to_string(self.root.join("xforge/specs/textkit.md")).unwrap()
    }

    fn governance(&self, yaml: &str) {
        std::fs::write(self.root.join(".conix/governance/xforge.yaml"), yaml).unwrap();
    }
}

#[tokio::test]
async fn the_governance_switch_closes_the_writing_subcommands_and_owes_nothing() {
    let w = world().await;
    let (run, _) = w.start("t").await;
    w.governance("governance:\n  spec: false\n  interface: true\n");
    for args in [&["add", "textkit", "t", SHAPED][..], &["modify", "TK-R2", WT], &["remove", "TK-R1"], &["revert", "TK-R1"], &["archive"], &["init", "x", "XX", "标题"]] {
        let e = w.err(&run, args).await;
        assert!(e.contains("这个项目没开规格基线") && e.contains(".conix/governance/xforge.yaml"), "{args:?}: {e}");
    }
    // What reads still reads, and `check` says it owes nothing.
    assert!(w.ok(&run, &["list"]).await.contains("textkit"));
    assert!(w.ok(&run, &["get", "TK-R1"]).await.contains("slugify"));
    assert!(w.ok(&run, &["check"]).await.contains("不欠"));
    assert!(std::fs::read_to_string(w.root.join("xforge/specs/textkit.md")).unwrap() == w.specs());
    // On again (and with no file at all): it writes.
    w.governance("governance:\n  spec: true\n");
    assert!(w.ok(&run, &["add", "textkit", "截断", SHAPED]).await.contains("TK-R3"));
    std::fs::remove_file(w.root.join(".conix/governance/xforge.yaml")).unwrap();
    assert!(w.ok(&run, &["modify", "TK-R2", WT]).await.contains("TK-R2"));
    let _ = &w.project;
}

#[tokio::test]
async fn a_body_without_when_then_is_refused_in_either_language() {
    let w = world().await;
    let (run, out) = w.start("t").await;
    for body in ["truncate 截断超长的文本", "- WHEN 文本很长", "- THEN 截断", "WHEN a\nTHEN b", "- 当文本很长时"] {
        let e = w.err(&run, &["add", "textkit", "截断", body]).await;
        assert!(e.contains("WHEN / THEN") && e.contains("- WHEN <条件>") && e.contains("- THEN"), "{body}: {e}");
    }
    let e = w.err(&run, &["modify", "TK-R1", "只是一句话，没有条目"]).await;
    assert!(e.contains("没有 WHEN 条目") && e.contains("没有 THEN 条目"), "{e}");
    assert!(!out.join("spec-delta.yaml").exists(), "nothing was written");
    assert!(w.ok(&run, &["add", "textkit", "截断", SHAPED]).await.contains("TK-R3"));
    assert!(w.ok(&run, &["add", "textkit", "中文", "- 当 文本为空 时\n- 应 返回空串"]).await.contains("TK-R4"));
    // A literal `\n` from a command line is a newline.
    assert!(w.ok(&run, &["add", "textkit", "字面", "- WHEN a\\n- THEN b"]).await.contains("TK-R5"));
    assert!(w.ok(&run, &["check"]).await.contains("对得上基线"));
    // The shipped baseline is itself in shape: its own bodies pass what a delta must.
    let delta = std::fs::read_to_string(out.join("spec-delta.yaml")).unwrap();
    assert!(delta.contains("TK-R3") && delta.contains("TK-R5"), "{delta}");
    let help = w.cmd(None, CommandBy::model("s"), &["help"]).await.unwrap();
    assert!(help.contains("xspec pending") && help.contains("WHEN / THEN"), "{help}");
}

#[tokio::test]
async fn in_flight_runs_see_each_other_number_after_each_other_and_conflict() {
    let w = world().await;
    let (a, _) = w.start("甲").await;
    let (b, _) = w.start("乙").await;
    assert_eq!(w.ok(&a, &["pending"]).await, "别的在途运行没有动规格");

    // A adds TK-R3 and modifies TK-R2; B sees both, and numbers its own addition after A's.
    assert!(w.ok(&a, &["add", "textkit", "截断", SHAPED]).await.contains("TK-R3"));
    w.ok(&a, &["modify", "TK-R2", WT]).await;
    let pending = w.ok(&b, &["pending"]).await;
    assert!(pending.contains(&a) && pending.contains("甲") && pending.contains("新增 TK-R3") && pending.contains("修改 TK-R2"), "{pending}");
    assert_eq!(w.ok(&a, &["pending"]).await, "别的在途运行没有动规格", "a run is not its own other");
    assert!(w.ok(&b, &["add", "textkit", "填充", "- WHEN 文本短于 n\n- THEN 用空格补齐"]).await.contains("TK-R4"));

    // No clash yet; then B touches what A touches.
    assert!(w.ok(&b, &["check"]).await.contains("对得上基线"));
    assert!(w.ok(&a, &["check"]).await.contains("对得上基线"));
    w.ok(&b, &["modify", "TK-R2", "- WHEN 对 None 调用 `word_count(text)`\n- THEN 抛 TypeError"]).await;
    for (run, other) in [(&b, &a), (&a, &b)] {
        let c = w.ok(run, &["check"]).await;
        assert!(c.contains("冲突") && c.contains("TK-R2") && c.contains(other.as_str()), "{c}");
        assert!(!c.contains("TK-R3 我") && !c.contains("TK-R4 我"), "only the shared id: {c}");
    }
    let e = w.err(&b, &["archive"]).await;
    assert!(e.contains("不能并") && e.contains("冲突"), "{e}");
    assert!(!w.specs().contains("TypeError"));
    // Removing what the other adds is a clash too; reverting settles it.
    w.ok(&b, &["revert", "TK-R2"]).await;
    assert!(w.ok(&b, &["check"]).await.contains("对得上基线"));
    w.ok(&b, &["remove", "TK-R1"]).await;
    w.ok(&a, &["remove", "TK-R1"]).await;
    assert!(w.ok(&a, &["check"]).await.contains("冲突") && w.ok(&a, &["pending"]).await.contains("删除 TK-R1"));
    // Reading `pending` works for a model outside any run too, and lists both.
    let all = w.cmd(None, CommandBy::person("me"), &["pending"]).await.unwrap();
    assert!(all.contains(&a) && all.contains(&b), "{all}");
}

#[tokio::test]
async fn archive_merges_the_spec_delta_and_the_archived_gate_agrees() {
    let w = world().await;
    let (run, _) = w.start("t").await;
    w.ok(&run, &["add", "textkit", "截断", SHAPED]).await;
    w.ok(&run, &["modify", "TK-R2", WT, "--title", "数单词（空值）"]).await;
    w.ok(&run, &["remove", "TK-R1"]).await;
    let t = w.ok(&run, &["archive"]).await;
    assert!(t.contains("textkit.md"), "{t}");
    let s = w.specs();
    assert!(s.contains("### TK-R3 截断\n- WHEN 文本超过 n 个字符") && s.contains("### TK-R2 数单词（空值）\n- WHEN 对 None") && !s.contains("TK-R1") && !s.contains("全是空白"), "{s}");
    assert!(w.ok(&run, &["list", "textkit"]).await.contains("TK-R3 截断"));
    // An empty delta is not mergeable.
    let (empty, _) = w.start("空").await;
    assert!(w.err(&empty, &["archive"]).await.contains("增量是空的"));
}

// ---- the gates, tried on a sample directory ----

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

struct Gates {
    tools: AuthorTools,
    _rn: Arc<Runner>,
    _project: tempfile::TempDir,
}

fn gates() -> Gates {
    let cfg = RunnerConfig { command_gates: true, profiles: vec!["code".into()], main_profile: Some("code".into()), submit_profiles: ["code".to_string()].into(), ..Default::default() };
    let layout = ProjectLayout::new(".conix", std::env::temp_dir().join("xforge-spec-worktrees"), "conix/flow").with_artifacts_dir("xforge");
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
    Gates { tools: AuthorTools::new(&rn, sandbox, host), _rn: rn, _project: project }
}

async fn verdict(t: &AuthorTools, gate: &str, files: Value) -> Value {
    let r = t
        .call(&"s".to_string(), TOOL_GATE_TRY, json!({"gate": format!("xforge/{gate}"), "files": files, "output_dir": "out", "data_dir": "data", "outputs": {}}), &Actor::new(CONVERSATION_KIND, "s"))
        .await;
    assert!(!r.is_error, "{gate}: {}", r.text);
    let v: Value = serde_json::from_str(&r.text).unwrap();
    assert_eq!(v["tried"], true, "{v}");
    v
}

#[tokio::test]
async fn the_spec_gates_judge_shape_and_baseline_and_owe_nothing_when_off() {
    let g = gates();
    let t = &g.tools;
    let spec = std::fs::read_to_string(example().join("xforge/specs/textkit.md")).unwrap();
    let good = format!("added:\n  - {{id: TK-R3, title: 截断, text: \"{}\"}}\nmodified:\n  - {{id: TK-R2, text: \"{}\"}}\n", SHAPED.replace('\n', "\\n"), WT.replace('\n', "\\n"));
    let on = |delta: &str| json!({"data/specs/textkit.md": spec, "out/spec-delta.yaml": delta});
    assert_eq!(verdict(t, "spec-delta-shape", on(&good)).await["verdict"], "pass");
    assert_eq!(verdict(t, "spec-delta", on(&good)).await["verdict"], "pass");
    // The shape gate: empty, wrong id, short text.
    assert_eq!(verdict(t, "spec-delta-shape", on("added: []\n")).await["verdict"], "fail");
    assert_eq!(verdict(t, "spec-delta-shape", on("added:\n  - {id: bad, title: x, text: 十个字以上的正文正文正文}\n")).await["verdict"], "fail");
    assert_eq!(verdict(t, "spec-delta-shape", json!({})).await["verdict"], "fail", "no delta at all");
    // WHEN / THEN, per requirement.
    let plain = "added:\n  - {id: TK-R3, title: 截断, text: 超过 n 个字符时截断并加省略号}\nmodified:\n  - {id: TK-R2, text: 数空白以外的片段个数，None 为零}\n";
    let r = verdict(t, "spec-delta", on(plain)).await;
    assert_eq!(r["verdict"], "fail", "{r}");
    let f = r["findings"].to_string();
    assert!(f.contains("added 的 TK-R3") && f.contains("modified 的 TK-R2") && f.contains("WHEN / THEN"), "{r}");
    let zh = "added:\n  - {id: TK-R3, title: 截断, text: \"- 当 文本过长 时\\n- 应 截断\"}\n";
    assert_eq!(verdict(t, "spec-delta", on(zh)).await["verdict"], "pass");
    // Against the baseline, as before.
    let r = verdict(t, "spec-delta", on("added:\n  - {id: TK-R1, title: x, text: \"- WHEN a\\n- THEN b\"}\nmodified:\n  - {id: TK-R9, text: \"- WHEN a\\n- THEN b\"}\n")).await;
    assert_eq!(r["verdict"], "fail");
    assert!(r["findings"].to_string().contains("TK-R9"), "{r}");

    // Archived: not merged, then merged (the spec carries the delta's bodies).
    assert_eq!(verdict(t, "archived", on(&good)).await["verdict"], "fail");
    let merged = spec.replace(&spec[spec.find("### TK-R2").unwrap()..], &format!("### TK-R2 数单词\n{WT}\n\n### TK-R3 截断\n{SHAPED}\n"));
    let r = verdict(t, "archived", json!({"data/specs/textkit.md": merged, "out/spec-delta.yaml": good})).await;
    assert_eq!(r["verdict"], "pass", "{r}");

    // Switched off: all three owe nothing, whatever the delta says.
    let off = json!({".conix/governance/xforge.yaml": "governance:\n  spec: false\n", "data/specs/textkit.md": spec, "out/spec-delta.yaml": "added: []\n"});
    for g in ["spec-delta-shape", "spec-delta", "archived"] {
        let r = verdict(t, g, off.clone()).await;
        assert_eq!(r["verdict"], "pass", "{g}: {r}");
        assert!(r["summary"].to_string().contains("governance.spec=false"), "{g}: {r}");
    }
}
