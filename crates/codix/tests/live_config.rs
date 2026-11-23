//! LIVE：在会话里配置自己（architecture §8.11）——会话里的真实模型调宿主工具：加一个 provider 不切换、
//! 测通、切换、切完还能答、退回；连不上的 provider 测不通也切不过去，原来的模型照常答。

mod live;

use live::{Answering, Live};
use serde_json::Value;

fn settings(lv: &Live) -> Value {
    serde_json::from_slice(&std::fs::read(lv.home.path().join("settings.json")).unwrap()).unwrap()
}

fn order(lv: &Live) -> Value {
    settings(lv)["model"]["order"].clone()
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_provider_is_added_tested_switched_to_and_rolled_back_from_a_session() {
    let lv = Live::new();
    let creds = live::credentials();
    let base_url = creds.get("ANTHROPIC_BASE_URL").cloned().unwrap_or_default();
    let key_env = if creds.contains_key("ANTHROPIC_AUTH_TOKEN") {
        "ANTHROPIC_AUTH_TOKEN"
    } else {
        "ANTHROPIC_API_KEY"
    };
    let model = live::model();
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    c.watch(&s).await;

    // 加：同一个端点再定义一个叫 backup 的 provider。现在只有自带的 anthropic，加之前它被照原样写成显式的、
    // 路由钉在它上面——加完路由不变。
    let config = serde_json::json!({"vendor": "anthropic", "api": "anthropic", "base_url": base_url,
        "api_key_env": key_env, "tiers": {"strong": model}});
    let t = c
        .turn(
            &s,
            &format!(
                "Call config_provider_add once with provider_id \"backup\" and config {config}. Then call \
                 config_provider_test with provider_id \"backup\". Do not ask me to confirm. Then reply with \
                 only the first word of the test's result (PASS or FAIL)."
            ),
        )
        .await;
    t.ended();
    assert!(
        t.tools.iter().any(|n| n == "config_provider_add"),
        "{:?}",
        t.tools
    );
    assert!(
        t.tools.iter().any(|n| n == "config_provider_test"),
        "{:?}",
        t.tools
    );
    assert!(
        t.results
            .iter()
            .any(|(r, err)| !err && r.starts_with("PASS")),
        "测通：{:?}",
        t.results
    );
    assert_eq!(order(&lv), serde_json::json!(["anthropic"]), "加了不切换");
    let ids: Vec<String> = settings(&lv)["providers"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| p["id"].as_str().map(str::to_string))
        .collect();
    assert!(
        ids.contains(&"anthropic".into()) && ids.contains(&"backup".into()),
        "{ids:?}"
    );

    // 切：先测、备份、写，切完立刻打一句。
    let t = c
        .turn(
            &s,
            "Call config_switch once with provider_id \"backup\". Do not ask me to confirm. Then reply \
             with only the word SWITCHED if it switched, or FAILED.",
        )
        .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| !err && r.starts_with("switched")),
        "{:?}",
        t.results
    );
    assert_eq!(order(&lv), serde_json::json!(["backup", "anthropic"]));
    let backups = std::fs::read_dir(lv.home.path().join("config-backups"))
        .expect("切换前留了备份")
        .count();
    assert!(backups >= 1);

    // 切完这个会话照常能答。
    let t = c.turn(&s, "Reply with only the word ALIVE.").await;
    t.ended();
    assert!(t.text.contains("ALIVE"), "{}", t.text);

    // 退。
    let t = c
        .turn(
            &s,
            "Call config_rollback once. Do not ask me to confirm. Then reply with only the word BACK.",
        )
        .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| !err && r.starts_with("rolled back")),
        "{:?}",
        t.results
    );
    assert_eq!(order(&lv), serde_json::json!(["anthropic"]));

    // 连不上的：测不通，切不过去，路由原样，原来的模型照常答。
    let t = c
        .turn(
            &s,
            "Call config_provider_add once with provider_id \"dead\" and config {\"vendor\": \"generic\", \
             \"api\": \"openai_chat\", \"base_url\": \"http://127.0.0.1:9/v1\", \"api_key_env\": \"HOME\", \
             \"tiers\": {\"strong\": \"m\"}}. Then call config_switch once with provider_id \"dead\". Do not \
             ask me to confirm. Then reply with only the word REFUSED if the switch did not happen.",
        )
        .await;
    t.ended();
    assert!(
        t.results
            .iter()
            .any(|(r, err)| *err && r.contains("not switched")),
        "切不过去：{:?}",
        t.results
    );
    assert_eq!(order(&lv), serde_json::json!(["anthropic"]), "路由原样");
    let t = c.turn(&s, "Reply with only the word STILL.").await;
    t.ended();
    assert!(t.text.contains("STILL"), "{}", t.text);
}
