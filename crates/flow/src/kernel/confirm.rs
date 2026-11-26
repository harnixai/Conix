//! Confirmations that count votes, keep authors out when asked to, and bind what was approved
//! (docs/design/flows.md §4, in Conix).
//!
//! The votes are the station's `ConfirmRec`s of its current round, so a rework to the station starts
//! counting afresh while the old votes stay on record. Recording what was approved, and checking it
//! later, read files, which the kernel does not: both go out as gate work items under reserved names
//! (`confirm-binds@<exit>`, `confirm-stale@`) that the runner runs beside the `check` gates, and their
//! results come back through `Engine::gate_result` like any gate's — so they show in `CheckRec`,
//! survive a restart (`dispatched_work`) and run again with `rerun_gates`.

use std::collections::BTreeMap;

use super::def::{Binds, BindsWord, ExitCond, FlowDef, RejectRule, BINDS_DELIVERABLES};
use super::host::{GateResult, WorkItem};
use super::refs::refs_in;
use super::run::{Actor, CheckRec, CheckStatus, ConfirmRec, RunRecord, StationRec, StationStatus, Votes, FLOW_KIND};

/// The work item that records what a `confirm` exit approved: this, then the exit's index.
pub(crate) const BINDS_GATE: &str = "confirm-binds@";
/// The work item that checks files approved at earlier stations are as they were.
pub(crate) const STALE_GATE: &str = "confirm-stale@";
/// The kind of the check it leaves, and the rule of its findings.
pub(crate) const STALE_KIND: &str = "confirm-stale";

/// What a `confirm` check says while its approval is being recorded. Compared, not only shown: it
/// tells a waiting check that already asked for the recording from one still waiting for votes.
const BINDING: &str = "recording what was approved";
/// What a `confirm` check says when the recording failed: an `error`, kept until run again.
const BIND_FAILED: &str = "what was approved could not be recorded: ";

/// A `confirm` exit's counting rules, defaults applied.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Rule {
    pub min: u32,
    pub distinct: bool,
    /// Rejections that make the exit not met.
    pub reject_at: u32,
    pub not_author: bool,
    pub binds: Binds,
}

impl Rule {
    pub(crate) fn of(e: &ExitCond) -> Option<Rule> {
        let ExitCond::Confirm { min, distinct, reject, not, binds, .. } = e else { return None };
        let min = min.unwrap_or(1).max(1);
        Some(Rule {
            min,
            distinct: distinct.unwrap_or(true),
            reject_at: match reject.unwrap_or_default() {
                RejectRule::Any => 1,
                RejectRule::Majority => min,
            },
            not_author: not.iter().any(|n| n == "author"),
            binds: binds.clone().unwrap_or(Binds::Word(BindsWord::Deliverables)),
        })
    }
}

/// Where a `confirm` exit's votes stand.
pub(crate) enum Decision<'a> {
    /// Enough approvals and not enough rejections: the vote that made it (its index in `confirms`).
    Approved(usize, &'a ConfirmRec),
    /// The latest rejection counted.
    Rejected(&'a ConfirmRec),
    Open,
}

pub(crate) struct Tally<'a> {
    pub approvals: u32,
    pub rejections: u32,
    pub min: u32,
    pub reject_at: u32,
    pub decision: Decision<'a>,
}

/// Count exit `cond`'s votes in `round`. Rejections are looked at first: under `any`, one stands
/// against any number of approvals, which is what "one rejection is enough" means.
pub(crate) fn tally<'a>(rule: &Rule, confirms: &'a [ConfirmRec], round: u32, cond: usize) -> Tally<'a> {
    let mut counted: Vec<usize> = vec![];
    for (i, c) in confirms.iter().enumerate() {
        if c.round != round || c.cond.is_some_and(|k| k != cond) {
            continue;
        }
        if rule.distinct {
            // The same person changing their mind, not a second voice.
            counted.retain(|&j| confirms[j].by.id != c.by.id);
        }
        counted.push(i);
    }
    let approvals = counted.iter().filter(|&&i| confirms[i].pass).count() as u32;
    let rejections = counted.len() as u32 - approvals;
    let decision = if rejections >= rule.reject_at {
        let last = counted.iter().rev().find(|&&i| !confirms[i].pass).copied().expect("counted");
        Decision::Rejected(&confirms[last])
    } else if approvals >= rule.min {
        let last = counted.iter().rev().find(|&&i| confirms[i].pass).copied().expect("counted");
        Decision::Approved(last, &confirms[last])
    } else {
        Decision::Open
    };
    Tally { approvals, rejections, min: rule.min, reject_at: rule.reject_at, decision }
}

