//! LIVE：Conix 内置的 xforge 流程包（源头 `examples/xforge/.conix/flows/xforge`）在演示项目 `examples/xforge` 上真跑。
//! 演示项目自己带着这个包（`.conix/flows/xforge/`，就是内置的源头），拷进临时仓库就是"装在项目里"，不用 `conix flow install`。
//!
//! `xforge/solid`，从提案到归档（docs/design/layout.md）：
//!
//! - 项目的制品目录配成 `xforge`（`.conix/conix.yaml`）：运行在 `xforge/runs/<运行>/`，包的数据（规格、接口、归档）就在 `xforge/`；
//!   两条基线都开着（`.conix/governance/xforge.yaml`）。
//! - 结构化的数据由包的命令写：规格增量（`xspec`）、接口增量（`xarch`）、台账（`xledger`）、工作包（`xpkg`）；策略不让模型直接写它们。
//! - 主会话在 Claude；独立检查是子 Agent，换 DeepSeek，逐条答原则。检查留下 open 的问题时，用例像人一样照检查的建议打回（至多三次）。
//! - 实施按工作包，每包交付时过任务级出口；两处人的确认（检查、验收）由用例来点；归档是两个命令步骤，
//!   规格与接口增量分别并进 `xforge/specs/`、`xforge/interfaces/`，项目的验证照样过。
//!
//! `xforge/dual`（docs/design/xforge.md §5.1、flows.md §6.5）：规格侧写一次、人批准、封存；两个子运行各在 `.wkts/` 下自己的
//! worktree 里、从同一个分叉提交出发，一个实现、一个只照规格写黑盒测试（`tests_blackbox/`）；合回父运行的分支，门禁 blackbox 用
//! 测试跑实现；验证、归档在父运行的 worktree 里，合不合进项目的分支由人定。用例演所有的人：答澄清的问题、规格侧定了、两路对上了、
//! 验收，破坏性改动的批准只在要的时候点（加一个函数不破坏什么，所以它记在 `not_required` 里）；检查、合起来没过时照建议打回，有上限。
//! 两份设计、两次检查、两路实施，比 solid 长得多：Claude 上大约 1–2 小时（每一步等 [`RUN`]），按两倍于 solid 的 token 估。
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
    StationStatus, TaskId, WorktreeEnd,
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
                tasks: vec![],
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
        // 决定与发现可以一条都没有（台账就不存在）；检查一定逐条答了原则。
        "ledgers/constitution.yaml",
    ] {
        assert!(Path::new(&out).join(f).is_file(), "{out}/{f}");
    }
    // 划的工作包里至少有一个登记了验证，名字来自项目治理文件的 verify 菜单：交付时门禁 package-tests 才有东西跑。
    let menu: serde_yaml::Value = serde_yaml::from_str(
        &std::fs::read_to_string(Path::new(&to).join(".conix/governance/xforge.yaml")).unwrap(),
    )
    .unwrap();
    let menu: Vec<String> = menu["verify"]
        .as_mapping()
        .map(|m| {
            m.keys()
                .filter_map(|k| k.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let plan_text = std::fs::read_to_string(Path::new(&out).join("work-packages.yaml")).unwrap();
    eprintln!("---- work-packages.yaml\n{plan_text}");
    let plan: serde_yaml::Value = serde_yaml::from_str(&plan_text).unwrap();
    let planned: Vec<String> = plan["packages"]
        .as_sequence()
        .into_iter()
        .flatten()
        .flat_map(|p| p["verify"].as_sequence().cloned().unwrap_or_default())
        .filter_map(|v| v["name"].as_str().map(String::from))
        .collect();
    assert!(
        planned.iter().any(|n| menu.contains(n)),
        "工作包要登记 verify 菜单（{menu:?}）里的验证，登记的是 {planned:?}"
    );
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
    // package-tests 真起了进程：它的日志（站目录里的 gate-package-tests.log）每跑一项记一行 `== <名字> [argv] in <目录>`。
    // 只看"过了"不够——没登记验证的包也是过。
    let logs = find_files(Path::new(&out), "gate-package-tests.log");
    let ran: Vec<String> = logs
        .iter()
        .filter_map(|l| std::fs::read_to_string(l).ok())
        .filter(|t| t.lines().any(|l| l.starts_with("== ")))
        .collect();
    assert!(!ran.is_empty(), "门禁 package-tests 没跑任何验证：{logs:?}");
    eprintln!("---- gate-package-tests.log\n{}", ran.join("\n----\n"));
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

/// `dir` 下（含子目录）叫 `name` 的文件。
fn find_files(dir: &Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            out.extend(find_files(&p, name));
        } else if e.file_name() == name {
            out.push(p);
        }
    }
    out
}

async fn view(c: &Client, id: &RunId) -> RunView {
    match c.ok(Command::GetRun { run: id.clone() }).await {
        Reply::Run(v) => *v,
        r => panic!("get_run 回的是 {r:?}"),
    }
}

/// 一站在等的、还没处理的建议退回的站，在 `allowed` 里才照它，否则退到 `fallback`。
fn proposed(v: &RunView, st: &str, allowed: &[&str], fallback: &str) -> String {
    v.proposals
        .iter()
        .rev()
        .find(|p| p.station.0 == st && p.status == ProposalStatus::Open)
        .map(|p| p.to.0.clone())
        .filter(|t| allowed.contains(&t.as_str()))
        .unwrap_or_else(|| fallback.into())
}

fn ledger_entries(path: &Path) -> Vec<serde_yaml::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_yaml::from_str::<serde_yaml::Value>(&t).ok())
        .and_then(|v| v["entries"].as_sequence().cloned())
        .unwrap_or_default()
}

