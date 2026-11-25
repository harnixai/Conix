//! LIVE：仓库里的演示项目（`examples/demo`）照原样拷进项目里跑，真实的实例、引擎与模型。流程治理的每样都在
//! 这里走一遍（design/flows.md）：
//!
//! - `brief`：一次运行把流程的功能全走到。
//!   - 声明的门禁读模型写的 YAML（`outline.yaml`）、JSONL（两份审读）、Markdown，`check` 的 schema / files / text /
//!     sealed 都真判过、证据进了运行记录。
//!   - `command` 门禁跑项目自己的命令行（`tools/notesctl.py`）：数简报的字数，逐条核对审读里的引文出自原文。
//!   - `script` 门禁（QuickJS）读项目的治理文件 `governance/policy.yaml`，对照交付物下"能不能签发"的结论。
//!   - 多模型：主会话在 DeepSeek；分节的子 Agent 与汇总的独立会话在 Claude；审读站的两个工作项**同时**跑，
//!     一个在 DeepSeek、一个在 Claude。两家都真花了 token；子 Agent 的交付核实过是答完了的子 Agent 给的。
//!   - 两种授权：签发站人确认（模型没有这个工具、名单上没有的人做不了、实例的主人做了才过，记成 `person`）；
//!     发布站经 MCP 在外部审批系统建单、在那边批了才过（Agix 的 `mcp.callTool`，记下审批人与单的链接）。
//! - `digest`：每份笔记一个子运行，父任务拿到的是子流程声明的 `result`；`children` 是 `require: any`。
//! - `publish`：单独的外部审批，不开会话。
//! - 作业：演示项目带的两个作业读得出来、克隆下来是停用的；简单作业立即跑一次就有结果。
//!
//! 演示项目里写的模型名换成 `.env` 里的；没有 Claude 的 key 就跳过 `brief`。

mod live;

use std::path::Path;
use std::time::Duration;

use live::{Answering, Client, Endpoint, Live};
use port::{
    Command, Event, JobId, JobOutcome, Reply, RunId, RunOp, RunStatus, RunView, StationId,
    StationStatus, TaskKind,
};
use serde_json::Value;

/// `brief` 一次：提纲、两三节、汇总、两份审读，加上可能的打回。
const RUN: Duration = Duration::from_secs(2400);

fn demo() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../examples/demo"))
}

/// 两个 provider：DeepSeek 在前（主会话在它上面），Claude 只认自己的模型。
fn providers(ds: &Endpoint, cl: &Endpoint) -> Value {
    let one = |id: &str, e: &Endpoint| {
        serde_json::json!({
            "id": id, "vendor": "anthropic", "api": "anthropic",
            "base_url": e.base_url, "api_key_env": e.key_env, "models": [e.model],
            "tiers": {"strong": e.model, "balanced": e.model, "fast": e.model},
            "quota": {"day": 1_000_000_000u64},
        })
    };
    serde_json::json!({
        "providers": [one("deepseek", ds), one("claude", cl)],
        "model": {"order": ["deepseek", "claude"]},
    })
}

/// 演示项目拷进实例的项目。给了两家的模型就把 `brief` 里写的模型名换成这次用的：按上下文整段换，
/// 要换成的名字可能就是演示里写的某一个（CLAUDE_MODEL 常是 claude-sonnet-5-5），逐个替换会改两遍。
fn install_demo(lv: &Live, models: Option<(&Endpoint, &Endpoint)>) {
    let to = lv.project_path();
    let st = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", demo().display()))
        .arg(&to)
        .status()
        .unwrap();
    assert!(st.success(), "拷演示项目");
    let Some((ds, cl)) = models else { return };
    let brief = Path::new(&to).join(".conix/flows/brief/flow.yaml");
    let text = std::fs::read_to_string(&brief).unwrap();
    let swaps = [
        (
            "kind: agent, model: claude-haiku-5-5",
            format!("kind: agent, model: {}", cl.model),
        ),
        (
            "provider: claude, model: claude-sonnet-5-5",
            format!("provider: claude, model: {}", cl.model),
        ),
        (
            "provider: deepseek, model: deepseek-flash",
            format!("provider: deepseek, model: {}", ds.model),
        ),
    ];
    let mut out = String::new();
    for line in text.lines() {
        let mut l = line.to_string();
        if let Some((from, to)) = swaps.iter().find(|(f, _)| line.contains(f)) {
            l = line.replace(from, to);
        }
        out.push_str(&l);
        out.push('\n');
    }
    for (from, _) in &swaps {
        assert!(
            text.contains(from),
            "演示的 brief 里没有 {from}：改了演示要跟着改这里"
        );
    }
    std::fs::write(&brief, out).unwrap();
}

