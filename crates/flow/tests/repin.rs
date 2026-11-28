//! A run pinned afresh to its flow as the packs hold it now (`Engine::repin`): for developing a
//! bundle — a long run resumed from a snapshot goes on under the edited bundle (in Conix: the LIVE
//! tests' snapshots, `CONIX_DEV_REPIN`).

mod common;

use std::path::Path;
use std::sync::Arc;

use common::*;
use harnix_flow::*;
use serde_json::json;

const STATION: &str = "executor: {kind: program, step: work}\n    output: {type: object}\n    exit: [{kind: output}]";

fn write(dir: &Path, stations: &[&str], title: &str) {
    let mut t = format!("name: f\ntitle: {title}\nstations:\n");
    for s in stations {
        t.push_str(&format!("  - id: {s}\n    {STATION}\n"));
    }
    std::fs::create_dir_all(dir.join("dev")).unwrap();
    std::fs::write(dir.join("dev/bundle.yaml"), "name: dev\nflows: [f]\n").unwrap();
    std::fs::write(dir.join("dev/f.yaml"), t).unwrap();
}

#[tokio::test]
async fn a_run_goes_on_under_the_edited_flow_and_keeps_what_it_has_been_through() {
    let d = tempfile::tempdir().unwrap();
    write(d.path(), &["a", "b"], "one");
    let catalog = Catalog { steps: ["work".to_string()].into(), ..Default::default() };
    let packs = DirPacks::new([d.path()]).with_catalog(catalog);
    let eng = Engine::new(Arc::new(MemStore::new()), Arc::new(packs)).with_clock(Arc::new(ManualClock::new(T0)));
    let o = eng.create(CreateIn { flow: "dev/f".into(), subject: "s".into(), ..Default::default() }, me()).await.unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    let j = jobs(&o).remove(0);
    eng.submit(&run, "a", "a", &j.token, json!({"n": 1}), Actor::flow("dispatch")).await.unwrap();
    let before = eng.record(&run).await.unwrap();
    assert_eq!(before.stations["a"].status, StationStatus::Passed);

    // Nothing edited: nothing to do.
    let o = eng.repin(&run, me()).await.unwrap();
    assert_eq!(o.value["unchanged"], true);

    // b edited, c added: the run is on the new definition, a's record as it was.
    write(d.path(), &["a", "b", "c"], "two");
    let o = eng.repin(&run, me()).await.unwrap();
    assert_eq!(o.value["from"], json!(before.def_digest));
    assert!(o.events.iter().any(|e| e.kind == "run.repinned"));
    let rec = eng.record(&run).await.unwrap();
    assert_ne!(rec.def_digest, before.def_digest);
    assert_eq!((rec.def.title.as_str(), rec.def.stations.len()), ("two", 3));
    assert_eq!(rec.stations["a"], before.stations["a"]);
    assert_eq!(rec.stations["c"].status, StationStatus::Pending);
    assert_eq!(rec.inputs, before.inputs);

    // A station the run has been through is gone from the flow: refused, nothing changed.
    write(d.path(), &["b", "c"], "three");
    let e = eng.repin(&run, me()).await.unwrap_err().to_string();
    assert!(e.contains("has been through a that dev/f no longer has"), "{e}");
    assert_eq!(eng.record(&run).await.unwrap().def_digest, rec.def_digest);
}
