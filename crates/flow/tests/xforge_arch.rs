//! The interface baseline of the xforge bundle (`xarch`, the three `interface-*` gates, in Conix;
//! docs/design/xforge.md §2.1, §2.2, §3, §4), without a model: the command's subcommands and the
//! lifecycle rules in a run of a small bundle that carries the same files, the gates on samples,
//! another run's delta seen as a conflict, and `archive` merging the delta into the baseline.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use harnix_flow::author::*;
use harnix_flow::dryrun::{Sandbox, SandboxConfig};
use harnix_flow::runner::{CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig, ScriptedHost, SessionId, Turn, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

const BUNDLE: &str = "examples/xforge/.conix/flows/xforge";

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

const GOV_ON: &str = "governance:\n  spec: true\n  interface: true\n";
const GOV_OFF: &str = "governance:\n  spec: true\n  interface: false\n";

/// A legacy module: one element of each state, for the rules that start from the baseline.
const LEGACY: &str = "module: legacy\ntitle: 旧模块\npaths: [\"src/legacy/**\"]\nelements:\n  - {id: \"fn:legacy.old\", signature: \"old() -> int\", description: 旧的, stability: deprecated}\n  - {id: \"fn:legacy.fresh\", signature: \"fresh() -> int\", description: 试验中的, stability: experimental}\n  - {id: \"fn:legacy.bare\", signature: \"bare() -> int\", description: 没有状态的按 stable 读}\n";

// ---- the command, in runs of a small bundle that carries xarch's own files ----

/// A bundle `xforge` with one flow `work` (a station that judges its output with the interface
/// gates), whose `commands/`, `lib/`, `scripts/` and `gates/` are the real bundle's xarch files.
fn write_bundle(dir: &Path) {
    let src = example().join(".conix/flows/xforge");
    let out = dir.join("xforge");
    for (sub, files) in [
        ("commands", vec!["xarch.js"]),
        ("lib", vec!["arch.js", "governance.js", "cli.js"]),
        ("scripts", vec!["interface-delta.js", "interface-compat.js", "interfaces-archived.js"]),
        ("gates", vec!["interface-delta.yaml", "interface-compat.yaml", "interfaces-archived.yaml"]),
    ] {
        std::fs::create_dir_all(out.join(sub)).unwrap();
        for f in files {
            std::fs::copy(src.join(sub).join(f), out.join(sub).join(f)).unwrap_or_else(|e| panic!("{BUNDLE}/{sub}/{f}: {e}"));
        }
    }
    std::fs::write(out.join("bundle.yaml"), "name: xforge\ncommands:\n  xarch: {read_only: [modules, list, get, delta, check, pending, help]}\n").unwrap();
    std::fs::write(
        out.join("work.yaml"),
        "name: work\nstations:\n  - id: write\n    executor: {kind: session}\n    skill: write\n    output: {type: object}\n    exit: [{kind: output}, {kind: gate, gate: interface-delta}]\n",
    )
    .unwrap();
    std::fs::create_dir_all(out.join("skills/write")).unwrap();
    std::fs::write(out.join("skills/write/SKILL.md"), "---\nname: write\n---\n写。\n").unwrap();
}

struct Rig {
    rn: Arc<Runner>,
    engine: Arc<Engine>,
    host: Arc<ScriptedHost>,
    root: PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

async fn rig() -> Rig {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path());
    let root = project.path().canonicalize().unwrap();
    std::fs::create_dir_all(root.join("xforge/interfaces")).unwrap();
    std::fs::create_dir_all(root.join(".conix/governance")).unwrap();
    std::fs::copy(example().join("xforge/interfaces/textkit.yaml"), root.join("xforge/interfaces/textkit.yaml")).unwrap();
    std::fs::write(root.join("xforge/interfaces/legacy.yaml"), LEGACY).unwrap();
    std::fs::write(root.join(".conix/governance/xforge.yaml"), GOV_ON).unwrap();
    sh(&root, "git init -q && git add -A && git -c user.email=x@y -c user.name=x commit -qm base");

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
    let layout = ProjectLayout::new(".conix", data.path().join("worktrees"), "conix/flow").with_artifacts_dir("xforge");
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
    Rig { rn, engine, host, root, _dirs: vec![packs_dir, data, project] }
}

impl Rig {
    async fn start(&self, subject: &str) -> String {
        let input = CreateIn { flow: "xforge/work".into(), subject: subject.into(), project_root: Some(self.root.to_string_lossy().into()), ..Default::default() };
        let o = self.rn.start_run(input, Actor::device("d")).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        self.rn.apply(o).await;
        run
    }

    /// `xarch <args>` as a model, in `run` (or outside any run).
    async fn xarch(&self, run: Option<&str>, args: &[&str]) -> std::result::Result<String, String> {
        self.xarch_by(run, CommandBy::model("s"), args).await
    }

    async fn xarch_by(&self, run: Option<&str>, by: CommandBy, args: &[&str]) -> std::result::Result<String, String> {
        let mut c = CommandCall::new("xforge", "xarch", args.iter().map(|a| a.to_string()).collect(), by);
        c.project_root = Some(self.root.clone());
        c.run = run.map(String::from);
        self.rn.run_command(c).await.map(|r| r.text)
    }

    async fn ok(&self, run: &str, args: &[&str]) -> String {
        self.xarch(Some(run), args).await.unwrap_or_else(|e| panic!("xarch {args:?}: {e}"))
    }

    async fn err(&self, run: &str, args: &[&str]) -> String {
        match self.xarch(Some(run), args).await {
            Ok(t) => panic!("xarch {args:?} should fail, said: {t}"),
            Err(e) => e,
        }
    }

    async fn out_dir(&self, run: &str) -> PathBuf {
        PathBuf::from(self.engine.record(run).await.unwrap().output_dir.unwrap())
    }

    async fn delta(&self, run: &str) -> Value {
        let text = std::fs::read_to_string(self.out_dir(run).await.join("interface-delta.yaml")).unwrap_or_default();
        serde_yaml::from_str(&text).unwrap_or(Value::Null)
    }

    fn baseline(&self, module: &str) -> Value {
        serde_yaml::from_str(&std::fs::read_to_string(self.root.join(format!("xforge/interfaces/{module}.yaml"))).unwrap()).unwrap()
    }
}

fn element<'a>(module: &'a Value, id: &str) -> &'a Value {
    module["elements"].as_array().unwrap().iter().find(|e| e["id"] == id).unwrap_or_else(|| panic!("no {id} in {module}"))
}

