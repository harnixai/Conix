//! xforge's ledgers and work packages (docs/design/xforge.md §2.3, §3, §4): the commands `xledger`
//! and `xpkg` keep the rules of who may give which conclusion and what a package must say, and the
//! gates `ledgers-settled`, `constitution-reply`, `packages-planned`, `package-check`, `unclaimed`
//! judge the same data again. No model: a run of a probe flow (one station, one gate, copied beside
//! the bundle) in a committed copy of the example, its commands called as a person, a station or a
//! model would call them.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::runner::{CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig, ScriptedHost, Turn, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh").arg("-c").arg(cmd).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "{cmd}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

/// The gates the probes try: (probe flow, gate, its own part when it is not the bundle's). The
/// probe gates differ from the real ones only in what `${item}` and the handed packages are.
const PROBES: &[(&str, &str)] = &[
    ("ledgers-settled", "ledgers-settled"),
    ("constitution-reply", "constitution-reply"),
    ("unclaimed", "unclaimed"),
    ("packages-done", "packages-done"),
    ("planned", "probe-planned"),
    ("check", "probe-check"),
];

/// The bundle as it is, plus probe flows and the two probe gates, in a directory of bundles.
fn bundles() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let b = d.path().join("xforge");
    sh(d.path(), &format!("cp -R {}/.conix/flows/xforge {}", example().display(), b.display()));
    std::fs::create_dir_all(b.join("skills/work")).unwrap();
    std::fs::write(b.join("skills/work/SKILL.md"), "---\nname: work\n---\nwork.\n").unwrap();
    for (flow, gate) in PROBES {
        let text = format!(
            "name: p-{flow}\nstations:\n  - id: work\n    executor: {{kind: session}}\n    skill: work\n    output: {{type: object}}\n    exit: [{{kind: output}}, {{kind: gate, gate: {gate}}}]\n"
        );
        std::fs::write(b.join(format!("p-{flow}.yaml")), text).unwrap();
    }
    // The real gates' params, with the work item named and the station's own output as the handed packages.
    std::fs::write(
        b.join("gates/probe-check.yaml"),
        "name: probe-check\nkind: script\nscript: package-check\nparams:\n  plan: \"${output_dir}/work-packages.yaml\"\n  item: p1\n",
    )
    .unwrap();
    std::fs::write(
        b.join("gates/probe-planned.yaml"),
        "name: probe-planned\nkind: script\nscript: packages-planned\nparams:\n  plan: \"${output_dir}/work-packages.yaml\"\n  handed: \"${stations.work.output.packages}\"\n",
    )
    .unwrap();
    d
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

/// A run of a probe flow in a committed copy of the example.
struct Rig {
    rn: Arc<Runner>,
    engine: Arc<Engine>,
    sid: String,
    run: String,
    root: PathBuf,
    out: PathBuf,
    _keep: (tempfile::TempDir, tempfile::TempDir, tempfile::TempDir),
}

