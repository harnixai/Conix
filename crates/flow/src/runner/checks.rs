//! `check` gates: declared checks the library runs itself (docs/design/flows.md §2.1, in Conix).
//!
//! What a file says is the run's to fix (`fail`, with a finding each); a path out of the
//! workspace or a file that cannot be read at all is not (`error`).

use std::collections::BTreeMap;

use serde_json::Value;

use super::files::{parse, OutOfBounds, Reader};
use crate::kernel::host::GateResult;
use crate::kernel::parts::{CheckKind, GateDef, ReadAs};
use crate::kernel::refs::{resolve, resolve_str};
use crate::kernel::run::{Evidence, Finding, RunRecord};

/// How many findings one check reports at most; the rest are counted in the summary.
const MAX_FINDINGS: usize = 200;

/// Run a `check` gate against the run's workspace. `ctx` is the run's reference context.
pub(crate) fn run(g: &GateDef, ctx: &Value, rec: &RunRecord, reader: &Reader) -> GateResult {
    let Some(kind) = g.check else {
        return GateResult::errored(format!("gate {} says no check", g.name));
    };
    let mut found = Findings::default();
    let r = match kind {
        CheckKind::Files => files(g, ctx, reader, &mut found),
        CheckKind::Schema => schema(g, ctx, reader, &mut found),
        CheckKind::Text => text(g, ctx, reader, &mut found),
        CheckKind::Output => output(g, ctx, &mut found).map(|()| 1),
        CheckKind::Sealed => sealed(rec, reader, &mut found),
    };
    let checked = match r {
        Ok(n) => n,
        Err(e) => {
            let mut out = GateResult::errored(e.0);
            out.evidence = reader.evidence();
            return out;
        }
    };
    found.into_result(checked, reader.evidence())
}

#[derive(Default)]
struct Findings {
    list: Vec<Finding>,
    more: usize,
}

impl Findings {
    fn push(&mut self, f: Finding) {
        if self.list.len() < MAX_FINDINGS {
            self.list.push(f);
        } else {
            self.more += 1;
        }
    }

    fn into_result(self, checked: usize, evidence: Vec<Evidence>) -> GateResult {
        let total = self.list.len() + self.more;
        let what = if checked == 1 { "1 file".to_string() } else { format!("{checked} files") };
        let mut out = if total == 0 {
            GateResult::passed(format!("{what} checked"))
        } else {
            GateResult::failed(format!("{total} problem{} in {what}", if total == 1 { "" } else { "s" }))
        };
        out.findings = self.list;
        out.evidence = evidence;
        out
    }
}

/// A path or pattern with its references resolved; `Err` when it resolves to nothing.
fn pattern(raw: &str, ctx: &Value) -> Result<String, OutOfBounds> {
    match resolve_str(raw, ctx) {
        Value::String(s) if !s.trim().is_empty() => Ok(s),
        Value::Null => Err(OutOfBounds(format!("{raw} resolves to nothing"))),
        Value::String(_) => Err(OutOfBounds(format!("{raw} resolves to an empty path"))),
        other => Err(OutOfBounds(format!("{raw} resolves to {other}, not a path"))),
    }
}

/// The files `raw` names; a finding when it names none.
fn matched(raw: &str, ctx: &Value, reader: &Reader, found: &mut Findings) -> Result<Vec<String>, OutOfBounds> {
    let p = pattern(raw, ctx)?;
    let files = reader.glob(&p)?;
    if files.is_empty() {
        let msg = if p.contains(['*', '?']) { "no file matches".to_string() } else { "does not exist".to_string() };
        found.push(Finding::new("exists", msg).at(p));
    }
    Ok(files)
}

fn files(g: &GateDef, ctx: &Value, reader: &Reader, found: &mut Findings) -> Result<usize, OutOfBounds> {
    let mut n = 0;
    for raw in &g.paths {
        for f in matched(raw, ctx, reader, found)? {
            n += 1;
            let Some((rel, text)) = reader.text(&f)? else { continue };
            if g.non_empty != Some(false) && text.trim().is_empty() {
                found.push(Finding::new("non_empty", "is empty").at(&rel));
            }
            for s in &g.not_containing {
                if let Some(line) = line_of(&text, s) {
                    found.push(Finding::new("not_containing", format!("still contains {s:?}")).at(&rel).line(line));
                }
            }
        }
    }
    Ok(n)
}

