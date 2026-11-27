//! LIVE：内置的 xreport 流程包（docs/design/xreport.md）在一个造好历史的临时仓库上真跑。
//!
//! - 包从全局库装进项目：`conix flow install xreport --yes --jobs yes`（flows.md §7.12）——包在 `.conix/flows/xreport/`，
//!   lock 记下它，建议的作业 `xreport-weekly` 进 `.conix/jobs.json`、本机停用。
//! - 统计（`xstat collect`，命令步骤）经执行服务跑系统的 git（flows.md §7.14）：两个作者、`.mailmap` 把一个人的旧名字并过来；
//!   每次起的 git 都在运行历史里记一条 `exec`，进 Agix 的沙箱（`enforced.backend` 是 `agix`，macOS 上读写与网络都守住）。
//! - 分析由真实模型照 `stats.json` 写 `analysis.md`；定稿（`xstat render`）后用例当主人点「报告定稿」，归档留三个文件。
//! - `flows.command_gates: off` 时统计站失败，原因写成人看得懂、知道该做什么的话（不调模型）。
//!
//! 单模型（`.env` 的缺省那家）：一个分析会话，几分钟、几万 token。

mod live;

use std::path::Path;
use std::time::Duration;

use live::{Answering, Live, QUICK};
use port::{Command, Event, Reply, RunId, RunOp, RunStatus, StationId, StationStatus};
use serde_json::{Value, json};

/// 一次运行跑完的上限：只有分析站用模型，至多打回两次。
const RUN: Duration = Duration::from_secs(900);

