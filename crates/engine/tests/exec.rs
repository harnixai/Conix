//! 执行服务的 Agix 后端对着真实嵌入的引擎（flows.md §7.14）：`read` 档写不了工作目录、`workspace` 档只写得了
//! 点名的目录、禁读名单读不到；平台没有沙箱时（Linux 上没装 bwrap）是拒绝，不是悄悄不隔离。

use std::sync::Arc;
use std::time::Duration;

use harnix_flow::{Exec, ExecError, ExecProfile, ExecRequest};
use host::exec::ConixExec;

fn sh(cwd: &std::path::Path, script: &str, profile: ExecProfile) -> ExecRequest {
    let mut r = ExecRequest::new(
        vec!["sh".into(), "-c".into(), script.into()],
        cwd,
        Duration::from_secs(30),
    );
    r.env = vec![("PATH".into(), std::env::var_os("PATH").unwrap_or_default())];
    r.profile = profile;
    r
}

#[tokio::test(flavor = "multi_thread")]
async fn processes_run_in_the_engines_sandbox_as_their_profile_says() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let p = project.path().canonicalize().unwrap();
    let (engine, client, _notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: home.path().to_path_buf(),
        project: p.clone(),
        groups_file: None,
        settings: host::hub::cli_settings(),
        model: None,
        host_lib: Default::default(),
    })
    .await
    .unwrap();
    let exec = ConixExec::agix(Arc::new(client), false);
    std::fs::create_dir_all(p.join("out")).unwrap();
    std::fs::create_dir_all(p.join("other")).unwrap();
    std::fs::write(p.join("other/secret.txt"), "the other side\n").unwrap();

    let read = exec
        .run(sh(&p, "echo hi; echo x > here.txt", ExecProfile::Read))
        .await;
    if let Err(ExecError::Refused(m)) = &read {
        // 这台机器没有沙箱后端：拒绝就是对的，下面的不必再看。
        eprintln!("no sandbox here: {m}");
        engine.shutdown().await;
        return;
    }
    let read = read.unwrap();
    assert_eq!(read.stdout, b"hi\n");
    assert_ne!(
        read.code,
        Some(0),
        "read: the working directory is not writable"
    );
    assert!(!p.join("here.txt").exists());
    assert_eq!(read.enforced.backend, "agix");

    let write = exec
        .run(sh(
            &p,
            "echo x > out/a.txt && echo y > b.txt",
            ExecProfile::Workspace {
                writable: vec![p.join("out")],
            },
        ))
        .await
        .unwrap();
    assert!(p.join("out/a.txt").is_file(), "the directory it was given");
    assert!(!p.join("b.txt").exists(), "nowhere else");
    assert_ne!(write.code, Some(0));

    let mut denied = sh(&p, "cat other/secret.txt", ExecProfile::Read);
    denied.deny_read = vec![p.join("other")];
    let denied = exec.run(denied).await.unwrap();
    assert_ne!(denied.code, Some(0));
    assert!(!String::from_utf8_lossy(&denied.stdout).contains("the other side"));

    engine.shutdown().await;
}
