//! 项目状态的认领（architecture §8.9）：`state/conix/` 的项目锁、认领凭证，认不出的挪进
//! `quarantine/`。一个仓库可以把 `.conix/state/` 提交进去，克隆下来里面的作业状态、没结束的运行
//! 会被当成自己的接着跑，所以只认这台机器在 `known-projects.json` 里登记过的。

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use client::layout::{read_json, write_json_atomic};
use client::{Layout, ProjectState};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("project {project} is already served by instance {holder}")]
    Busy { project: String, holder: String },
    #[error("{} is a symlink or not a directory: project state is not kept through it", .0.display())]
    NotADirectory(PathBuf),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// 认领下来的项目状态。锁跟着它活，进程怎么退出内核都会放掉。
pub struct ProjectClaim {
    state: ProjectState,
    /// 认不出、刚挪进 `quarantine/` 的那一批在哪。
    quarantined: Option<PathBuf>,
    _lock: Flock<File>,
}

impl ProjectClaim {
    pub fn state(&self) -> &ProjectState {
        &self.state
    }

    pub fn quarantined(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Owner {
    token: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct KnownProject {
    token: String,
    path: String,
}

/// 先拿项目锁，再认领：锁在被认领的目录里，挪走认不出的东西时锁文件留下。
pub fn claim(
    layout: &Layout,
    state: ProjectState,
    instance: &str,
    now_ms: u64,
) -> Result<ProjectClaim, ProjectError> {
    let data = state.data_dir();
    for dir in [data.clone(), data.join("state"), state.dir()] {
        real_dir(&dir)?;
    }
    let lock = lock(&state, instance)?;
    ensure_ignored(&data);
    let quarantined = recognize(layout, &state, now_ms)?;
    Ok(ProjectClaim {
        state,
        quarantined,
        _lock: lock,
    })
}

/// 不跟随符号链接：指向别处的链接会让锁与认领都落到项目外面。
fn real_dir(dir: &Path) -> Result<(), ProjectError> {
    match fs::symlink_metadata(dir) {
        Ok(m) if m.is_dir() => Ok(()),
        Ok(_) => Err(ProjectError::NotADirectory(dir.into())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            match fs::create_dir(dir) {
                Err(e) if e.kind() != io::ErrorKind::AlreadyExists => return Err(e.into()),
                _ => {}
            }
            match fs::symlink_metadata(dir) {
                Ok(m) if m.is_dir() => Ok(()),
                _ => Err(ProjectError::NotADirectory(dir.into())),
            }
        }
        Err(e) => Err(e.into()),
    }
}

/// `.conix/.gitignore` 里要有 `state/`。引擎也维护这一行，但要等它第一次往项目里写本机数据；实例一起来
/// 就建了 `state/conix/`，中间那段 `git status` 会看到它。只补缺的这一行，别的不动；`.gitignore` 是
/// 符号链接时不动它，也不动它指向的文件（与引擎的做法一样）。
fn ensure_ignored(data_dir: &Path) {
    const LINE: &str = "state/";
    let file = OpenOptions::new()
        .read(true)
        .append(true)
        .create(true)
        .custom_flags(libc_nofollow())
        .open(data_dir.join(".gitignore"));
    let Ok(mut file) = file else { return };
    let mut had = String::new();
    if file.read_to_string(&mut had).is_err() || had.lines().any(|l| l.trim() == LINE) {
        return;
    }
    let sep = if had.is_empty() || had.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let _ = file.write_all(format!("{sep}{LINE}\n").as_bytes());
}

fn lock(state: &ProjectState, instance: &str) -> Result<Flock<File>, ProjectError> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .custom_flags(libc_nofollow())
        .open(state.lock())?;
    let mut lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
        Ok(l) => l,
        Err((mut file, e)) if e == Errno::EWOULDBLOCK => {
            let mut holder = String::new();
            let _ = file.read_to_string(&mut holder);
            return Err(ProjectError::Busy {
                project: state.project().display().to_string(),
                holder: holder.trim().to_string(),
            });
        }
        Err((_, e)) => return Err(io::Error::from(e).into()),
    };
    // 锁文件里记着是谁拿着：第二个实例据此说清楚该接入哪个。
    lock.set_len(0)?;
    lock.write_all(instance.as_bytes())?;
    Ok(lock)
}

fn libc_nofollow() -> i32 {
    nix::fcntl::OFlag::O_NOFOLLOW.bits()
}

/// 认得出就照用（项目搬了家就改写登记的路径）；认不出就把东西挪进 `quarantine/<时刻>/`，
/// 发一个新凭证。返回挪到了哪里。
fn recognize(
    layout: &Layout,
    state: &ProjectState,
    now_ms: u64,
) -> Result<Option<PathBuf>, ProjectError> {
    let _guard = known_lock(layout)?;
    let path = state.project().to_string_lossy().into_owned();
    let mut known: Vec<KnownProject> = read_json(&layout.known_projects()).unwrap_or_default();
    let owner: Option<Owner> = read_json(&state.owner());
    if let Some(o) = &owner
        && let Some(k) = known.iter_mut().find(|k| k.token == o.token)
    {
        if k.path != path {
            k.path = path;
            write_json_atomic(&layout.known_projects(), &known)?;
        }
        return Ok(None);
    }
    let quarantined = quarantine(state, now_ms)?;
    let token = uuid::Uuid::new_v4().simple().to_string();
    write_json_atomic(
        &state.owner(),
        &Owner {
            token: token.clone(),
        },
    )?;
    known.push(KnownProject { token, path });
    write_json_atomic(&layout.known_projects(), &known)?;
    Ok(quarantined)
}

fn known_lock(layout: &Layout) -> Result<Flock<File>, ProjectError> {
    fs::create_dir_all(layout.home())?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(layout.known_projects_lock())?;
    Flock::lock(file, FlockArg::LockExclusive).map_err(|(_, e)| io::Error::from(e).into())
}

/// 锁文件与以前的 `quarantine/` 不动，其余的挪走；什么都没有（第一次）就不建目录。
fn quarantine(state: &ProjectState, now_ms: u64) -> Result<Option<PathBuf>, ProjectError> {
    let dir = state.dir();
    // 锁文件不是状态：命令行（`conix flow install`）常在实例第一次起来之前就建了它的锁；挪走它，
    // 一个新项目头一回起实例就会报"不是这台机器写的"，正拿着它的命令行也锁不住了。
    let keep = [state.lock(), state.quarantine(), state.flows_cli_lock()];
    let entries: Vec<PathBuf> = fs::read_dir(&dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| !keep.contains(p))
        .collect();
    if entries.is_empty() {
        return Ok(None);
    }
    let to = state.quarantine().join(now_ms.to_string());
    fs::create_dir_all(&to)?;
    for p in entries {
        if let Some(name) = p.file_name() {
            fs::rename(&p, to.join(name))?;
        }
    }
    Ok(Some(to))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, Layout, ProjectState) {
        let t = tempfile::tempdir().unwrap();
        let layout = Layout::new(t.path().join("home"));
        let project = t.path().join("proj");
        fs::create_dir_all(&project).unwrap();
        (t, layout, ProjectState::new(project))
    }

