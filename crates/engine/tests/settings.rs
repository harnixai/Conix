//! Conix 的 cli 层设置在真实的 Agix 上的效果。单独一个测试文件：一个进程只能嵌入一份引擎
//! （进程组的登记只有一份），和 `embedded.rs` 放一起会互相挡。

/// Conix 的 cli 层（`host::hub::cli_settings`）在真实的 Agix 0.4 上生效：开了 `client_may_loosen`，
/// 流程站要的 `acceptEdits` 不被夹回；只读与流程工具的 allow 落在 cli 层。不调模型，离线可跑。
#[tokio::test]
async fn the_cli_layer_lets_a_session_loosen_its_mode() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let (engine, client, _notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: home.path().to_path_buf(),
        project: project_path.clone(),
        groups_file: home.path().join("groups.json"),
        settings: host::hub::cli_settings(),
        model: None,
        host_lib: Default::default(),
    })
    .await
    .unwrap();
    let r = client
        .call(
            "session.create",
            serde_json::json!({"project_root": project_path, "options": {
                "settings": {"permissions": {"mode": "acceptEdits", "ask": ["Write"]},
                             "sandbox": {"network": {"mode": "deny_all"}},
                             "session": {"token_budget": 1000}}}}),
        )
        .await
        .unwrap();
    assert!(r.get("clamped_options").is_none(), "没被夹：{r}");
    let sid = r["session_id"].as_str().unwrap().to_string();
    let resumed = client
        .call("session.resume", serde_json::json!({"session_id": sid}))
        .await
        .unwrap();
    assert_eq!(resumed["permission"]["mode"], "acceptEdits", "{resumed}");
    let explain = |key: &'static str| {
        let client = &client;
        let project_path = &project_path;
        async move {
            client
                .call(
                    "config.explain",
                    serde_json::json!({"key": key, "scene": "code", "project_root": project_path}),
                )
                .await
                .unwrap()
        }
    };
    let loosen = explain("permissions.client_may_loosen").await;
    assert_eq!(loosen["keys"][0]["value"], true, "{loosen}");
    let allow = explain("permissions.allow").await;
    let v = allow["keys"][0]["value"].to_string();
    assert!(
        v.contains("flow_submit") && v.contains("job_list"),
        "{allow}"
    );
    let history = explain("app.history").await;
    assert_eq!(history["keys"][0]["value"], "project", "{history}");
    engine.shutdown().await;
}
