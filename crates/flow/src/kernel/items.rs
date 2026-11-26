//! A `foreach` station's items: where they come from and the order their tasks are done in.
//!
//! The items are a reference resolved on entry, or a bundle's script (`foreach: {script}`). The
//! kernel runs no script: entering such a station hands out [`WorkItem::Items`] and waits — the
//! station working, no tasks, a `foreach` check waiting — until the runner reports the list
//! ([`Engine::foreach_items`]). A list that cannot be had (not a list, too long, a script that
//! threw, an `after` that names no item or goes round) leaves the station unmet with the reason,
//! as a reference that does not resolve always has: no task could start, so there is nothing for
//! an automatic rework to fix that a person should not look at first.
//!
//! An item that is an object may say `after: [<id>…]`: the `id`s of other items whose tasks must
//! be done (handed in and taken) before its own is dispatched or claimed. One whose predecessor
//! failed for good fails too, so the station is judged rather than left waiting.

use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::def::{Foreach, Work};
use super::engine::{transact, Engine, MAX_FOREACH_ITEMS};
use super::host::{Outcome, WorkItem};
use super::run::{Actor, CheckRec, CheckStatus, RunRecord, StationRec, StationStatus, TaskRec, TaskStatus};
use super::txn::{foreach_problem, item_ids, Txn, M};
use super::{FlowError, Result};

/// The `kind` of the check that stands for a station's items.
pub(crate) const FOREACH: &str = "foreach";

/// An item's own `id`, as `after` names it.
fn own_id(v: &Value) -> Option<String> {
    let x = v.as_object()?.get("id")?;
    x.as_str().map(String::from).or_else(|| x.as_u64().map(|n| n.to_string()))
}

/// The `id`s an item says it comes after; `Err` when `after` is not a list of them.
fn after_of(v: &Value) -> std::result::Result<Vec<String>, String> {
    match v.as_object().and_then(|o| o.get("after")) {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::Array(a)) => a
            .iter()
            .map(|x| x.as_str().map(String::from).or_else(|| x.as_u64().map(|n| n.to_string())))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| "after is a list of other items' ids".to_string()),
        Some(_) => Err("after is a list of other items' ids".to_string()),
    }
}

/// The tasks a list makes, each with its item; why there are none, said.
pub(crate) fn list_tasks(items: &Foreach, got: &Value) -> std::result::Result<Vec<TaskRec>, String> {
    let list = match got {
        Value::Array(list) if list.len() <= MAX_FOREACH_ITEMS => list,
        other => return Err(foreach_problem(&items.to_string(), other)),
    };
    let ids = item_ids(list);
    order_problem(list, &ids).map_err(|e| format!("{items}: {e}"))?;
    Ok(ids.into_iter().zip(list.iter().cloned()).map(|(id, v)| TaskRec::new(id, Some(v))).collect())
}

/// Every `after` names another item, and none goes round.
fn order_problem(list: &[Value], ids: &[String]) -> std::result::Result<(), String> {
    let by_id: BTreeMap<String, usize> = list.iter().enumerate().filter_map(|(i, v)| own_id(v).map(|k| (k, i))).collect();
    let mut edges: Vec<Vec<usize>> = vec![vec![]; list.len()];
    for (i, v) in list.iter().enumerate() {
        for a in after_of(v).map_err(|e| format!("item {}: {e}", ids[i]))? {
            match by_id.get(&a) {
                None => return Err(format!("item {}: after {a}: no item has that id", ids[i])),
                Some(&j) if j == i => return Err(format!("item {}: it comes after itself", ids[i])),
                Some(&j) => edges[i].push(j),
            }
        }
    }
    // Depth-first, three colours: a grey node met again is a cycle.
    fn visit(i: usize, edges: &[Vec<usize>], colour: &mut [u8], path: &mut Vec<usize>) -> Option<Vec<usize>> {
        colour[i] = 1;
        path.push(i);
        for &j in &edges[i] {
            if colour[j] == 1 {
                let from = path.iter().position(|&p| p == j).unwrap_or(0);
                let mut cycle = path[from..].to_vec();
                cycle.push(j);
                return Some(cycle);
            }
            if colour[j] == 0 {
                if let Some(c) = visit(j, edges, colour, path) {
                    return Some(c);
                }
            }
        }
        path.pop();
        colour[i] = 2;
        None
    }
    let mut colour = vec![0u8; list.len()];
    for i in 0..list.len() {
        if colour[i] == 0 {
            if let Some(c) = visit(i, &edges, &mut colour, &mut vec![]) {
                let names: Vec<&str> = c.iter().map(|&k| ids[k].as_str()).collect();
                return Err(format!("the items' after goes round: {}", names.join(" → ")));
            }
        }
    }
    Ok(())
}

