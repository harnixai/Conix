//! The flow's tools, which a host registers to every flow session: one set, answered here. A tool
//! finds the task it serves by the calling session's id; a session that serves none is told so.
//!
//! | tool | does |
//! |---|---|
//! | `flow_state` | the run as far as this task is concerned: the station, what is still unmet |
//! | `flow_submit {output}` | hands the output in under the session's claim; problems come back to fix |
//! | `flow_propose {kind, to?, reason}` | records a proposal on the run (and sends a `proposal` note); a person accepts it or not |
//! | `flow_command {command, args, run?}` | runs a command of a bundle (in Conix): in a run's session the run's own; for the main conversation `bundle:command`. It writes only the run's directory and the bundle's data; outside a run a model only runs the read-only subcommands |
//!
//! `flow_command` is the one tool here a session with no task may call ([`FlowTools::call_in`]);
//! [`command_tool_spec`] is its spec alone, for a host that offers it to such a session next to
//! the authoring tools without registering the other three — a session is never given two tools of one name.

use std::sync::Weak;

use serde_json::{json, Value};

use super::host::{AgentTaskStatus, Note, SessionId, ToolSpec};
use super::state::Binding;
use super::Runner;
use crate::kernel::def::Executor;
use crate::kernel::run::{Actor, AgentDelivery, ProposalKind};
use crate::kernel::FlowError;

pub const TOOL_STATE: &str = "flow_state";
pub const TOOL_SUBMIT: &str = "flow_submit";
pub const TOOL_PROPOSE: &str = "flow_propose";
pub const TOOL_COMMAND: &str = "flow_command";

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
        self.call_in(session, tool, input, None).await
    }

    /// [`FlowTools::call`] with the project the session works in: `flow_command` is answered for a
    /// session that serves no task (the main conversation) too, outside a run in that project.
    /// The other tools are a task's own and need its binding.
    pub async fn call_in(&self, session: &SessionId, tool: &str, input: Value, project: Option<&std::path::Path>) -> ToolReply {
        let Some(rn) = super::upgrade(&self.runner) else {
            return ToolReply::error("the flow runner has stopped");
        };
        let b = rn.state.binding(session);
        if tool == TOOL_COMMAND {
            return command(&rn, session, b.as_ref(), project, input).await;
        }
        let Some(b) = b else {
            return ToolReply::error(
                "This session is not working on a flow task (or its task is over): there is nothing \
                 to read or hand in here.",
            );
        };
        match tool {
            TOOL_STATE => state(&rn, &b).await,
            TOOL_SUBMIT => submit(&rn, &b, session, input).await,
            TOOL_PROPOSE => propose(&rn, &b, session, input).await,
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
            description: "Propose that a person send the run back to a station (kind `rework`, `to` one of the \
                 stations this one may go back to, as the flow protocol lists them — a problem born in an \
                 earlier station goes back there) or cancel the run (kind `cancel`), with your reason. It is \
                 recorded on the run and shown to people; nothing happens unless one of them accepts it."
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
            description: "Run one of a flow bundle's commands, as on a command line: `args` a list of strings \
                 (`[\"help\"]` or no args says what it does and which subcommands only read, without \
                 running it). In a flow run's session the command is the run's bundle's and `run` is this \
                 run; elsewhere name it `bundle:command` (as `xforge:xspec`). A command reads the project \
                 and writes only the run's directory and the bundle's data directory; structured data the \
                 bundle keeps is written with them. Outside a run only the read-only subcommands are for \
                 you: for the rest, give a `run` of that bundle, or ask the person."
                .into(),
            input_schema: json!({
                "type": "object",
                "required": ["command"],
                "properties": {
                    "command": {"type": "string", "description": "`name` in a run's session, else `bundle:name`."},
                    "args": {"type": "array", "items": {"type": "string"}},
                    "run": {"type": "string", "description": "The flow run it is for (its pinned command, its directory); only outside a run's own session."}
                }
            }),
            // It writes, but only where the run keeps its own: no person is asked.
            read_only: false,
        },
    ]
}

/// The spec of `flow_command` alone (also the last of [`tool_specs`]).
pub fn command_tool_spec() -> ToolSpec {
    tool_specs().into_iter().find(|t| t.name == TOOL_COMMAND).expect("flow_command is one of the tools")
}

fn pretty(v: &Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|_| v.to_string())
}

