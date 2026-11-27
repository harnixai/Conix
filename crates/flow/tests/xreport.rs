//! The xreport bundle Conix ships (`examples/xforge/.conix/flows/xreport`, in Conix; its
//! docs/design/xreport.md): a report on a project's git history. Over a repository made here — two
//! authors (one under two identities a `.mailmap` joins), chosen dates, times and offsets, a rename,
//! a binary file, a merge — `xstat collect` counts exactly, by the author's own clock; what goes into
//! git's argv is checked first; where nothing may run it says what to do; `render` is the same
//! twice; a real run goes collect → analyze (its session played here) → publish → the owner →
//! finished, the three files archived. The bundle holds together; its doctor speaks of a repository
//! and of a directory that is none. No model.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use harnix_flow::bundle::{self, BundleMeta};
use harnix_flow::runner::{CatalogPacks, CommandBy, CommandCall, PlainExec, Runner, RunnerConfig, ScriptedHost, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

const T0: u64 = 1_794_787_200_000;

fn example() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/xforge")
}

/// As Conix offers the bundle: a built-in directory of bundles, command gates on.
fn packs() -> DirPacks {
    let catalog = MemPacks::new().scene("code").main_scene("code").submit_scene("code").command_gates().catalog();
    DirPacks::new([example().join(".conix/flows")]).with_catalog(catalog)
}

fn layout(wkts: &Path) -> ProjectLayout {
    ProjectLayout::new(".conix", wkts, "conix/flow").with_artifacts_dir("conix")
}

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").args(["-c", "commit.gpgsign=false", "-c", "init.defaultBranch=main"]).args(args).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// Commit what is staged as `name <email>` at `date` (authored and committed then).
fn commit(dir: &Path, name: &str, email: &str, date: &str, msg: &str) {
    let o = Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "--allow-empty", "-m", msg])
        .envs([("GIT_AUTHOR_NAME", name), ("GIT_AUTHOR_EMAIL", email), ("GIT_AUTHOR_DATE", date)])
        .envs([("GIT_COMMITTER_NAME", name), ("GIT_COMMITTER_EMAIL", email), ("GIT_COMMITTER_DATE", date)])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(o.status.success(), "commit {msg}: {}", String::from_utf8_lossy(&o.stderr));
}

