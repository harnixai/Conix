//! A `foreach` station's items from a script and in an order (`after`), and the `task_exit` gates a
//! task's hand-in passes before it is taken — the kernel's side, the runner mocked by hand: work
//! items read off the outcomes, results reported as the runner would.

mod common;

use common::*;
use harnix_flow::*;
use serde_json::{json, Value};

fn ordered() -> MemPacks {
    packs().flow(
        "name: ordered\ninputs: {items: {schema: {type: array}}}\nstations:\n  - id: work\n    foreach: '${inputs.items}'\n    executor: {kind: program, step: work}\n    output: {type: object}\n    exit: [{kind: output}]\n",
    )
}

fn task_ids(o: &Outcome) -> Vec<String> {
    jobs(o).into_iter().map(|j| j.task).collect()
}

#[tokio::test]
async fn an_item_after_others_is_dispatched_once_they_are_done() {
    let h = harness(ordered());
    let items = json!([{"id": "a"}, {"id": "b", "after": ["a"]}, {"id": "c", "after": ["a", "b"]}, {"id": "d"}]);
    let (run, o) = start_with(&h, "ordered", json!({"items": items})).await;
    assert_eq!(task_ids(&o), ["a", "d"], "only those after nothing");
    let js = jobs(&o);
    let o = submit(&h, &js[1], json!({})).await;
    assert!(task_ids(&o).is_empty(), "d being done frees nobody");
    let o = submit(&h, &js[0], json!({})).await;
    assert_eq!(task_ids(&o), ["b"]);
    let o = submit(&h, &jobs(&o)[0], json!({})).await;
    assert_eq!(task_ids(&o), ["c"]);
    submit(&h, &jobs(&o)[0], json!({})).await;
    assert_eq!(status(&h, &run, "work").await, StationStatus::Passed);
}

#[tokio::test]
async fn after_keeps_within_max_parallel_and_a_failed_item_fails_what_comes_after_it() {
    let h = harness(packs().flow(
        "name: ordered\ninputs: {items: {schema: {type: array}}}\nstations:\n  - id: work\n    foreach: '${inputs.items}'\n    max_parallel: 1\n    executor: {kind: program, step: work}\n    output: {type: object}\n    exit: [{kind: output}]\n",
    ));
    let items = json!([{"id": "b", "after": ["a"]}, {"id": "a"}, {"id": "c", "after": ["b"]}, {"id": "d"}]);
    let (run, o) = start_with(&h, "ordered", json!({"items": items})).await;
    // b is first in the list but waits; a takes the one place.
    assert_eq!(task_ids(&o), ["a"]);
    let j = jobs(&o)[0].clone();
    let o = h.eng.fail(&run, "work", "a", &j.token, "broke", Actor::flow("dispatch")).await.unwrap();
    // a failed for good: b and c cannot start and fail with it; d still runs.
    assert_eq!(task_ids(&o), ["d"]);
    let b = task(&h, &run, "work", "b").await;
    assert_eq!((b.status, b.error.as_deref()), (TaskStatus::Failed, Some("it comes after a, which failed")));
    assert_eq!(task(&h, &run, "work", "c").await.error.as_deref(), Some("it comes after b, which failed"));
    submit(&h, &jobs(&o)[0], json!({})).await;
    assert_eq!(status(&h, &run, "work").await, StationStatus::Unmet);
}

#[tokio::test]
async fn an_after_naming_no_item_or_going_round_leaves_the_station_unmet_on_entry() {
    for (items, says) in [
        (json!([{"id": "a", "after": ["z"]}]), "after z: no item has that id"),
        (json!([{"id": "a", "after": ["b"]}, {"id": "b", "after": ["a"]}]), "goes round"),
    ] {
        let h = harness(ordered());
        let (run, o) = start_with(&h, "ordered", json!({"items": items})).await;
        assert!(jobs(&o).is_empty());
        let v = h.eng.get(&run).await.unwrap();
        assert_eq!(v.record.stations["work"].status, StationStatus::Unmet);
        let c = v.unmet["work"].iter().find(|c| c.kind == "foreach").expect("the foreach check");
        assert!(c.detail.contains(says), "{}", c.detail);
        // Judged again (an evaluate), it stays so.
        h.eng.evaluate(&run).await.unwrap();
        assert_eq!(status(&h, &run, "work").await, StationStatus::Unmet);
    }
}

