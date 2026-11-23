//! LIVE：插件（docs/design/plugins.md）经真实的实例进程——可装的包从 `CODIX_PLUGIN_DIRS` 列出、装完停用、
//! 两份披露、启用后流程挂进来、运行里的程序步骤与 host 门禁调插件函数。测试插件是 engine 的 fixture，
//! 不调模型。

mod live;

use std::path::PathBuf;
use std::time::Duration;

use live::{Live, Opts, QUICK};
use port::{Command, Event, FlowOrigin, Reply, RunStatus};

const DEMO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../engine/tests/fixtures/plugins/demo"
);

/// 把测试插件打成 `<dir>/demo-0.1.0.zip`。
fn pack(dir: &std::path::Path) -> PathBuf {
    let zip = dir.join("demo-0.1.0.zip");
    let st = std::process::Command::new("zip")
        .arg("-qr")
        .arg(&zip)
        .arg(".")
        .current_dir(DEMO)
        .status()
        .expect("zip");
    assert!(st.success());
    zip
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_plugin_from_the_package_dirs_is_installed_enabled_and_its_flow_runs() {
    let store = tempfile::tempdir().unwrap();
    pack(store.path());
    let lv = Live::with(Opts {
        env: vec![(
            "CODIX_PLUGIN_DIRS".into(),
            store.path().to_string_lossy().into_owned(),
        )],
        ..Opts::default()
    });
    let mut c = lv.start_trusted().await;

    let Reply::Plugins(p) = c.ok(Command::ListPlugins).await else {
        panic!("list_plugins")
    };
    assert_eq!(
        p.available
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>(),
        ["demo"],
        "{p:?}"
    );
    let Reply::Plugin { plugin, disclosure } = c
        .ok(Command::InstallPlugin {
            source: "demo".into(),
        })
        .await
    else {
        panic!("install_plugin")
    };
    assert!(!plugin.enabled, "装完不自动启用");
    assert!(
        disclosure.codix.iter().any(|l| l.contains("demo-open")),
        "{disclosure:?}"
    );

    c.ok(Command::SetPluginEnabled {
        name: "demo".into(),
        enabled: true,
    })
    .await;
    c.wait_event("插件的流程挂进来", QUICK, |e| match e {
        Event::FlowUpserted(f) if f.name == "demo-flow" => {
            assert_eq!(f.origin, FlowOrigin::Plugin);
            assert_eq!(f.source.as_deref(), Some("demo"));
            Some(())
        }
        _ => None,
    })
    .await;

    let Reply::RunStarted(run) = c
        .ok(Command::StartRun {
            flow: "demo-flow".into(),
            subject: "live".into(),
            inputs: serde_json::Value::Null,
        })
        .await
    else {
        panic!("start_run")
    };
    let id = run.id.clone();
    c.wait_event("运行跑完", Duration::from_secs(60), |e| match e {
        Event::RunUpserted(s) if s.id == id && s.status != RunStatus::Running => {
            Some(s.status.clone())
        }
        _ => None,
    })
    .await;
    let Reply::Run(v) = c
        .ok(Command::GetRun {
            run: run.id.clone(),
        })
        .await
    else {
        panic!("get_run")
    };
    assert_eq!(v.summary.status, RunStatus::Finished, "{v:#?}");
    let opened = PathBuf::from(lv.project_path()).join("demo/opened.txt");
    assert_eq!(
        std::fs::read_to_string(opened).unwrap().trim(),
        run.id.0,
        "程序步骤经插件函数写的"
    );

    c.ok(Command::UninstallPlugin {
        name: "demo".into(),
    })
    .await;
    c.wait_event("插件的流程撤下", QUICK, |e| {
        matches!(e, Event::FlowRemoved { name } if name == "demo-flow").then_some(())
    })
    .await;
}
