//! 实例目录的认领（architecture §8.1、§8.7）：锁、`meta.json`、发现文件、socket。

use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

use client::layout::{read_json, write_json_atomic};
use client::{InstanceFile, InstanceMeta, Layout};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use tokio::net::UnixListener;

pub use client::local::{hostname, now_ms};

#[derive(Debug, thiserror::Error)]
pub enum ClaimError {
    #[error("instance {0} is already running")]
    AlreadyRunning(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// 认领下来的实例目录。锁跟着它活：进程怎么退出（包括崩溃）内核都会放掉 flock，所以
/// 残留的只会是发现文件与 socket，`list` 凭 pid 与连接能认出来。
pub struct Claim {
    layout: Layout,
    name: String,
    _lock: Flock<File>,
}

/// 拿不到锁就是同名实例已经在跑，返回 [`ClaimError::AlreadyRunning`]，调用方应立即退出。
/// `project` 为 `None` 时沿用 `meta.json` 里原来的。
pub fn claim(layout: &Layout, name: &str, project: Option<&str>) -> Result<Claim, ClaimError> {
    let dir = layout.instance(name);
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)?;
    // `mode` 只作用于新建的目录；目录是上次运行留下的，就补设一次。
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(layout.lock(name))?;
    let lock = Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, e)| {
        if e == Errno::EWOULDBLOCK {
            ClaimError::AlreadyRunning(name.into())
        } else {
            ClaimError::Io(e.into())
        }
    })?;
    let old = read_json::<InstanceMeta>(&layout.meta(name));
    // 看护进程记的重启次数留着：它在实例认领之前就写了。
    let old = old.unwrap_or_default();
    let meta = InstanceMeta {
        name: name.into(),
        project: project.map(str::to_owned).or(old.project),
        stopped_ms: None,
        ..old
    };
    write_json_atomic(&layout.meta(name), &meta)?;
    Ok(Claim {
        layout: layout.clone(),
        name: name.into(),
        _lock: lock,
    })
}

impl Claim {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// 要在 tokio 运行时里调用。拿着锁，所以已有的 `host.sock` 一定是上一次留下的，删掉再绑；
    /// 不删的话 bind 报 `EADDRINUSE`。绑定与 chmod 之间 socket 短暂是 umask 给的权限，
    /// 但所在目录是 0700，别的用户本来就进不来。
    pub fn bind(&self) -> io::Result<UnixListener> {
        let path = self.layout.socket(&self.name);
        match fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let listener = UnixListener::bind(&path)?;
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
        Ok(listener)
    }

    /// 写或重写发现文件（起来之后写一次，接入者变了再写）。
    pub fn write_discovery(&self, f: &InstanceFile) -> io::Result<()> {
        write_json_atomic(&self.layout.discovery(&self.name), f)
    }

    /// 正常退出的收尾。发现文件最后删：`conix stop` 以它消失作为"停好了"。
    pub fn release(self) -> io::Result<()> {
        let _ = fs::remove_file(self.layout.socket(&self.name));
        let meta_path = self.layout.meta(&self.name);
        let mut meta = read_json::<InstanceMeta>(&meta_path).unwrap_or(InstanceMeta {
            name: self.name.clone(),
            ..Default::default()
        });
        meta.stopped_ms = Some(now_ms());
        write_json_atomic(&meta_path, &meta)?;
        match fs::remove_file(self.layout.discovery(&self.name)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}
