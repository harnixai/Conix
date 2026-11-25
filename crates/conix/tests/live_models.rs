//! LIVE：一次运行里用两个模型（architecture §8.6、§8.12）。设置里定义两个 provider（DeepSeek、Anthropic），
//! `model.order` 是 DeepSeek 在前。`.env` 里没有 Anthropic 的 key（CLAUDE_API_KEY、CLAUDE_MODEL）就跳过。
//!
//! - A / D：一站生成、一站检视，各在自己的会话里、各走自己的 provider（站的执行者写 `model` 与 `provider`）；
//!   检视站经 `foreach` 拿到生成站交的稿子（流程库不把上游的产出写进任务消息，item 是递过去的那条路）。
//! - B：检视打回、返工，两轮都还在各自的模型上。
//! - C：主会话在 DeepSeek 上，一站子 Agent 点名 Claude 的模型，子 Agent 真跑在 Claude 上。
//!
//! "这一站真是那家答的"看引擎的用量账本：有 quota 的 provider，引擎把它真花掉的 token 记进
//! `<data_root>/quota/<id>.json`。会话上写的模型是请求的；`ProviderSwitched` 只在与会话上一次调用不同家时才推，
//! 第一次调用就回退了是看不出来的。

mod live;

use std::time::{Duration, SystemTime};

use live::{Answering, Client, Endpoint, Live};
use port::{Command, Event, Reply, RunId, RunOp, RunStatus, RunView, SessionId, StationId};
use port::{SessionSummary, StationStatus, TaskKind};
use serde_json::Value;

/// 两站各一轮模型，外加主会话；返工的再多两轮。
const RUN: Duration = Duration::from_secs(300);

const WRITE_SKILL: &str = "---\nname: write-line\ndescription: Write one sentence on the run's subject.\n---\n\n\
Write exactly one plain English sentence about the run's subject. Hand in `drafts`: a list holding \
that one sentence. Nothing else.\n";

const REVIEW_SKILL: &str = "---\nname: review-line\ndescription: Review one sentence.\n---\n\n\
Your item is a sentence someone else wrote about the run's subject. Check it: it is one English \
sentence and it is about the subject. Hand in `verdict` (`pass` or `fail`) and `notes` (one short \
line saying why). Do not rewrite the sentence.\n";

/// B 的生成站：第一轮不知道检视要什么，返工后（第 2 轮起）知道。流程的返工不带理由（任务里只写第几轮），
/// 所以"打回后改了什么"写在技能里，按轮次分开。
const WRITE_WORD_SKILL: &str = "---\nname: write-word\ndescription: Write one sentence on the run's subject.\n---\n\n\
Write exactly one plain English sentence about the run's subject. Your task message says which round \
this is. In round 1, do not use the word `quickly`. From round 2 on, the sentence must contain the \
word `quickly`. Hand in `drafts`: a list holding that one sentence. Nothing else.\n";

const REVIEW_WORD_SKILL: &str = "---\nname: review-word\ndescription: Check one sentence for a word.\n---\n\n\
Your item is a sentence someone else wrote. The only rule: it must contain the word `quickly`. \
Hand in `verdict`: `pass` if it does, `fail` if it does not, and `notes` (one short line). Do not \
rewrite the sentence.\n";

/// C 的子 Agent 站。
const ASK_SKILL: &str = "---\nname: one-line\ndescription: Say one sentence on the subject.\n---\n\n\
Write exactly one plain English sentence about the run's subject. Hand in `line`: that sentence.\n";

const DRAFTS: &str = "{type: object, required: [drafts], properties: {drafts: {type: array, minItems: 1, maxItems: 1, items: {type: string}}}}";
const VERDICT: &str = "{type: object, required: [verdict, notes], properties: {verdict: {type: string, enum: [pass, fail]}, notes: {type: string}}}";

fn session_exec(provider: &str, e: &Endpoint) -> String {
    format!(
        "{{kind: session, scene: code, provider: {provider}, model: {}}}",
        e.model
    )
}

