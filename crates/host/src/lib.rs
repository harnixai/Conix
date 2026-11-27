//! Conix 的实例进程（architecture §8）：会话代理 `Hub` 经 [`Engine`] 用 Agix，把帧翻译成
//! 领域事件；作业调度；unix socket 服务与单一接入；实例目录、锁与发现文件。
//! 这个 crate 不依赖 Agix：嵌入的引擎由 `engine` crate 提供。

pub mod engine;
pub mod exec;
pub mod flows;
pub mod hub;
pub mod instance;
pub mod jobs;
pub mod notify;
pub mod plugins;
pub mod project;
pub mod server;
pub mod service;
mod sync;

pub use engine::{Engine, Notification, RpcError};
pub use hub::{Hub, HubConfig};
pub use service::{Service, Sink};
