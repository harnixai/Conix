//! 宿主库 `conix.*` 的处理者（plugins.md §5.1）：插件命令与插件函数在引擎的 QuickJS 里调它，
//! 执行走 Hub 和端口命令同一套流程代码，界面照常收到事件。
//!
//! 谁能调按调用上下文定，不按包名（包名是包自报的）：
//! - 门禁与步骤里（`via: host`）或没有会话时：只开放读的函数——一次门禁检查不该反过来推进流程。
//! - 流程运行里的会话：只看得到自己的运行，不能开新运行；作业会话：对流程只能看。
//! - 其余（人开的会话）：都能用。`flow.start` 不再问人：调它的插件命令不是只读子命令时，那次 `RunExt`
//!   已经过了 Agix 的权限判定。

use serde_json::{Value, json};
pub use wire::hostlib::HostLibHandler;
use wire::hostlib::{HostLibCall, HostLibError};

use port::{Command, Reply};

use crate::hub::Hub;

/// 发起调用的那一方能做什么。
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scope {
    /// 只能读，且只看这一次运行（流程站的会话）。
    Run(String),
    /// 只能读。
    Read,
    /// 都能做。
    Full,
}

fn scope(hub: &Hub, call: &HostLibCall) -> Scope {
    if call.via == "host" {
        return Scope::Read;
    }
    let Some(sid) = &call.session_id else {
        return Scope::Read;
    };
    match hub.origin_of(sid) {
        Some(port::Origin::FlowStation { run, .. }) => Scope::Run(run.0),
        Some(port::Origin::Job { .. }) => Scope::Read,
        Some(_) => Scope::Full,
        // 认不出的会话（子 Agent 的侧链也报根会话，这里不该出现）：按最紧的算。
        None => Scope::Read,
    }
}

pub async fn handle(hub: Hub, call: HostLibCall) -> Result<Value, HostLibError> {
    let scope = scope(&hub, &call);
    let own = |run: &str| match &scope {
        Scope::Run(r) => r == run,
        _ => true,
    };
    let port = |cmd: Command| {
        let hub = hub.clone();
        async move {
            hub.flow_command(cmd)
                .await
                .map_err(|e| HostLibError::new("failed", e.to_string()))
        }
    };
    match call.path.as_str() {
        "flow.defs" => match port(Command::ListFlows).await? {
            Reply::Flows(defs) => Ok(json!(
                defs.into_iter()
                    .map(
                        |f| json!({"name": f.name, "title": f.title, "origin": f.origin,
                                    "source": f.source})
                    )
                    .collect::<Vec<_>>()
            )),
            r => Err(unexpected(r)),
        },
        "flow.runs" => {
            let flows: Vec<String> = match call.input["flow"].as_str() {
                Some(f) => vec![f.to_string()],
                None => match port(Command::ListFlows).await? {
                    Reply::Flows(defs) => defs.into_iter().map(|f| f.name).collect(),
                    r => return Err(unexpected(r)),
                },
            };
            let active = call.input["active"].as_bool().unwrap_or(false);
            let mut out = vec![];
            for flow in flows {
                let Reply::Runs(runs) = port(Command::ListRuns { flow }).await? else {
                    continue;
                };
                for r in runs.into_iter().filter(|r| own(&r.id.0)) {
                    if active
                        && matches!(
                            r.status,
                            port::RunStatus::Finished | port::RunStatus::Cancelled
                        )
                    {
                        continue;
                    }
                    out.push(serde_json::to_value(&r).unwrap_or_default());
                }
            }
            Ok(Value::Array(out))
        }
        "flow.run" => {
            let id = call.input["run"]
                .as_str()
                .or_else(|| call.input.as_str())
                .ok_or_else(|| {
                    HostLibError::new("invalid_input", "give the run: \"<id>\" or {run}")
                })?;
            if !own(id) {
                return Err(HostLibError::new(
                    "denied",
                    "a flow station's session sees only its own run",
                ));
            }
            match port(Command::GetRun {
                run: port::RunId(id.to_string()),
            })
            .await?
            {
                Reply::Run(v) => {
                    serde_json::to_value(*v).map_err(|e| HostLibError::new("failed", e.to_string()))
                }
                r => Err(unexpected(r)),
            }
        }
        "flow.current" => Ok(
            match call.session_id.as_deref().and_then(|s| hub.origin_of(s)) {
                Some(port::Origin::FlowStation { run, station, .. }) => {
                    json!({"run": run.0, "station": station.0})
                }
                _ => Value::Null,
            },
        ),
        "flow.start" => {
            if scope != Scope::Full {
                return Err(HostLibError::new(
                    "denied",
                    "only a person's session may start a flow run (not a gate, a step, a flow station or a job)",
                ));
            }
            let flow = call.input["flow"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| HostLibError::new("invalid_input", "flow is required"))?;
            let subject = call.input["subject"]
                .as_str()
                .or_else(|| call.input["title"].as_str())
                .unwrap_or(flow)
                .to_string();
            let inputs = call.input.get("inputs").cloned().unwrap_or(Value::Null);
            match port(Command::StartRun {
                flow: flow.to_string(),
                subject,
                inputs,
                step: false,
                session: None,
            })
            .await?
            {
                Reply::RunStarted(s) => Ok(json!({"run": s.id.0, "subject": s.subject})),
                r => Err(unexpected(r)),
            }
        }
        "plugin.call" => plugin_call(&hub, &call).await,
        other => Err(HostLibError::new(
            "unknown",
            format!("conix.{other} is not a function"),
        )),
    }
}

/// 插件命令调自己包里的插件函数（不带会话）。为什么要有：流程站的会话里，派工器要逐次判 `Write`、`Edit`、
/// `Bash`（会话上有这几样的 ask 规则），而插件命令里的子调用问不了人，命令写自己的数据就一律失败。
/// 经这里，写入仍走 Agix 的权限：只写得了函数声明的 `writes`（安装时披露过），不带会话也就没有站的 ask。
/// 只许调自己包的函数；门禁与步骤里（`via: host`，本来就是函数）不许再调，免得绕圈。
async fn plugin_call(hub: &Hub, call: &HostLibCall) -> Result<Value, HostLibError> {
    if call.via == "host" {
        return Err(HostLibError::new(
            "denied",
            "a plugin function cannot call plugin functions through the host",
        ));
    }
    let function = call.input["function"]
        .as_str()
        .ok_or_else(|| HostLibError::new("invalid_input", "function is required"))?;
    if !function.starts_with(&format!("{}.", call.plugin)) {
        return Err(HostLibError::new(
            "denied",
            format!(
                "plugin `{}` may call only its own functions, not `{function}`",
                call.plugin
            ),
        ));
    }
    let cwd = call
        .project_root
        .as_ref()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|| hub.project().to_string());
    let r = hub
        .call(
            "plugin.call",
            json!({"function": function, "input": call.input.get("input").cloned().unwrap_or(Value::Null),
                   "cwd": cwd, "writes": true, "timeout_ms": super::CALL_TIMEOUT_MS}),
        )
        .await
        .map_err(|e| HostLibError::new("failed", e.to_string()))?;
    if r["ok"] == true {
        Ok(r["value"].clone())
    } else {
        Err(HostLibError::new(
            r["error"]["code"].as_str().unwrap_or("failed"),
            r["error"]["message"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
        ))
    }
}

fn unexpected(r: Reply) -> HostLibError {
    HostLibError::new("failed", format!("unexpected reply {r:?}"))
}
