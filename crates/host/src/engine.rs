//! 会话代理只经这个 trait 用引擎：真实实现是嵌入的 Agix（`engine` crate），测试用脚本化的
//! 假引擎。通知（`session.event` / `daemon.event`）在构造时以 channel 另行交给 `Hub`。

use async_trait::async_trait;
use serde_json::{Value, json};

pub use wire::{Notification, RpcError};

/// Codix 说的 Agix 协议版本：0.4.0 起是 5。版本号不同就是不同的协议（会话选项、设置键都变过），
/// 不按"大于等于"放过——0.4 对旧写法一律拒，反过来新版本也不会认 Codix 写的旧东西。
pub const PROTOCOL_VERSION: u64 = 5;

/// Codix 用到、且版本号推不出的能力位（协议：能力位单独查，不靠版本号推）。
pub const REQUIRED_CAPABILITIES: [&str; 2] = ["session_settings", "config_explain"];

/// `daemon.ping` 的结果是不是 Codix 认得的引擎；不是就给一句能照着做的错。
pub fn check_engine(ping: &Value) -> Result<(), RpcError> {
    let version = ping["version"].as_str().unwrap_or("?");
    let wrong = |why: String| RpcError::Remote {
        code: -32603,
        message: format!(
            "Agix {version}: {why}; Codix needs Agix 0.4 (protocol {PROTOCOL_VERSION})"
        ),
        data: json!({"kind": "engine_mismatch"}),
    };
    match ping["protocol_version"].as_u64() {
        Some(PROTOCOL_VERSION) => {}
        Some(v) => return Err(wrong(format!("protocol {v}"))),
        None => return Err(wrong("no protocol version".into())),
    }
    let caps: Vec<&str> = ping["capabilities"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let missing: Vec<&str> = REQUIRED_CAPABILITIES
        .into_iter()
        .filter(|c| !caps.contains(c))
        .collect();
    if !missing.is_empty() {
        return Err(wrong(format!("missing {}", missing.join(", "))));
    }
    Ok(())
}


#[async_trait]
pub trait Engine: Send + Sync + 'static {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError>;
}

#[async_trait]
impl Engine for wire::Client {
    async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        wire::Client::call(self, method, params).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_protocol_5_with_the_needed_capabilities_passes() {
        let ok = json!({"version": "0.4.0", "protocol_version": 5,
                        "capabilities": ["x", "session_settings", "config_explain"]});
        assert!(check_engine(&ok).is_ok());
        let old = json!({"version": "0.3.20", "protocol_version": 4, "capabilities": []});
        let e = check_engine(&old).unwrap_err().to_string();
        assert!(e.contains("protocol 4") && e.contains("Agix 0.4"), "{e}");
        let bare = json!({"version": "0.4.0", "protocol_version": 5, "capabilities": ["session_settings"]});
        let e = check_engine(&bare).unwrap_err().to_string();
        assert!(e.contains("missing config_explain"), "{e}");
        assert!(check_engine(&json!({})).is_err());
    }
}
