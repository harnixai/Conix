//! The flow's tools, which a host registers to every flow session: one set, answered here. A tool
//! finds the task it serves by the calling session's id; a session that serves none is told so.
//!
//! | tool | does |
//! |---|---|
//! | `flow_state` | the run as far as this task is concerned: the station, what is still unmet |
//! | `flow_submit {output}` | hands the output in under the session's claim; problems come back to fix |
//! | `flow_propose {kind, to?, reason}` | writes a `proposal` note; a person accepts it or not |
//! | `flow_command {command, args}` | runs a command of the run's bundle (in Conix): it writes only the run's directory and the bundle's data |

use std::sync::Weak;

use serde_json::{json, Value};

use super::host::{AgentTaskStatus, Note, SessionId, ToolSpec};
use super::state::Binding;
use super::Runner;
use crate::kernel::def::Executor;
use crate::kernel::run::{Actor, AgentDelivery};
use crate::kernel::FlowError;

pub const TOOL_STATE: &str = "flow_state";
pub const TOOL_SUBMIT: &str = "flow_submit";
pub const TOOL_PROPOSE: &str = "flow_propose";
pub const TOOL_COMMAND: &str = "flow_command";
/// How long a bundle's command may run.
const COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What a tool answers: text for the model, and whether it is an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReply {
    pub text: String,
    pub is_error: bool,
}

impl ToolReply {
    pub(crate) fn text(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: false }
    }
    pub(crate) fn error(text: impl Into<String>) -> Self {
        Self { text: text.into(), is_error: true }
    }
}

/// The flow's three tools: their specs, for the host to register; their calls, for the host to
/// route here.
pub struct FlowTools {
    runner: Weak<Runner>,
    specs: Vec<ToolSpec>,
}

impl FlowTools {
    pub(crate) fn new(runner: Weak<Runner>) -> Self {
        Self { runner, specs: tool_specs() }
    }

    /// `flow_state`, `flow_submit`, `flow_propose`. None of them changes anything outside the
    /// flow's own record, and a headless session has nobody to ask: all read-only.
    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }

    /// Answer a call of `tool` from the session `session` (the id the host got from `open`).
    pub async fn call(&self, session: &SessionId, tool: &str, input: Value) -> ToolReply {
        let Some(rn) = super::upgrade(&self.runner) else {
            return ToolReply::error("the flow runner has stopped");
        };
        let Some(b) = rn.state.binding(session) else {
            return ToolReply::error(
                "This session is not working on a flow task (or its task is over): there is nothing \
                 to read or hand in here.",
            );
        };
        match tool {
            TOOL_STATE => state(&rn, &b).await,
            TOOL_SUBMIT => submit(&rn, &b, session, input).await,
            TOOL_PROPOSE => propose(&rn, &b, session, input).await,
            TOOL_COMMAND => command(&rn, &b, input).await,
            other => ToolReply::error(format!("no flow tool `{other}`")),
        }
    }
}

