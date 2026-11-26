//! Starting by a bundle's name (route, triage), switching a run to another flow of its bundle,
//! stepping through stations, the rework points gates name, and what stations' sessions propose
//! (in Conix: the flow refactor's §4 and §13).

mod common;

use std::path::Path;
use std::sync::Arc;

use common::*;
use harnix_flow::*;
use serde_json::{json, Value};

const STATION: &str = "executor: {kind: program, step: work}\n    output: {type: object}\n    exit: [{kind: output}]";

/// A bundle `dev`: `quick` (risk low), `solid` (low or medium), `major` (high; it starts with
/// `clarify`, then `propose`), and a triage skill.
fn write_dev(dir: &Path, bundle_yaml: &str) {
    let flow = |name: &str, risk: &str, stations: &[&str]| {
        let mut t = format!("name: {name}\ntitle: {name} 路径\ninputs:\n  risk: {{schema: {{enum: [{risk}]}}}}\n  note: {{schema: {{type: string}}, default: ''}}\nstations:\n");
        for s in stations {
            t.push_str(&format!("  - id: {s}\n    {STATION}\n"));
        }
        t
    };
    let files = [
        ("bundle.yaml".to_string(), bundle_yaml.to_string()),
        ("quick.yaml".into(), flow("quick", "low", &["propose", "apply"])),
        ("solid.yaml".into(), flow("solid", "low, medium", &["propose", "design", "apply"])),
        ("major.yaml".into(), flow("major", "high", &["clarify", "propose", "apply"])),
        ("skills/triage/SKILL.md".into(), "---\nname: triage\n---\n按改动的范围判风险。\n".into()),
    ];
    for (p, t) in files {
        let f = dir.join("dev").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
}

fn bundle_engine(bundle_yaml: &str) -> (tempfile::TempDir, Engine) {
    let d = tempfile::tempdir().unwrap();
    write_dev(d.path(), bundle_yaml);
    let catalog = Catalog { steps: ["work".to_string()].into(), ..Default::default() };
    let packs = DirPacks::new([d.path()]).with_catalog(catalog);
    let eng = Engine::new(Arc::new(MemStore::new()), Arc::new(packs)).with_clock(Arc::new(ManualClock::new(T0)));
    (d, eng)
}

const DEV: &str = "name: dev\nflows: [quick, solid, major]\nstart: {triage: triage}\n";

async fn create(eng: &Engine, input: CreateIn) -> (String, Outcome) {
    let o = eng.create(input, me()).await.expect("create");
    (o.value["run"].as_str().unwrap().to_string(), o)
}

fn start(flow: &str, inputs: Value) -> CreateIn {
    CreateIn { flow: flow.into(), subject: "s".into(), inputs, ..Default::default() }
}

#[tokio::test]
async fn a_bundles_name_routes_to_the_first_flow_that_takes_the_inputs() {
    let (_d, eng) = bundle_engine(DEV);
    let Route::Flow { flow, by, reason } = eng.route("dev", &json!({"risk": "medium"}), None).unwrap() else { panic!() };
    assert_eq!((flow.as_str(), by), ("dev/solid", RoutedBy::Route));
    assert!(reason.contains("dev/quick: input risk"), "{reason}");
    let Route::Flow { flow, .. } = eng.route("dev", &json!({"risk": "low"}), None).unwrap() else { panic!() };
    assert_eq!(flow, "dev/quick");
    let Route::Flow { by, .. } = eng.route("dev/major", &json!({}), None).unwrap() else { panic!() };
    assert_eq!(by, RoutedBy::Named);

    // Nothing decides: triage, with every flow and what it takes.
    let Route::Triage { skill, candidates, .. } = eng.route("dev", &json!({"note": "x"}), None).unwrap() else { panic!() };
    assert_eq!(skill, "dev/triage");
    assert_eq!(candidates.iter().map(|c| c.flow.as_str()).collect::<Vec<_>>(), ["dev/quick", "dev/solid", "dev/major"]);
    assert_eq!(candidates[0].inputs["properties"]["risk"]["enum"], json!(["low"]));
    // A value no flow takes is not for triage: it is refused, saying what each takes.
    let e = eng.route("dev", &json!({"risk": "extreme"}), None).unwrap_err().to_string();
    assert!(e.contains("no flow of bundle dev takes these inputs") && e.contains("dev/major takes"), "{e}");
    assert!(matches!(eng.route("nope", &json!({}), None), Err(FlowError::NotFound(_))));

    // A start of the bundle is a run of the flow it routes to, recorded so.
    let (run, _) = create(&eng, start("dev", json!({"risk": "medium"}))).await;
    let rec = eng.record(&run).await.unwrap();
    assert_eq!(rec.flow, "dev/solid");
    assert_eq!(rec.routed.as_ref().unwrap().by, RoutedBy::Route);
    // What a model judged by triage, and why, is kept.
    let judged = RoutedRec { by: RoutedBy::Triage, reason: Some("只改一个文件".into()) };
    let (run, _) = create(&eng, CreateIn { routed: Some(judged), ..start("dev", json!({"risk": "low"})) }).await;
    let r = eng.record(&run).await.unwrap().routed.unwrap();
    assert_eq!(r.by, RoutedBy::Triage);
    assert!(r.reason.unwrap().starts_with("只改一个文件; dev/quick is the first"));
    // A start that needs triage starts nothing.
    let e = eng.create(start("dev", json!({})), me()).await.unwrap_err().to_string();
    assert!(e.contains("judge them by the skill dev/triage"), "{e}");
    let (run, _) = create(&eng, start("dev/major", json!({"risk": "high"}))).await;
    assert_eq!(eng.record(&run).await.unwrap().routed.unwrap().by, RoutedBy::Named);
}

#[tokio::test]
async fn without_a_triage_skill_inputs_that_do_not_decide_are_refused_and_the_route_is_checked() {
    let (_d, eng) = bundle_engine("name: dev\nstart: {route: [solid, quick]}\n");
    let e = eng.route("dev", &json!({}), None).unwrap_err().to_string();
    assert!(e.contains("give the inputs that decide") && e.contains("dev/solid takes"), "{e}");
    // The route's order, not the flows' names.
    let Route::Flow { flow, .. } = eng.route("dev", &json!({"risk": "low"}), None).unwrap() else { panic!() };
    assert_eq!(flow, "dev/solid");

    let (_d, eng) = bundle_engine("name: dev\nflows: [quick, nope]\nstart: {route: [solid], triage: gone}\n");
    let (def, _) = eng.packs().flow("dev/quick").unwrap();
    let problems: Vec<String> = validate(&def, eng.packs().as_ref()).iter().map(ToString::to_string).collect();
    let all = problems.join("\n");
    assert!(all.contains("flows names nope"), "{all}");
    assert!(all.contains("start.route names solid, which is not one of its flows"), "{all}");
    assert!(all.contains("start.triage names the skill gone"), "{all}");
}

#[tokio::test]
async fn a_run_switches_flows_of_its_bundle_while_its_first_station_is_open() {
    let (_d, eng) = bundle_engine(DEV);
    let (run, o) = create(&eng, start("dev/quick", json!({"risk": "low", "note": "n"}))).await;
    let first = jobs(&o);
    assert_eq!(first[0].station, "propose");

    let e = eng.switch(&run, "major", "大", None, me()).await.unwrap_err().to_string();
    assert!(e.contains("does not start at station propose"), "{e}");
    let e = eng.switch(&run, "solid", "x", Some(json!({"risk": "high"})), me()).await.unwrap_err().to_string();
    assert!(e.contains("the inputs for dev/solid"), "{e}");
    assert!(eng.switch(&run, "solid", " ", None, me()).await.is_err(), "a reason is needed");

    let o = eng.switch(&run, "solid", "要设计", Some(json!({"risk": "medium"})), me()).await.unwrap();
    assert!(o.events.iter().any(|e| e.kind == "run.switched"));
    assert!(o.events.iter().any(|e| e.kind == "task.cancelled"), "the open station's task is cancelled");
    let again = jobs(&o);
    assert_eq!((again[0].station.as_str(), again.len()), ("propose", 1));
    let rec = eng.record(&run).await.unwrap();
    assert_eq!((rec.flow.as_str(), rec.def.name.as_str()), ("dev/solid", "dev/solid"));
    assert_eq!(rec.inputs, json!({"risk": "medium", "note": "n"}), "inputs the new flow knows carry over");
    assert_eq!(rec.switched.len(), 1);
    assert_eq!((rec.switched[0].from.as_str(), rec.switched[0].to.as_str()), ("dev/quick", "dev/solid"));
    assert!(rec.stations.contains_key("design"));
    let p = &rec.stations["propose"];
    assert_eq!(p.round, 2);
    assert!(p.reworked.as_ref().unwrap().reason.contains("switched from flow dev/quick to dev/solid"));
    // The old claim is gone; the new one hands in.
    assert!(eng.submit(&run, "propose", "propose", &first[0].token, json!({}), Actor::flow("dispatch")).await.is_err());
    eng.submit(&run, "propose", "propose", &again[0].token, json!({}), Actor::flow("dispatch")).await.unwrap();
    let e = eng.switch(&run, "quick", "回去", None, me()).await.unwrap_err().to_string();
    assert!(e.contains("past its first station"), "{e}");
}

#[tokio::test]
async fn a_flow_in_no_bundle_has_nothing_to_switch_to() {
    let h = harness(packs().flow("{name: f, stations: [{id: a, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}]}"));
    let (run, _) = start_with(&h, "f", json!({})).await;
    let e = h.eng.switch(&run, "g", "x", None, me()).await.unwrap_err().to_string();
    assert!(e.contains("in no bundle"), "{e}");
}

#[tokio::test]
async fn a_stepped_run_holds_every_station_until_a_person_releases_it() {
    let (_d, eng) = bundle_engine(DEV);
    let (run, o) = create(&eng, CreateIn { pace: Some(Pace::Step), ..start("dev/quick", json!({"risk": "low"})) }).await;
    assert!(jobs(&o).is_empty(), "held: nothing is dispatched");
    let rec = eng.record(&run).await.unwrap();
    assert_eq!((rec.pace, rec.stations["propose"].status), (Pace::Step, StationStatus::Held));
    let o = eng.release(&run, "propose", me()).await.unwrap();
    let j = jobs(&o).remove(0);
    eng.submit(&run, "propose", "propose", &j.token, json!({}), Actor::flow("dispatch")).await.unwrap();
    assert_eq!(eng.record(&run).await.unwrap().stations["apply"].status, StationStatus::Held);

    // The bundle's word, and the flow's own.
    let (_d, eng) = bundle_engine("name: dev\npace: step\n");
    let (run, _) = create(&eng, start("dev/quick", json!({"risk": "low"}))).await;
    assert_eq!(eng.record(&run).await.unwrap().pace, Pace::Step);
    let (run, _) = create(&eng, CreateIn { pace: Some(Pace::Auto), ..start("dev/quick", json!({"risk": "low"})) }).await;
    assert_eq!(eng.record(&run).await.unwrap().stations["propose"].status, StationStatus::Working);
    let h = harness(packs().flow("{name: f, pace: step, stations: [{id: a, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}]}"));
    let (run, _) = start_with(&h, "f", json!({})).await;
    assert_eq!(status(&h, &run, "a").await, StationStatus::Held);
}

/// `a` → `b` → `c`; `c` has two tool gates, `early` naming `a` as its point, `late` none; its
/// automatic rework goes to `b`.
const GATED: &str = r#"name: g
stations:
  - {id: a, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}
  - {id: b, executor: {kind: program, step: work}, output: {type: object}, exit: [{kind: output}]}
  - id: c
    executor: {kind: program, step: work}
    rework_to: [a]
    on_unmet: {rework_to: b, max: 3}
    exit:
      - {kind: gate, gate: early, rework_to: a}
      - {kind: gate, gate: late}
"#;

fn gated() -> H {
    harness(
        packs()
            .tool("t-early")
            .tool("t-late")
            .gate_def("{name: early, kind: tool, tool: t-early, assert: [{path: ok, eq: true}]}")
            .gate_def("{name: late, kind: tool, tool: t-late, assert: [{path: ok, eq: true}]}")
            .flow(GATED),
    )
}

/// Drive the run up to `c`'s first judgement; the jobs it handed out after.
async fn to_c(h: &H, early: bool, late: bool) -> (String, Vec<Job>) {
    h.tools.set("t-early", json!({"ok": early}));
    h.tools.set("t-late", json!({"ok": late}));
    let (run, o) = start_with(h, "g", json!({})).await;
    let mut queue = jobs(&o);
    let mut last = vec![];
    while let Some(j) = queue.first().cloned() {
        queue.remove(0);
        let o = submit(h, &j, json!({})).await;
        if j.station == "c" {
            last = jobs(&o);
            break;
        }
        queue.extend(jobs(&o));
    }
    (run, last)
}

#[tokio::test]
async fn a_failed_gate_sends_the_rework_to_its_own_point_and_the_earliest_wins() {
    let h = gated();
    let (_, back) = to_c(&h, false, true).await;
    assert_eq!(back[0].station, "a", "early failed: its point");
    let h = gated();
    let (run, back) = to_c(&h, true, false).await;
    assert_eq!(back[0].station, "b", "late failed: the station's on_unmet");
    assert_eq!(h.eng.record(&run).await.unwrap().stations["c"].auto_reworks, 1);
    let h = gated();
    let (_, back) = to_c(&h, false, false).await;
    assert_eq!(back[0].station, "a", "both: the earliest");
}

#[tokio::test]
async fn a_gates_rework_point_is_one_of_the_stations_and_needs_on_unmet() {
    let h = gated();
    let check = |yaml: &str| -> String {
        let def = FlowDef::parse(yaml).unwrap();
        validate(&def, h.packs.as_ref()).iter().map(ToString::to_string).collect::<Vec<_>>().join("\n")
    };
    assert_eq!(check(GATED), "");
    let bad = GATED.replace("gate: early, rework_to: a", "gate: early, rework_to: b").replace("rework_to: b, max", "rework_to: c, max");
    assert!(check(&bad).contains("gate early: rework_to b is not one of the station's rework points"), "{}", check(&bad));
    let none = GATED.replace("    on_unmet: {rework_to: b, max: 3}\n", "");
    assert!(check(&none).contains("needs on_unmet"), "{}", check(&none));
    let def = FlowDef::parse(GATED).unwrap();
    assert_eq!(def.station("c").unwrap().rework_points(), ["a", "b"]);
}

#[tokio::test]
async fn a_proposal_is_recorded_and_accepted_by_a_rework_there_or_dismissed() {
    let h = gated();
    h.tools.set("t-early", json!({"ok": true}));
    h.tools.set("t-late", json!({"ok": true}));
    let (run, o) = start_with(&h, "g", json!({})).await;
    let mut j = jobs(&o).remove(0);
    while j.station != "c" {
        j = jobs(&submit(&h, &j, json!({})).await).remove(0);
    }
    let by = Actor::agent("s1");
    let e = h.eng.propose(&run, "c", "c", &j.token, ProposalKind::Rework, Some("zzz"), "x", by.clone()).await.unwrap_err().to_string();
    assert!(e.contains("may go back to a, b only"), "{e}");
    assert!(h.eng.propose(&run, "c", "c", "not-it", ProposalKind::Cancel, None, "x", by.clone()).await.is_err(), "only under the claim");
    let o = h.eng.propose(&run, "c", "c", &j.token, ProposalKind::Rework, Some("a"), "提案就错了", by.clone()).await.unwrap();
    assert!(o.events.iter().any(|e| e.kind == "proposal.opened"));
    h.eng.propose(&run, "c", "c", &j.token, ProposalKind::Cancel, None, "没必要", by.clone()).await.unwrap();
    let v = h.eng.get(&run).await.unwrap();
    assert_eq!(v.proposals.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["p1", "p2"]);
    assert_eq!((v.proposals[0].round, v.proposals[0].task.as_str()), (1, "c"));

    h.eng.dismiss_proposal(&run, "p2", me()).await.unwrap();
    assert!(h.eng.dismiss_proposal(&run, "p2", me()).await.is_err(), "decided already");
    let o = h.eng.rework(&run, "a", None, "照建议", me()).await.unwrap();
    assert!(o.events.iter().any(|e| e.kind == "proposal.accepted"));
    let rec = h.eng.record(&run).await.unwrap();
    assert_eq!((rec.proposals[0].status, rec.proposals[0].decided_by.clone()), (ProposalStatus::Accepted, Some(me())));
    assert_eq!(rec.proposals[1].status, ProposalStatus::Dismissed);
    assert!(h.eng.get(&run).await.unwrap().proposals.is_empty());

    // One the station passes by is not taken; one reworked elsewhere neither.
    let mut j = jobs(&o).remove(0);
    while j.station != "c" {
        j = jobs(&submit(&h, &j, json!({})).await).remove(0);
    }
    h.eng.propose(&run, "c", "c", &j.token, ProposalKind::Rework, Some("b"), "设计欠一块", by.clone()).await.unwrap();
    submit(&h, &j, json!({})).await;
    let rec = h.eng.record(&run).await.unwrap();
    assert_eq!(rec.status, RunStatus::Finished);
    assert_eq!(rec.proposals[2].status, ProposalStatus::Dismissed);
}

// ── the authoring tools: flow_start by a bundle, triage in the conversation, flow_switch ──────

struct NoCurrent;

#[async_trait::async_trait]
impl harnix_flow::author::AuthorHost for NoCurrent {
    async fn current(&self, _: &harnix_flow::runner::SessionId) -> Option<String> {
        None
    }
    async fn project(&self, _: &harnix_flow::runner::SessionId) -> Option<String> {
        None
    }
}

#[tokio::test]
async fn flow_start_routes_a_bundle_hands_triage_to_the_model_and_flow_switch_switches() {
    use harnix_flow::author::{AuthorTools, CONVERSATION_KIND, TOOL_START, TOOL_SWITCH};
    use harnix_flow::dryrun::{Sandbox, SandboxConfig};
    use harnix_flow::runner::{Runner, RunnerConfig, ScriptedHost};
    let d = tempfile::tempdir().unwrap();
    write_dev(d.path(), DEV);
    let catalog = Catalog { steps: ["work".to_string()].into(), ..Default::default() };
    let packs: Arc<dyn Packs> = Arc::new(DirPacks::new([d.path()]).with_catalog(catalog));
    let engine = Arc::new(Engine::new(Arc::new(MemStore::new()), packs.clone()));
    let rn = Runner::new(engine.clone(), ScriptedHost::new(), RunnerConfig { lease_ms: 60_000, ..Default::default() });
    let sandbox = Arc::new(Sandbox::new(packs, SandboxConfig { guarded: vec![], submit_scenes: vec![], main_scene: None }));
    let tools = AuthorTools::new(&rn, sandbox, Arc::new(NoCurrent));
    let by = Actor::new(CONVERSATION_KIND, "main/s1");
    let call = |tool: &'static str, input: Value| {
        let (tools, by) = (&tools, by.clone());
        async move {
            let r = tools.call(&"s1".to_string(), tool, input, &by).await;
            assert!(!r.is_error, "{tool}: {}", r.text);
            serde_json::from_str::<Value>(&r.text).unwrap()
        }
    };

    let v = call(TOOL_START, json!({"flow": "dev", "subject": "x"})).await;
    assert_eq!(v["started"], false);
    assert_eq!(v["triage"]["skill"], "dev/triage");
    assert!(v["triage"]["text"].as_str().unwrap().contains("按改动的范围判风险"), "{v}");
    assert_eq!(v["candidates"].as_array().unwrap().len(), 3);
    assert!(engine.list(Default::default()).await.unwrap().is_empty(), "nothing started");

    let v = call(TOOL_START, json!({"flow": "dev", "subject": "x", "inputs": {"risk": "low"}, "reason": "一行改动"})).await;
    assert_eq!((v["flow"].as_str(), v["routed"]["by"].as_str()), (Some("dev/quick"), Some("triage")));
    let run = v["run"].as_str().unwrap().to_string();

    let v = call(TOOL_SWITCH, json!({"run": run, "to": "solid", "reason": "要设计"})).await;
    assert_eq!(v["done"]["to"], "dev/solid");
    assert_eq!(engine.record(&run).await.unwrap().flow, "dev/solid");
}
