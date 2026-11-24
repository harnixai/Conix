use serde::{Deserialize, Serialize};

use crate::{InstanceName, Millis};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InstanceInfo {
    pub name: InstanceName,
    pub host: String,
    #[serde(default)]
    pub pid: Option<u32>,
    #[serde(default)]
    pub project: Option<String>,
    pub state: InstanceState,
    #[serde(default)]
    pub started_ms: Option<Millis>,
    #[serde(default)]
    pub stopped_ms: Option<Millis>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub sessions: u32,
    #[serde(default)]
    pub waiting: u32,
    #[serde(default)]
    pub jobs: u32,
    #[serde(default)]
    pub holder: Option<ClientInfo>,
    /// 这次启动以来看护进程重启过它几次（architecture §8.7）。
    #[serde(default)]
    pub restarts: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstanceState {
    Running,
    Stopped,
    /// 进程已不在或 socket 连不上；运行与作业的状态还在，可以 `start`。
    Stale,
}

/// 接入实例的那一个 TUI。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClientInfo {
    pub user: String,
    pub pid: u32,
    #[serde(default)]
    pub tty: Option<String>,
    /// 取自 `SSH_CONNECTION` 的来源地址。
    #[serde(default)]
    pub ssh_from: Option<String>,
    pub attached_ms: Millis,
    pub last_input_ms: Millis,
}

impl ClientInfo {
    /// `me@pts/3 (ssh 10.0.0.7)` 这种给人看的写法。
    pub fn label(&self) -> String {
        let mut s = self.user.clone();
        if let Some(tty) = &self.tty {
            s.push('@');
            s.push_str(tty.trim_start_matches("/dev/"));
        }
        if let Some(from) = &self.ssh_from {
            s.push_str(" (ssh ");
            s.push_str(from);
            s.push(')');
        }
        s
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Attach {
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewInstance {
    #[serde(default)]
    pub name: Option<InstanceName>,
    #[serde(default)]
    pub project: Option<String>,
}
