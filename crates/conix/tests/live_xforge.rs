//! LIVE：Conix 内置的 xforge 流程包（源头 `examples/xforge/.conix/flows/xforge`）在演示项目 `examples/xforge` 上真跑
//! `xforge/solid`，从提案到归档（docs/design/layout.md）：
//!
//! - 项目的制品目录配成 `xforge`（`.conix/conix.yaml`）：运行在 `xforge/runs/<运行>/`，包的数据（规格、接口、归档）就在 `xforge/`；
//!   两条基线都开着（`.conix/governance/xforge.yaml`）。
//! - 结构化的数据由包的命令写：规格增量（`xspec`）、接口增量（`xarch`）、台账（`xledger`）、工作包（`xpkg`）；策略不让模型直接写它们。
//! - 主会话在 Claude；独立检查是子 Agent，换 DeepSeek，逐条答原则。检查留下 open 的问题时，用例像人一样照检查的建议打回（至多三次）。
//! - 实施按工作包，每包交付时过任务级出口；两处人的确认（检查、验收）由用例来点；归档是两个命令步骤，
//!   规格与接口增量分别并进 `xforge/specs/`、`xforge/interfaces/`，项目的验证照样过。
//!
//! 这条流程的约定见 docs/design/xforge.md。
//!
//! 没有 Claude 的 key 就跳过。

mod live;

use std::path::Path;
use std::time::Duration;

use live::{Answering, Client, Endpoint, Live};
use port::{
    Command, Event, ProposalStatus, Reply, RunId, RunOp, RunStatus, RunView, StationId,
    StationStatus,
};
use serde_json::Value;

const RUN: Duration = Duration::from_secs(3000);

fn example() -> &'static Path {
    Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../examples/xforge"
    ))
}

fn providers(cl: &Endpoint, ds: &Endpoint) -> Value {
    let one = |id: &str, e: &Endpoint| {
        serde_json::json!({
            "id": id, "vendor": "anthropic", "api": "anthropic",
            "base_url": e.base_url, "api_key_env": e.key_env, "models": [e.model],
            "tiers": {"strong": e.model, "balanced": e.model, "fast": e.model},
            "quota": {"day": 1_000_000_000u64},
        })
    };
    serde_json::json!({
        "providers": [one("claude", cl), one("deepseek", ds)],
        "model": {"order": ["claude", "deepseek"]},
    })
}

fn sh(dir: &str, cmd: &str) -> String {
    let o = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{cmd}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

struct LogOnPanic<'a>(&'a Live);

impl Drop for LogOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("---- host log ----\n{}", self.0.host_log());
        }
    }
}

/// `unmet_ok`：这一站没达标时交回来（用例像人那样决定打回）；别的站没达标就是卡住了。
async fn wait_run(
    c: &mut Client,
    id: &RunId,
    what: &str,
    unmet_ok: Option<&str>,
    want: impl Fn(&RunView) -> bool,
) -> RunView {
    c.wait_event(what, RUN, |e| match e {
        Event::RunDetail(v) if v.summary.id == *id => {
            if let Some(st) = unmet_ok
                && v.stations
                    .iter()
                    .any(|s| s.id.0 == st && s.status == StationStatus::Unmet)
                && matches!(&v.summary.status, RunStatus::Waiting { .. })
            {
                return Some((**v).clone());
            }
            if let RunStatus::Waiting { reason } = &v.summary.status
                && ["unmet", "failed", "held"].contains(&reason.as_str())
            {
                panic!("运行停下了：{reason}\n{:#?}", v.stations)
            }
            if v.summary.status == RunStatus::Cancelled {
                panic!("运行被取消了：{:#?}", v.stations)
            }
            want(v).then(|| (**v).clone())
        }
        _ => None,
    })
    .await
}

fn waiting_at(v: &RunView, st: &str) -> bool {
    v.stations
        .iter()
        .any(|s| s.id.0 == st && s.status == StationStatus::Working)
        && matches!(&v.summary.status, RunStatus::Waiting { reason } if reason == "confirm")
}