fn write(dir: &Path, rel: &str, text: impl AsRef<[u8]>) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The history the counts below are of. Carol's commit is a month before the range; every other is in
/// November 2026, at the author's own time:
///
/// | | who | when (their clock) | what |
/// |---|---|---|---|
/// | c1 | Carol | 10-01 | old.txt, .mailmap (out of range) |
/// | c2 | Alice | Mon 11-02 09:15 +08:00 | src/a.txt +3, README.md +1, bin.dat (binary) |
/// | c3 | Bob | Wed 11-04 22:30 −05:00 (Thu 03:30 UTC) | src/a.txt +2 −1, docs/guide.md +4 |
/// | c4 | alice, an old address `.mailmap` makes Alice's | Sat 11-07 14:00 +08:00 | src/a.txt → src/b.txt, bin.dat again |
/// | c5 | Bob, on a branch | Tue 11-10 10:00 +00:00 | lib/x.txt +2 |
/// | c6 | Alice | Tue 11-10 11:00 +08:00 | README.md +1 |
/// | m | Bob merges the branch | Wed 11-11 09:00 +00:00 | — |
fn history() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let r = d.path();
    git(r, &["init", "-q"]);
    write(r, "old.txt", "old\n");
    write(r, ".mailmap", "Alice <alice@example.com> alice <alice@old.example>\n");
    git(r, &["add", "-A"]);
    commit(r, "Carol", "carol@example.com", "2026-10-01T12:00:00+00:00", "init");
    write(r, "src/a.txt", "1\n2\n3\n");
    write(r, "README.md", "# r\n");
    write(r, "bin.dat", [0u8, 1, 2, 0]);
    git(r, &["add", "-A"]);
    commit(r, "Alice", "alice@example.com", "2026-11-02T09:15:00+08:00", "加 a 与说明");
    write(r, "src/a.txt", "1\nX\n3\n4\n");
    write(r, "docs/guide.md", "a\nb\nc\nd\n");
    git(r, &["add", "-A"]);
    commit(r, "Bob", "bob@example.com", "2026-11-04T22:30:00-05:00", "改 a，写指南");
    git(r, &["mv", "src/a.txt", "src/b.txt"]);
    write(r, "bin.dat", [0u8, 9, 9, 0]);
    git(r, &["add", "-A"]);
    commit(r, "alice", "alice@old.example", "2026-11-07T14:00:00+08:00", "a 改名为 b | 换图");
    git(r, &["checkout", "-q", "-b", "feature"]);
    write(r, "lib/x.txt", "x\ny\n");
    git(r, &["add", "-A"]);
    commit(r, "Bob", "bob@example.com", "2026-11-10T10:00:00+00:00", "加 lib");
    git(r, &["checkout", "-q", "main"]);
    write(r, "README.md", "# r\nmore\n");
    git(r, &["add", "-A"]);
    commit(r, "Alice", "alice@example.com", "2026-11-10T11:00:00+08:00", "说明多一行");
    let o = Command::new("git")
        .args(["-c", "commit.gpgsign=false", "merge", "-q", "--no-ff", "-m", "合并 feature", "feature"])
        .envs([("GIT_AUTHOR_NAME", "Bob"), ("GIT_AUTHOR_EMAIL", "bob@example.com"), ("GIT_AUTHOR_DATE", "2026-11-11T09:00:00+00:00")])
        .envs([("GIT_COMMITTER_NAME", "Bob"), ("GIT_COMMITTER_EMAIL", "bob@example.com"), ("GIT_COMMITTER_DATE", "2026-11-11T09:00:00+00:00")])
        .current_dir(r)
        .output()
        .unwrap();
    assert!(o.status.success(), "merge: {}", String::from_utf8_lossy(&o.stderr));
    d
}

/// November 2026, and `more`.
fn inputs(more: Value) -> Value {
    let mut v = json!({"since": "2026-11-01", "until": "2026-11-30"});
    v.as_object_mut().unwrap().extend(more.as_object().unwrap().clone());
    v
}

/// The runner over a repository, its analyze session played by the test.
struct Drive {
    rn: Arc<Runner>,
    engine: Arc<Engine>,
    host: Arc<ScriptedHost>,
    root: PathBuf,
    _project: tempfile::TempDir,
    _wkts: tempfile::TempDir,
    seen: Mutex<HashSet<(String, String, String)>>,
    /// What the analyze sessions write, in turn; then [`ANALYSIS`].
    reports: Mutex<Vec<String>>,
}

async fn drive(project: tempfile::TempDir, command_gates: bool) -> Drive {
    drive_as(project, command_gates, true).await
}

async fn drive_as(project: tempfile::TempDir, command_gates: bool, trusted: bool) -> Drive {
    let root = std::fs::canonicalize(project.path()).unwrap();
    let wkts = tempfile::tempdir().unwrap();
    let cfg = RunnerConfig {
        command_gates,
        profiles: vec!["code".into()],
        main_profile: Some("code".into()),
        submit_profiles: ["code".to_string()].into(),
        lease_ms: 1500,
        tick_ms: 1000,
        ..Default::default()
    };
    let layout = layout(&std::fs::canonicalize(wkts.path()).unwrap());
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
    host.trust(&root, trusted);
    let rn = Runner::new(engine.clone(), host.clone(), cfg);
    rn.set_trust(&root, trusted);
    rn.start().await;
    Drive { rn, engine, host, root, _project: project, _wkts: wkts, seen: Default::default(), reports: Default::default() }
}

fn person() -> Actor {
    Actor::new("person", "owner")
}