/// A / D：`writer` 写、`reviewer` 检视。
fn pair_flow(writer: (&str, &Endpoint), reviewer: (&str, &Endpoint)) -> String {
    format!(
        "name: live-pair
title: Live pair
stations:
  - id: write
    executor: {w}
    skill: write-line
    output: {DRAFTS}
    exit: [{{kind: output}}]
  - id: review
    foreach: ${{stations.write.output.drafts}}
    executor: {r}
    skill: review-line
    output: {VERDICT}
    exit: [{{kind: output}}]
",
        w = session_exec(writer.0, writer.1),
        r = session_exec(reviewer.0, reviewer.1),
    )
}

/// B：DeepSeek 写、Claude 检视；检视站交了结论还要人确认，人看了结论可以退回生成站。
/// 不用门禁自动返工：Conix 不给流程库接工具、不开命令门禁，"看结论再退回"就是人的确认那条路。
fn rework_flow(ds: &Endpoint, cl: &Endpoint) -> String {
    format!(
        "name: live-rework
title: Live rework
stations:
  - id: write
    executor: {w}
    skill: write-word
    output: {DRAFTS}
    exit: [{{kind: output}}]
  - id: review
    foreach: ${{stations.write.output.drafts}}
    executor: {r}
    skill: review-word
    output: {VERDICT}
    exit: [{{kind: output}}, {{kind: confirm}}]
    rework_to: [write]
",
        w = session_exec("deepseek", ds),
        r = session_exec("claude", cl),
    )
}

/// C：一站子 Agent，只写模型（流程的 agent 执行者没有 provider）；Agix 把有确定归属的模型 id 路由到它的家。
fn agent_flow(cl: &Endpoint) -> String {
    format!(
        "name: live-agent
title: Live agent
stations:
  - id: ask
    executor: {{kind: agent, model: {m}}}
    skill: one-line
    output: {{type: object, required: [line], properties: {{line: {{type: string}}}}}}
    exit: [{{kind: output}}]
",
        m = cl.model
    )
}

/// 两个 provider：每个只认自己的模型（`models`）。都给一个用不完的 `quota`，好让引擎记账本。
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

/// 两家的端点与一个装了两个 provider 的实例；没有 Anthropic 的 key 就是 `None`。
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
    Some((ds, cl, lv))
}

fn install(lv: &Live, name: &str, yaml: &str, skills: &[(&str, &str)]) {
    let root = std::path::Path::new(&lv.project_path()).join(".conix/flows");
    for (skill, body) in skills {
        std::fs::create_dir_all(root.join("skills").join(skill)).unwrap();
        std::fs::write(root.join("skills").join(skill).join("SKILL.md"), body).unwrap();
    }
    std::fs::write(root.join(format!("{name}.yaml")), yaml).unwrap();
}

async fn start(lv: &Live, flow: &str) -> (Client, RunId) {
    let c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let run = match c
        .ok(Command::StartRun {
            flow: flow.into(),
            subject: "why tests should be fast".into(),
            inputs: Value::Null,
        })
        .await
    {
        Reply::RunStarted(s) => s,
        r => panic!("start_run 回的是 {r:?}"),
    };
    (c, run.id)
}

/// 等这次运行的一个画面满足 `want`；取消了就立刻失败，不等满超时。
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

/// 一路走完，中途不该停下等人。
async fn finished(c: &mut Client, id: &RunId) -> RunView {
    wait_run(c, id, "运行结束", |v| {
        if let RunStatus::Waiting { reason } = &v.summary.status {
            panic!("运行停下了：{reason}\n{:#?}", v.stations)
        }
        v.summary.status == RunStatus::Finished
    })
    .await
}

fn station<'a>(v: &'a RunView, id: &str) -> &'a port::StationView {
    v.stations
        .iter()
        .find(|s| s.id.0 == id)
        .unwrap_or_else(|| panic!("没有站 {id}"))
}

fn session_of(v: &RunView, id: &str) -> SessionId {
    let st = station(v, id);
    st.session
        .clone()
        .or_else(|| st.tasks.iter().find_map(|t| t.session.clone()))
        .unwrap_or_else(|| panic!("站 {id} 没有自己的会话：{st:?}"))
}

