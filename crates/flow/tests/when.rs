//! Exits required only in some cases (docs/design/flows.md §4.2, in Conix): a `when` that reads a
//! gate's facts or a command step's output. A definite yes requires the exit, a definite no leaves it
//! out and says so on the station; everything else — no result, a gate that errored, a fact not
//! there or of another kind — requires it. A rework that changes the fact changes the requirement.
//! What validation refuses: a step that is not a command step, a gate no exit before it judges.

mod common;

use std::sync::Arc;

use common::*;
use harnix_flow::*;
use serde_json::{json, Value};

fn person(id: &str) -> Actor {
    Actor::new("person", id)
}

fn flows() -> MemPacks {
    packs()
        .check("compat")
        .gate_def("{name: compat, kind: host}")
        .flow("name: one\nstations:\n  - id: a\n    exit:\n      - {kind: gate, gate: compat}\n      - {kind: confirm, label: approve breaking, when: {gate: compat, fact: breaking}}")
        .flow(
            "name: two\nstations:\n  - {id: a, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}, {kind: gate, gate: compat}]}\n  - id: b\n    rework_to: [a]\n    exit:\n      - {kind: gate, gate: lint}\n      - {kind: confirm, label: approve breaking, when: {gate: compat, fact: breaking}}",
        )
}

fn with_facts(facts: Value) -> GateResult {
    let mut r = GateResult::passed("judged");
    r.facts = facts.as_object().unwrap().clone();
    r
}

async fn view(h: &H, run: &str) -> RunView {
    h.eng.get(run).await.unwrap()
}

/// The check of exit `cond` of `station`, as the latest judgement left it.
async fn check(h: &H, run: &str, station: &str, cond: usize) -> CheckRec {
    let v = view(h, run).await;
    v.record.stations[station].checks.iter().find(|c| c.cond == Some(cond)).cloned().unwrap_or_else(|| panic!("no check {cond}: {:?}", v.record.stations[station].checks))
}

#[tokio::test]
async fn a_true_fact_requires_the_confirmation_and_waits_for_it() {
    let h = harness(flows());
    let (run, o) = start(&h, "one").await;
    assert!(o.work.iter().any(|w| matches!(w, WorkItem::Gate { gate, .. } if gate == "compat")));
    // Until the gate says, the confirmation waits with it and asks for nothing.
    let c = check(&h, &run, "a", 1).await;
    assert_eq!(c.status, CheckStatus::Wait);
    assert_eq!(c.when.as_ref().unwrap().required, None, "{c:?}");
    assert!(c.detail.contains("waiting for gate compat"), "{c:?}");

    h.eng.gate_result(&run, "a", 1, "compat", with_facts(json!({"breaking": true}))).await.unwrap();
    let c = check(&h, &run, "a", 1).await;
    assert_eq!((c.status, c.when.as_ref().unwrap().required), (CheckStatus::Wait, Some(true)));
    assert_eq!(c.detail, "awaiting confirmation");
    // What a person reads in the station: why it is required.
    let v = view(&h, &run).await;
    let why = v.unmet["a"].iter().find(|c| c.cond == Some(1)).unwrap().explain();
    assert!(why.contains("required: gate compat fact breaking is true"), "{why}");
    assert_eq!(v.votes["a"].len(), 1);
    // The gate's facts are kept with its result.
    assert_eq!(check(&h, &run, "a", 0).await.facts["breaking"], json!(true));
    assert!(v.record.stations["a"].not_required.is_empty());

    let o = h.eng.confirm(&run, "a", true, None, person("owner")).await.unwrap();
    assert!(has(&o, "run.finished"), "{:?}", kinds(&o));
}

#[tokio::test]
async fn a_definite_false_leaves_it_out_on_the_record() {
    let h = harness(flows());
    let (run, _) = start(&h, "one").await;
    let o = h.eng.gate_result(&run, "a", 1, "compat", with_facts(json!({"breaking": false}))).await.unwrap();
    assert!(has(&o, "run.finished"), "{:?}", kinds(&o));
    let v = view(&h, &run).await;
    let st = &v.record.stations["a"];
    assert_eq!(st.not_required.len(), 1, "{st:?}");
    let n = &st.not_required[0];
    assert_eq!((n.cond, n.label.as_str(), n.round, &n.value), (1, "approve breaking", 1, &json!(false)));
    assert_eq!(n.when.gate.as_deref(), Some("compat"));
    let c = check(&h, &run, "a", 1).await;
    assert_eq!(c.status, CheckStatus::Pass);
    assert_eq!(c.detail, "not required: gate compat fact breaking is false, not is true");
    assert_eq!(c.when.as_ref().unwrap().required, Some(false));
    // No one is asked for a vote it does not need.
    assert!(!v.votes.contains_key("a"), "{:?}", v.votes);
}

