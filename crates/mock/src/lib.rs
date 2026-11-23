//! Codix 的 mock 后端（docs/design/testing.md §3）：种子数据给出初始状态，场景脚本按时间
//! 推事件、按规则回应命令。不依赖 Agix、Harnix 或网络；时钟走 tokio，测试里用暂停时钟即可确定。

mod backend;
mod catalog;
mod instances;
mod plugins;
mod scenario;
mod seed;
mod state;

pub use backend::{CallLog, MockOptions, ScriptedBackend};
pub use catalog::{scenario_names, seed_names};
pub use instances::{MockInstance, MockInstances};
pub use scenario::{Matcher, Rule, Scenario, Step, StepAction, Timed};
pub use seed::{FlowSeed, JobSeed, Seed, SessionSeed};
pub use state::MockState;

#[derive(Debug, thiserror::Error)]
pub enum MockError {
    #[error("没有内置的种子或场景：{0}")]
    Unknown(String),
    #[error("读 {0} 失败：{1}")]
    Io(String, std::io::Error),
    #[error("解析 {0} 失败：{1}")]
    Parse(String, String),
}

/// 按名字（内置）或路径加载场景。
pub fn load_scenario(name_or_path: &str) -> Result<Scenario, MockError> {
    match catalog::scenario(name_or_path) {
        Some(text) => Scenario::parse(name_or_path, text),
        None => {
            let text = std::fs::read_to_string(name_or_path)
                .map_err(|e| MockError::Io(name_or_path.to_string(), e))?;
            Scenario::parse(name_or_path, &text)
        }
    }
}

/// 按名字（内置）或路径加载种子；`busy` 由代码生成。
pub fn load_seed(name_or_path: &str) -> Result<Seed, MockError> {
    if name_or_path == "busy" {
        return Ok(seed::busy());
    }
    match catalog::seed(name_or_path) {
        Some(text) => Seed::parse(name_or_path, text),
        None => {
            let text = std::fs::read_to_string(name_or_path)
                .map_err(|e| MockError::Io(name_or_path.to_string(), e))?;
            Seed::parse(name_or_path, &text)
        }
    }
}

/// YAML 经 JSON 再反序列化：serde_yaml 0.9 只认 `!tag` 形式的枚举，而场景与种子里写的是
/// `{variant: {...}}`，与线上 JSON 同形。
pub(crate) fn from_yaml_value<T: serde::de::DeserializeOwned>(
    value: serde_yaml::Value,
) -> Result<T, String> {
    let json = serde_json::to_value(value).map_err(|e| e.to_string())?;
    serde_json::from_value(json).map_err(|e| e.to_string())
}
