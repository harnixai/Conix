//! 插件的接缝（docs/design/plugins.md）在真实嵌入的 Agix 上：装 zip、看两份披露、启用后流程包挂进来、
//! 运行里的程序步骤与 host 门禁经 `plugin.call` 调插件函数、插件函数里的 `conix.*` 宿主库。不调模型。
//! 单独一个测试文件：一个进程只能嵌入一份引擎。

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use host::hub::Registry;
use host::{Hub, HubConfig, Service};
use port::{Command, FlowOrigin, Reply, RunStatus};

const FIXTURE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/plugins/demo");

/// 把测试插件打成 `<dir>/demo-0.1.0.zip`（`plugin.toml` 在 zip 的根上）。
fn pack(dir: &Path) -> PathBuf {
    let zip = dir.join("demo-0.1.0.zip");
    let st = std::process::Command::new("zip")
        .arg("-qr")
        .arg(&zip)
        .arg(".")
        .current_dir(FIXTURE)
        .status()
        .expect("zip");
    assert!(st.success());
    zip
}

async fn wait_run(
    hub: &Hub,
    run: &port::RunId,
    want: impl Fn(&port::RunView) -> bool,
) -> port::RunView {
    for _ in 0..200 {
        if let Ok(Reply::Run(v)) = hub.handle(Command::GetRun { run: run.clone() }).await
            && want(&v)
        {
            return *v;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let last = hub.handle(Command::GetRun { run: run.clone() }).await;
    panic!("the run did not get there: {last:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_is_installed_enabled_and_its_flow_runs_on_its_functions() {
    let home = tempfile::tempdir().unwrap();
    let store = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    pack(store.path());

    let host_lib: Arc<std::sync::OnceLock<host::plugins::hostlib::HostLibHandler>> =
        Default::default();
    let (engine, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: home.path().to_path_buf(),
        project: project_path.clone(),
        groups_file: Some(home.path().join("groups.json")),
        settings: host::hub::cli_settings(),
        model: None,
        host_lib: host_lib.clone(),
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
    hub.set_plugin_dirs(vec![store.path().to_path_buf()]);
    let lib_hub = hub.clone();
    let _ = host_lib.set(Arc::new(move |call| {
        Box::pin(host::plugins::hostlib::handle(lib_hub.clone(), call))
    }));
    let flows = host::flows::start(
        &hub,
        host::flows::FlowsConfig {
            runs_dir: home.path().join("runs"),
            worktrees_dir: project_path.join(".wkts"),
            workspaces_dir: home.path().join("ws"),
            policies_file: None,
            command_gates: true,
            artifacts_dir: None,
            library_home: None,
        },
    )
    .await
    .unwrap();

    // 可装的包按文件名列出；装完是停用的，回两份披露。
    let Reply::Plugins(p) = hub.handle(Command::ListPlugins).await.unwrap() else {
        panic!()
    };
    assert_eq!(p.available.len(), 1, "{p:?}");
    assert!(p.installed.is_empty());
    let Reply::Plugin { plugin, disclosure } = hub
        .handle(Command::InstallPlugin {
            source: "demo".into(),
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    assert!(!plugin.enabled, "装完不自动启用");
    assert_eq!(plugin.flows, ["demo-flow"]);
    assert!(
        disclosure
            .capabilities
            .iter()
            .any(|c| c.contains("demo.open") && c.contains("demo/**")),
        "{disclosure:?}"
    );
    assert!(
        disclosure
            .capabilities
            .iter()
            .any(|c| c.contains("host library `conix`")),
        "{disclosure:?}"
    );
    assert!(
        disclosure
            .conix
            .iter()
            .any(|c| c.contains("demo-check") && c.contains("demo.check")),
        "{disclosure:?}"
    );
    let flow_names = |r: Reply| match r {
        Reply::Flows(f) => f,
        r => panic!("{r:?}"),
    };
    let before = flow_names(hub.handle(Command::ListFlows).await.unwrap());
    assert!(
        !before.iter().any(|f| f.name == "demo-flow"),
        "停用的插件不挂"
    );

    // 启用：流程挂进来，来源是这个插件。
    hub.handle(Command::SetPluginEnabled {
        name: "demo".into(),
        enabled: true,
    })
    .await
    .unwrap();
    let after = flow_names(hub.handle(Command::ListFlows).await.unwrap());
    let demo = after
        .iter()
        .find(|f| f.name == "demo-flow")
        .expect("demo-flow");
    assert_eq!(demo.origin, FlowOrigin::Plugin);
    assert_eq!(demo.source.as_deref(), Some("demo"));

    // 运行：程序步骤调 demo.open（可写），门禁调 demo.check（只读，读到步骤写的文件）。
    let Reply::RunStarted(run) = hub
        .handle(Command::StartRun {
            flow: "demo-flow".into(),
            subject: "try it".into(),
            inputs: serde_json::Value::Null,
            step: false,
            session: None,
        })
        .await
        .unwrap()
    else {
        panic!()
    };
    let done = wait_run(&hub, &run.id, |v| {
        matches!(
            v.summary.status,
            RunStatus::Finished | RunStatus::Waiting { .. } | RunStatus::Cancelled
        )
    })
    .await;
    assert_eq!(done.summary.status, RunStatus::Finished, "{done:#?}");
    assert_eq!(
        std::fs::read_to_string(project_path.join("demo/opened.txt"))
            .unwrap()
            .trim(),
        run.id.0
    );

    // 插件函数里的宿主库：经 plugin.call（via host）能读，不能开运行。
    let defs = client
        .call(
            "plugin.call",
            serde_json::json!({"function": "demo.defs", "cwd": project_path}),
        )
        .await
        .unwrap();
    assert_eq!(defs["ok"], true, "{defs}");
    assert!(
        defs["value"]
            .as_array()
            .unwrap()
            .iter()
            .any(|f| f["name"] == "demo-flow" && f["source"] == "demo"),
        "{defs}"
    );
    let start = client
        .call(
            "plugin.call",
            serde_json::json!({"function": "demo.start", "cwd": project_path}),
        )
        .await
        .unwrap();
    assert_eq!(start["ok"], false, "门禁与步骤里不能开运行：{start}");
    assert!(
        start
            .to_string()
            .contains("only a person's session may start"),
        "{start}"
    );

    // 停用：流程从列表里消失；卸载：插件没了。
    hub.handle(Command::SetPluginEnabled {
        name: "demo".into(),
        enabled: false,
    })
    .await
    .unwrap();
    let off = flow_names(hub.handle(Command::ListFlows).await.unwrap());
    assert!(!off.iter().any(|f| f.name == "demo-flow"));
    hub.handle(Command::UninstallPlugin {
        name: "demo".into(),
    })
    .await
    .unwrap();
    let Reply::Plugins(p) = hub.handle(Command::ListPlugins).await.unwrap() else {
        panic!()
    };
    assert!(p.installed.is_empty(), "{p:?}");
    flows.stop().await;
    engine.shutdown().await;
}