#[tokio::test]
async fn no_definite_answer_requires_it() {
    for (what, result, says) in [
        ("an errored gate", GateResult::errored("the script threw"), "gate compat of a did not conclude (the script threw)"),
        ("no such fact", with_facts(json!({"other": false})), "gate compat of a gave no fact breaking"),
        ("a fact of another kind", with_facts(json!({"breaking": "no"})), "gate compat fact breaking is \"no\", not true or false"),
        ("a list where a boolean is read", with_facts(json!({"breaking": [false]})), "not true or false"),
    ] {
        let h = harness(flows());
        let (run, _) = start(&h, "one").await;
        h.eng.gate_result(&run, "a", 1, "compat", result).await.unwrap();
        let c = check(&h, &run, "a", 1).await;
        let w = c.when.clone().unwrap();
        assert_eq!(w.required, Some(true), "{what}: {c:?}");
        assert!(w.why.contains(says) && w.why.ends_with("required until it says otherwise"), "{what}: {}", w.why);
        assert_ne!(c.status, CheckStatus::Pass, "{what}");
        assert!(view(&h, &run).await.record.stations["a"].not_required.is_empty(), "{what}");
    }
    // A gate result whose facts are not scalars is no gate result: it does not deserialize.
    for bad in [json!({"verdict": "pass", "facts": {"x": null}}), json!({"verdict": "pass", "facts": {"x": {"y": 1}}}), json!({"verdict": "pass", "facts": {"x": [[1]]}})] {
        let e = serde_json::from_value::<GateResult>(bad.clone()).unwrap_err().to_string();
        assert!(e.contains("a fact is a string, number or boolean, or a list of them"), "{bad}: {e}");
    }
    let ok: GateResult = serde_json::from_value(json!({"verdict": "pass", "facts": {"breaking": true, "ids": ["a", "b"], "n": 2}})).unwrap();
    assert_eq!(ok.facts.len(), 3);
}

#[tokio::test]
async fn a_rework_that_changes_the_fact_changes_the_requirement() {
    let h = harness(flows());
    let (run, o) = start(&h, "two").await;
    let j = &jobs(&o)[0];
    submit(&h, j, json!({})).await;
    h.eng.gate_result(&run, "a", 1, "compat", with_facts(json!({"breaking": false}))).await.unwrap();
    // b reads a's latest result: not required; its own gate fails.
    let o = h.eng.gate_result(&run, "b", 1, "lint", GateResult::failed("lint fails")).await.unwrap();
    assert!(has(&o, "station.unmet:b"), "{:?}", kinds(&o));
    let st = view(&h, &run).await.record.stations["b"].clone();
    assert_eq!(st.not_required.iter().map(|n| n.cond).collect::<Vec<_>>(), [1]);

    // Done again, a's change turns out breaking: b now waits for the person.
    let o = h.eng.rework(&run, "a", None, "redo it", me()).await.unwrap();
    let j = &jobs(&o)[0];
    submit(&h, j, json!({})).await;
    h.eng.gate_result(&run, "a", 2, "compat", with_facts(json!({"breaking": true}))).await.unwrap();
    let round = view(&h, &run).await.record.stations["b"].round;
    h.eng.gate_result(&run, "b", round, "lint", GateResult::passed("")).await.unwrap();
    let v = view(&h, &run).await;
    assert_eq!(v.record.stations["b"].status, StationStatus::Working);
    assert!(v.record.stations["b"].not_required.is_empty());
    let c = check(&h, &run, "b", 1).await;
    assert_eq!((c.status, c.when.unwrap().required), (CheckStatus::Wait, Some(true)));
    let o = h.eng.confirm(&run, "b", true, None, person("owner")).await.unwrap();
    assert!(has(&o, "run.finished"));
}