#[tokio::test]
async fn the_baseline_reads_outside_a_run_and_a_write_wants_one() {
    let r = rig().await;
    let t = r.xarch(None, &["modules"]).await.unwrap();
    assert!(t.contains("textkit（src/textkit/**）：2 个元素（stable 2、experimental 0、deprecated 0）— 文本工具"), "{t}");
    assert!(t.contains("legacy") && t.contains("deprecated 1"), "{t}");
    let t = r.xarch(None, &["list", "textkit"]).await.unwrap();
    assert!(t.contains("fn:textkit.slugify [stable] slugify(text: str) -> str") && t.contains("fn:textkit.word_count [stable]"), "{t}");
    assert!(!t.contains("legacy"), "{t}");
    let t = r.xarch(None, &["list"]).await.unwrap();
    assert!(t.contains("fn:legacy.old [deprecated]") && t.contains("fn:textkit.slugify"), "{t}");
    let t = r.xarch(None, &["get", "fn:textkit.slugify"]).await.unwrap();
    assert!(t.contains("[stable]") && t.contains("签名：slugify(text: str) -> str") && t.contains("URL 片段"), "{t}");
    // An element with no `stability` reads as stable (a baseline written before there were states).
    let t = r.xarch(None, &["get", "fn:legacy.bare"]).await.unwrap();
    assert!(t.contains("[stable]"), "{t}");
    assert!(r.xarch(None, &["list", "nope"]).await.unwrap_err().contains("没有模块 nope（有 legacy、textkit）"));
    assert!(r.xarch(None, &["get", "fn:textkit.nope"]).await.unwrap_err().contains("没有 fn:textkit.nope"));
    assert_eq!(r.xarch(None, &["pending"]).await.unwrap(), "没有别的在途运行动接口");
    // `delta` and `check` are a run's; a person writing outside a run is told to start one.
    assert!(r.xarch(None, &["delta"]).await.unwrap_err().contains("要在运行里"));
    let e = r.xarch_by(None, CommandBy::person("me"), &["add", "textkit", "fn", "x", "x()", "x"]).await.unwrap_err();
    assert!(e.contains("要在运行里"), "{e}");
    // A model outside a run may not write at all (the declaration), and `help` is the file's header.
    let e = r.xarch(None, &["promote", "fn:legacy.fresh"]).await.unwrap_err();
    assert!(e.contains("writes") || e.contains("run"), "{e}");
    let t = r.xarch(None, &["help"]).await.unwrap();
    assert!(t.contains("xarch modify <编号> [--signature S] [--description D] [--breaking]") && t.contains("Read-only: `modules`"), "{t}");
}