fn git(dir: &str, args: &[&str]) {
    let o = std::process::Command::new("git")
        .args(["-c", "commit.gpgsign=false"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn write(dir: &str, rel: &str, text: &str) {
    let p = Path::new(dir).join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// 把暂存的提交成 `name <email>`，作者时间与提交时间都是 `date`（统计的范围按提交时间筛）。
fn commit(dir: &str, name: &str, email: &str, date: &str, msg: &str) {
    let o = std::process::Command::new("git")
        .args(["-c", "commit.gpgsign=false", "commit", "-q", "-m", msg])
        .envs([
            ("GIT_AUTHOR_NAME", name),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_AUTHOR_DATE", date),
            ("GIT_COMMITTER_NAME", name),
            ("GIT_COMMITTER_EMAIL", email),
            ("GIT_COMMITTER_DATE", date),
        ])
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "commit {msg}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
}

/// 2026 年 10 月的四个提交（示例项目的 `init` 是今天的，不在范围里）：
///
/// | 谁 | 何时（作者的钟） | 改了什么 |
/// |---|---|---|
/// | Alice Chen | 10-05 09:00 +08:00 | `.mailmap` +1，`src/report.py` +3 |
/// | achen（旧地址，`.mailmap` 并成 Alice Chen） | 10-07 14:30 +08:00 | `src/report.py` +2 −1 |
/// | Bob Li | 10-12 10:00 +00:00 | `docs/guide.md` +4 |
/// | Bob Li | 10-20 16:00 +00:00 | `docs/guide.md` +1 |
fn history(dir: &str) {
    write(
        dir,
        ".mailmap",
        "Alice Chen <alice@example.com> achen <alice@old.example>\n",
    );
    write(dir, "src/report.py", "def report():\n    return []\n\n");
    git(dir, &["add", "-A"]);
    commit(
        dir,
        "Alice Chen",
        "alice@example.com",
        "2026-10-05T09:00:00+08:00",
        "加报告的骨架与 mailmap",
    );
    write(
        dir,
        "src/report.py",
        "def report(rows):\n    return list(rows)\n\n# 排序在调用的地方做\n",
    );
    git(dir, &["add", "-A"]);
    commit(
        dir,
        "achen",
        "alice@old.example",
        "2026-10-07T14:30:00+08:00",
        "报告收行",
    );
    write(
        dir,
        "docs/guide.md",
        "# 指南\n\n跑 report。\n结果是列表。\n",
    );
    git(dir, &["add", "-A"]);
    commit(
        dir,
        "Bob Li",
        "bob@example.com",
        "2026-10-12T10:00:00+00:00",
        "写使用指南",
    );
    write(
        dir,
        "docs/guide.md",
        "# 指南\n\n跑 report。\n结果是列表。\n空的时候是 []。\n",
    );
    git(dir, &["add", "-A"]);
    commit(
        dir,
        "Bob Li",
        "bob@example.com",
        "2026-10-20T16:00:00+00:00",
        "指南补一句",
    );
}

const RANGE: (&str, &str) = ("2026-10-01", "2026-10-31");

async fn start(c: &live::Client) -> RunId {
    match c
        .ok(Command::StartRun {
            flow: "xreport".into(),
            subject: "十月的提交".into(),
            inputs: serde_json::json!({"since": RANGE.0, "until": RANGE.1}),
            step: false,
            session: None,
        })
        .await
    {
        Reply::RunStarted(s) => s.id,
        r => panic!("start_run 回的是 {r:?}"),
    }
}

fn find_archive(dir: &Path) -> Option<std::path::PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.join("record.json").is_file())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn xreport_installed_from_the_library_reports_a_scripted_history_and_waits_for_the_owner() {
    let lv = Live::new();
    let to = lv.project_path();
    history(&to);

    // 从全局库装进项目，连同建议的作业（停用）。
    let out = lv.conix_flow_ok(&["install", "xreport", "--yes", "--jobs", "yes"]);
    assert!(
        out.contains("Added the job xreport-weekly (disabled)"),
        "{out}"
    );
    let installed = Path::new(&to).join(".conix/flows/xreport");
    for f in [
        "bundle.yaml",
        "flow.yaml",
        "commands/xstat.js",
        "skills/analyze/SKILL.md",
    ] {
        assert!(installed.join(f).is_file(), "{f}");
    }
    let lock: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(Path::new(&to).join(".conix/flows.lock.yaml")).unwrap(),
    )
    .unwrap();
    let entry = &lock["bundles"]["xreport"];
    let meta: serde_yaml::Value =
        serde_yaml::from_str(&std::fs::read_to_string(installed.join("bundle.yaml")).unwrap())
            .unwrap();
    assert_eq!(entry["from"].as_str(), Some("global"), "{lock:?}");
    assert_eq!(entry["version"], meta["version"], "{lock:?}");
    assert!(
        entry["digest"].as_str().is_some_and(|d| d.len() == 64),
        "{lock:?}"
    );
    let defs = lv.read_jobs();
    let weekly = defs
        .as_array()
        .unwrap()
        .iter()
        .find(|j| j["id"] == "xreport-weekly")
        .unwrap_or_else(|| panic!("{defs}"));
    assert_eq!(weekly["kind"]["flow"]["flow"], "xreport", "{weekly}");
    assert_eq!(weekly["kind"]["flow"]["inputs"]["days"], 7, "{weekly}");
    assert_eq!(
        lv.read_state("jobs-state.json")["xreport-weekly"]["enabled"],
        false
    );

    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let Reply::Jobs(jobs) = c.ok(Command::ListJobs).await else {
        panic!("list_jobs")
    };
    assert!(
        jobs.iter()
            .any(|j| j.id.0 == "xreport-weekly" && !j.enabled),
        "实例读得出、停着：{jobs:?}"
    );

    let id = start(&c).await;
    let rec_of = |lv: &Live| lv.read_state(&format!("flow-runs/{}.json", id.0));
    c.wait_event("定稿等主人", RUN, |e| match e {
        Event::RunDetail(v) if v.summary.id == id => {
            if let RunStatus::Waiting { reason } = &v.summary.status
                && ["unmet", "failed", "held"].contains(&reason.as_str())
            {
                panic!("运行停下了：{reason}\n{:#?}", v.stations)
            }
            let waits = matches!(&v.summary.status, RunStatus::Waiting { reason } if reason == "confirm")
                && v.stations
                    .iter()
                    .any(|s| s.id.0 == "publish" && s.status == StationStatus::Working);
            waits.then_some(())
        }
        _ => None,
    })
    .await;

    let rec = rec_of(&lv);
    let out = rec["output_dir"].as_str().expect("运行的目录").to_string();
    assert!(out.starts_with(&format!("{to}/conix/runs/")), "{out}");
    let stats_text = std::fs::read_to_string(Path::new(&out).join("stats.json")).unwrap();
    let stats: Value = serde_json::from_str(&stats_text).unwrap();
    let report = std::fs::read_to_string(Path::new(&out).join("REPORT.md")).unwrap();
    eprintln!(
        "---- analysis.md\n{}\n---- REPORT.md\n{report}",
        std::fs::read_to_string(Path::new(&out).join("analysis.md")).unwrap_or_default()
    );
    // 统计是 git 数的：范围里四个提交、两个人（achen 并进了 Alice Chen），邮箱缺省不记。
    let t = &stats["totals"];
    assert_eq!(t["commits"], 4, "{t}");
    assert_eq!(t["authors"], 2, "{t}");
    let mut names: Vec<&str> = stats["authors"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|a| a["name"].as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["Alice Chen", "Bob Li"], "{stats_text}");
    assert!(
        !stats_text.contains("example.com") && !stats_text.contains("old.example"),
        "邮箱不记：{stats_text}"
    );
    assert_eq!(stats["range"]["since"], RANGE.0);
    assert_eq!(stats["range"]["until"], RANGE.1);
    // 报告的头与表从 stats.json 生成：名字是并过的，数对得上。
    assert!(
        report.contains(&format!("- 范围：{} 至 {}", RANGE.0, RANGE.1)),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "- 提交 {} 个（不含合并），合并 {} 个；作者 2 人；+{} / −{} 行",
            t["commits"], t["merges"], t["insertions"], t["deletions"]
        )),
        "{report}"
    );
    for a in stats["authors"].as_array().unwrap() {
        let row = format!(
            "| {} | {} | {} | +{} / −{} |",
            a["name"].as_str().unwrap(),
            a["commits"],
            a["merges"],
            a["insertions"],
            a["deletions"]
        );
        assert!(report.contains(&row), "{row}\n{report}");
    }
    assert!(!report.contains("achen"), "旧名字并掉了：{report}");
    assert!(!report.contains("example.com"), "没有邮箱：{report}");
    for section in ["## 概览", "## 贡献者", "## 时间分布", "## 热点", "## 观察"] {
        assert!(report.contains(section), "{section}");
    }

    // 统计起的 git 都记进了运行历史：argv、档位、后端守住了什么（不记输出）。
    let execs: Vec<Value> = lv
        .run_history(&id.0)
        .into_iter()
        .filter(|h| h["op"] == "exec")
        .collect();
    let gits: Vec<&Value> = execs
        .iter()
        .filter(|h| h["detail"]["argv"][0] == "git")
        .collect();
    assert!(!gits.is_empty(), "没有 git 的 exec 记录：{execs:#?}");
    for h in &gits {
        let d = &h["detail"];
        eprintln!(
            "---- exec [{}] {} → {}",
            h["station"], d["argv"], d["enforced"]
        );
        assert_eq!(h["station"], "collect", "{h}");
        assert!(
            d["for"].as_str().is_some_and(|f| f.contains("xstat")),
            "{h}"
        );
        assert_eq!(
            d["profile"]["kind"], "workspace",
            "命令步骤是 workspace：{h}"
        );
        assert_eq!(d["network"], false, "包没声明网络：{h}");
        assert_eq!(d["code"], 0, "{h}");
        let e = &d["enforced"];
        assert_eq!(e["backend"], "agix", "进的是引擎的沙箱：{h}");
        if cfg!(target_os = "macos") {
            assert_eq!(
                (&e["filesystem"], &e["network"]),
                (&json!(true), &json!(true)),
                "macOS 上整份守住：{h}"
            );
        }
        assert!(d.get("stdout").is_none(), "不记输出：{h}");
    }

    // 主人定稿，运行结束、归档。
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::Confirm {
            station: StationId("publish".into()),
            reject: false,
            reason: Some("报告定稿".into()),
        },
    })
    .await;
    c.wait_event("归档后结束", RUN, |e| match e {
        Event::RunDetail(v) if v.summary.id == id => {
            (v.summary.status == RunStatus::Finished).then_some(())
        }
        _ => None,
    })
    .await;
    let mut archived = None;
    for _ in 0..100 {
        archived = find_archive(&Path::new(&to).join("conix/xreport/archive"));
        if archived.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let archived = archived.expect("归档了");
    assert!(
        archived
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-xreport"),
        "{}",
        archived.display()
    );
    for f in ["REPORT.md", "analysis.md", "stats.json", "record.json"] {
        assert!(archived.join(f).is_file(), "{}/{f}", archived.display());
    }
    assert_eq!(
        std::fs::read_to_string(archived.join("REPORT.md")).unwrap(),
        report,
        "归档的就是批的那一份"
    );
}

