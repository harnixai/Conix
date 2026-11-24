//! `conix host` 的看护（architecture §8.7）：由 new / start 拉起的是它，它再拉起真正的实例进程
//! （同样的参数，环境里带 `CONIX_HOST_CHILD`）并等着。实例非正常退出（崩溃、被杀）就退避重启；
//! 正常停（SIGTERM、StopInstance）就跟着结束。起来不到几秒就自己报错退出的不重启：那是锁被占、
//! 配置错，重启救不回来。

use std::time::{Duration, Instant};

use anyhow::Result;
use client::Layout;
use client::layout::{read_json, write_json_atomic};
use tokio::signal::unix::{SignalKind, signal};

/// 实例进程认这个环境变量：有它就是被看护的那个，自己不再看护。
pub const CHILD_ENV: &str = "CONIX_HOST_CHILD";

/// 起来不到这么久就失败的，不是"崩了"，是起不来。
const STARTUP: Duration = Duration::from_secs(5);
const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(5),
    Duration::from_secs(30),
];
/// 这么久里重启超过 `MAX_RESTARTS` 次就放弃。
const WINDOW: Duration = Duration::from_secs(600);
const MAX_RESTARTS: usize = 5;

pub async fn run(name: String, project: std::path::PathBuf) -> Result<()> {
    let layout = Layout::new(app::config::home());
    let exe = std::env::current_exe()?;
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    let mut hup = signal(SignalKind::hangup())?;
    let mut recent: Vec<Instant> = vec![];
    record(&layout, &name, |m| {
        m.restarts = 0;
        m.crashed = false;
    });
    loop {
        let started = Instant::now();
        let mut child = tokio::process::Command::new(&exe)
            .args(["host", "--name", &name, "--project"])
            .arg(&project)
            .env(CHILD_ENV, "1")
            .spawn()?;
        let status = loop {
            tokio::select! {
                s = child.wait() => break s?,
                _ = term.recv() => forward(&child, nix::sys::signal::Signal::SIGTERM),
                _ = int.recv() => forward(&child, nix::sys::signal::Signal::SIGTERM),
                _ = hup.recv() => {}
            }
        };
        if status.success() {
            return Ok(());
        }
        // 自己报错退出（有退出码）而且刚起来：锁被占、配置错，重启救不回来。被信号杀死的
        // （kill -9、段错误）没有退出码，总是重启。
        if let Some(code) = status.code()
            && started.elapsed() < STARTUP
        {
            std::process::exit(code);
        }
        let now = Instant::now();
        recent.retain(|t| now.duration_since(*t) < WINDOW);
        if recent.len() >= MAX_RESTARTS {
            eprintln!("conix host {name}: crashed {MAX_RESTARTS} times in 10 minutes; giving up");
            record(&layout, &name, |m| m.crashed = true);
            std::process::exit(status.code().unwrap_or(1));
        }
        let wait = BACKOFF[recent.len().min(BACKOFF.len() - 1)];
        recent.push(now);
        eprintln!("conix host {name}: exited with {status}; restarting in {wait:?}");
        record(&layout, &name, |m| {
            m.restarts += 1;
            m.last_crash_ms = Some(client::now_ms());
        });
        tokio::time::sleep(wait).await;
    }
}

fn forward(child: &tokio::process::Child, sig: nix::sys::signal::Signal) {
    if let Some(pid) = child.id() {
        let _ = nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid as i32), sig);
    }
}

fn record(layout: &Layout, name: &str, f: impl FnOnce(&mut client::InstanceMeta)) {
    let path = layout.meta(name);
    let Some(mut m) = read_json::<client::InstanceMeta>(&path) else {
        return;
    };
    f(&mut m);
    let _ = write_json_atomic(&path, &m);
}