#[tokio::test]
async fn every_subcommand_writes_the_delta_in_the_shape_the_contract_gives() {
    let r = rig().await;
    let run = r.start("加 truncate").await;
    assert_eq!(r.ok(&run, &["delta"]).await, "增量是空的");

    // add: experimental unless --stable; the id is <kind>:<module>.<name>.
    assert!(r.ok(&run, &["add", "textkit", "fn", "truncate", "truncate(text: str, n: int) -> str", "截断到 n 个字符"]).await.contains("fn:textkit.truncate（experimental）"));
    assert!(r.ok(&run, &["add", "textkit", "fn", "title_case", "title_case(text: str) -> str", "已经在用的", "--stable"]).await.contains("（stable）"));
    assert!(r.err(&run, &["add", "textkit", "fn", "truncate", "x()", "重复"]).await.contains("这次已经加了"));
    assert!(r.err(&run, &["add", "textkit", "fn", "slugify", "x()", "重复"]).await.contains("基线里已经有"));
    assert!(r.err(&run, &["add", "textkit", "func", "x", "x()", "x"]).await.contains("种类是 fn、type、endpoint、event、config"));
    assert!(r.err(&run, &["add", "ghost", "fn", "x", "x()", "x"]).await.contains("没有模块 ghost"));
    assert!(r.err(&run, &["add", "textkit", "fn", "x", "x()", "x", "--nope"]).await.contains("不认识 --nope"));

    // module: needs paths; a new module takes elements.
    assert!(r.err(&run, &["module", "render", "渲染"]).await.contains("--paths"));
    assert!(r.err(&run, &["module", "textkit", "x", "--paths", "a"]).await.contains("基线里已经有模块"));
    assert!(r.ok(&run, &["module", "render", "渲染", "--paths", "src/render/**,src/tpl/**"]).await.contains("新建模块 render"));
    assert!(r.ok(&run, &["add", "render", "type", "Page", "class Page", "页面"]).await.contains("type:render.Page"));
    assert!(r.ok(&run, &["modules"]).await.contains("render（src/render/**、src/tpl/**）：这次新建 — 渲染"));

    // promote only experimental → stable; deprecate; remove only what the baseline already deprecated.
    assert!(r.ok(&run, &["promote", "fn:legacy.fresh"]).await.contains("转正"));
    assert!(r.err(&run, &["promote", "fn:textkit.slugify"]).await.contains("现在是 stable：只有 experimental 能转正"));
    assert!(r.err(&run, &["promote", "fn:textkit.truncate"]).await.contains("这次新增的"));
    assert!(r.ok(&run, &["deprecate", "fn:textkit.word_count"]).await.contains("弃用"));
    assert!(r.err(&run, &["deprecate", "fn:legacy.old"]).await.contains("已经是 deprecated"));
    assert!(r.ok(&run, &["remove", "fn:legacy.old"]).await.contains("删除"));
    let e = r.err(&run, &["remove", "fn:textkit.slugify"]).await;
    assert!(e.contains("先 xarch deprecate") && e.contains("另开一次变更"), "{e}");
    let e = r.err(&run, &["remove", "fn:textkit.word_count"]).await;
    assert!(e.contains("这次刚弃用的"), "{e}");
    assert!(r.err(&run, &["remove", "fn:legacy.fresh"]).await.contains("先 xarch deprecate"));

    // modify an experimental element: no --breaking; modify one added by this run edits that entry.
    let e = r.err(&run, &["modify", "fn:legacy.bare", "--signature", "bare(x) -> int"]).await;
    assert!(e.contains("--breaking") && e.contains("deprecate"), "{e}");
    assert!(r.ok(&run, &["modify", "fn:textkit.truncate", "--description", "截断，超出加省略号"]).await.contains("改了增量里新增的"));
    assert!(r.err(&run, &["modify", "fn:textkit.truncate", "--description", "x", "--breaking"]).await.contains("不用 --breaking"));
    assert!(r.err(&run, &["modify", "fn:legacy.old", "--description", "x"]).await.contains("已经弃用"));
    assert!(r.err(&run, &["modify", "fn:textkit.nope", "--description", "x"]).await.contains("基线里没有"));
    // An experimental element may be edited and promoted in the same change.
    r.ok(&run, &["modify", "fn:legacy.fresh", "--description", "转正前改的说明"]).await;
    assert_eq!(r.delta(&run).await["modified"][0], json!({"id": "fn:legacy.fresh", "description": "转正前改的说明", "breaking": false}));

    let d = r.delta(&run).await;
    assert_eq!(d["modules"], json!([{"module": "render", "title": "渲染", "paths": ["src/render/**", "src/tpl/**"]}]));
    assert_eq!(d["added"][0], json!({"id": "fn:textkit.truncate", "signature": "truncate(text: str, n: int) -> str", "description": "截断，超出加省略号", "stability": "experimental"}));
    assert_eq!(d["added"][1]["stability"], "stable");
    assert_eq!(d["promoted"], json!(["fn:legacy.fresh"]));
    assert_eq!(d["deprecated"], json!(["fn:textkit.word_count"]));
    assert_eq!(d["removed"], json!(["fn:legacy.old"]));
    let t = r.ok(&run, &["list", "textkit"]).await;
    assert!(t.contains("fn:textkit.word_count [stable] word_count(text: str) -> int  ← 这次：deprecated") && t.contains("fn:textkit.truncate [experimental]"), "{t}");
    let t = r.ok(&run, &["get", "fn:textkit.truncate"]).await;
    assert!(t.contains("这次：新增"), "{t}");

    // check is the gate's judgement; revert takes a number, or a module with what it holds, out.
    let t = r.ok(&run, &["check"]).await;
    assert!(t.contains("对得上基线"), "{t}");
    assert!(r.ok(&run, &["revert", "fn:legacy.fresh"]).await.contains("拿掉了"));
    assert!(r.err(&run, &["revert", "fn:legacy.fresh"]).await.contains("增量里没有"));
    assert!(r.ok(&run, &["revert", "render"]).await.contains("拿掉了"));
    let d = r.delta(&run).await;
    assert!(d.get("modules").is_none() && d["added"].as_array().unwrap().len() == 2 && d.get("promoted").is_none(), "{d}");
    r.ok(&run, &["revert", "fn:textkit.truncate"]).await;
    r.ok(&run, &["revert", "fn:textkit.title_case"]).await;
    r.ok(&run, &["revert", "fn:textkit.word_count"]).await;
    r.ok(&run, &["revert", "fn:legacy.old"]).await;
    assert_eq!(r.ok(&run, &["delta"]).await, "增量是空的");
    assert!(r.ok(&run, &["check"]).await.contains("增量是空的"));
}