/// 工作包登记的路径（`work-packages.yaml` 的 `packages[].paths`）。
fn package_paths(dir: &Path) -> Vec<String> {
    let text = std::fs::read_to_string(dir.join("work-packages.yaml"))
        .unwrap_or_else(|e| panic!("{}/work-packages.yaml：{e}", dir.display()));
    let plan: serde_yaml::Value = serde_yaml::from_str(&text).unwrap();
    plan["packages"]
        .as_sequence()
        .into_iter()
        .flatten()
        .flat_map(|p| p["paths"].as_sequence().cloned().unwrap_or_default())
        .filter_map(|p| p.as_str().map(String::from))
        .collect()
}

/// 演人的那一边：打回与重判都有上限，免得模型绕圈时用例一直烧钱。
#[derive(Default)]
struct People {
    confirmed: Vec<String>,
    clarify_rounds: u32,
    spec_reworks: u32,
    join_reworks: u32,
    child_reworks: std::collections::BTreeMap<String, u32>,
    /// 子运行还在跑时看到的它的 worktree（合回去之后就删了）。
    worktrees_seen: std::collections::BTreeSet<String>,
}

/// 子运行无人值守：自己打回过了还没达标，像人那样照它的建议（没有就退到设计）再打回一次，至多两次。
async fn tend_child(c: &mut Client, lv: &Live, child: &RunId, p: &mut People) {
    if let Some(rec) = lv.try_state(&format!("flow-runs/{}.json", child.0))
        && let Some(w) = rec["worktree"]["path"].as_str()
        && Path::new(w).is_dir()
    {
        p.worktrees_seen.insert(w.to_string());
    }
    let v = view(c, child).await;
    match &v.summary.status {
        RunStatus::Waiting { reason } if reason == "unmet" => {
            let st = v
                .stations
                .iter()
                .find(|s| s.status == StationStatus::Unmet)
                .map(|s| s.id.0.clone())
                .unwrap_or_default();
            let n = p.child_reworks.entry(child.0.clone()).or_default();
            *n += 1;
            assert!(
                *n <= 2,
                "子运行 {} 打回两次还没过：{:#?}",
                child.0,
                v.stations
            );
            let to = proposed(&v, &st, &["design", "check", "apply"], "design");
            eprintln!("==== 子运行 {} 在 {st} 没达标，打回到 {to}", child.0);
            c.ok(Command::RunOp {
                run: child.clone(),
                op: RunOp::Rework {
                    to: StationId(to),
                    tasks: vec![],
                    reason: Some("没达标的照台账与门禁的说明改，规格侧不动".into()),
                },
            })
            .await;
        }
        RunStatus::Waiting { reason } if ["failed", "held"].contains(&reason.as_str()) => {
            panic!("子运行 {} 停下了：{reason}\n{:#?}", child.0, v.stations)
        }
        RunStatus::Cancelled => panic!("子运行 {} 被取消了", child.0),
        _ => {}
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_builtin_xforge_dual_forks_an_implementation_and_black_box_tests_and_joins_them() {
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
        &format!("cp -R {}/. . && git add -A", example().display()),
    );
    // 检查（规格侧的、两路各自的）换一家模型，同 solid。
    for (file, skill) in [
        ("dual.yaml", "spec-check"),
        ("dual-impl.yaml", "dual-check"),
        ("dual-tests.yaml", "dual-check"),
    ] {
        let flow = Path::new(&to).join(".conix/flows/xforge").join(file);
        let text = std::fs::read_to_string(&flow).unwrap();
        let was = format!("    executor: {{kind: agent}}\n    skill: {skill}");
        assert!(text.contains(&was), "演示的 {file} 改了，这里跟着改");
        std::fs::write(
            &flow,
            text.replace(
                &was,
                &format!(
                    "    executor: {{kind: agent, model: {}}}\n    skill: {skill}",
                    ds.model
                ),
            ),
        )
        .unwrap();
    }
    // 提交了再起：运行的 worktree 从 HEAD 建，人最后要往这个分支上合。
    sh(
        &to,
        "git add -A && git -c user.name=live -c user.email=live@conix commit -qm xforge",
    );
    assert_eq!(sh(&to, "git status --porcelain"), "", "从干净的工作区开始");
    let base = sh(&to, "git rev-parse HEAD").trim().to_string();

    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = match c
        .ok(Command::StartRun {
            flow: "xforge/dual".into(),
            subject: "给 textkit 加 truncate(text, n)：文字超过 n 个字符时截成前 n-1 个字符再加一个省略号 …，不超过时原样返回；n 小于 1 时抛 ValueError。要一套照规格独立写的黑盒测试".into(),
            inputs: serde_json::json!({"title": "truncate", "risk": "high"}),
            step: false,
            session: None,
        })
        .await
    {
        Reply::RunStarted(s) => s.id,
        r => panic!("{r:?}"),
    };
    let rec_of = |lv: &Live, run: &str| lv.read_state(&format!("flow-runs/{run}.json"));

    let mut p = People::default();
    let done = loop {
        let at = c
            .wait_event("双路的运行有动静", RUN, |e| match e {
                Event::RunDetail(v)
                    if v.summary.id == id || v.summary.flow.starts_with("xforge/dual-") =>
                {
                    Some(v.summary.id.clone())
                }
                _ => None,
            })
            .await;
        if at != id {
            tend_child(&mut c, &lv, &at, &mut p).await;
            continue;
        }
        // 照此刻的样子判，不照事件里的：事件可能是做了上一步之前发的。
        let v = view(&c, &id).await;
        let out = rec_of(&lv, &id.0)["output_dir"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match &v.summary.status {
            RunStatus::Finished => break v,
            RunStatus::Cancelled => panic!("运行被取消了：{:#?}", v.stations),
            RunStatus::Waiting { reason } if reason == "confirm" => {
                let st = v
                    .stations
                    .iter()
                    .find(|s| s.status == StationStatus::Working)
                    .unwrap_or_else(|| panic!("等确认却没有在做的站：{:#?}", v.stations))
                    .id
                    .0
                    .clone();
                let why = match st.as_str() {
                    "propose" => "破坏性改动看过了，批准",
                    "clarify" => "范围定了",
                    "check" => "规格侧定了",
                    "join" => "两路对上了",
                    "verify" => "验收通过",
                    _ => "确认",
                };
                eprintln!("==== {st} 等人：{why}");
                report(&rec_of(&lv, &id.0));
                c.ok(Command::RunOp {
                    run: id.clone(),
                    op: RunOp::Confirm {
                        station: StationId(st.clone()),
                        reject: false,
                        reason: Some(why.into()),
                    },
                })
                .await;
                p.confirmed.push(st);
            }
            RunStatus::Waiting { reason } if reason == "unmet" => {
                let st = v
                    .stations
                    .iter()
                    .find(|s| s.status == StationStatus::Unmet)
                    .map(|s| s.id.0.clone())
                    .unwrap_or_default();
                eprintln!("==== {st} 没达标");
                report(&rec_of(&lv, &id.0));
                match st.as_str() {
                    // 澄清的问题只有人答：每条照它给的默认答，再重判这一站（/xforge:clarify）。
                    "clarify" => {
                        p.clarify_rounds += 1;
                        assert!(
                            p.clarify_rounds <= 3,
                            "答了三轮澄清还没过：{:#?}",
                            v.stations
                        );
                        let qs = ledger_entries(&Path::new(&out).join("ledgers/questions.yaml"));
                        for q in qs
                            .iter()
                            .filter(|q| q["conclusion"].as_str() == Some("open"))
                        {
                            let qid = q["id"].as_str().unwrap().to_string();
                            let text = q["text"].as_str().unwrap_or_default();
                            let answer = text
                                .split_once("默认：")
                                .map(|(_, d)| format!("照默认：{}", d.trim()))
                                .unwrap_or_else(|| "照你建议的做，范围不扩大".into());
                            eprintln!("---- 答 {qid}：{text} → {answer}");
                            c.ok(Command::FlowCommand {
                                bundle: "xforge".into(),
                                command: "xledger".into(),
                                args: vec![
                                    "set".into(),
                                    "questions".into(),
                                    qid,
                                    "answered".into(),
                                    answer,
                                ],
                                run: Some(id.clone()),
                            })
                            .await;
                        }
                        c.ok(Command::RunOp {
                            run: id.clone(),
                            op: RunOp::Goto {
                                station: StationId("clarify".into()),
                                reason: Some("问题答完了".into()),
                            },
                        })
                        .await;
                    }
                    "check" => {
                        p.spec_reworks += 1;
                        assert!(p.spec_reworks <= 3, "规格侧打回三次还没过");
                        let back = proposed(&v, "check", &["propose", "clarify"], "propose");
                        eprintln!("==== 规格侧打回到 {back}");
                        c.ok(Command::RunOp {
                            run: id.clone(),
                            op: RunOp::Rework {
                                to: StationId(back),
                                tasks: vec![],
                                reason: Some(
                                    "规格侧检查还留着 open 的问题：照台账改，改完让检查再看".into(),
                                ),
                            },
                        })
                        .await;
                    }
                    // 黑盒测试没过：错在哪一边由人定，这里照合起来那一站的建议。会话判出错在一路时（交出的
                    // `rework.tasks`），只重做分叉站里那一路，另一路的结果留着；判不出就两路都重新分叉。
                    "join" => {
                        p.join_reworks += 1;
                        assert!(p.join_reworks <= 2, "合起来打回两次还没对上");
                        let back = proposed(&v, "join", &["fork", "propose"], "fork");
                        let rec = rec_of(&lv, &id.0);
                        let tasks: Vec<TaskId> = (back == "fork")
                            .then(|| {
                                rec["stations"]["join"]["tasks"]
                                    .as_array()
                                    .and_then(|t| t.last())
                                    .and_then(|t| t["output"]["rework"]["tasks"].as_array())
                                    .map(|t| {
                                        t.iter()
                                            .filter_map(|x| x.as_str())
                                            .map(|x| TaskId(x.into()))
                                            .collect()
                                    })
                            })
                            .flatten()
                            .unwrap_or_default();
                        eprintln!("==== 只重做：{tasks:?}（空是整站）");
                        eprintln!(
                            "==== 合起来没对上，打回到 {back}\n---- ledgers/findings.yaml\n{}",
                            std::fs::read_to_string(Path::new(&out).join("ledgers/findings.yaml"))
                                .unwrap_or_default()
                        );
                        c.ok(Command::RunOp {
                            run: id.clone(),
                            op: RunOp::Rework {
                                to: StationId(back),
                                tasks,
                                reason: Some(
                                    "黑盒测试没过：照 findings 台账按规格修，不照对面凑".into(),
                                ),
                            },
                        })
                        .await;
                    }
                    other => panic!("{other} 没达标，没有该做的人：{:#?}", v.stations),
                }
            }
            RunStatus::Waiting { reason } if ["failed", "held"].contains(&reason.as_str()) => {
                panic!("运行停下了：{reason}\n{:#?}", v.stations)
            }
            _ => {}
        }
    };
    let rec = rec_of(&lv, &id.0);
    eprintln!(
        "==== 结束（澄清 {} 轮，规格侧打回 {} 次，合起来打回 {} 次，子运行打回 {:?}；确认了 {:?}）",
        p.clarify_rounds, p.spec_reworks, p.join_reworks, p.child_reworks, p.confirmed
    );
    report(&rec);
    for s in &done.stations {
        assert_eq!(s.status, StationStatus::Passed, "{}", s.id.0);
    }

    // 父运行在仓库根的 .wkts/ 下自己的 worktree 里，运行目录在那里面。
    let wt = rec["worktree"]["path"]
        .as_str()
        .expect("父运行的 worktree")
        .to_string();
    let branch = rec["worktree"]["branch"].as_str().unwrap().to_string();
    assert!(wt.starts_with(&format!("{to}/.wkts/")), "{wt}");
    let out = rec["output_dir"].as_str().unwrap().to_string();
    assert!(out.starts_with(&format!("{wt}/xforge/runs/")), "{out}");

    // 加一个函数不破坏什么：批准破坏性改动没要，记在提案站的 not_required 里（带读到的值），人没被问。
    let left_out = rec["stations"]["propose"]["not_required"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        left_out
            .iter()
            .any(|n| n["label"] == "批准破坏性改动" && n["value"] == false),
        "{left_out:?}"
    );
    assert!(
        !p.confirmed.iter().any(|s| s == "propose"),
        "{:?}",
        p.confirmed
    );
    let propose = done.stations.iter().find(|s| s.id.0 == "propose").unwrap();
    assert!(
        propose
            .conditional
            .iter()
            .any(|x| x.label == "批准破坏性改动" && x.required == Some(false)),
        "界面看得到这条为什么没要：{:?}",
        propose.conditional
    );
    for st in ["clarify", "check", "join", "verify"] {
        assert!(
            p.confirmed.iter().any(|s| s == st),
            "{st} 由人确认：{:?}",
            p.confirmed
        );
    }

    // 两个子运行都是 fork 站起的，从这一轮同一个分叉提交出发，各在 .wkts/ 下自己的 worktree 里。
    let fork = &rec["stations"]["fork"];
    let commit = fork["fork"]["commit"]
        .as_str()
        .expect("分叉提交")
        .to_string();
    let children: std::collections::BTreeMap<String, String> = fork["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["id"].as_str().unwrap().to_string(),
                t["child"].as_str().expect("任务的子运行").to_string(),
            )
        })
        .collect();
    assert_eq!(
        children.keys().collect::<Vec<_>>(),
        ["impl", "tests"],
        "{children:?}"
    );
    let crec: std::collections::BTreeMap<&str, Value> = children
        .iter()
        .map(|(t, run)| (t.as_str(), rec_of(&lv, run)))
        .collect();
    let path_of = |t: &str| crec[t]["worktree"]["path"].as_str().unwrap().to_string();
    for (task, other) in [("impl", "tests"), ("tests", "impl")] {
        let r = &crec[task];
        assert_eq!(r["status"], "finished", "{task}");
        assert_eq!(
            r["base_commit"].as_str(),
            Some(commit.as_str()),
            "{task}：同一个分叉提交"
        );
        let mine = path_of(task);
        assert!(
            mine.starts_with(&format!("{to}/.wkts/")) && mine != wt,
            "{task}: {mine}"
        );
        assert_ne!(mine, path_of(other));
        assert!(
            p.worktrees_seen.contains(&mine),
            "{task} 跑的时候它的 worktree 在：{:?}",
            p.worktrees_seen
        );
        // 读不到父运行的 worktree，也读不到兄弟的（名单在分叉提交时就定全了）。
        let f = &r["worktree"]["fork"];
        assert_eq!(f["parent_path"].as_str(), Some(wt.as_str()), "{task}");
        assert_eq!(f["parent_branch"].as_str(), Some(branch.as_str()), "{task}");
        let siblings: Vec<&str> = f["siblings"]
            .as_array()
            .expect("禁读的兄弟名单")
            .iter()
            .filter_map(|s| s.as_str())
            .collect();
        assert!(
            siblings.contains(&path_of(other).as_str()),
            "{task}: {siblings:?}"
        );
        // 交付了，派工器合回父运行的分支（不是项目的）。
        let end = &r["worktree"]["outcome"];
        assert_eq!(end["kind"], "merged", "{task}: {end}");
        assert_eq!(end["into"].as_str(), Some(branch.as_str()), "{task}");
        // 自己的目录是父运行目录下的 children/fork/<任务>/，合回来了。
        let own = r["output_dir"].as_str().unwrap();
        assert!(
            own.ends_with(&format!("children/fork/{task}")),
            "{task}: {own}"
        );
        let back = Path::new(&out).join("children/fork").join(task);
        for f in ["design.md", "work-packages.yaml"] {
            assert!(back.join(f).is_file(), "{}/{f}", back.display());
        }
        // 每一站都核对过规格侧没动。
        for st in ["design", "check", "apply"] {
            let checks = r["stations"][st]["checks"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                checks.iter().any(|c| c["label"]
                    .as_str()
                    .is_some_and(|l| l.contains("spec-unchanged"))
                    && c["status"] == "pass"),
                "{task} {st}: {checks:#?}"
            );
        }
    }
    // 测试那一路只写 tests_blackbox/，实现那一路改 src/、不碰黑盒测试。
    let tests_paths = package_paths(&Path::new(&out).join("children/fork/tests"));
    assert!(
        !tests_paths.is_empty() && tests_paths.iter().all(|p| p.starts_with("tests_blackbox/")),
        "{tests_paths:?}"
    );
    let impl_paths = package_paths(&Path::new(&out).join("children/fork/impl"));
    assert!(
        impl_paths.iter().any(|p| p.starts_with("src/"))
            && !impl_paths.iter().any(|p| p.starts_with("tests_blackbox/")),
        "{impl_paths:?}"
    );
    let changed = sh(&wt, &format!("git diff --name-only {commit} --"));
    eprintln!("---- 分叉之后改了\n{changed}");
    assert!(
        changed.lines().any(|l| l.starts_with("tests_blackbox/")),
        "{changed}"
    );
    assert!(
        changed.lines().any(|l| l == "src/textkit/__init__.py"),
        "{changed}"
    );

    // 门禁 blackbox 真跑了黑盒测试（日志每跑一项记一行 `== blackbox …`），最后一轮过了。
    let logs = find_files(Path::new(&out), "gate-blackbox.log");
    let ran: Vec<String> = logs
        .iter()
        .filter_map(|l| std::fs::read_to_string(l).ok())
        .filter(|t| t.lines().any(|l| l.starts_with("== blackbox")))
        .collect();
    assert!(!ran.is_empty(), "门禁 blackbox 没跑：{logs:?}");
    eprintln!("---- gate-blackbox.log\n{}", ran.join("\n----\n"));
    let join = rec["stations"]["join"]["checks"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(
        join.iter().any(
            |c| c["label"].as_str().is_some_and(|l| l.contains("blackbox"))
                && c["status"] == "pass"
        ),
        "{join:#?}"
    );
    // 它起的进程记进了运行历史（exec），带着后端守住了什么。
    let execs: Vec<Value> = lv
        .run_history(&id.0)
        .into_iter()
        .filter(|h| h["op"] == "exec")
        .collect();
    let bb = execs
        .iter()
        .find(|h| {
            h["detail"]["for"]
                .as_str()
                .is_some_and(|f| f.contains("gate blackbox"))
        })
        .unwrap_or_else(|| panic!("没有门禁 blackbox 的 exec 记录：{execs:#?}"));
    eprintln!("---- exec {}", bb["detail"]);
    assert!(bb["detail"]["enforced"]["backend"].is_string(), "{bb}");

    // 归档在父运行的 worktree 里：两条基线都并进了，两路的设计都留着。
    let spec = std::fs::read_to_string(Path::new(&wt).join("xforge/specs/textkit.md")).unwrap();
    assert!(spec.contains("### TK-R3"), "{spec}");
    let api =
        std::fs::read_to_string(Path::new(&wt).join("xforge/interfaces/textkit.yaml")).unwrap();
    assert!(api.contains("fn:textkit.truncate"), "{api}");
    let archived = Path::new(&wt).join(rec["archive"]["dir"].as_str().expect("归档了"));
    eprintln!(
        "---- {}\n{}",
        archived.display(),
        sh(&archived.to_string_lossy(), "find . -type f | sort")
    );
    for f in [
        "record.json",
        "proposal.md",
        "verification.md",
        "children/fork/impl/design.md",
        "children/fork/tests/design.md",
    ] {
        assert!(archived.join(f).is_file(), "{}/{f}", archived.display());
    }

    // 项目的分支没动：父运行的 worktree 怎么收尾由人定（`/worktree merge`，端口的 `EndWorktree`）。
    // 合之前项目里没有这次的改动；视图里写着等人收尾。
    assert!(
        rec["worktree"]["outcome"].is_null(),
        "没人决定过：{}",
        rec["worktree"]
    );
    assert_eq!(
        sh(&to, "git rev-parse HEAD").trim(),
        base,
        "项目的分支没被自动提交"
    );
    assert!(!sh(&to, "git show HEAD:src/textkit/__init__.py").contains("def truncate"));
    let w = view(&c, &id).await.worktree.expect("父运行有 worktree");
    assert!(w.decidable && w.branch == branch, "{w:?}");
    let Reply::Text { text } = c
        .ok(Command::RunOp {
            run: id.clone(),
            op: RunOp::EndWorktree {
                how: WorktreeEnd::Merged,
            },
        })
        .await
    else {
        panic!("收尾回一句话")
    };
    eprintln!("==== 收尾：{text}");
    let w = view(&c, &id).await.worktree.unwrap();
    assert_eq!(
        (w.outcome, w.decidable),
        (Some(WorktreeEnd::Merged), false),
        "{w:?}"
    );
    assert!(!Path::new(&wt).exists(), "合了之后 worktree 删了：{wt}");
    assert!(
        sh(&to, &format!("git branch --list {branch}"))
            .trim()
            .is_empty(),
        "分支也删了"
    );
    assert!(
        std::fs::read_to_string(Path::new(&to).join("src/textkit/__init__.py"))
            .unwrap()
            .contains("def truncate"),
        "人合进来之后才有"
    );
    for f in changed.lines().filter(|l| l.starts_with("tests_blackbox/")) {
        assert!(Path::new(&to).join(f).is_file(), "{f}");
    }
    eprintln!(
        "---- verify\n{}\n---- blackbox\n{}",
        sh(&to, "sh .conix/governance/verify.sh 2>&1"),
        sh(
            &to,
            "python3 -B -m unittest discover -s tests_blackbox -q 2>&1"
        )
    );
}
