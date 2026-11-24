//! LIVE：一轮因为上限而收尾——引擎的 `max_turns`、`budget_exceeded` 在端口上
//! 是 `StopReason::MaxTurns` / `Budget`。
//! 上限用实例的 `settings.json` 给，断言只看 Conix 的翻译。

mod live;

use live::{Answering, Live};
use port::StopReason;

fn settings(lv: &Live, extra: serde_json::Value) {
    let mut v = serde_json::json!({"permissions": {"prompt_timeout_secs": 600}});
    for (k, x) in extra.as_object().unwrap() {
        v[k] = x.clone();
    }
    std::fs::write(
        lv.home.path().join("settings.json"),
        serde_json::to_string_pretty(&v).unwrap(),
    )
    .unwrap();
}

/// 一轮里模型只许调一次：它要先调工具，第二次调用被拦下，这一轮以 `MaxTurns` 收尾。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_turn_over_the_model_call_limit_ends_as_max_turns() {
    let lv = Live::new();
    settings(&lv, serde_json::json!({"turn": {"max_model_calls": 1}}));
    let mut c = lv.start().await;
    c.answering(Answering::Auto);
    let s = c.new_session().await;
    let t = c
        .turn(
            &s,
            "Use the Bash tool to run exactly: echo limit-test . Then, after you see its output, reply \
             with only the word DONE.",
        )
        .await;
    assert_eq!(t.end, Some(Ok(StopReason::MaxTurns)), "{t:?}");
    // 会话照常可用：下一轮重新计数。
    let t = c.turn(&s, "Reply with only the word OK.").await;
    t.ended();
}

/// 会话的 token 预算比一次调用的输入还小：引擎在调用前就拦住，这一轮以 `Budget` 收尾（作业那一路见 live_jobs）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_turn_over_the_token_budget_ends_as_budget() {
    let lv = Live::new();
    settings(&lv, serde_json::json!({"turn": {"token_budget": 1000}}));
    let mut c = lv.start().await;
    let s = c.new_session().await;
    let t = c.turn(&s, "Reply with only the word OK.").await;
    assert!(
        matches!(t.end, Some(Ok(StopReason::Budget(_)))),
        "{:?}\n{}",
        t.end,
        t.text
    );
}
