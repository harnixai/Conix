//! Exits required only in some cases (in Conix: docs/design/flows.md §4.2): an exit's `when` reads a
//! fact a gate of the run worked out, or what a command step returned, and the exit is left out only
//! when that says no for certain.
//!
//! Where the value may come from is the point. A gate's facts come from its code over the files;
//! a command step's output from the bundle's code, run by the flow with no model. A station's
//! output a session hands in is not among them, and neither is anything else a model writes
//! straight into the record: a `when` read off it would be a person's confirmation the model can
//! switch off by saying `false`. And every way of not knowing — no result yet, a gate that errored,
//! a fact not there, a value of another kind — keeps the exit required: a person who is needed is
//! never routed around because something broke.

use serde_json::Value;

use super::def::{ExitCond, ExitWhen, FlowDef, Work, WhenAnswer};
use super::refs::select;
use super::run::{CheckRec, CheckStatus, RunRecord, StationRec, TaskStatus, WhenRec};

/// How `w`, the `when` of an exit of station `sid` in `round`, comes out: `r` is the station's
/// record as the judgement found it (its gates' results of this round in its checks).
pub(crate) fn judge(rec: &RunRecord, sid: &str, round: u32, r: &StationRec, w: &ExitWhen) -> WhenRec {
    let read = match (&w.gate, &w.fact, &w.step, &w.path) {
        (Some(g), Some(f), None, None) => from_gate(rec, sid, round, r, g, f),
        (None, None, Some(s), Some(p)) => from_step(rec, sid, r, s, p),
        // Validation refuses it; a definition that got past it is no reason to leave a person out.
        _ => Read::Unsure("its when names neither {gate, fact} nor {step, path}".into()),
    };
    match read {
        Read::Pending(why) => WhenRec { required: None, why, value: None },
        Read::Unsure(why) => WhenRec { required: Some(true), why: format!("{why} — required until it says otherwise"), value: None },
        Read::Value(v) => {
            let (required, why) = match w.answer(&v) {
                WhenAnswer::Yes => (true, format!("{} is {v}", w.source())),
                WhenAnswer::No => (false, format!("{} is {v}, not {}", w.source(), w.test())),
                WhenAnswer::Unsure(m) => (true, format!("{m} — required until it says otherwise")),
            };
            WhenRec { required: Some(required), why, value: Some(v) }
        }
    }
}

enum Read {
    Value(Value),
    /// A gate of this station still running.
    Pending(String),
    /// No definite value.
    Unsure(String),
}

/// The exit of `station` that is gate `gate` with no `when` of its own.
pub(crate) fn gate_exit(def: &FlowDef, station: &str, gate: &str) -> Option<usize> {
    def.station(station)?.exit.iter().position(|e| matches!(e, ExitCond::Gate { gate: g, when: None, .. } if g == gate))
}

/// The stations `id` needs, directly or not, latest first in definition order.
pub(crate) fn before(def: &FlowDef, id: &str) -> Vec<String> {
    def.stations.iter().rev().filter(|s| s.id != id && def.is_self_or_ancestor(&s.id, id)).map(|s| s.id.clone()).collect()
}

fn from_gate(rec: &RunRecord, sid: &str, round: u32, r: &StationRec, gate: &str, fact: &str) -> Read {
    let def = &rec.def;
    let found = match gate_exit(def, sid, gate) {
        // This station's: its result of this round, which may be still to come.
        Some(j) => match r.checks.iter().find(|k| k.cond == Some(j) && k.round == round && !k.held_by_when()) {
            None => return Read::Pending(format!("waiting for gate {gate} to say whether this is required")),
            Some(k) if k.status == CheckStatus::Wait => return Read::Pending(format!("waiting for gate {gate} to say whether this is required")),
            Some(k) => (k, sid.to_string()),
        },
        None => {
            let Some((at, j)) = before(def, sid).into_iter().find_map(|s| gate_exit(def, &s, gate).map(|j| (s, j))) else {
                return Read::Unsure(format!("gate {gate} is no exit of {sid} or of a station before it"));
            };
            let Some(st) = rec.stations.get(&at) else {
                return Read::Unsure(format!("gate {gate} of {at} has not run"));
            };
            match st.checks.iter().find(|k| k.cond == Some(j) && k.round == st.round) {
                None => return Read::Unsure(format!("gate {gate} of {at} has no result")),
                Some(k) => (k, at),
            }
        }
    };
    let (k, at): (&CheckRec, String) = found;
    match k.status {
        CheckStatus::Pass | CheckStatus::Fail => match k.facts.get(fact) {
            Some(v) => Read::Value(v.clone()),
            None => Read::Unsure(format!("gate {gate} of {at} gave no fact {fact}")),
        },
        CheckStatus::Error => Read::Unsure(format!("gate {gate} of {at} did not conclude ({})", k.detail)),
        CheckStatus::Wait => Read::Unsure(format!("gate {gate} of {at} has no result yet")),
    }
}