#[tokio::test]
async fn an_external_item_after_another_is_not_claimed_before_it() {
    let h = harness(packs().flow(
        "name: ext\nstations:\n  - id: work\n    foreach: '${inputs.items}'\n    executor: {kind: external}\n    output: {type: object}\n    exit: [{kind: output}]\ninputs: {items: {schema: {type: array}}}\n",
    ));
    let (run, _) = start_with(&h, "ext", json!({"items": [{"id": "a"}, {"id": "b", "after": ["a"]}]})).await;
    let e = h.eng.claim(&run, "work", "b", me(), 60_000).await.unwrap_err();
    assert!(e.to_string().contains("comes after"), "{e}");
    let t = h.eng.claim(&run, "work", "a", me(), 60_000).await.unwrap();
    let token = t.value["token"].as_str().unwrap().to_string();
    h.eng.submit(&run, "work", "a", &token, json!({}), me()).await.unwrap();
    h.eng.claim(&run, "work", "b", me(), 60_000).await.unwrap();
}

// ── items from a script ─────────────────────────────────────────────────────

fn scripted() -> MemPacks {
    packs()
        .part(PartKind::Script, "pending", "export default function (input) { return input.inputs.n; }")
        .flow("name: scripted\ninputs: {n: {schema: {}}}\nstations:\n  - id: work\n    foreach: {script: pending}\n    executor: {kind: program, step: work}\n    output: {type: object}\n    exit: [{kind: output}]\n")
}

fn items_work(o: &Outcome) -> Option<(String, String, u32, String)> {
    o.work.iter().find_map(|w| match w {
        WorkItem::Items { run, station, round, script } => Some((run.clone(), station.clone(), *round, script.clone())),
        _ => None,
    })
}

#[tokio::test]
async fn a_scripts_items_are_asked_for_on_entry_and_dispatched_when_they_come() {
    let h = harness(scripted());
    let (run, o) = start_with(&h, "scripted", json!({"n": 0})).await;
    let (_, station, round, script) = items_work(&o).expect("the items are asked for");
    assert_eq!((station.as_str(), round, script.as_str()), ("work", 1, "pending"));
    assert!(jobs(&o).is_empty());
    let rec = h.eng.record(&run).await.unwrap();
    assert_eq!(rec.parts.scripts.get("pending").map(|s| s.contains("export")), Some(true), "pinned with the run");
    // Nothing is judged meanwhile: an output exit over no tasks would pass.
    h.eng.evaluate(&run).await.unwrap();
    assert_eq!(status(&h, &run, "work").await, StationStatus::Working);
    // After a restart it is asked for again.
    let again = h.eng.dispatched_work(&rec, h.clock.now_ms());
    assert!(again.iter().any(|w| matches!(w, WorkItem::Items { .. })), "{again:?}");
    // The list comes: tasks made, dispatched; a late copy of it is not taken.
    let o = h.eng.foreach_items(&run, "work", 1, Ok(json!([{"id": "x"}, {"id": "y", "after": ["x"]}]))).await.unwrap();
    assert_eq!(o.value["applied"], true);
    assert_eq!(task_ids(&o), ["x"]);
    let late = h.eng.foreach_items(&run, "work", 1, Ok(json!([]))).await.unwrap();
    assert_eq!(late.value["applied"], false);
    let o = submit(&h, &jobs(&o)[0], json!({})).await;
    submit(&h, &jobs(&o)[0], json!({})).await;
    assert_eq!(status(&h, &run, "work").await, StationStatus::Passed);
}

#[tokio::test]
async fn a_script_that_throws_or_gives_no_list_leaves_the_station_unmet() {
    for (got, says) in [
        (Err("the script failed: boom".to_string()), "boom"),
        (Ok(json!({"not": "a list"})), "is not a list"),
        (Ok(json!([{"id": "a", "after": ["a"]}])), "after itself"),
    ] {
        let h = harness(scripted());
        let (run, _) = start_with(&h, "scripted", json!({"n": 0})).await;
        let o = h.eng.foreach_items(&run, "work", 1, got).await.unwrap();
        assert!(has(&o, "station.unmet:work"), "{:?}", kinds(&o));
        let v = h.eng.get(&run).await.unwrap();
        let c = v.unmet["work"].iter().find(|c| c.kind == "foreach").unwrap();
        assert!(c.detail.contains(says) && c.detail.contains("script pending"), "{}", c.detail);
        h.eng.evaluate(&run).await.unwrap();
        assert_eq!(status(&h, &run, "work").await, StationStatus::Unmet);
    }
}

#[tokio::test]
async fn an_empty_list_from_a_script_passes_like_an_empty_reference() {
    let h = harness(scripted());
    let (run, _) = start_with(&h, "scripted", json!({"n": 0})).await;
    h.eng.foreach_items(&run, "work", 1, Ok(json!([]))).await.unwrap();
    assert_eq!(status(&h, &run, "work").await, StationStatus::Passed);
}

