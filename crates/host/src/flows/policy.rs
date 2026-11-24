//! 作业起的运行在会话上多出的限制（网络、禁 `CronCreate`）：运行开始时按作业当时的护栏定下，记在
//! `state/conix/run-policies.json`，这次运行的每个站会话都照它开。一次运行一行，不清理：结束的运行
//! 还能重做（rework 会再开站会话），而行数与运行数同量级。
//!
//! 不在开站时现查作业：引擎按 `session_id` 认会话，同一个键再开时 `options` 与记下的不同就拒绝。运行跨
//! 重启续跑时作业的护栏可能已经改了，现查就会让续跑的站开不出来；所以按运行定一次，之后不变。

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

use port::Network;
use serde::{Deserialize, Serialize};

use crate::sync::Locked;

/// 一次运行的会话限制。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunPolicy {
    /// 发起它的作业（只为看得懂，不参与判断）。
    pub job: String,
    pub network: Network,
}

pub struct RunPolicies {
    path: PathBuf,
    map: Mutex<BTreeMap<String, RunPolicy>>,
}

impl RunPolicies {
    /// 读不出来按空的算：丢了限制的运行退回到普通运行的会话（仍是有人值守、按项目设置），不会更宽。
    pub fn open(path: PathBuf) -> RunPolicies {
        let map = std::fs::read(&path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default();
        RunPolicies {
            path,
            map: Mutex::new(map),
        }
    }

    pub fn in_memory() -> RunPolicies {
        RunPolicies {
            path: PathBuf::new(),
            map: Mutex::default(),
        }
    }

    pub fn get(&self, run: &str) -> Option<RunPolicy> {
        self.map.locked().get(run).cloned()
    }

    /// 运行开始时记一次；同一次运行再记不覆盖（快照只取第一次的）。
    pub fn put(&self, run: &str, p: RunPolicy) {
        let mut m = self.map.locked();
        if m.contains_key(run) {
            return;
        }
        m.insert(run.to_string(), p);
        self.save(&m);
    }

    fn save(&self, m: &BTreeMap<String, RunPolicy>) {
        if self.path.as_os_str().is_empty() {
            return;
        }
        let tmp = self.path.with_extension("json.tmp");
        let wrote = serde_json::to_vec_pretty(m)
            .map_err(std::io::Error::other)
            .and_then(|b| {
                if let Some(d) = self.path.parent() {
                    std::fs::create_dir_all(d)?;
                }
                std::fs::write(&tmp, b)?;
                std::fs::rename(&tmp, &self.path)
            });
        if let Err(e) = wrote {
            tracing::warn!(path = %self.path.display(), "run policies not saved: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_policy_of_a_run_is_kept_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run-policies.json");
        let p = RunPolicies::open(path.clone());
        let deny = RunPolicy {
            job: "j".into(),
            network: Network::DenyAll,
        };
        p.put("r1", deny.clone());
        p.put(
            "r1",
            RunPolicy {
                job: "j".into(),
                network: Network::Inherit,
            },
        );
        assert_eq!(p.get("r1"), Some(deny.clone()), "快照只取第一次");
        let again = RunPolicies::open(path);
        assert_eq!(again.get("r1"), Some(deny), "跨重启还在");
    }
}
