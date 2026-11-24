//! 实例用到的 `conix.yaml` 那几段（`jobs`、`notify`）：启动时与会话里改配置（`conix_config_set`，
//! architecture §8.11）走同一套解析与生效，免得两边对同一份文件读出两种结果。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use app::config::{self, Config};
use async_trait::async_trait;
use host::Hub;
use serde_json::Value;

/// 通知出口要知道自己是谁发的。
#[derive(Clone)]
pub struct NotifyOrigin {
    pub origin: host::notify::Origin,
    pub log: PathBuf,
}

/// 把 `jobs` 与 `notify` 交给实例；回读不懂的项（写进 `/doctor` 的 conix.config）。
pub fn apply(hub: &Hub, cfg: &Config, notify: &NotifyOrigin) -> Vec<String> {
    let j = &cfg.jobs;
    let text = |a: &Option<config::Amount>| a.as_ref().map(config::Amount::text);
    let (guard, mut problems) =
        host::jobs::guard::GuardConfig::parse(&host::jobs::guard::GuardText {
            timeout: text(&j.defaults.timeout),
            max_tokens: text(&j.defaults.max_tokens),
            overlap: j.defaults.overlap.clone(),
            disable_after: j.defaults.disable_after,
            network: j.defaults.network.clone(),
            limit_timeout: text(&j.limits.timeout),
            limit_max_tokens: text(&j.limits.max_tokens),
            daily_tokens: text(&j.daily_tokens),
            max_concurrent: j.max_concurrent,
        });
    hub.set_guard_config(guard);
    if cfg.notify.is_null() {
        hub.clear_notifier();
    } else {
        match serde_yaml::from_value::<host::notify::NotifyConfig>(cfg.notify.clone()) {
            Ok(n) => {
                let mut n =
                    host::notify::Notifier::new(n, notify.origin.clone(), Some(notify.log.clone()));
                // 只给 LIVE 测试用：合并窗口缩短，免得每条用例干等一分钟。不进 conix.yaml。
                if let Some(ms) = std::env::var("CONIX_TEST_NOTIFY_WINDOW_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                {
                    n.window_ms = ms;
                }
                hub.set_notifier(n);
            }
            Err(e) => problems.push(format!("notify is not understood, nothing is sent: {e}")),
        }
    }
    for p in &problems {
        tracing::warn!("conix.yaml: {p}");
    }
    hub.set_config_problems(problems.clone());
    problems
}

/// 点分路径上换一个值（`null` 删掉这个键）；中间缺的层建成映射。
fn set_path(
    root: &mut serde_yaml::Value,
    path: &str,
    value: serde_yaml::Value,
) -> Result<(), String> {
    let keys: Vec<&str> = path.split('.').filter(|k| !k.is_empty()).collect();
    let Some((last, parents)) = keys.split_last() else {
        return Err("path is empty".into());
    };
    let mut cur = root;
    for k in parents {
        if !cur.is_mapping() {
            *cur = serde_yaml::Value::Mapping(Default::default());
        }
        let m = cur.as_mapping_mut().expect("just made a mapping");
        cur = m
            .entry(serde_yaml::Value::String(k.to_string()))
            .or_insert_with(|| serde_yaml::Value::Mapping(Default::default()));
    }
    if !cur.is_mapping() {
        *cur = serde_yaml::Value::Mapping(Default::default());
    }
    let m = cur.as_mapping_mut().expect("just made a mapping");
    let key = serde_yaml::Value::String(last.to_string());
    if value.is_null() {
        m.remove(&key);
    } else {
        m.insert(key, value);
    }
    Ok(())
}

/// 这几段改了要等下次启动（TUI 或实例启动时才读）。
const RESTART: [&str; 3] = ["ui", "instance", "engine"];

pub struct YamlStore {
    pub home: PathBuf,
    pub notify: NotifyOrigin,
}

impl YamlStore {
    fn path(&self) -> PathBuf {
        self.home.join(config::FILE)
    }
}

/// 与启动时同一个解析；读不懂的项（`jobs`、`notify` 的写法）也算不过。
fn check(text: &str) -> Result<Config, String> {
    let cfg: Config =
        serde_yaml::from_str(text).map_err(|e| format!("conix.yaml would not parse: {e}"))?;
    Ok(cfg)
}

fn sections(text: &str) -> serde_yaml::Value {
    serde_yaml::from_str(text).unwrap_or(serde_yaml::Value::Null)
}

#[async_trait]
impl host::hub::ConixConfigStore for YamlStore {
    fn read(&self) -> Result<String, String> {
        std::fs::read_to_string(self.path()).map_err(|e| format!("{}: {e}", self.path().display()))
    }