#[tokio::test]
async fn a_stable_element_is_modified_only_with_breaking_and_a_deprecated_one_not_at_all() {
    let r = rig().await;
    let run = r.start("改签名").await;
    let e = r.err(&run, &["modify", "fn:textkit.slugify", "--signature", "slugify(text: str, sep: str = '-') -> str"]).await;
    assert!(e.contains("别人在依赖它") && e.contains("--breaking") && e.contains("另加一个元素") && e.contains("deprecate"), "{e}");
    assert!(r.delta(&run).await.is_null() || r.delta(&run).await.as_object().is_none_or(|o| o.is_empty()));
    let t = r.ok(&run, &["modify", "fn:textkit.slugify", "--signature", "slugify(text: str, sep: str = '-') -> str", "--breaking"]).await;
    assert!(t.contains("（breaking）"), "{t}");
    // A second edit of the same element merges into the one entry; breaking stays declared.
    r.ok(&run, &["modify", "fn:textkit.slugify", "--description", "可以换分隔符", "--breaking"]).await;
    let d = r.delta(&run).await;
    assert_eq!(d["modified"].as_array().unwrap().len(), 1, "{d}");
    assert_eq!(d["modified"][0], json!({"id": "fn:textkit.slugify", "signature": "slugify(text: str, sep: str = '-') -> str", "description": "可以换分隔符", "breaking": true}));
    let t = r.ok(&run, &["list", "textkit"]).await;
    assert!(t.contains("modified breaking"), "{t}");
    assert!(r.err(&run, &["modify", "fn:textkit.slugify", "--signature", "slugify(text: str) -> str", "--breaking"]).await.contains("一样"));
    assert!(r.ok(&run, &["check"]).await.contains("interface-compat"), "check points at the compat gate when something breaks");
    // A stable element deprecated after it was edited: refused until the edit is taken out.
    assert!(r.err(&run, &["deprecate", "fn:textkit.slugify"]).await.contains("先 xarch revert"));
    // A bare-signature element (no stability on disk) is stable too.
    assert!(r.err(&run, &["modify", "fn:legacy.bare", "--description", "x"]).await.contains("--breaking"));
}

#[tokio::test]
async fn with_the_interface_baseline_off_the_writes_say_so_and_archive_has_nothing_to_do() {
    let r = rig().await;
    let run = r.start("没开接口基线").await;
    std::fs::write(r.root.join(".conix/governance/xforge.yaml"), GOV_OFF).unwrap();
    for args in [
        &["module", "m", "t", "--paths", "a"][..],
        &["add", "textkit", "fn", "x", "x()", "x"],
        &["modify", "fn:textkit.slugify", "--description", "x", "--breaking"],
        &["promote", "fn:legacy.fresh"],
        &["deprecate", "fn:textkit.slugify"],
        &["remove", "fn:legacy.old"],
        &["revert", "fn:textkit.slugify"],
    ] {
        let e = r.err(&run, args).await;
        assert!(e.contains("这个项目没开接口基线（.conix/governance/xforge.yaml）"), "{args:?}: {e}");
    }
    // Reading is still fine, and archive (every run's last station runs it) is not an error.
    assert!(r.ok(&run, &["modules"]).await.contains("textkit"));
    assert!(r.ok(&run, &["archive"]).await.contains("没开接口基线"));
    assert!(r.delta(&run).await.is_null(), "nothing was written");
}

#[tokio::test]
async fn two_runs_that_touch_the_same_element_see_each_other_as_conflicts() {
    let r = rig().await;
    let (a, b) = (r.start("甲").await, r.start("乙").await);
    assert_eq!(r.ok(&a, &["pending"]).await, "没有别的在途运行动接口");
    r.ok(&a, &["add", "textkit", "fn", "truncate", "truncate(text: str, n: int) -> str", "甲加的"]).await;
    r.ok(&a, &["modify", "fn:textkit.slugify", "--description", "甲改的", "--breaking"]).await;
    r.ok(&a, &["module", "render", "渲染", "--paths", "src/render/**"]).await;

    let t = r.ok(&b, &["pending"]).await;
    assert!(t.contains(&format!("运行 {a}（甲）")) && t.contains("fn:textkit.truncate") && t.contains("fn:textkit.slugify") && t.contains("module:render"), "{t}");
    assert_eq!(r.ok(&a, &["pending"]).await, "没有别的在途运行动接口", "a run does not count itself");
    // The other run's `add` of the same id says so as it writes, and `check` calls it a conflict.
    let t = r.ok(&b, &["add", "textkit", "fn", "truncate", "truncate(s, n)", "乙也加的"]).await;
    assert!(t.contains("注意在途运行") && t.contains(&a), "{t}");
    let t = r.ok(&b, &["check"]).await;
    assert!(t.contains("对不上基线") && t.contains(&format!("fn:textkit.truncate 在途运行 {a}（甲）也在改")), "{t}");
    // Something the other run does not touch is no conflict.
    r.ok(&b, &["revert", "fn:textkit.truncate"]).await;
    r.ok(&b, &["add", "textkit", "fn", "pad", "pad(text: str, n: int) -> str", "乙加的"]).await;
    assert!(r.ok(&b, &["check"]).await.contains("对得上基线"));
    // Once the other run is over its delta no longer counts.
    r.ok(&b, &["modify", "fn:textkit.slugify", "--description", "乙也改", "--breaking"]).await;
    assert!(r.ok(&b, &["check"]).await.contains("fn:textkit.slugify 在途运行"));
    r.ok(&a, &["revert", "fn:textkit.slugify"]).await;
    assert!(r.ok(&b, &["check"]).await.contains("对得上基线"));
}

