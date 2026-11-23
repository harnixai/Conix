//! 嵌入的 Agix（architecture §8.4）：`agix::embed` 起引擎，经 `duplex` 上的 JSON-RPC 连上，
//! 交给 `host` 当 [`host::Engine`] 用。这是整个 Codix 里唯一依赖 Agix 的地方。

use std::path::PathBuf;

use anyhow::Context as _;
use tokio::sync::mpsc;

pub use agix::process::log_filter;

/// 引擎在项目里放东西的目录名（Agix#86）与它自称的产品名。
pub const PROJECT_DIR: &str = ".codix";
pub const PRODUCT_NAME: &str = "Codix";

/// 进程内两个 1 MiB 的缓冲：帧上限是 16 MiB，但缓冲只是流水，不必放下整帧。
const DUPLEX_BUF: usize = 1 << 20;

pub struct EngineConfig {
    /// `~/.codix`：Agix 的 data_root 与 Codix 的家目录是同一个（architecture §8.1）。
    pub data_root: PathBuf,
    pub project: PathBuf,
    pub groups_file: PathBuf,
    /// cli 层的设置（`EmbedConfig.settings`）：叠在每个项目、场景的设置之上，不受项目信任限制。
    /// 由 `host::hub::cli_settings` 给；`app.history` 在这里补上。写错的键让启动失败。
    pub settings: serde_json::Value,
    /// 引擎自带的 `anthropic` 用哪个模型（只在设置里没定义任何 `providers` 时有用）；`None` 用 Agix 的缺省。
    pub model: Option<String>,
}

pub struct Embedded {
    engine: agix::embed::Embedded,
}

impl Embedded {
    /// 起引擎并接上一条连接。返回的客户端与通知流交给 `host::Hub::start`。
    pub async fn start(
        cfg: EngineConfig,
    ) -> anyhow::Result<(
        Embedded,
        wire::Client,
        mpsc::UnboundedReceiver<wire::Notification>,
    )> {
        let mut c = agix::embed::EmbedConfig::new(cfg.data_root);
        c.cwd = Some(cfg.project);
        // 不给 instance：Agix 的实例锁按"数据根 + 场景"算，同一场景只能锁一份，而 Codix 每个项目
        // 一个实例都在 code 场景里；实例锁由 Codix 自己管（architecture §8.4）。
        c.instance = None;
        c.groups_file = Some(cfg.groups_file);
        c.project_dir = Some(PROJECT_DIR.into());
        c.product = PRODUCT_NAME.into();
        // 会话是项目的（architecture §8.4）：受信项目的会话放进 `.codix/state/history/`，全局只留索引。
        // 0.4 起经 cli 层设置 `app.history` 给。
        let mut settings = cfg.settings;
        if let Some(o) = settings.as_object_mut() {
            o.entry("app")
                .or_insert_with(|| serde_json::json!({}))
                .as_object_mut()
                .map(|a| a.insert("history".into(), "project".into()));
        }
        c.settings = Some(settings);
        if let Some(m) = cfg.model {
            c.model = m;
        }
        let engine = agix::embed::Embedded::start(c)
            .await
            .context("start the Agix engine")?;
        let (ours, theirs) = tokio::io::duplex(DUPLEX_BUF);
        engine.serve_stream(theirs);
        let (read, write) = tokio::io::split(ours);
        let (client, notes) = wire::Client::spawn(read, write);
        Ok((Embedded { engine }, client, notes))
    }

    /// 停掉所有会话（Agix 至多等 10 秒）、放掉进程组登记。
    pub async fn shutdown(&self) {
        self.engine.shutdown().await;
    }
}