const ANALYSIS: &str = "# 提交历史分析\n\n## 概览\n11 月里 5 个提交、1 个合并，两位作者，+13 / −1 行。\n\n\
    ## 贡献者\n- Alice：3 个提交，+5 行。\n- Bob：2 个提交、1 个合并，+8 / −1 行。\n\n\
    ## 时间分布\n周二最多（2 个），时间从 9 点到 22 点。\n\n## 热点\nsrc/ 改得最多，src/a.txt 是改动最多的文件。\n\n\
    ## 观察\n- 有一次改名（a 改名为 b），提交说明里提到清理 TODO 的没有。\n";

impl Drive {
    async fn start(&self, inputs: Value) -> String {
        let input = CreateIn {
            flow: "xreport".into(),
            subject: "11 月的提交".into(),
            inputs,
            project_root: Some(self.root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let o = self.rn.start_run(input, person()).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        self.rn.apply(o).await;
        run
    }

    async fn record(&self, run: &str) -> RunRecord {
        self.engine.record(run).await.unwrap()
    }

    fn out(&self, rec: &RunRecord) -> PathBuf {
        PathBuf::from(rec.output_dir.clone().unwrap())
    }

    /// `xstat <args>` in the run, for a person.
    async fn xstat(&self, run: &str, args: &[&str]) -> std::result::Result<String, String> {
        let mut c = CommandCall::new("xreport", "xstat", args.iter().map(|a| a.to_string()).collect(), CommandBy::person("owner"));
        c.run = Some(run.to_string());
        self.rn.run_command(c).await.map(|r| r.text)
    }

    /// Play every analyze session not played yet (write the report, hand in), until `done`.
    async fn until(&self, run: &str, what: &str, done: impl Fn(&RunRecord) -> bool) -> RunRecord {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
        loop {
            let rec = self.record(run).await;
            if done(&rec) {
                return rec;
            }
            assert!(rec.is_active(), "{what}: the run ended: {:?}", rec.status);
            let failed: Vec<_> = rec.stations.values().flat_map(|s| &s.tasks).filter(|t| t.status == TaskStatus::Failed).collect();
            assert!(failed.is_empty(), "{what}: {failed:#?}");
            assert!(tokio::time::Instant::now() < deadline, "{what}: not reached: {:#?}", self.engine.get(run).await.unwrap().unmet);
            for spec in self.host.opened() {
                let Some(id) = self.host.session_of(&spec.key) else { continue };
                let Some(b) = self.rn.binding(&id) else { continue };
                if b.station == "analyze" && self.seen.lock().unwrap().insert((b.run.clone(), b.task.clone(), b.token.clone())) {
                    let rec = self.record(&b.run).await;
                    let text = { let mut r = self.reports.lock().unwrap(); if r.is_empty() { ANALYSIS.to_string() } else { r.remove(0) } };
                    std::fs::write(self.out(&rec).join("analysis.md"), text).unwrap();
                    let r = self.rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"summary": "两人、5 个提交"}})).await;
                    assert!(!r.is_error && r.text.starts_with("Accepted"), "{}", r.text);
                }
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }

    /// Start a run and wait for its stats: what collect wrote.
    async fn collected(&self, inputs: Value) -> (String, Value) {
        let run = self.start(inputs).await;
        let rec = self.until(&run, "collected", |r| r.stations.get("collect").is_some_and(|s| s.status == StationStatus::Passed)).await;
        let stats: Value = serde_json::from_str(&std::fs::read_to_string(self.out(&rec).join("stats.json")).unwrap()).unwrap();
        (run, stats)
    }
}

fn author<'a>(stats: &'a Value, name: &str) -> &'a Value {
    stats["authors"].as_array().unwrap().iter().find(|a| a["name"] == name).unwrap_or_else(|| panic!("no {name}: {}", stats["authors"]))
}

fn column(rows: &Value, key: &str) -> Vec<Value> {
    rows.as_array().unwrap().iter().map(|r| r[key].clone()).collect()
}

