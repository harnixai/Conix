//! 建会话时给引擎的设置（Agix 0.4 的 `options.settings`），以及实例起引擎时的 cli 层底线。
//!
//! 0.4 起会话层缺省只能收紧：比设置宽的值被夹回、allow 规则被扣下。所以 Conix 要放宽的东西分两处放：
//! 对所有会话都成立的（只读工具、流程自己的工具不问）放 cli 层；按会话放宽的（流程站的 `acceptEdits`）
//! 靠 cli 层开的 `permissions.client_may_loosen`。会话层其余的都是收紧：ask / deny、网络、预算。

use serde_json::{Map, Value, json};

/// 实例起引擎时的 cli 层（`EmbedConfig.settings`）：叠在每个项目、场景的设置之上，不受项目信任限制。
/// `app.history` 由 engine 自己加。
pub fn cli_settings() -> Value {
    let mut allow: Vec<String> = super::tools::READ_ONLY
        .iter()
        .map(|t| t.to_string())
        .chain(crate::flows::tools::read_only_names())
        // 流程自己的工具只动这次运行的记录，且只挂在流程站的会话上；缺省每次都问人的话，
        // 无人看着的运行会卡在一站交付上。
        .chain(
            harnix_flow::runner::tool_specs()
                .into_iter()
                .map(|t| t.name),
        )
        .collect();
    allow.sort();
    allow.dedup();
    json!({
        "permissions": {
            // 流程站要 `acceptEdits`（比项目缺省的 `default` 宽）。协议说这是安全缺省、不是安全边界：
            // 连得上 socket 的客户端本来就能 `config.set`；managed 层写了的仍压过它。`/doctor` 里明示。
            "client_may_loosen": true,
            "allow": allow,
        }
    })
}

/// 会话层的设置。空的字段不写；全空时 [`SessionSettings::to_value`] 给 `None`，`options` 里就不带 `settings`。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionSettings {
    /// `permissions.mode`：`default` / `acceptEdits` / `plan` / `dontAsk` / `bypassPermissions`。
    pub mode: Option<String>,
    /// 规则串，如 `Write`、`Bash(git push:*)`、`mcp__db`。
    pub ask: Vec<String>,
    pub deny: Vec<String>,
    /// `model.effort`。
    pub effort: Option<String>,
    /// `sandbox.network.mode`：`deny_all` / `allowlist` / `proxy` / `unrestricted`。
    pub network: Option<String>,
    /// `session.token_budget`：会话从创建起能花的 token（不计缓存命中）。
    pub token_budget: Option<u64>,
}

impl SessionSettings {
    pub fn to_value(&self) -> Option<Value> {
        let mut root = Map::new();
        let mut put = |path: &[&str], v: Value| {
            let mut m = &mut root;
            for k in &path[..path.len() - 1] {
                m = m
                    .entry(k.to_string())
                    .or_insert_with(|| Value::Object(Map::new()))
                    .as_object_mut()
                    .expect("settings path is objects all the way");
            }
            m.insert(path[path.len() - 1].to_string(), v);
        };
        if let Some(m) = &self.mode {
            put(&["permissions", "mode"], json!(m));
        }
        if !self.ask.is_empty() {
            put(&["permissions", "ask"], json!(self.ask));
        }
        if !self.deny.is_empty() {
            put(&["permissions", "deny"], json!(self.deny));
        }
        if let Some(e) = &self.effort {
            put(&["model", "effort"], json!(e));
        }
        if let Some(n) = &self.network {
            put(&["sandbox", "network", "mode"], json!(n));
        }
        if let Some(b) = self.token_budget {
            put(&["session", "token_budget"], json!(b));
        }
        (!root.is_empty()).then_some(Value::Object(root))
    }

    /// 写进 `options`（有内容才写）。
    pub fn apply(&self, options: &mut Value) {
        if let (Some(o), Some(s)) = (options.as_object_mut(), self.to_value()) {
            o.insert("settings".into(), s);
        }
    }
}

/// 0.3 形状的规则（`{tool_name, behavior}`，流程库的 `ask_rules` 还给这个）→ 指定 behavior 的规则串。
pub fn rule_strings(rules: &Value, behavior: &str) -> Vec<String> {
    rules
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["behavior"] == behavior)
        .filter_map(|r| {
            let tool = r["tool_name"].as_str()?;
            Some(match r["rule_content"].as_str() {
                Some(c) => format!("{tool}({c})"),
                None => tool.to_string(),
            })
        })
        .collect()
}

/// `session.create` 结果里被夹住的项：`(field, requested, applied)`。
pub fn clamped(result: &Value) -> Vec<(String, Value, Value)> {
    result["clamped_options"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            (
                c["field"].as_str().unwrap_or_default().to_string(),
                c["requested"].clone(),
                c["applied"].clone(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_settings_are_left_out() {
        let mut o = json!({"attended": true});
        SessionSettings::default().apply(&mut o);
        assert_eq!(o, json!({"attended": true}));
    }

    #[test]
    fn every_field_lands_on_its_settings_key() {
        let s = SessionSettings {
            mode: Some("acceptEdits".into()),
            ask: vec!["Write".into()],
            deny: vec!["CronCreate".into()],
            effort: Some("high".into()),
            network: Some("deny_all".into()),
            token_budget: Some(2_000_000),
        };
        assert_eq!(
            s.to_value().unwrap(),
            json!({
                "permissions": {"mode": "acceptEdits", "ask": ["Write"], "deny": ["CronCreate"]},
                "model": {"effort": "high"},
                "sandbox": {"network": {"mode": "deny_all"}},
                "session": {"token_budget": 2_000_000},
            })
        );
    }

    #[test]
    fn old_rules_become_rule_strings() {
        let rules = json!([
            {"source": "session", "behavior": "ask", "tool_name": "Write"},
            {"source": "session", "behavior": "ask", "tool_name": "Bash", "rule_content": "git push:*"},
            {"source": "session", "behavior": "allow", "tool_name": "Read"},
        ]);
        assert_eq!(rule_strings(&rules, "ask"), ["Write", "Bash(git push:*)"]);
        assert_eq!(rule_strings(&rules, "allow"), ["Read"]);
    }

    #[test]
    fn the_cli_layer_lets_sessions_loosen_and_allows_read_only_and_flow_tools() {
        let c = cli_settings();
        assert_eq!(c["permissions"]["client_may_loosen"], true);
        let allow: Vec<&str> = c["permissions"]["allow"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        for t in [
            "session_list",
            "job_validate",
            "flow_state",
            "flow_submit",
            "flow_propose",
        ] {
            assert!(allow.contains(&t), "{t} in {allow:?}");
        }
        assert!(!allow.contains(&"job_save"), "writes still ask");
    }

    #[test]
    fn clamped_options_are_read() {
        let r = json!({"session_id": "s", "clamped_options": [
            {"field": "settings.permissions.mode", "requested": "acceptEdits", "applied": "default"}]});
        assert_eq!(
            clamped(&r),
            [(
                "settings.permissions.mode".into(),
                json!("acceptEdits"),
                json!("default")
            )]
        );
    }
}
