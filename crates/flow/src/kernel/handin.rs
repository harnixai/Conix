//! A task's hand-in, and the station's `task_exit` gates it must pass before it is taken.
//!
//! Without `task_exit` a hand-in that satisfies the contract is taken at once, as it always was.
//! With it, the hand-in waits in the task ([`TaskExitRec`]): a `tool` gate is judged on the spot,
//! every other gate is handed out as [`WorkItem::TaskGate`] and its result comes back through
//! [`Engine::task_gate_result`] — the same split as a station's own gates. The task stays claimed
//! meanwhile and its lease does not run out (the wait is the flow's, not the worker's); after a
//! restart the gates still waiting are handed out again (`pending_work`), so the hand-in is judged
//! whatever happened in between.
//!
//! When every gate has concluded: all passed, the output is taken as if handed in now, by whoever
//! handed it in. Otherwise it is sent back. A session, a sub-agent or someone outside is told what
//! was found (the tool's reply, the next message, the record) and keeps its claim, with a fresh
//! lease, to fix it and hand in again; a program, a command or a child run cannot fix anything by
//! being told, so its attempt fails and `retry` decides.

use serde_json::{json, Value};

use super::def::{Executor, ExitCond};
use super::engine::{transact, Engine, MAX_LEASE_MS};
use super::host::{GateResult, Outcome, WorkItem};
use super::parts::GateKind;
use super::refs::{asserts_fail, context_in, resolve};
use super::run::{Actor, AgentDelivery, CheckRec, CheckStatus, RunRecord, StationRec, StationStatus, TaskExitRec, TaskRec, TaskStatus};
use super::txn::{Txn, M};
use super::{FlowError, Result};