impl Rig {
    async fn new(probe: &str) -> Rig {
        let (packs_dir, data, project) = (bundles(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let root = project.path().canonicalize().unwrap();
        sh(&root, &format!("cp -R {}/. . && rm -rf .conix/flows && git init -q && git add -A && git -c user.email=x@y -c user.name=x commit -qm base", example().display()));
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
            .start_run(CreateIn { flow: format!("xforge/p-{probe}"), subject: "s".into(), project_root: Some(root.to_string_lossy().into()), ..Default::default() }, Actor::device("d"))
            .await
            .unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        rn.apply(o).await;
        let main = eventually("the main session opened", async || host.opened().into_iter().find(|s| s.main)).await;
        let sid = eventually("the session got its key", async || host.session_of(&main.key)).await;
        eventually("the task was sent", async || host.sent_to(&sid).into_iter().next()).await;
        let rec = engine.record(&run).await.unwrap();
        let out = PathBuf::from(rec.output_dir.unwrap());
        Rig { rn, engine, sid, run, root, out, _keep: (packs_dir, data, project) }
    }

    /// A command as `by` calls it, `station` being the one whose session or step it is.
    async fn cmd(&self, command: &str, by: CommandBy, station: Option<&str>, args: &[&str]) -> Said {
        let mut c = CommandCall::new("xforge", command, args.iter().map(|a| a.to_string()).collect(), by);
        c.run = Some(self.run.clone());
        c.station = station.map(String::from);
        self.rn.run_command(c).await.map(|r| r.text).map_err(|e| e.to_string())
    }

    /// The same, as a person.
    async fn me(&self, command: &str, args: &[&str]) -> Said {
        self.cmd(command, CommandBy::person("me"), None, args).await
    }

    /// The same, as the model of `station`'s session.
    async fn model(&self, station: &str, command: &str, args: &[&str]) -> Said {
        self.cmd(command, CommandBy::model("m"), Some(station), args).await
    }

    async fn ok(&self, r: Said) -> String {
        r.unwrap_or_else(|e| panic!("refused: {e}"))
    }

    fn write(&self, rel: &str, text: &str) {
        let p = self.root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    fn ledger(&self, kind: &str) -> Value {
        serde_yaml::from_str(&std::fs::read_to_string(self.out.join(format!("ledgers/{kind}.yaml"))).unwrap()).unwrap()
    }

    /// Hand the station in with `output` and give the gate's check once it has judged.
    async fn judge(&self, output: Value) -> CheckRec {
        let r = self.rn.tools().call(&self.sid, TOOL_SUBMIT, json!({"output": output})).await;
        assert!(!r.is_error, "{}", r.text);
        eventually("the gate judged", async || {
            let rec = self.engine.record(&self.run).await.unwrap();
            let c = rec.stations["work"].checks.iter().find(|c| c.kind == "gate")?.clone();
            (c.status != CheckStatus::Wait).then_some(c)
        })
        .await
    }
}

fn said(c: &CheckRec) -> String {
    format!("{} {}", c.detail, serde_json::to_string(&c.findings).unwrap())
}

type Said = std::result::Result<String, String>;

fn station(id: &str) -> CommandBy {
    CommandBy::station(id)
}

fn refused(r: Said, words: &[&str]) {
    let e = r.expect_err("it should have been refused");
    for w in words {
        assert!(e.contains(w), "{w:?} not in: {e}");
    }
}

// ── xledger ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn findings_need_a_ref_and_only_the_check_station_or_a_person_may_reject_defer_or_reopen() {
    let r = Rig::new("ledgers-settled").await;
    refused(r.model("design", "xledger", &["add", "findings", "没测空串"]).await, &["至少一个 --ref"]);
    let t = r.ok(r.model("check", "xledger", &["add", "findings", "没测空串", "--ref", "design.md#做法,TK-R2"]).await).await;
    assert!(t.contains("F1"), "{t}");
    let f = r.ledger("findings");
    assert_eq!(f["entries"][0]["refs"], json!(["design.md#做法", "TK-R2"]));
    assert_eq!(f["entries"][0]["by"], json!({"kind": "station", "id": "check"}), "a model in a station is recorded as the station");

    // The author (the design station, a model in its session) answers resolved or disputed, nothing else.
    for no in ["rejected", "deferred", "open", "accepted"] {
        let want: &[&str] = if no == "accepted" { &["findings 的结论是"] } else { &["作者只能给 resolved", "检查站"] };
        refused(r.model("design", "xledger", &["set", "findings", "F1", no]).await, want);
    }
    // A model with no station (the main conversation) is no better than an author; a station's own step counts as the station.
    refused(r.cmd("xledger", CommandBy::model("m"), None, &["set", "findings", "F1", "rejected"]).await, &["模型 m"]);
    r.ok(r.model("design", "xledger", &["set", "findings", "F1", "disputed", "测了"]).await).await;
    r.ok(r.model("design", "xledger", &["set", "findings", "F1", "resolved", "补了一条"]).await).await;
    // The check station and a person may give any.
    r.ok(r.model("check", "xledger", &["set", "findings", "F1", "open"]).await).await;
    r.ok(r.cmd("xledger", station("check"), None, &["set", "findings", "F1", "rejected", "不成立"]).await).await;
    r.ok(r.model("check", "xledger", &["set", "findings", "F1", "deferred"]).await).await;
    r.ok(r.me("xledger", &["set", "findings", "F1", "rejected", "我定的"]).await).await;
    let e = &r.ledger("findings")["entries"][0];
    assert_eq!(e["conclusion"], "rejected");
    assert_eq!(e["by"], json!({"kind": "person", "id": "me"}));
    let h = e["history"].as_array().unwrap();
    assert_eq!(h.iter().map(|x| x["conclusion"].as_str().unwrap()).collect::<Vec<_>>(), ["open", "disputed", "resolved", "open", "rejected", "deferred", "rejected"]);
    assert!(h.iter().all(|x| x["by"]["kind"].is_string() && x["at"].is_string()), "{h:?}");
    // list says who.
    let t = r.ok(r.me("xledger", &["list"]).await).await;
    assert!(t.contains("F1 [rejected]") && t.contains("人 me") && t.contains("refs design.md#做法"), "{t}");
    // A station other than design is an author too.
    refused(r.model("apply", "xledger", &["set", "findings", "F1", "deferred"]).await, &["apply 站"]);
}

#[tokio::test]
async fn decisions_and_risks_are_anyones_to_conclude_and_only_a_person_answers_a_question() {
    let r = Rig::new("ledgers-settled").await;
    r.ok(r.model("design", "xledger", &["add", "decisions", "用", "U+2026", "作省略号"]).await).await;
    r.ok(r.model("design", "xledger", &["add", "risks", "超长文本慢"]).await).await;
    r.ok(r.model("design", "xledger", &["set", "decisions", "D1", "accepted", "与提案一致"]).await).await;
    r.ok(r.model("design", "xledger", &["set", "risks", "R1", "deferred"]).await).await;
    refused(r.model("design", "xledger", &["set", "decisions", "D1", "answered"]).await, &["decisions 的结论是"]);
    refused(r.model("design", "xledger", &["set", "decisions", "D9", "accepted"]).await, &["没有 decisions D9"]);
    refused(r.model("design", "xledger", &["add", "stories", "x"]).await, &["种类是"]);

    r.ok(r.model("clarify", "xledger", &["add", "questions", "n", "为负数怎么办"]).await).await;
    refused(r.model("clarify", "xledger", &["set", "questions", "Q1", "answered", "抛错"]).await, &["只有人能回答", "clarify 站"]);
    refused(r.cmd("xledger", station("clarify"), None, &["set", "questions", "Q1", "deferred"]).await, &["只有人能回答"]);
    refused(r.me("xledger", &["set", "questions", "Q1", "answered"]).await, &["回答写在说明里"]);
    r.ok(r.me("xledger", &["set", "questions", "Q1", "answered", "抛", "ValueError"]).await).await;
    let q = &r.ledger("questions")["entries"][0];
    assert_eq!((q["conclusion"].as_str(), q["note"].as_str(), q["by"]["kind"].as_str()), (Some("answered"), Some("抛 ValueError"), Some("person")));
    // The constitution ledger is written by `reply` alone.
    refused(r.me("xledger", &["add", "constitution", "x"]).await, &["xledger reply"]);
    refused(r.me("xledger", &["set", "constitution", "P1", "complies"]).await, &["xledger reply"]);
    // Ids go on from the highest, not from the count.
    r.ok(r.me("xledger", &["add", "decisions", "第二条"]).await).await;
    assert_eq!(r.ledger("decisions")["entries"][1]["id"], "D2");
}

#[tokio::test]
async fn reply_answers_the_projects_principles_from_the_check_station_or_a_person() {
    let r = Rig::new("constitution-reply").await;
    refused(r.model("design", "xledger", &["reply", "P1", "complies", "守了"]).await, &["只有检查站", "design 站"]);
    refused(r.cmd("xledger", CommandBy::model("m"), None, &["reply", "P1", "complies"]).await, &["只有检查站"]);
    refused(r.model("check", "xledger", &["reply", "P9", "complies", "x"]).await, &["没有原则 P9", "P1"]);
    refused(r.model("check", "xledger", &["reply", "P1", "maybe", "x"]).await, &["结论是 complies"]);
    refused(r.model("check", "xledger", &["reply", "P1", "violates"]).await, &["violates 要说明"]);
    r.ok(r.model("check", "xledger", &["reply", "P1", "complies", "只用了标准库"]).await).await;
    r.ok(r.me("xledger", &["reply", "P2", "violates", "TK-R3 没有测试"]).await).await;
    r.ok(r.model("check", "xledger", &["reply", "P1", "n-a", "没有新依赖"]).await).await;
    let l = r.ledger("constitution");
    assert_eq!(l["entries"].as_array().unwrap().len(), 2);
    assert_eq!(l["entries"][0]["conclusion"], "n-a");
    assert_eq!(l["entries"][0]["history"].as_array().unwrap().len(), 2);
    assert_eq!(l["entries"][1]["by"]["kind"], "person");
    let t = r.ok(r.me("xledger", &["list", "constitution"]).await).await;
    assert!(t.contains("P2 [violates]") && t.contains("TK-R3 没有测试"), "{t}");
}

// ── the ledger gates ─────────────────────────────────────────────────────────

#[tokio::test]
async fn ledgers_settled_passes_a_settled_set_and_fails_each_open_end() {
    // No ledgers is nothing owed.
    let r = Rig::new("ledgers-settled").await;
    assert_eq!(r.judge(json!({})).await.status, CheckStatus::Pass);

    // Settled: a decision accepted, a finding resolved by the author, a question answered by a person.
    let r = Rig::new("ledgers-settled").await;
    r.ok(r.model("design", "xledger", &["add", "decisions", "用省略号"]).await).await;
    r.ok(r.model("design", "xledger", &["set", "decisions", "D1", "accepted"]).await).await;
    r.ok(r.model("check", "xledger", &["add", "findings", "没测空串", "--ref", "TK-R2"]).await).await;
    r.ok(r.model("design", "xledger", &["set", "findings", "F1", "resolved"]).await).await;
    r.ok(r.model("clarify", "xledger", &["add", "questions", "负数？"]).await).await;
    r.ok(r.me("xledger", &["set", "questions", "Q1", "answered", "抛错"]).await).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Pass, "{}", said(&c));

    // Each of these fails, and says which entry and why.
    let cases: [(&str, &str, &[&str]); 5] = [
        ("decisions", "entries:\n  - {id: D1, text: x, conclusion: open}\n", &["D1", "还是 open"]),
        ("findings", "entries:\n  - {id: F1, text: x, conclusion: disputed, refs: [a], by: {kind: station, id: design}}\n", &["F1", "disputed", "算没结"]),
        ("findings", "entries:\n  - {id: F1, text: x, conclusion: resolved, refs: [], by: {kind: station, id: design}}\n", &["F1", "没有 ref"]),
        ("findings", "entries:\n  - {id: F1, text: x, conclusion: rejected, refs: [a], by: {kind: station, id: design}}\n", &["F1", "rejected", "只有检查站或人"]),
        ("questions", "entries:\n  - {id: Q1, text: x, conclusion: answered, by: {kind: station, id: clarify}}\n", &["Q1", "只有人能答"]),
    ];
    for (kind, text, words) in cases {
        let r = Rig::new("ledgers-settled").await;
        r.write(&format!("{}/ledgers/{kind}.yaml", r.out.strip_prefix(&r.root).unwrap().display()), text);
        let c = r.judge(json!({})).await;
        assert_eq!(c.status, CheckStatus::Fail, "{kind}: {}", said(&c));
        for w in words {
            assert!(said(&c).contains(w), "{w:?} not in {}", said(&c));
        }
    }
    // The check station's rejection and a person's deferral stand.
    let r = Rig::new("ledgers-settled").await;
    r.write(
        &format!("{}/ledgers/findings.yaml", r.out.strip_prefix(&r.root).unwrap().display()),
        "entries:\n  - {id: F1, text: x, conclusion: rejected, refs: [a], by: {kind: station, id: check}}\n  - {id: F2, text: y, conclusion: deferred, refs: [b], by: {kind: person, id: me}}\n",
    );
    assert_eq!(r.judge(json!({})).await.status, CheckStatus::Pass);
}

#[tokio::test]
async fn constitution_reply_wants_every_principle_answered_by_the_check_station_and_a_note_on_violates() {
    let r = Rig::new("constitution-reply").await;
    complies(&r, &["P1", "P2", "P3", "P4"]).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Pass, "{}", said(&c));

