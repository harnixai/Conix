//! LIVE：插件（docs/design/plugins.md）经真实的实例进程——可装的包从 `CONIX_PLUGIN_DIRS` 列出、装完停用、
//! 两份披露、启用后流程挂进来、运行里的程序步骤与 host 门禁调插件函数。测试插件是 engine 的 fixture，
//! 不调模型。

mod live;

use std::path::PathBuf;
use std::time::Duration;

use live::{Answering, Live, Opts, QUICK};
use port::{Command, Event, FlowOrigin, Reply, RunOp, RunStatus, StationStatus};

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
            "CONIX_PLUGIN_DIRS".into(),
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
        disclosure.conix.iter().any(|l| l.contains("demo-open")),
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

/// 仓库里的插件打成 zip（`scripts/build-plugins.sh` → `target/plugins/`），回那个目录。
fn built_plugins() -> PathBuf {
    let root = PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    let st = std::process::Command::new(root.join("scripts/build-plugins.sh"))
        .current_dir(&root)
        .status()
        .expect("build-plugins.sh");
    assert!(st.success());
    root.join("target/plugins")
}

/// 真实模型把 `xforge-quick` 走完一次（plugins.md §7 第 5 步）：开变更（插件函数）→ 提案与规格增量（会话，
/// 门禁 structure、spec-delta）→ 实施（会话，门禁 unclaimed）→ 验证（会话，门禁 structure，人确认）→ 归档
/// （插件函数）。结束时规格并进了 `xforge/specs/`，变更进了 `xforge/archive/`。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn xforge_quick_goes_from_open_to_archive_with_a_real_model() {
    let lv = Live::with(Opts {
        env: vec![(
            "CONIX_PLUGIN_DIRS".into(),
            built_plugins().to_string_lossy().into_owned(),
        )],
        ..Opts::default()
    });
    let project = PathBuf::from(lv.project_path());
    std::fs::create_dir_all(project.join("xforge")).unwrap();
    std::fs::write(
        project.join("xforge/manifest.yaml"),
        "version: 1\nverification:\n  commands: {}\n",
    )
    .unwrap();
    std::fs::write(project.join("greet.txt"), "hello\n").unwrap();
    let mut c = lv.start_trusted().await;
    c.answering(Answering::Auto);
    c.ok(Command::InstallPlugin {
        source: "xforge".into(),
    })
    .await;
    c.ok(Command::SetPluginEnabled {
        name: "xforge".into(),
        enabled: true,
    })
    .await;
    c.wait_event("xforge 的流程挂进来", QUICK, |e| {
        matches!(e, Event::FlowUpserted(f) if f.name == "xforge-quick").then_some(())
    })
    .await;

    let Reply::RunStarted(run) = c
        .ok(Command::StartRun {
            flow: "xforge-quick".into(),
            subject: "Add a farewell line to greet.txt".into(),
            inputs: serde_json::json!({
                "title": "Add a farewell line to greet.txt: after the line hello, add the line goodbye",
                "risk": "low"
            }),
        })
        .await
    else {
        panic!("start_run")
    };
    let id = run.id.clone();
    let station = |v: &port::RunView, s: &str| {
        v.stations
            .iter()
            .find(|x| x.id.0 == s)
            .map(|x| x.status)
            .unwrap_or(StationStatus::Pending)
    };
    // 走到验证站等人确认；中途卡住（门禁没过、任务失败）就带着运行的样子失败。
    let at_confirm = c
        .wait_event("走到验证站等确认", Duration::from_secs(900), |e| match e {
            Event::RunDetail(v) if v.summary.id == id => {
                if station(v, "verify") == StationStatus::Working
                    && station(v, "apply") == StationStatus::Passed
                    && matches!(v.summary.status, RunStatus::Waiting { .. })
                {
                    return Some(Ok(()));
                }
                match &v.summary.status {
                    RunStatus::Waiting { reason }
                        if reason != "confirm" && reason != "awaiting confirmation" =>
                    {
                        if v.stations.iter().any(|s| s.status == StationStatus::Unmet) {
                            return Some(Err((format!("{v:#?}"), v.main_session.clone())));
                        }
                        None
                    }
                    RunStatus::Cancelled | RunStatus::Finished => {
                        Some(Err((format!("{v:#?}"), v.main_session.clone())))
                    }
                    _ => None,
                }
            }
            _ => None,
        })
        .await;
    if let Err((v, main)) = at_confirm {
        // 带上主会话最后的对话：站没交、门禁没过时，原因多半在模型调的命令与它们的结果里。
        let w = c.watch(&main).await;
        let tail: Vec<String> = w
            .history
            .iter()
            .rev()
            .take(40)
            .rev()
            .map(|h| match h {
                port::HistoryItem::Tool {
                    name,
                    input,
                    content,
                    is_error,
                    ..
                } => format!(
                    "TOOL {name} {} -> {}{}",
                    input.chars().take(300).collect::<String>(),
                    if *is_error { "ERROR " } else { "" },
                    content.chars().take(400).collect::<String>()
                ),
                other => format!("{other:?}").chars().take(400).collect(),
            })
            .collect();
        panic!(
            "xforge-quick 没走到验证站：{v}\n--- 主会话最后的对话 ---\n{}",
            tail.join("\n")
        );
    }
    c.ok(Command::RunOp {
        run: id.clone(),
        op: RunOp::Confirm {
            station: port::StationId("verify".into()),
        },
    })
    .await;
    c.wait_event("归档后结束", Duration::from_secs(120), |e| match e {
        Event::RunDetail(v) if v.summary.id == id && v.summary.status == RunStatus::Finished => {
            Some(())
        }
        _ => None,
    })
    .await;

    let greet = std::fs::read_to_string(project.join("greet.txt")).unwrap();
    assert!(greet.contains("goodbye"), "实施改了文件：{greet}");
    let specs: Vec<_> = std::fs::read_dir(project.join("xforge/specs"))
        .expect("规格并进了 xforge/specs/")
        .flatten()
        .collect();
    assert!(!specs.is_empty(), "至少一个能力的规格");
    let archived: Vec<_> = std::fs::read_dir(project.join("xforge/archive"))
        .expect("变更进了 xforge/archive/")
        .flatten()
        .collect();
    assert_eq!(archived.len(), 1);
    let change = std::fs::read_to_string(archived[0].path().join("change.yaml")).unwrap();
    assert!(change.contains("archived"), "{change}");
}
