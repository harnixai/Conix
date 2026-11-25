//! 流程的外部审批（`via: mcp` 的确认，design/flows.md §4）：经 Agix 调 MCP 工具。
//!
//! 不在宿主侧自己连 MCP：server 的配置、OAuth 凭据、`mcp.policy` 与项目信任都在 Agix，自己连就得再配一份、
//! 再授权一次，还绕开了企业的 MCP 策略。用的是 Agix 的 `mcp.callTool`（harnixai/Agix#111）；引擎还没有这个
//! 方法时，审批就是 error，写明缺什么。

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
        let r = hub
            .call(
                "mcp.callTool",
                json!({"server": server, "tool": tool, "arguments": arguments,
                       "project_root": root, "timeout_ms": CALL_TIMEOUT_MS}),
            )
            .await
            .map_err(|e| match e.code() {
                Some(-32601) => {
                    "the engine cannot call MCP tools for the host yet (harnixai/Agix#111)"
                        .to_string()
                }
                _ => format!("mcp.callTool {server}/{tool}: {e}"),
            })?;
        read_result(server, tool, &r)
    }
}

/// `mcp.callTool` 回的 `{ok, structured?, text?, is_error?}` → 工具的结构化结果；没有结构化结果时，文本是 JSON
/// 就按 JSON 读，否则就是那段文本。工具自己报的错（`is_error`）是 `Err`。
fn read_result(server: &str, tool: &str, r: &Value) -> Result<Value, String> {
    let text = r["text"].as_str().unwrap_or_default();
    if r["ok"] == false || r["is_error"] == true {
        let why = r["error"]["message"].as_str().unwrap_or(text);
        return Err(format!("{server}/{tool}: {why}"));
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
    }
}