/// 这台机器关掉了流程起进程（全局 `conix.yaml` 的 `flows.command_gates: off`）：统计站起不了 git，失败的原因说清楚
/// 开关在哪、改成什么；没有 `stats.json`。统计站是命令步骤，不调模型。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn with_commands_off_the_collect_step_fails_saying_which_switch_to_turn() {
    let lv = Live::with(live::Opts {
        conix_yaml: Some("flows:\n  command_gates: off\n".into()),
        ..Default::default()
    });
    history(&lv.project_path());
    lv.conix_flow_ok(&["install", "xreport", "--yes", "--jobs", "no"]);
    let c = lv.start_trusted().await;
    let id = start(&c).await;
    let deadline = std::time::Instant::now() + QUICK;
    let err = loop {
        let rec = lv.try_state(&format!("flow-runs/{}.json", id.0));
        if let Some(e) = rec.as_ref().and_then(|r| {
            r["stations"]["collect"]["tasks"]
                .as_array()?
                .iter()
                .find_map(|t| t["error"].as_str().map(String::from))
        }) {
            break e;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "统计站没有失败：{rec:#?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(
        err.contains("flows.command_gates") && err.contains("trusted"),
        "{err}"
    );
    let rec = lv.read_state(&format!("flow-runs/{}.json", id.0));
    let out = rec["output_dir"].as_str().unwrap();
    assert!(!Path::new(out).join("stats.json").exists());
}
