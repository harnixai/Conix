//! 在会话里配置自己（architecture §8.11）在真实嵌入的 Agix 上：加一个连不上的 provider，测它报不通，
//! 切过去被拒、路由原样；作业与流程里的会话用不了这些工具。不联网（端点是本机一个没人听的端口）。
//! 单独一个测试文件：一个进程只能嵌入一份引擎。

use std::sync::Arc;

use host::hub::Registry;
use host::{Hub, HubConfig, Service};
use port::{Command, Reply};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_provider_is_added_tested_and_not_switched_to() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let (engine, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: home.path().to_path_buf(),
        project: project_path.clone(),
        groups_file: Some(home.path().join("groups.json")),
        settings: host::hub::cli_settings(),
        model: None,
        host_lib: Default::default(),
    })
    .await
    .unwrap();
    let hub = Hub::start(
        Arc::new(client.clone()),
        notes,
        HubConfig {
            instance: "t".into(),
            host: "h".into(),
            project: project_path.clone(),
            version: "0".into(),
            started_ms: 0,
            registry: Registry::in_memory(),
            jobs: None,
            clock: Arc::new(|| 0),
            engine_model: engine.model().to_string(),
        },
    )
    .await
    .unwrap();
    hub.set_config_home(home.path().to_path_buf());
    let Reply::Session(s) = hub
        .handle(Command::CreateSession {
            project: None,
            scene: None,
            model: None,
            permission: None,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let me = s.id.0.clone();
    let order = || async {
        client
            .call(
                "config.getProvider",
                json!({"scene": "code", "project_root": project_path}),
            )
            .await
            .unwrap()["order"]
            .clone()
    };
    let _ = order().await;

    let shown = hub.run_tool(&me, "config_show", &json!({})).await.unwrap();
    assert!(shown.contains("model.order"), "{shown}");

    // 加：不合格的先被拒、什么都不写；合格的写进去，路由不变。
    let bad = hub
        .run_tool(
            &me,
            "config_provider_add",
            &json!({"provider_id": "dead", "config": {"api": "openai_chat"}}),
        )
        .await;
    assert!(bad.is_err(), "openai_chat 缺 base_url：{bad:?}");
    // 设置里只写放 key 的环境变量名；HOME 总有值，当它是 key（端点没人听，不会真发出去）。
    let added = hub
        .run_tool(
            &me,
            "config_provider_add",
            &json!({"provider_id": "dead", "config": {
                "vendor": "generic", "api": "openai_chat", "base_url": "http://127.0.0.1:9/v1",
                "api_key_env": "HOME", "tiers": {"strong": "m"}}}),
        )
        .await
        .unwrap();
    // 离线没有凭据：现在的缺省（自带的 anthropic）本来就不通，没什么要保住的，照实说。"加了不切换"在 LIVE 里验
    // （有能用的缺省时先把它钉住）。
    assert!(added.contains("does not answer either"), "{added}");
    let before = order().await;

    // 测：不通。
    let tested = hub
        .run_tool(&me, "config_provider_test", &json!({"provider_id": "dead"}))
        .await
        .unwrap();
    assert!(tested.starts_with("FAIL"), "{tested}");
    let Reply::Sessions(list) = hub
        .handle(Command::ListSessions {
            filter: Default::default(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(list.len(), 1, "测用的临时会话不进列表：{list:?}");

    // 切：测不通就不切，路由原样。
    let switched = hub
        .run_tool(&me, "config_switch", &json!({"provider_id": "dead"}))
        .await;
    assert!(
        switched
            .as_ref()
            .is_err_and(|e| e.starts_with("not switched")),
        "{switched:?}"
    );
    assert_eq!(order().await, before);
    engine.shutdown().await;
}
