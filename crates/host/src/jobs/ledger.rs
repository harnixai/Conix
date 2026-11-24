//! 无人值守的用量账（architecture §8.5 每日额度）：`state/conix/usage/<YYYYMMDD>.json`，按作业记当天
//! 用了多少 token。日期按本地时区：额度是"今天"的，人看的也是本地的今天。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

pub struct Ledger {
    dir: PathBuf,
}

impl Ledger {
    pub fn new(dir: impl Into<PathBuf>) -> Ledger {
        Ledger { dir: dir.into() }
    }

    fn path(&self, day: &str) -> PathBuf {
        self.dir.join(format!("{day}.json"))
    }

    pub fn day(&self, day: &str) -> BTreeMap<String, u64> {
        fs::read(self.path(day))
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    pub fn total(&self, day: &str) -> u64 {
        self.day(day).values().sum()
    }

    pub fn of(&self, day: &str, job: &str) -> u64 {
        self.day(day).get(job).copied().unwrap_or(0)
    }

    /// 记一笔。调用方持着作业存储的锁，读改写不会和别的记账交错。
    pub fn add(&self, day: &str, job: &str, tokens: u64) -> Result<(), String> {
        if tokens == 0 {
            return Ok(());
        }
        let mut m = self.day(day);
        *m.entry(job.to_string()).or_insert(0) += tokens;
        fs::create_dir_all(&self.dir).map_err(|e| e.to_string())?;
        let path = self.path(day);
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec(&m).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }
}

/// 本地日期 `YYYYMMDD`。
pub fn day_of(ms: u64) -> String {
    use chrono::TimeZone;
    chrono::Local
        .timestamp_millis_opt(ms as i64)
        .single()
        .map(|t| t.format("%Y%m%d").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_is_summed_per_job() {
        let d = tempfile::tempdir().unwrap();
        let l = Ledger::new(d.path().join("usage"));
        l.add("20261119", "j1", 100).unwrap();
        l.add("20261119", "j1", 50).unwrap();
        l.add("20261119", "j2", 7).unwrap();
        l.add("20261120", "j1", 1).unwrap();
        assert_eq!(l.of("20261119", "j1"), 150);
        assert_eq!(l.total("20261119"), 157);
        assert_eq!(l.total("20261121"), 0);
    }
}
