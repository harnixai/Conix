//! 插件（docs/design/plugins.md §3）：可装的包、装好的插件、安装时摆给人看的两份披露。

use serde::{Deserialize, Serialize};

/// 可装的包：自带目录与 `~/.conix/plugin-store/` 里的 zip，只按文件名 `<名>-<版本>.zip` 列，不解包。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPackage {
    pub name: String,
    pub version: String,
    /// zip 的绝对路径。
    pub path: String,
    /// 这个名字已经装了（可能是别的版本）。
    #[serde(default)]
    pub installed: bool,
}

/// 装好的插件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub enabled: bool,
    /// 启用了却不会加载的原因（Agix 报的，如用了宿主没提供的库）。
    #[serde(default)]
    pub disabled_reason: Option<String>,
    /// 它带来的流程（`[x-conix]`）。
    #[serde(default)]
    pub flows: Vec<String>,
    /// 它带来的 host 门禁与程序步骤名。
    #[serde(default)]
    pub gates: Vec<String>,
    #[serde(default)]
    pub steps: Vec<String>,
    /// 与别的插件同名、被遮住的东西，一条一句。
    #[serde(default)]
    pub conflicts: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plugins {
    pub available: Vec<PluginPackage>,
    pub installed: Vec<PluginInfo>,
}

/// 一段会一字不差到达模型的文字，标明来处。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisibleText {
    pub origin: String,
    pub text: String,
}

/// 安装时摆给人看的：Agix 的一份（它读得懂的部分），Conix 的一份（`[x-conix]` 带来的，Agix 不读）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginDisclosure {
    /// 能力：命令能写什么、函数、脚本点、用了哪个宿主库……
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub model_visible: Vec<VisibleText>,
    /// Conix 的：流程（名字 — 标题）、门禁与步骤各对应哪个插件函数、是否要 command 门禁。
    #[serde(default)]
    pub conix: Vec<String>,
    /// Conix 的：流程包里站的 skill 正文等，会到达模型。
    #[serde(default)]
    pub conix_model_visible: Vec<VisibleText>,
}