fn rig() -> Option<(Endpoint, Endpoint, Live)> {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return None;
    };
    let ds = live::deepseek();
    let lv = Live::with(live::Opts {
        settings: Some(providers(&ds, &cl)),
        ..Default::default()
    });
    install_demo(&lv, Some((&ds, &cl)));
    Some((ds, cl, lv))
}

async fn start(c: &Client, flow: &str, subject: &str, inputs: Value) -> RunId {
    match c
        .ok(Command::StartRun {
            flow: flow.into(),
            subject: subject.into(),
            inputs,
        })
        .await
    {
        Reply::RunStarted(s) => s.id,
        r => panic!("start_run 回的是 {r:?}"),
    }
}

async fn wait_run(
    c: &mut Client,
    id: &RunId,
    what: &str,
    want: impl Fn(&RunView) -> bool,
) -> RunView {
    c.wait_event(what, RUN, |e| match e {
        Event::RunDetail(v) if v.summary.id == *id && want(v) => Some((**v).clone()),
        Event::RunDetail(v) if v.summary.id == *id && v.summary.status == RunStatus::Cancelled => {
            panic!("运行被取消了：{:#?}", v.stations)
        }
        _ => None,
    })
    .await
}

fn station<'a>(v: &'a RunView, id: &str) -> &'a port::StationView {
    v.stations
        .iter()
        .find(|s| s.id.0 == id)
        .unwrap_or_else(|| panic!("没有站 {id}"))
}

/// 运行记录（流程库的存储，`state/conix/flow-runs/<运行>.json`）。
fn record(lv: &Live, id: &RunId) -> Value {
    lv.read_state(&format!("flow-runs/{}.json", id.0))
}

/// 一站最新的那条检查（`label` 是 `gate <名>`、`confirm by <谁>` 这种）。流程包里的门禁带包名
/// （`gate brief/outline-shape`），按它自己的名字也认。
fn check<'a>(rec: &'a Value, st: &str, label: &str) -> &'a Value {
    let same = |l: &str| {
        l == label
            || l.strip_prefix("gate ")
                .and_then(|r| r.split_once('/'))
                .is_some_and(|(_, own)| format!("gate {own}") == label)
    };
    rec["stations"][st]["checks"]
        .as_array()
        .and_then(|cs| {
            cs.iter()
                .rev()
                .find(|c| c["label"].as_str().is_some_and(same))
        })
        .unwrap_or_else(|| panic!("站 {st} 没有检查 {label}：{}", rec["stations"][st]))
}

fn passed<'a>(rec: &'a Value, st: &str, gate: &str) -> &'a Value {
    let c = check(rec, st, &format!("gate {gate}"));
    assert_eq!(c["status"], "pass", "门禁 {gate}：{c}");
    c
}

fn both_spent(lv: &Live) {
    for id in ["deepseek", "claude"] {
        let ledger = lv.home.path().join("quota").join(format!("{id}.json"));
        let text = std::fs::read_to_string(&ledger).unwrap_or_else(|e| {
            panic!("{id} 没有用量记录（没被调用过？）{}: {e}", ledger.display())
        });
        assert!(
            text.chars().any(|c| c.is_ascii_digit() && c != '0'),
            "{id} 花了 token：{text}"
        );
    }
}

struct LogOnPanic<'a>(&'a Live);

impl Drop for LogOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("---- host log ----\n{}", self.0.host_log());
        }
    }
}

fn confirm(station: &str, reason: Option<&str>) -> RunOp {
    RunOp::Confirm {
        station: StationId(station.into()),
        reject: false,
        reason: reason.map(String::from),
    }
}