    let r = Rig::new("constitution-reply").await;
    complies(&r, &["P1", "P2", "P4"]).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("没有答 P3"), "{}", said(&c));

    let r = Rig::new("constitution-reply").await;
    assert_eq!(r.judge(json!({})).await.status, CheckStatus::Fail, "no ledger: nothing answered");

    // A violates with no note, an answer by another station, an entry for no principle: all by hand.
    let r = Rig::new("constitution-reply").await;
    let dir = r.out.strip_prefix(&r.root).unwrap().display().to_string();
    let by = "{kind: station, id: check}";
    r.write(
        &format!("{dir}/ledgers/constitution.yaml"),
        &format!("entries:\n  - {{id: P1, text: a, conclusion: violates, by: {by}}}\n  - {{id: P2, text: b, conclusion: complies, by: {{kind: station, id: design}}}}\n  - {{id: P3, text: c, conclusion: complies, by: {by}}}\n  - {{id: P4, text: d, conclusion: complies, by: {by}}}\n  - {{id: P7, text: e, conclusion: complies, by: {by}}}\n"),
    );
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail);
    for w in ["P1 判了 violates 但没有说明", "P2 是 design 站 答的", "P7 不是项目的原则"] {
        assert!(said(&c).contains(w), "{w:?} not in {}", said(&c));
    }

    // A violates with its note passes the gate (the finding is the check station's to raise).
    let r = Rig::new("constitution-reply").await;
    complies(&r, &["P1", "P3", "P4"]).await;
    r.ok(r.model("check", "xledger", &["reply", "P2", "violates", "TK-R3 没有测试"]).await).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Pass, "{}", said(&c));
    assert!(c.detail.contains("违反：P2"), "{}", c.detail);

    // A project with no principles: pass, with a warning.
    let r = Rig::new("constitution-reply").await;
    std::fs::remove_file(r.root.join(".conix/governance/constitution.yaml")).unwrap();
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Pass);
    assert!(serde_json::to_string(&c.findings).unwrap().contains("warn"), "{:?}", c.findings);
}

