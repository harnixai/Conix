//! Confirmations that count votes (docs/design/flows.md §4, in Conix): `min`, `distinct`, `reject`,
//! `not: [author]`, and what the run view says of them. Binding files is in the runner's tests
//! (`runner::checks`), where files are read.

mod common;

use common::*;
use harnix_flow::*;
use serde_json::json;

fn person(id: &str) -> Actor {
    Actor::new("person", id)
}

fn flows() -> MemPacks {
    packs()
        .flow("name: two\nstations: [{id: a, exit: [{kind: confirm, min: 2}]}]")
        .flow("name: majority\nstations: [{id: a, exit: [{kind: confirm, min: 2, reject: majority}]}]")
        .flow("name: same\nstations: [{id: a, exit: [{kind: confirm, min: 2, distinct: false}]}]")
        .flow(
            "name: sod\nstations:\n  - {id: sign, executor: {kind: external, who: dev}, output: {type: object}, exit: [{kind: output}]}\n  - {id: ok, exit: [{kind: confirm, not: [author]}]}",
        )
}

async fn votes(h: &H, run: &str) -> (u32, u32, u32) {
    let v = h.eng.get(run).await.unwrap();
    let t = v.votes["a"][0];
    (t.approvals, t.rejections, t.min)
}

async fn status(h: &H, run: &str, station: &str) -> StationStatus {
    h.eng.get(run).await.unwrap().record.stations[station].status
}

#[tokio::test]
async fn one_rejection_stands_against_approvals_by_default() {
    let h = harness(flows());
    let (run, _) = start(&h, "two").await;
    assert_eq!(votes(&h, &run).await, (0, 0, 2));
    h.eng.confirm(&run, "a", true, None, person("ann")).await.unwrap();
    assert_eq!(status(&h, &run, "a").await, StationStatus::Working);
    let o = h.eng.confirm(&run, "a", false, Some("no tests"), person("bob")).await.unwrap();
    assert!(has(&o, "station.unmet:a"));
    let v = h.eng.get(&run).await.unwrap();
    assert_eq!(v.unmet["a"][0].detail, "rejected by person:bob: no tests");
    // a third approval does not outvote it; bob changing his mind does
    h.eng.confirm(&run, "a", true, None, person("cy")).await.unwrap();
    assert_eq!(status(&h, &run, "a").await, StationStatus::Unmet);
    assert_eq!(votes(&h, &run).await, (2, 1, 2));
    let o = h.eng.confirm(&run, "a", true, None, person("bob")).await.unwrap();
    assert!(has(&o, "run.finished"), "{:?}", kinds(&o));
}

#[tokio::test]
async fn under_majority_it_takes_as_many_rejections_as_approvals_needed() {
    let h = harness(flows());
    let (run, _) = start(&h, "majority").await;
    h.eng.confirm(&run, "a", false, Some("meh"), person("ann")).await.unwrap();
    assert_eq!(status(&h, &run, "a").await, StationStatus::Working);
    h.eng.confirm(&run, "a", false, Some("no"), person("bob")).await.unwrap();
    assert_eq!(status(&h, &run, "a").await, StationStatus::Unmet);
    let v = h.eng.get(&run).await.unwrap();
    assert_eq!(v.unmet["a"][0].detail, "2 of 2 rejections; rejected by person:bob: no");

    let (run, _) = start(&h, "majority").await;
    h.eng.confirm(&run, "a", false, None, person("ann")).await.unwrap();
    h.eng.confirm(&run, "a", true, None, person("bob")).await.unwrap();
    let v = h.eng.get(&run).await.unwrap();
    assert_eq!(v.unmet["a"][0].detail, "awaiting confirmation (1 of 2 approvals, 1 rejected)");
    let o = h.eng.confirm(&run, "a", true, None, person("cy")).await.unwrap();
    assert!(has(&o, "run.finished"));
}

#[tokio::test]
async fn without_distinct_one_person_may_vote_twice() {
    let h = harness(flows());
    let (run, _) = start(&h, "two").await;
    h.eng.confirm(&run, "a", true, None, person("ann")).await.unwrap();
    h.eng.confirm(&run, "a", true, None, person("ann")).await.unwrap();
    assert_eq!(votes(&h, &run).await, (1, 0, 2));
    let (run, _) = start(&h, "same").await;
    h.eng.confirm(&run, "a", true, None, person("ann")).await.unwrap();
    let o = h.eng.confirm(&run, "a", true, None, person("ann")).await.unwrap();
    assert!(has(&o, "run.finished"));
}

#[tokio::test]
async fn whoever_delivered_may_not_confirm_where_the_exit_says_not_author() {
    let h = harness(flows());
    let (run, _) = start(&h, "sod").await;
    let dev = person("dev");
    let c = h.eng.claim(&run, "sign", "sign", dev.clone(), 1000).await.unwrap();
    let token = c.value["token"].as_str().unwrap().to_string();
    h.eng.submit(&run, "sign", "sign", &token, json!({}), dev.clone()).await.unwrap();
    let e = h.eng.confirm(&run, "ok", true, None, dev).await.unwrap_err();
    let FlowError::Denied(why) = &e else { panic!("{e:?}") };
    assert!(why.contains("handed in sign/sign"), "{why}");
    // nor the one who started the run
    let e = h.eng.confirm(&run, "ok", true, None, me()).await.unwrap_err();
    assert!(e.to_string().contains("started the run"), "{e}");
    // someone else may
    let o = h.eng.confirm(&run, "ok", true, None, person("rev")).await.unwrap();
    assert!(has(&o, "run.finished"));
}

#[tokio::test]
async fn the_new_fields_are_validated() {
    let p = packs();
    let problems = |exit: &str| -> Vec<String> {
        validate_yaml(&format!("name: f\nstations: [{{id: a, exit: [{exit}]}}]"), &p).iter().map(ToString::to_string).collect()
    };
    assert_eq!(problems("{kind: confirm, min: 2, distinct: false, reject: majority, not: [author], binds: [deliverables, 'docs/*.md']}"), Vec::<String>::new());
    assert_eq!(problems("{kind: confirm, binds: deliverables}"), Vec::<String>::new());
    assert!(problems("{kind: confirm, min: 0}")[0].contains("at least 1"));
    assert!(problems("{kind: confirm, not: [owner]}")[0].contains("only author"));
    assert!(problems("{kind: confirm, binds: ['../secrets']}")[0].contains("no `..`"));
    assert!(problems("{kind: confirm, binds: ['${inputs.dir}/x']}")[0].contains("output_dir, data_dir and workspace"));
    // an unknown word is a parse error, not a silent default
    assert!(!problems("{kind: confirm, reject: all}").is_empty());
}
