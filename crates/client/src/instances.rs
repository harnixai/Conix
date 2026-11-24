//! 本机实例的目录（architecture §8.1、§8.7）：扫 `instances/*`、拉起 `conix host`、接入、停止。

use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use nix::errno::Errno;
use nix::sys::signal::{Signal, kill};
use nix::unistd::Pid;
use port::{
    Attach, Backend, InstanceInfo, InstanceName, InstanceState, Instances, NewInstance, PortError,
};
use tokio::net::UnixStream;
use tokio::time::{Instant, sleep};

use crate::backend::SocketBackend;
use crate::layout::{InstanceFile, InstanceMeta, Layout, read_json};
use crate::local::{hostname, pid_alive, this_client};

/// macOS 第一次执行一个新写出的二进制要先扫描，大的调试版本能卡十来秒；之后每次是毫秒级。
const START_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_millis(100);
/// 名字进 socket 路径，而 `sun_path` 在 macOS 上只有 104 字节。
const MAX_NAME_BYTES: usize = 40;

pub struct SocketInstances {
    layout: Layout,
    exe: PathBuf,
    idle_timeout_ms: Option<u64>,
    env: Vec<(String, String)>,
}

impl SocketInstances {
    /// `exe` 是拉起实例用的 conix 二进制（通常是 `std::env::current_exe()`）。
    pub fn new(layout: Layout, exe: impl Into<PathBuf>) -> SocketInstances {
        SocketInstances {
            layout,
            exe: exe.into(),
            idle_timeout_ms: None,
            env: vec![],
        }
    }

    /// 拉起的实例进程额外带的环境变量。LIVE 测试用它把凭据只交给实例，而不改测试进程
    /// 自己的环境（Rust 2024 里改进程环境是 unsafe，workspace 禁了 unsafe）。
    pub fn with_env(mut self, env: impl IntoIterator<Item = (String, String)>) -> SocketInstances {
        self.env.extend(env);
        self
    }

