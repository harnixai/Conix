//! 全局的流程库（docs/design/flows.md §7.12）：`~/.conix/flows/<包>/`（`CONIX_HOME` 指定时以它为准），
//! 旁边 `.registry.yaml` 记每个包从哪来、什么版本、摘要，内置的再记它是从哪一版内置的解出来的（`shipped`）。
//! 库里的包不参与哪个项目的运行，只是 `conix flow install` 的来源。
//!
//! 几个实例、几条命令行可能同时动它：写之前拿 `.lock`（flock），写的东西先进 `.staging/` 整包校验，
//! 过了才改名换上，换下来的进 `.backups/`。

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use harnix_flow::Catalog;
use harnix_flow::bundle;
use nix::fcntl::{Flock, FlockArg};
use serde::{Deserialize, Serialize};

use super::builtin;
use super::package::{self, Files};

/// 全局目录下流程库的目录名。
pub const DIR: &str = "flows";
pub const REGISTRY: &str = ".registry.yaml";
pub const BACKUPS: &str = ".backups";
pub const STAGING: &str = ".staging";
pub const LOCK: &str = ".lock";

/// 库里一个包是怎么来的：最后一次写它的是谁。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Builtin,
    Import,
    Backfill,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Source::Builtin => "builtin",
            Source::Import => "import",
            Source::Backfill => "backfill",
        }
    }
}

/// 一版内置包：库里那份是从它解出来的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shipped {
    pub version: String,
    pub digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub source: Source,
    pub version: String,
    pub digest: String,
    /// 内置包才有：库里这份的底子是哪一版内置的。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shipped: Option<Shipped>,
    #[serde(default)]
    pub at: String,
    /// 内置包被 `remove --global` 删了：起来时不再解开，`reset` 才回来。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub removed: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub bundles: BTreeMap<String, Entry>,
}

/// 内置包在库里的状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinState {
    /// 与它解出来的那一版一样。
    Pristine,
    /// 改过（回填、导入或手改）。
    Modified,
    /// `remove --global` 删了。
    Removed,
    /// 目录没了、也没删过：起来时会重新解开。
    Missing,
}

/// 起来时对内置包做了什么：一行提示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootNote {
    Extracted {
        name: String,
        version: String,
    },
    Upgraded {
        name: String,
        from: String,
        to: String,
    },
    Failed {
        name: String,
        error: String,
    },
}

pub fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// `x.y.z` 当数比；读不出的当 0.0.0。
pub fn version_key(v: &str) -> (u64, u64, u64) {
    bundle::parse_version(v).unwrap_or((0, 0, 0))
}

/// 写到一半的锁文件句柄：放掉它就放了锁。
pub struct Locked {
    _lock: Flock<File>,
}

/// 拿 `path` 上的排他 flock（等到拿到为止）：进程怎么退出内核都会放掉。
pub fn lock_file(path: &Path) -> Result<Locked, String> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Flock::lock(f, FlockArg::LockExclusive)
        .map(|l| Locked { _lock: l })
        .map_err(|(_, e)| format!("{}: {e}", path.display()))
}

