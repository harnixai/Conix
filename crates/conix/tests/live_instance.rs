//! LIVE：实例的生命周期（architecture §8.1、§8.7）——新建、列出、发现文件与 `meta.json`、
//! 目录与 socket 的权限、同名实例的锁、停掉再照 `meta.json` 拉起、崩溃留下的残留。

mod live;

use std::os::unix::fs::PermissionsExt;

use live::{Live, NAME};
use port::{InstanceState, Instances};

fn mode(p: &std::path::Path) -> u32 {
    std::fs::metadata(p).unwrap().permissions().mode() & 0o777
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_new_instance_is_listed_with_its_discovery_file_meta_and_private_socket() {
    let lv = Live::new();
    let c = lv.start().await;

    let list = lv.instances.list(false).await.unwrap();
    assert_eq!(list.len(), 1);
    let i = &list[0];
    assert_eq!(i.name.0, NAME);
    assert_eq!(i.state, InstanceState::Running);
    assert_eq!(i.project.as_deref(), Some(lv.project_path().as_str()));
    assert!(i.holder.is_some(), "发现文件里记着接入者");

    let disc = lv.read("instance.json");
    assert_eq!(disc["name"], NAME);
    assert_eq!(disc["project"], lv.project_path());
    assert_eq!(disc["pid"].as_u64().map(|p| p as u32), i.pid);
    assert_eq!(disc["holder"]["pid"], std::process::id());
    let meta = lv.read("meta.json");
    assert_eq!(meta["project"], lv.project_path());
    assert!(meta["stopped_ms"].is_null());

    assert_eq!(mode(&lv.instance_dir()), 0o700, "实例目录只给自己");
    assert_eq!(mode(&lv.layout().socket(NAME)), 0o600, "socket 只给自己");
    drop(c);
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_second_host_with_the_same_name_cannot_take_the_lock() {
    let lv = Live::new();
    let _c = lv.start().await;
    let elsewhere = live::new_project("elsewhere");
    let out = lv.run_host_directly_in(NAME, &elsewhere.path().to_string_lossy());
    assert!(!out.status.success(), "同名的第二个实例进程应当立即退出");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("already running"), "{err}");
    let list = lv.instances.list(false).await.unwrap();
    assert_eq!(list[0].state, InstanceState::Running, "原来的实例不受影响");
}

/// 作业属于项目：同一个项目上换个名字再起一个实例也不行，报出占着它的是谁（architecture §8.9）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_second_instance_on_the_same_project_is_refused() {
    let lv = Live::new();
    let _c = lv.start().await;
    let out = lv.run_host_directly("other");
    assert!(!out.status.success(), "同一项目的第二个实例应当立即退出");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("already served by instance") && err.contains(NAME),
        "{err}"
    );
    assert!(lv.state().owner().exists(), "第一个实例认领了项目状态");
    let all = lv.instances.list(true).await.unwrap();
    assert_eq!(all.len(), 1, "第二个没起来，也没留下 meta.json：{all:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_stopped_instance_is_started_again_from_its_meta() {
    let lv = Live::new();
    let c = lv.start().await;
    let pid = lv.read("instance.json")["pid"].as_u64().unwrap();
    drop(c);
    lv.stop().await;

    assert!(lv.instances.list(false).await.unwrap().is_empty());
    let all = lv.instances.list(true).await.unwrap();
    assert_eq!(all[0].state, InstanceState::Stopped);
    assert_eq!(all[0].project.as_deref(), Some(lv.project_path().as_str()));
    assert!(all[0].stopped_ms.is_some(), "meta.json 记下停止的时刻");
    assert!(!lv.instance_dir().join("instance.json").exists());
    assert!(!lv.layout().socket(NAME).exists());

    let _c = lv.start_again().await;
    let list = lv.instances.list(false).await.unwrap();
    assert_eq!(list[0].state, InstanceState::Running);
    assert_eq!(list[0].project.as_deref(), Some(lv.project_path().as_str()));
    assert_ne!(lv.read("instance.json")["pid"].as_u64().unwrap(), pid);
    assert!(lv.read("meta.json")["stopped_ms"].is_null());
}

/// 实例进程崩了（被 kill -9），看护进程把它重新拉起来，`list` 写出重启过几次（architecture §8.7）。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_crashed_instance_is_restarted_by_its_supervisor() {
    let lv = Live::new();
    let c = lv.start().await;
    drop(c);
    let pid = lv.read("instance.json")["pid"].as_u64().unwrap();
    lv.kill_host().await;
    let _c = lv.wait_restarted(pid).await;
    let list = lv.instances.list(false).await.unwrap();
    assert_eq!(list[0].state, InstanceState::Running);
    assert_eq!(list[0].restarts, 1, "{list:?}");
    assert!(lv.host_log().contains("restarting"), "看护进程写了日志");
}

/// 连看护进程一起被杀：没人拉起，列出来是残留，清掉后照 `meta.json` 拉起。
#[tokio::test(flavor = "multi_thread")]
#[ignore = "live"]
async fn a_killed_instance_is_listed_as_stale_cleaned_and_can_be_started() {
    let lv = Live::new();
    let c = lv.start().await;
    drop(c);
    lv.kill_host_and_supervisor().await;

    let list = lv.instances.list(false).await.unwrap();
    assert_eq!(list.len(), 1, "残留的不论 all 都列出来");
    assert_eq!(list[0].state, InstanceState::Stale);
    assert!(
        !lv.instance_dir().join("instance.json").exists(),
        "列出时清掉发现文件"
    );
    assert!(!lv.layout().socket(NAME).exists(), "列出时清掉 socket");
    assert_eq!(
        lv.instances.list(true).await.unwrap()[0].state,
        InstanceState::Stopped,
        "清过之后就是已停"
    );

    let _c = lv.start_again().await;
    assert_eq!(
        lv.instances.list(false).await.unwrap()[0].state,
        InstanceState::Running
    );
}
