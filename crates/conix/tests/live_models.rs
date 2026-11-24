//! LIVE：一次运行里用两个模型（architecture §8.6、§8.12）——一站生成、一站检视，各在自己的会话里、
//! 各走自己的 provider。设置里定义两个 provider（DeepSeek、Anthropic），站的执行者写 `model` 与
//! `provider`；检视站经 `foreach` 拿到生成站交的稿子（流程库不把上游的产出写进任务消息，item 是
//! 递过去的那条路）。`.env` 里没有 Anthropic 的 key（CLAUDE_API_KEY、CLAUDE_MODEL）就跳过。

mod live;

use std::time::Duration;

use live::{Answering, Endpoint, Live};
use port::{Event, Reply, RunStatus, StationStatus};

/// 两站各一轮模型，外加主会话。
const RUN: Duration = Duration::from_secs(300);

const WRITE_SKILL: &str = "---\nname: write-line\ndescription: Write one sentence on the run's subject.\n---\n\n\
Write exactly one plain English sentence about the run's subject. Hand in `drafts`: a list holding \
that one sentence. Nothing else.\n";

const REVIEW_SKILL: &str = "---\nname: review-line\ndescription: Review one sentence.\n---\n\n\
Your item is a sentence someone else wrote about the run's subject. Check it: it is one English \
sentence and it is about the subject. Hand in `verdict` (`pass` or `fail`) and `notes` (one short \
line saying why). Do not rewrite the sentence.\n";

fn flow(writer: (&str, &Endpoint), reviewer: (&str, &Endpoint)) -> String {
    format!(
        r#"name: live-pair
title: Live pair
stations:
  - id: write
    title: Write
    executor: {{kind: session, scene: code, provider: {wp}, model: {wm}}}
    skill: write-line
    output:
      type: object
      required: [drafts]
      properties:
        drafts: {{type: array, minItems: 1, maxItems: 1, items: {{type: string}}}}
    exit: [{{kind: output}}]
  - id: review
    title: Review
    foreach: ${{stations.write.output.drafts}}
    executor: {{kind: session, scene: code, provider: {rp}, model: {rm}}}
    skill: review-line
    output:
      type: object
      required: [verdict, notes]
      properties:
        verdict: {{type: string, enum: [pass, fail]}}
        notes: {{type: string}}
    exit: [{{kind: output}}]
"#,
        wp = writer.0,
        wm = writer.1.model,
        rp = reviewer.0,
        rm = reviewer.1.model,
    )
}

