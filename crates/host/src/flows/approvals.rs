//! 流程的外部审批（`via: mcp` 的确认，design/flows.md §4）：经 Agix 调 MCP 工具。
//!
//! 不在宿主侧自己连 MCP：server 的配置、OAuth 凭据、`mcp.policy` 与项目信任都在 Agix，自己连就得再配一份、
//! 再授权一次，还绕开了企业的 MCP 策略。用的是 Agix 的 `mcp.callTool`（harnixai/Agix#111），不带会话：调用算
//! 宿主批准，deny 规则与 plan 模式照样拦。引擎没有这个方法时，审批就是 error，写明缺什么。

use async_trait::async_trait;
use harnix_flow::{Approvals, RunRecord};
use serde_json::{Value, json};

use crate::hub::WeakHub;

/// 一次审批工具调用至多等多久：建单、查状态都应当很快。
const CALL_TIMEOUT_MS: u64 = 60_000;

pub struct McpApprovals(pub WeakHub);

#[async_trait]
impl Approvals for McpApprovals {
    async fn call(
        &self,
        run: &RunRecord,
        server: &str,
        tool: &str,
        arguments: &Value,
    ) -> Result<Value, String> {
        let hub = self
            .0
            .upgrade()
            .ok_or_else(|| "the instance is stopping".to_string())?;
        let root = run
            .project_root
            .clone()
            .unwrap_or_else(|| hub.project().to_string());
        // 场景与流程的会话一样（`code`）：同一对 (项目, 场景) 的连接、规则与信任。
        let r = hub
            .call(
                "mcp.callTool",
                json!({"server": server, "tool": tool, "arguments": arguments,
                       "project_root": root, "scene": super::SCENE, "timeout_ms": CALL_TIMEOUT_MS}),
            )
            .await
            .map_err(|e| call_error(server, tool, &e))?;
        read_result(server, tool, &r)
    }
}

/// 调用没成立（JSON-RPC 错误）时说清楚是哪一种：引擎没有这个方法、server 现在调不了（带它的状态）、别的。
fn call_error(server: &str, tool: &str, e: &crate::engine::RpcError) -> String {
    match e {
        crate::engine::RpcError::Remote { code: -32601, .. } => {
            "the engine cannot call MCP tools for the host yet (harnixai/Agix#111)".to_string()
        }
        crate::engine::RpcError::Remote {
            code: MCP_SERVER_UNAVAILABLE,
            data,
            ..
        } => {
            let state = data["state"].as_str().unwrap_or("unavailable");
            let more = data["error"]
                .as_str()
                .map(|m| format!(": {m}"))
                .unwrap_or_default();
            let hint = match state {
                "untrusted" => " (trust the project)",
                "auth_required" => " (sign in to it first)",
                "disabled" | "denied" => " (it is turned off or denied by the MCP policy)",
                _ => "",
            };
            format!("MCP server {server} cannot be called now: {state}{hint}{more}")
        }
        e => format!("mcp.callTool {server}/{tool}: {e}"),
    }
}

/// Agix 的 `MCP_SERVER_UNAVAILABLE`：server 不在 `connected` / `disconnected`，`data` 是 `{server, state, error?}`。
const MCP_SERVER_UNAVAILABLE: i64 = -32029;

/// `mcp.callTool` 回的 `{ok, structured?, text?, is_error?}` → 工具的结构化结果；没有结构化结果时，文本是 JSON
/// 就按 JSON 读，否则就是那段文本。工具自己报的错（`is_error`）是 `Err`。`ok: false` 且 `reached_server` 时，
/// server 可能已经照做了（单建了），只是答复没回来：写明，免得人看了 error 就重跑、再建一张。
fn read_result(server: &str, tool: &str, r: &Value) -> Result<Value, String> {
    let text = r["text"].as_str().unwrap_or_default();
    if r["ok"] == false {
        let why = r["error"]["message"].as_str().unwrap_or(text);
        let code = r["error"]["code"].as_str().unwrap_or("failed");
        let reached = if r["reached_server"] == true {
            "; the server got the call and may have acted on it: check there before running it again"
        } else {
            ""
        };
        return Err(format!("{server}/{tool}: {code}: {why}{reached}"));
    }
    if r["is_error"] == true {
        return Err(format!("{server}/{tool}: {text}"));
    }
    if let Some(v) = r.get("structured").filter(|v| !v.is_null()) {
        return Ok(v.clone());
    }
    Ok(serde_json::from_str(text).unwrap_or_else(|_| Value::String(text.to_string())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tools_result_is_its_structured_content_else_its_text() {
        let r = read_result(
            "s",
            "t",
            &json!({"ok": true, "structured": {"id": "A-1"}, "text": "x"}),
        );
        assert_eq!(r.unwrap(), json!({"id": "A-1"}));
        let r = read_result(
            "s",
            "t",
            &json!({"ok": true, "text": "{\"state\":\"APPROVED\"}"}),
        );
        assert_eq!(r.unwrap(), json!({"state": "APPROVED"}));
        assert_eq!(
            read_result("s", "t", &json!({"ok": true, "text": "done"})).unwrap(),
            json!("done")
        );
        let e = read_result(
            "s",
            "t",
            &json!({"ok": true, "is_error": true, "text": "no such request"}),
        )
        .unwrap_err();
        assert_eq!(e, "s/t: no such request");
        let e = read_result(
            "s",
            "t",
            &json!({"ok": false, "reached_server": true, "error": {"code": "timeout", "message": "took too long"}}),
        )
        .unwrap_err();
        assert!(
            e.starts_with("s/t: timeout: took too long") && e.contains("may have acted"),
            "{e}"
        );
    }

    #[test]
    fn a_call_that_did_not_happen_says_why() {
        use crate::engine::RpcError;
        let gone = RpcError::Remote {
            code: -32601,
            message: "no method".into(),
            data: Value::Null,
        };
        assert!(call_error("s", "t", &gone).contains("Agix#111"));
        let off = RpcError::Remote {
            code: MCP_SERVER_UNAVAILABLE,
            message: "x".into(),
            data: json!({"server": "s", "state": "untrusted"}),
        };
        assert_eq!(
            call_error("s", "t", &off),
            "MCP server s cannot be called now: untrusted (trust the project)"
        );
    }
}