fn schema(g: &GateDef, ctx: &Value, reader: &Reader, found: &mut Findings) -> Result<usize, OutOfBounds> {
    let raw = g.file.as_deref().unwrap_or_default();
    let validator = compile(g.schema.as_ref())?;
    let mut n = 0;
    for f in matched(raw, ctx, reader, found)? {
        n += 1;
        let Some((rel, text)) = reader.text(&f)? else { continue };
        let read_as = g.read_as.unwrap_or_else(|| ReadAs::of(&rel));
        match parse(&text, read_as) {
            Err(e) => found.push(Finding::new("parse", e).at(&rel)),
            Ok(v) => {
                for e in validator.iter_errors(&v) {
                    found.push(Finding::new("schema", schema_message(&e)).at(&rel));
                }
            }
        }
    }
    Ok(n)
}

fn text(g: &GateDef, ctx: &Value, reader: &Reader, found: &mut Findings) -> Result<usize, OutOfBounds> {
    let raw = g.file.as_deref().unwrap_or_default();
    let compile = |list: &[String]| -> Result<Vec<regex::Regex>, OutOfBounds> {
        list.iter()
            .map(|r| regex::Regex::new(r).map_err(|e| OutOfBounds(format!("{r:?} is not a regular expression: {e}"))))
            .collect()
    };
    let (must, must_not) = (compile(&g.matches)?, compile(&g.not_matches)?);
    let mut n = 0;
    for f in matched(raw, ctx, reader, found)? {
        n += 1;
        let Some((rel, text)) = reader.text(&f)? else { continue };
        for re in &must {
            if !re.is_match(&text) {
                found.push(Finding::new("matches", format!("does not match {:?}", re.as_str())).at(&rel));
            }
        }
        for re in &must_not {
            for m in re.find_iter(&text) {
                let line = text[..m.start()].matches('\n').count() as u64 + 1;
                found.push(Finding::new("not_matches", format!("matches {:?}: {:?}", re.as_str(), clip(m.as_str()))).at(&rel).line(line));
            }
        }
    }
    Ok(n)
}

fn output(g: &GateDef, ctx: &Value, found: &mut Findings) -> Result<(), OutOfBounds> {
    let validator = compile(g.schema.as_ref())?;
    let v = if g.value.is_null() { ctx.clone() } else { resolve(&g.value, ctx) };
    for e in validator.iter_errors(&v) {
        found.push(Finding::new("schema", schema_message(&e)));
    }
    Ok(())
}

/// Every file the run's gates recorded is still what it was: the latest record of each path,
/// over the stations' current checks, hashed again.
fn sealed(rec: &RunRecord, reader: &Reader, found: &mut Findings) -> Result<usize, OutOfBounds> {
    let mut latest: BTreeMap<String, (u64, Evidence, String)> = BTreeMap::new();
    for (sid, st) in &rec.stations {
        for c in &st.checks {
            for e in &c.evidence {
                let newer = latest.get(&e.path).is_none_or(|(at, ..)| c.at >= *at);
                if newer {
                    latest.insert(e.path.clone(), (c.at, e.clone(), format!("{} at {sid}", c.label)));
                }
            }
        }
    }
    for (path, (_, was, by)) in &latest {
        match reader.stat(path)? {
            None => found.push(Finding::new("sealed", format!("is gone since {by} read it")).at(path)),
            Some(now) if now.sha256 != was.sha256 => {
                found.push(Finding::new("sealed", format!("changed since {by} read it")).at(path))
            }
            Some(_) => {}
        }
    }
    Ok(latest.len())
}

