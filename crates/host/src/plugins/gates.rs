//! 插件带来的 host 门禁与程序步骤（plugins.md §5.2）：按 [`PluginMap`] 找到插件函数，经 `plugin.call` 调。
//!
//! 不带会话：结果只取决于项目里的数据，与站的会话还在不在无关；不带会话的调用要问人的一律失败，
//! 所以门禁里要跑命令的，规则得在项目设置里写好（函数会把被拒说清楚）。门禁"只读"靠的是插件函数声明的
//! `writes` 为空，不是调用时的 `writes:false`（见 `PluginGates::run`）。

use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use harnix_flow::kernel::refs::station_output;
use harnix_flow::{Engine, GateResult, Gates, Programs, RunRecord};
use serde_json::{Value, json};

use super::PluginMap;
use crate::hub::WeakHub;

/// `plugin.call` 的上限，门禁与步骤都用它。
const CALL_TIMEOUT_MS: u64 = 120_000;

pub struct PluginCalls {
    hub: WeakHub,
    map: Arc<RwLock<PluginMap>>,
    pub(crate) engine: OnceLock<Arc<Engine>>,
}

impl PluginCalls {
    pub(crate) fn new(hub: WeakHub, map: Arc<RwLock<PluginMap>>) -> Arc<Self> {
        Arc::new(PluginCalls {
            hub,
            map,
            engine: OnceLock::new(),
        })
    }

    /// 交给插件函数的：这次运行、这一站、各站到此刻的输出、在哪干活。
    fn input(&self, run: &RunRecord, station: &str) -> (Value, Option<String>) {
        let outputs: serde_json::Map<String, Value> = run
            .def
            .stations
            .iter()
            .map(|s| (s.id.clone(), station_output(&run.def, run, &s.id)))
            .collect();
        let workspace = self
            .engine
            .get()
            .and_then(|e| e.workspace_for(run))
            .map(|p| p.to_string_lossy().into_owned())
            .or_else(|| run.project_root.clone());
        (
            json!({
                "run": run.id,
                "flow": run.flow,
                "station": station,
                "inputs": run.inputs,
                "outputs": outputs,
                "workspace": workspace,
                "project_root": run.project_root,
            }),
            workspace,
        )
    }

    async fn call(
        &self,
        function: &str,
        input: Value,
        cwd: Option<String>,
        writes: bool,
    ) -> Result<Value, String> {
        let hub = self
            .hub
            .upgrade()
            .ok_or_else(|| "the instance is stopping".to_string())?;
        let cwd = cwd.unwrap_or_else(|| hub.project().to_string());
        let r = hub
            .call(
                "plugin.call",
                json!({"function": function, "input": input, "cwd": cwd, "writes": writes,
                       "timeout_ms": CALL_TIMEOUT_MS}),
            )
            .await
            .map_err(|e| format!("plugin.call {function}: {e}"))?;
        if r["ok"] == true {
            Ok(r["value"].clone())
        } else {
            Err(format!(
                "{function}: {} {}",
                r["error"]["code"].as_str().unwrap_or("failed"),
                r["error"]["message"].as_str().unwrap_or_default()
            ))
        }
    }
}

pub struct PluginGates(pub Arc<PluginCalls>);

#[async_trait]
impl Gates for PluginGates {
    async fn run(&self, run: &RunRecord, station: &str, gate: &str) -> GateResult {
        let found = self
            .0
            .map
            .read()
            .ok()
            .and_then(|m| m.gates.get(gate).cloned());
        let Some((_, function)) = found else {
            return GateResult {
                pass: false,
                detail: format!("no enabled plugin provides the gate `{gate}`"),
            };
        };
        let (mut input, cwd) = self.0.input(run, station);
        input["gate"] = json!(gate);
        // writes:true 而不是 false：false 时引擎把会改东西的命令（`cargo test` 之类）一律拒掉，规则也放不开，
        // 跑测试的门禁就做不成。经文件工具能写的仍只是函数自己声明的 `writes`（门禁函数声明为空，安装时披露过），
        // 命令按项目的规则判，要问人的照样失败。
        match self.0.call(&function, input, cwd, true).await {
            Ok(v) => match serde_json::from_value::<GateResult>(v.clone()) {
                Ok(g) => g,
                Err(_) => GateResult {
                    pass: false,
                    detail: format!("{function} answered {v}, not {{pass, detail}}"),
                },
            },
            Err(e) => {
                tracing::warn!(run = %run.id, station, gate, "{e}");
                GateResult {
                    pass: false,
                    detail: e,
                }
            }
        }
    }
}

pub struct PluginPrograms(pub Arc<PluginCalls>);

#[async_trait]
impl Programs for PluginPrograms {
    async fn run(
        &self,
        step: &str,
        run: &RunRecord,
        item: Option<&Value>,
    ) -> Result<Value, String> {
        let found = self
            .0
            .map
            .read()
            .ok()
            .and_then(|m| m.steps.get(step).cloned());
        let Some((_, function)) = found else {
            return Err(format!("no enabled plugin provides the step `{step}`"));
        };
        let station = run
            .stations
            .iter()
            .find(|(_, s)| s.status == harnix_flow::StationStatus::Working)
            .map(|(id, _)| id.clone())
            .unwrap_or_default();
        let (mut input, cwd) = self.0.input(run, &station);
        input["step"] = json!(step);
        input["item"] = item.cloned().unwrap_or(Value::Null);
        self.0.call(&function, input, cwd, true).await
    }
}