async fn complies(r: &Rig, ids: &[&str]) {
    for id in ids {
        r.ok(r.model("check", "xledger", &["reply", id, "complies", "守了"]).await).await;
    }
}

// ── xpkg ─────────────────────────────────────────────────────────────────────

const PLAN: &[&str] = &["plan", "p1", "加 truncate", "--paths", "src/textkit/**,tests/**", "--criteria", "超长截断;不超长原样返回", "--verify", "单测"];

#[tokio::test]
async fn xpkg_plans_with_numbered_criteria_and_done_needs_evidence_for_each_and_files_in_the_paths() {
    let r = Rig::new("check").await;
    refused(r.model("design", "xpkg", &["plan", "p1", "t", "--criteria", "一"]).await, &["--paths"]);
    refused(r.model("design", "xpkg", &["plan", "p1", "t", "--paths", "src/**"]).await, &["--criteria"]);
    refused(r.model("design", "xpkg", &["plan", "p1", "t", "--paths", "../x", "--criteria", "一"]).await, &["..", "相对路径"]);
    refused(r.model("design", "xpkg", &["plan", "P_1", "t", "--paths", "src/**", "--criteria", "一"]).await, &["小写字母"]);
    refused(r.model("design", "xpkg", &["plan", "p1", "t", "--paths", "src/**", "--criteria", "一", "--after", "p1"]).await, &["指着自己"]);
    let t = r.ok(r.model("design", "xpkg", PLAN).await).await;
    assert!(t.contains("2 条判据"), "{t}");
    let mut second = PLAN.to_vec();
    second[1] = "p2";
    second.extend(["--after", "p1"]);
    second[4] = "docs/**";
    r.ok(r.model("design", "xpkg", &second).await).await;
    let plan: Value = serde_yaml::from_str(&std::fs::read_to_string(r.out.join("work-packages.yaml")).unwrap()).unwrap();
    let p1 = &plan["packages"][0];
    assert_eq!(p1["criteria"], json!([{"id": "C1", "text": "超长截断"}, {"id": "C2", "text": "不超长原样返回"}]));
    assert_eq!((p1["status"].as_str(), p1["verify"].as_str(), p1["paths"].clone()), (Some("planned"), Some("单测"), json!(["src/textkit/**", "tests/**"])));
    assert_eq!(plan["packages"][1]["after"], json!(["p1"]));
    let t = r.ok(r.me("xpkg", &["list"]).await).await;
    assert!(t.contains("p1 [planned]") && t.contains("C2 不超长原样返回") && t.contains("先做：p1"), "{t}");

    let done = |files: &str, ev: &str| ["done", "p1", "加了函数", "--files", files, "--evidence", ev].map(String::from).to_vec();
    let run = async |a: Vec<String>| {
        let a: Vec<&str> = a.iter().map(String::as_str).collect();
        r.model("apply", "xpkg", &a).await
    };
    refused(r.model("apply", "xpkg", &["done", "p1", "做了", "--evidence", "C1=a;C2=b"]).await, &["--files"]);
    refused(run(done("src/textkit/a.py", "C1=跑了单测")).await, &["判据 C2 没有证据"]);
    refused(run(done("src/textkit/a.py", "C1=a;C2=")).await, &["判据 C2 没有证据"]);
    refused(run(done("src/textkit/a.py", "C1=a;C2=b;C3=c")).await, &["没有判据 C3"]);
    refused(run(done("src/textkit/a.py", "C1 a;C2=b")).await, &["C1=…;C2=…"]);
    refused(run(done("src/textkit/a.py,README.md,docs/x.md", "C1=a;C2=b")).await, &["README.md, docs/x.md"]);
    assert_eq!(r.ok(r.me("xpkg", &["list"]).await).await.matches("[planned]").count(), 2, "nothing was registered by the refusals");
    r.ok(run(done("src/textkit/a.py,tests/test_a.py", "C1=单测 test_trunc 通过;C2=test_short 通过")).await).await;
    let plan: Value = serde_yaml::from_str(&std::fs::read_to_string(r.out.join("work-packages.yaml")).unwrap()).unwrap();
    let p1 = &plan["packages"][0];
    assert_eq!(p1["status"], "done");
    assert_eq!(p1["files"], json!(["src/textkit/a.py", "tests/test_a.py"]));
    assert_eq!(p1["evidence"], json!({"C1": "单测 test_trunc 通过", "C2": "test_short 通过"}));
    refused(r.model("design", "xpkg", PLAN).await, &["已经 done"]);
    // blocked says where, and a package that is not planned is told.
    refused(r.model("apply", "xpkg", &["blocked", "p2"]).await, &["卡在哪"]);
    r.ok(r.model("apply", "xpkg", &["blocked", "p2", "要先改接口"]).await).await;
    refused(r.model("apply", "xpkg", &["blocked", "p9", "x"]).await, &["没有工作包 p9"]);
    assert!(r.ok(r.me("xpkg", &["list"]).await).await.contains("p2 [blocked]"));
    // Outside a run there is no package list to write.
    let mut c = CommandCall::new("xforge", "xpkg", vec!["plan".into()], CommandBy::person("me"));
    c.project_root = Some(r.root.clone());
    assert!(r.rn.run_command(c).await.unwrap_err().contains("要在运行里"));
}