/// A run's gate sees the other run's delta: submit B's station and read its gate.
#[tokio::test]
async fn the_gate_of_a_run_fails_on_a_conflict_with_another_in_flight_run() {
    let r = rig().await;
    let a = r.start("甲").await;
    r.ok(&a, &["add", "textkit", "fn", "truncate", "truncate(text: str, n: int) -> str", "甲加的"]).await;
    let b = r.start("乙").await;
    r.ok(&b, &["add", "textkit", "fn", "truncate", "truncate(s, n)", "乙也加的"]).await;

    let main = eventually("the main session opened", async || r.host.opened().into_iter().find(|s| s.main)).await;
    let sid = eventually("the session got its key", async || r.host.session_of(&main.key)).await;
    let t = r.rn.tools().call(&sid, TOOL_SUBMIT, json!({"output": {}})).await;
    assert!(!t.is_error, "{}", t.text);
    // Whichever run the main session works on is the one judged; both touch truncate, so it fails.
    let c = eventually("a gate judged", async || {
        for run in [&a, &b] {
            let Ok(rec) = r.engine.record(run).await else { continue };
            let Some(c) = rec.stations.get("write").and_then(|s| s.checks.iter().find(|c| c.kind == "gate")) else { continue };
            if c.status != CheckStatus::Wait {
                return Some(c.clone());
            }
        }
        None
    })
    .await;
    assert_eq!(c.status, CheckStatus::Fail, "{c:?}");
    assert!(c.detail.contains("处对不上基线"), "{}", c.detail);
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
async fn archive_merges_the_delta_into_the_baseline_and_checks_first() {
    let r = rig().await;
    let run = r.start("几样都改").await;
    assert!(r.ok(&run, &["archive"]).await.contains("增量是空的"), "no interface change is nothing to merge, not an error");
    r.ok(&run, &["module", "render", "渲染", "--paths", "src/render/**"]).await;
    r.ok(&run, &["add", "render", "type", "Page", "class Page", "页面"]).await;
    r.ok(&run, &["add", "textkit", "fn", "truncate", "truncate(text: str, n: int) -> str", "截断"]).await;
    r.ok(&run, &["modify", "fn:textkit.slugify", "--signature", "slugify(text: str, sep: str = '-') -> str", "--description", "可换分隔符", "--breaking"]).await;
    r.ok(&run, &["deprecate", "fn:textkit.word_count"]).await;
    r.ok(&run, &["promote", "fn:legacy.fresh"]).await;
    r.ok(&run, &["remove", "fn:legacy.old"]).await;
    let before = std::fs::read_to_string(r.root.join("xforge/interfaces/textkit.yaml")).unwrap();

    // A delta that does not fit is refused, and nothing is written.
    let delta_file = r.out_dir(&run).await.join("interface-delta.yaml");
    let good = std::fs::read_to_string(&delta_file).unwrap();
    std::fs::write(&delta_file, "removed: [\"fn:textkit.slugify\"]\n").unwrap();
    let e = r.err(&run, &["archive"]).await;
    assert!(e.contains("不能并") && e.contains("先 xarch deprecate"), "{e}");
    assert_eq!(std::fs::read_to_string(r.root.join("xforge/interfaces/textkit.yaml")).unwrap(), before);
    std::fs::write(&delta_file, good).unwrap();

    let t = r.ok(&run, &["archive"]).await;
    assert!(t.contains("xforge/interfaces/textkit.yaml") && t.contains("xforge/interfaces/render.yaml") && t.contains("xforge/interfaces/legacy.yaml"), "{t}");
    let tk = r.baseline("textkit");
    assert_eq!(element(&tk, "fn:textkit.slugify")["signature"], "slugify(text: str, sep: str = '-') -> str");
    assert_eq!(element(&tk, "fn:textkit.slugify")["description"], "可换分隔符");
    assert_eq!(element(&tk, "fn:textkit.slugify")["stability"], "stable");
    assert_eq!(element(&tk, "fn:textkit.word_count")["stability"], "deprecated");
    assert_eq!(element(&tk, "fn:textkit.truncate")["stability"], "experimental");
    assert_eq!((tk["title"].as_str(), &tk["paths"]), (Some("文本工具"), &json!(["src/textkit/**"])));
    let render = r.baseline("render");
    assert_eq!((render["module"].as_str(), &render["paths"]), (Some("render"), &json!(["src/render/**"])));
    assert_eq!(element(&render, "type:render.Page")["stability"], "experimental");
    let legacy = r.baseline("legacy");
    assert_eq!(element(&legacy, "fn:legacy.fresh")["stability"], "stable");
    assert!(!legacy["elements"].as_array().unwrap().iter().any(|e| e["id"] == "fn:legacy.old"));
    // What was merged reads back through the commands.
    assert!(r.ok(&run, &["list", "render"]).await.contains("type:render.Page [experimental]"));

    // The gate `interfaces-archived` on the files before and after.
    let b = bench();
    let files = |textkit: &str, legacy: &str, render: Option<&str>| {
        let mut f = json!({"out/interface-delta.yaml": good_delta(&delta_file), ".conix/governance/xforge.yaml": GOV_ON,
            "data/interfaces/textkit.yaml": textkit, "data/interfaces/legacy.yaml": legacy});
        if let Some(t) = render {
            f["data/interfaces/render.yaml"] = json!(t);
        }
        f
    };
    let read = |m: &str| std::fs::read_to_string(r.root.join(format!("xforge/interfaces/{m}.yaml"))).unwrap();
    let v = b.gate("interfaces-archived", files(&read("textkit"), &read("legacy"), Some(&read("render"))), json!({})).await;
    assert_eq!(v["verdict"], "pass", "{v}");
    let v = b.gate("interfaces-archived", files(&before, LEGACY, None), json!({})).await;
    assert_eq!(v["verdict"], "fail", "{v}");
    let msgs = v["findings"].to_string();
    assert!(msgs.contains("没有并进 modules 的 render") && msgs.contains("没有并进 added 的 fn:textkit.truncate") && msgs.contains("removed 的 fn:legacy.old 还在"), "{v}");
    // Half merged: the signature still the old one.
    let v = b.gate("interfaces-archived", files(&before, &read("legacy"), Some(&read("render"))), json!({})).await;
    assert!(v["findings"].to_string().contains("fn:textkit.slugify 的 signature 还没换成增量里的"), "{v}");
}

fn good_delta(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}

// ---- the gates, on samples ----

#[derive(Default)]
struct Convo;

#[async_trait]
impl AuthorHost for Convo {
    async fn current(&self, _: &SessionId) -> Option<String> {
        None
    }
    async fn project(&self, _: &SessionId) -> Option<String> {
        None
    }
}

struct Bench {
    tools: AuthorTools,
    _rn: Arc<Runner>,
}

fn bench() -> Bench {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    let packs = Arc::new(CatalogPacks::over(
        &RunnerConfig { command_gates: true, profiles: vec!["code".into()], main_profile: Some("code".into()), submit_profiles: ["code".to_string()].into(), ..Default::default() },
        vec![],
        Arc::new(DirPacks::new([example().join(".conix/flows")]).with_catalog(catalog)),
    ));
    let cfg = RunnerConfig { command_gates: true, profiles: vec!["code".into()], main_profile: Some("code".into()), submit_profiles: ["code".to_string()].into(), ..Default::default() };
    let layout = ProjectLayout::new(".conix", std::env::temp_dir().join("xforge-arch-worktrees"), "conix/flow").with_artifacts_dir("xforge");
    packs.set_layout(layout.clone());
    let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), packs.clone()).with_main_scene("code").with_layout(layout));
    let rn = Runner::new(engine, ScriptedHost::new(), cfg);
    let sandbox = Arc::new(Sandbox::new(packs, SandboxConfig { guarded: vec![], submit_scenes: vec!["code".into()], main_scene: Some("code".into()) }));
    Bench { tools: AuthorTools::new(&rn, sandbox, Arc::new(Convo)), _rn: rn }
}