/// The flow's three tools' specs, as a host registers them: the same whether or not a runner is
/// there yet.
pub fn tool_specs() -> Vec<ToolSpec> {
    vec![
        ToolSpec {
            name: TOOL_STATE.into(),
            description: "The flow task this session is working on: the run, the station and its round, your \
                 task and item, the output contract, what you handed in, and which exit conditions are \
                 still not met."
                .into(),
            input_schema: json!({"type": "object", "properties": {}}),
            read_only: true,
        },
        ToolSpec {
            name: TOOL_SUBMIT.into(),
            description: "Hand in this task's output: one JSON value satisfying the station's output contract. \
                 Problems come back for you to fix; submit again until it is accepted. You may submit \
                 again until the station passes; the last submission counts."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["output"],
                "properties": {
                    "output": {"description": "The output, as the flow protocol's contract says."}
                }
            }),
            read_only: true,
        },
        ToolSpec {
            name: TOOL_PROPOSE.into(),
            description: "Propose that a person rework an upstream station (kind `rework`, `to` the station) or \
                 cancel the run (kind `cancel`), with your reason. It is shown to people as a card; \
                 nothing happens unless one of them accepts it."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["kind", "reason"],
                "properties": {
                    "kind": {"type": "string", "enum": ["rework", "cancel"]},
                    "to": {"type": "string", "description": "With `rework`: the station to go back to."},
                    "reason": {"type": "string"}
                }
            }),
            read_only: true,
        },
        ToolSpec {
            name: TOOL_COMMAND.into(),
            description: "Run one of this flow's own commands (the flow protocol names them), as on a command \
                 line: `args` a list of strings (`[\"help\"]` says what it does). A command reads the project \
                 and writes only this run's directory and the flow's data directory; structured data the \
                 flow keeps is written with them."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {"type": "string"},
                    "args": {"type": "array", "items": {"type": "string"}}
                }
            }),
            // It writes, but only where the run keeps its own: no person is asked.
            read_only: false,
        },
    ]
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Run one of the run's bundle's commands (pinned when the run started) for the task's session.
async fn command(rn: &Runner, b: &Binding, input: Value) -> ToolReply {
    let engine = rn.engine();
    let rec = match engine.record(&b.run).await {
        Ok(r) => r,
        Err(e) => return ToolReply::error(e.to_string()),
    };
    let have = || rec.parts.commands.keys().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ");
    let Some(name) = input.get("command").and_then(Value::as_str).map(str::trim) else {
        return ToolReply::error(format!("say which command: {{command, args}}; this flow has {}", have()));
    };
    let Some(source) = rec.parts.commands.get(name) else {
        return ToolReply::error(if rec.parts.commands.is_empty() {
            format!("this flow has no commands (no `{name}`)")
        } else {
            format!("no command `{name}`; this flow has {}", have())
        });
    };
    let args: Vec<String> = match input.get("args") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect(),
        Some(Value::String(s)) => s.split_whitespace().map(String::from).collect(),
        Some(other) => return ToolReply::error(format!("args is a list of strings, not {other}")),
    };
    let Some(ws) = engine.workspace_for(&rec) else {
        return ToolReply::error("the run has no place it works in");
    };
    let reader = match super::files::Reader::new(&ws) {
        Ok(r) => r,
        Err(e) => return ToolReply::error(e.0),
    };
    let roots: Vec<std::path::PathBuf> = [&rec.output_dir, &rec.data_dir].into_iter().flatten().map(std::path::PathBuf::from).collect();
    let denied: Vec<std::path::PathBuf> = super::project::process_dir(engine, &rec).into_iter().collect();
    let rel = |p: &Option<String>| -> Value {
        p.as_deref().map_or(Value::Null, |p| {
            Value::String(std::path::Path::new(p).strip_prefix(&ws).map(|r| r.to_string_lossy().into_owned()).unwrap_or_else(|_| p.to_string()))
        })
    };
    let given = json!({
        "command": name, "args": args, "run": rec.id, "flow": rec.def.name, "station": b.station, "task": b.task,
        "subject": rec.subject, "inputs": rec.inputs, "output_dir": rel(&rec.output_dir), "data_dir": rel(&rec.data_dir),
    });
    let writer = super::script::Writer { workspace: ws.clone(), roots, denied, written: Default::default() };
    let out = super::script::command(source.clone(), given, std::sync::Arc::new(reader), std::sync::Arc::new(writer), COMMAND_TIMEOUT).await;
    let mut text = out.text;
    if !out.written.is_empty() {
        let mut w = out.written.clone();
        w.dedup();
        text.push_str(&format!("\n(written: {})", w.join(", ")));
    }
    if out.ok { ToolReply::text(text) } else { ToolReply::error(text) }
}

async fn state(rn: &Runner, b: &Binding) -> ToolReply {
    // The whole record, to tell whether the claim is still this session's; only the fields below
    // leave here, never a token.
    let full = match rn.engine().record(&b.run).await {
        Ok(r) => r,
        Err(e) => return ToolReply::error(e.to_string()),
    };
    let current = full
        .stations
        .get(&b.station)
        .and_then(|s| s.task(&b.task))
        .is_some_and(|t| t.lease.as_ref().is_some_and(|l| l.token == b.token));
    let view = crate::kernel::engine::view(full);
    let rec = &view.record;
    let st = rec.stations.get(&b.station);
    let task = st.and_then(|s| s.task(&b.task));
    let sdef = rec.def.station(&b.station);
    let state = json!({
        "run": rec.id,
        "flow": rec.flow,
        "subject": rec.subject,
        "run_status": rec.status,
        "station": b.station,
        "round": st.map(|s| s.round),
        "station_status": st.map(|s| s.status),
        "task": b.task,
        "task_status": task.map(|t| t.status),
        "item": task.and_then(|t| t.item.clone()),
        "submitted": task.and_then(|t| t.output.clone()),
        "contract": sdef.and_then(|s| s.output.clone()),
        "exits": sdef.map(|s| s.exit.iter().map(|e| e.label()).collect::<Vec<_>>()),
        "unmet": view.unmet.get(&b.station),
        "permissions": b.permissions,
        "current": current,
    });
    ToolReply::text(pretty(&state))
}