// ── the package gates ────────────────────────────────────────────────────────

async fn plan(r: &Rig, id: &str, paths: &str, extra: &[&str]) {
    let mut a = vec!["plan", id, "t", "--paths", paths, "--criteria", "一;二"];
    a.extend(extra.iter().copied());
    r.ok(r.model("design", "xpkg", &a).await).await;
}

fn handed(ids: &[&str]) -> Value {
    json!({"packages": ids.iter().map(|i| json!({"id": i, "title": "t"})).collect::<Vec<_>>()})
}

#[tokio::test]
async fn packages_planned_matches_the_handed_packages_and_checks_paths_criteria_order_and_overlap() {
    // Two packages that keep to their own files, one after the other.
    let r = Rig::new("planned").await;
    plan(&r, "p1", "src/textkit/**", &[]).await;
    plan(&r, "p2", "tests/**", &["--after", "p1"]).await;
    let c = r.judge(handed(&["p1", "p2"])).await;
    assert_eq!(c.status, CheckStatus::Pass, "{}", said(&c));

    // Handed and planned must be the same set.
    let r = Rig::new("planned").await;
    plan(&r, "p1", "src/textkit/**", &[]).await;
    let c = r.judge(handed(&["p1", "p3"])).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("交出的工作包 p3 没有"), "{}", said(&c));
    let r = Rig::new("planned").await;
    plan(&r, "p1", "src/textkit/**", &[]).await;
    plan(&r, "p2", "tests/**", &[]).await;
    let c = r.judge(handed(&["p1"])).await;
    assert!(said(&c).contains("划了的工作包 p2 没有交出来"), "{}", said(&c));

    // An `after` that names nothing, and a cycle.
    let r = Rig::new("planned").await;
    plan(&r, "p1", "src/textkit/**", &["--after", "ghost"]).await;
    let c = r.judge(handed(&["p1"])).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("ghost 不存在"), "{}", said(&c));
    let r = Rig::new("planned").await;
    plan(&r, "p1", "src/textkit/**", &["--after", "p2"]).await;
    plan(&r, "p2", "tests/**", &["--after", "p1"]).await;
    let c = r.judge(handed(&["p1", "p2"])).await;
    assert!(said(&c).contains("after 成环：p1 → p2 → p1"), "{}", said(&c));

    // Globs that cover the same real file.
    let r = Rig::new("planned").await;
    plan(&r, "p1", "src/**", &[]).await;
    plan(&r, "p2", "src/textkit/*.py", &[]).await;
    let c = r.judge(handed(&["p1", "p2"])).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("p1 与 p2 的路径重叠（都盖住 src/textkit/__init__.py）"), "{}", said(&c));

    // A package with no path or no criterion (written by hand: the command would not).
    let r = Rig::new("planned").await;
    let dir = r.out.strip_prefix(&r.root).unwrap().display().to_string();
    r.write(&format!("{dir}/work-packages.yaml"), "packages:\n  - {id: p1, title: t, paths: [], criteria: [{id: C1, text: x}], status: planned}\n  - {id: p2, title: t, paths: [tests/**], status: planned}\n");
    let c = r.judge(handed(&["p1", "p2"])).await;
    assert!(said(&c).contains("p1 没有路径") && said(&c).contains("p2 没有判据"), "{}", said(&c));

    // No plan at all.
    let r = Rig::new("planned").await;
    assert_eq!(r.judge(handed(&["p1"])).await.status, CheckStatus::Fail);
}