impl Bench {
    /// The gate's whole result on `files`; `inputs` is what the run started with.
    async fn gate(&self, gate: &str, files: Value, inputs: Value) -> Value {
        let r = self
            .tools
            .call(
                &"s".to_string(),
                TOOL_GATE_TRY,
                json!({"gate": format!("xforge/{gate}"), "files": files, "output_dir": "out", "data_dir": "data", "inputs": inputs}),
                &Actor::new(CONVERSATION_KIND, "s"),
            )
            .await;
        assert!(!r.is_error, "{gate}: {}", r.text);
        let v: Value = serde_json::from_str(&r.text).unwrap();
        assert_eq!(v["tried"], true, "{gate}: {v}");
        v
    }
}

fn textkit() -> String {
    std::fs::read_to_string(example().join("xforge/interfaces/textkit.yaml")).unwrap()
}

fn sample(delta: &str) -> Value {
    json!({"out/interface-delta.yaml": delta, ".conix/governance/xforge.yaml": GOV_ON, "data/interfaces/textkit.yaml": textkit(), "data/interfaces/legacy.yaml": LEGACY})
}

#[test]
fn the_example_baseline_is_textkits_public_functions_as_stable() {
    let doc: Value = serde_yaml::from_str(&textkit()).unwrap();
    assert_eq!((doc["module"].as_str(), &doc["paths"]), (Some("textkit"), &json!(["src/textkit/**"])));
    let init = std::fs::read_to_string(example().join("src/textkit/__init__.py")).unwrap();
    let all = init.lines().find(|l| l.starts_with("__all__")).unwrap();
    let ids: Vec<&str> = doc["elements"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["fn:textkit.slugify", "fn:textkit.word_count"]);
    for e in doc["elements"].as_array().unwrap() {
        assert_eq!(e["stability"], "stable");
        let name = e["id"].as_str().unwrap().rsplit('.').next().unwrap();
        assert!(all.contains(name), "{name} is public");
        assert!(init.contains(&format!("def {}", e["signature"].as_str().unwrap().split(" ->").next().unwrap())), "{e}: the signature is the real one");
    }
}