#[test]
fn the_comparisons_read_a_value_or_a_list_and_take_another_kind_as_no_answer() {
    let w = |yaml: &str| serde_yaml::from_str::<ExitWhen>(yaml).unwrap();
    let plain = w("{gate: g, fact: f}");
    assert_eq!(plain.answer(&json!(true)), WhenAnswer::Yes);
    assert_eq!(plain.answer(&json!(false)), WhenAnswer::No);
    assert!(matches!(plain.answer(&json!(1)), WhenAnswer::Unsure(_)));
    let eq = w("{gate: g, fact: f, equals: major}");
    assert_eq!(eq.answer(&json!("major")), WhenAnswer::Yes);
    assert_eq!(eq.answer(&json!("minor")), WhenAnswer::No);
    assert_eq!(eq.answer(&json!(["minor", "major"])), WhenAnswer::Yes, "a list holding it");
    assert_eq!(eq.answer(&json!([])), WhenAnswer::No, "an empty list holds nothing");
    assert!(matches!(eq.answer(&json!(3)), WhenAnswer::Unsure(_)), "a number is not compared with a word");
    assert!(matches!(eq.answer(&json!(["major", 3])), WhenAnswer::Unsure(_)));
    let any = w("{step: s, path: p, in: [2, 3]}");
    assert_eq!(any.answer(&json!(3.0)), WhenAnswer::Yes, "numbers compare as numbers");
    assert_eq!(any.answer(&json!(1)), WhenAnswer::No);
    assert!(matches!(any.answer(&json!(null)), WhenAnswer::Unsure(_)));
}

// ── a command step's output ──────────────────────────────────────────────────

fn bundle(dir: &std::path::Path) {
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", "name: pk\ncommands: [probe]\n"),
        ("commands/probe.js", "export default function () { return {}; }\n"),
        (
            "step.yaml",
            "name: step\nstations:\n  - {id: measure, executor: {kind: command, command: probe}, output: {type: object}, exit: [{kind: output}]}\n  - id: sign\n    exit: [{kind: confirm, label: high risk, when: {step: measure, path: risk.level, in: [high, critical]}}]\n",
        ),
    ];
    for (p, t) in files {
        let path = dir.join("pk").join(p);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, t).unwrap();
    }
}

fn dir_engine(dir: &std::path::Path) -> Engine {
    let packs = DirPacks::new([dir]).with_catalog(packs().catalog());
    Engine::new(Arc::new(MemStore::new()), Arc::new(packs)).with_clock(Arc::new(ManualClock::new(T0)))
}

#[tokio::test]
async fn a_command_steps_output_decides_and_nothing_there_requires_it() {
    let d = tempfile::tempdir().unwrap();
    bundle(d.path());
    let eng = dir_engine(d.path());
    let flow = "pk/step";
    let (def, _) = eng.packs().flow(flow).unwrap();
    assert_eq!(validate(&def, eng.packs().as_ref()), Vec::<Problem>::new());
    for (output, finished, says) in [
        (json!({"risk": {"level": "low"}}), true, "not required: step measure output risk.level is \"low\", not in [\"high\",\"critical\"]"),
        (json!({"risk": {"level": "high"}}), false, "required: step measure output risk.level is \"high\""),
        (json!({"risk": {}}), false, "required: step measure gave nothing at risk.level"),
    ] {
        let o = eng.create(CreateIn { flow: flow.into(), subject: "s".into(), ..Default::default() }, me()).await.unwrap();
        let run = o.value["run"].as_str().unwrap().to_string();
        let (station, task, token) = o.work.iter().find_map(|w| match w {
            WorkItem::Task { station, task, token, .. } => Some((station.clone(), task.clone(), token.clone())),
            _ => None,
        }).unwrap();
        eng.submit(&run, &station, &task, &token, output.clone(), Actor::flow("dispatch")).await.unwrap();
        let v = eng.get(&run).await.unwrap();
        assert_eq!(v.record.status == RunStatus::Finished, finished, "{output}: {:?}", v.record.stations["sign"]);
        let c = &v.record.stations["sign"].checks[0];
        let read = if finished { c.detail.clone() } else { c.explain() };
        assert!(read.contains(says), "{output}: {read}");
    }
}

// ── validation ───────────────────────────────────────────────────────────────