#[tokio::test]
async fn packages_done_wants_every_package_done() {
    let r = Rig::new("packages-done").await;
    r.ok(r.model("design", "xpkg", PLAN).await).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail, "{}", said(&c));
    let r = Rig::new("packages-done").await;
    r.ok(r.model("design", "xpkg", PLAN).await).await;
    r.ok(r.model("apply", "xpkg", &["done", "p1", "做了", "--files", "src/textkit/a.py", "--evidence", "C1=a;C2=b"]).await).await;
    assert_eq!(r.judge(json!({})).await.status, CheckStatus::Pass);
}

/// A package planned over `src/textkit/**` and `tests/**`, a change committed after the run began and one not.
async fn worked(probe: &str) -> Rig {
    let r = Rig::new(probe).await;
    r.ok(r.model("design", "xpkg", PLAN).await).await;
    r.write("src/textkit/__init__.py", &(std::fs::read_to_string(example().join("src/textkit/__init__.py")).unwrap() + "\n\ndef truncate(text, n):\n    return text\n"));
    sh(&r.root, "git add -A && git -c user.email=x@y -c user.name=x commit -qm second");
    r.write("tests/test_trunc.py", "def test_trunc():\n    pass\n");
    r
}

const DONE: &[&str] = &["done", "p1", "加了 truncate", "--files", "src/textkit/__init__.py,tests/test_trunc.py", "--evidence", "C1=test_trunc;C2=test_short"];

