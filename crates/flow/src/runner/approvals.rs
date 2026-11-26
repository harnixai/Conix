//! `via: mcp` confirmations: ask an outside approval system, and poll it until it decides
//! (docs/design/flows.md §4, in Conix).
//!
//! The host calls the tools (`Approvals`): the library never talks MCP itself. The request is
//! made once a round — what it gave back is in the run — so a restart polls the same request.
//! There is no callback to wait for (the host listens on no port): polling is the only way the
//! decision arrives. A timeout is the kernel's (the confirmation's `timeout`), not this loop's.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};

use super::dispatch::settle;
use super::Runner;
use crate::kernel::def::{ConfirmVia, ExitCond, McpApproval};
use crate::kernel::refs::{resolve, select};
use crate::kernel::run::{Actor, RunRecord};

/// Status calls in a row that may fail (a server restarting) before the confirmation is an error.
const POLL_FAILURES: u32 = 5;

pub(crate) async fn run_approval(rn: &Arc<Runner>, run: String, station: String, round: u32, cond: usize) {
    let engine = rn.engine();
    let Ok(rec) = engine.record(&run).await else { return };
    let Some(mcp) = approval_of(&rec, &station, cond) else { return };
    let Some(host) = rn.cfg.approvals.clone() else {
        let r = engine
            .approval_failed(&run, &station, round, cond, "the host calls no MCP tools: an outside approval cannot be asked for")
            .await;
        settle(rn, &run, "approval_failed", r).await;
        return;
    };
    let ctx = |rec: &RunRecord, request: &Value| {
        let mut c = crate::kernel::refs::context_in(rec, None, None, None);
        c["station"] = json!(station);
        c["request"] = request.clone();
        c
    };
    // The request, once a round.
    let request = match rec.stations.get(&station).and_then(|s| s.approvals.iter().find(|a| a.cond == cond && a.round == round)) {
        Some(a) => a.request.clone(),
        None => {
            let args = resolve(&mcp.request.arguments, &ctx(&rec, &Value::Null));
            match host.call(&rec, &mcp.server, &mcp.request.tool, &args).await {
                Ok(v) => {
                    let link = mcp.link.as_deref().and_then(|p| text_at(&v, p));
                    let r = engine.approval_requested(&run, &station, round, cond, v.clone(), link).await;
                    settle(rn, &run, "approval_requested", r).await;
                    v
                }
                Err(e) => {
                    let detail = format!("{}: {} could not be asked: {e}", mcp.server, mcp.request.tool);
                    let r = engine.approval_failed(&run, &station, round, cond, &detail).await;
                    settle(rn, &run, "approval_failed", r).await;
                    return;
                }
            }
        }
    };
    let every = Duration::from_millis(mcp.poll_ms());
    let mut failures = 0u32;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(every) => {}
            _ = rn.cancelled().cancelled() => return,
        }
        // Still waited for? A timeout, a rework or a cancel ends the loop.
        let Ok(rec) = engine.record(&run).await else { return };
        if !still_waiting(&rec, &station, round, cond) {
            return;
        }
        let args = resolve(&mcp.status.arguments, &ctx(&rec, &request));
        let got = match host.call(&rec, &mcp.server, &mcp.status.tool, &args).await {
            Ok(v) => {
                failures = 0;
                v
            }
            Err(e) => {
                failures += 1;
                tracing::warn!(run, station, failures, error = %e, "an outside approval could not be read");
                if failures >= POLL_FAILURES {
                    let detail = format!("{}: {} failed {failures} times in a row: {e}", mcp.server, mcp.status.tool);
                    let r = engine.approval_failed(&run, &station, round, cond, &detail).await;
                    settle(rn, &run, "approval_failed", r).await;
                    return;
                }
                continue;
            }
        };
        match decision(&mcp, &got) {
            None => {
                let _ = engine.approval_polled(&run, &station, round, cond).await;
            }
            Some(pass) => {
                let who = mcp.by.as_deref().and_then(|p| text_at(&got, p)).unwrap_or_else(|| mcp.server.clone());
                let reference = mcp
                    .link
                    .as_deref()
                    .and_then(|p| text_at(&got, p).or_else(|| text_at(&request, p)))
                    .or_else(|| text_at(&request, "id"));
                let state = select(&got, &mcp.decision.path).map(plain);
                let reason = state.map(|s| format!("{} in {}", s, mcp.server));
                let r = engine
                    .approval_decided(&run, &station, round, cond, pass, Actor::new("mcp", who), reason, reference)
                    .await;
                settle(rn, &run, "approval_decided", r).await;
                return;
            }
        }
    }
}

