//! 全局目录与项目里的布局、发现文件（architecture §8.1）。实例进程写，TUI 读，两边共用这一份定义。

use std::path::{Path, PathBuf};

use port::ClientInfo;
use serde::{Deserialize, Serialize};

/// `~/.conix`（或 `CONIX_HOME`）下的路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    home: PathBuf,
}

impl Layout {
    pub fn new(home: impl Into<PathBuf>) -> Layout {
        Layout { home: home.into() }
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    pub fn instances(&self) -> PathBuf {
        self.home.join("instances")
    }

    pub fn instance(&self, name: &str) -> PathBuf {
        self.instances().join(name)
    }

    pub fn discovery(&self, name: &str) -> PathBuf {
        self.instance(name).join("instance.json")
    }

    pub fn lock(&self, name: &str) -> PathBuf {
        self.instance(name).join("instance.lock")
    }

    pub fn socket(&self, name: &str) -> PathBuf {
        self.instance(name).join("host.sock")
    }

    pub fn groups(&self, name: &str) -> PathBuf {
        self.instance(name).join("groups.json")
    }

    pub fn log(&self, name: &str) -> PathBuf {
        self.instance(name).join("host.log")
    }
}

/// `instance.json`：实例进程起来、socket 能连之后写，接入者变了就重写，退出时删掉。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceFile {
    pub name: String,
    pub pid: u32,
    pub host: String,
    #[serde(default)]
    pub project: Option<String>,
    pub started_ms: u64,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub holder: Option<ClientInfo>,
    /// 看护它的进程（architecture §8.7）；它的父进程。
    #[serde(default)]
    pub supervisor_pid: Option<u32>,
}

/// 实例自己的设定（`instance.json` 删掉之后也留着），`conix start` 照它重新拉起。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InstanceMeta {
    pub name: String,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub stopped_ms: Option<u64>,
    /// 这次启动以来看护进程重启过实例几次。
    #[serde(default)]
    pub restarts: u32,
    #[serde(default)]
    pub last_crash_ms: Option<u64>,
    /// 短时间里崩了太多次，看护进程放弃了。
    #[serde(default)]
    pub crashed: bool,
}

impl Layout {
    /// 全局流程包（可写层），所有实例共用。
    pub fn flows(&self) -> PathBuf {
        self.home.join("flows")
    }

    pub fn meta(&self, name: &str) -> PathBuf {
        self.instance(name).join("meta.json")
    }

    /// 认领过的项目：凭证 → 项目路径（architecture §8.9）。
    pub fn known_projects(&self) -> PathBuf {
        self.home.join("known-projects.json")
    }

    pub fn known_projects_lock(&self) -> PathBuf {
        self.home.join("known-projects.lock")
    }
}

/// 项目层目录，与引擎的同一个（`EmbedConfig.project_dir`）。
pub const PROJECT_DIR: &str = ".conix";

/// 一个项目里 Conix 的东西：定义在项目层目录下随项目提交，本机状态在 `state/conix/`——Agix 给嵌入方的
/// `state/<产品名小写>/`（`Layout::host_state_dir`），引擎不写它、按配置目录保护它。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectState {
    project: PathBuf,
}

impl ProjectState {
    pub fn new(project: impl Into<PathBuf>) -> ProjectState {
        ProjectState {
            project: project.into(),
        }
    }

    pub fn project(&self) -> &Path {
        &self.project
    }

    pub fn data_dir(&self) -> PathBuf {
        self.project.join(PROJECT_DIR)
    }

    /// `state/conix/`。
    pub fn dir(&self) -> PathBuf {
        self.data_dir().join("state").join("conix")
    }

    pub fn owner(&self) -> PathBuf {
        self.dir().join("owner.json")
    }

    pub fn lock(&self) -> PathBuf {
        self.dir().join("host.lock")
    }

    pub fn quarantine(&self) -> PathBuf {
        self.dir().join("quarantine")
    }

    pub fn sessions(&self) -> PathBuf {
        self.dir().join("sessions.json")
    }

    /// 作业定义：随项目提交。
    pub fn jobs(&self) -> PathBuf {
        self.data_dir().join("jobs.json")
    }

    /// 作业在这台机器上的启用状态与调度基线。
    pub fn jobs_state(&self) -> PathBuf {
        self.dir().join("jobs-state.json")
    }

    pub fn job_history_dir(&self) -> PathBuf {
        self.dir().join("jobs")
    }

    pub fn job_history(&self, job: &str) -> PathBuf {
        self.job_history_dir().join(format!("{job}.jsonl"))
    }

    pub fn flow_runs(&self) -> PathBuf {
        self.dir().join("flow-runs")
    }

    pub fn flow_workspaces(&self) -> PathBuf {
        self.dir().join("workspaces")
    }

    /// 作业起的运行在会话上多出的限制（网络、禁 `CronCreate`），按运行记。
    pub fn run_policies(&self) -> PathBuf {
        self.dir().join("run-policies.json")
    }
}

/// `host.log` 至多多大就轮转（只追加不轮转的话常驻实例的日志会一直长）。
pub const LOG_MAX_BYTES: u64 = 10 * 1024 * 1024;
/// 轮转留几份旧的：`host.log.1` … `host.log.3`。
pub const LOG_KEEP: usize = 3;

/// 超过 `max_bytes` 就把 `log` 挪成 `log.1`，原来的 `.1` 挪成 `.2`……至多留 `keep` 份。回有没有轮转。
pub fn rotate_log(log: &Path, max_bytes: u64, keep: usize) -> std::io::Result<bool> {
    match std::fs::metadata(log) {
        Ok(m) if m.len() > max_bytes => {}
        Ok(_) => return Ok(false),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    }
    let nth = |n: usize| {
        let mut p = log.as_os_str().to_owned();
        p.push(format!(".{n}"));
        PathBuf::from(p)
    };
    let _ = std::fs::remove_file(nth(keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(nth(n), nth(n + 1));
    }
    std::fs::rename(log, nth(1))?;
    Ok(true)
}

/// 先写同目录的临时文件再改名：读的一方（另一个进程的 `list`）不会读到写了一半的 JSON。
pub fn write_json_atomic(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    std::fs::rename(&tmp, path)
}

/// 文件不在或解析不了都当没有：发现文件随时可能被实例进程删掉。
pub fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_log_is_rotated_keeping_a_few() {
        let d = tempfile::tempdir().unwrap();
        let log = d.path().join("host.log");
        std::fs::write(&log, "small").unwrap();
        assert!(!rotate_log(&log, 10, 2).unwrap(), "没超不动");
        for i in 0..4 {
            std::fs::write(&log, format!("big log number {i}")).unwrap();
            assert!(rotate_log(&log, 10, 2).unwrap());
        }
        assert!(!log.exists());
        assert_eq!(
            std::fs::read_to_string(d.path().join("host.log.1")).unwrap(),
            "big log number 3"
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("host.log.2")).unwrap(),
            "big log number 2"
        );
        assert!(!d.path().join("host.log.3").exists(), "至多留两份");
    }
}
