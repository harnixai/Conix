//! An archive checked after the fact (`harnix_flow::archive::verify`): a finished run's archive is
//! untouched → ok; a file edited, deleted or added, `record.json` rewritten, an archive from before
//! the listing existed. The ledger holds the digest of `record.json`, which the model cannot write.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::archive::{ledger_record, verify, RecordState};
use harnix_flow::runner::{CatalogPacks, Runner, RunnerConfig, ScriptedHost, Turn, TOOL_SUBMIT};
use harnix_flow::*;
use serde_json::{json, Value};

const T0: u64 = 1_794_787_200_000;

/// A bundle `dev` whose flow `work` hands in `report.md` and has it approved.
fn write_bundle(dir: &Path) {
    let files: &[(&str, &str)] = &[
        ("bundle.yaml", "name: dev\ntitle: 开发\n"),
        (
            "work.yaml",
            "name: work\nstations:\n  - id: write\n    executor: {kind: session}\n    skill: write\n    output: {type: object}\n    deliverables:\n      - {path: report.md, description: 报告}\n    exit: [{kind: output}]\n",
        ),
        ("skills/write/SKILL.md", "---\nname: write\n---\n写报告。\n"),
    ];
    for (p, t) in files {
        let f = dir.join("dev").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
}

async fn eventually<T>(what: &str, mut f: impl AsyncFnMut() -> Option<T>) -> T {
    let r = tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(v) = f().await {
                return v;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    r.unwrap_or_else(|_| panic!("{what} did not happen"))
}

struct Done {
    engine: Arc<Engine>,
    root: PathBuf,
    archive: PathBuf,
    run: String,
    _keep: Vec<tempfile::TempDir>,
}

/// Run `dev/work` to its end and return where it was archived.
async fn archived_run() -> Done {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path());
    let host = ScriptedHost::new();
    host.main(vec![Turn::Runs]);
    let cfg = RunnerConfig {
        profiles: vec!["flow-harnix".into()],
        submit_profiles: ["flow-harnix".to_string()].into(),
        main_profile: Some("flow-harnix".into()),
        lease_ms: 60_000,
        tick_ms: 1000,
        ..Default::default()
    };
    let packs = CatalogPacks::over(&cfg, vec![], Arc::new(DirPacks::new([packs_dir.path()])));
    let layout = ProjectLayout::new(".conix", data.path().join("worktrees"), "conix/flow").with_artifacts_dir("conix");
    packs.set_layout(layout.clone());
    let engine = Arc::new(
        Engine::new(Arc::new(MemStore::new()), Arc::new(packs))
            .with_clock(Arc::new(ManualClock::new(T0)))
            .with_lease_ms(60_000)
            .with_submit_scenes(cfg.profiles.clone())
            .with_main_scene("flow-harnix")
            .with_workspaces(data.path().join("workspaces"))
            .with_layout(layout),
    );
    let rn = Runner::new(engine.clone(), host.clone(), cfg);
    rn.start().await;

    let root = project.path().canonicalize().unwrap();
    let o = rn
        .start_run(CreateIn { flow: "dev/work".into(), subject: "s".into(), project_root: Some(root.to_string_lossy().into()), ..Default::default() }, Actor::device("d"))
        .await
        .unwrap();
    let run = o.value["run"].as_str().unwrap().to_string();
    rn.apply(o).await;
    let main = eventually("the main session opened", async || host.opened().into_iter().find(|s| s.main)).await;
    let sid = eventually("the session got its key", async || host.session_of(&main.key)).await;
    eventually("the task was sent", async || host.sent_to(&sid).into_iter().next()).await;

    let run_dir = root.join("conix/runs").join(run.replace('.', "-"));
    std::fs::write(run_dir.join("report.md"), "# 报告\n\n结论：可以发布。\n").unwrap();
    std::fs::create_dir_all(run_dir.join("data")).unwrap();
    let r = rn.tools().call(&sid, TOOL_SUBMIT, json!({"output": {}})).await;
    assert!(!r.is_error, "{}", r.text);
    let archive = eventually("the run was archived", async || {
        let base = root.join("conix/dev/archive");
        let a = std::fs::read_dir(&base).ok()?.flatten().map(|e| e.path()).find(|p| p.join("record.json").is_file())?;
        // The ledger is told after the archive is made.
        engine.record(&run).await.ok()?.archive.is_some().then_some(a)
    })
    .await;
    Done { engine, root, archive, run, _keep: vec![packs_dir, data, project] }
}

async fn report(d: &Done) -> harnix_flow::archive::ArchiveReport {
    let ledger = ledger_record(&d.engine, &d.archive).await;
    assert_eq!(ledger.as_ref().map(|l| l.id.as_str()), Some(d.run.as_str()));
    verify(&d.archive, ledger.as_ref())
}

fn record_json(d: &Done) -> Value {
    serde_json::from_str(&std::fs::read_to_string(d.archive.join("record.json")).unwrap()).unwrap()
}

#[tokio::test]
async fn an_untouched_archive_is_ok_and_the_ledger_knows_it() {
    let d = archived_run().await;
    let rec = d.engine.record(&d.run).await.unwrap();
    let a = rec.archive.clone().unwrap();
    assert_eq!(Path::new(&a.dir), d.archive.strip_prefix(&d.root).unwrap(), "relative to the project");
    assert_eq!(a.record_sha256.len(), 64);

    let record = record_json(&d);
    assert_eq!(record["format"], 1);
    let files = record["files"].as_array().unwrap();
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!((files[0]["path"].as_str(), files[0]["size"].as_u64()), (Some("report.md"), Some("# 报告\n\n结论：可以发布。\n".len() as u64)));
    assert_eq!(files[0]["sha256"].as_str().unwrap().len(), 64);

    let r = report(&d).await;
    assert!(r.ok && !r.unlisted && r.record == RecordState::Matches, "{r:?}");
    assert_eq!(r.checked, 1);
    // Without the ledger the record is only unconfirmed.
    let r = verify(&d.archive, None);
    assert!(r.ok && r.record == RecordState::NoLedger, "{r:?}");
}

#[tokio::test]
async fn an_edited_deleted_or_added_file_is_reported() {
    let d = archived_run().await;
    let report_md = d.archive.join("report.md");
    let original = std::fs::read(&report_md).unwrap();

    std::fs::write(&report_md, "# 报告\n\n结论：不可以发布。\n").unwrap();
    let r = report(&d).await;
    assert!(!r.ok && r.changed == ["report.md"] && r.missing.is_empty() && r.extra.is_empty(), "{r:?}");
    // The record itself is untouched, so the evidence the gates read no longer matches either.
    assert!(r.evidence.iter().all(|e| e.path == "report.md" && e.problem == "changed"), "{r:?}");
    assert_eq!(r.record, RecordState::Matches);

    std::fs::write(&report_md, &original).unwrap();
    assert!(report(&d).await.ok);

    std::fs::remove_file(&report_md).unwrap();
    let r = report(&d).await;
    assert!(!r.ok && r.missing == ["report.md"] && r.changed.is_empty(), "{r:?}");
    std::fs::write(&report_md, &original).unwrap();

    std::fs::create_dir_all(d.archive.join("extra")).unwrap();
    std::fs::write(d.archive.join("extra/more.md"), "x").unwrap();
    let r = report(&d).await;
    assert!(!r.ok && r.extra == ["extra/more.md"], "{r:?}");
}

#[tokio::test]
async fn a_rewritten_record_differs_from_the_ledger() {
    let d = archived_run().await;
    let mut record = record_json(&d);
    record["title"] = json!("另一个标题");
    std::fs::write(d.archive.join("record.json"), serde_json::to_string_pretty(&record).unwrap()).unwrap();
    let r = report(&d).await;
    assert!(!r.ok && r.record == RecordState::Differs && r.changed.is_empty(), "{r:?}");
}

#[tokio::test]
async fn an_archive_from_before_the_listing_is_unlisted_but_its_digests_are_still_checked() {
    let d = archived_run().await;
    let mut record = record_json(&d);
    let object = record.as_object_mut().unwrap();
    object.remove("files");
    object.remove("format");
    // As the first archives were: evidence in a station's check, no listing, no ledger entry.
    let (sha, _) = harnix_flow::archive::hash_file(&d.archive.join("report.md")).unwrap();
    record["stations"] = json!([{"id": "write", "checks": [{"label": "deliverables", "evidence": [{"path": "report.md", "sha256": sha, "size": 1}]}]}]);
    std::fs::write(d.archive.join("record.json"), serde_json::to_string_pretty(&record).unwrap()).unwrap();
    let r = verify(&d.archive, None);
    assert!(r.ok && r.unlisted && r.record == RecordState::NoLedger && r.extra.is_empty(), "{r:?}");

    std::fs::write(d.archive.join("report.md"), "changed").unwrap();
    let r = verify(&d.archive, None);
    assert!(!r.ok && r.unlisted && r.evidence.len() == 1 && r.evidence[0].by == "check write/deliverables", "{r:?}");
}