fn approval_of(rec: &RunRecord, station: &str, cond: usize) -> Option<McpApproval> {
    match rec.def.station(station)?.exit.get(cond)? {
        ExitCond::Confirm { via: Some(ConfirmVia::Mcp), mcp: Some(m), .. } => Some((**m).clone()),
        _ => None,
    }
}

fn still_waiting(rec: &RunRecord, station: &str, round: u32, cond: usize) -> bool {
    let Some(st) = rec.stations.get(station) else { return false };
    rec.is_active()
        && st.round == round
        && st.status == crate::kernel::run::StationStatus::Working
        && !crate::kernel::confirm::mcp_voted(st, round, cond)
        && st.checks.iter().any(|c| c.cond == Some(cond) && c.round == round && c.status == crate::kernel::run::CheckStatus::Wait)
}

/// Approved (`Some(true)`), rejected (`Some(false)`), or still open.
pub(crate) fn decision(mcp: &McpApproval, status: &Value) -> Option<bool> {
    let v = plain(select(status, &mcp.decision.path)?);
    if mcp.decision.approved.contains(&v) {
        Some(true)
    } else if mcp.decision.rejected.contains(&v) {
        Some(false)
    } else {
        None
    }
}

fn plain(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn text_at(v: &Value, path: &str) -> Option<String> {
    select(v, path).filter(|x| !x.is_null()).map(plain).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::host::{Approvals, WorkItem};
    use crate::kernel::run::{CheckStatus, StationStatus};
    use crate::runner::{RunnerConfig, ScriptedHost};
    use crate::{CreateIn, Engine, ManualClock, MemPacks, MemStore};
    use std::sync::Mutex;

    /// An approval system that answers `pending` `n` times, then the decision.
    struct Desk {
        calls: Mutex<Vec<(String, Value)>>,
        pending: Mutex<u32>,
        decide: &'static str,
        fail_request: bool,
    }

    #[async_trait::async_trait]
    impl Approvals for Desk {
        async fn call(&self, _: &RunRecord, server: &str, tool: &str, args: &Value) -> Result<Value, String> {
            assert_eq!(server, "desk");
            self.calls.lock().unwrap().push((tool.to_string(), args.clone()));
            match tool {
                "open" if self.fail_request => Err("desk is down".into()),
                "open" => Ok(json!({"id": "A-7", "url": "https://desk/A-7"})),
                _ => {
                    let mut p = self.pending.lock().unwrap();
                    if *p > 0 {
                        *p -= 1;
                        Ok(json!({"state": "PENDING"}))
                    } else {
                        Ok(json!({"state": self.decide, "approver": {"name": "alice"}}))
                    }
                }
            }
        }
    }

    const FLOW: &str = "{name: a, stations: [{id: ok, exit: [{kind: confirm, via: mcp, mcp: {server: desk, request: {tool: open, arguments: {title: '${run} ${station}'}}, status: {tool: get, arguments: {id: '${request.id}'}}, decision: {path: state, approved: [APPROVED], rejected: [REJECTED]}, by: approver.name, link: url, poll: 30s}}]}]}";

    async fn setup(desk: Desk) -> (Arc<Runner>, Arc<Engine>, String, WorkItem, Arc<Desk>) {
        setup_with(FLOW, desk, Actor::device("d")).await
    }

    async fn setup_with(flow: &str, desk: Desk, by: Actor) -> (Arc<Runner>, Arc<Engine>, String, WorkItem, Arc<Desk>) {
        let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), Arc::new(MemPacks::new().flow(flow))).with_clock(Arc::new(ManualClock::new(1_000_000))));
        let desk = Arc::new(desk);
        let cfg = RunnerConfig { approvals: Some(desk.clone()), ..Default::default() };
        let rn = Runner::for_tests_with(engine.clone(), ScriptedHost::new(), cfg);
        let o = engine.create(CreateIn { flow: "a".into(), subject: "s".into(), ..Default::default() }, by).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        let work = o.work.into_iter().find(|w| matches!(w, WorkItem::Approval { .. })).expect("an approval is asked for");
        (rn, engine, run, work, desk)
    }

    #[tokio::test(start_paused = true)]
    async fn an_outside_approval_is_asked_once_polled_and_recorded_with_who_and_where() {
        let desk = Desk { calls: Mutex::new(vec![]), pending: Mutex::new(2), decide: "APPROVED", fail_request: false };
        let (rn, engine, run, work, desk) = setup(desk).await;
        let WorkItem::Approval { station, round, cond, .. } = work else { unreachable!() };
        run_approval(&rn, run.clone(), station, round, cond).await;
        let calls = desk.calls.lock().unwrap().clone();
        assert_eq!(calls[0], ("open".to_string(), json!({"title": format!("{run} ok")})));
        assert_eq!(calls.iter().filter(|c| c.0 == "get").count(), 3);
        assert_eq!(calls[1].1, json!({"id": "A-7"}));
        let rec = engine.record(&run).await.unwrap();
        assert_eq!(rec.stations["ok"].status, StationStatus::Passed);
        let c = &rec.stations["ok"].confirms[0];
        assert_eq!((c.by.kind.as_str(), c.by.id.as_str(), c.pass), ("mcp", "alice", true));
        assert_eq!(c.reference.as_deref(), Some("https://desk/A-7"));
        assert_eq!(c.via, Some(ConfirmVia::Mcp));
        assert_eq!(rec.stations["ok"].approvals[0].request["id"], "A-7");
    }

    #[tokio::test(start_paused = true)]
    async fn a_rejection_is_a_failed_confirmation() {
        let desk = Desk { calls: Mutex::new(vec![]), pending: Mutex::new(0), decide: "REJECTED", fail_request: false };
        let (rn, engine, run, work, _) = setup(desk).await;
        let WorkItem::Approval { station, round, cond, .. } = work else { unreachable!() };
        run_approval(&rn, run.clone(), station, round, cond).await;
        let rec = engine.record(&run).await.unwrap();
        assert_eq!(rec.stations["ok"].status, StationStatus::Unmet);
        assert!(rec.stations["ok"].checks[0].detail.contains("rejected by mcp:alice"), "{:?}", rec.stations["ok"].checks);
    }

    #[tokio::test(start_paused = true)]
    async fn a_request_that_cannot_be_made_is_an_error_until_asked_again() {
        let desk = Desk { calls: Mutex::new(vec![]), pending: Mutex::new(0), decide: "APPROVED", fail_request: true };
        let (rn, engine, run, work, _) = setup(desk).await;
        let WorkItem::Approval { station, round, cond, .. } = work else { unreachable!() };
        run_approval(&rn, run.clone(), station.clone(), round, cond).await;
        let rec = engine.record(&run).await.unwrap();
        let c = &rec.stations["ok"].checks[0];
        assert_eq!((rec.stations["ok"].status, c.status), (StationStatus::Unmet, CheckStatus::Error));
        assert!(c.detail.contains("desk is down"), "{c:?}");
        // Asked again: handed out afresh.
        let o = engine.rerun_gates(&run, &station, Actor::device("d")).await.unwrap();
        assert!(o.work.iter().any(|w| matches!(w, WorkItem::Approval { .. })), "{:?}", o.work);
    }

    /// An outside decision is one vote; a person's adds to it.
    #[tokio::test(start_paused = true)]
    async fn an_outside_approval_is_one_vote_of_several() {
        let desk = Desk { calls: Mutex::new(vec![]), pending: Mutex::new(0), decide: "APPROVED", fail_request: false };
        let flow = FLOW.replace("kind: confirm, via: mcp", "kind: confirm, min: 2, via: mcp");
        let (rn, engine, run, work, _) = setup_with(&flow, desk, Actor::device("d")).await;
        let WorkItem::Approval { station, round, cond, .. } = work else { unreachable!() };
        run_approval(&rn, run.clone(), station, round, cond).await;
        let v = engine.get(&run).await.unwrap();
        assert_eq!(v.record.stations["ok"].status, StationStatus::Working);
        assert_eq!((v.votes["ok"][0].approvals, v.votes["ok"][0].min), (1, 2));
        assert_eq!(v.unmet["ok"][0].detail, "awaiting confirmation (1 of 2 approvals)");
        engine.confirm(&run, "ok", true, None, Actor::new("person", "bob")).await.unwrap();
        assert_eq!(engine.record(&run).await.unwrap().stations["ok"].status, StationStatus::Passed);
    }

    /// Decided outside by whoever started the run, where authors may not decide: an error, no vote.
    #[tokio::test(start_paused = true)]
    async fn an_outside_approval_by_an_author_is_refused() {
        let desk = Desk { calls: Mutex::new(vec![]), pending: Mutex::new(0), decide: "APPROVED", fail_request: false };
        let flow = FLOW.replace("kind: confirm, via: mcp", "kind: confirm, not: [author], via: mcp");
        let (rn, engine, run, work, _) = setup_with(&flow, desk, Actor::new("person", "alice")).await;
        let WorkItem::Approval { station, round, cond, .. } = work else { unreachable!() };
        run_approval(&rn, run.clone(), station, round, cond).await;
        let rec = engine.record(&run).await.unwrap();
        let st = &rec.stations["ok"];
        assert!(st.confirms.is_empty());
        assert_eq!((st.status, st.checks[0].status), (StationStatus::Unmet, CheckStatus::Error));
        assert!(st.checks[0].detail.contains("not: [author]"), "{:?}", st.checks);
    }

    #[tokio::test]
    async fn no_decision_within_the_timeout_fails_the_confirmation() {
        let flow = "{name: t, stations: [{id: ok, exit: [{kind: confirm, timeout: 2h}]}]}";
        let clock = Arc::new(ManualClock::new(1_000_000));
        let engine = Engine::new(Arc::new(MemStore::new()), Arc::new(MemPacks::new().flow(flow))).with_clock(clock.clone());
        let o = engine.create(CreateIn { flow: "t".into(), subject: "s".into(), ..Default::default() }, Actor::device("d")).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        let rec = engine.record(&run).await.unwrap();
        assert_eq!(rec.next_expiry(), Some(1_000_000 + 2 * 3_600_000));
        clock.advance(3_600_000);
        engine.tick().await.unwrap();
        assert_eq!(engine.record(&run).await.unwrap().stations["ok"].status, StationStatus::Working);
        clock.advance(3_600_000);
        engine.tick().await.unwrap();
        let rec = engine.record(&run).await.unwrap();
        assert_eq!(rec.stations["ok"].status, StationStatus::Unmet);
        assert_eq!(rec.stations["ok"].checks[0].detail, "no decision within 2h");
    }

    #[test]
    fn durations_read_as_definitions_write_them() {
        use crate::kernel::def::duration_ms;
        assert_eq!(duration_ms("90s"), Some(90_000));
        assert_eq!(duration_ms("1h30m"), Some(5_400_000));
        assert_eq!(duration_ms("3d"), Some(3 * 86_400_000));
        assert_eq!(duration_ms("30"), None);
        assert_eq!(duration_ms("0m"), None);
        assert_eq!(duration_ms("2w"), None);
    }
}
