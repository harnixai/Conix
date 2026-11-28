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

    /// 命令行（`conix flow install` 这类）写项目的流程包时拿的 flock。
    pub fn flows_cli_lock(&self) -> PathBuf {
        self.dir().join("flows-cli.lock")
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

    /// 一次运行的过程目录（流程库的 `with_runs_dir`，与 `flows::RUNS_DIR` 同一处）：Conix 的
    /// `conix-report.json` 也写在这里，运行的东西都在一个目录里。
    pub fn run_dir(&self, run: &str) -> PathBuf {
        self.dir().join("runs").join(run)
    }

    /// 作业触发的运行时目录的根：`job-runs/<作业>/<触发时刻>/`。
    pub fn job_runs(&self) -> PathBuf {
        self.dir().join("job-runs")
    }

    pub fn job_run_dir(&self, job: &str, fired_ms: u64) -> PathBuf {
        self.job_runs().join(job).join(fired_ms.to_string())
    }

    pub fn flow_workspaces(&self) -> PathBuf {
        self.dir().join("workspaces")
    }

    /// 人用 `/trust` 定过这个项目信不信：Agix 只记"信任"（取消就是删掉那一条），分不出"没人定过"和
    /// "人取消过"，个人项目第一次用就自动信任要靠这一份认出后者。
    pub fn trust_choice(&self) -> PathBuf {
        self.dir().join("trust.json")
    }

    /// 作业起的运行在会话上多出的限制（网络、禁 `CronCreate`），按运行记。
    pub fn run_policies(&self) -> PathBuf {
        self.dir().join("run-policies.json")
    }
}

/// 用户的家目录：unix 是 `HOME`，Windows 是 `USERPROFILE`。
pub fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
}

/// 用户的"文档"目录：macOS、Windows 是家目录下的 `Documents`；Linux 按 XDG（环境变量
/// `XDG_DOCUMENTS_DIR`，再看 `~/.config/user-dirs.dirs`），都没有就是 `~/Documents`。
/// Windows 的文档目录可以被重定向（OneDrive），这里不读注册表，重定向了就在 `conix.yaml` 里写明。
pub fn documents_dir() -> Option<PathBuf> {
    let home = user_home()?;
    if cfg!(target_os = "linux") {
        let xdg = std::env::var("XDG_DOCUMENTS_DIR").ok().or_else(|| {
            let cfg = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".config"));
            let text = std::fs::read_to_string(cfg.join("user-dirs.dirs")).ok()?;
            text.lines().find_map(|l| {
                let v = l.trim().strip_prefix("XDG_DOCUMENTS_DIR=")?;
                Some(v.trim_matches('"').to_string())
            })
        });
        if let Some(d) = xdg.filter(|d| !d.is_empty()) {
            let d = match d.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(d),
            };
            // user-dirs.dirs 里写成 `$HOME/` 表示"没有单独的文档目录"：那就用缺省的。
            if d != home {
                return Some(d);
            }
        }
    }
    Some(home.join("Documents"))
}

/// 个人项目（architecture §8.1）：`configured`（`conix.yaml` 的 `instance.personal_project`，`~` 开头按家目录
/// 展开），没写就是 `<文档目录>/Conix`。
pub fn personal_project(configured: Option<&str>) -> PathBuf {
    let home = user_home().unwrap_or_default();
    match configured.map(str::trim).filter(|c| !c.is_empty()) {
        Some("~") => home,
        Some(c) => match c.strip_prefix("~/").or_else(|| c.strip_prefix("~\\")) {
            Some(rest) => home.join(rest),
            None => PathBuf::from(c),
        },
        None => documents_dir().unwrap_or(home).join("Conix"),
    }
}

/// 这个目录能不能当项目。不能的：家目录与它的上级（含根目录）——项目层目录 `<项目>/.conix` 落在家目录就是
/// 全局目录 `~/.conix`，设置分层撞在一起，引擎还把那里当别的实例的状态目录守着；全局目录本身与它下面。
/// 两边都按已解析的真实路径比，免得一个符号链接绕过去。
pub fn unfit_project(dir: &Path, home: Option<&Path>, conix_home: &Path) -> bool {
    let real = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    let dir = real(dir);
    let conix = real(conix_home);
    if dir.starts_with(&conix) || dir.join(PROJECT_DIR) == conix {
        return true;
    }
    match home {
        Some(h) => real(h).starts_with(&dir),
        None => dir.parent().is_none(),
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
    fn the_home_directory_its_parents_and_the_global_directory_are_no_projects() {
        let d = tempfile::tempdir().unwrap();
        let home = d.path().join("me");
        let conix = home.join(".conix");
        std::fs::create_dir_all(conix.join("instances")).unwrap();
        std::fs::create_dir_all(home.join("code/app")).unwrap();
        let unfit = |p: &Path| unfit_project(p, Some(&home), &conix);
        assert!(unfit(&home), "家目录的 .conix 就是全局目录");
        assert!(unfit(d.path()), "家目录的上级");
        assert!(unfit(Path::new("/")));
        assert!(unfit(&conix) && unfit(&conix.join("instances")));
        assert!(!unfit(&home.join("code/app")));
        // 全局目录换到别处（CONIX_HOME），它的上一级也不行：那里的 .conix 就是它。
        let elsewhere = d.path().join("data/.conix");
        std::fs::create_dir_all(&elsewhere).unwrap();
        assert!(unfit_project(
            &d.path().join("data"),
            Some(&home),
            &elsewhere
        ));
    }

    #[test]
    fn the_personal_project_is_conix_in_the_documents_or_as_configured() {
        let home = user_home().unwrap();
        assert_eq!(
            personal_project(None),
            documents_dir().unwrap().join("Conix")
        );
        assert_eq!(personal_project(Some("~/notes")), home.join("notes"));
        assert_eq!(personal_project(Some("/srv/p")), PathBuf::from("/srv/p"));
        assert_eq!(personal_project(Some("  ")), personal_project(None));
    }

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
