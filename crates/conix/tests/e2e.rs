//! 端到端：真实的 `conix host` 进程（嵌入的 Agix）+ `SocketInstances` + 无终端的 `Core`。
//! 新建实例、接入、在输入框里说一句话，这一轮要有结局显示在转录里；再接入一次被占用、
//! 强制接入把前一个踢掉，最后停掉实例。没有凭据时那一轮以 no_credentials 结束，所以离线可跑。

use std::sync::Arc;
use std::time::{Duration, Instant};

use app::Core;
use app::core::Start;
use client::{Layout, SocketInstances};
use model::InstOp;
use port::Instances;
use tui::Size;

const SIZE: Size = Size {
    cols: 160,
    rows: 45,
};

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn core(instances: Arc<SocketInstances>, start: Start) -> Core {
    let (core, _) = Core::new(
        instances,
        Arc::new(keys::Keymap::default()),
        SIZE,
        Box::new(now_ms),
        start,
        model::Settings::default(),
    );
    core
}

/// 一直泵消息，直到画面里出现 `want`（或超时）。
async fn until(core: &mut Core, want: impl Fn(&str) -> bool, what: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let _ = tokio::time::timeout(Duration::from_millis(100), core.next_msg()).await;
        core.pump();
        // 心跳、空闲这些定时由 tick 驱动：不 tick，实例 35 秒收不到心跳就会断开。
        if core.deadline().is_some_and(|d| d <= core.now_ms()) {
            core.tick();
        }
        let text = app::snapshot::render_text(&core.frame(), SIZE);
        if want(&text) {
            return text;
        }
        assert!(Instant::now() < deadline, "等不到{what}：\n{text}");
    }
}

/// 用例中途失败也要停掉实例：否则留下一个常驻的 `conix host` 进程。
struct StopOnDrop(std::path::PathBuf);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = std::process::Command::new(env!("CARGO_BIN_EXE_conix"))
            .args(["stop", "e2e"])
            .env("CONIX_HOME", &self.0)
            .output();
    }
}

fn keys(core: &mut Core, seq: &str) {
    for k in seq.split_whitespace() {
        core.key(keys::Key::parse(k).unwrap());
    }
}

fn type_text(core: &mut Core, s: &str) {
    for c in s.chars() {
        core.handle(model::Msg::Text(c));
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn create_attach_talk_kick_and_stop_a_real_instance() {
    // socket 路径有长度上限（macOS 104 字节）：家目录放在短路径下。
    let home = tempfile::Builder::new()
        .prefix("cx")
        .tempdir_in("/tmp")
        .unwrap();

    let _guard = StopOnDrop(home.path().to_path_buf());
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    // 刚链接出的二进制第一次执行要被 macOS 扫描，先跑一次，免得算进实例的启动时间。
    std::process::Command::new(env!("CARGO_BIN_EXE_conix"))
        .arg("--version")
        .output()
        .unwrap();
    let instances = Arc::new(SocketInstances::new(
        Layout::new(home.path()),
        env!("CARGO_BIN_EXE_conix"),
    ));

    let mut a = core(
        instances.clone(),
        Start::Op(InstOp::Create {
            name: Some("e2e".into()),
            project: Some(project_path.to_string_lossy().into_owned()),
        }),
    )
    .await;
    until(&mut a, |t| t.contains("[CONIX] e2e"), "接入新实例").await;

    type_text(&mut a, "Reply with the single word: pong");
    keys(&mut a, "Enter");
    let text = until(
        &mut a,
        |t| t.contains("credential") || t.to_lowercase().contains("pong\n") || t.contains("Error:"),
        "这一轮的结局",
    )
    .await;
    assert!(!text.contains("Disconnected"), "{text}");

    let listed = instances.list(false).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].sessions, 0, "list 不为了计数去接入");
    assert!(listed[0].holder.is_some(), "发现文件里记着接入者");

    // 第二个 TUI：被占用；强制接入后第一个收到被踢。
    match instances.attach("e2e", Default::default()).await {
        Err(port::PortError::Occupied { .. }) => {}
        Err(e) => panic!("{e:?}"),
        Ok(_) => panic!("同一实例只能接一个 TUI"),
    }
    let mut b = core(
        instances.clone(),
        Start::Op(InstOp::Attach {
            name: "e2e".into(),
            force: true,
        }),
    )
    .await;
    until(&mut b, |t| t.contains("[CONIX] e2e"), "强制接入").await;
    until(
        &mut a,
        |t| t.contains("踢") || t.contains("强制接入"),
        "前一个 TUI 被踢的画面",
    )
    .await;
    // 新接入者看得到刚才那个会话（会话属于实例，不属于 TUI）。
    until(&mut b, |t| t.contains("Reply with"), "会话列表里的那个会话").await;

    instances.stop("e2e").await.unwrap();
    assert!(instances.list(false).await.unwrap().is_empty());
    let all = instances.list(true).await.unwrap();
    assert_eq!(all[0].state, port::InstanceState::Stopped);
}
