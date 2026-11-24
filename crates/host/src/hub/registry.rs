//! Conix 自己记的会话归属（architecture §8.4，`sessions.json`）。Agix 不知道一个会话是作业
//! 触发的、哪个会话分叉来的，冷会话的名字它也不报，所以这些由实例记下来。

use std::collections::BTreeMap;
use std::path::PathBuf;

use port::Origin;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub origin: Origin,
    #[serde(default)]
    pub title: Option<String>,
    /// 名字只是第一句话的摘头：引擎自动起的名字来了可以替掉它。
    #[serde(default)]
    pub title_from_text: bool,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub updated_ms: u64,
    /// 实例建这个会话的时刻（会话列表的顺序、`#n` 按它）；0 是没记，用引擎报的。
    #[serde(default)]
    pub created_ms: u64,
    /// 在 Conix 里关掉的：Agix 的列表里还有它（transcript 在），会话标签里不再列，在「已关闭」里。
    #[serde(default)]
    pub closed: bool,
    /// 关掉的时刻：「已关闭」按它新的在前。
    #[serde(default)]
    pub closed_ms: u64,
}

#[derive(Debug, Default)]
pub struct Registry {
    path: Option<PathBuf>,
    records: BTreeMap<String, Record>,
}

impl Registry {
    /// 读不出来就从空的开始：归属丢了只是退回 `User`，不该让实例起不来。逐条读：一条读不懂
    /// （旧版本写的、已不存在的归属）只丢那一条，不连累别的。
    pub fn open(path: PathBuf) -> Registry {
        let raw: BTreeMap<String, serde_json::Value> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        let records = raw
            .into_iter()
            .filter_map(|(id, v)| Some((id, serde_json::from_value(v).ok()?)))
            .collect();
        Registry {
            path: Some(path),
            records,
        }
    }

    pub fn in_memory() -> Registry {
        Registry::default()
    }

    pub fn get(&self, id: &str) -> Option<&Record> {
        self.records.get(id)
    }

    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut Record)) {
        f(self.records.entry(id.to_string()).or_default());
        self.save();
    }

    pub fn remove(&mut self, id: &str) {
        if self.records.remove(id).is_some() {
            self.save();
        }
    }

    fn save(&self) {
        let Some(path) = &self.path else { return };
        let tmp = path.with_extension("json.tmp");
        if let Ok(text) = serde_json::to_string_pretty(&self.records)
            && std::fs::write(&tmp, text).is_ok()
        {
            let _ = std::fs::rename(&tmp, path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_survive_reopen_and_an_unreadable_one_drops_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.json");
        let mut r = Registry::open(path.clone());
        r.update("s1", |x| {
            x.origin = Origin::Fork {
                from: port::SessionId("s0".into()),
                from_title: "t".into(),
            };
            x.created_ms = 7;
        });
        let mut text: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        text["old"] = serde_json::json!({"origin": {"editor": {"of": "job", "name": "x"}}});
        std::fs::write(&path, text.to_string()).unwrap();
        let r = Registry::open(path);
        assert!(matches!(r.get("s1").unwrap().origin, Origin::Fork { .. }));
        assert_eq!(r.get("s1").unwrap().created_ms, 7);
        assert!(r.get("old").is_none());
    }
}