/// 会话的用量在一轮结束（`turn_ended`）时才记上；站交了产出就算过，运行可能在那一站的一轮收尾之前就结束了。
async fn settled(c: &Client, ids: &[&SessionId]) -> Vec<SessionSummary> {
    let mut last = vec![];
    for _ in 0..120 {
        let all = c.sessions().await;
        last = ids
            .iter()
            .map(|id| {
                all.iter()
                    .find(|x| &x.id == *id)
                    .cloned()
                    .unwrap_or_else(|| panic!("会话 {id:?} 不在列表里"))
            })
            .collect();
        if last.iter().all(|s: &SessionSummary| s.usage.output > 0) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    last
}

fn no_switch(c: &Client, ids: &[&SessionId]) {
    let switched: Vec<_> = c
        .events()
        .into_iter()
        .filter(|e| matches!(e, Event::ProviderSwitched { session, .. } if ids.contains(&session)))
        .collect();
    assert!(switched.is_empty(), "没有回退到别家：{switched:?}");
}

/// 两家都真花了 token。
fn both_spent(lv: &Live) {
    for id in ["deepseek", "claude"] {
        let ledger = lv.home.path().join("quota").join(format!("{id}.json"));
        let text = std::fs::read_to_string(&ledger).unwrap_or_else(|e| {
            panic!("{id} 没有用量记录（没被调用过？）{}: {e}", ledger.display())
        });
        let v: Value = serde_json::from_str(&text).unwrap();
        assert!(max_number(&v) > 0, "{id} 花了 token：{text}");
    }
}

/// 运行时目录里的报告（architecture §8.12），运行结束时写。
async fn report(lv: &Live, id: &RunId) -> String {
    let path = lv.process_dir(&id.0).join("conix-report.json");
    for _ in 0..100 {
        if let Ok(t) = std::fs::read_to_string(&path) {
            return t;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("没有报告：{}", path.display())
}

/// 一站最新交的产出与它的时间（流程库的过程目录 `_run/stations/<站>/`：一个工作项是 `output.json`，几个是
/// `<工作项>.json`）。返工后的一轮是新的工作项、新的文件，上一轮的留着，所以只认最新的那份。
fn latest(lv: &Live, id: &RunId, st: &str) -> Option<(Value, SystemTime)> {
    let dir = lv.process_dir(&id.0).join("stations").join(st);
    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|f| {
            let f = f.ok()?;
            let at = f.metadata().ok()?.modified().ok()?;
            // 写一半的文件读不出 JSON：读得出来再算。
            let v = serde_json::from_str(&std::fs::read_to_string(f.path()).ok()?).ok()?;
            Some((v, at))
        })
        .max_by_key(|(_, at)| *at)
}

fn output(lv: &Live, id: &RunId, st: &str) -> Value {
    latest(lv, id, st).map(|(v, _)| v).unwrap_or(Value::Null)
}

/// 等一站交的产出落进过程目录、且比 `after` 新。
async fn handed_in(
    lv: &Live,
    id: &RunId,
    st: &str,
    after: Option<SystemTime>,
) -> (Value, SystemTime) {
    let deadline = std::time::Instant::now() + RUN;
    while std::time::Instant::now() < deadline {
        if let Some((v, at)) = latest(lv, id, st)
            && after.is_none_or(|a| at > a)
        {
            return (v, at);
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    panic!("{}s 内站 {st} 没交新的产出", RUN.as_secs())
}

async fn view(c: &Client, id: &RunId) -> RunView {
    match c.ok(Command::GetRun { run: id.clone() }).await {
        Reply::Run(v) => *v,
        r => panic!("get_run 回的是 {r:?}"),
    }
}

/// 跑一次：`writer` 写、`reviewer` 检视；各站的会话用的是各自的模型，两边都真答了，没有回退到别家，
/// 运行时目录里的报告写对了模型。
async fn pair(writer: &str, reviewer: &str) {
    let Some((ds, cl, lv)) = rig() else { return };
    let pick = |id: &str| {
        if id == "deepseek" {
            ds.clone()
        } else {
            cl.clone()
        }
    };
    let (w, r) = (pick(writer), pick(reviewer));
    install(
        &lv,
        "live-pair",
        &pair_flow((writer, &w), (reviewer, &r)),
        &[("write-line", WRITE_SKILL), ("review-line", REVIEW_SKILL)],
    );
    let _log = LogOnPanic(&lv);
    let (mut c, id) = start(&lv, "live-pair").await;
    let done = finished(&mut c, &id).await;
    for st in &done.stations {
        assert_eq!(st.status, StationStatus::Passed, "{st:?}");
    }
    let (ws, rs) = (session_of(&done, "write"), session_of(&done, "review"));
    assert_ne!(ws, rs, "两站各有自己的会话");
    let s = settled(&c, &[&ws, &rs]).await;
    assert_eq!(s[0].provenance.model, w.model, "生成站用 {writer}");
    assert_eq!(s[1].provenance.model, r.model, "检视站用 {reviewer}");
    assert!(
        s.iter().all(|x| x.usage.output > 0),
        "两边都真答了：{:?} / {:?}",
        s[0].usage,
        s[1].usage
    );
    no_switch(&c, &[&ws, &rs]);
    // 检视或生成落在 Claude 上的那一站，只能是 Claude 答的。
    both_spent(&lv);
    let text = report(&lv, &id).await;
    assert!(
        text.contains(&w.model) && text.contains(&r.model),
        "报告写对了两边的模型：{text}"
    );
}

/// 用例失败时把实例日志的末尾打出来：路由、凭据、额度的问题都在那里，临时目录随后就删了。
struct LogOnPanic<'a>(&'a Live);

impl Drop for LogOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            let log = self.0.host_log();
            let tail: Vec<&str> = log.lines().rev().take(60).collect();
            eprintln!("--- host.log (last 60 lines)");
            for l in tail.iter().rev() {
                eprintln!("{l}");
            }
        }
    }
}