#[test]
fn the_bundle_and_every_part_it_names_hold_together() {
    let p = packs();
    let meta: BundleMeta = serde_yaml::from_str(&p.bundle_file("xreport", "bundle.yaml").unwrap()).unwrap();
    assert_eq!((meta.name.as_str(), meta.version.as_str(), meta.pace), ("xreport", "0.1.0", Some(Pace::Auto)));
    assert_eq!(meta.commands.names().collect::<Vec<_>>(), ["xstat"]);
    assert_eq!(meta.commands.get("xstat").unwrap().read_only, ["show", "help"]);
    assert!(meta.exec.is_none(), "no network, nothing of the environment");
    assert!(meta.requires.is_none());
    let defaults = &meta.settings.as_ref().unwrap().defaults;
    assert_eq!((&defaults["privacy"]["subjects"], &defaults["privacy"]["emails"]), (&json!(true), &json!(false)));
    let stems = bundle::offered_stems(&p, "xreport");
    assert!(meta.start_problems(&stems, &|s| p.skill(&format!("xreport/{s}")).is_some()).is_empty());
    assert!(meta.doctor_problems(&|s| p.bundle_file("xreport", &format!("scripts/{s}.js")).is_some()).is_empty());
    assert_eq!(harnix_flow::kernel::validate::bundle_problems("xreport", &p), Vec::<String>::new());
    let src = p.bundle_file("xreport", "commands/xstat.js").unwrap();
    assert!(bundle::command_help(&src).unwrap().contains("xstat collect"), "the usage is its file's header");

    let (def, _) = p.flow("xreport").expect("flow.yaml is offered as the bundle's name");
    assert_eq!(validate(&def, &p), Vec::<Problem>::new());
    assert_eq!(def.stations.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["collect", "analyze", "publish"]);
    assert_eq!(def.inputs.keys().collect::<Vec<_>>(), ["days", "paths", "rev", "since", "until"]);
    assert!(def.inputs.values().all(|i| i.default.is_some()), "every input may be left out");
    assert_eq!(def.archive.as_ref().unwrap().keep, ["REPORT.md", "analysis.md", "stats.json"]);
    for (st, command) in [("collect", "collect"), ("publish", "render")] {
        assert!(matches!(&def.station(st).unwrap().executor, Some(Executor::Command { command: c, args }) if c == "xstat" && args == &[command]), "{st}: no model");
    }
    let publish = def.station("publish").unwrap();
    let confirm: Vec<_> = publish.exit.iter().filter_map(|e| match e {
        ExitCond::Confirm { who, label, separate, binds, .. } => Some((who.as_deref(), label.as_deref(), separate.clone(), binds.clone())),
        _ => None,
    }).collect();
    assert_eq!(confirm, [(Some("owner"), Some("报告定稿"), Some(OrSetting::Setting("${settings.review.separate}".into())), Some(Binds::List(vec!["${output_dir}/REPORT.md".into()])))]);
    for s in &def.stations {
        for e in &s.exit {
            if let ExitCond::Gate { gate, .. } = e {
                let (text, _) = p.part(PartKind::Gate, gate).unwrap_or_else(|| panic!("no gate {gate}"));
                assert_eq!(part_problems(PartKind::Gate, gate, &text, &p), Vec::<String>::new(), "{gate}");
            }
        }
    }
    assert!(p.skill("xreport/analyze").unwrap().0.contains("stats.json"));
    // A start with what git would read as an option is refused by the flow's own inputs.
    for bad in [json!({"since": "-x"}), json!({"rev": "--all"}), json!({"paths": ["-x"]}), json!({"paths": [":!src"]}), json!({"paths": ["/etc"]})] {
        assert!(def.apply_inputs(&bad).is_err(), "{bad}");
    }
    assert_eq!(def.apply_inputs(&json!({})).unwrap(), json!({"since": "", "days": 0, "until": "", "rev": "HEAD", "paths": []}));
}