async fn submit(rn: &std::sync::Arc<Runner>, b: &Binding, session: &SessionId, input: Value) -> ToolReply {
    let output = input.get("output").cloned().unwrap_or(Value::Null);
    let by = Actor::agent(session.clone());
    let agent = match sub_agent(rn, b, session).await {
        Ok(a) => a,
        Err(why) => return ToolReply::error(why),
    };
    let result = match agent {
        Some(a) => rn.engine().submit_from_agent(&b.run, &b.station, &b.task, &b.token, output, by, a).await,
        None => rn.engine().submit(&b.run, &b.station, &b.task, &b.token, output, by).await,
    };
    match result {
        Ok(o) => {
            let mut status = o.value.get("station").cloned().unwrap_or(Value::Null);
            let checking = o.value.get("checking").and_then(Value::as_bool).unwrap_or(false);
            let sent_back = o.value.get("sent_back").and_then(Value::as_str).map(String::from);
            rn.apply_for(&b.run, o).await;
            // The station's `task_exit` gates answer before the session does: what they found is
            // this call's reply, as a contract's refusal is.
            let key = super::state::TaskKey { run: b.run.clone(), station: b.station.clone(), round: b.round, task: b.task.clone() };
            let checked = match (sent_back, checking) {
                (Some(why), _) => super::steps::Checked::SentBack(why),
                (None, true) => super::steps::wait_checked(rn, &key, &b.token).await,
                (None, false) => super::steps::Checked::Taken,
            };
            match checked {
                super::steps::Checked::Taken if checking => {
                    if let Ok(rec) = rn.engine().record(&b.run).await {
                        status = serde_json::to_value(rec.stations.get(&b.station).map(|s| s.status)).unwrap_or(Value::Null);
                    }
                }
                super::steps::Checked::Taken => {}
                super::steps::Checked::SentBack(why) => {
                    return ToolReply::error(format!(
                        "Not accepted: the checks of your task did not pass: {why}. Fix the work and call {TOOL_SUBMIT} again."
                    ))
                }
                super::steps::Checked::Gone | super::steps::Checked::Open => {
                    return ToolReply::error("Not accepted: your claim on this task is no longer current; stop working on it.")
                }
            }
            ToolReply::text(format!(
                "Accepted. Station `{}` is now {}. If it is still working, others' tasks or its checks \
                 are outstanding; your part is done unless you want to improve your output.",
                b.station,
                status.as_str().unwrap_or("unknown")
            ))
        }
        Err(FlowError::Contract(m)) => ToolReply::error(format!("Not accepted: {m}. Fix the output and call {TOOL_SUBMIT} again.")),
        Err(e @ FlowError::Invalid(_)) => ToolReply::error(format!("Not accepted: {e}. The station is not taking submissions now.")),
        Err(e) => ToolReply::error(format!("Not accepted: {e}. Your claim on this task is no longer current; stop working on it.")),
    }
}

/// For a sub-agent task: the sub-agent the output came from, which must have ended with an
/// answer — otherwise the main session did the task itself, or hands in before it is done.
/// `Ok(None)`: not a sub-agent task, or the host cannot tell.
async fn sub_agent(rn: &Runner, b: &Binding, session: &SessionId) -> Result<Option<AgentDelivery>, String> {
    let rec = rn.engine().record(&b.run).await.map_err(|e| e.to_string())?;
    let is_agent = rec
        .def
        .station(&b.station)
        .and_then(|s| s.task_spec(&b.task))
        .is_some_and(|t| matches!(t.executor, Executor::Agent { .. }));
    if !is_agent {
        return Ok(None);
    }
    let tag = crate::agent_tag(&b.run, &b.station, &b.task);
    // The `Agent` tool's result can reach the model a moment before the host hears the task end.
    let mut seen = None;
    for i in 0..SUB_AGENT_SETTLE_TRIES {
        if i > 0 {
            tokio::time::sleep(SUB_AGENT_SETTLE_STEP).await;
        }
        let tasks = match rn.host().agent_tasks(session, &tag).await {
            Ok(Some(t)) => t,
            Ok(None) => return Ok(None),
            Err(e) => return Err(format!("Not accepted: the sub-agent could not be looked up ({e}); try again.")),
        };
        if let Some(t) = tasks.iter().rev().find(|t| t.status == AgentTaskStatus::Done) {
            return Ok(Some(AgentDelivery {
                task: t.id.clone(),
                session: t.session.clone(),
                status: t.status.as_str().into(),
                verified: true,
                usage: t.usage,
            }));
        }
        let running = tasks.iter().any(|t| t.status == AgentTaskStatus::Running);
        seen = Some(running);
        if !running {
            break;
        }
    }
    Err(match seen {
        Some(true) => format!(
            "Not accepted: the sub-agent `{tag}` is still working. Wait for its answer, check it against the contract, then call {TOOL_SUBMIT}."
        ),
        _ => format!(
            "Not accepted: this task is a sub-agent's, and no sub-agent named `{tag}` has answered. Call the `Agent` tool with \
             `description: \"{tag}\"` to have it done, then hand in its result."
        ),
    })
}