/// A confirmation's reserved work item (`kernel::confirm`): record what exit `n` of the station
/// approved — its files' digests, as evidence — or check that what earlier stations approved is
/// unchanged. Paths resolve and stay in the workspace as a `files` check's do.
pub(crate) fn implicit(engine: &crate::Engine, rec: &RunRecord, station: &str, gate: &str) -> GateResult {
    use crate::kernel::confirm::{approvals, bind_patterns, implicit, Implicit, STALE_KIND};
    let Some(ws) = engine.workspace_for(rec) else {
        return GateResult::errored("the run has no workspace to read what was approved in");
    };
    let reader = match Reader::new(&ws) {
        Ok(r) => r,
        Err(e) => return GateResult::errored(e.0),
    };
    let ctx = crate::kernel::refs::context_in(rec, None, None, ws.to_str());
    match implicit(gate) {
        Some(Implicit::Binds(i)) => {
            let Some(e) = rec.def.station(station).and_then(|s| s.exit.get(i)) else {
                return GateResult::errored(format!("station {station} has no exit {i}"));
            };
            let mut n = 0;
            for raw in bind_patterns(rec, station, e) {
                let files = match pattern(&raw, &ctx).and_then(|p| reader.glob(&p)) {
                    Ok(f) => f,
                    Err(e) => return GateResult::errored(e.0),
                };
                for f in files {
                    if let Err(e) = reader.stat(&f) {
                        return GateResult::errored(e.0);
                    }
                    n += 1;
                }
            }
            let mut out = GateResult::passed(format!("{n} file{} approved", if n == 1 { "" } else { "s" }));
            out.evidence = reader.evidence();
            out
        }
        Some(Implicit::Stale) => {
            let mut found = Findings::default();
            let list = approvals(rec, station);
            for a in &list {
                let why = match reader.stat(&a.path) {
                    Ok(None) => "is gone",
                    Ok(Some(now)) if now.sha256 != a.sha256 => "changed",
                    Ok(Some(_)) => continue,
                    Err(e) => return GateResult::errored(e.0),
                };
                let msg = format!("{why} since station {} approved it; rework to {} to have it approved again", a.station, a.station);
                found.push(Finding::new(STALE_KIND, msg).at(&a.path));
            }
            found.into_result(list.len(), vec![])
        }
        None => GateResult::errored(format!("{gate} is not one of the library's own checks")),
    }
}

fn compile(schema: Option<&Value>) -> Result<jsonschema::Validator, OutOfBounds> {
    let s = schema.ok_or_else(|| OutOfBounds("the check has no schema".into()))?;
    jsonschema::validator_for(s).map_err(|e| OutOfBounds(format!("the schema does not compile: {e}")))
}

fn schema_message(e: &jsonschema::ValidationError) -> String {
    let at = e.instance_path().to_string();
    if at.is_empty() {
        e.to_string()
    } else {
        format!("{at}: {e}")
    }
}

fn line_of(text: &str, needle: &str) -> Option<u64> {
    text.find(needle).map(|i| text[..i].matches('\n').count() as u64 + 1)
}