fn from_step(rec: &RunRecord, sid: &str, r: &StationRec, step: &str, path: &str) -> Read {
    let st = if step == sid { Some(r) } else { rec.stations.get(step) };
    let is_command = rec.def.station(step).is_some_and(|s| matches!(s.work(), Work::One(t) if t.executor.kind() == "command"));
    if !is_command {
        return Read::Unsure(format!("{step} is not a command step"));
    }
    let output = st.and_then(|st| st.tasks.first()).filter(|t| t.status == TaskStatus::Submitted).and_then(|t| t.output.as_ref());
    let Some(output) = output else {
        return Read::Unsure(format!("step {step} has no output"));
    };
    match select(output, path) {
        Some(v) if !v.is_null() => Read::Value(v.clone()),
        _ => Read::Unsure(format!("step {step} gave nothing at {path}")),
    }
}

/// What is wrong with exit `e`'s `when` on station `sid` of `def`, the gates' kinds told by `gate_kind`
/// (`None`: no such gate).
pub(crate) fn problems(def: &FlowDef, sid: &str, e: &ExitCond, gate_kind: &dyn Fn(&str) -> Option<super::parts::GateKind>) -> Vec<String> {
    use super::parts::GateKind;
    let Some(w) = e.when() else { return vec![] };
    let mut out = vec![];
    let scalar = |v: &Value| matches!(v, Value::Bool(_) | Value::Number(_) | Value::String(_));
    let blank = |s: &Option<String>| s.as_deref().is_none_or(|s| s.trim().is_empty());
    match (&w.gate, &w.step) {
        (Some(_), Some(_)) | (None, None) => out.push("when reads one thing: {gate, fact} or {step, path}".to_string()),
        (Some(g), None) => {
            if blank(&w.fact) {
                out.push(format!("when: gate {g} says which fact (fact: <key>)"));
            }
            if w.path.is_some() {
                out.push("when: path goes with step, fact with gate".to_string());
            }
            if matches!(e, ExitCond::Gate { gate, .. } if gate == g) {
                out.push(format!("when: gate {g} is this exit itself"));
            } else if gate_exit(def, sid, g).is_none() && !before(def, sid).iter().any(|s| gate_exit(def, s, g).is_some()) {
                out.push(format!(
                    "when: gate {g} is no exit of this station or of a station before it (one without a when of its own): only a gate the run judges by then has a result to read"
                ));
            }
            let plain = match gate_kind(g) {
                Some(GateKind::Check) => Some("check"),
                Some(GateKind::Tool) => Some("tool"),
                _ => None,
            };
            if let Some(k) = plain {
                out.push(format!("when: gate {g} is a {k} gate, which gives no facts (a script or command gate does)"));
            }
        }
        (None, Some(s)) => {
            if blank(&w.path) {
                out.push(format!("when: step {s} says where in its output (path: <dotted path>)"));
            }
            if w.fact.is_some() {
                out.push("when: fact goes with gate, path with step".to_string());
            }
            match def.station(s) {
                None => out.push(format!("when: step {s} is not a station")),
                Some(st) => {
                    if !def.is_self_or_ancestor(s, sid) {
                        out.push(format!("when: step {s} is neither this station nor one before it"));
                    }
                    if !matches!(st.work(), Work::One(t) if t.executor.kind() == "command") {
                        out.push(format!(
                            "when: step {s} is not a command step (executor: {{kind: command}}, one task): only a command's output is the flow's own; a session's is what a model wrote"
                        ));
                    }
                }
            }
        }
    }
    if w.equals.is_some() && w.any_of.is_some() {
        out.push("when: equals or in, not both".to_string());
    }
    if w.equals.as_ref().is_some_and(|v| !scalar(v)) {
        out.push("when: equals is a string, number or boolean".to_string());
    }
    if let Some(l) = &w.any_of {
        if l.is_empty() || !l.iter().all(scalar) {
            out.push("when: in is a list of strings, numbers or booleans, not empty".to_string());
        }
    }
    out
}