/// 用量记录的形状归引擎：只认"里面有个正数"。
fn max_number(v: &Value) -> u64 {
    match v {
        Value::Number(n) => n.as_u64().unwrap_or(0),
        Value::Array(a) => a.iter().map(max_number).max().unwrap_or(0),
        Value::Object(o) => o
            .iter()
            .filter(|(k, _)| !k.contains("limit") && !k.contains("start"))
            .map(|(_, v)| max_number(v))
            .max()
            .unwrap_or(0),
        _ => 0,
    }
}

/// A：DeepSeek 生成，Claude 检视。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn deepseek_writes_and_claude_reviews_in_one_run() {
    pair("deepseek", "claude").await;
}

/// D：反过来，Claude 生成，DeepSeek 检视。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn claude_writes_and_deepseek_reviews_in_one_run() {
    pair("claude", "deepseek").await;
}

/// B：Claude 检视打回 DeepSeek 写的第一稿，人照结论退回生成站；第二轮补上了，检视通过、人确认，运行结束。
/// 两轮的生成都在 DeepSeek 上、检视都在 Claude 上，没有回退到别家。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn the_reviewer_sends_the_draft_back_and_both_rounds_stay_on_their_models() {
    let Some((ds, cl, lv)) = rig() else { return };
    install(
        &lv,
        "live-rework",
        &rework_flow(&ds, &cl),
        &[
            ("write-word", WRITE_WORD_SKILL),
            ("review-word", REVIEW_WORD_SKILL),
        ],
    );
    let _log = LogOnPanic(&lv);
    let (mut c, id) = start(&lv, "live-rework").await;
    // 检视交了结论：产出落进过程目录。运行画面看不出来——单项的 `foreach` 站不列工作项，"等确认"在结论交上来之前
    // 就挂着了。
    let (verdict, at) = handed_in(&lv, &id, "review", None).await;
    let first = view(&c, &id).await;
    assert_eq!(
        verdict["verdict"],
        "fail",
        "第一稿没有那个词，检视打回：{verdict}\n稿子：{}",
        output(&lv, &id, "write")
    );
    let (w1, r1) = (session_of(&first, "write"), session_of(&first, "review"));
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::Rework {
            to: StationId("write".into()),
            reason: None,
        },
    })
    .await;
    // 先看到返工落下（生成站重新在做），免得把第一轮停下时的画面当成第二轮的。
    wait_run(&mut c, &id, "退回生成站", |v| {
        station(v, "write").status != StationStatus::Passed
    })
    .await;
    let (verdict, _) = handed_in(&lv, &id, "review", Some(at)).await;
    let second = view(&c, &id).await;
    let draft = output(&lv, &id, "write");
    assert_eq!(
        verdict["verdict"], "pass",
        "第二稿补上了，检视通过：{verdict}\n稿子：{draft}"
    );
    assert!(
        draft["drafts"][0]
            .as_str()
            .is_some_and(|s| s.to_lowercase().contains("quickly")),
        "{draft}"
    );
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::Confirm {
            station: StationId("review".into()),
            reject: false,
            reason: None,
        },
    })
    .await;
    // 确认之前停在人身上的那些画面还在后面排着，不能用 `finished`（它见到等人就失败）。
    wait_run(&mut c, &id, "确认后运行结束", |v| {
        v.summary.status == RunStatus::Finished
    })
    .await;
    let (w2, r2) = (session_of(&second, "write"), session_of(&second, "review"));
    let mut ids = vec![&w1, &r1, &w2, &r2];
    ids.dedup();
    let s = settled(&c, &ids).await;
    for x in &s {
        let want = if [&w1, &w2].contains(&&x.id) {
            &ds.model
        } else {
            &cl.model
        };
        assert_eq!(&x.provenance.model, want, "{:?} 两轮都在自己的模型上", x.id);
        assert!(x.usage.output > 0, "{:?} 真答了：{:?}", x.id, x.usage);
    }
    no_switch(&c, &ids);
    both_spent(&lv);
}

