//! A bundle's settings (in Conix; docs/design/flows.md §7.1, §4.1, §7.7): `bundle.yaml`'s
//! `settings: {file, defaults}` read from the project when a run starts, merged over the defaults
//! and pinned with the run; `${settings.*}` in a confirm exit's `min`, `reject`, `not` and
//! `separate`, checked right after they are pinned; `flow.settings` in a run's scripts and in a
//! command outside a run.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use harnix_flow::runner::{CatalogPacks, CommandBy, CommandCall, Runner, RunnerConfig, ScriptedHost, Turn};
use harnix_flow::*;
use serde_json::{json, Value};

const BUNDLE: &str = "name: gov
settings:
  file: .conix/governance/gov.yaml
  defaults:
    governance: {spec: true, interface: false}
    review: {separate: false, min: 1, not: []}
commands: [show]
";

/// Someone delivers, a script looks at the settings, someone confirms by the rules they say.
const REVIEW: &str = "name: review
stations:
  - {id: sign, executor: {kind: external, who: dev}, output: {type: object}, exit: [{kind: output}]}
  - {id: look, exit: [{kind: gate, gate: sees}]}
  - id: ok
    exit:
      - {kind: confirm, separate: '${settings.review.separate}', min: '${settings.review.min}', not: ['${settings.review.not}']}
";

fn write_bundle(dir: &Path, flows: &[(&str, &str)]) {
    let mut files: Vec<(&str, &str)> = vec![
        ("bundle.yaml", BUNDLE),
        ("review.yaml", REVIEW),
        ("gates/sees.yaml", "name: sees\nkind: script\nscript: sees\n"),
        (
            "scripts/sees.js",
            "export default function (input, flow) {\n  const s = flow.settings;\n  return { verdict: \"pass\", summary: JSON.stringify(s) };\n}\n",
        ),
        ("commands/show.js", "export default function (input, flow) { return flow.settings; }\n"),
    ];
    files.extend_from_slice(flows);
    for (p, t) in files {
        let f = dir.join("gov").join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, t).unwrap();
    }
}

struct Rig {
    rn: Arc<Runner>,
    engine: Arc<Engine>,
    root: PathBuf,
    _dirs: Vec<tempfile::TempDir>,
}

async fn rig(flows: &[(&str, &str)]) -> Rig {
    let (packs_dir, data, project) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    write_bundle(packs_dir.path(), flows);
    let root = project.path().canonicalize().unwrap();
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
            .with_lease_ms(60_000)
            .with_submit_scenes(cfg.profiles.clone())
            .with_main_scene("flow-harnix")
            .with_workspaces(data.path().join("workspaces"))
            .with_layout(layout),
    );
    let rn = Runner::new(engine.clone(), host, cfg);
    rn.start().await;
    Rig { rn, engine, root, _dirs: vec![packs_dir, data, project] }
}