    fn set(&self, path: &str, value: &Value) -> Result<String, String> {
        let old = self.read().unwrap_or_default();
        let mut root: serde_yaml::Value = if old.trim().is_empty() {
            serde_yaml::Value::Mapping(Default::default())
        } else {
            serde_yaml::from_str(&old).map_err(|e| format!("conix.yaml does not parse now: {e}"))?
        };
        let v: serde_yaml::Value =
            serde_json::from_value(value.clone()).map_err(|e| e.to_string())?;
        set_path(&mut root, path, v)?;
        let text = serde_yaml::to_string(&root).map_err(|e| e.to_string())?;
        let cfg = check(&text)?;
        // jobs / notify 写法上的问题在启动时只是告警；这里人正要改它，直接拒。
        let (_, problems) = jobs_problems(&cfg);
        if !problems.is_empty() {
            return Err(problems.join("; "));
        }
        Ok(text)
    }

    async fn apply(&self, hub: &Hub, text: &str) -> Result<(PathBuf, Vec<String>), String> {
        let cfg = check(text)?;
        let old = self.read().unwrap_or_default();
        let dir = self.home.join("config-backups");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let backup = dir.join(format!("conix-{}.yaml", host::instance::now_ms()));
        std::fs::write(&backup, &old).map_err(|e| format!("backup {}: {e}", backup.display()))?;
        let tmp = self.path().with_extension("yaml.tmp");
        std::fs::write(&tmp, text)
            .and_then(|_| std::fs::rename(&tmp, self.path()))
            .map_err(|e| format!("{}: {e}", self.path().display()))?;
        apply(hub, &cfg, &self.notify);
        let (before, after) = (sections(&old), sections(text));
        let restart = RESTART
            .iter()
            .filter(|s| before.get(**s) != after.get(**s))
            .map(|s| s.to_string())
            .collect();
        Ok((backup, restart))
    }
}

/// 只看 `jobs` 那几项的写法（`notify` 的在 apply 时报）。
fn jobs_problems(cfg: &Config) -> (host::jobs::guard::GuardConfig, Vec<String>) {
    let j = &cfg.jobs;
    let text = |a: &Option<config::Amount>| a.as_ref().map(config::Amount::text);
    host::jobs::guard::GuardConfig::parse(&host::jobs::guard::GuardText {
        timeout: text(&j.defaults.timeout),
        max_tokens: text(&j.defaults.max_tokens),
        overlap: j.defaults.overlap.clone(),
        disable_after: j.defaults.disable_after,
        network: j.defaults.network.clone(),
        limit_timeout: text(&j.limits.timeout),
        limit_max_tokens: text(&j.limits.max_tokens),
        daily_tokens: text(&j.daily_tokens),
        max_concurrent: j.max_concurrent,
    })
}

pub fn store(home: &Path, notify: NotifyOrigin) -> Arc<YamlStore> {
    Arc::new(YamlStore {
        home: home.to_path_buf(),
        notify,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dotted_path_sets_and_removes_values() {
        let mut v: serde_yaml::Value =
            serde_yaml::from_str("jobs:\n  defaults:\n    timeout: 30m\n").unwrap();
        set_path(&mut v, "jobs.defaults.network", "allowlist".into()).unwrap();
        set_path(&mut v, "ui.ratio", 50.into()).unwrap();
        set_path(&mut v, "jobs.defaults.timeout", serde_yaml::Value::Null).unwrap();
        let s = serde_yaml::to_string(&v).unwrap();
        assert!(
            s.contains("network: allowlist") && s.contains("ratio: 50"),
            "{s}"
        );
        assert!(!s.contains("timeout"), "{s}");
    }

    #[test]
    fn a_bad_value_is_refused_before_anything_is_written() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(config::FILE),
            "jobs:\n  defaults:\n    timeout: 30m\n",
        )
        .unwrap();
        let s = YamlStore {
            home: dir.path().to_path_buf(),
            notify: NotifyOrigin {
                origin: host::notify::Origin {
                    instance: "t".into(),
                    host: "h".into(),
                    project: "p".into(),
                },
                log: dir.path().join("n.jsonl"),
            },
        };
        use host::hub::ConixConfigStore as _;
        assert!(
            s.set("jobs.defaults.network", &serde_json::json!("everywhere"))
                .is_err()
        );
        assert!(
            s.set("jobs.defaults.timeout", &serde_json::json!("soon"))
                .is_err()
        );
        let ok = s
            .set("jobs.defaults.network", &serde_json::json!("allowlist"))
            .unwrap();
        assert!(ok.contains("allowlist"));
        assert!(
            s.read().unwrap().contains("30m") && !s.read().unwrap().contains("allowlist"),
            "set 不写"
        );
    }
}
