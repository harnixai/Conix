//! LIVE 的快照与续跑（testing.md §8.2）：长的用例在人要动手的地方停掉实例、把整个世界（CONIX_HOME 与项目，连同
//! worktree）打成一个包；改了流程包或 Conix 以后，从失败前的那一处接着跑，不用每次从头。
//!
//! - 世界放在固定的短路径 `/tmp/cxw-<用例>/`：worktree、项目登记、运行记录里都是绝对路径，换个地方就断；socket 的路径
//!   有 104 字节的上限，所以不放 `target/` 下。
//! - 快照在 `target/tmp/live-snap/<用例>/<序号>-<名字>.tar`，旁边一份 `.json`：Conix 的提交、流程包的摘要、
//!   用例记下的变量（运行的 id 这类）、到这一处的用时与统计。
//! - `LIVE_FROM=<用例>/<序号>-<名字>`（`scripts/live.sh snaps` 列出有哪些）：解开到同一个路径，用例给的 `refresh` 把项目里的
//!   流程包换成现在的，实例带 `CONIX_DEV_REPIN=1` 起来，把没结束的运行换钉成包现在的样子（flows.md §7.15），驱动接着走。
//!   驱动本来就是看运行记录行事的，不用知道自己走到了哪一步。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde_json::{Value, json};

use super::{Dir, Live, Opts};

/// 快照放在哪：cargo 给集成测试的临时目录。
fn snaps_root() -> PathBuf {
    Path::new(env!("CARGO_TARGET_TMPDIR")).join("live-snap")
}

fn world_of(case: &str) -> PathBuf {
    PathBuf::from(format!("/tmp/cxw-{case}"))
}

fn sh(dir: &Path, cmd: &str) -> String {
    let o = std::process::Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "{cmd}: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    String::from_utf8_lossy(&o.stdout).into_owned()
}

/// 这份仓库现在是哪个提交（带不带没提交的改动）：续跑时说清快照之后改了什么。
fn conix_commit() -> String {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let head = sh(&repo, "git rev-parse --short HEAD").trim().to_string();
    let dirty = !sh(&repo, "git status --porcelain -- crates examples")
        .trim()
        .is_empty();
    if dirty { format!("{head}+") } else { head }
}

/// 一个目录里每个文件的摘要（相对路径 → sha256），看流程包在快照之后改了哪些。
fn digests(dir: &Path) -> BTreeMap<String, String> {
    if !dir.is_dir() {
        return BTreeMap::new();
    }
    sh(dir, "find . -type f | LC_ALL=C sort | xargs shasum -a 256")
        .lines()
        .filter_map(|l| {
            let (sum, path) = l.split_once("  ")?;
            Some((path.trim_start_matches("./").to_string(), sum.to_string()))
        })
        .collect()
}

/// 一个用例的快照：拍到哪了、从哪续的。
pub struct Snaps {
    case: String,
    /// 流程包的源头（拿来比快照之后改了什么）。
    bundle: PathBuf,
    begun: Instant,
    taken: Vec<String>,
    /// 用例记下的东西：续跑时原样回来。
    pub vars: serde_json::Map<String, Value>,
    /// 从哪一处续的；`None` 是从头跑。
    pub resumed: Option<String>,
}

impl Snaps {
    /// `case` 短（进 `/tmp/cxw-<case>`）；`bundle` 是流程包的源头目录。
    pub fn new(case: &str, bundle: &Path) -> Snaps {
        Snaps {
            case: case.into(),
            bundle: bundle.into(),
            begun: Instant::now(),
            taken: vec![],
            vars: Default::default(),
            resumed: None,
        }
    }

    fn dir(&self) -> PathBuf {
        snaps_root().join(&self.case)
    }