/// 两个 provider：每个只认自己的模型（`models`）。都给一个用不完的 `quota`：有 quota 的 provider，引擎把
/// 它真花掉的 token 记进 `<data_root>/quota/<id>.json`——"这一站真是那家答的"就看它，不看会话上写的模型
/// （那是请求的）。`ProviderSwitched` 只在与会话上一次调用不同家时才推，第一次调用就回退了是看不出来的。
fn providers(ds: &Endpoint, cl: &Endpoint) -> serde_json::Value {
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

fn install(lv: &Live, yaml: &str) {
    let root = std::path::Path::new(&lv.project_path()).join(".conix/flows");
    for (name, body) in [("write-line", WRITE_SKILL), ("review-line", REVIEW_SKILL)] {
        std::fs::create_dir_all(root.join("skills").join(name)).unwrap();
        std::fs::write(root.join("skills").join(name).join("SKILL.md"), body).unwrap();
    }
    std::fs::write(root.join("live-pair.yaml"), yaml).unwrap();
}

/// 跑一次：`writer` 写、`reviewer` 检视；各站的会话用的是各自的模型，两边都真答了，没有回退到别家，
/// 运行时目录里的报告写对了模型。
async fn pair(writer: &str, reviewer: &str) {
    let Some(cl) = live::claude() else {
        eprintln!("skipped: .env has no CLAUDE_API_KEY / CLAUDE_MODEL");
        return;
    };
    let ds = live::deepseek();
    let pick = |id: &str| -> (String, Endpoint) {
        match id {
            "deepseek" => (id.into(), ds.clone()),
            _ => (id.into(), cl.clone()),
        }
    };
    let (w, r) = (pick(writer), pick(reviewer));
    let lv = Live::with(live::Opts {
        settings: Some(providers(&ds, &cl)),
        ..Default::default()
    });
    install(&lv, &flow((&w.0, &w.1), (&r.0, &r.1)));
    let _log = LogOnPanic(&lv);
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let run = match c
        .ok(port::Command::StartRun {
            flow: "live-pair".into(),
            subject: "why tests should be fast".into(),
            inputs: serde_json::Value::Null,
        })
        .await
    {
        Reply::RunStarted(s) => s,
        r => panic!("start_run 回的是 {r:?}"),
    };
    let id = run.id.clone();
    let done = c
        .wait_event("运行结束", RUN, |e| match e {
            Event::RunDetail(v)
                if v.summary.id == id && v.summary.status == RunStatus::Finished =>
            {
                Some((**v).clone())
            }
            Event::RunDetail(v)
                if v.summary.id == id
                    && matches!(
                        v.summary.status,
                        RunStatus::Cancelled | RunStatus::Waiting { .. }
                    ) =>
            {
                panic!("运行没有走完：{:?}\n{:#?}", v.summary.status, v.stations)
            }
            _ => None,
        })
        .await;
    for st in &done.stations {
        assert_eq!(st.status, StationStatus::Passed, "{st:?}");
    }
    let session_of = |station: &str| {
        let st = done.stations.iter().find(|s| s.id.0 == station).unwrap();
        st.session
            .clone()
            .or_else(|| st.tasks.iter().find_map(|t| t.session.clone()))
            .unwrap_or_else(|| panic!("站 {station} 没有自己的会话：{st:?}"))
    };
    let (ws, rs) = (session_of("write"), session_of("review"));
    assert_ne!(ws, rs, "两站各有自己的会话");
    let all = c.sessions().await;
    let sum = |s: &port::SessionId| {
        all.iter()
            .find(|x| &x.id == s)
            .cloned()
            .unwrap_or_else(|| panic!("会话 {s:?} 不在列表里"))
    };
    let (wsum, rsum) = (sum(&ws), sum(&rs));
    assert_eq!(wsum.provenance.model, w.1.model, "生成站用 {writer}");
    assert_eq!(rsum.provenance.model, r.1.model, "检视站用 {reviewer}");
    assert!(
        wsum.usage.output > 0 && rsum.usage.output > 0,
        "两边都真答了：{:?} / {:?}",
        wsum.usage,
        rsum.usage
    );
    let switched: Vec<_> = c
        .events()
        .into_iter()
        .filter(|e| matches!(e, Event::ProviderSwitched { session, .. } if *session == ws || *session == rs))
        .collect();
    assert!(switched.is_empty(), "没有回退到别家：{switched:?}");
    // 两家都真花了 token：检视或生成落在 Claude 上的那一站，只能是 Claude 答的。
    for id in ["deepseek", "claude"] {
        let ledger = lv.home.path().join("quota").join(format!("{id}.json"));
        let text = std::fs::read_to_string(&ledger).unwrap_or_else(|e| {
            panic!("{id} 没有用量记录（没被调用过？）{}: {e}", ledger.display())
        });
        let v: serde_json::Value = serde_json::from_str(&text).unwrap();
        assert!(max_number(&v) > 0, "{id} 花了 token：{text}");
    }
    // 运行时目录里的报告（architecture §8.12）写着每一站的会话与模型。
    let report = lv.state().run_dir(&id.0).join("conix-report.json");
    let mut text = String::new();
    for _ in 0..100 {
        if let Ok(t) = std::fs::read_to_string(&report) {
            text = t;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        text.contains(&w.1.model) && text.contains(&r.1.model),
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
fn max_number(v: &serde_json::Value) -> u64 {
    match v {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
        serde_json::Value::Array(a) => a.iter().map(max_number).max().unwrap_or(0),
        serde_json::Value::Object(o) => o
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