fn clip(s: &str) -> String {
    let t: String = s.chars().take(80).collect();
    if t.len() < s.len() {
        format!("{t}…")
    } else {
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::host::GateResult;
    use crate::kernel::run::GateVerdict;
    use serde_json::json;

    fn gate(yaml: &str) -> GateDef {
        serde_yaml::from_str(yaml).unwrap()
    }

    fn rec() -> RunRecord {
        let def = crate::FlowDef::parse("{name: f, stations: [{id: a, exit: [{kind: confirm}]}]}").unwrap();
        crate::kernel::txn::new_record("r1".into(), def, Default::default(), "d".into(), "t".into(), "s".into(), json!({}), crate::Actor::device("d"), 0, None)
    }

    fn judge(yaml: &str, files: &[(&str, &str)]) -> GateResult {
        let d = tempfile::tempdir().unwrap();
        for (p, t) in files {
            let full = d.path().join(p);
            std::fs::create_dir_all(full.parent().unwrap()).unwrap();
            std::fs::write(full, t).unwrap();
        }
        let reader = Reader::new(d.path()).unwrap();
        let ctx = json!({"output_dir": "out", "stations": {"a": {"output": {"coverage": 0.7}}}});
        run(&gate(yaml), &ctx, &rec(), &reader)
    }

    #[test]
    fn files_must_exist_be_written_and_not_be_the_template() {
        let g = "{name: g, kind: check, check: files, paths: ['${output_dir}/design.md', 'out/adr/*.md'], not_containing: ['TEMPLATE']}";
        let r = judge(g, &[("out/design.md", "# Design\nreal\n"), ("out/adr/1.md", "x")]);
        assert_eq!(r.verdict, GateVerdict::Pass, "{r:?}");
        assert_eq!(r.evidence.len(), 2);
        let r = judge(g, &[("out/design.md", "  \n"), ("out/adr/1.md", "TEMPLATE")]);
        assert_eq!(r.verdict, GateVerdict::Fail);
        let msgs: Vec<String> = r.findings.iter().map(|f| f.line_text()).collect();
        assert_eq!(msgs, ["out/design.md: is empty", "out/adr/1.md:1: still contains \"TEMPLATE\""]);
        let r = judge(g, &[]);
        let msgs: Vec<String> = r.findings.iter().map(|f| f.line_text()).collect();
        assert_eq!(msgs, ["out/design.md: does not exist", "out/adr/*.md: no file matches"]);
    }

    #[test]
    fn schema_reads_jsonl_yaml_and_front_matter() {
        let g = "{name: g, kind: check, check: schema, file: out/risks.jsonl, schema: {type: array, items: {type: object, required: [id], properties: {status: {enum: [closed]}}}}}";
        let r = judge(g, &[("out/risks.jsonl", "{\"id\":\"R1\",\"status\":\"closed\"}\n")]);
        assert_eq!(r.verdict, GateVerdict::Pass, "{r:?}");
        let r = judge(g, &[("out/risks.jsonl", "{\"id\":\"R1\",\"status\":\"open\"}\n{\"status\":\"closed\"}\n")]);
        assert_eq!(r.verdict, GateVerdict::Fail);
        assert_eq!(r.findings.len(), 2, "{r:?}");
        assert!(r.findings[0].message.starts_with("/0/status"), "{r:?}");
        let g = "{name: g, kind: check, check: schema, file: 'out/*.md', schema: {type: object, required: [owner]}}";
        let r = judge(g, &[("out/a.md", "---\nowner: x\n---\n# A\n"), ("out/b.md", "# no front matter\n")]);
        assert_eq!(r.findings.len(), 1);
        assert_eq!(r.findings[0].rule, "parse");
    }

    #[test]
    fn text_needs_its_matches_and_none_of_its_forbidden_ones() {
        let g = "{name: g, kind: check, check: text, file: out/design.md, matches: ['\\bP1\\b'], not_matches: ['TODO']}";
        assert_eq!(judge(g, &[("out/design.md", "covers P1\n")]).verdict, GateVerdict::Pass);
        let r = judge(g, &[("out/design.md", "covers P2\nTODO later\n")]);
        let msgs: Vec<String> = r.findings.iter().map(|f| f.line_text()).collect();
        assert_eq!(msgs, ["out/design.md: does not match \"\\\\bP1\\\\b\"", "out/design.md:2: matches \"TODO\": \"TODO\""]);
    }

    #[test]
    fn output_checks_a_value_of_the_run() {
        let g = "{name: g, kind: check, check: output, value: '${stations.a.output}', schema: {type: object, properties: {coverage: {minimum: 0.8}}}}";
        let r = judge(g, &[]);
        assert_eq!(r.verdict, GateVerdict::Fail);
        assert!(r.findings[0].message.contains("0.7"), "{r:?}");
    }

    #[test]
    fn a_path_out_of_the_workspace_is_an_error_not_a_failure() {
        let r = judge("{name: g, kind: check, check: files, paths: ['../elsewhere.md']}", &[]);
        assert_eq!(r.verdict, GateVerdict::Error, "{r:?}");
        let r = judge("{name: g, kind: check, check: files, paths: ['${inputs.none}']}", &[]);
        assert_eq!(r.verdict, GateVerdict::Error, "{r:?}");
    }

    #[test]
    fn sealed_finds_a_file_changed_after_a_gate_read_it() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.md"), "one").unwrap();
        let mut rec = rec();
        let ev = super::super::files::evidence("a.md", b"one");
        rec.stations.get_mut("a").unwrap().checks.push(crate::CheckRec { label: "gate g".into(), evidence: vec![ev], ..Default::default() });
        let g = gate("{name: s, kind: check, check: sealed}");
        let reader = Reader::new(d.path()).unwrap();
        assert_eq!(run(&g, &json!({}), &rec, &reader).verdict, GateVerdict::Pass);
        std::fs::write(d.path().join("a.md"), "two").unwrap();
        let reader = Reader::new(d.path()).unwrap();
        let r = run(&g, &json!({}), &rec, &reader);
        assert_eq!(r.verdict, GateVerdict::Fail);
        assert_eq!(r.findings[0].line_text(), "a.md: changed since gate g at a read it");
    }

    /// Confirmations that bind what they approve, end to end: the runner records the digests and
    /// checks them again before each later station passes.
    mod confirmations {
        use std::sync::Arc;

        use crate::kernel::host::{Outcome, WorkItem};
        use crate::kernel::run::{CheckStatus, StationStatus};
        use crate::runner::{Runner, RunnerConfig, ScriptedHost};
        use crate::{Actor, CreateIn, Engine, FlowError, ManualClock, MemPacks, MemStore, ProjectLayout};

        const FLOW: &str = r#"
name: g
stations:
  - {id: design, deliverables: [{path: design.md}], exit: [{kind: confirm, min: 2}]}
  - {id: build, exit: [{kind: confirm, binds: []}]}
  - {id: ship, exit: [{kind: confirm, binds: []}], rework_to: [design]}
"#;

        struct R {
            rn: Arc<Runner>,
            engine: Arc<Engine>,
            run: String,
            dir: std::path::PathBuf,
            _ws: tempfile::TempDir,
        }

        fn person(id: &str) -> Actor {
            Actor::new("person", id)
        }

        impl R {
            async fn new(flow: &str) -> Self {
                let ws = tempfile::tempdir().unwrap();
                let engine = Arc::new(
                    Engine::new(Arc::new(MemStore::new()), Arc::new(MemPacks::new().flow(flow)))
                        .with_clock(Arc::new(ManualClock::new(1_000_000)))
                        .with_workspaces(ws.path())
                        .with_layout(ProjectLayout::new(".harnix", ws.path().join("worktrees"), "harnix/flow")),
                );
                let cfg = RunnerConfig { lease_ms: 60_000, guarded: vec![], ..Default::default() };
                let rn = Runner::for_tests_with(engine.clone(), ScriptedHost::new(), cfg);
                let o = engine.create(CreateIn { flow: "g".into(), subject: "s".into(), ..Default::default() }, person("owner")).await.unwrap();
                let run = o.value["run"].as_str().unwrap().to_string();
                let rec = engine.record(&run).await.unwrap();
                let dir = engine.workspace_for(&rec).unwrap().join(rec.output_dir.clone().unwrap_or_default());
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(dir.join("design.md"), "# Design v1\n").unwrap();
                R { rn, engine, run, dir, _ws: ws }
            }

            /// Run the reserved gate work an operation handed out, as the dispatcher would.
            async fn work(&self, o: Outcome) -> Vec<String> {
                let mut ran = vec![];
                let mut todo = o.work;
                while let Some(w) = todo.pop() {
                    if let WorkItem::Gate { run, station, round, gate } = w {
                        ran.push(gate.clone());
                        super::super::super::gate::run_gate(&self.rn, run.clone(), station, round, gate).await;
                    }
                }
                ran
            }

            async fn confirm(&self, station: &str, pass: bool, by: &str) -> crate::Result<Vec<String>> {
                let o = self.engine.confirm(&self.run, station, pass, Some("because"), person(by)).await?;
                Ok(self.work(o).await)
            }

            async fn status(&self, station: &str) -> StationStatus {
                self.engine.record(&self.run).await.unwrap().stations[station].status
            }
        }

        #[tokio::test]
        async fn two_approvals_bind_the_design_and_a_later_edit_is_caught() {
            let r = R::new(FLOW).await;
            // one vote per person: alice twice is still one of two
            assert!(r.confirm("design", true, "alice").await.unwrap().is_empty());
            r.confirm("design", true, "alice").await.unwrap();
            let v = r.engine.get(&r.run).await.unwrap();
            assert_eq!((v.votes["design"][0].approvals, v.votes["design"][0].min), (1, 2));
            assert_eq!(v.unmet["design"][0].detail, "awaiting confirmation (1 of 2 approvals)");
            assert_eq!(r.status("design").await, StationStatus::Working);
            // the second person passes it, once what they approved is recorded
            assert_eq!(r.confirm("design", true, "bob").await.unwrap(), ["confirm-binds@0"]);
            let rec = r.engine.record(&r.run).await.unwrap();
            let st = &rec.stations["design"];
            assert_eq!(st.status, StationStatus::Passed, "{:?}", st.checks);
            let binds = st.confirms.last().unwrap().binds.clone().unwrap();
            assert_eq!(binds.len(), 1);
            assert!(binds.keys().next().unwrap().ends_with("design.md"), "{binds:?}");
            assert_eq!(st.checks.iter().find(|c| c.kind == "confirm").unwrap().evidence.len(), 1);

            // unchanged: the next station passes after checking it
            assert_eq!(r.confirm("build", true, "alice").await.unwrap(), ["confirm-stale@"]);
            assert_eq!(r.status("build").await, StationStatus::Passed);
            let rec = r.engine.record(&r.run).await.unwrap();
            assert_eq!(rec.stations["build"].checks.iter().find(|c| c.kind == "confirm-stale").unwrap().status, CheckStatus::Pass);

            // edited after it was approved: the station after is not met, saying which and where to go back
            std::fs::write(r.dir.join("design.md"), "# Design v2\n").unwrap();
            assert_eq!(r.confirm("ship", true, "alice").await.unwrap(), ["confirm-stale@"]);
            assert_eq!(r.status("ship").await, StationStatus::Unmet);
            let v = r.engine.get(&r.run).await.unwrap();
            let stale = v.unmet["ship"].iter().find(|c| c.kind == "confirm-stale").unwrap();
            assert_eq!(stale.status, CheckStatus::Fail);
            assert_eq!(stale.findings[0].rule, "confirm-stale");
            let line = stale.findings[0].line_text();
            assert!(line.contains("design.md: changed since station design approved it; rework to design"), "{line}");

            // put back as it was and run the checks again: passes
            std::fs::write(r.dir.join("design.md"), "# Design v1\n").unwrap();
            let o = r.engine.rerun_gates(&r.run, "ship", person("alice")).await.unwrap();
            assert_eq!(r.work(o).await, ["confirm-stale@"]);
            assert_eq!(r.status("ship").await, StationStatus::Passed);
        }

        #[tokio::test]
        async fn a_rework_to_the_confirming_station_counts_afresh() {
            let r = R::new(FLOW).await;
            r.confirm("design", true, "alice").await.unwrap();
            r.confirm("design", true, "bob").await.unwrap();
            r.confirm("build", true, "alice").await.unwrap();
            std::fs::write(r.dir.join("design.md"), "# Design v2\n").unwrap();
            r.confirm("ship", true, "alice").await.unwrap();
            assert_eq!(r.status("ship").await, StationStatus::Unmet);
            let o = r.engine.rework(&r.run, "design", None, "approve the new design", person("alice")).await.unwrap();
            r.work(o).await;
            let v = r.engine.get(&r.run).await.unwrap();
            assert_eq!(v.record.stations["design"].status, StationStatus::Working);
            assert_eq!((v.votes["design"][0].approvals, v.votes["design"][0].rejections), (0, 0));
            // the old votes stay on record, in their round
            assert_eq!(v.record.stations["design"].confirms.len(), 2);
            // a new round: two votes again, binding v2; the rest follows without a stale finding
            r.confirm("design", true, "alice").await.unwrap();
            assert_eq!(r.status("design").await, StationStatus::Working);
            r.confirm("design", true, "bob").await.unwrap();
            r.confirm("build", true, "alice").await.unwrap();
            r.confirm("ship", true, "alice").await.unwrap();
            let rec = r.engine.record(&r.run).await.unwrap();
            assert_eq!(rec.status, crate::RunStatus::Finished);
        }

        #[tokio::test]
        async fn an_author_may_vote_unless_the_exit_says_not() {
            let r = R::new(FLOW).await;
            // self-review is allowed by default: the one who started the run votes
            r.confirm("design", true, "owner").await.unwrap();
            let r = R::new("{name: g, stations: [{id: a, exit: [{kind: confirm, not: [author]}]}]}").await;
            let e = r.confirm("a", true, "owner").await.unwrap_err();
            let FlowError::Denied(why) = e else { panic!("{e:?}") };
            assert!(why.contains("started the run") && why.contains("not: [author]"), "{why}");
            r.confirm("a", true, "alice").await.unwrap();
            assert_eq!(r.status("a").await, StationStatus::Passed);
        }

        #[tokio::test]
        async fn a_binds_glob_out_of_the_workspace_is_an_error() {
            let r = R::new("{name: g, stations: [{id: a, exit: [{kind: confirm, binds: ['${data_dir}/specs/*.md']}]}]}").await;
            r.confirm("a", true, "alice").await.unwrap();
            let rec = r.engine.record(&r.run).await.unwrap();
            let c = rec.stations["a"].checks.iter().find(|c| c.kind == "confirm").unwrap();
            assert_eq!(c.status, CheckStatus::Error, "{c:?}");
            assert!(c.detail.starts_with("what was approved could not be recorded"), "{}", c.detail);
        }
    }
}