    /// TUI 这一侧的空闲超时（配置或 `--idle-timeout`）随 `Hello` 带给实例，实例的兜底断开按它算。
    pub fn with_idle_timeout(mut self, ms: u64) -> SocketInstances {
        self.idle_timeout_ms = Some(ms);
        self
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    fn exists(&self, name: &str) -> bool {
        self.layout.instance(name).is_dir()
    }

    /// 发现文件在、pid 活着、socket 连得上才算在跑。只连不握手：实例把握手前就断开的
    /// 连接直接丢掉，不会影响当前的接入者。
    async fn running(&self, name: &str) -> Option<InstanceFile> {
        let f = read_json::<InstanceFile>(&self.layout.discovery(name))?;
        (pid_alive(f.pid) && UnixStream::connect(self.layout.socket(name)).await.is_ok())
            .then_some(f)
    }

    fn clean_stale(&self, name: &str) {
        let _ = fs::remove_file(self.layout.socket(name));
        let _ = fs::remove_file(self.layout.discovery(name));
    }

    async fn connect(
        &self,
        name: &str,
        force: bool,
        idle_timeout_ms: Option<u64>,
    ) -> Result<SocketBackend, PortError> {
        if !self.exists(name) {
            return Err(PortError::NoSuchInstance(name.into()));
        }
        if self.running(name).await.is_none() {
            return Err(PortError::Failed(format!("instance {name} is not running")));
        }
        SocketBackend::connect(
            self.layout.socket(name),
            Some(self.layout.discovery(name)),
            this_client(),
            force,
            idle_timeout_ms.or(self.idle_timeout_ms),
        )
        .await
    }

    /// 子进程自成进程组：TUI 所在终端的 Ctrl+C / SIGHUP 不会波及实例。
    async fn launch(&self, name: &str, project: &Path) -> Result<(), PortError> {
        let failed = |e: std::io::Error| PortError::Failed(e.to_string());
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(self.layout.instance(name))
            .map_err(failed)?;
        let log_path = self.layout.log(name);
        let _ = crate::layout::rotate_log(
            &log_path,
            crate::layout::LOG_MAX_BYTES,
            crate::layout::LOG_KEEP,
        );
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(failed)?;
        let mut child = std::process::Command::new(&self.exe)
            .args(["host", "--name", name, "--project"])
            .arg(project)
            // 实例与 TUI 必须认同一个家目录：TUI 的 Layout 不一定来自环境变量（测试、--home）。
            .env("CONIX_HOME", self.layout.home())
            // cargo run / cargo test 设的加载器回退路径里有 target/debug/deps：macOS 的 dyld 照着它
            // 逐个目录找系统库，一次 exec 要十几秒。实例也不该带着开发时的加载路径常驻。
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .envs(self.env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(failed)?)
            .stderr(log)
            .process_group(0)
            .spawn()
            .map_err(failed)?;
        let socket = self.layout.socket(name);
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            // 实例先绑 socket、再写发现文件；接入要读发现文件，两样都在才算起来了。
            if self.layout.discovery(name).exists() && UnixStream::connect(&socket).await.is_ok() {
                break;
            }
            if let Ok(Some(status)) = child.try_wait() {
                return Err(PortError::Failed(format!(
                    "instance host exited ({status}):\n{}",
                    log_tail(&log_path)
                )));
            }
            if Instant::now() >= deadline {
                return Err(PortError::Failed(format!(
                    "instance host did not come up within {}s:\n{}",
                    START_TIMEOUT.as_secs(),
                    log_tail(&log_path)
                )));
            }
            sleep(POLL).await;
        }
        // 实例比 TUI 活得久，不等它；但 TUI 还在时它若退出，得有人收尸，否则留下的僵尸
        // 进程让 `pid_alive` 一直为真。
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

#[async_trait]
impl Instances for SocketInstances {
    async fn list(&self, all: bool) -> Result<Vec<InstanceInfo>, PortError> {
        let Ok(dir) = fs::read_dir(self.layout.instances()) else {
            return Ok(vec![]);
        };
        let mut names: Vec<String> = dir
            .flatten()
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        names.sort();
        let host = hostname();
        let mut out = vec![];
        for name in names {
            let blank = |state| InstanceInfo {
                name: InstanceName(name.clone()),
                host: host.clone(),
                pid: None,
                project: None,
                state,
                started_ms: None,
                stopped_ms: None,
                version: String::new(),
                sessions: 0,
                waiting: 0,
                jobs: 0,
                holder: None,
                restarts: 0,
            };
            let meta = read_json::<InstanceMeta>(&self.layout.meta(&name));
            if let Some(f) = read_json::<InstanceFile>(&self.layout.discovery(&name)) {
                let state = if self.running(&name).await.is_some() {
                    InstanceState::Running
                } else {
                    self.clean_stale(&name);
                    InstanceState::Stale
                };
                // 残留的不论 `all` 都列出来：实例意外退出了，用户该知道（需求 I-08）。
                out.push(InstanceInfo {
                    host: f.host,
                    pid: Some(f.pid),
                    project: f.project,
                    started_ms: Some(f.started_ms),
                    version: f.version,
                    holder: f.holder.filter(|_| state == InstanceState::Running),
                    restarts: meta.as_ref().map_or(0, |m| m.restarts),
                    ..blank(state)
                });
            } else if all && let Some(m) = meta {
                out.push(InstanceInfo {
                    project: m.project,
                    stopped_ms: m.stopped_ms,
                    ..blank(InstanceState::Stopped)
                });
            }
        }
        Ok(out)
    }

    async fn create(
        &self,
        opts: NewInstance,
    ) -> Result<(InstanceInfo, Arc<dyn Backend>), PortError> {
        let project = match opts.project {
            Some(p) => PathBuf::from(p),
            None => std::env::current_dir().map_err(|e| PortError::Failed(e.to_string()))?,
        };
        let project = project
            .canonicalize()
            .map_err(|e| PortError::Failed(format!("{}: {e}", project.display())))?;
        if crate::layout::unfit_project(
            &project,
            crate::layout::user_home().as_deref(),
            self.layout.home(),
        ) {
            return Err(PortError::Failed(format!(
                "{} cannot be a project (its .conix would be, or be inside, the global directory {}): run conix in a project, or in your home directory to use the personal project",
                project.display(),
                self.layout.home().display()
            )));
        }
        let name = match opts.name {
            Some(n) if !valid_name(n.as_str()) => {
                return Err(PortError::Failed(format!("invalid instance name: {n}")));
            }
            Some(n) if self.exists(n.as_str()) => {
                return Err(PortError::Failed(format!("instance {n} already exists")));
            }
            Some(n) => n.0,
            None => derive_name(&project, |n| self.exists(n)),
        };
        self.launch(&name, &project).await?;
        let b = self.connect(&name, false, None).await?;
        Ok((b.instance().clone(), Arc::new(b)))
    }

    async fn attach(&self, name: &str, opts: Attach) -> Result<Arc<dyn Backend>, PortError> {
        Ok(Arc::new(
            self.connect(name, opts.force, opts.idle_timeout_ms).await?,
        ))
    }

    async fn start(&self, name: &str) -> Result<Arc<dyn Backend>, PortError> {
        let meta = read_json::<InstanceMeta>(&self.layout.meta(name))
            .ok_or_else(|| PortError::NoSuchInstance(name.into()))?;
        if self.running(name).await.is_none() {
            self.clean_stale(name);
            let project = meta
                .project
                .ok_or_else(|| PortError::Failed(format!("instance {name} has no project")))?;
            self.launch(name, Path::new(&project)).await?;
        }
        Ok(Arc::new(self.connect(name, false, None).await?))
    }

    async fn stop(&self, name: &str) -> Result<(), PortError> {
        if !self.exists(name) {
            return Err(PortError::NoSuchInstance(name.into()));
        }
        let discovery = self.layout.discovery(name);
        // 先确认实例真在（socket 连得上）：崩溃留下的发现文件里的 pid 可能已被别的进程用了，
        // 照它发 SIGTERM 会误杀。
        let Some(f) = self.running(name).await else {
            self.clean_stale(name);
            return Ok(());
        };
        let pid = i32::try_from(f.pid)
            .ok()
            .filter(|p| *p > 0)
            .ok_or_else(|| PortError::Failed(format!("bad pid {}", f.pid)))?;
        match kill(Pid::from_raw(pid), Signal::SIGTERM) {
            Ok(()) => {}
            Err(Errno::ESRCH) => {
                self.clean_stale(name);
                return Ok(());
            }
            Err(e) => return Err(PortError::Failed(e.to_string())),
        }
        // 实例收尾的最后一步才删发现文件，所以它没了就是真的停了。
        let deadline = Instant::now() + STOP_TIMEOUT;
        while discovery.exists() {
            if !pid_alive(f.pid) {
                self.clean_stale(name);
                break;
            }
            if Instant::now() >= deadline {
                return Err(PortError::Failed(format!(
                    "instance {name} did not stop within {}s",
                    STOP_TIMEOUT.as_secs()
                )));
            }
            sleep(POLL).await;
        }
        Ok(())
    }
}

/// 实例名：会成为目录名与 socket 路径的一段，所以限字符与长度，也不许以 `.` / `-` 开头
/// （`.` 开头会被当成隐藏目录或 `..`，`-` 开头在命令行上像个选项）。
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.starts_with(['.', '-'])
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// 由项目目录名生成实例名：不合法的字符换成 `-`，与已有的重名时依次加 `-2`、`-3`……
pub fn derive_name(project: &Path, taken: impl Fn(&str) -> bool) -> String {
    let raw = project
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let mut base = String::new();
    for c in raw.chars() {
        let c = if c.is_alphanumeric() || matches!(c, '_' | '.') {
            c
        } else {
            '-'
        };
        if c == '-' && base.ends_with('-') {
            continue;
        }
        // 留出 `-NN` 的位置。
        if base.len() + c.len_utf8() > MAX_NAME_BYTES - 4 {
            break;
        }
        base.push(c);
    }
    let base = base.trim_start_matches(['.', '-']).trim_end_matches('-');
    let base = if base.is_empty() { "conix" } else { base };
    std::iter::once(base.to_string())
        .chain((2..).map(|i| format!("{base}-{i}")))
        .find(|n| !taken(n))
        .unwrap()
}

/// 这个项目的实例：在跑的优先，其次残留的，最后已停的。
pub fn for_project<'a>(list: &'a [InstanceInfo], project: &Path) -> Option<&'a InstanceInfo> {
    let want = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    list.iter()
        .filter(|i| i.project.as_deref().map(Path::new) == Some(want.as_path()))
        .min_by_key(|i| match i.state {
            InstanceState::Running => 0,
            InstanceState::Stale => 1,
            InstanceState::Stopped => 2,
        })
}

fn log_tail(path: &Path) -> String {
    let text = fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(20)..].join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_derive_from_the_project_dir_and_dedup() {
        let none = |_: &str| false;
        assert_eq!(derive_name(Path::new("/w/My Project!"), none), "my-project");
        assert_eq!(derive_name(Path::new("/w/.dotfiles"), none), "dotfiles");
        assert_eq!(derive_name(Path::new("/"), none), "conix");
        assert_eq!(derive_name(Path::new("/w/项目"), none), "项目");
        let taken = |n: &str| n == "app" || n == "app-2";
        assert_eq!(derive_name(Path::new("/w/app"), taken), "app-3");
        let long = derive_name(Path::new(&format!("/w/{}", "x".repeat(100))), none);
        assert!(valid_name(&long) && long.len() <= MAX_NAME_BYTES - 4);
    }

    #[test]
    fn explicit_names_are_checked() {
        assert!(valid_name("api-2"));
        for bad in ["", ".", "..", "-x", "a/b", "a b"] {
            assert!(!valid_name(bad), "{bad:?}");
        }
    }
}
