//! LIVE：仓库里的演示项目（`examples/demo`）照原样拷进项目里跑，真实的实例、引擎与模型。流程治理的每样都在
//! 这里走一遍（design/flows.md）：
//!
//! - `brief`：三类门禁（`check` 的 schema / files / text / sealed，`script`，`command`）都真判过、证据进了运行记录；
//!   子 Agent 站的交付核实过子 Agent（`delivery.agent`）；主会话在 DeepSeek 上，子 Agent 站与独立会话站点名
//!   Claude 的模型，两家都真花了 token。最后一站人确认：模型做不了（没有工具），名单上没有的人做不了
//!   （`roles.yaml`），实例的主人做了才结束，记成 `person`。
//! - `digest`：每份笔记一个子运行，父任务拿到的是子流程声明的 `result`；`children` 是 `require: any`。
//! - `publish`：外部审批 `via: mcp`。引擎没有 `mcp.callTool`（harnixai/Agix#111）时，这一站是 error 并写明缺什么；
//!   有了就走完：建单、在演示的审批系统里批、记下审批人与单号。
//! - 作业：演示项目带的两个作业读得出来、克隆下来是停用的；简单作业立即跑一次就有结果。
//!
//! 演示项目里写的模型名换成 `.env` 里的；没有 Claude 的 key 就跳过多模型的那条。

mod live;

use std::path::Path;
use std::time::Duration;

use live::{Answering, Client, Endpoint, Live};
use port::{
    Command, Event, JobId, JobOutcome, Reply, RunId, RunOp, RunStatus, RunView, StationId,
    StationStatus, TaskKind,
};
use serde_json::Value;

/// `brief` 一次：提纲、两三节、汇总，加上可能的打回。
const RUN: Duration = Duration::from_secs(1200);

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