impl Rig {
    fn governance(&self, yaml: &str) {
        let f = self.root.join(".conix/governance/gov.yaml");
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, yaml).unwrap();
    }

    async fn start_in(&self, flow: &str, project: bool) -> Result<String> {
        let input = CreateIn {
            flow: flow.into(),
            subject: "s".into(),
            project_root: project.then(|| self.root.to_string_lossy().into_owned()),
            ..Default::default()
        };
        let o = self.rn.start_run(input, Actor::device("d")).await?;
        let run = o.value["run"].as_str().unwrap().to_string();
        self.rn.apply(o).await;
        Ok(run)
    }

    async fn start(&self, flow: &str) -> Result<String> {
        self.start_in(flow, true).await
    }

    async fn pinned(&self, run: &str) -> Value {
        self.engine.record(run).await.unwrap().parts.settings.unwrap()
    }

    async fn show(&self, run: Option<&str>) -> std::result::Result<Value, String> {
        let mut c = CommandCall::new("gov", "show", vec!["go".into()], CommandBy::person("me"));
        c.run = run.map(String::from);
        c.project_root = Some(self.root.clone());
        let r = self.rn.run_command(c).await?;
        Ok(serde_json::from_str(&r.text).unwrap_or_else(|e| panic!("{e}: {}", r.text)))
    }

    /// `dev` hands in the first station: an author of the run from then on.
    async fn deliver(&self, run: &str) {
        let dev = Actor::new("person", "dev");
        let c = self.engine.claim(run, "sign", "sign", dev.clone(), 1000).await.unwrap();
        let token = c.value["token"].as_str().unwrap().to_string();
        let o = self.engine.submit(run, "sign", "sign", &token, json!({}), dev).await.unwrap();
        self.rn.apply(o).await;
    }

    async fn at(&self, run: &str, station: &str) -> StationStatus {
        self.engine.get(run).await.unwrap().record.stations[station].status
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

#[tokio::test]
async fn the_file_is_merged_over_the_defaults_and_pinned_and_an_edit_mid_run_changes_nothing() {
    let r = rig(&[]).await;
    // No file: the defaults.
    let run = r.start("gov/review").await.unwrap();
    assert_eq!(r.pinned(&run).await, json!({"governance": {"spec": true, "interface": false}, "review": {"separate": false, "min": 1, "not": []}}));

    // Mappings merge key by key; anything else replaces.
    r.governance("# the project's rules\nreview:\n  separate: true\n  not: [author]\nextra: {k: 1}\n");
    let run = r.start("gov/review").await.unwrap();
    let want = json!({"governance": {"spec": true, "interface": false}, "review": {"separate": true, "min": 1, "not": ["author"]}, "extra": {"k": 1}});
    assert_eq!(r.pinned(&run).await, want);

    // Edited mid-run: the run keeps what it started under — its record, its commands, its gates.
    r.governance("review:\n  separate: false\n  min: 3\n");
    assert_eq!(r.pinned(&run).await, want);
    assert_eq!(r.show(Some(&run)).await.unwrap(), want, "a command in the run reads what it pinned");
    r.deliver(&run).await;
    let c = eventually("the gate judged", async || {
        let rec = r.engine.record(&run).await.unwrap();
        let c = rec.stations["look"].checks.iter().find(|c| c.kind == "gate")?.clone();
        (c.status != CheckStatus::Wait).then_some(c)
    })
    .await;
    assert_eq!(c.status, CheckStatus::Pass, "{c:?}");
    assert_eq!(serde_json::from_str::<Value>(&c.detail).unwrap(), want, "the gate's script reads what the run pinned");

    // Outside a run, a command reads the file as it is now, over the defaults.
    let live = r.show(None).await.unwrap();
    assert_eq!(live["review"], json!({"separate": false, "min": 3, "not": []}));
    assert_eq!(live["governance"]["spec"], true);
    let v = r.engine.get(&run).await.unwrap();
    assert_eq!(v.record.parts.settings.as_ref().unwrap()["review"]["separate"], true, "the run's view shows what it pinned");
}

#[tokio::test]
async fn a_file_that_does_not_read_refuses_the_start_and_a_run_of_no_project_has_the_defaults() {
    let r = rig(&[]).await;
    r.governance("review: [separate: true\n");
    let e = r.start("gov/review").await.unwrap_err().to_string();
    assert!(e.contains("the settings of bundle gov") && e.contains(".conix/governance/gov.yaml"), "{e}");
    assert!(r.engine.list(RunQuery::default()).await.unwrap().is_empty(), "nothing was started");
    r.governance("- a list\n");
    let e = r.start("gov/review").await.unwrap_err().to_string();
    assert!(e.contains("settings are a mapping"), "{e}");
    // Outside a run, the command that asks says so too.
    let e = r.show(None).await.unwrap_err();
    assert!(e.contains("flow.settings") && e.contains("settings are a mapping"), "{e}");

    let run = r.start_in("gov/review", false).await.unwrap();
    assert_eq!(r.pinned(&run).await["review"]["separate"], false, "no project, no file: the defaults");
}

#[tokio::test]
async fn separate_keeps_the_author_out_only_when_the_settings_turn_it_on() {
    let r = rig(&[]).await;
    // Off (the defaults): whoever delivered may confirm.
    let run = r.start("gov/review").await.unwrap();
    r.deliver(&run).await;
    eventually("ok entered", async || (r.at(&run, "ok").await == StationStatus::Working).then_some(())).await;
    let o = r.engine.confirm(&run, "ok", true, None, Actor::new("person", "dev")).await.unwrap();
    r.rn.apply(o).await;
    assert_eq!(r.at(&run, "ok").await, StationStatus::Passed);

    // On: the author is refused, someone else decides.
    r.governance("review: {separate: true}\n");
    let run = r.start("gov/review").await.unwrap();
    r.deliver(&run).await;
    eventually("ok entered", async || (r.at(&run, "ok").await == StationStatus::Working).then_some(())).await;
    let e = r.engine.confirm(&run, "ok", true, None, Actor::new("person", "dev")).await.unwrap_err();
    let FlowError::Denied(why) = &e else { panic!("{e:?}") };
    assert!(why.contains("handed in sign/sign") && why.contains("someone else decides"), "{why}");
    let o = r.engine.confirm(&run, "ok", true, None, Actor::new("person", "rev")).await.unwrap();
    r.rn.apply(o).await;
    assert_eq!(r.at(&run, "ok").await, StationStatus::Passed);

    // `not` and `min` from the settings: two approvals, the author still out.
    r.governance("review: {not: [author], min: 2}\n");
    let run = r.start("gov/review").await.unwrap();
    r.deliver(&run).await;
    eventually("ok entered", async || (r.at(&run, "ok").await == StationStatus::Working).then_some(())).await;
    assert!(r.engine.confirm(&run, "ok", true, None, Actor::new("person", "dev")).await.is_err());
    r.engine.confirm(&run, "ok", true, None, Actor::new("person", "ann")).await.unwrap();
    assert_eq!(r.engine.get(&run).await.unwrap().votes["ok"][0].min, 2);
    assert_eq!(r.at(&run, "ok").await, StationStatus::Working);
    let o = r.engine.confirm(&run, "ok", true, None, Actor::new("person", "bob")).await.unwrap();
    r.rn.apply(o).await;
    assert_eq!(r.at(&run, "ok").await, StationStatus::Passed);
}

#[tokio::test]
async fn a_reference_that_does_not_read_is_reported_at_the_start() {
    let bad_path = "name: typo\nstations:\n  - {id: ok, exit: [{kind: confirm, separate: '${settings.review.seperate}'}]}\n";
    let r = rig(&[("typo.yaml", bad_path)]).await;
    let e = r.start("gov/typo").await.unwrap_err().to_string();
    assert!(e.contains("station ok: confirm separate: ${settings.review.seperate}: the settings have nothing at review.seperate"), "{e}");

    // The wrong kind of value: a governance file's `yes` is not a switch.
    r.governance("review: {separate: 'yes', min: 0, not: owner}\n");
    let e = r.start("gov/review").await.unwrap_err().to_string();
    assert!(e.contains("confirm separate: ${settings.review.separate} is \"yes\", not true or false"), "{e}");
    assert!(e.contains("confirm min: ${settings.review.min} is 0, not a number of approvals"), "{e}");
    assert!(e.contains("confirm not: ${settings.review.not} is \"owner\" — only author"), "{e}");
    assert!(r.engine.list(RunQuery::default()).await.unwrap().is_empty());
}

#[test]
fn the_form_is_checked_when_the_flow_is_read() {
    let p = MemPacks::new();
    let problems = |exit: &str| -> Vec<String> {
        validate_yaml(&format!("name: f\nstations: [{{id: a, exit: [{exit}]}}]"), &p).iter().map(ToString::to_string).collect()
    };
    // A flow of no bundle has no settings to read.
    let e = problems("{kind: confirm, separate: '${settings.review.separate}'}");
    assert!(e.iter().any(|m| m.contains("declares none")), "{e:?}");
    // Not one reference: said, whatever the bundle.
    for (exit, field) in [("{kind: confirm, min: 'two'}", "min"), ("{kind: confirm, separate: 'on'}", "separate"), ("{kind: confirm, reject: '${inputs.r}'}", "reject")] {
        let e = problems(exit);
        assert!(e.iter().any(|m| m.contains(&format!("confirm {field}:")) && m.contains("${settings.<path>}")), "{exit}: {e:?}");
    }
    assert!(problems("{kind: confirm, not: ['${inputs.who}']}").iter().any(|m| m.contains("only author")));
    assert!(problems("{kind: confirm, min: 2, separate: true, not: [author]}").is_empty());

    // The settings file is the project's: relative, inside it.
    for (file, why) in [("/etc/gov.yaml", "absolute"), ("../gov.yaml", "no `..`"), ("", "relative to the project")] {
        let meta = bundle::BundleMeta::parse(&format!("name: g\nsettings: {{file: '{file}'}}\n")).unwrap();
        let e = meta.start_problems(&Default::default(), &|_| true);
        assert!(e.iter().any(|m| m.contains("settings: file") && m.contains(why)), "{file}: {e:?}");
    }
    let meta = bundle::BundleMeta::parse("name: g\nsettings: {file: a.yaml, defaults: [1]}\n").unwrap();
    assert!(meta.start_problems(&Default::default(), &|_| true).iter().any(|m| m.contains("defaults is a mapping")));
}