#[test]
fn validation_keeps_when_to_what_the_flow_computes_and_judges_by_then() {
    let p = flows().check("files-there").gate_def("{name: files-there, kind: check, check: files, paths: [a.md]}");
    let problems = |stations: &str| -> Vec<String> {
        validate_yaml(&format!("name: f\nstations:\n{stations}"), &p).iter().map(ToString::to_string).collect()
    };
    let one = |exit: &str| problems(&format!("  - {{id: a, exit: [{{kind: gate, gate: compat}}, {exit}]}}"));
    assert_eq!(one("{kind: confirm, when: {gate: compat, fact: breaking}}"), Vec::<String>::new());
    assert_eq!(one("{kind: confirm, when: {gate: compat, fact: level, in: [major, critical]}}"), Vec::<String>::new());
    // a gate of a station before it
    assert_eq!(
        problems("  - {id: a, exit: [{kind: gate, gate: compat}]}\n  - {id: b, exit: [{kind: confirm, when: {gate: compat, fact: breaking}}]}"),
        Vec::<String>::new()
    );
    let has = |ps: Vec<String>, says: &str| assert!(ps.iter().any(|p| p.contains(says)), "{says}: {ps:?}");
    // a gate no exit judges by then: not an exit at all, or one of a later station
    has(problems("  - {id: a, exit: [{kind: confirm, when: {gate: compat, fact: breaking}}]}"), "is no exit of this station or of a station before it");
    has(
        problems("  - {id: a, exit: [{kind: confirm, when: {gate: compat, fact: breaking}}]}\n  - {id: b, exit: [{kind: gate, gate: compat}]}"),
        "is no exit of this station or of a station before it",
    );
    // a gate whose own exit is conditional, and the exit reading itself
    has(problems("  - {id: a, exit: [{kind: gate, gate: lint}, {kind: gate, gate: compat, when: {gate: lint, fact: x}}, {kind: confirm, when: {gate: compat, fact: y}}]}"), "is no exit of this station");
    has(one("{kind: gate, gate: compat, when: {gate: compat, fact: x}}"), "is this exit itself");
    // a gate that gives no facts
    has(problems("  - {id: a, exit: [{kind: gate, gate: files-there}, {kind: confirm, when: {gate: files-there, fact: x}}]}"), "is a check gate, which gives no facts");
    // the shape
    has(one("{kind: confirm, when: {gate: compat}}"), "says which fact");
    has(one("{kind: confirm, when: {gate: compat, fact: x, step: a, path: p}}"), "reads one thing");
    has(one("{kind: confirm, when: {fact: x}}"), "reads one thing");
    has(one("{kind: confirm, when: {gate: compat, fact: x, equals: a, in: [b]}}"), "equals or in, not both");
    has(one("{kind: confirm, when: {gate: compat, fact: x, in: []}}"), "not empty");
    has(one("{kind: confirm, when: {gate: compat, fact: x, equals: [a]}}"), "equals is a string");
    // a step that is not a command step: what a session hands in is the model's
    has(
        problems("  - {id: a, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}\n  - {id: b, exit: [{kind: confirm, when: {step: a, path: risky}}]}"),
        "is not a command step",
    );
    has(problems("  - {id: a, exit: [{kind: confirm, when: {step: nope, path: x}}]}"), "step nope is not a station");
    has(
        problems("  - {id: a, exit: [{kind: confirm, when: {step: b, path: x}}]}\n  - {id: b, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}"),
        "neither this station nor one before it",
    );
    // a task's hand-in has no when
    has(
        problems("  - {id: a, executor: {kind: program, step: work}, output: {type: object}, task_exit: [{kind: gate, gate: compat, when: {gate: compat, fact: x}}], exit: [{kind: output}]}"),
        "a when goes on the station's exit",
    );
    // an unknown field in when is a parse error
    assert!(!one("{kind: confirm, when: {gate: compat, fact: x, is: true}}").is_empty());
}

#[test]
fn a_definition_without_when_reads_and_writes_as_before() {
    let def = FlowDef::parse("name: f\nstations: [{id: a, exit: [{kind: confirm, who: owner}, {kind: gate, gate: lint}]}]").unwrap();
    let out = serde_json::to_value(&def).unwrap();
    assert!(!out.to_string().contains("when"), "{out}");
    let def = FlowDef::parse("name: f\nstations: [{id: a, exit: [{kind: gate, gate: compat}, {kind: confirm, when: {gate: compat, fact: breaking}}]}]").unwrap();
    assert_eq!(def.stations[0].exit[1].when().and_then(|w| w.fact.as_deref()), Some("breaking"));
    assert!(def.stations[0].exit[0].when().is_none());
}