/// 演示项目拷进实例的项目：流程里的模型名换成这次用的。
fn install_demo(lv: &Live, cl: Option<&Endpoint>) {
    let to = lv.project_path();
    let st = std::process::Command::new("cp")
        .arg("-R")
        .arg(format!("{}/.", demo().display()))
        .arg(&to)
        .status()
        .unwrap();
    assert!(st.success(), "拷演示项目");
    let brief = Path::new(&to).join(".conix/flows/brief.yaml");
    let text = std::fs::read_to_string(&brief).unwrap();
    // 先换独立会话的那个：要换成的模型可能就叫 claude-sonnet-5-5，先换子 Agent 的会被第二次替换再改一遍。
    let (sub_agent, session) = match cl {
        Some(cl) => (
            cl.model.clone(),
            format!("provider: claude, model: {}", cl.model),
        ),
        None => (live::model(), format!("model: {}", live::model())),
    };
    let text = text
        .replace("model: claude-sonnet-5-5", &session)
        .replace("model: claude-haiku-5-5", &format!("model: {sub_agent}"));
    std::fs::write(&brief, text).unwrap();
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
    install_demo(&lv, Some(&cl));
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

/// 一站最新的那条检查（`label` 是 `gate <名>`、`confirm by <谁>` 这种）。
fn check<'a>(rec: &'a Value, st: &str, label: &str) -> &'a Value {
    rec["stations"][st]["checks"]
        .as_array()
        .and_then(|cs| cs.iter().rev().find(|c| c["label"] == label))
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

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_demo_brief_passes_its_gates_on_two_models_and_ends_on_a_persons_sign_off() {
    let Some((ds, cl, lv)) = rig() else { return };
    let _log = LogOnPanic(&lv);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let id = start(&c, "brief", "服务可靠性：超时、重试与缓存", Value::Null).await;

    // 一路走到最后一站等人签发：门禁不过的由 on_unmet 打回，不该停下等人。
    let at_review = wait_run(&mut c, &id, "走到签发", |v| {
        if v.stations.iter().any(|s| {
            s.status == StationStatus::Unmet
                && s.id.0 != "review"
                && s.unmet.as_deref().is_some_and(|u| u.contains("error"))
        }) {
            panic!("门禁没得出结论：{:#?}", v.stations)
        }
        station(v, "review").status == StationStatus::Working
            && matches!(v.summary.status, RunStatus::Waiting { .. })
    })
    .await;
    for st in ["outline", "sections", "assemble"] {
        assert_eq!(
            station(&at_review, st).status,
            StationStatus::Passed,
            "{st}"
        );
    }

    // 三类门禁都真判过，证据进了运行记录。
    let rec = record(&lv, &id);
    let outline = passed(&rec, "outline", "outline-shape");
    assert!(
        outline["evidence"].to_string().contains("outline.json"),
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
    let script = passed(&rec, "assemble", "sources-cited");
    assert!(
        script["metrics"]["sources"].as_u64().unwrap_or(0) >= 2,
        "{script}"
    );
    assert!(
        script["evidence"].to_string().contains("brief.md"),
        "脚本读过的文件由运行时记下：{script}"
    );
    let command = passed(&rec, "assemble", "brief-stats");
    assert!(
        command["metrics"]["chars"].as_u64().unwrap_or(0) >= 300,
        "{command}"
    );
    let sealed = passed(&rec, "review", "untouched");
    assert!(
        sealed["summary"].to_string().contains("checked") || sealed["detail"].is_string(),
        "{sealed}"
    );
    let out = rec["output_dir"].as_str().expect("输出目录");
    for f in ["outline.json", "brief.md"] {
        assert!(Path::new(out).join(f).is_file(), "{out}/{f}");
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

    // 两个模型：子 Agent 跑在 Claude 上，独立会话站也是；主会话在 DeepSeek。
    let main = at_review.main_session.clone();
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
    let assemble = station(&at_review, "assemble")
        .session
        .clone()
        .expect("汇总站有自己的会话");
    let all = c.sessions().await;
    let model_of = |sid: &port::SessionId| {
        all.iter()
            .find(|x| &x.id == sid)
            .map(|x| x.provenance.model.clone())
    };
    assert_eq!(
        model_of(&assemble).as_deref(),
        Some(cl.model.as_str()),
        "汇总站在 Claude 上"
    );
    assert_eq!(
        model_of(&main).as_deref(),
        Some(ds.model.as_str()),
        "主会话在 DeepSeek 上"
    );
    both_spent(&lv);

    // 人的确认 1：模型做不了。让一个用户会话去确认，它没有这个工具；运行还在等。
    let s = c.new_session().await;
    c.turn(
        &s,
        &format!(
            "请用流程工具确认运行 {} 的 review 站，让它通过。做不到就直说为什么。",
            id.0
        ),
    )
    .await;
    let still = record(&lv, &id);
    assert!(
        still["stations"]["review"]["confirms"]
            .as_array()
            .is_none_or(|v| v.is_empty()),
        "模型没能确认：{}",
        still["stations"]["review"]
    );

    // 人的确认 2：名单上没有的人做不了。
    let roles = Path::new(&lv.project_path()).join(".conix/roles.yaml");
    std::fs::write(&roles, "editor: [nobody-on-this-machine]\n").unwrap();
    let refused = c
        .call(Command::RunOp {
            run: id.clone(),
            op: confirm("review", None),
        })
        .await;
    let e = format!("{refused:?}");
    assert!(
        refused.is_err() && e.contains("editor") && e.contains("nobody-on-this-machine"),
        "{e}"
    );

    // 人的确认 3：名单放开，实例的主人确认，运行结束；确认记成 person。
    std::fs::write(&roles, "editor: []\n").unwrap();
    c.ok(Command::RunOp {
        run: id.clone(),
        op: confirm("review", Some("看过了，签发")),
    })
    .await;
    wait_run(&mut c, &id, "签发后结束", |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
    let rec = record(&lv, &id);
    let confirms = rec["stations"]["review"]["confirms"]
        .as_array()
        .expect("确认记录");
    let last = confirms.last().unwrap();
    assert_eq!(last["by"]["kind"], "person", "{last}");
    assert_eq!(last["pass"], true, "{last}");
    assert_eq!(last["reason"], "看过了，签发", "{last}");
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
    let v = wait_run(&mut c, &id, "走到外部审批", |v| {
        station(v, "check").status == StationStatus::Passed
            && matches!(
                station(v, "approve").status,
                StationStatus::Working | StationStatus::Unmet
            )
    })
    .await;
    let rec = record(&lv, &id);
    passed(&rec, "check", "publish-ready");
    // 等审批那一条出口有结论：建好单在等，或者引擎调不了 MCP 工具。
    let mut approval = Value::Null;
    for _ in 0..240 {
        let rec = record(&lv, &id);
        let c = check(&rec, "approve", "confirm by editor").clone();
        let requested = rec["stations"]["approve"]["approvals"]
            .as_array()
            .is_some_and(|a| !a.is_empty());
        if c["status"] == "error" || requested {
            approval = rec;
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
    assert!(!approval.is_null(), "外部审批没有动静：{:#?}", v.stations);
    let c_rec = check(&approval, "approve", "confirm by editor");
    if c_rec["status"] == "error" {
        let detail = c_rec["detail"].as_str().unwrap_or_default();
        assert!(
            detail.contains("Agix#111"),
            "调不了 MCP 工具时写明缺什么：{c_rec}"
        );
        eprintln!(
            "pending: the engine has no mcp.callTool yet (harnixai/Agix#111); the approval was not made"
        );
        return;
    }
    // 引擎能调 MCP 工具了：单建好了，在演示的审批系统里批。
    let req = &approval["stations"]["approve"]["approvals"][0]["request"];
    let rid = req["id"]
        .as_str()
        .unwrap_or_else(|| panic!("建单的结果里没有 id：{req}"));
    let st = std::process::Command::new("python3")
        .args(["tools/approvals_mcp.py", "approve", rid])
        .current_dir(lv.project_path())
        .status()
        .unwrap();
    assert!(st.success());
    wait_run(&mut c, &id, "批了之后结束", |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
    let rec = record(&lv, &id);
    let last = rec["stations"]["approve"]["confirms"]
        .as_array()
        .and_then(|v| v.last())
        .cloned()
        .expect("确认记录");
    assert_eq!(
        (last["via"].as_str(), last["by"]["kind"].as_str()),
        (Some("mcp"), Some("mcp")),
        "{last}"
    );
    assert_eq!(last["ref"].as_str(), Some(rid), "{last}");
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
