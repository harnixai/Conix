//! 给插件代码的宿主 JS 库 `codix.*`（docs/design/plugins.md §5.1）的一次调用。
//!
//! `engine` 在 `EmbedConfig.js_libs` 里登记函数表（启动时引擎就要），实例的 `Hub` 起来之后才接上处理者；
//! 两边都依赖 `wire`，所以调用的形状放在这里，engine 不必依赖 host。

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;

/// `codix.*` 的函数（点分路径，命名空间之下）。
pub const FUNCTIONS: [&str; 5] = [
    "flow.defs",
    "flow.runs",
    "flow.run",
    "flow.current",
    "flow.start",
];

/// 一次调用：哪个函数、传了什么，以及谁在调（Agix 的 `HostCallContext`）。
#[derive(Debug, Clone)]
pub struct HostLibCall {
    pub path: String,
    pub input: Value,
    /// 包自报的名字：谁都能装一个叫 `xforge` 的包，不拿它当授权。
    pub plugin: String,
    /// 包从哪来：宿主自带的包为 true（Codix 现在没有）。
    pub host_package: bool,
    /// 命令名或函数名。
    pub entry: String,
    pub session_id: Option<String>,
    pub project_root: Option<PathBuf>,
    /// `model` / `user` / `skill` / `RunCode` / `host`（宿主调插件函数，即门禁与步骤里）。
    pub via: String,
}

/// 出错：`code` 与 `message`，插件里 reject 一个带这两个字段的 `Error`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLibError {
    pub code: String,
    pub message: String,
}

impl HostLibError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        HostLibError {
            code: code.into(),
            message: message.into(),
        }
    }
}

pub type HostLibFuture = Pin<Box<dyn Future<Output = Result<Value, HostLibError>> + Send>>;

/// 处理者：实例起来后接上。
pub type HostLibHandler = Arc<dyn Fn(HostLibCall) -> HostLibFuture + Send + Sync>;