impl Tally<'_> {
    /// Why the exit is not met, for a rejection.
    pub(crate) fn rejected(&self, c: &ConfirmRec) -> String {
        let why = format!("rejected by {}: {}", c.by, c.reason.clone().unwrap_or_default());
        if self.reject_at > 1 {
            format!("{} of {} rejections; {why}", self.rejections, self.reject_at)
        } else {
            why
        }
    }

    /// What a waiting exit adds to "awaiting confirmation": the votes so far, once there are any
    /// or once more than one is needed.
    pub(crate) fn so_far(&self) -> String {
        if self.min <= 1 && self.approvals + self.rejections == 0 {
            return String::new();
        }
        let mut s = format!(" ({} of {} approvals", self.approvals, self.min);
        if self.rejections > 0 {
            s.push_str(&format!(", {} rejected", self.rejections));
        }
        s.push(')');
        s
    }
}

/// Whether an outside approval of exit `cond` has voted this round: it is asked once a round.
pub(crate) fn mcp_voted(st: &StationRec, round: u32, cond: usize) -> bool {
    st.confirms.iter().any(|c| {
        c.round == round
            && c.via == Some(super::def::ConfirmVia::Mcp)
            && c.cond.is_none_or(|k| k == cond)
    })
}

/// Who authored this run: the actors that handed in its tasks and the sessions that worked them,
/// and whoever started it (a run's sessions work on their behalf). The kernel's own actors author
/// nothing. Each with what made them one.
pub(crate) fn authors(rec: &RunRecord) -> Vec<(Actor, String)> {
    let mut out: Vec<(Actor, String)> = vec![];
    let mut add = |a: Actor, why: String| {
        if a.kind != FLOW_KIND && !out.iter().any(|(b, _)| b.id == a.id) {
            out.push((a, why));
        }
    };
    add(rec.created_by.clone(), "started the run".into());
    for (sid, st) in &rec.stations {
        for t in &st.tasks {
            let Some(d) = &t.delivery else { continue };
            add(d.by.clone(), format!("handed in {sid}/{}", t.id));
            if let Some(s) = &t.session {
                add(Actor::agent(s), format!("worked on {sid}/{}", t.id));
            }
            if let Some(s) = d.agent.as_ref().and_then(|a| a.session.as_ref()) {
                add(Actor::agent(s), format!("worked on {sid}/{} as a sub-agent", t.id));
            }
        }
    }
    out
}

/// Why `by` may not vote on `station`: a `confirm` exit of it says `not: [author]` and `by` is one.
pub(crate) fn refused(rec: &RunRecord, station: &str, by: &Actor) -> Option<String> {
    let sdef = rec.def.station(station)?;
    if !sdef.exit.iter().filter_map(Rule::of).any(|r| r.not_author) {
        return None;
    }
    let (_, why) = authors(rec).into_iter().find(|(a, _)| a.id == by.id)?;
    Some(format!(
        "{by} {why} and is an author of run {}: the confirmation of {station} says not: [author], so someone else decides it",
        rec.id
    ))
}

/// Every station `id` needs, directly or not.
fn ancestors(def: &FlowDef, id: &str) -> Vec<String> {
    let mut found: Vec<String> = vec![];
    let mut todo = def.needs_of(id);
    while let Some(n) = todo.pop() {
        if !found.contains(&n) {
            todo.extend(def.needs_of(&n));
            found.push(n);
        }
    }
    found
}