/// 先写到旁边再改名：读的一方看到的不是旧的就是新的。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name().and_then(|n| n.to_str()).unwrap_or("file")
    ));
    std::fs::write(&tmp, bytes).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    /// `home` 是全局目录（`~/.conix` 或 `CONIX_HOME`）。
    pub fn new(home: &Path) -> Library {
        Library {
            root: home.join(DIR),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn path_of(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// 库里这个包的目录（有 `bundle.yaml` 才算）。
    pub fn dir(&self, name: &str) -> Option<PathBuf> {
        bundle::bundle_in(&self.root, name)
    }

    pub fn backups(&self) -> PathBuf {
        self.root.join(BACKUPS)
    }

    pub fn staging(&self) -> PathBuf {
        self.root.join(STAGING)
    }

    /// 库里有目录的包（带 `bundle.yaml` 的子目录），按名字。
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = std::fs::read_dir(&self.root)
            .map(|rd| {
                rd.flatten()
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| self.dir(n).is_some())
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        out
    }

    pub fn lock(&self) -> Result<Locked, String> {
        lock_file(&self.root.join(LOCK))
    }

    pub fn registry(&self) -> Result<Registry, String> {
        let p = self.root.join(REGISTRY);
        match std::fs::read_to_string(&p) {
            Ok(t) => serde_yaml::from_str(&t).map_err(|e| format!("{}: {e}", p.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Registry::default()),
            Err(e) => Err(format!("{}: {e}", p.display())),
        }
    }

    pub fn save(&self, reg: &Registry) -> Result<(), String> {
        let text = serde_yaml::to_string(reg).map_err(|e| e.to_string())?;
        write_atomic(
            &self.root.join(REGISTRY),
            format!("# {}\n{text}", i18n::t!("flowlib.registry_header")).as_bytes(),
        )
    }

    /// 内置包在库里的状态；不是内置的（registry 里没有 `shipped`、这个 Conix 也不带）是 `None`。
    pub fn builtin_state(&self, name: &str, reg: &Registry) -> Option<BuiltinState> {
        let e = reg.bundles.get(name);
        let shipped = e.and_then(|e| e.shipped.clone());
        if shipped.is_none() && builtin::package(name).is_none() {
            return None;
        }
        if e.is_some_and(|e| e.removed) {
            return Some(BuiltinState::Removed);
        }
        let Some(dir) = self.dir(name) else {
            return Some(BuiltinState::Missing);
        };
        let actual = package::dir_digest(&dir).ok();
        Some(match (shipped, actual) {
            (Some(s), Some(a)) if s.digest == a => BuiltinState::Pristine,
            _ => BuiltinState::Modified,
        })
    }

    /// 把一份内容写成库里的 `name`（调用方拿着锁）：暂存、校验、换上（旧的进备份）、记进 registry。
    /// `shipped` 为 `None` 时沿用原来那条的。回旧的备份。
    pub fn write(
        &self,
        reg: &mut Registry,
        name: &str,
        files: &Files,
        source: Source,
        shipped: Option<Shipped>,
        catalog: Catalog,
    ) -> Result<Option<PathBuf>, String> {
        let staged = package::stage(name, files, None, Some(&self.staging()), catalog)?;
        let version = staged.version();
        let digest = staged.digest.clone();
        let backup = package::place(staged, &self.path_of(name), &self.backups(), false)?;
        let shipped = shipped.or_else(|| reg.bundles.get(name).and_then(|e| e.shipped.clone()));
        reg.bundles.insert(
            name.to_string(),
            Entry {
                source,
                version,
                digest,
                shipped,
                at: now(),
                removed: false,
            },
        );
        self.save(reg)?;
        Ok(backup)
    }

    /// 从库里删掉（调用方拿着锁）：目录进备份；内置包留一条 `removed`，起来时不再解开。
    pub fn remove(&self, reg: &mut Registry, name: &str) -> Result<Option<PathBuf>, String> {
        let backup = package::back_up(&self.path_of(name), &self.backups(), name)?;
        let builtin = reg.bundles.get(name).is_some_and(|e| e.shipped.is_some())
            || builtin::package(name).is_some();
        if builtin {
            let e = reg
                .bundles
                .entry(name.to_string())
                .or_insert_with(|| Entry {
                    source: Source::Builtin,
                    version: String::new(),
                    digest: String::new(),
                    shipped: None,
                    at: String::new(),
                    removed: true,
                });
            e.removed = true;
            e.at = now();
        } else {
            reg.bundles.remove(name);
        }
        self.save(reg)?;
        Ok(backup)
    }

    /// 把内置包恢复成这个 Conix 带的那版（调用方拿着锁）。
    pub fn reset(&self, reg: &mut Registry, name: &str) -> Result<Option<PathBuf>, String> {
        let p =
            builtin::package(name).ok_or_else(|| i18n::t!("flowlib.not_builtin", name = name))?;
        let shipped = Shipped {
            version: p.version.clone(),
            digest: p.digest.clone(),
        };
        self.write(
            reg,
            name,
            &p.files,
            Source::Builtin,
            Some(shipped),
            package::default_catalog(),
        )
    }

    /// 起来时查一次内置包：库里没有（也没删过）就解开；没改过、而这个 Conix 带的不同且不更旧就换成它；
    /// 改过的不动（`doctor` 报有新的内置版）。
    pub fn bootstrap(&self) -> Vec<BootNote> {
        let packages = builtin::packages();
        // 先不拿锁看一眼：多数时候什么都不用做，不该让每条命令都排队。
        if let Ok(reg) = self.registry()
            && packages.iter().all(|p| !self.needs_boot(&reg, p))
        {
            return vec![];
        }
        let _lock = match self.lock() {
            Ok(l) => l,
            Err(error) => {
                return packages
                    .into_iter()
                    .map(|p| BootNote::Failed {
                        name: p.name,
                        error: error.clone(),
                    })
                    .collect();
            }
        };
        let mut reg = match self.registry() {
            Ok(r) => r,
            Err(error) => {
                return vec![BootNote::Failed {
                    name: REGISTRY.into(),
                    error,
                }];
            }
        };
        let mut out = vec![];
        for p in packages {
            if !self.needs_boot(&reg, &p) {
                continue;
            }
            let from = reg
                .bundles
                .get(&p.name)
                .filter(|_| self.dir(&p.name).is_some())
                .and_then(|e| e.shipped.as_ref().map(|s| s.version.clone()));
            let shipped = Shipped {
                version: p.version.clone(),
                digest: p.digest.clone(),
            };
            match self.write(
                &mut reg,
                &p.name,
                &p.files,
                Source::Builtin,
                Some(shipped),
                package::default_catalog(),
            ) {
                Ok(_) => out.push(match from {
                    Some(from) => BootNote::Upgraded {
                        name: p.name,
                        from,
                        to: p.version,
                    },
                    None => BootNote::Extracted {
                        name: p.name,
                        version: p.version,
                    },
                }),
                Err(error) => out.push(BootNote::Failed {
                    name: p.name,
                    error,
                }),
            }
        }
        out
    }

    fn needs_boot(&self, reg: &Registry, p: &builtin::Builtin) -> bool {
        let Some(e) = reg.bundles.get(&p.name) else {
            // 库里有个同名目录、registry 里却没有：是谁放的不知道，不盖它（doctor 报）。
            return self.dir(&p.name).is_none();
        };
        if e.removed {
            return false;
        }
        match self.builtin_state(&p.name, reg) {
            Some(BuiltinState::Missing) => true,
            Some(BuiltinState::Pristine) => e.shipped.as_ref().is_some_and(|s| {
                s.digest != p.digest && version_key(&p.version) >= version_key(&s.version)
            }),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_extracts_then_leaves_alone_and_a_removed_builtin_stays_removed() {
        let home = tempfile::tempdir().unwrap();
        let lib = Library::new(home.path());
        let notes = lib.bootstrap();
        let extracted: Vec<&str> = notes
            .iter()
            .filter_map(|n| match n {
                BootNote::Extracted { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(extracted, builtin::bundles(), "{notes:?}");
        assert_eq!(notes.len(), extracted.len(), "{notes:?}");
        assert!(lib.dir("xforge").is_some());
        let reg = lib.registry().unwrap();
        let e = &reg.bundles["xforge"];
        assert_eq!(e.source, Source::Builtin);
        assert_eq!(e.shipped.as_ref().unwrap().digest, e.digest);
        assert_eq!(
            lib.builtin_state("xforge", &reg),
            Some(BuiltinState::Pristine)
        );
        assert!(lib.bootstrap().is_empty(), "nothing to do the second time");

        let mut reg = lib.registry().unwrap();
        let _l = lib.lock().unwrap();
        lib.remove(&mut reg, "xforge").unwrap();
        drop(_l);
        assert!(lib.dir("xforge").is_none());
        assert!(
            lib.bootstrap().is_empty(),
            "a removed built-in is not brought back"
        );
        assert_eq!(
            lib.builtin_state("xforge", &lib.registry().unwrap()),
            Some(BuiltinState::Removed)
        );
        let mut reg = lib.registry().unwrap();
        lib.reset(&mut reg, "xforge").unwrap();
        assert!(lib.dir("xforge").is_some());
        assert!(!lib.registry().unwrap().bundles["xforge"].removed);
    }

    #[test]
    fn a_pristine_older_builtin_is_upgraded_and_a_modified_one_is_kept() {
        let home = tempfile::tempdir().unwrap();
        let lib = Library::new(home.path());
        lib.bootstrap();
        // 装成"上一版内置的、没改过"：registry 说底子是 0.0.1，目录就是那一版。
        let mut reg = lib.registry().unwrap();
        let dir = lib.dir("xforge").unwrap();
        let yaml = dir.join("bundle.yaml");
        let text = std::fs::read_to_string(&yaml).unwrap();
        let old = regex_version(&text, "0.0.1");
        std::fs::write(&yaml, &old).unwrap();
        let digest = package::dir_digest(&dir).unwrap();
        let e = reg.bundles.get_mut("xforge").unwrap();
        e.version = "0.0.1".into();
        e.digest = digest.clone();
        e.shipped = Some(Shipped {
            version: "0.0.1".into(),
            digest,
        });
        lib.save(&reg).unwrap();
        let notes = lib.bootstrap();
        assert!(
            matches!(&notes[..], [BootNote::Upgraded { from, .. }] if from == "0.0.1"),
            "{notes:?}"
        );
        assert_eq!(std::fs::read_to_string(&yaml).unwrap(), text);

        // 改过：不动。
        std::fs::write(dir.join("extra.md"), "mine").unwrap();
        let mut reg = lib.registry().unwrap();
        reg.bundles.get_mut("xforge").unwrap().shipped = Some(Shipped {
            version: "0.0.1".into(),
            digest: "old".into(),
        });
        lib.save(&reg).unwrap();
        assert!(lib.bootstrap().is_empty());
        assert!(dir.join("extra.md").is_file());
        assert_eq!(
            lib.builtin_state("xforge", &lib.registry().unwrap()),
            Some(BuiltinState::Modified)
        );
    }

    fn regex_version(text: &str, to: &str) -> String {
        text.lines()
            .map(|l| {
                if l.starts_with("version:") {
                    format!("version: {to}")
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
            + "\n"
    }
}