/// The tasks a task's item comes after, by task id.
fn predecessors(r: &StationRec, t: &TaskRec) -> Vec<String> {
    let Some(item) = &t.item else { return vec![] };
    let after = after_of(item).unwrap_or_default();
    if after.is_empty() {
        return vec![];
    }
    r.tasks
        .iter()
        .filter(|o| o.item.as_ref().and_then(own_id).is_some_and(|k| after.contains(&k)))
        .map(|o| o.id.clone())
        .collect()
}

/// A task whose item comes after others not done yet: it is not dispatched or claimed.
pub(crate) fn waits(r: &StationRec, t: &TaskRec) -> bool {
    predecessors(r, t)
        .iter()
        .any(|p| r.task(p).is_none_or(|o| o.status != TaskStatus::Submitted))
}

/// The items' check of this round, if one is there: waiting for a script, or why there are none.
fn check_of(r: &StationRec, round: u32) -> Option<&CheckRec> {
    r.checks.iter().find(|c| c.cond.is_none() && c.kind == FOREACH && c.round == round)
}

/// A station whose items a script is still working out: nothing to judge yet.
pub(crate) fn pending(r: &StationRec) -> bool {
    check_of(r, r.round).is_some_and(|c| c.status == CheckStatus::Wait)
}

fn check(round: u32, status: CheckStatus, detail: String, at: u64) -> CheckRec {
    CheckRec { cond: None, kind: FOREACH.into(), label: FOREACH.into(), round, status, detail, at, ..Default::default() }
}

/// What a run handed out for items and has not had back: after a restart, asked for again.
pub(crate) fn pending_work(rec: &RunRecord, sid: &str, r: &StationRec) -> Option<WorkItem> {
    let Some(Work::Foreach { items, .. }) = rec.def.station(sid).map(|s| s.work()) else { return None };
    let script = items.script()?;
    (matches!(r.status, StationStatus::Working | StationStatus::Held) && pending(r)).then(|| WorkItem::Items {
        run: rec.id.clone(),
        station: sid.to_string(),
        round: r.round,
        script: script.to_string(),
    })
}

impl M<'_> {
    /// The tasks of a `foreach` station entered for a new round: the list resolved now, or none
    /// yet while a script works it out (its work handed out). `None`: the station is left unmet,
    /// said why, and its entry is over.
    pub(crate) fn foreach_entered(&mut self, sid: &str, items: &Foreach, ctx: &Value, round: u32) -> Option<Vec<TaskRec>> {
        let now = self.fx.now;
        if let Some(script) = items.script() {
            self.rec.stations.entry(sid.to_string()).or_default().checks.push(check(
                round,
                CheckStatus::Wait,
                format!("script {script} is working out the items"),
                now,
            ));
            self.fx.work.push(WorkItem::Items {
                run: self.rec.id.clone(),
                station: sid.to_string(),
                round,
                script: script.to_string(),
            });
            return Some(vec![]);
        }
        let Foreach::Ref(r) = items else { return Some(vec![]) };
        match list_tasks(items, &super::refs::resolve_str(r, ctx)) {
            Ok(tasks) => Some(tasks),
            Err(detail) => {
                self.foreach_unmet(sid, round, detail, true);
                None
            }
        }
    }

    /// No items to be had: the station unmet, why said.
    fn foreach_unmet(&mut self, sid: &str, round: u32, detail: String, entering: bool) {
        let now = self.fx.now;
        let r = self.rec.stations.entry(sid.to_string()).or_default();
        r.clear_tasks();
        r.status = StationStatus::Unmet;
        r.checks = vec![check(round, CheckStatus::Fail, detail.clone(), now)];
        if entering {
            self.emit("station.entered", Some(sid), None, json!({"round": round}));
        }
        let unmet = self.rec.stations[sid].checks.clone();
        self.emit("station.unmet", Some(sid), None, json!({"round": round, "unmet": unmet}));
        self.log("enter", Some(sid), None, json!({"round": round, "error": FOREACH, "detail": detail}));
    }

    /// What `judge` counts for the items of a station that has no tasks: a script still working
    /// (`Wait`: nothing is judged), the reason there are none, or nothing when the list is empty
    /// (or the station has tasks, or is no `foreach`).
    pub(crate) fn foreach_judged(&self, sid: &str) -> Option<CheckRec> {
        let st = self.rec.def.station(sid)?;
        let Work::Foreach { items, .. } = st.work() else { return None };
        let r = self.rec.stations.get(sid)?;
        if !r.tasks.is_empty() {
            return None;
        }
        if let Some(c) = check_of(r, r.round) {
            return Some(c.clone());
        }
        // A list that never resolved: worked out again, as it always was.
        let Foreach::Ref(reference) = &items else { return None };
        let ws = self.eng.workspace_for(self.rec);
        let ctx = super::refs::context_in(self.rec, None, None, ws.as_ref().and_then(|p| p.to_str()));
        list_tasks(&items, &super::refs::resolve_str(reference, &ctx))
            .err()
            .map(|detail| check(r.round, CheckStatus::Fail, detail, self.fx.now))
    }

    /// Tasks still ready whose item comes after one that failed for good, or was cancelled: they
    /// cannot start, so they fail too and the station is judged.
    pub(crate) fn fail_unreachable(&mut self, sid: &str) {
        loop {
            let Some(r) = self.rec.stations.get(sid).filter(|r| r.status == StationStatus::Working) else { return };
            let stuck: Vec<(String, String)> = r
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Ready)
                .filter_map(|t| {
                    predecessors(r, t).into_iter().find_map(|p| match r.task(&p).map(|o| o.status) {
                        Some(TaskStatus::Failed) => Some((t.id.clone(), format!("it comes after {p}, which failed"))),
                        Some(TaskStatus::Cancelled) => Some((t.id.clone(), format!("it comes after {p}, which was cancelled"))),
                        _ => None,
                    })
                })
                .collect();
            if stuck.is_empty() {
                return;
            }
            let now = self.fx.now;
            for (tid, why) in stuck {
                if let Some(t) = self.task_mut(sid, &tid) {
                    t.status = TaskStatus::Failed;
                    t.error = Some(why.clone());
                    t.ended_at = Some(now);
                    t.lease = None;
                }
                let data = json!({"error": why, "retrying": false});
                self.emit("task.failed", Some(sid), Some(&tid), data.clone());
                self.log("fail", Some(sid), Some(&tid), data);
            }
            self.judge_later(sid);
        }
    }

    /// The items a script worked out, for the round that asked: the station's tasks made from
    /// them and dispatched, or the station unmet.
    fn items_came(&mut self, sid: &str, round: u32, got: std::result::Result<Value, String>) {
        let Some(Work::Foreach { items, .. }) = self.rec.def.station(sid).map(|s| s.work()) else { return };
        let made = got.map_err(|e| format!("{items}: {e}")).and_then(|v| list_tasks(&items, &v));
        let tasks = match made {
            Ok(t) => t,
            Err(detail) => return self.foreach_unmet(sid, round, detail, false),
        };
        let ids: Vec<String> = tasks.iter().map(|t| t.id.clone()).collect();
        let r = self.rec.stations.entry(sid.to_string()).or_default();
        r.checks.retain(|c| !(c.cond.is_none() && c.kind == FOREACH));
        r.tasks = tasks;
        let held = r.status == StationStatus::Held;
        self.log("items", Some(sid), None, json!({"round": round, "tasks": ids}));
        for t in &ids {
            self.emit("task.ready", Some(sid), Some(t), Value::Null);
        }
        if ids.is_empty() && !held {
            self.judge_later(sid);
        }
        self.dispatch(sid);
        self.advance_later();
    }
}