#[tokio::test]
async fn package_check_wants_done_evidence_and_the_files_to_agree_with_what_git_says_changed() {
    // Committed and uncommitted changes, all registered: pass.
    let r = worked("check").await;
    r.ok(r.model("apply", "xpkg", DONE).await).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Pass, "{}", said(&c));
    assert_eq!(c.metrics["changed"], 2);

    // Not done yet, and blocked (says why).
    let r = worked("check").await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("还是 planned"), "{}", said(&c));
    let r = worked("check").await;
    r.ok(r.model("apply", "xpkg", &["blocked", "p1", "接口要先改"]).await).await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("卡住了：接口要先改"), "{}", said(&c));

    // A change under the package's paths that it did not register, committed or not.
    for (skip, left) in [("tests/test_trunc.py", "src/textkit/__init__.py"), ("src/textkit/__init__.py", "tests/test_trunc.py")] {
        let r = worked("check").await;
        r.ok(r.model("apply", "xpkg", &["done", "p1", "做了", "--files", left, "--evidence", "C1=a;C2=b"]).await).await;
        let c = r.judge(json!({})).await;
        assert_eq!(c.status, CheckStatus::Fail, "{skip}");
        assert!(said(&c).contains(&format!("没登记在 files 里：{skip}")), "{}", said(&c));
    }
    // A change outside its paths is not this package's business.
    let r = worked("check").await;
    r.write("README.md", "changed");
    r.ok(r.model("apply", "xpkg", DONE).await).await;
    assert_eq!(r.judge(json!({})).await.status, CheckStatus::Pass);
    // A file registered that nothing changed; and evidence cut out of the file by hand.
    let r = worked("check").await;
    r.ok(r.model("apply", "xpkg", &["done", "p1", "做了", "--files", "src/textkit/__init__.py,tests/test_trunc.py,tests/test_untouched.py", "--evidence", "C1=a;C2=b"]).await).await;
    let c = r.judge(json!({})).await;
    assert!(said(&c).contains("没有这个改动：tests/test_untouched.py"), "{}", said(&c));
    let r = worked("check").await;
    let dir = r.out.strip_prefix(&r.root).unwrap().display().to_string();
    r.write(&format!("{dir}/work-packages.yaml"), "packages:\n  - {id: p1, title: t, paths: [src/textkit/**, tests/**], criteria: [{id: C1, text: x}, {id: C2, text: y}], status: done, files: [src/textkit/__init__.py, tests/test_trunc.py, README.md], evidence: {C1: a}}\n");
    let c = r.judge(json!({})).await;
    for w in ["判据 C2（y）没有证据", "不在包的路径", "README.md"] {
        assert!(said(&c).contains(w), "{w:?} not in {}", said(&c));
    }
}

#[tokio::test]
async fn unclaimed_wants_every_change_since_the_base_in_some_packages_paths() {
    let r = worked("unclaimed").await;
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Pass, "{}", said(&c));
    assert_eq!(c.metrics["changed"], 2, "the committed change and the uncommitted one; not the run's own directory");

    let r = worked("unclaimed").await;
    r.write("README.md", "changed");
    r.write("docs/new.md", "n");
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("README.md") && said(&c).contains("docs/new.md"), "{}", said(&c));
    assert!(!said(&c).contains("src/textkit"), "{}", said(&c));

    // A change committed since the base counts, though the work tree is clean.
    let r = worked("unclaimed").await;
    r.write("README.md", "changed");
    sh(&r.root, "git add -A && git -c user.email=x@y -c user.name=x commit -qm third");
    let c = r.judge(json!({})).await;
    assert_eq!(c.status, CheckStatus::Fail);
    assert!(said(&c).contains("README.md"), "{}", said(&c));

    // No plan: the gate cannot judge and says so.
    let r = Rig::new("unclaimed").await;
    let c = r.judge(json!({})).await;
    assert_ne!(c.status, CheckStatus::Pass);
    assert!(said(&c).contains("xpkg plan"), "{}", said(&c));
}
