//! 本机与本进程的几样信息：实例进程与 TUI 两边都要用。

use std::time::{SystemTime, UNIX_EPOCH};

use nix::errno::Errno;
use nix::sys::signal::kill;
use nix::unistd::Pid;
use port::ClientInfo;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

pub fn hostname() -> String {
    nix::unistd::gethostname()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_default()
}

/// 信号 0 只做存在与权限检查。EPERM 说明进程在、只是不归我们：同一个 uid 下不该出现，
/// 但把它当"死了"会删掉别人的发现文件，所以算活着。
pub fn pid_alive(pid: u32) -> bool {
    // pid 0 与负数在 kill(2) 里指整个进程组，不能拿来探测。
    match i32::try_from(pid) {
        Ok(p) if p > 0 => matches!(kill(Pid::from_raw(p), None), Ok(()) | Err(Errno::EPERM)),
        _ => false,
    }
}

/// 当前进程作为接入者的样子（`conix list` 里"被谁接入"那一栏）。
pub fn this_client() -> ClientInfo {
    let now = now_ms();
    ClientInfo {
        user: std::env::var("USER").unwrap_or_default(),
        pid: std::process::id(),
        tty: nix::unistd::ttyname(std::io::stdin())
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        ssh_from: std::env::var("SSH_CONNECTION")
            .ok()
            .and_then(|s| s.split_whitespace().next().map(str::to_owned)),
        attached_ms: now,
        last_input_ms: now,
    }
}