/// Exact counts: by author (the `.mailmap` joining Alice's two identities), lines without the
/// binary file, the rename counted as no lines, the merge on its own; the weekday and the hour on
/// each author's own clock; directories and files by how much changed.
#[tokio::test]
async fn collect_counts_the_history_exactly_on_each_authors_own_clock() {
    let d = drive(history(), true).await;
    let (_, s) = d.collected(inputs(json!({}))).await;
    assert_eq!(s["range"]["since"], "2026-11-01");
    assert_eq!(s["range"]["head"].as_str().unwrap(), git(&d.root, &["rev-parse", "HEAD"]).trim());
    assert_eq!(s["truncated"], false);
    let t = &s["totals"];
    assert_eq!(
        (&t["commits"], &t["merges"], &t["authors"], &t["insertions"], &t["deletions"], &t["files"], &t["binary_changes"], &t["active_days"]),
        (&json!(5), &json!(1), &json!(2), &json!(13), &json!(1), &json!(6), &json!(2), &json!(4)),
        "{t}"
    );
    assert_eq!(column(&s["authors"], "name"), [json!("Alice"), json!("Bob")], "Carol is out of the range; alice is Alice");
    let a = author(&s, "Alice");
    assert_eq!((&a["commits"], &a["merges"], &a["insertions"], &a["deletions"], &a["files"], &a["active_days"]), (&json!(3), &json!(0), &json!(5), &json!(0), &json!(4), &json!(3)), "{a}");
    assert_eq!((a["first"].as_str(), a["last"].as_str()), (Some("2026-11-02T09:15:00+08:00"), Some("2026-11-10T11:00:00+08:00")));
    let b = author(&s, "Bob");
    assert_eq!((&b["commits"], &b["merges"], &b["insertions"], &b["deletions"], &b["files"], &b["active_days"]), (&json!(2), &json!(1), &json!(8), &json!(1), &json!(3), &json!(2)), "{b}");
    assert_eq!(b["last"], "2026-11-11T09:00:00Z", "the merge is Bob's latest");
    assert!(a.get("email").is_none() && !s.to_string().contains("example.com"), "no address unless the settings say");

    // Bob's Wednesday 22:30 is Thursday 03:30 in UTC: his own clock.
    assert_eq!(column(&s["by_weekday"], "commits"), [1, 2, 1, 0, 0, 1, 0].map(|n| json!(n)));
    let hours: Vec<(u64, u64)> = s["by_hour"].as_array().unwrap().iter().filter(|h| h["commits"] != 0).map(|h| (h["hour"].as_u64().unwrap(), h["commits"].as_u64().unwrap())).collect();
    assert_eq!(hours, [(9, 1), (10, 1), (11, 1), (14, 1), (22, 1)]);
    assert_eq!(column(&s["by_day"], "date"), ["2026-11-02", "2026-11-04", "2026-11-07", "2026-11-10"].map(|d| json!(d)));
    assert_eq!(column(&s["by_dir"], "dir"), ["src", "docs", ".", "lib"].map(|d| json!(d)));
    assert_eq!(s["by_dir"][0], json!({"dir": "src", "commits": 3, "insertions": 5, "deletions": 1, "files": 2}));
    assert_eq!(column(&s["top_files"], "path"), ["src/a.txt", "docs/guide.md", "README.md", "lib/x.txt", "bin.dat", "src/b.txt"].map(|p| json!(p)));
    let bin = s["top_files"].as_array().unwrap().iter().find(|f| f["path"] == "bin.dat").unwrap();
    assert_eq!(bin, &json!({"path": "bin.dat", "commits": 2, "insertions": 0, "deletions": 0, "binary": true}));
    let renamed = s["top_files"].as_array().unwrap().iter().find(|f| f["path"] == "src/b.txt").unwrap();
    assert_eq!((&renamed["insertions"], &renamed["deletions"]), (&json!(0), &json!(0)), "a rename moves no lines");

    // The list for the model: newest first, the merge marked, subjects as git has them.
    let commits = s["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 6);
    assert_eq!((&commits[0]["subject"], &commits[0]["merge"]), (&json!("合并 feature"), &json!(true)));
    assert!(commits.iter().any(|c| c["subject"] == "a 改名为 b | 换图" && c["author"] == "Alice"));
    assert_eq!(commits[0]["sha"].as_str().unwrap().len(), 12);
}

/// The settings decide what is kept: an address only when asked, no subjects when turned off; and
/// the paths given narrow it; no start date is the 30 days to the end.
#[tokio::test]
async fn the_settings_say_whether_addresses_and_subjects_are_kept_and_paths_narrow_it() {
    let repo = history();
    write(repo.path(), ".conix/governance/xreport.yaml", "privacy: {emails: true, subjects: false}\n");
    let d = drive(repo, true).await;
    // No since: until and the 30 days before it (range.days).
    let (_, s) = d.collected(json!({"until": "2026-11-30", "paths": ["src"]})).await;
    assert_eq!(s["range"]["since"], "2026-11-01");
    // The input days, as the weekly job gives, wins over range.days.
    let (_, w) = d.collected(json!({"until": "2026-11-30", "days": 7})).await;
    assert_eq!(w["range"]["since"], "2026-11-24");
    assert_eq!(s["privacy"], json!({"subjects": false, "emails": true}));
    assert_eq!(author(&s, "Alice")["email"], "alice@example.com", "the address .mailmap gives");
    assert!(s["commits"].as_array().unwrap().iter().all(|c| c.get("subject").is_none()), "{}", s["commits"]);
    assert_eq!(s["range"]["paths"], json!(["src"]));
    assert_eq!((&s["totals"]["commits"], &s["totals"]["files"]), (&json!(3), &json!(2)));
    assert_eq!(column(&s["by_dir"], "dir"), [json!("src")]);
}

/// What would go into git's argv is checked before: a date that is none, a rev git would read as an
/// option, a path out of the repository or with pathspec magic, an option not known.
#[tokio::test]
async fn what_goes_into_gits_argv_is_checked_first() {
    let d = drive(history(), true).await;
    let (run, _) = d.collected(inputs(json!({}))).await;
    for (args, why) in [
        (&["collect", "--since=-x"][..], "since 要写成 YYYY-MM-DD"),
        (&["collect", "--since", "-x"], "since 要写成 YYYY-MM-DD"),
        (&["collect", "--until", "2026-02-30"], "不是日历上的一天"),
        (&["collect", "--since", "2026-12-01", "--until", "2026-11-01"], "在 until"),
        (&["collect", "--rev", "-x"], "rev 不能以 - 开头"),
        (&["collect", "--rev=--all"], "rev 不能以 - 开头"),
        (&["collect", "--rev", "main..feature"], "rev 的写法不对"),
        (&["collect", "--rev", "HEAD;rm"], "rev 只收"),
        (&["collect", "--rev", "nope"], "找不到提交"),
        (&["collect", "--path", "../etc"], "路径不能有 .."),
        (&["collect", "--path", "src/../../x"], "路径不能有 .."),
        (&["collect", "--path", ":(top)x"], "pathspec"),
        (&["collect", "--path", "/etc"], "相对项目根"),
        (&["collect", "--path=-x"], "不能以 - 开头"),
        (&["collect", "--author", "x"], "不认识的选项 --author"),
    ] {
        let e = d.xstat(&run, args).await.unwrap_err();
        assert!(e.contains(why), "{args:?}: {e}");
    }
    let ok = d.xstat(&run, &["collect", "--rev", "feature", "--path", "lib"]).await.unwrap();
    assert!(ok.contains("\"commits\": 1"), "{ok}");
    let shown = d.xstat(&run, &["show"]).await.unwrap();
    assert!(shown.contains("Bob：1 个提交"), "{shown}");
}

/// Where the host's switch is off, or the project is not trusted, nothing starts, and the step says
/// what to do in words a person reads.
#[tokio::test]
async fn where_nothing_may_run_the_step_says_what_to_do() {
    for (commands, trusted, says) in [(false, true, ["flows.command_gates", "trusted"]), (true, false, ["没受信", "/trust"])] {
        let d = drive_as(history(), commands, trusted).await;
        let run = d.start(inputs(json!({}))).await;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
        let err = loop {
            let rec = d.record(&run).await;
            if let Some(e) = rec.stations.get("collect").and_then(|s| s.tasks.iter().find_map(|t| t.error.clone())) {
                break e;
            }
            assert!(tokio::time::Instant::now() < deadline, "{:?}", rec.stations);
            tokio::time::sleep(Duration::from_millis(25)).await;
        };
        assert!(says.iter().all(|w| err.contains(w)), "{says:?}: {err}");
        assert!(!d.out(&d.record(&run).await).join("stats.json").exists());
    }
}

/// A real run: collect, the analysis played, the final report built twice to the same bytes, the
/// owner confirms it, the run finishes and its archive holds the three files.
#[tokio::test]
async fn a_run_goes_collect_analyze_publish_and_waits_for_the_owner_then_archives() {
    let d = drive(history(), true).await;
    let run = d.start(inputs(json!({}))).await;
    let rec = d
        .until(&run, "waits for the owner", |r| {
            r.stations.get("publish").is_some_and(|s| s.checks.iter().any(|c| c.label == "报告定稿" && c.status == CheckStatus::Wait))
        })
        .await;
    assert_eq!(rec.stations["analyze"].status, StationStatus::Passed);
    let out = d.out(&rec);
    let report = std::fs::read_to_string(out.join("REPORT.md")).unwrap();
    assert!(report.starts_with("# 提交历史报告\n"), "{report}");
    assert!(report.contains("- 范围：2026-11-01 至 2026-11-30"), "{report}");
    assert!(report.contains("## 观察\n\n- 有一次改名"), "the analysis as written: {report}");
    assert!(report.contains("| Alice | 3 | 0 | +5 / −0 | 4 | 3 | 2026-11-02 | 2026-11-10 |"), "{report}");
    assert!(report.contains("| 周二 | 2 |"), "{report}");
    assert!(report.contains("| bin.dat | 2 | +0 / −0（含二进制） |"), "{report}");
    assert!(!report.contains("example.com"), "{report}");
    let render = rec.stations["publish"].tasks[0].output.clone().unwrap();
    assert_eq!(render["report"], "REPORT.md");

    // The same two files, the same report: the person approves what can be built again.
    d.xstat(&run, &["render"]).await.unwrap();
    assert_eq!(std::fs::read_to_string(out.join("REPORT.md")).unwrap(), report);

    d.rn.settle(d.engine.confirm(&run, "publish", true, None, person()).await).await.unwrap();
    let rec = d.until(&run, "finished", |r| r.status == RunStatus::Finished).await;
    let archive = d.root.join(&rec.archive.as_ref().unwrap().dir);
    let mut kept: Vec<String> = std::fs::read_dir(&archive).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    kept.sort();
    assert_eq!(kept, ["REPORT.md", "analysis.md", "record.json", "stats.json"], "{}", archive.display());
    assert!(archive.starts_with(d.root.join("conix/xreport/archive")), "{}", archive.display());
    assert_eq!(std::fs::read_to_string(archive.join("REPORT.md")).unwrap(), report);
    let vote = &rec.stations["publish"].confirms[0];
    assert!(vote.binds.as_ref().unwrap().keys().any(|k| k.ends_with("REPORT.md")));
    // Outside the run, the latest archive's stats.
    let mut c = CommandCall::new("xreport", "xstat", vec!["show".into()], CommandBy::model("main"));
    c.project_root = Some(d.root.clone());
    let shown = d.rn.run_command(c).await.unwrap().text;
    assert!(shown.contains("archive/") && shown.contains("Alice：3 个提交"), "{shown}");
}

/// An analysis with a section missing, or one left to fill, does not pass: the station goes back to
/// its session with why, and the next one passes.
#[tokio::test]
async fn an_analysis_left_to_fill_goes_back_to_its_session() {
    let d = drive(history(), true).await;
    d.reports.lock().unwrap().extend([
        ANALYSIS.replace("## 热点\n", "## 热门\n"),
        ANALYSIS.replace("周二最多（2 个），时间从 9 点到 22 点。", "- TODO：按小时再看"),
    ]);
    let run = d.start(inputs(json!({}))).await;
    let rec = d.until(&run, "publish reached", |r| r.stations.get("publish").is_some_and(|s| s.status.is_open())).await;
    let analyze = &rec.stations["analyze"];
    assert_eq!((analyze.status, analyze.round), (StationStatus::Passed, 3), "{analyze:?}");
    let sent = d.host.opened().iter().filter_map(|s| d.host.session_of(&s.key)).flat_map(|id| d.host.sent_to(&id)).collect::<Vec<_>>().join("\n");
    assert!(sent.contains("## 热点") && sent.contains("TODO"), "each time told why: {sent}");
}

fn doctor_exec(refused: Option<&str>) -> DoctorExec {
    DoctorExec { exec: Arc::new(PlainExec), refused: refused.map(String::from), env_deny: vec![] }
}

#[tokio::test]
async fn the_doctor_is_quiet_on_a_repository_and_says_what_is_missing_elsewhere() {
    let p = packs();
    let repo = history();
    let root = std::fs::canonicalize(repo.path()).unwrap();
    let l = layout(Path::new("/nowhere"));
    let r = doctor_with(&p, Some(&l), "xreport", &root, Some(&doctor_exec(None))).await.unwrap();
    assert_eq!(r.script.as_deref(), Some("scripts/doctor.js"));
    let loud: Vec<_> = r.findings.iter().filter(|f| f.level != DoctorLevel::Info).collect();
    assert!(loud.is_empty(), "{r:?}");
    assert!(r.findings.iter().any(|f| f.message.contains(".mailmap")), "{r:?}");
    assert!(r.findings.iter().any(|f| f.message.contains("用缺省")), "{r:?}");

    let empty = tempfile::tempdir().unwrap();
    let r = doctor_with(&p, Some(&l), "xreport", empty.path(), Some(&doctor_exec(None))).await.unwrap();
    assert!(r.findings.iter().any(|f| f.level == DoctorLevel::Error && f.message.contains("不是 git 仓库")), "{r:?}");

    write(&root, ".conix/governance/xreport.yaml", "privacy: {emails: \"no\"}\nrange: {days: 0}\n");
    let r = doctor_with(&p, Some(&l), "xreport", &root, Some(&doctor_exec(None))).await.unwrap();
    let errors: Vec<_> = r.findings.iter().filter(|f| f.level == DoctorLevel::Error).map(|f| f.message.as_str()).collect();
    assert!(errors.len() == 2 && errors.iter().any(|m| m.contains("privacy.emails")) && errors.iter().any(|m| m.contains("range.days")), "{errors:?}");
    std::fs::remove_file(root.join(".conix/governance/xreport.yaml")).unwrap();

    // Why nothing starts comes in the library's words in a run and in the host's, in its language, in
    // `conix flow doctor`: each names the switch, or what to do, all the same.
    for (why, says) in [
        ("the project is not trusted: what it asks to run does not run", "/trust"),
        ("项目没受信（或看不出来）：包的自检不起进程（在实例里 /trust）", "/trust"),
        ("the project is not trusted (or that cannot be told): a bundle's doctor starts no process (/trust in the instance)", "/trust"),
        ("commands are not turned on here (the host's switch for every command a project or a bundle runs)", "flows.command_gates 是 off"),
        ("命令没开（conix.yaml 的 flows.command_gates: off）：包的自检不起进程", "flows.command_gates 是 off"),
        ("commands are off (flows.command_gates: off in conix.yaml): a bundle's doctor starts no process", "flows.command_gates 是 off"),
    ] {
        let r = doctor_with(&p, Some(&l), "xreport", &root, Some(&doctor_exec(Some(why)))).await.unwrap();
        assert!(
            r.findings.iter().any(|f| f.level == DoctorLevel::Error && f.message.contains(says) && !f.message.contains("git 起不来")),
            "{why}: {r:?}"
        );
    }
    let r = doctor(&p, Some(&l), "xreport", &root).await.unwrap();
    assert!(r.findings.iter().any(|f| f.level == DoctorLevel::Warn && f.message.contains("没有执行服务")), "{r:?}");
}