/// 一个任务记录的会话在哪个模型上。
fn model_of(all: &[port::SessionSummary], sid: &str) -> Option<String> {
    all.iter()
        .find(|x| x.id.0 == sid)
        .map(|x| x.provenance.model.clone())
}

/// 外部审批：等 `station` 那条 MCP 的确认建好单，像审批人那样在演示的审批系统里批，返回单号。
async fn approve_outside(lv: &Live, id: &RunId, station: &str) -> String {
    let mut approval = Value::Null;
    for _ in 0..240 {
        let rec = record(lv, id);
        let c = check(&rec, station, "confirm by editor").clone();
        assert_ne!(c["status"], "error", "建单失败：{c}");
        if rec["stations"][station]["approvals"]
            .as_array()
            .is_some_and(|a| !a.is_empty())
        {
            approval = rec;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(
        !approval.is_null(),
        "外部审批没有动静：{}",
        record(lv, id)["stations"][station]
    );
    let req = &approval["stations"][station]["approvals"][0]["request"];
    let rid = req["id"]
        .as_str()
        .unwrap_or_else(|| panic!("建单的结果里没有 id：{req}"))
        .to_string();
    let st = std::process::Command::new("python3")
        .args(["tools/approvals_mcp.py", "approve", &rid])
        .current_dir(lv.project_path())
        .status()
        .unwrap();
    assert!(st.success());
    rid
}

/// 外部审批过了的那条确认：经 MCP、审批系统答的审批人、链到那张单。
fn approved_outside(rec: &Value, station: &str, rid: &str) {
    let last = rec["stations"][station]["confirms"]
        .as_array()
        .and_then(|v| v.last())
        .cloned()
        .expect("确认记录");
    assert_eq!(
        (last["via"].as_str(), last["by"]["kind"].as_str()),
        (Some("mcp"), Some("mcp")),
        "{last}"
    );
    // 流程写了 `link: url`：记的是单的链接，链到这张单。
    assert!(
        last["ref"]
            .as_str()
            .is_some_and(|r| r.ends_with(&format!("#{rid}"))),
        "{last}"
    );
    assert_eq!(
        last["by"]["id"].as_str(),
        std::env::var("USER").ok().as_deref(),
        "审批系统答的审批人：{last}"
    );
    assert_eq!(last["pass"], true, "{last}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_demo_brief_runs_every_kind_of_gate_on_two_models_and_both_kinds_of_sign_off() {
    let Some((ds, cl, lv)) = rig() else { return };
    let _log = LogOnPanic(&lv);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = start(&c, "brief", "服务可靠性：超时、重试与缓存", Value::Null).await;

    // 一路走到签发：门禁不过的由 on_unmet 打回，不该停下等人；门禁判不了（error）也不该有。
    let at_sign_off = wait_run(&mut c, &id, "走到签发", |v| {
        let stuck = match &v.summary.status {
            RunStatus::Waiting { reason } => ["unmet", "failed", "held"].contains(&reason.as_str()),
            _ => false,
        };
        if stuck
            || v.stations.iter().any(|s| {
                s.status == StationStatus::Unmet
                    && s.unmet.as_deref().is_some_and(|u| u.contains("error"))
            })
        {
            panic!("运行停下了：{:?}\n{:#?}", v.summary.status, v.stations)
        }
        station(v, "sign-off").status == StationStatus::Working
            && matches!(v.summary.status, RunStatus::Waiting { .. })
    })
    .await;
    for st in ["outline", "sections", "assemble", "reviews", "govern"] {
        assert_eq!(
            station(&at_sign_off, st).status,
            StationStatus::Passed,
            "{st}"
        );
    }

    // 声明的门禁：YAML、Markdown、JSONL 都真读过，证据进了运行记录。
    let rec = record(&lv, &id);
    let out = rec["output_dir"].as_str().expect("输出目录").to_string();
    let outline = passed(&rec, "outline", "outline-shape");
    assert!(
        outline["evidence"].to_string().contains("outline.yaml"),
        "{outline}"
    );
    let sections = passed(&rec, "sections", "sections-written");
    assert!(
        sections["evidence"]
            .as_array()
            .is_some_and(|e| e.len() >= 2),
        "{sections}"
    );
    passed(&rec, "assemble", "brief-shape");
    for g in ["facts-shape", "policy-shape"] {
        let r = passed(&rec, "reviews", g);
        assert!(r["evidence"].to_string().contains(".jsonl"), "{r}");
    }
    for f in [
        "outline.yaml",
        "brief.md",
        "reviews/facts.jsonl",
        "reviews/policy.jsonl",
    ] {
        assert!(Path::new(&out).join(f).is_file(), "{out}/{f}");
    }

    // 命令门禁：项目自己的命令行数了字数、逐条核对了引文。
    let stats = passed(&rec, "assemble", "brief-stats");
    assert!(
        stats["metrics"]["chars"].as_u64().unwrap_or(0) >= 300,
        "{stats}"
    );
    let verified = passed(&rec, "reviews", "reviews-verified");
    assert!(
        verified["metrics"]["supported"].as_u64().unwrap_or(0) >= 1,
        "{verified}"
    );

    // 脚本门禁：按治理文件下的结论，读过治理文件与两份审读。
    let gov = passed(&rec, "govern", "governance");
    // 运行记录里门禁的 summary 记在 `detail`。
    assert!(
        gov["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("可以签发"),
        "{gov}"
    );
    assert_eq!(gov["metrics"]["rules"], 3, "治理文件的 style 三条：{gov}");
    let read = gov["evidence"].to_string();
    for f in [
        "governance/policy.yaml",
        "brief.md",
        "facts.jsonl",
        "policy.jsonl",
    ] {
        assert!(read.contains(f), "脚本读过的文件由运行时记下：{f} {gov}");
    }

    // 子 Agent 站：每节的交付都核实过是一个答完了的子 Agent 给的。
    let tasks = rec["stations"]["sections"]["tasks"]
        .as_array()
        .expect("分节的工作项");
    assert!(tasks.len() >= 2, "{tasks:?}");
    for t in tasks.iter().filter(|t| t["status"] == "submitted") {
        let d = &t["delivery"];
        assert_eq!(d["executor"], "agent", "{t}");
        assert_eq!(d["agent"]["verified"], true, "{t}");
        assert_eq!(d["agent"]["status"], "done", "{t}");
    }

    // 多模型。子 Agent 跑在 Claude 上，汇总的独立会话也是；主会话在 DeepSeek。
    let main = at_sign_off.main_session.clone();
    let main_tasks = match c
        .ok(Command::SessionTasks {
            session: main.clone(),
        })
        .await
    {
        Reply::Tasks(v) => v,
        r => panic!("session_tasks 回的是 {r:?}"),
    };
    let agents: Vec<_> = main_tasks
        .iter()
        .filter(|t| t.kind == TaskKind::Agent)
        .collect();
    assert!(!agents.is_empty(), "主会话起了子 Agent：{main_tasks:#?}");
    for a in &agents {
        assert_eq!(a.model.as_deref(), Some(cl.model.as_str()), "{a:?}");
    }
    let all = c.sessions().await;
    let assemble = station(&at_sign_off, "assemble")
        .session
        .clone()
        .expect("汇总站有自己的会话");
    assert_eq!(
        model_of(&all, &assemble.0).as_deref(),
        Some(cl.model.as_str()),
        "汇总站在 Claude 上"
    );
    assert_eq!(
        model_of(&all, &main.0).as_deref(),
        Some(ds.model.as_str()),
        "主会话在 DeepSeek 上"
    );
    // 审读站：两个工作项各在自己的会话、各在一家，时间上重叠（同时派出，不是一个做完再做另一个）。
    let reviews = &rec["stations"]["reviews"]["tasks"];
    let task = |t: &str| {
        reviews
            .as_array()
            .and_then(|v| v.iter().find(|x| x["id"] == t))
            .unwrap_or_else(|| panic!("审读站没有工作项 {t}：{reviews}"))
    };
    let (facts, policy) = (task("facts"), task("policy"));
    for (t, want) in [(facts, &ds.model), (policy, &cl.model)] {
        assert_eq!(t["status"], "submitted", "{t}");
        let sid = t["session"]
            .as_str()
            .unwrap_or_else(|| panic!("工作项没记会话：{t}"));
        assert_eq!(model_of(&all, sid).as_deref(), Some(want.as_str()), "{t}");
        assert_eq!(t["delivery"]["executor"], "session", "{t}");
    }
    let at = |t: &Value, k: &str| t[k].as_u64().unwrap_or_else(|| panic!("{k}：{t}"));
    assert!(
        at(facts, "started_at") < at(policy, "ended_at")
            && at(policy, "started_at") < at(facts, "ended_at"),
        "两份审读同时在做：{facts}\n{policy}"
    );
    both_spent(&lv);

    // 授权一：人。模型做不了——让一个用户会话去确认，它没有这个工具；运行还在等。
    let s = c.new_session().await;
    c.turn(
        &s,
        &format!(
            "请用流程工具确认运行 {} 的 sign-off 站，让它通过。做不到就直说为什么。",
            id.0
        ),
    )
    .await;
    let still = record(&lv, &id);
    assert!(
        still["stations"]["sign-off"]["confirms"]
            .as_array()
            .is_none_or(|v| v.is_empty()),
        "模型没能确认：{}",
        still["stations"]["sign-off"]
    );
    // 名单上没有的人做不了。
    let roles = Path::new(&lv.project_path()).join(".conix/roles.yaml");
    std::fs::write(&roles, "editor: [nobody-on-this-machine]\n").unwrap();
    let refused = c
        .call(Command::RunOp {
            run: id.clone(),
            op: confirm("sign-off", None),
        })
        .await;
    let e = format!("{refused:?}");
    assert!(
        refused.is_err() && e.contains("editor") && e.contains("nobody-on-this-machine"),
        "{e}"
    );
    // 名单放开，实例的主人确认；确认记成 person，运行走到发布。
    std::fs::write(&roles, "editor: []\n").unwrap();
    c.ok(Command::RunOp {
        run: id.clone(),
        op: confirm("sign-off", Some("看过了，签发")),
    })
    .await;
    let rec = record(&lv, &id);
    let last = rec["stations"]["sign-off"]["confirms"]
        .as_array()
        .and_then(|v| v.last())
        .cloned()
        .expect("确认记录");
    assert_eq!(last["by"]["kind"], "person", "{last}");
    assert_eq!(last["pass"], true, "{last}");
    assert_eq!(last["reason"], "看过了，签发", "{last}");
    passed(&rec, "sign-off", "untouched");

    // 授权二：MCP。发布站在外部审批系统建单，那边批了才过。
    let rid = approve_outside(&lv, &id, "publish").await;
    wait_run(&mut c, &id, "批了之后结束", |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
    approved_outside(&record(&lv, &id), "publish", &rid);

    // 归档（docs/design/layout.md §3）：运行的目录在制品目录的 runs/ 下、不进 git；结束后留下的制品连同
    // record.json 进了包的数据目录。
    let project = Path::new(&lv.project_path()).to_path_buf();
    assert_eq!(
        std::fs::read_to_string(project.join("conix/.gitignore")).unwrap_or_default(),
        "runs/\n"
    );
    assert!(
        out.starts_with(&*project.join("conix/runs").to_string_lossy()),
        "{out}"
    );
    let mut archived = None;
    for _ in 0..100 {
        archived = std::fs::read_dir(project.join("conix/brief/archive"))
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
    for f in [
        "brief.md",
        "outline.yaml",
        "reviews/facts.jsonl",
        "reviews/policy.jsonl",
    ] {
        assert!(archived.join(f).is_file(), "{}/{f}", archived.display());
    }
    assert!(!archived.join("_run").exists(), "过程目录不进归档");
    let kept: Value =
        serde_json::from_str(&std::fs::read_to_string(archived.join("record.json")).unwrap())
            .unwrap();
    assert_eq!(kept["status"], "finished", "{kept}");
    let sign_off = kept["stations"]
        .as_array()
        .and_then(|v| v.iter().find(|s| s["id"] == "sign-off"))
        .expect("签发站");
    assert_eq!(
        sign_off["confirms"][0]["by"]["kind"], "person",
        "{sign_off}"
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_demo_digest_hands_each_childs_declared_result_to_its_parent() {
    let lv = Live::new();
    install_demo(&lv, None);
    let _log = LogOnPanic(&lv);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = start(&c, "digest", "笔记摘要", Value::Null).await;
    let done = wait_run(&mut c, &id, "摘要结束", |v| {
        // 等子运行是在干活；别的等待是卡住了。
        if let RunStatus::Waiting { reason } = &v.summary.status
            && reason != "children"
        {
            panic!("运行停下了：{reason}\n{:#?}", v.stations)
        }
        v.summary.status == RunStatus::Finished
    })
    .await;
    assert_eq!(station(&done, "check").status, StationStatus::Passed);
    let rec = record(&lv, &id);
    let tasks = rec["stations"]["each"]["tasks"]
        .as_array()
        .expect("子运行的工作项");
    assert!(tasks.len() >= 3, "每份笔记一个：{tasks:?}");
    let delivered: Vec<&Value> = tasks
        .iter()
        .filter(|t| t["status"] == "submitted")
        .collect();
    assert!(!delivered.is_empty(), "{tasks:?}");
    for t in delivered {
        let out = &t["output"];
        // 子流程声明的 result：{path, line}，不是 {run, stations}。
        assert!(
            out["path"]
                .as_str()
                .is_some_and(|p| p.starts_with("notes/")),
            "{out}"
        );
        assert!(
            out["line"]
                .as_str()
                .is_some_and(|l| l.chars().count() >= 10),
            "{out}"
        );
        assert!(out.get("stations").is_none(), "{out}");
        assert_eq!(t["delivery"]["executor"], "subflow", "{t}");
    }
    passed(&rec, "check", "digest-complete");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_demo_publish_asks_the_outside_approval_system() {
    let lv = Live::new();
    install_demo(&lv, None);
    let _log = LogOnPanic(&lv);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = start(
        &c,
        "publish",
        "超时笔记",
        serde_json::json!({"brief": "notes/timeouts.md"}),
    )
    .await;
    // 两站都不开会话：check 是声明的门禁，approve 是外部审批。
    wait_run(&mut c, &id, "走到外部审批", |v| {
        station(v, "check").status == StationStatus::Passed
            && matches!(
                station(v, "approve").status,
                StationStatus::Working | StationStatus::Unmet
            )
    })
    .await;
    let rec = record(&lv, &id);
    passed(&rec, "check", "publish-ready");
    let rid = approve_outside(&lv, &id, "approve").await;
    wait_run(&mut c, &id, "批了之后结束", |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
    approved_outside(&record(&lv, &id), "approve", &rid);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_demo_jobs_come_disabled_and_the_simple_one_runs_on_demand() {
    let lv = Live::new();
    install_demo(&lv, None);
    let _log = LogOnPanic(&lv);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let jobs = match c.ok(Command::ListJobs).await {
        Reply::Jobs(v) => v,
        r => panic!("list_jobs 回的是 {r:?}"),
    };
    let names: Vec<&str> = jobs.iter().map(|j| j.id.0.as_str()).collect();
    assert!(
        names.contains(&"notes-check") && names.contains(&"weekly-brief"),
        "{names:?}"
    );
    assert!(
        jobs.iter().all(|j| !j.enabled),
        "克隆下来是停用的：{jobs:#?}"
    );
    c.ok(Command::RunJobNow {
        job: JobId("notes-check".into()),
        subject: None,
        prompt_append: None,
    })
    .await;
    c.wait_event("巡检跑完", Duration::from_secs(300), |e| {
        matches!(e, Event::JobUpserted(x) if x.id.0 == "notes-check" && x.last == Some(JobOutcome::Done)).then_some(())
    })
    .await;
    let history = lv.job_history_file("notes-check");
    assert_eq!(
        history.last().map(|e| e.outcome.clone()),
        Some(JobOutcome::Done),
        "{history:?}"
    );
}
