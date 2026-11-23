//! LIVE：真实实例上的自检与查询（需求 D-01、C-13、C-21）——`Doctor` 的每一项来自真实引擎与实例自己的检查；
//! 可选模型的列表；`@` 补全用的项目文件列表。不调模型。

mod live;

use live::Live;
use port::{Command, HealthStatus, Reply};

async fn doctor(c: &live::Client) -> Vec<port::HealthCheck> {
    let Reply::Doctor(v) = c.ok(Command::Doctor).await else {
        panic!("doctor 回的不是 Doctor")
    };
    v
}

fn status(v: &[port::HealthCheck], name: &str) -> HealthStatus {
    v.iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("没有 {name}：{v:?}"))
        .status
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn doctor_reports_the_engines_checks_and_the_instances_own() {
    let lv = Live::new();
    let c = lv.start().await;
    let v = doctor(&c).await;
    let names: Vec<&str> = v.iter().map(|c| c.name.as_str()).collect();
    // 引擎自己的检查（daemon.doctor）与实例自己的几项都在。
    assert!(
        names.iter().any(|n| !n.starts_with("codix.")),
        "没有引擎的检查：{names:?}"
    );
    for own in [
        "codix.project_state",
        "codix.gitignore",
        "codix.config",
        "codix.notify",
        "codix.session_loosen",
        "codix.jobs_network",
        "codix.flows",
    ] {
        assert!(names.contains(&own), "缺 {own}：{names:?}");
    }
    assert_eq!(status(&v, "codix.flows"), HealthStatus::Ok, "流程在跑");
    assert_eq!(status(&v, "codix.config"), HealthStatus::Ok);
    // Codix 自己在 cli 层开了放宽，没有 managed 层压它；作业缺省 deny_all。
    assert_eq!(
        status(&v, "codix.session_loosen"),
        HealthStatus::Ok,
        "{v:?}"
    );
    assert_eq!(status(&v, "codix.jobs_network"), HealthStatus::Ok, "{v:?}");
    let state = v.iter().find(|c| c.name == "codix.project_state").unwrap();
    assert!(
        state.summary.contains(&lv.project_path()),
        "项目状态目录在项目里：{state:?}"
    );
}

/// `jobs.defaults.network: inherit` 时无人值守的命令会被 Agix 拒：自检要报降级；
/// `codix.yaml` 读不懂的项也在 `codix.config` 里报出来。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn doctor_flags_an_inherited_job_network_and_an_unreadable_config() {
    let lv = Live::with(live::Opts {
        codix_yaml: Some("jobs:\n  defaults:\n    network: inherit\n    timeout: soon\n".into()),
        ..Default::default()
    });
    let c = lv.start().await;
    let v = doctor(&c).await;
    assert_eq!(
        status(&v, "codix.jobs_network"),
        HealthStatus::Degraded,
        "{v:?}"
    );
    assert_eq!(status(&v, "codix.config"), HealthStatus::Degraded, "{v:?}");
    let cfg = v.iter().find(|c| c.name == "codix.config").unwrap();
    assert!(cfg.summary.contains("timeout"), "{cfg:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn models_and_project_files_are_listed_for_the_pickers() {
    let lv = Live::new();
    let c = lv.start().await;
    // `@` 补全：git 跟踪的与没被忽略的新文件，相对路径；`.git` 里的不出现。
    std::fs::create_dir_all(lv.project.path().join("src")).unwrap();
    std::fs::write(lv.project.path().join("src/lib.rs"), "// new\n").unwrap();
    std::fs::write(lv.project.path().join(".gitignore"), "secret.txt\n").unwrap();
    std::fs::write(lv.project.path().join("secret.txt"), "x").unwrap();
    let Reply::Files(files) = c.ok(Command::ProjectFiles).await else {
        panic!("project_files")
    };
    assert!(files.contains(&"README.md".to_string()), "{files:?}");
    assert!(
        files.contains(&"src/lib.rs".to_string()),
        "未提交的新文件也在：{files:?}"
    );
    assert!(
        !files.contains(&"secret.txt".to_string()),
        "被忽略的不出现：{files:?}"
    );
    assert!(files.iter().all(|f| !f.starts_with(".git/")), "{files:?}");

    // `/model` 的选择列表：引擎向各 provider 的端点要模型；第三方的兼容端点可能不给列表（此时是空的），
    // 所以只验命令成功、给出来的每项有 provider 与 id。
    let Reply::Models(models) = c.ok(Command::ListModels).await else {
        panic!("list_models")
    };
    for m in &models {
        assert!(!m.provider.is_empty() && !m.id.is_empty(), "{m:?}");
    }
}