/// A station's `task_exit` gates: each condition's index and gate.
fn gates_of(rec: &RunRecord, sid: &str) -> Vec<(usize, String, String)> {
    rec.def
        .station(sid)
        .map(|s| {
            s.task_exit
                .iter()
                .enumerate()
                .filter_map(|(i, c)| match c {
                    ExitCond::Gate { gate, .. } => Some((i, gate.clone(), c.label())),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `task_exit` gates still to be run for the hand-ins a run is waiting on: after a restart,
/// handed out again.
pub(crate) fn pending_work(rec: &RunRecord, sid: &str, r: &StationRec) -> Vec<WorkItem> {
    let mut out = vec![];
    if !matches!(r.status, StationStatus::Working | StationStatus::Unmet) {
        return out;
    }
    let gates = gates_of(rec, sid);
    for t in r.tasks.iter().filter(|t| t.checking()) {
        let Some(x) = &t.task_exit else { continue };
        for c in x.checks.iter().filter(|c| c.status == CheckStatus::Wait) {
            let Some((cond, gate, _)) = c.cond.and_then(|i| gates.iter().find(|g| g.0 == i)) else { continue };
            out.push(WorkItem::TaskGate {
                run: rec.id.clone(),
                station: sid.to_string(),
                round: r.round,
                task: t.id.clone(),
                seq: x.seq,
                cond: *cond,
                gate: gate.clone(),
            });
        }
    }
    out
}

/// What the `task_exit` gates found in a task's last hand-in, when they sent it back and nothing
/// was taken since.
pub fn sent_back(t: &TaskRec) -> Option<String> {
    let x = t.task_exit.as_ref()?;
    (t.status != TaskStatus::Submitted && matches!(x.status, CheckStatus::Fail | CheckStatus::Error)).then(|| x.explain())
}

impl M<'_> {
    /// A task handed in `output`, already checked against the contract: taken now when its station
    /// has no `task_exit`, else held for those gates.
    pub(crate) async fn hand_in(&mut self, sid: &str, tid: &str, output: Value, agent: Option<AgentDelivery>) {
        let gates = gates_of(self.rec, sid);
        if gates.is_empty() {
            self.task_submitted(sid, tid, output);
            if let Some(a) = agent {
                if let Some(d) = self.task_mut(sid, tid).and_then(|x| x.delivery.as_mut()) {
                    d.agent = Some(a);
                }
            }
            return;
        }
        let now = self.fx.now;
        let Some(t) = self.rec.stations.get(sid).and_then(|s| s.task(tid)).cloned() else { return };
        let round = self.rec.stations[sid].round;
        let seq = t.task_exit.as_ref().map_or(0, |x| x.seq) + 1;
        let ws = self.eng.workspace_for(self.rec);
        let ctx = context_in(self.rec, t.item.as_ref(), Some(tid), ws.as_ref().and_then(|p| p.to_str()));
        let mut checks = vec![];
        for (cond, gate, label) in &gates {
            let (status, detail) = match self.rec.parts.gates.get(gate).cloned() {
                None => (CheckStatus::Error, format!("gate {gate} was not in the packs when the run started")),
                // A tool answers at once, as for a station's own gates.
                Some(g) if g.kind == GateKind::Tool => {
                    let tool = g.tool.clone().unwrap_or_default();
                    let args = if g.args.is_null() { json!({}) } else { g.args.clone() };
                    let args = resolve(&args, &ctx);
                    match self.eng.tools.call(&tool, &args).await {
                        Ok(v) => match asserts_fail(&v, &g.assert) {
                            None => (CheckStatus::Pass, String::new()),
                            Some(d) => (CheckStatus::Fail, d),
                        },
                        Err(e) => (CheckStatus::Error, format!("{tool} failed: {e}")),
                    }
                }
                Some(_) => {
                    self.fx.work.push(WorkItem::TaskGate {
                        run: self.rec.id.clone(),
                        station: sid.to_string(),
                        round,
                        task: tid.to_string(),
                        seq,
                        cond: *cond,
                        gate: gate.clone(),
                    });
                    (CheckStatus::Wait, "requested".to_string())
                }
            };
            checks.push(CheckRec {
                cond: Some(*cond),
                kind: "gate".into(),
                label: label.clone(),
                round,
                status,
                detail,
                at: now,
                ..Default::default()
            });
        }
        let by = self.by.clone();
        if let Some(t) = self.task_mut(sid, tid) {
            // Handed in again after it was taken: the last hand-in counts, so the task is not done
            // until this one is judged.
            t.status = TaskStatus::Claimed;
            t.task_exit = Some(TaskExitRec { seq, status: CheckStatus::Wait, checks, output, by, at: now, agent });
        }
        let gates: Vec<&String> = gates.iter().map(|g| &g.1).collect();
        self.log("hand_in", Some(sid), Some(tid), json!({"seq": seq, "task_exit": gates}));
        self.conclude(sid, tid);
    }

    /// Once every `task_exit` gate on the waiting hand-in has concluded: taken, or sent back.
    fn conclude(&mut self, sid: &str, tid: &str) {
        let Some(x) = self.rec.stations.get(sid).and_then(|s| s.task(tid)).and_then(|t| t.task_exit.clone()) else { return };
        if x.status != CheckStatus::Wait || x.checks.iter().any(|c| c.status == CheckStatus::Wait) {
            return;
        }
        let pass = x.checks.iter().all(|c| c.status == CheckStatus::Pass);
        let now = self.fx.now;
        if pass {
            if let Some(t) = self.task_mut(sid, tid).and_then(|t| t.task_exit.as_mut()) {
                t.status = CheckStatus::Pass;
                t.output = Value::Null;
            }
            self.emit("task.checked", Some(sid), Some(tid), json!({"seq": x.seq, "pass": true}));
            // Taken as handed in: by whoever handed it in, not by the gate that let it through.
            let gate = std::mem::replace(&mut self.by, x.by.clone());
            self.task_submitted(sid, tid, x.output.clone());
            self.by = gate;
            if let Some(a) = x.agent {
                if let Some(d) = self.task_mut(sid, tid).and_then(|t| t.delivery.as_mut()) {
                    d.agent = Some(a);
                }
            }
            return;
        }
        let status = if x.checks.iter().any(|c| c.status == CheckStatus::Fail) { CheckStatus::Fail } else { CheckStatus::Error };
        let explain = TaskExitRec { status, ..x.clone() }.explain();
        let fixes = self
            .rec
            .def
            .station(sid)
            .and_then(|s| s.task_spec(tid))
            .is_some_and(|s| matches!(s.executor, Executor::Session { .. } | Executor::Agent { .. } | Executor::External { .. }));
        let lease_ms = self.eng.lease_ms.min(MAX_LEASE_MS);
        if let Some(t) = self.task_mut(sid, tid) {
            if let Some(e) = t.task_exit.as_mut() {
                e.status = status;
            }
            // Whoever fixes it gets the whole of a lease to: the checks took some of it.
            if let Some(l) = t.lease.as_mut().filter(|_| fixes) {
                l.until = l.until.max(now.saturating_add(lease_ms));
            }
        }
        let data = json!({"seq": x.seq, "pass": false, "detail": explain});
        self.emit("task.checked", Some(sid), Some(tid), data.clone());
        self.log("task_checked", Some(sid), Some(tid), data);
        if !fixes {
            self.task_failed(sid, tid, format!("task_exit: {explain}"));
        }
    }

    /// A `task_exit` gate's result on hand-in `seq`; whether it was waited for.
    fn task_gate_came(&mut self, sid: &str, tid: &str, seq: u32, cond: usize, result: &GateResult) -> bool {
        let now = self.fx.now;
        let Some(c) = self
            .task_mut(sid, tid)
            .filter(|t| t.checking())
            .and_then(|t| t.task_exit.as_mut())
            .filter(|x| x.seq == seq)
            .and_then(|x| x.checks.iter_mut().find(|c| c.cond == Some(cond) && c.status == CheckStatus::Wait))
        else {
            return false;
        };
        c.status = result.verdict.status();
        c.detail = result.summary.clone();
        c.findings = result.findings.clone();
        c.evidence = result.evidence.clone();
        c.metrics = result.metrics.clone();
        c.at = now;
        true
    }
}

impl Engine {
    /// A `task_exit` gate's result for hand-in `seq` of a task, in `round`. A result for a round
    /// that has moved on, another hand-in, or a gate nobody is waiting for is ignored
    /// (`value.applied` = false).
    #[allow(clippy::too_many_arguments)]
    pub async fn task_gate_result(
        &self,
        run: &str,
        station: &str,
        round: u32,
        task: &str,
        seq: u32,
        cond: usize,
        result: GateResult,
    ) -> Result<Outcome> {
        transact!(self, Actor::flow("gate"), |t| {
            t.load(run).await?;
            let rec = t.get(run);
            let open = rec.stations.get(station).is_some_and(|s| {
                s.round == round && matches!(s.status, StationStatus::Working | StationStatus::Unmet)
            });
            if !rec.is_active() || !open {
                return Ok(json!({"applied": false}));
            }
            let gate = rec.def.station(station).and_then(|s| s.task_exit.get(cond)).and_then(|c| match c {
                ExitCond::Gate { gate, .. } => Some(gate.clone()),
                _ => None,
            });
            let fail_on = gate.as_ref().and_then(|g| rec.parts.gates.get(g)).and_then(|g| g.fail_on);
            let result = result.clone().strict(fail_on);
            let mut m = t.m(run);
            if !m.task_gate_came(station, task, seq, cond, &result) {
                return Ok(json!({"applied": false}));
            }
            m.log(
                "task_gate_result",
                Some(station),
                Some(task),
                json!({"gate": gate, "seq": seq, "round": round, "pass": result.is_pass(), "verdict": result.verdict,
                       "detail": result.summary, "findings": result.findings, "evidence": result.evidence,
                       "metrics": result.metrics}),
            );
            m.conclude(station, task);
            m.advance_later();
            t.settle().await?;
            Ok(json!({"applied": true}))
        })
    }
}