// ── task_exit ───────────────────────────────────────────────────────────────

fn exited(executor: &str) -> MemPacks {
    packs().flow(&format!(
        "name: exited\nstations:\n  - id: work\n    foreach: '${{inputs.items}}'\n    executor: {executor}\n    retry: {{max: 1}}\n    output: {{type: object}}\n    task_exit: [{{kind: gate, gate: lint}}, {{kind: gate, gate: per-item}}]\n    exit: [{{kind: output}}]\ninputs: {{items: {{schema: {{type: array}}}}}}\n"
    ))
    .gate_def("{name: per-item, kind: tool, tool: count, args: {item: '${item.id}', task: '${task}'}, assert: [{path: total, eq: 0}]}")
}

fn task_gates(o: &Outcome) -> Vec<(String, u32, usize, String)> {
    o.work
        .iter()
        .filter_map(|w| match w {
            WorkItem::TaskGate { task, seq, cond, gate, .. } => Some((task.clone(), *seq, *cond, gate.clone())),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn a_hand_in_is_taken_once_its_tasks_gates_pass_with_the_tasks_own_context() {
    let h = harness(exited("{kind: program, step: work}"));
    h.tools.set("count", json!({"total": 0}));
    let (run, o) = start_with(&h, "exited", json!({"items": [{"id": "p1"}]})).await;
    let j = jobs(&o)[0].clone();
    let o = submit(&h, &j, json!({"n": 1})).await;
    assert_eq!(o.value["checking"], true);
    // The tool gate judged at once, with the task's item; the host gate handed out.
    assert_eq!(h.tools.calls.lock().unwrap().last().unwrap().1, json!({"item": "p1", "task": "p1"}));
    assert_eq!(task_gates(&o), [("p1".to_string(), 1, 0, "lint".to_string())]);
    let t = task(&h, &run, "work", "p1").await;
    assert_eq!(t.status, TaskStatus::Claimed);
    assert!(t.output.is_none() && t.checking());
    // Its lease runs out while it waits: the wait is the flow's, nothing expires.
    h.clock.advance(120_000);
    assert!(h.eng.tick().await.unwrap().is_empty());
    // A restart hands the gate out again.
    let rec = h.eng.record(&run).await.unwrap();
    let again = h.eng.dispatched_work(&rec, h.clock.now_ms());
    assert!(again.iter().any(|w| matches!(w, WorkItem::TaskGate { gate, seq: 1, .. } if gate == "lint")), "{again:?}");
    // Handing in again meanwhile is refused.
    let e = h.eng.submit(&run, "work", "p1", &j.token, json!({}), Actor::flow("dispatch")).await;
    assert!(matches!(e, Err(FlowError::Lease(_)) | Err(FlowError::Invalid(_))), "{e:?}");
    // Another hand-in's result, or another gate's, is not this one's.
    let stale = h.eng.task_gate_result(&run, "work", 1, "p1", 2, 0, GateResult::passed("")).await.unwrap();
    assert_eq!(stale.value["applied"], false);
    let o = h.eng.task_gate_result(&run, "work", 1, "p1", 1, 0, GateResult::passed("fine")).await.unwrap();
    assert!(has(&o, "task.checked:work/p1") && has(&o, "task.submitted:work/p1"), "{:?}", kinds(&o));
    let t = task(&h, &run, "work", "p1").await;
    assert_eq!((t.status, t.output.clone()), (TaskStatus::Submitted, Some(json!({"n": 1}))));
    let x = t.task_exit.unwrap();
    assert_eq!((x.status, x.checks.len()), (CheckStatus::Pass, 2));
    assert_eq!(t.delivery.unwrap().by, Actor::flow("dispatch"), "taken as handed in, not by the gate");
    assert_eq!(status(&h, &run, "work").await, StationStatus::Passed);
}

#[tokio::test]
async fn a_program_sent_back_by_its_tasks_gates_fails_and_is_retried() {
    let h = harness(exited("{kind: program, step: work}"));
    h.tools.set("count", json!({"total": 2}));
    let (run, o) = start_with(&h, "exited", json!({"items": [{"id": "p1"}]})).await;
    submit(&h, &jobs(&o)[0], json!({})).await;
    let o = h.eng.task_gate_result(&run, "work", 1, "p1", 1, 0, GateResult::passed("")).await.unwrap();
    // The tool gate failed: the attempt fails, and the retry dispatches it again.
    let t = task(&h, &run, "work", "p1").await;
    assert!(t.error.as_deref().unwrap().starts_with("task_exit: gate per-item"), "{:?}", t.error);
    assert_eq!(t.task_exit.as_ref().unwrap().status, CheckStatus::Fail);
    let again = jobs(&o);
    assert_eq!(again.len(), 1);
    assert_eq!(again[0].attempt, 2);
    // The second attempt fails the same way: out of retries, the station is unmet.
    let o = submit(&h, &again[0], json!({})).await;
    assert_eq!(task_gates(&o)[0].1, 2, "the second hand-in");
    h.eng.task_gate_result(&run, "work", 1, "p1", 2, 0, GateResult::passed("")).await.unwrap();
    assert_eq!(task(&h, &run, "work", "p1").await.status, TaskStatus::Failed);
    assert_eq!(status(&h, &run, "work").await, StationStatus::Unmet);
}

#[tokio::test]
async fn someone_outside_sent_back_keeps_the_claim_and_hands_in_again() {
    let h = harness(exited("{kind: external}"));
    h.tools.set("count", json!({"total": 1}));
    let (run, _) = start_with(&h, "exited", json!({"items": [{"id": "p1"}]})).await;
    let c = h.eng.claim(&run, "work", "p1", me(), 60_000).await.unwrap();
    let token = c.value["token"].as_str().unwrap().to_string();
    h.eng.submit(&run, "work", "p1", &token, json!({}), me()).await.unwrap();
    h.clock.advance(50_000);
    let o = h.eng.task_gate_result(&run, "work", 1, "p1", 1, 0, GateResult::failed("lint: 3 problems")).await.unwrap();
    assert!(has(&o, "task.checked:work/p1"));
    let t = task(&h, &run, "work", "p1").await;
    assert_eq!(t.status, TaskStatus::Claimed, "still theirs, to fix");
    assert!(t.lease.as_ref().unwrap().until >= h.clock.now_ms() + 60_000, "with a whole lease to fix it in");
    let why = t.task_exit.as_ref().unwrap().explain();
    assert!(why.contains("3 problems") && why.contains("per-item"), "{why}");
    // Fixed: handed in again under the same claim, and taken.
    h.tools.set("count", json!({"total": 0}));
    let o = h.eng.submit(&run, "work", "p1", &token, json!({"ok": true}), me()).await.unwrap();
    assert_eq!(o.value["sent_back"], Value::Null);
    h.eng.task_gate_result(&run, "work", 1, "p1", 2, 0, GateResult::passed("")).await.unwrap();
    assert_eq!(task(&h, &run, "work", "p1").await.status, TaskStatus::Submitted);
    assert_eq!(status(&h, &run, "work").await, StationStatus::Passed);
}

#[test]
fn validation_says_what_task_exit_items_and_commands_may_be() {
    let packs = packs().part(PartKind::Script, "pending", "export default function () { return []; }");
    let problems = |yaml: &str| -> String {
        let def = FlowDef::parse(yaml).unwrap();
        validate(&def, &packs).iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
    };
    let ok = "name: f\nstations: [{id: a, foreach: {script: pending}, executor: {kind: program, step: work}, output: {type: object}, task_exit: [{kind: gate, gate: lint}], exit: [{kind: output}]}]";
    assert_eq!(problems(ok), "");
    let p = problems("name: f\nstations: [{id: a, foreach: {script: nope}, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}]");
    assert!(p.contains("no script nope"), "{p}");
    let p = problems("name: f\nstations: [{id: a, executor: {kind: program, step: work}, output: {type: object}, task_exit: [{kind: confirm}, {kind: gate, gate: nope}], exit: [{kind: output}]}]");
    assert!(p.contains("task_exit holds only gates") && p.contains("gate nope is not in the packs"), "{p}");
    let p = problems("name: f\nstations: [{id: a, task_exit: [{kind: gate, gate: lint}], exit: [{kind: confirm}]}]");
    assert!(p.contains("a checkpoint has no tasks"), "{p}");
    let p = problems("name: f\nstations: [{id: a, executor: {kind: command, command: x}, output: {type: object}, exit: [{kind: output}]}]");
    assert!(p.contains("a command is a bundle's"), "{p}");
    // Read as before: a reference.
    let def = FlowDef::parse("name: f\nstations: [{id: a, foreach: '${inputs.x}', executor: {kind: program, step: work}, exit: [{kind: confirm}]}]").unwrap();
    assert_eq!(def.stations[0].foreach, Some(Foreach::Ref("${inputs.x}".into())));
    assert_eq!(serde_json::to_value(&def.stations[0].foreach).unwrap(), json!("${inputs.x}"));
}