/// C：主会话在 DeepSeek 上，子 Agent 站点名 Claude 的模型。子 Agent 的任务报它实际用的模型与 provider
/// （Agix 的 `task_model`），侧链会话的模型跟着它；两家都花了 token，主会话没有被换到 Claude 上。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_sub_agent_station_runs_on_the_other_provider() {
    let Some((ds, cl, lv)) = rig() else { return };
    install(
        &lv,
        "live-agent",
        &agent_flow(&cl),
        &[("one-line", ASK_SKILL)],
    );
    let _log = LogOnPanic(&lv);
    let (mut c, id) = start(&lv, "live-agent").await;
    let done = finished(&mut c, &id).await;
    assert_eq!(station(&done, "ask").status, StationStatus::Passed);
    let main = done.main_session.clone();
    let tasks = match c
        .ok(Command::SessionTasks {
            session: main.clone(),
        })
        .await
    {
        Reply::Tasks(v) => v,
        r => panic!("session_tasks 回的是 {r:?}"),
    };
    let agent = tasks
        .iter()
        .find(|t| t.kind == TaskKind::Agent && t.owner == main)
        .unwrap_or_else(|| panic!("主会话没有起子 Agent：{tasks:#?}"));
    assert_eq!(agent.model.as_deref(), Some(cl.model.as_str()), "{agent:?}");
    assert_eq!(agent.provider.as_deref(), Some("claude"), "{agent:?}");
    let side = agent.agent_session.clone().expect("子 Agent 有侧链会话");
    // 运行结束时主会话随之关闭、不再列出：它在哪家看报告与账本。
    let all = c.sessions().await;
    if let Some(x) = all.iter().find(|x| x.id == side) {
        assert_eq!(x.provenance.model, cl.model, "侧链会话跟着任务改了模型");
    }
    no_switch(&c, &[&main]);
    both_spent(&lv);
    let text = report(&lv, &id).await;
    assert!(
        text.contains(&cl.model) && text.contains(&ds.model),
        "报告里有主会话与子 Agent 各自的模型：{text}"
    );
}