/// The paths and globs exit `e` of `station` binds, references unresolved: `deliverables` stands
/// for those the station and the stations it needs declare, in the run's directory (as written, for
/// a run that has none: the protocol tells sessions the same).
pub(crate) fn bind_patterns(rec: &RunRecord, station: &str, e: &ExitCond) -> Vec<String> {
    let Some(rule) = Rule::of(e) else { return vec![] };
    let def = &rec.def;
    let deliverables = || {
        let before = ancestors(def, station);
        def.stations
            .iter()
            .filter(|s| s.id == station || before.contains(&s.id))
            .flat_map(|s| {
                s.deliverables.iter().map(|d| match rec.output_dir {
                    Some(_) => format!("${{output_dir}}/{}", d.path),
                    None => d.path.clone(),
                })
            })
            .collect::<Vec<_>>()
    };
    let mut out: Vec<String> = vec![];
    match rule.binds {
        Binds::Word(BindsWord::Deliverables) => out.extend(deliverables()),
        Binds::List(list) => {
            for p in list {
                if p == BINDS_DELIVERABLES {
                    out.extend(deliverables());
                } else {
                    out.push(p);
                }
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

/// The work item that records what exit `cond` of `station` approved.
fn binds_item(rec: &RunRecord, station: &str, round: u32, cond: usize) -> WorkItem {
    WorkItem::Gate { run: rec.id.clone(), station: station.to_string(), round, gate: format!("{BINDS_GATE}{cond}") }
}

/// A `confirm` exit that has its approvals, judged: passed once what it approved is recorded.
/// `ask` says the recording is to be asked for now.
pub(crate) struct Approved {
    pub status: CheckStatus,
    pub detail: String,
    /// What it rests on, kept with the check (the recorded files, as evidence).
    pub carried: Option<CheckRec>,
    pub ask: Option<WorkItem>,
}

pub(crate) fn approved(
    rec: &RunRecord,
    station: &str,
    round: u32,
    cond: usize,
    vote: &ConfirmRec,
    before: Option<&CheckRec>,
) -> Approved {
    let pass = |carried| Approved { status: CheckStatus::Pass, detail: String::new(), carried, ask: None };
    if vote.binds.is_some() {
        return pass(before.filter(|k| !k.evidence.is_empty()).cloned());
    }
    let e = rec.def.station(station).and_then(|s| s.exit.get(cond));
    if e.is_none_or(|e| bind_patterns(rec, station, e).is_empty()) {
        return pass(None);
    }
    match before {
        Some(k) if k.status == CheckStatus::Error && k.detail.starts_with(BIND_FAILED) => Approved {
            status: CheckStatus::Error,
            detail: k.detail.clone(),
            carried: Some(k.clone()),
            ask: None,
        },
        Some(k) if k.status == CheckStatus::Wait && k.detail == BINDING => {
            Approved { status: CheckStatus::Wait, detail: BINDING.into(), carried: None, ask: None }
        }
        _ => Approved {
            status: CheckStatus::Wait,
            detail: BINDING.into(),
            carried: None,
            ask: Some(binds_item(rec, station, round, cond)),
        },
    }
}

/// A file approved at an earlier station, as it was then.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Approval {
    pub station: String,
    pub path: String,
    pub sha256: String,
}

/// What the passed stations other than `except` approved, through the vote that passed each of
/// their `confirm` exits this round. A station reworked since (stale, or in a new round) approved
/// nothing yet.
pub(crate) fn approvals(rec: &RunRecord, except: &str) -> Vec<Approval> {
    let mut out = vec![];
    for (sid, st) in &rec.stations {
        if sid == except || st.status != StationStatus::Passed {
            continue;
        }
        let Some(sdef) = rec.def.station(sid) else { continue };
        for (i, e) in sdef.exit.iter().enumerate() {
            let Some(rule) = Rule::of(e) else { continue };
            let Decision::Approved(_, vote) = tally(&rule, &st.confirms, st.round, i).decision else { continue };
            for (path, sha256) in vote.binds.iter().flatten() {
                out.push(Approval { station: sid.clone(), path: path.clone(), sha256: sha256.clone() });
            }
        }
    }
    out
}

/// The implicit check that what earlier stations approved is unchanged, for station `sid` being
/// judged with `checks` (its exits). Asked for last, once everything else passed: the files are
/// read right before the station would pass, not when its tasks settled and a person then took a
/// day to confirm. A result is used in the round it was asked for; `bool` says to ask now.
pub(crate) fn stale_check(
    rec: &RunRecord,
    sid: &str,
    round: u32,
    r: &StationRec,
    checks: &[CheckRec],
    now: u64,
) -> Option<(CheckRec, Option<WorkItem>)> {
    if checks.iter().any(|c| c.status != CheckStatus::Pass) || approvals(rec, sid).is_empty() {
        return None;
    }
    if let Some(k) = r.checks.iter().find(|k| is_stale_check(k) && k.round == round) {
        return Some((k.clone(), None));
    }
    let check = CheckRec {
        cond: None,
        kind: STALE_KIND.into(),
        label: "approved files unchanged".into(),
        round,
        status: CheckStatus::Wait,
        detail: "checking the files approved earlier".into(),
        at: now,
        ..Default::default()
    };
    let item = WorkItem::Gate { run: rec.id.clone(), station: sid.to_string(), round, gate: STALE_GATE.into() };
    Some((check, Some(item)))
}

pub(crate) fn is_stale_check(c: &CheckRec) -> bool {
    c.cond.is_none() && c.kind == STALE_KIND
}

/// Which reserved work item a gate name is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Implicit {
    /// Record what exit `.0` approved.
    Binds(usize),
    Stale,
}

pub(crate) fn implicit(gate: &str) -> Option<Implicit> {
    if gate == STALE_GATE {
        return Some(Implicit::Stale);
    }
    gate.strip_prefix(BINDS_GATE).and_then(|i| i.parse().ok()).map(Implicit::Binds)
}

/// A reserved work item's result taken into `rec`: `None` when `gate` is not one, else whether it
/// was still waited for.
pub(crate) fn take_result(
    rec: &mut RunRecord,
    station: &str,
    round: u32,
    gate: &str,
    result: &GateResult,
    now: u64,
) -> Option<bool> {
    let which = implicit(gate)?;
    let Some(sdef) = rec.def.station(station).cloned() else { return Some(false) };
    let Some(st) = rec.stations.get_mut(station) else { return Some(false) };
    Some(match which {
        Implicit::Stale => match st.checks.iter_mut().find(|k| is_stale_check(k) && k.round == round && k.status == CheckStatus::Wait) {
            Some(k) => {
                k.status = result.verdict.status();
                k.detail = result.summary.clone();
                k.findings = result.findings.clone();
                k.evidence = result.evidence.clone();
                k.at = now;
                true
            }
            None => false,
        },
        Implicit::Binds(cond) => {
            let Some(rule) = sdef.exit.get(cond).and_then(Rule::of) else { return Some(false) };
            let waiting = st.checks.iter().any(|k| k.cond == Some(cond) && k.round == round && k.status == CheckStatus::Wait && k.detail == BINDING);
            let vote = match tally(&rule, &st.confirms, round, cond).decision {
                Decision::Approved(i, v) if v.binds.is_none() => Some(i),
                _ => None,
            };
            let (true, Some(vote)) = (waiting, vote) else { return Some(false) };
            let k = st.checks.iter_mut().find(|k| k.cond == Some(cond) && k.round == round).expect("waiting");
            if result.is_pass() {
                st.confirms[vote].binds = Some(result.evidence.iter().map(|e| (e.path.clone(), e.sha256.clone())).collect::<BTreeMap<_, _>>());
                k.evidence = result.evidence.clone();
            } else {
                k.status = CheckStatus::Error;
                k.detail = format!("{BIND_FAILED}{}", result.summary);
                k.findings = result.findings.clone();
            }
            k.at = now;
            true
        }
    })
}

/// The reserved work item a check still waits for, to hand out again after a restart.
pub(crate) fn pending(rec: &RunRecord, sid: &str, st: &StationRec, c: &CheckRec) -> Option<WorkItem> {
    if c.round != st.round || c.status != CheckStatus::Wait {
        return None;
    }
    if is_stale_check(c) {
        return Some(WorkItem::Gate { run: rec.id.clone(), station: sid.to_string(), round: c.round, gate: STALE_GATE.into() });
    }
    let i = c.cond?;
    let confirm = rec.def.station(sid).and_then(|s| s.exit.get(i)).is_some_and(|e| matches!(e, ExitCond::Confirm { .. }));
    (confirm && c.detail == BINDING).then(|| binds_item(rec, sid, c.round, i))
}

/// Every station's `confirm` exits, counted for the run view.
pub(crate) fn votes(rec: &RunRecord) -> BTreeMap<String, Vec<Votes>> {
    let mut out = BTreeMap::new();
    for (sid, st) in &rec.stations {
        let Some(sdef) = rec.def.station(sid) else { continue };
        let list: Vec<Votes> = sdef
            .exit
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let t = tally(&Rule::of(e)?, &st.confirms, st.round, i);
                Some(Votes { cond: i, approvals: t.approvals, rejections: t.rejections, min: t.min })
            })
            .collect();
        if !list.is_empty() {
            out.insert(sid.clone(), list);
        }
    }
    out
}

