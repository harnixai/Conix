//! LIVE：会话里配置自己的另外几个工具与引擎换 provider 的提示——`config_show` 看生效配置、`codix_config_set`
//! 改 `codix.yaml`（校验、备份、jobs 当场生效）；排在前面的 provider 连不上时引擎换到下一个，实例把
//! `provider_switched` 推给界面。provider 的加、测、切、退在 `live_config.rs`。

mod live;

use live::{Answering, Live, Opts};
use port::{Command, Event, HealthStatus, Reply};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_session_shows_the_config_and_sets_a_codix_yaml_value() {
    let lv = Live::with(Opts {
        codix_yaml: Some("jobs:\n  defaults:\n    timeout: 5m\n".into()),
        ..Default::default()
    });
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;

    // 看：不问人；里面有路由与 provider，没有密钥的值。
    let t = c
        .turn(
            &s,
            "Call config_show once. Do not ask me to confirm. Then reply with only the word SHOWN.",
        )
        .await;
    t.ended();
    assert!(t.tools.iter().any(|n| n == "config_show"), "{:?}", t.tools);
    assert!(t.prompts.is_empty(), "查看不问人：{:?}", t.prompts);
    let shown = t
        .results
        .first()
        .map(|(r, _)| r.clone())
        .unwrap_or_default();
    assert!(shown.contains("anthropic"), "{shown}");
    for (k, v) in live::credentials() {
        if k.contains("TOKEN") || k.contains("KEY") {
            assert!(!shown.contains(v.as_str()), "密钥不该出现在 config_show 里");
        }
    }

    // 校验不过的不写：路径对、值读不懂。
    let before = std::fs::read_to_string(lv.home.path().join("codix.yaml")).unwrap();
    let t = c
        .turn(
            &s,
            "Call codix_config_set once with path \"jobs.defaults.network\" and value \"nonsense\". Do not \
             ask me to confirm. Then reply with only the word TRIED.",
        )
        .await;
    t.ended();
    assert!(
        t.results.iter().any(|(_, err)| *err),
        "读不懂的值被拒：{:?}",
        t.results
    );
    assert_eq!(
        std::fs::read_to_string(lv.home.path().join("codix.yaml")).unwrap(),
        before,
        "没写"
    );

    // 写：jobs 下的当场生效（`/doctor` 里的作业网络检查跟着变），旧文件进备份。
    let t = c
        .turn(
            &s,
            "Call codix_config_set once with path \"jobs.defaults.network\" and value \"inherit\". Do not \
             ask me to confirm. Then reply with only the word SET.",
        )
        .await;
    t.ended();
    assert!(
        t.results.iter().any(|(_, err)| !err),
        "写成了：{:?}",
        t.results
    );
    let yaml = std::fs::read_to_string(lv.home.path().join("codix.yaml")).unwrap();
    assert!(
        yaml.contains("inherit") && yaml.contains("timeout"),
        "{yaml}"
    );
    let backups: Vec<_> = std::fs::read_dir(lv.home.path().join("config-backups"))
        .map(|d| d.flatten().collect())
        .unwrap_or_default();
    assert!(!backups.is_empty(), "改之前备份了");
    let Reply::Doctor(checks) = c.ok(Command::Doctor).await else {
        panic!("doctor")
    };
    let check = checks
        .into_iter()
        .find(|x| x.name == "codix.jobs_network")
        .unwrap();
    assert_eq!(
        check.status,
        HealthStatus::Degraded,
        "jobs 当场重读，不用重启：{check:?}"
    );
}

/// 排在前面的 provider 连不上，引擎这次调用换到下一个：转录里有一条换了的提示（`ProviderSwitched`），这一轮照常答完。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_dead_first_provider_falls_over_and_the_switch_is_pushed() {
    let lv = Live::new();
    let creds = live::credentials();
    let key_env = if creds.contains_key("ANTHROPIC_AUTH_TOKEN") {
        "ANTHROPIC_AUTH_TOKEN"
    } else {
        "ANTHROPIC_API_KEY"
    };
    let model = live::model();
    let tiers = serde_json::json!({"strong": model, "balanced": model, "fast": model});
    let settings = serde_json::json!({
        "permissions": {"prompt_timeout_secs": 600},
        "providers": [
            {"id": "dead", "vendor": "generic", "api": "openai_chat",
             "base_url": "http://127.0.0.1:9/v1", "api_key_env": "HOME",
             "tiers": {"strong": "m", "balanced": "m", "fast": "m"}},
            {"id": "real", "vendor": "anthropic", "api": "anthropic",
             "base_url": creds.get("ANTHROPIC_BASE_URL").cloned().unwrap_or_default(),
             "api_key_env": key_env, "tiers": tiers},
        ],
        "model": {"order": ["dead", "real"]},
    });
    std::fs::write(
        lv.home.path().join("settings.json"),
        serde_json::to_string_pretty(&settings).unwrap(),
    )
    .unwrap();
    let mut c = lv.start().await;
    let s = c.new_session().await;
    let t = c.turn(&s, "Reply with only the word OK.").await;
    t.ended();
    assert!(t.text.contains("OK"), "{}", t.text);
    let (from, to, reason) = c
        .events()
        .into_iter()
        .find_map(|e| match e {
            Event::ProviderSwitched {
                session,
                from,
                to,
                reason,
                ..
            } if session == s => Some((from, to, reason)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("没有换 provider 的事件：{}", c.recent(20)));
    assert_eq!(to, "real", "{from:?} → {to}：{reason}");
    assert!(reason.contains("dead"), "原因点名连不上的那个：{reason}");
}