    #[test]
    fn first_claim_registers_and_later_claims_are_recognized() {
        let (_t, layout, state) = setup();
        let c = claim(&layout, state.clone(), "a", 1).unwrap();
        assert!(c.quarantined().is_none());
        fs::write(state.sessions(), "{}").unwrap();
        drop(c);
        let c = claim(&layout, state.clone(), "b", 2).unwrap();
        assert!(c.quarantined().is_none());
        assert!(state.sessions().exists());
    }

    #[test]
    fn a_second_instance_is_refused_and_told_who_holds_it() {
        let (_t, layout, state) = setup();
        let _c = claim(&layout, state.clone(), "first", 1).unwrap();
        match claim(&layout, state, "second", 2) {
            Err(ProjectError::Busy { holder, .. }) => assert_eq!(holder, "first"),
            other => panic!("{:?}", other.err()),
        }
    }

    /// 提交进仓库的状态：凭证没登记过，整个挪走，从空状态开始。
    #[test]
    fn state_this_machine_did_not_write_is_quarantined() {
        let (_t, layout, state) = setup();
        fs::create_dir_all(state.flow_runs()).unwrap();
        fs::write(state.jobs_state(), "{}").unwrap();
        fs::write(state.owner(), r#"{"token":"planted"}"#).unwrap();
        let c = claim(&layout, state.clone(), "a", 7).unwrap();
        let q = c.quarantined().unwrap().to_path_buf();
        assert_eq!(q, state.quarantine().join("7"));
        assert!(q.join("jobs-state.json").exists() && q.join("flow-runs").is_dir());
        assert!(!state.jobs_state().exists() && !state.flow_runs().exists());
        let owner: Owner = read_json(&state.owner()).unwrap();
        assert_ne!(owner.token, "planted");
    }

    /// 先 `conix flow install` 再起实例：命令行的锁文件留着，不算认不出的状态。
    #[test]
    fn a_lock_the_command_line_took_first_is_not_quarantined() {
        let (_t, layout, state) = setup();
        fs::create_dir_all(state.dir()).unwrap();
        fs::write(state.flows_cli_lock(), "").unwrap();
        let c = claim(&layout, state.clone(), "a", 7).unwrap();
        assert!(c.quarantined().is_none());
        assert!(state.flows_cli_lock().exists());
    }

    #[test]
    fn a_moved_project_is_still_recognized() {
        let (t, layout, state) = setup();
        drop(claim(&layout, state.clone(), "a", 1).unwrap());
        fs::write(state.sessions(), "{}").unwrap();
        let moved = t.path().join("moved");
        fs::rename(state.project(), &moved).unwrap();
        let state = ProjectState::new(&moved);
        let c = claim(&layout, state.clone(), "a", 2).unwrap();
        assert!(c.quarantined().is_none());
        let known: Vec<KnownProject> = read_json(&layout.known_projects()).unwrap();
        assert_eq!(known.len(), 1);
        assert_eq!(known[0].path, moved.to_string_lossy());
    }

    #[test]
    fn a_symlinked_state_directory_is_refused() {
        let (t, layout, state) = setup();
        let elsewhere = t.path().join("elsewhere");
        fs::create_dir_all(&elsewhere).unwrap();
        fs::create_dir_all(state.data_dir()).unwrap();
        std::os::unix::fs::symlink(&elsewhere, state.data_dir().join("state")).unwrap();
        assert!(matches!(
            claim(&layout, state, "a", 1),
            Err(ProjectError::NotADirectory(_))
        ));
        assert_eq!(fs::read_dir(&elsewhere).unwrap().count(), 0);
    }

    #[test]
    fn the_state_directory_is_kept_out_of_git_once() {
        let (_t, layout, state) = setup();
        fs::create_dir_all(state.data_dir()).unwrap();
        fs::write(state.data_dir().join(".gitignore"), "settings.local.json").unwrap();
        drop(claim(&layout, state.clone(), "a", 1).unwrap());
        drop(claim(&layout, state.clone(), "a", 2).unwrap());
        assert_eq!(
            fs::read_to_string(state.data_dir().join(".gitignore")).unwrap(),
            "settings.local.json\nstate/\n"
        );
    }

    #[test]
    fn a_symlinked_gitignore_is_left_alone() {
        let (t, layout, state) = setup();
        let target = t.path().join("bashrc");
        fs::write(&target, "x").unwrap();
        fs::create_dir_all(state.data_dir()).unwrap();
        std::os::unix::fs::symlink(&target, state.data_dir().join(".gitignore")).unwrap();
        drop(claim(&layout, state, "a", 1).unwrap());
        assert_eq!(fs::read_to_string(&target).unwrap(), "x");
    }
}