#[tokio::test]
async fn interface_delta_judges_the_delta_against_the_baseline() {
    let b = bench();
    let ok = "modules:\n  - {module: render, title: 渲染, paths: [\"src/render/**\"]}\nadded:\n  - {id: \"type:render.Page\", signature: class Page, description: 页面, stability: experimental}\n  - {id: \"fn:textkit.truncate\", signature: \"truncate(text, n)\", description: 截断, stability: stable}\nmodified:\n  - {id: \"fn:textkit.slugify\", description: 新说明, breaking: true}\n  - {id: \"fn:legacy.fresh\", description: 试验的随便改, breaking: false}\npromoted: [\"fn:legacy.fresh\"]\ndeprecated: [\"fn:textkit.word_count\"]\nremoved: [\"fn:legacy.old\"]\n";
    let v = b.gate("interface-delta", sample(ok), json!({})).await;
    assert_eq!(v["verdict"], "pass", "{v}");
    assert_eq!(v["metrics"]["added"], 2, "{v}");

    // An empty or missing delta is a change that touches no interface: nothing owed.
    let v = b.gate("interface-delta", sample("{}\n"), json!({})).await;
    assert_eq!((v["verdict"].as_str(), v["summary"].as_str()), (Some("pass"), Some("没有接口改动")), "{v}");
    let mut s = sample("");
    s.as_object_mut().unwrap().remove("out/interface-delta.yaml");
    assert_eq!(b.gate("interface-delta", s, json!({})).await["verdict"], "pass");

    for (what, delta, says) in [
        ("a stable edit without breaking", "modified:\n  - {id: \"fn:textkit.slugify\", description: x, breaking: false}\n", "是 stable"),
        ("a stable edit that forgot the field", "modified:\n  - {id: \"fn:textkit.slugify\", description: x}\n", "是 stable"),
        ("breaking on an experimental one", "modified:\n  - {id: \"fn:legacy.fresh\", description: x, breaking: true}\n", "不构成承诺"),
        ("an edit of a deprecated one", "modified:\n  - {id: \"fn:legacy.old\", description: x, breaking: false}\n", "已经弃用"),
        ("an edit that changes nothing", "modified:\n  - {id: \"fn:textkit.slugify\", signature: \"slugify(text: str) -> str\", breaking: true}\n", "和基线一样"),
        ("removing a stable one", "removed: [\"fn:textkit.slugify\"]\n", "先 xarch deprecate"),
        ("removing one that is not there", "removed: [\"fn:textkit.nope\"]\n", "基线里没有"),
        ("promoting a stable one", "promoted: [\"fn:textkit.slugify\"]\n", "只有 experimental 能转正"),
        ("deprecating a deprecated one", "deprecated: [\"fn:legacy.old\"]\n", "已经弃用"),
        ("a bad id", "added:\n  - {id: \"textkit.x\", signature: x(), description: x, stability: experimental}\n", "编号是 <种类>:<模块>.<名字>"),
        ("an unknown kind", "added:\n  - {id: \"func:textkit.x\", signature: x(), description: x, stability: experimental}\n", "种类是 fn、type"),
        ("a module that does not exist", "added:\n  - {id: \"fn:ghost.x\", signature: x(), description: x, stability: experimental}\n", "没有模块 ghost"),
        ("an id already in the baseline", "added:\n  - {id: \"fn:textkit.slugify\", signature: x(), description: x, stability: stable}\n", "已经有了"),
        ("a new module without paths", "modules:\n  - {module: render, title: 渲染}\n", "要写 paths"),
        ("a module the baseline has", "modules:\n  - {module: textkit, title: x, paths: [a]}\n", "已经有了"),
        ("a deprecated element added", "added:\n  - {id: \"fn:textkit.x\", signature: x(), description: x, stability: deprecated}\n", "stability 是 experimental 或 stable"),
        ("one id in two lists", "promoted: [\"fn:legacy.fresh\"]\nremoved: [\"fn:legacy.fresh\"]\n", "同时在"),
    ] {
        let v = b.gate("interface-delta", sample(delta), json!({})).await;
        assert_eq!(v["verdict"], "fail", "{what}: {v}");
        assert!(v["findings"].to_string().contains(says), "{what}: {v}");
    }
    // Without the project's switch the gate owes nothing, whatever the delta says.
    let mut s = sample("removed: [\"fn:textkit.slugify\"]\n");
    s.as_object_mut().unwrap().remove(".conix/governance/xforge.yaml");
    let v = b.gate("interface-delta", s, json!({})).await;
    assert_eq!((v["verdict"].as_str(), v["summary"].as_str()), (Some("pass"), Some("不欠（governance.interface=false）")), "{v}");
    let mut s = sample("removed: [\"fn:textkit.slugify\"]\n");
    s["out/interface-delta.yaml"] = json!("removed: [\"fn:textkit.slugify\"]\n");
    s[".conix/governance/xforge.yaml"] = json!(GOV_OFF);
    assert_eq!(b.gate("interface-delta", s, json!({})).await["verdict"], "pass");
}