/// 每站每道检查的结论，给人看。
fn report(rec: &Value) {
    let Some(stations) = rec["stations"].as_object() else {
        return;
    };
    for (st, r) in stations {
        eprintln!("[{st}] {}", r["status"]);
        for c in r["checks"].as_array().into_iter().flatten() {
            eprintln!(
                "    {} → {} {}",
                c["label"],
                c["status"],
                c["detail"].as_str().unwrap_or_default()
            );
        }
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_xforge_runs_a_change_from_proposal_to_archive_with_its_own_commands() {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&cl, &ds)),
        ..Default::default()
    });
    let _log = LogOnPanic(&lv);
    let to = lv.project_path();
    sh(
        &to,
        &format!(
            "cp -R {}/. . && git add -A && git -c user.name=live -c user.email=live@conix commit -qm xforge",
            example().display()
        ),
    );
    assert_eq!(sh(&to, "git status --porcelain"), "", "从干净的工作区开始");
    // 独立检查换一家模型。
    let flow = Path::new(&to).join(".conix/flows/xforge/solid.yaml");
    let text = std::fs::read_to_string(&flow).unwrap();
    assert!(
        text.contains("    executor: {kind: agent}\n    skill: check"),
        "演示的 solid 改了，这里跟着改"
    );
    std::fs::write(
        &flow,
        text.replace(
            "    executor: {kind: agent}\n    skill: check",
            &format!(
                "    executor: {{kind: agent, model: {}}}\n    skill: check",
                ds.model
            ),
        ),
    )
    .unwrap();

    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = match c
        .ok(Command::StartRun {
            flow: "xforge/solid".into(),
            subject: "给 textkit 加 truncate(text, n)：文字超过 n 个字符时截成前 n-1 个字符再加一个省略号 …，不超过时原样返回".into(),
            inputs: serde_json::json!({"title": "truncate", "risk": "medium"}),
            step: false,
            session: None,
        })
        .await
    {
        Reply::RunStarted(s) => s.id,
        r => panic!("{r:?}"),
    };
    let rec_of = |lv: &Live| lv.read_state(&format!("flow-runs/{}.json", id.0));
    let confirm = |st: &str, why: &str| RunOp::Confirm {
        station: StationId(st.into()),
        reject: false,
        reason: Some(why.into()),
    };

    // 运行在制品目录 xforge/ 下自己的目录里，包的数据就是 xforge/。
    let rec = rec_of(&lv);
    let out = rec["output_dir"].as_str().expect("运行的目录").to_string();
    assert!(out.starts_with(&format!("{to}/xforge/runs/")), "{out}");
    assert_eq!(
        rec["data_dir"].as_str(),
        Some(format!("{to}/xforge").as_str())
    );

    // 独立检查：问题留成 open 时台账门禁挡住这一站，人看了打回设计（至多两次），过了就确认。
    let mut reworks = 0;
    let mut proposed_back = false;
    loop {
        let v = wait_run(&mut c, &id, "独立检查有结论", Some("check"), |v| {
            waiting_at(v, "check")
        })
        .await;
        eprintln!("==== 独立检查第 {} 轮", reworks + 1);
        report(&rec_of(&lv));
        eprintln!(
            "---- ledgers/findings.yaml\n{}",
            std::fs::read_to_string(Path::new(&out).join("ledgers/findings.yaml"))
                .unwrap_or_default()
        );
        if waiting_at(&v, "check") {
            break;
        }
        reworks += 1;
        assert!(reworks <= 3, "打回三次检查还没过");
        // 像人那样听检查的建议：问题的根在提案就退到提案，没有建议就退到设计。
        let to = v
            .proposals
            .iter()
            .rev()
            .find(|p| p.station.0 == "check" && p.status == ProposalStatus::Open)
            .map(|p| p.to.0.clone())
            .filter(|t| t == "propose" || t == "design")
            .unwrap_or_else(|| "design".into());
        eprintln!(
            "==== 打回到 {to}（建议：{:?}）",
            v.proposals
                .iter()
                .map(|p| (&p.to.0, &p.reason))
                .collect::<Vec<_>>()
        );
        if to == "propose" {
            proposed_back = true;
        }
        c.ok(Command::RunOp {
            run: id.clone(),
            op: RunOp::Rework {
                to: StationId(to),
                reason: Some("检查还留着 open 的问题：照台账改，改完让检查再看".into()),
            },
        })
        .await;
    }
    // 结构化的数据都是命令写的（策略不让模型直接写这几样）。
    for f in [
        "spec-delta.yaml",
        "interface-delta.yaml",
        "work-packages.yaml",
        "ledgers/decisions.yaml",
        "ledgers/findings.yaml",
        "ledgers/constitution.yaml",
    ] {
        assert!(Path::new(&out).join(f).is_file(), "{out}/{f}");
    }
    c.ok(Command::RunOp {
        run: id.clone(),
        op: confirm("check", "检查过了，开工"),
    })
    .await;

    wait_run(&mut c, &id, "走到验收等人", None, |v| {
        waiting_at(v, "verify")
    })
    .await;
    c.ok(Command::RunOp {
        run: id.clone(),
        op: confirm("verify", "验收通过"),
    })
    .await;

    let done = wait_run(&mut c, &id, "归档后结束", None, |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
    let rec = rec_of(&lv);
    eprintln!("==== 结束（打回 {reworks} 次，按建议退到提案：{proposed_back}）");
    report(&rec);
    for s in &done.stations {
        assert_eq!(s.status, StationStatus::Passed, "{}", s.id.0);
    }

    // 规格增量并进了规格；代码与测试改了，项目的验证照样过。
    let spec = std::fs::read_to_string(Path::new(&to).join("xforge/specs/textkit.md")).unwrap();
    eprintln!("---- xforge/specs/textkit.md\n{spec}");
    assert!(spec.contains("### TK-R3"), "归档并进了新需求");
    // 接口基线也开着：新函数登记进了 textkit 模块（新元素缺省是试验）。
    let api =
        std::fs::read_to_string(Path::new(&to).join("xforge/interfaces/textkit.yaml")).unwrap();
    eprintln!("---- xforge/interfaces/textkit.yaml\n{api}");
    assert!(api.contains("fn:textkit.truncate"), "归档并进了接口增量");
    assert!(
        std::fs::read_to_string(Path::new(&to).join("src/textkit/__init__.py"))
            .unwrap()
            .contains("def truncate")
    );
    eprintln!(
        "---- verify\n{}",
        sh(&to, "sh .conix/governance/verify.sh 2>&1")
    );

    // 归档：留下的制品与 record.json 在 xforge/archive/<日期序号>-truncate/。
    let mut archived = None;
    for _ in 0..100 {
        archived = std::fs::read_dir(Path::new(&to).join("xforge/archive"))
            .ok()
            .and_then(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .find(|p| p.join("record.json").is_file())
            });
        if archived.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let archived = archived.expect("归档了");
    eprintln!(
        "---- {}\n{}",
        archived.display(),
        sh(&archived.to_string_lossy(), "find . -type f | sort")
    );
    assert!(
        archived
            .file_name()
            .unwrap()
            .to_string_lossy()
            .ends_with("-truncate"),
        "{}",
        archived.display()
    );
    for f in ["proposal.md", "design.md", "verification.md"] {
        let t = std::fs::read_to_string(archived.join(f)).unwrap_or_else(|e| panic!("{f}: {e}"));
        assert!(!t.contains("conix:template"), "{f} 写过了：{t}");
    }
    let kept: Value =
        serde_json::from_str(&std::fs::read_to_string(archived.join("record.json")).unwrap())
            .unwrap();
    assert_eq!(kept["status"], "finished");
    let ignored = sh(&to, "git status --porcelain --ignored xforge/runs");
    assert!(
        ignored.lines().all(|l| l.starts_with("!!")),
        "运行的目录不进 git：{ignored}"
    );
}