const SUB_AGENT_SETTLE_TRIES: u32 = 6;
const SUB_AGENT_SETTLE_STEP: std::time::Duration = std::time::Duration::from_millis(500);

async fn propose(rn: &Runner, b: &Binding, session: &SessionId, input: Value) -> ToolReply {
    let kind = input.get("kind").and_then(Value::as_str).unwrap_or("");
    let reason = input.get("reason").and_then(Value::as_str).unwrap_or("").trim().to_string();
    let to = input.get("to").and_then(Value::as_str).filter(|s| !s.is_empty()).map(String::from);
    if !matches!(kind, "rework" | "cancel") {
        return ToolReply::error("`kind` is `rework` or `cancel`");
    }
    if kind == "rework" && to.is_none() {
        return ToolReply::error("a rework proposal names the station to go back to in `to`");
    }
    if reason.is_empty() {
        return ToolReply::error("a proposal needs a reason");
    }
    let payload = json!({
        "run": b.run, "station": b.station, "task": b.task,
        "kind": kind, "to": to, "reason": reason,
    });
    match rn.host().note(session, Note { event: "proposal".into(), payload }).await {
        Ok(()) => ToolReply::text(
            "Proposed. A person will accept or dismiss it; carry on with your task meanwhile, or stop \
             if you cannot do it without the change.",
        ),
        Err(e) => ToolReply::error(format!("The proposal was not recorded: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::kernel::host::WorkItem;
    use crate::runner::{Runner, ScriptedHost, SessionHost};
    use crate::{CreateIn, Engine, MemPacks, MemStore, StationStatus};

    const FLOW: &str = "{name: f, stations: [\
        {id: a, executor: {kind: session, scene: flow-harnix}, skill: do, output: {type: object, required: [n]}, exit: [{kind: output}]},\
        {id: b, exit: [{kind: confirm}]}]}";

    async fn bound() -> (Arc<Runner>, Arc<ScriptedHost>, String, Binding) {
        bound_to(FLOW).await
    }

    async fn bound_to(flow: &str) -> (Arc<Runner>, Arc<ScriptedHost>, String, Binding) {
        let packs = MemPacks::new().scene("flow-harnix").skill("do", "Do it.").flow(flow);
        let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), Arc::new(packs)).with_submit_scenes(["flow-harnix"]));
        let host = ScriptedHost::new();
        let rn = Runner::for_tests(engine.clone(), host.clone());
        let o = engine.create(CreateIn { flow: "f".into(), subject: "s".into(), ..Default::default() }, Actor::device("d")).await.unwrap();
        let WorkItem::Task { run, station, round, task, token, permissions, .. } = &o.work[0] else { panic!() };
        let b = Binding { run: run.clone(), station: station.clone(), round: *round, task: task.clone(), token: token.clone(), permissions: permissions.clone() };
        let id = host.open(crate::runner::OpenSpec::new("flow:r/a/a@t", run, "flow-harnix", crate::Delivery::FlowSubmit)).await.unwrap().id;
        rn.state.bind(&id, b.clone());
        (rn, host, id, b)
    }

    #[test]
    fn the_tools_touch_nothing_but_a_bundles_command_its_own() {
        let rn = Runner::for_tests(Arc::new(Engine::new(Arc::new(MemStore::new()), Arc::new(MemPacks::new()))), ScriptedHost::new());
        let names: Vec<&str> = rn.tools().specs().iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, [TOOL_STATE, TOOL_SUBMIT, TOOL_PROPOSE, TOOL_COMMAND]);
        assert!(rn.tools().specs().iter().all(|t| t.read_only == (t.name != TOOL_COMMAND)));
    }

    #[tokio::test]
    async fn a_session_that_serves_no_task_is_told_so() {
        let rn = Runner::for_tests(Arc::new(Engine::new(Arc::new(MemStore::new()), Arc::new(MemPacks::new()))), ScriptedHost::new());
        let out = rn.tools().call(&"nobody".to_string(), TOOL_STATE, json!({})).await;
        assert!(out.is_error && out.text.contains("not working on a flow task"));
        let out = rn.tools().call(&"nobody".to_string(), "flow_nope", json!({})).await;
        assert!(out.is_error);
    }

    #[tokio::test]
    async fn the_state_and_a_hand_in_are_the_tasks_and_a_proposal_is_a_note() {
        let (rn, host, id, b) = bound().await;
        let state = rn.tools().call(&id, TOOL_STATE, json!({})).await;
        assert!(!state.is_error);
        let v: Value = serde_json::from_str(&state.text).unwrap();
        assert_eq!((v["station"].as_str(), v["current"].as_bool()), (Some("a"), Some(true)));
        assert!(v.get("token").is_none());
        let refused = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"m": 1}})).await;
        assert!(refused.is_error && refused.text.starts_with("Not accepted: "), "{}", refused.text);
        let bad = rn.tools().call(&id, TOOL_PROPOSE, json!({"kind": "rework", "reason": "x"})).await;
        assert!(bad.is_error && bad.text.contains("`to`"));
        let ok = rn.tools().call(&id, TOOL_PROPOSE, json!({"kind": "cancel", "reason": "no point"})).await;
        assert!(!ok.is_error, "{}", ok.text);
        let notes = host.notes();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].1.event, "proposal");
        assert_eq!(notes[0].1.payload["reason"], "no point");
        assert_eq!(notes[0].1.payload["run"], b.run);
        let taken = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"n": 1}})).await;
        assert!(!taken.is_error && taken.text.starts_with("Accepted. Station `a` is now passed"), "{}", taken.text);
        assert_eq!(rn.engine().record(&b.run).await.unwrap().stations["a"].status, StationStatus::Passed);
        // The station passed: the session serves no task any more.
        let over = rn.tools().call(&id, TOOL_STATE, json!({})).await;
        assert!(over.is_error && over.text.contains("not working on a flow task"), "{}", over.text);
    }

    const AGENT_FLOW: &str = "{name: f, main: {scene: flow-harnix}, stations: [\
        {id: a, executor: {kind: agent}, skill: do, output: {type: object, required: [n]}, exit: [{kind: output}]},\
        {id: b, exit: [{kind: confirm}]}]}";

    fn agent(id: &str, status: AgentTaskStatus) -> crate::runner::AgentTask {
        let usage = crate::Usage { output_tokens: 7, ..Default::default() };
        crate::runner::AgentTask { id: id.into(), session: Some(format!("side-{id}")), status, usage }
    }

    #[tokio::test(start_paused = true)]
    async fn a_sub_agent_tasks_output_is_taken_once_its_sub_agent_has_answered() {
        let (rn, host, id, b) = bound_to(AGENT_FLOW).await;
        let tag = crate::agent_tag(&b.run, &b.station, &b.task);
        host.tells_agent_tasks();
        // Done without a sub-agent: refused, told how.
        let out = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"n": 1}})).await;
        assert!(out.is_error && out.text.contains(&format!("description: \"{tag}\"")), "{}", out.text);
        host.agent_task(&tag, agent("t1", AgentTaskStatus::Failed));
        host.agent_task(&tag, agent("t2", AgentTaskStatus::Running));
        let out = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"n": 1}})).await;
        assert!(out.is_error && out.text.contains("still working"), "{}", out.text);
        host.agent_task(&tag, agent("t3", AgentTaskStatus::Done));
        let out = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"n": 1}})).await;
        assert!(!out.is_error, "{}", out.text);
        let rec = rn.engine().record(&b.run).await.unwrap();
        let d = rec.stations["a"].task(&b.task).unwrap().delivery.clone().unwrap();
        assert_eq!(d.executor, "agent");
        let a = d.agent.unwrap();
        assert_eq!((a.task.as_str(), a.session.as_deref(), a.verified, a.usage.output_tokens), ("t3", Some("side-t3"), true, 7));
    }

    #[tokio::test]
    async fn a_host_that_cannot_tell_sub_agents_takes_the_sessions_word() {
        let (rn, _host, id, b) = bound_to(AGENT_FLOW).await;
        let out = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"n": 1}})).await;
        assert!(!out.is_error, "{}", out.text);
        let rec = rn.engine().record(&b.run).await.unwrap();
        let d = rec.stations["a"].task(&b.task).unwrap().delivery.clone().unwrap();
        assert!(d.agent.is_none() && d.by.kind == crate::AGENT_KIND);
    }
}