    /// 这个用例的 `Live`：`LIVE_FROM` 指着它的一份快照就解开它（`refresh` 把项目里的流程包换成现在的）、实例带换钉起来；
    /// 否则在固定的目录里从头建。
    pub fn live(&mut self, mut opts: Opts, refresh: impl FnOnce(&Live)) -> Live {
        let world = world_of(&self.case);
        let from = std::env::var("LIVE_FROM").ok().and_then(|f| {
            let (case, name) = f.split_once('/')?;
            (case == self.case).then(|| name.to_string())
        });
        let Some(name) = from else {
            opts.world = Some(world);
            return Live::with(opts);
        };
        let tar = self.dir().join(format!("{name}.tar"));
        assert!(tar.is_file(), "没有快照 {}", tar.display());
        let meta: Value =
            serde_json::from_str(&std::fs::read_to_string(tar.with_extension("json")).unwrap())
                .unwrap();
        // 上一回留下的实例（用例中途被打断）先停掉，再换世界。
        let _ = std::process::Command::new(super::conix())
            .args(["stop", "lv"])
            .env("CONIX_HOME", world.join("home"))
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .output();
        let _ = std::fs::remove_dir_all(&world);
        std::fs::create_dir_all(&world).unwrap();
        sh(&world, &format!("tar -xf {}", tar.display()));
        opts.env.push(("CONIX_DEV_REPIN".into(), "1".into()));
        let lv = Live::over(
            Dir::Fixed(world.join("home")),
            Dir::Fixed(world.join("project")),
            &opts,
        );
        // 快照之后改了什么：Conix 的提交、流程包的文件。
        let was: BTreeMap<String, String> =
            serde_json::from_value(meta["bundle"].clone()).unwrap_or_default();
        let now = digests(&self.bundle);
        let changed: Vec<&String> = now
            .keys()
            .chain(was.keys())
            .filter(|k| now.get(*k) != was.get(*k))
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect();
        eprintln!(
            "==== 从快照 {}/{name} 续跑：拍时 Conix {}，现在 {}；流程包改了 {} 个文件{}",
            self.case,
            meta["conix"].as_str().unwrap_or("?"),
            conix_commit(),
            changed.len(),
            if changed.is_empty() {
                String::new()
            } else {
                format!("：{changed:?}")
            }
        );
        refresh(&lv);
        self.vars = meta["vars"].as_object().cloned().unwrap_or_default();
        self.taken = meta["taken"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|t| t.as_str().map(String::from))
            .collect();
        self.resumed = Some(name);
        lv
    }

    /// 这一处拍过没有（续跑时，快照之前的那些也算拍过）。
    pub fn has(&self, name: &str) -> bool {
        self.taken.iter().any(|t| t == name)
    }

    /// 拍一张：实例要已经停了（端口的用例用 [`Snaps::point`]，真终端的退出 `conix` 再调它）。回快照的名字。
    pub fn take(&mut self, lv: &Live, name: &str, stats: Value) -> String {
        self.taken.push(name.to_string());
        let file = format!("{:02}-{name}", self.taken.len());
        let dir = self.dir();
        std::fs::create_dir_all(&dir).unwrap();
        let tar = dir.join(format!("{file}.tar"));
        let world = world_of(&self.case);
        assert_eq!(
            lv.home.path(),
            world.join("home"),
            "拍快照的用例要用 Snaps::live 建的 Live"
        );
        // 实例的 socket 与锁不进包：解开后由 `conix start` 重建。
        sh(
            &world,
            &format!(
                "tar --exclude '*.sock' --exclude '*.lock' -cf {} home project",
                tar.display()
            ),
        );
        let meta = json!({
            "case": self.case,
            "name": name,
            "conix": conix_commit(),
            "bundle": digests(&self.bundle),
            "vars": self.vars,
            "taken": self.taken,
            "elapsed_s": self.begun.elapsed().as_secs(),
            "stats": stats,
        });
        std::fs::write(
            tar.with_extension("json"),
            serde_json::to_string_pretty(&meta).unwrap(),
        )
        .unwrap();
        eprintln!(
            "==== 快照 {}/{file}（{}s）：LIVE_FROM={}/{file} 从这里续",
            self.case,
            self.begun.elapsed().as_secs(),
            self.case
        );
        file
    }

    /// 这一处要不要拍：没拍过，也没有 `LIVE_SNAPS=0`（只想一口气跑完、不停实例时）。
    pub fn wants(&self, name: &str) -> bool {
        !self.has(name) && std::env::var("LIVE_SNAPS").as_deref() != Ok("0")
    }

    /// 实例重启过（拍过快照，或从快照续的）：实例内存里的东西（子 Agent 的任务这类）只剩重启之后的。
    pub fn restarted(&self) -> bool {
        !self.taken.is_empty() || self.resumed.is_some()
    }

    /// 端口的用例：停实例、拍、再起，换成新的接入。拍过的不再拍；`stats` 只在真拍时算。回拍了没有。
    pub async fn point(
        &mut self,
        lv: &Live,
        c: &mut super::Client,
        name: &str,
        stats: impl FnOnce() -> Value,
    ) -> bool {
        if !self.wants(name) {
            return false;
        }
        // 站等人时会话的这一轮可能还没收尾（交付在轮中，`turn_complete` 带着这一轮的用量随后才到）：等所有会话都
        // 不在跑了再停实例，不然这一轮被掐断、用量也没记上。
        let end = Instant::now() + std::time::Duration::from_secs(180);
        let mut quiet = 0;
        while quiet < 3 && Instant::now() < end {
            let busy = c
                .sessions()
                .await
                .iter()
                .any(|s| s.state == port::SessionState::Running);
            quiet = if busy { 0 } else { quiet + 1 };
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        let stats = stats();
        let answering = c.answering_now();
        lv.stop().await;
        self.take(lv, name, stats);
        *c = lv.start_again().await;
        c.answering(answering);
        true
    }
}