/// What is wrong with a `confirm` exit's counting and binding fields.
pub(crate) fn problems(e: &ExitCond) -> Vec<String> {
    let ExitCond::Confirm { min, not, binds, .. } = e else { return vec![] };
    let mut out = vec![];
    if *min == Some(0) {
        out.push("confirm min: at least 1 approval".to_string());
    }
    for n in not {
        if n != "author" {
            out.push(format!("confirm not: {n:?} — only author (whoever authored the run may not confirm it)"));
        }
    }
    if let Some(Binds::List(list)) = binds {
        for p in list.iter().filter(|p| *p != BINDS_DELIVERABLES) {
            if let Some(m) = bind_problem(p) {
                out.push(format!("confirm binds {p:?}: {m}"));
            }
        }
    }
    out
}

/// The references a `binds` path may use: the places a run's files are.
const BIND_REFS: &[&str] = &["output_dir", "data_dir", "workspace"];

/// A `binds` entry is a path or glob in the run's workspace: relative and without `..`, or
/// starting at one of the places named by reference; `*`, `**` and `?` match.
fn bind_problem(p: &str) -> Option<String> {
    let t = p.trim();
    if t.is_empty() {
        return Some("an empty path".into());
    }
    if t.matches("${").count() != refs_in(t).len() {
        return Some("a reference is not closed".into());
    }
    if let Some(r) = refs_in(t).into_iter().find(|r| !BIND_REFS.contains(r)) {
        return Some(format!("${{{r}}}: a binds path may refer to output_dir, data_dir and workspace"));
    }
    if t.starts_with('/') || t.starts_with('\\') {
        return Some("an absolute path; start it at ${workspace}, ${output_dir} or ${data_dir}".into());
    }
    if t.split(['/', '\\']).any(|s| s == "..") {
        return Some("a path stays inside the run's workspace (no `..`)".into());
    }
    if t.contains(['[', ']', '{']) && !t.contains("${") {
        return Some("only *, ** and ? match".into());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vote(id: &str, pass: bool, round: u32) -> ConfirmRec {
        ConfirmRec { by: Actor::new("person", id), at: 0, round, pass, reason: None, via: None, reference: None, cond: None, binds: None }
    }

    fn rule(yaml: &str) -> Rule {
        Rule::of(&serde_yaml::from_str::<ExitCond>(yaml).unwrap()).unwrap()
    }

    #[test]
    fn votes_are_counted_per_person_and_round() {
        let r = rule("{kind: confirm, min: 2}");
        let v = [vote("a", true, 1), vote("a", true, 1)];
        let t = tally(&r, &v, 1, 0);
        assert_eq!((t.approvals, t.rejections), (1, 0));
        assert!(matches!(t.decision, Decision::Open));
        // the same person changing their mind replaces their vote
        let v = [vote("a", false, 1), vote("b", true, 1), vote("a", true, 1)];
        let t = tally(&r, &v, 1, 0);
        assert!(matches!(t.decision, Decision::Approved(2, _)));
        // an earlier round's votes do not count
        let v = [vote("a", true, 1), vote("b", true, 2)];
        assert_eq!(tally(&r, &v, 2, 0).approvals, 1);
        // not distinct: every vote counts
        let r = rule("{kind: confirm, min: 2, distinct: false}");
        assert!(matches!(tally(&r, &[vote("a", true, 1), vote("a", true, 1)], 1, 0).decision, Decision::Approved(1, _)));
    }

    #[test]
    fn one_rejection_is_enough_unless_a_majority_is_asked_for() {
        let v = [vote("a", true, 1), vote("b", true, 1), vote("c", false, 1)];
        assert!(matches!(tally(&rule("{kind: confirm, min: 2}"), &v, 1, 0).decision, Decision::Rejected(_)));
        let r = rule("{kind: confirm, min: 2, reject: majority}");
        assert!(matches!(tally(&r, &v, 1, 0).decision, Decision::Approved(1, _)));
        let v = [vote("a", false, 1), vote("b", false, 1), vote("c", true, 1)];
        let t = tally(&r, &v, 1, 0);
        assert!(matches!(t.decision, Decision::Rejected(_)));
        let Decision::Rejected(c) = t.decision else { unreachable!() };
        assert_eq!(t.rejected(c), "2 of 2 rejections; rejected by person:b: ");
    }

    #[test]
    fn an_outside_vote_counts_for_its_own_exit() {
        let r = rule("{kind: confirm}");
        let mut v = vote("feishu", true, 1);
        v.cond = Some(1);
        assert!(matches!(tally(&r, std::slice::from_ref(&v), 1, 0).decision, Decision::Open));
        assert!(matches!(tally(&r, std::slice::from_ref(&v), 1, 1).decision, Decision::Approved(0, _)));
    }

    #[test]
    fn binds_paths_are_checked() {
        let e = |y: &str| problems(&serde_yaml::from_str::<ExitCond>(y).unwrap());
        assert!(e("{kind: confirm, min: 2, not: [author], binds: [deliverables, '${output_dir}/*.md', 'specs/**']}").is_empty());
        assert_eq!(e("{kind: confirm, min: 0}"), ["confirm min: at least 1 approval"]);
        assert_eq!(e("{kind: confirm, not: [owner]}").len(), 1);
        for bad in ["''", "'/etc/passwd'", "'../x'", "'${inputs.x}/a'", "'${output_dir/a'", "'a/[bc]'"] {
            assert_eq!(e(&format!("{{kind: confirm, binds: [{bad}]}}")).len(), 1, "{bad}");
        }
    }
}