impl Engine {
    /// A `foreach: {script}` station's items, as its script worked them out for `round` (`Err`:
    /// why it gave none). A result for a round that has moved on, or a station not waiting for
    /// one, is ignored (`value.applied` = false).
    pub async fn foreach_items(
        &self,
        run: &str,
        station: &str,
        round: u32,
        items: std::result::Result<Value, String>,
    ) -> Result<Outcome> {
        transact!(self, Actor::flow("items"), |t| {
            t.load(run).await?;
            let rec = t.get(run);
            let waiting = rec.is_active()
                && rec.stations.get(station).is_some_and(|s| {
                    s.round == round && matches!(s.status, StationStatus::Working | StationStatus::Held) && pending(s)
                });
            if !waiting {
                return Ok(json!({"applied": false}));
            }
            t.m(run).items_came(station, round, items.clone());
            t.settle().await?;
            Ok(json!({"applied": true}))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn after_must_name_items_and_not_go_round() {
        let ok = json!([{"id": "a"}, {"id": "b", "after": ["a"]}, {"id": "c", "after": ["a", "b"]}]);
        assert!(list_tasks(&Foreach::Ref("x".into()), &ok).is_ok());
        let unknown = json!([{"id": "a", "after": ["z"]}]);
        assert!(list_tasks(&Foreach::Ref("x".into()), &unknown).unwrap_err().contains("after z: no item"));
        let round = json!([{"id": "a", "after": ["c"]}, {"id": "b", "after": ["a"]}, {"id": "c", "after": ["b"]}]);
        let e = list_tasks(&Foreach::Ref("x".into()), &round).unwrap_err();
        assert!(e.contains("goes round") && e.contains("a → c → b → a"), "{e}");
        let bad = json!([{"id": "a", "after": "b"}, {"id": "b"}]);
        assert!(list_tasks(&Foreach::Ref("x".into()), &bad).unwrap_err().contains("list of other items"));
        let own = json!([{"id": "a", "after": ["a"]}]);
        assert!(list_tasks(&Foreach::Ref("x".into()), &own).unwrap_err().contains("after itself"));
    }
}