/// Run a bundle's command for a session: a station's (its run's pinned commands, the run's bundle
/// the default), or the main conversation's (`bundle:command`, or the run its main session is of).
/// `b` is the task the session serves, if any.
async fn command(rn: &Runner, session: &SessionId, b: Option<&Binding>, project: Option<&std::path::Path>, input: Value) -> ToolReply {
    let engine = rn.engine();
    let Some(name) = input.get("command").and_then(Value::as_str).map(str::trim) else {
        return ToolReply::error("say which command: {command, args} — `bundle:command` outside a run (`args: [\"help\"]` says what it does)");
    };
    let args: Vec<String> = match input.get("args") {
        None | Some(Value::Null) => vec![],
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string())).collect(),
        Some(Value::String(s)) => s.split_whitespace().map(String::from).collect(),
        Some(other) => return ToolReply::error(format!("args is a list of strings, not {other}")),
    };
    let named = input.get("run").and_then(Value::as_str).map(str::trim).filter(|r| !r.is_empty());
    // A station's session works for its own run only: naming another is a mistake, not a way into
    // someone else's directory.
    if let (Some(b), Some(r)) = (b, named) {
        if r != b.run {
            return ToolReply::error(format!("this session works for run {}: `run` is that one here", b.run));
        }
    }
    let run = named.map(String::from).or_else(|| b.map(|b| b.run.clone())).or_else(|| rn.state.watched_run(session));
    let run_bundle = match &run {
        Some(r) => match engine.record(r).await {
            Ok(rec) => rec.def.bundle.clone(),
            Err(e) => return ToolReply::error(e.to_string()),
        },
        None => None,
    };
    let Some((bundle, command)) = super::commands::split_command(name, run_bundle.as_deref()) else {
        return ToolReply::error(format!("`{name}`: outside a run a command is named with its bundle, `bundle:command` (`xforge:xspec`)"));
    };
    let mut call = super::commands::CommandCall::new(bundle, command, args, super::commands::CommandBy::model(session.clone()));
    call.run = run;
    call.project_root = project.map(std::path::PathBuf::from);
    call.station = b.map(|b| b.station.clone());
    call.task = b.map(|b| b.task.clone());
    match rn.run_command(call).await {
        Ok(r) => ToolReply::text(r.shown()),
        Err(e) => ToolReply::error(e),
    }
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

async fn propose(rn: &std::sync::Arc<Runner>, b: &Binding, session: &SessionId, input: Value) -> ToolReply {
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
    let pk = if kind == "rework" { ProposalKind::Rework } else { ProposalKind::Cancel };
    // Recorded on the run first: a proposal the run does not hold is one nobody can accept.
    let o = match rn
        .engine()
        .propose(&b.run, &b.station, &b.task, &b.token, pk, to.as_deref(), &reason, Actor::agent(session.clone()))
        .await
    {
        Ok(o) => o,
        Err(e) => return ToolReply::error(format!("The proposal was not recorded: {e}")),
    };
    let id = o.value.pointer("/proposal/id").and_then(Value::as_str).unwrap_or_default().to_string();
    rn.apply_for(&b.run, o).await;
    let payload = json!({
        "run": b.run, "station": b.station, "task": b.task, "id": id,
        "kind": kind, "to": to, "reason": reason,
    });
    // The host's note is how a person hears of it now; the run keeps it whether or not they do.
    if let Err(e) = rn.host().note(session, Note { event: "proposal".into(), payload }).await {
        tracing::warn!(run = %b.run, error = %e, "proposal note not delivered");
    }
    ToolReply::text(format!(
        "Proposed ({id}). A person will accept or dismiss it; carry on with your task meanwhile, or stop \
         if you cannot do it without the change."
    ))
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
        let b = Binding { run: run.clone(), station: station.clone(), round: *round, task: task.clone(), token: token.clone(), permissions: permissions.clone(), writes: None };
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
        assert_eq!(notes[0].1.payload["id"], "p1");
        // Recorded on the run, open; a rework to a point the station has not is refused, naming none.
        let rec = rn.engine().record(&b.run).await.unwrap();
        assert_eq!((rec.proposals.len(), rec.proposals[0].status), (1, crate::kernel::run::ProposalStatus::Open));
        let nowhere = rn.tools().call(&id, TOOL_PROPOSE, json!({"kind": "rework", "to": "b", "reason": "x"})).await;
        assert!(nowhere.is_error && nowhere.text.contains("names no station to go back to"), "{}", nowhere.text);
        let taken = rn.tools().call(&id, TOOL_SUBMIT, json!({"output": {"n": 1}})).await;
        assert!(!taken.is_error && taken.text.starts_with("Accepted. Station `a` is now passed"), "{}", taken.text);
        let rec = rn.engine().record(&b.run).await.unwrap();
        assert_eq!(rec.stations["a"].status, StationStatus::Passed);
        // The station moved on without it: not taken.
        assert_eq!(rec.proposals[0].status, crate::kernel::run::ProposalStatus::Dismissed);
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