#[tokio::test]
async fn interface_compat_wants_the_impact_and_an_accepted_decision_for_each_breaking_change() {
    let b = bench();
    let delta = "modified:\n  - {id: \"fn:textkit.slugify\", signature: \"slugify(text, sep)\", breaking: true}\n";
    let sample = |decisions: Option<&str>| {
        let mut s = json!({"out/interface-delta.yaml": delta, ".conix/governance/xforge.yaml": GOV_ON});
        if let Some(d) = decisions {
            s["out/ledgers/decisions.yaml"] = json!(d);
        }
        s
    };
    let accepted = "entries:\n  - {id: D1, text: 破坏 slugify 的签名，调用方一并改, conclusion: accepted, refs: [\"fn:textkit.slugify\"]}\n";
    let v = b.gate("interface-compat", sample(Some(accepted)), json!({"impact": ["interface"]})).await;
    assert_eq!(v["verdict"], "pass", "{v}");
    assert!(v["summary"].as_str().unwrap().contains("fn:textkit.slugify"), "{v}");
    let v = b.gate("interface-compat", sample(Some(accepted)), json!({"impact": ["security", "interface"]})).await;
    assert_eq!(v["verdict"], "pass", "{v}");

    // The impact was not declared when the run started.
    for inputs in [json!({}), json!({"impact": []}), json!({"impact": ["data-migration"]})] {
        let v = b.gate("interface-compat", sample(Some(accepted)), inputs).await;
        assert_eq!(v["verdict"], "fail", "{v}");
        assert!(v["findings"].to_string().contains("inputs.impact 要含 interface"), "{v}");
    }
    // The decision is missing, not accepted, about something else, or only still open.
    for (what, ledger) in [
        ("no ledger", None),
        ("open", Some("entries:\n  - {id: D1, text: x, conclusion: open, refs: [\"fn:textkit.slugify\"]}\n")),
        ("rejected", Some("entries:\n  - {id: D1, text: x, conclusion: rejected, refs: [\"fn:textkit.slugify\"]}\n")),
        ("another ref", Some("entries:\n  - {id: D1, text: x, conclusion: accepted, refs: [\"fn:textkit.word_count\"]}\n")),
        ("no refs", Some("entries:\n  - {id: D1, text: x, conclusion: accepted}\n")),
    ] {
        let v = b.gate("interface-compat", sample(ledger), json!({"impact": ["interface"]})).await;
        assert_eq!(v["verdict"], "fail", "{what}: {v}");
        let f = v["findings"].to_string();
        assert!(f.contains("decisions 台账里没有指向它") && f.contains("--ref fn:textkit.slugify"), "{what}: {v}");
    }
    // No breaking change, no debt: a modified experimental element, additions and a deprecation.
    let calm = "added:\n  - {id: \"fn:textkit.x\", signature: x(), description: x, stability: stable}\ndeprecated: [\"fn:textkit.word_count\"]\n";
    let v = b.gate("interface-compat", json!({"out/interface-delta.yaml": calm, ".conix/governance/xforge.yaml": GOV_ON}), json!({})).await;
    assert_eq!((v["verdict"].as_str(), v["summary"].as_str()), (Some("pass"), Some("没有破坏性的接口改动")), "{v}");
    // The project's switch off: not owed, even with the breaking change and nothing else.
    let v = b.gate("interface-compat", json!({"out/interface-delta.yaml": delta}), json!({})).await;
    assert_eq!(v["summary"], "不欠（governance.interface=false）", "{v}");
    assert_eq!(v["verdict"], "pass");
}

#[tokio::test]
async fn interfaces_archived_owes_nothing_without_a_delta_or_the_switch() {
    let b = bench();
    let v = b.gate("interfaces-archived", json!({".conix/governance/xforge.yaml": GOV_ON, "out/interface-delta.yaml": "{}\n", "data/interfaces/textkit.yaml": textkit()}), json!({})).await;
    assert_eq!(v["verdict"], "pass", "{v}");
    let v = b.gate("interfaces-archived", json!({"out/interface-delta.yaml": "removed: [\"fn:textkit.slugify\"]\n", "data/interfaces/textkit.yaml": textkit()}), json!({})).await;
    assert_eq!(v["verdict"], "pass", "{v}");
    let v = b.gate("interfaces-archived", json!({".conix/governance/xforge.yaml": GOV_ON, "out/interface-delta.yaml": "removed: [\"fn:textkit.slugify\"]\n", "data/interfaces/textkit.yaml": textkit()}), json!({})).await;
    assert_eq!(v["verdict"], "fail", "{v}");
}
