//! Structural validation: everything a JSON Schema cannot say. A definition with any
//! problem cannot start a run; [`validate`] reports all of them at once.

use std::collections::{BTreeMap, BTreeSet};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::def::{Assert, Executor, ExitCond, FlowDef, Station, TaskSpec, Work, MAX_REWORK_LIMIT};
use super::host::{Catalog, Packs, EVENT_KINDS};
use super::parts::{
    permissions_of, GateDef, GateKind, HookDef, PartKind, Permission, PolicyDef, MAX_GATE_SECONDS,
};
use super::refs::{refs_in, refs_in_value, ROOTS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct Problem {
    /// The station it is about; `None` for the flow as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub station: Option<String>,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.station {
            Some(s) => write!(f, "station {s}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

/// Parse and validate a definition text.
pub fn validate_yaml(yaml: &str, packs: &dyn Packs) -> Vec<Problem> {
    match FlowDef::parse(yaml) {
        Ok(def) => validate(&def, packs),
        Err(e) => vec![Problem {
            station: None,
            message: format!("not a flow definition: {e}"),
        }],
    }
}

/// A skill's name, as a pack can store it: `[a-z][a-z0-9_-]*`.
pub fn is_skill_name(s: &str) -> bool {
    is_name(s, b"_")
}

fn is_name(s: &str, extra: &[u8]) -> bool {
    let b = s.as_bytes();
    !b.is_empty()
        && b[0].is_ascii_lowercase()
        && b.iter().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-' || extra.contains(c)
        })
}

struct Ctx<'a> {
    def: &'a FlowDef,
    packs: &'a dyn Packs,
    catalog: Catalog,
    out: Vec<Problem>,
}

impl Ctx<'_> {
    fn flow(&mut self, m: impl Into<String>) {
        self.out.push(Problem {
            station: None,
            message: m.into(),
        });
    }

    fn at(&mut self, st: &str, m: impl Into<String>) {
        self.out.push(Problem {
            station: Some(st.to_string()),
            message: m.into(),
        });
    }
}

/// Every structural problem of a definition; empty = valid.
pub fn validate(def: &FlowDef, packs: &dyn Packs) -> Vec<Problem> {
    let mut c = Ctx {
        def,
        packs,
        catalog: packs.catalog(),
        out: vec![],
    };
    if let Some(why) = super::def::format_problem(u64::from(def.format), "flow") {
        c.flow(why);
        return c.out;
    }
    // A bundle's flow is named `<bundle>/<flow>` (in Conix: docs/design/layout.md §2).
    let named = match crate::bundle::split(&def.name) {
        Some((b, f)) => def.bundle.as_deref() == Some(b) && is_name(b, &[]) && is_name(f, &[]),
        None => is_name(&def.name, &[]),
    };
    if !named {
        c.flow(format!("name {:?} must match [a-z][a-z0-9-]*", def.name));
    } else if def.name.starts_with(DRY_PREFIX) {
        c.flow(format!(
            "name {:?}: names starting `{DRY_PREFIX}` are kept for dry runs",
            def.name
        ));
    }
    if def.stations.is_empty() {
        c.flow("a flow needs at least one station");
    }
    if let Some(r) = &def.result {
        if let Some(schema) = &r.schema {
            schema_problems(&mut c, None, "result.schema", schema);
        }
        for x in refs_in_value(&r.from) {
            let mut parts = x.split(['.', '[']);
            let root = parts.next().unwrap_or("");
            // The result is read when the run is over: every station is before it, and no task's.
            match root {
                "inputs" | "stations" | "run" | "subject" | "output_dir" => {}
                _ => {
                    c.flow(format!("result: reference {x}: a result may refer to run, subject, inputs, stations and output_dir"));
                    continue;
                }
            }
            let name = parts.next();
            match (root, name) {
                ("inputs", Some(k)) if !def.inputs.contains_key(k) => c.flow(format!("result: reference {x}: no input {k}")),
                ("stations", Some(k)) if def.station(k).is_none() => c.flow(format!("result: reference {x}: no station {k}")),
                _ => {}
            }
        }
    }
    if let Some(n) = def.rework_limit {
        if n == 0 || n > MAX_REWORK_LIMIT {
            c.flow(format!(
                "rework_limit {n} must be between 1 and {MAX_REWORK_LIMIT}"
            ));
        }
    }
    for (k, i) in &def.inputs {
        if !is_name(k, b"_") {
            c.flow(format!("input name {k:?} must match [a-z][a-z0-9_-]*"));
        }
        schema_problems(&mut c, None, &format!("input {k}"), &i.schema);
        if let Some(d) = &i.default {
            if let Ok(v) = jsonschema::validator_for(&i.schema) {
                if !v.is_valid(d) {
                    c.flow(format!(
                        "input {k}: the default does not satisfy the schema"
                    ));
                }
            }
        }
    }
    if let Some(t) = &def.output_dir {
        for m in super::output::template_problems(t, &def.inputs) {
            c.flow(format!("output_dir {t:?}: {m}"));
        }
    }
    // In Conix (docs/design/layout.md §3): a bundle's flow works in the run's own directory.
    if def.bundle.is_some() && def.output_dir.is_some() {
        c.flow("output_dir: a bundle's flow works in the run's own directory; say where it is archived with archive.name".to_string());
    }
    // A bundle's flows, commands and stations share one namespace (the address `/<bundle>:<name>`
    // names any of them); read from the packs, so a flow reports its bundle's collisions too.
    if let Some(b) = &def.bundle {
        for m in bundle_problems(b, packs) {
            c.flow(format!("bundle {b}: {m}"));
        }
    }
    if let Some(a) = &def.archive {
        if let Some(t) = &a.name {
            for m in super::output::template_problems(t, &def.inputs) {
                c.flow(format!("archive.name {t:?}: {m}"));
            }
        }
        for k in &a.keep {
            if !relative_inside(k) {
                c.flow(format!("archive.keep {k:?}: a path in the run's directory, relative, without .."));
            }
        }
    }
    for s in &def.stations {
        for d in &s.deliverables {
            if !relative_inside(&d.path) {
                c.at(&s.id, format!("deliverable {:?}: a path in the run's directory, relative, without ..", d.path));
            }
            if let (Some(t), Some(b)) = (&d.template, &def.bundle) {
                if c.packs.bundle_file(b, &format!("templates/{t}")).is_none() {
                    c.at(&s.id, format!("deliverable {:?}: no template {t} in the bundle's templates/", d.path));
                }
            }
            if d.template.is_some() && def.bundle.is_none() {
                c.at(&s.id, format!("deliverable {:?}: a template is a bundle's (templates/); this flow is in none", d.path));
            }
        }
    }
    if def.workspace == Some(super::run::Workspace::Scratch) {
        c.flow("workspace: a flow's tasks work in the project or a worktree of it (project | worktree)".to_string());
    }
    names_problems(&mut c, None, &def.rules, &def.policies);
    for (i, p) in def.permissions.iter().enumerate() {
        for m in permission_problems(p) {
            c.flow(format!("permissions[{i}]: {m}"));
        }
    }

    let mut ids = BTreeSet::new();
    for s in &def.stations {
        if !is_name(&s.id, b"_") {
            c.at(&s.id, "station id must match [a-z][a-z0-9_-]*");
        }
        if !ids.insert(s.id.as_str()) {
            c.at(&s.id, "station id is not unique");
        }
    }
    let cyclic = graph_problems(&mut c);
    for s in &def.stations {
        station_problems(&mut c, s, cyclic);
    }
    shared_session_problems(&mut c);
    main_problems(&mut c);
    c.out
}

/// The prefix of a dry run's id; no flow may be named with it.
pub const DRY_PREFIX: &str = "dry-";

/// What a session is opened with and keeps: everything a task sharing it must agree on.
#[derive(PartialEq)]
struct SessionSettings {
    scene: String,
    output: Option<Value>,
    model: Option<String>,
    provider: Option<String>,
    effort: Option<String>,
    instance: Option<String>,
    template: Option<String>,
    params: BTreeMap<String, String>,
    space: Option<String>,
}

impl SessionSettings {
    /// The names of the fields that differ.
    fn differ(&self, o: &Self) -> Vec<&'static str> {
        let mut d = vec![];
        if self.scene != o.scene {
            d.push("scene (and so how it hands in)");
        }
        if self.output != o.output {
            d.push("output contract");
        }
        if self.model != o.model || self.provider != o.provider {
            d.push("model");
        }
        if self.effort != o.effort {
            d.push("effort");
        }
        if self.instance != o.instance
            || self.template != o.template
            || self.params != o.params
            || self.space != o.space
        {
            d.push("placement");
        }
        d
    }
}

/// Tasks that may share a session (`per_run`, or the same `{key}` expression) must be opened
/// the same way: the engine takes a session's scene, `output_schema`, permission rules, model
/// and place when it is created and keeps them, so a task arriving later would work under the
/// first one's — another contract. (What a task may do is not kept by its session: each call is
/// judged against the permissions of the task its turn works on.)
fn shared_session_problems(c: &mut Ctx) {
    use super::def::SessionBinding;
    let def = c.def;
    // group → the first (station, task, settings) seen
    let mut first: BTreeMap<String, (String, String, SessionSettings)> = BTreeMap::new();
    for s in &def.stations {
        for t in s.task_specs() {
            let Executor::Session {
                scene,
                model,
                provider,
                effort,
                instance,
                template,
                params,
                space,
                ..
            } = &t.executor
            else {
                continue;
            };
            let group = match t.executor.binding().unwrap_or(SessionBinding::New) {
                // The main session's settings are the flow's `main`, alike for every task.
                SessionBinding::Main | SessionBinding::New => continue,
                SessionBinding::PerRun => "`session: per_run`".to_string(),
                SessionBinding::Key(k) => format!("`session: {{key: {k}}}`"),
            };
            let mine = SessionSettings {
                scene: scene.clone(),
                // Kept by the session only when it hands in by structured output.
                output: if c.catalog.submit_scenes.contains(scene) {
                    None
                } else {
                    s.output.clone()
                },
                model: model.clone(),
                provider: provider.clone(),
                effort: effort.clone(),
                instance: instance.clone(),
                template: template.clone(),
                params: params.clone(),
                space: space.clone(),
            };
            match first.get(&group) {
                None => {
                    first.insert(group, (s.id.clone(), t.id.clone(), mine));
                }
                Some((st, task, theirs)) => {
                    let d = theirs.differ(&mine);
                    if !d.is_empty() {
                        let other = if *st == s.id {
                            format!("task {task} of this station")
                        } else {
                            format!("station {st}")
                        };
                        let m = format!(
                            "task {} shares its session ({group}) with {other}, but its {} \
                             differ: a session keeps what it was opened with — give them the same, \
                             or a session each",
                            t.id,
                            d.join(", ")
                        );
                        c.at(&s.id, m);
                    }
                }
            }
        }
    }
}

/// Needs exist, are not the station itself, and form no cycle. Returns whether a cycle
/// was found (ancestor checks are skipped then).
fn graph_problems(c: &mut Ctx) -> bool {
    let def = c.def;
    let ids: BTreeSet<&str> = def.stations.iter().map(|s| s.id.as_str()).collect();
    let mut edges: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for s in &def.stations {
        let needs = def.needs_of(&s.id);
        let mut seen = BTreeSet::new();
        for n in &needs {
            if n == &s.id {
                c.at(&s.id, "needs itself");
            } else if !ids.contains(n.as_str()) {
                c.at(&s.id, format!("needs {n}, which is not a station"));
            }
            if !seen.insert(n) {
                c.at(&s.id, format!("needs {n} twice"));
            }
        }
        edges.insert(
            s.id.clone(),
            needs
                .into_iter()
                .filter(|n| ids.contains(n.as_str()) && *n != s.id)
                .collect(),
        );
    }
    // Kahn's algorithm: whatever cannot be ordered is on or behind a cycle.
    let mut done: BTreeSet<String> = BTreeSet::new();
    loop {
        let next: Vec<String> = edges
            .iter()
            .filter(|(id, ns)| !done.contains(*id) && ns.iter().all(|n| done.contains(n)))
            .map(|(id, _)| id.clone())
            .collect();
        if next.is_empty() {
            break;
        }
        done.extend(next);
    }
    let stuck: Vec<String> = edges
        .keys()
        .filter(|k| !done.contains(*k))
        .cloned()
        .collect();
    if !stuck.is_empty() {
        c.flow(format!("needs form a cycle through {}", stuck.join(", ")));
    }
    !stuck.is_empty()
}

fn station_problems(c: &mut Ctx, s: &Station, cyclic: bool) {
    let def = c.def;
    let id = s.id.as_str();

    // work shape
    if s.executor.is_some() && s.tasks.is_some() {
        c.at(id, "write either executor or tasks, not both");
    }
    if s.foreach.is_some() && s.executor.is_none() {
        c.at(id, "foreach needs an executor (and no tasks)");
    }
    if s.max_parallel.is_some() && s.foreach.is_none() {
        c.at(id, "max_parallel only goes with foreach");
    }
    if s.max_parallel == Some(0) {
        c.at(id, "max_parallel must be at least 1");
    }
    if s.tasks.is_some() && (s.skill.is_some() || s.retry.is_some()) {
        c.at(id, "with tasks, skill and retry go on each task");
    }
    let work = s.work();
    if matches!(work, Work::None) && (s.skill.is_some() || s.retry.is_some()) {
        c.at(id, "a checkpoint (no executor) has no skill or retry");
    }
    if let Some(ts) = &s.tasks {
        if ts.is_empty() {
            c.at(id, "tasks is empty");
        }
        let mut seen = BTreeSet::new();
        for t in ts {
            if !is_name(&t.id, b"_") {
                c.at(
                    id,
                    format!("task id {:?} must match [a-z][a-z0-9_-]*", t.id),
                );
            }
            if !seen.insert(t.id.as_str()) {
                c.at(id, format!("task id {} is not unique", t.id));
            }
        }
    }
    let specs = work.clone().into_specs();
    for t in &specs {
        executor_problems(c, id, t);
    }
    if let Some(items) = &s.foreach {
        let r = refs_in(items);
        if r.len() != 1 || !items.trim().starts_with("${") || !items.trim().ends_with('}') {
            c.at(
                id,
                "foreach must be one reference, like ${stations.<id>.output.<list>}",
            );
        }
    }

    // exits
    if s.exit.is_empty() {
        c.at(id, "no exit condition");
    }
    let has_subflow = specs
        .iter()
        .any(|t| matches!(t.executor, Executor::Subflow { .. }));
    for e in &s.exit {
        match e {
            ExitCond::Output { .. } => {
                if s.output.is_none() {
                    c.at(id, "an output exit needs an output contract");
                }
                if matches!(work, Work::None) {
                    c.at(id, "an output exit needs tasks; a checkpoint has none");
                }
            }
            ExitCond::Children { .. } if !has_subflow => {
                c.at(id, "a children exit only goes with a subflow executor");
            }
            ExitCond::Children { require: Some(super::def::Require::AtLeast(n)), .. } => {
                let tasks = match &work {
                    Work::Tasks(t) => Some(t.len()),
                    Work::One(_) => Some(1),
                    _ => None,
                };
                if *n == 0 {
                    c.at(id, "children require: at least 1 (or `any`, `all`)");
                } else if let Some(k) = tasks.filter(|k| *n as usize > *k) {
                    c.at(id, format!("children require: {n}, but the station has only {k} task(s)"));
                }
            }
            ExitCond::Gate { gate, .. } => match c.packs.part(PartKind::Gate, gate) {
                None => c.at(id, format!("gate {gate} is not in the packs")),
                Some((text, _)) => {
                    for m in part_problems(PartKind::Gate, gate, &text, c.packs) {
                        c.at(id, format!("gate {gate}: {m}"));
                    }
                    if let Some(g) = c.packs.gate(gate) {
                        if !g.args.is_null() {
                            ref_problems(c, s, &refs_in_value(&g.args));
                        }
                    }
                }
            },
            ExitCond::Confirm { via, mcp, timeout, .. } => {
                use super::def::{duration_ms, ConfirmVia};
                if let Some(t) = timeout {
                    if duration_ms(t).is_none() {
                        c.at(id, format!("confirm timeout {t:?}: a duration such as 90m, 8h, 3d or 1h30m"));
                    }
                }
                match (via.unwrap_or_default(), mcp) {
                    (ConfirmVia::Mcp, None) => c.at(id, "a confirm via mcp says how: mcp {server, request, status, decision}"),
                    (ConfirmVia::Person, Some(_)) => c.at(id, "a confirm with mcp is via: mcp"),
                    (ConfirmVia::Mcp, Some(m)) => {
                        if m.server.trim().is_empty() || m.request.tool.trim().is_empty() || m.status.tool.trim().is_empty() {
                            c.at(id, "confirm mcp: server, request.tool and status.tool are named");
                        }
                        if m.decision.path.trim().is_empty() || m.decision.approved.is_empty() || m.decision.rejected.is_empty() {
                            c.at(id, "confirm mcp: decision says its path and the values that approve and reject");
                        }
                        if let Some(p) = &m.poll {
                            if duration_ms(p).is_none() {
                                c.at(id, format!("confirm mcp poll {p:?}: a duration such as 30s or 5m"));
                            }
                        }
                        for r in refs_in_value(&m.request.arguments) {
                            if r.split(['.', '[']).next() == Some("request") {
                                c.at(id, format!("confirm mcp request: {r} — `request` is only there for status"));
                            }
                        }
                        let refs: Vec<String> = refs_in_value(&m.request.arguments)
                            .into_iter()
                            .chain(refs_in_value(&m.status.arguments))
                            .filter(|r| !matches!(r.split(['.', '[']).next(), Some("request" | "station")))
                            .collect();
                        ref_problems(c, s, &refs);
                    }
                    (ConfirmVia::Person, None) => {}
                }
            }
            _ => {}
        }
    }
    if let Some(o) = &s.output {
        schema_problems(c, Some(id), "output", o);
    }

    // rules, policies, permissions
    names_problems(c, Some(id), &s.rules, &s.policies);
    for (i, p) in s.permissions.iter().enumerate() {
        for m in permission_problems(p) {
            c.at(id, format!("permissions[{i}]: {m}"));
        }
    }
    let policies: BTreeMap<String, PolicyDef> = def
        .policies
        .iter()
        .chain(s.policies.iter())
        .filter_map(|n| c.packs.policy(n).map(|p| (n.clone(), p)))
        .collect();
    let refs: Vec<String> = permissions_of(def, s, &policies)
        .iter()
        .flat_map(|g| {
            g.permission
                .patterns()
                .flat_map(|p| refs_in(p).into_iter().map(String::from).collect::<Vec<_>>())
                .collect::<Vec<_>>()
        })
        .collect();
    // writes: the allow-list of where its tasks write in the project
    let mut refs = refs;
    for (i, w) in s.writes.iter().enumerate() {
        if w.trim().is_empty() {
            c.at(id, format!("writes[{i}]: a glob is empty"));
        } else if w.split('/').any(|seg| seg.contains("**") && seg != "**") {
            c.at(id, format!("writes[{i}]: {w}: `**` must be a whole path segment"));
        }
        if w.matches('[').count() != w.matches(']').count() {
            c.at(id, format!("writes[{i}]: {w}: unbalanced brackets"));
        }
        for r in refs_in(w) {
            let root = r.split(['.', '[']).next().unwrap_or("");
            if !ROOTS.contains(&root) {
                c.at(id, format!("writes[{i}]: reference {r}: unknown root {root:?}"));
            } else {
                refs.push(r.to_string());
            }
        }
    }
    ref_problems(c, s, &refs);

    // when
    for a in &s.when {
        assert_problems(c, id, a, true);
    }

    // rework
    if !cyclic {
        for r in &s.rework_to {
            if def.station(r).is_none() {
                c.at(id, format!("rework_to {r} is not a station"));
            } else if !def.is_self_or_ancestor(r, id) {
                c.at(
                    id,
                    format!("rework_to {r} is neither this station nor an ancestor"),
                );
            }
        }
        if let Some(u) = &s.on_unmet {
            if def.station(&u.rework_to).is_none() {
                c.at(
                    id,
                    format!("on_unmet.rework_to {} is not a station", u.rework_to),
                );
            } else if !def.is_self_or_ancestor(&u.rework_to, id) {
                c.at(
                    id,
                    format!(
                        "on_unmet.rework_to {} is neither this station nor an ancestor",
                        u.rework_to
                    ),
                );
            }
        }
    }
    if let Some(u) = &s.on_unmet {
        if u.max == 0 {
            c.at(id, "on_unmet.max must be at least 1");
        }
    }
}

fn executor_problems(c: &mut Ctx, id: &str, t: &TaskSpec) {
    match &t.executor {
        e @ Executor::Session {
            scene,
            model,
            provider,
            effort,
            instance,
            template,
            params,
            space,
            ..
        } if e.in_main() => {
            let placed = !scene.is_empty()
                || model.is_some()
                || provider.is_some()
                || effort.is_some()
                || instance.is_some()
                || template.is_some()
                || !params.is_empty()
                || space.is_some();
            if placed {
                c.at(
                    id,
                    format!(
                        "session task {}: a task in the main session takes its scene, model and \
                         place from the flow's `main` (another model: a sub-agent, `kind: agent`; \
                         a session of its own: `session: new`)",
                        t.id
                    ),
                );
            }
            if t.skill.is_none() {
                c.at(id, format!("session task {} needs a skill", t.id));
            }
            needs_output(c, id, &t.id, "session");
        }
        Executor::Agent { agent_type, model, .. } => {
            for (what, v) in [("agent_type", agent_type), ("model", model)] {
                if v.as_ref().is_some_and(|s| s.trim().is_empty()) {
                    c.at(id, format!("agent task {}: {what} is empty", t.id));
                }
            }
            needs_output(c, id, &t.id, "agent");
        }
        Executor::Session {
            scene,
            session,
            model,
            provider,
            effort,
            instance,
            template,
            params,
            space,
        } => {
            if let Some(why) = effort_problem(effort.as_deref()) {
                c.at(id, format!("session task {}: {why}", t.id));
            }
            if scene.trim().is_empty() {
                c.at(id, format!("session task {} needs a scene", t.id));
            } else if !c.catalog.scenes.contains(scene) {
                c.at(id, format!("scene {scene} is not in the catalog"));
            }
            if t.skill.is_none() {
                c.at(id, format!("session task {} needs a skill", t.id));
            }
            // The output is what a session hands in, by structured output or by `flow_submit`.
            if c.def.station(id).is_some_and(|s| s.output.is_none()) {
                c.at(
                    id,
                    format!(
                        "session task {} needs an output contract: it is what the session hands in",
                        t.id
                    ),
                );
            }
            if let Some(super::def::SessionBinding::Key(k)) = session {
                if refs_in(k).is_empty() {
                    c.at(id, "a session key should contain a reference");
                }
                if k.starts_with('@') {
                    c.at(id, "a session key cannot start with `@` (kept for the main session)");
                }
            }
            placement_problems(
                c,
                id,
                &t.id,
                Placed {
                    model,
                    provider,
                    instance,
                    template,
                    params,
                    space,
                },
            );
        }
        Executor::Program { step } => {
            if !c.catalog.steps.contains(step) {
                c.at(id, format!("step {step} is not in the catalog"));
            }
        }
        Executor::External { .. } => {}
        Executor::Subflow { flow, .. } => {
            if c.packs.flow(flow).is_none() {
                c.at(id, format!("subflow {flow} is not a flow in the packs"));
            } else if let Some(path) = subflow_cycle(c.packs, &c.def.name, flow) {
                c.at(
                    id,
                    format!(
                        "subflow {flow} leads back to this flow: {}",
                        path.join(" → ")
                    ),
                );
            }
        }
    }
    if let Some(skill) = &t.skill {
        if c.packs.skill(skill).is_none() {
            c.at(id, format!("skill {skill} is not in the packs"));
        }
    }
}

/// A session executor's placement fields.
struct Placed<'a> {
    model: &'a Option<String>,
    provider: &'a Option<String>,
    instance: &'a Option<String>,
    template: &'a Option<String>,
    params: &'a BTreeMap<String, String>,
    space: &'a Option<String>,
}

/// `instance` or `template`, not both; `params` only with `template`, their references sound;
/// each name in the catalog when the application filled that part of it in.
/// A task the station's sessions or sub-agents hand in: it needs the station's output contract.
fn needs_output(c: &mut Ctx, id: &str, task: &str, kind: &str) {
    if c.def.station(id).is_some_and(|s| s.output.is_none()) {
        c.at(
            id,
            format!("{kind} task {task} needs an output contract: it is what is handed in"),
        );
    }
}

/// The flow's `main`: a scene that hands in with `flow_submit`, and a place that exists.
fn main_problems(c: &mut Ctx) {
    let uses_main = c.def.stations.iter().any(|s| {
        s.task_specs().iter().any(|t| t.executor.in_main())
    });
    let m = c.def.main.clone().unwrap_or_default();
    let scene = m.scene.clone().or_else(|| c.catalog.main_scene.clone());
    match &scene {
        Some(sc) if sc.trim().is_empty() => c.flow("main.scene is empty"),
        Some(sc) => {
            if !c.catalog.scenes.is_empty() && !c.catalog.scenes.contains(sc) {
                c.flow(format!("main scene {sc} is not in the catalog"));
            } else if !c.catalog.submit_scenes.is_empty() && !c.catalog.submit_scenes.contains(sc) {
                c.flow(format!(
                    "main scene {sc} does not hand in with flow_submit: the main session's tasks \
                     have different contracts, so it needs a wrapped flow-* scene"
                ));
            }
        }
        None if uses_main && !c.catalog.scenes.is_empty() => c.flow(
            "tasks run in the main session, but the flow names no main.scene and the \
             application has no main scene",
        ),
        None => {}
    }
    let empty = |v: &Option<String>| v.as_ref().is_some_and(|s| s.trim().is_empty());
    for (what, v) in [
        ("model", &m.model),
        ("provider", &m.provider),
        ("instance", &m.instance),
        ("template", &m.template),
        ("space", &m.space),
    ] {
        if empty(v) {
            c.flow(format!("main.{what} is empty"));
        }
    }
    if let Some(why) = effort_problem(m.effort.as_deref()) {
        c.flow(format!("main: {why}"));
    }
    if m.instance.is_some() && m.template.is_some() {
        c.flow("main: name an instance or a template, not both");
    }
    if !m.params.is_empty() && m.template.is_none() {
        c.flow("main: params only go with a template");
    }
    let check = |set: &BTreeSet<String>, v: &Option<String>| -> Option<String> {
        let v = v.as_ref()?;
        (!set.is_empty() && !set.contains(v)).then(|| v.clone())
    };
    if let Some(t) = check(&c.catalog.templates, &m.template) {
        c.flow(format!("main: template {t} is not in the catalog"));
    }
    if let Some(i) = check(&c.catalog.instances, &m.instance) {
        c.flow(format!("main: instance {i} is not in the catalog"));
    }
    if let Some(s) = check(&c.catalog.spaces, &m.space) {
        c.flow(format!("main: space {s} is not in the catalog"));
    }
}

/// An `effort` that is not one of [`super::def::EFFORTS`], said.
fn effort_problem(effort: Option<&str>) -> Option<String> {
    let e = effort?;
    (!super::def::EFFORTS.contains(&e)).then(|| format!("effort {e:?} is not one of {}", super::def::EFFORTS.join(", ")))
}

fn placement_problems(c: &mut Ctx, id: &str, task: &str, p: Placed) {
    let empty = |v: &Option<String>| v.as_ref().is_some_and(|s| s.trim().is_empty());
    for (what, v) in [
        ("model", p.model),
        ("provider", p.provider),
        ("instance", p.instance),
        ("template", p.template),
        ("space", p.space),
    ] {
        if empty(v) {
            c.at(id, format!("session task {task}: {what} is empty"));
        }
    }
    if p.instance.is_some() && p.template.is_some() {
        c.at(
            id,
            format!("session task {task}: name an instance or a template, not both"),
        );
    }
    if !p.params.is_empty() && p.template.is_none() {
        c.at(
            id,
            format!("session task {task}: params only go with a template"),
        );
    }
    let st = c.def.station(id).cloned();
    if let Some(st) = &st {
        let refs: Vec<String> = p
            .params
            .values()
            .flat_map(|v| refs_in(v).into_iter().map(String::from).collect::<Vec<_>>())
            .collect();
        ref_problems(c, st, &refs);
    }
    let check = |set: &BTreeSet<String>, v: &Option<String>| -> Option<String> {
        let v = v.as_ref()?;
        (!set.is_empty() && !set.contains(v)).then(|| v.clone())
    };
    if let Some(t) = check(&c.catalog.templates, p.template) {
        c.at(id, format!("template {t} is not in the catalog"));
    }
    if let Some(i) = check(&c.catalog.instances, p.instance) {
        c.at(id, format!("instance {i} is not in the catalog"));
    }
    if let Some(s) = check(&c.catalog.spaces, p.space) {
        c.at(id, format!("space {s} is not in the catalog"));
    }
    if let (Some(m), false) = (p.model, c.catalog.models.is_empty()) {
        let known = c.catalog.models.contains(m)
            || p.provider
                .as_ref()
                .is_some_and(|pr| c.catalog.models.contains(&format!("{pr}/{m}")))
            || (p.provider.is_none()
                && c.catalog
                    .models
                    .iter()
                    .any(|e| e.split_once('/').is_some_and(|(_, mm)| mm == m)));
        if !known {
            let named = match p.provider {
                Some(pr) => format!("{pr}/{m}"),
                None => m.clone(),
            };
            c.at(id, format!("model {named} is not in the catalog"));
        }
    }
}

/// A chain of subflows from `start` back to `root`, if there is one: `[root, start, …, root]`.
/// The pack's current definitions are followed; a flow already visited is not entered again.
fn subflow_cycle(packs: &dyn Packs, root: &str, start: &str) -> Option<Vec<String>> {
    fn children(def: &FlowDef) -> Vec<String> {
        let mut out = vec![];
        for s in &def.stations {
            for t in s.task_specs() {
                if let Executor::Subflow { flow, .. } = t.executor {
                    out.push(flow);
                }
            }
        }
        out
    }
    let mut seen = BTreeSet::new();
    let mut stack = vec![vec![root.to_string(), start.to_string()]];
    while let Some(path) = stack.pop() {
        let last = path.last().cloned().unwrap_or_default();
        if last == root {
            return Some(path);
        }
        if !seen.insert(last.clone()) {
            continue;
        }
        let Some((def, _)) = packs.flow(&last) else {
            continue;
        };
        for next in children(&def) {
            let mut p = path.clone();
            p.push(next);
            stack.push(p);
        }
    }
    None
}

fn assert_problems(c: &mut Ctx, id: &str, a: &Assert, against_context: bool) {
    if a.path.trim().is_empty() {
        c.at(id, "an assertion needs a path");
    }
    if a.op_count() != 1 {
        c.at(
            id,
            format!(
                "assertion on {:?} needs exactly one of eq, ne, len, min_len, exists",
                a.path
            ),
        );
    }
    if against_context && !a.path.trim().is_empty() {
        let st = c.def.station(id).cloned();
        if let Some(st) = st {
            ref_problems(c, &st, std::slice::from_ref(&a.path));
        }
    }
}

/// A reference's root exists; `inputs.k` is declared; `stations.x` is an ancestor;
/// `item` only inside foreach.
fn ref_problems(c: &mut Ctx, s: &Station, refs: &[String]) {
    for r in refs {
        let mut parts = r.split(['.', '[']);
        let root = parts.next().unwrap_or("");
        if !ROOTS.contains(&root) {
            c.at(&s.id, format!("reference {r}: unknown root {root:?}"));
            continue;
        }
        match root {
            "inputs" => {
                if let Some(k) = parts.next() {
                    if !c.def.inputs.contains_key(k) {
                        c.at(&s.id, format!("reference {r}: no input {k}"));
                    }
                }
            }
            "stations" => {
                if let Some(x) = parts.next() {
                    if c.def.station(x).is_none() {
                        c.at(&s.id, format!("reference {r}: no station {x}"));
                    } else if x == s.id || !c.def.descendants(x).contains(&s.id) {
                        c.at(
                            &s.id,
                            format!("reference {r}: station {x} is not an ancestor"),
                        );
                    }
                }
            }
            "item" if s.foreach.is_none() => {
                c.at(
                    &s.id,
                    format!("reference {r}: item only exists in a foreach station"),
                );
            }
            _ => {}
        }
    }
}

fn schema_problems(c: &mut Ctx, station: Option<&str>, what: &str, schema: &Value) {
    if let Err(e) = jsonschema::meta::validate(schema) {
        let m = format!("{what} is not a valid JSON Schema: {e}");
        match station {
            Some(s) => c.at(s, m),
            None => c.flow(m),
        }
    }
}

/// The rules and policies a flow or station names are in the packs (a policy that does not
/// parse is reported as such).
fn names_problems(c: &mut Ctx, station: Option<&str>, rules: &[String], policies: &[String]) {
    let mut ms = vec![];
    for r in rules {
        if c.packs.rule(r).is_none() {
            ms.push(format!("rule {r} is not in the packs"));
        }
    }
    for p in policies {
        match c.packs.part(PartKind::Policy, p) {
            None => ms.push(format!("policy {p} is not in the packs")),
            Some((text, _)) => {
                for m in part_problems(PartKind::Policy, p, &text, c.packs) {
                    ms.push(format!("policy {p}: {m}"));
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    for n in rules {
        if !seen.insert(("rule", n)) {
            ms.push(format!("rule {n} is named twice"));
        }
    }
    for n in policies {
        if !seen.insert(("policy", n)) {
            ms.push(format!("policy {n} is named twice"));
        }
    }
    for m in ms {
        match station {
            Some(s) => c.at(s, m),
            None => c.flow(m),
        }
    }
}

/// What is wrong with one permission, on its own.
pub fn permission_problems(p: &Permission) -> Vec<String> {
    let mut out = vec![];
    if p.tools.is_empty() {
        out.push("names no tool".to_string());
    }
    for t in &p.tools {
        if let Some(m) = super::protocol::guard_problem(t) {
            out.push(m.replacen("guarded tool pattern", "tool pattern", 1));
        }
    }
    for pat in p.patterns() {
        if pat.trim().is_empty() {
            out.push("a pattern is empty".to_string());
        }
    }
    for k in p.args.keys() {
        if k.trim().is_empty() {
            out.push("an args field has no name".to_string());
        }
    }
    for pat in p.patterns() {
        for r in refs_in(pat) {
            let root = r.split(['.', '[']).next().unwrap_or("");
            if !ROOTS.contains(&root) {
                out.push(format!("reference {r}: unknown root {root:?}"));
            }
        }
    }
    out
}

/// A part's name, as a pack can store it: `[a-z][a-z0-9-]*` (skills may have `_`).
pub fn is_part_name(kind: PartKind, name: &str) -> bool {
    // A bundle's part: `<bundle>/<name>`, each a name (Conix: docs/design/layout.md §2).
    if let Some((bundle, local)) = crate::bundle::split(name) {
        return is_name(bundle, &[]) && (crate::bundle::is_made(kind, local) || is_part_name(kind, local));
    }
    match kind {
        PartKind::Skill => is_skill_name(name),
        _ => is_name(name, &[]),
    }
}

/// The problems in a bundle's one namespace (see [`crate::bundle::namespace_problems`]), read from
/// the packs: its flows, the commands its `bundle.yaml` declares, their stations. In Conix.
pub fn bundle_problems(bundle: &str, packs: &dyn Packs) -> Vec<String> {
    let meta = packs.bundle_file(bundle, crate::bundle::BUNDLE_FILE).and_then(|t| serde_yaml::from_str::<crate::bundle::BundleMeta>(&t).ok());
    let commands: Vec<&str> = meta.as_ref().map(|m| m.commands.names().collect()).unwrap_or_default();
    // The bundle's own main flow (`flow.yaml`, offered as the bundle's name) has no local name.
    let flows: Vec<(String, Vec<String>)> = packs
        .flows()
        .into_iter()
        .filter_map(|n| {
            let (b, local) = crate::bundle::split(&n)?;
            (b == bundle).then(|| (local.to_string(), packs.flow(&n).map(|(d, _)| d.stations.iter().map(|s| s.id.clone()).collect()).unwrap_or_default()))
        })
        .collect();
    crate::bundle::namespace_problems(&commands, &flows)
}

/// Every problem of a part's text on its own (a flow that uses it is judged by [`validate`]);
/// empty = it may be saved.
pub fn part_problems(kind: PartKind, name: &str, text: &str, packs: &dyn Packs) -> Vec<String> {
    let mut out = vec![];
    if !is_part_name(kind, name) {
        out.push(format!("name {name:?} must match [a-z][a-z0-9-]*"));
    }
    let named = |declared: &str, out: &mut Vec<String>| {
        if !crate::bundle::names_match(declared, name) {
            out.push(format!("it declares the name {declared:?}, but is saved as {name:?}"));
        }
    };
    match kind {
        PartKind::Skill | PartKind::Rule => {
            if text.trim().is_empty() {
                out.push("it is empty".to_string());
            }
        }
        PartKind::Policy => match super::def::parse_part::<PolicyDef>(text, "policy") {
            Err(e) => out.push(format!("not a policy: {e}")),
            Ok(p) => {
                named(&p.name, &mut out);
                if p.permissions.is_empty() {
                    out.push("it has no permissions".to_string());
                }
                for (i, x) in p.permissions.iter().enumerate() {
                    for m in permission_problems(x) {
                        out.push(format!("permissions[{i}]: {m}"));
                    }
                }
            }
        },
        PartKind::Gate => match super::def::parse_part::<GateDef>(text, "gate") {
            Err(e) => out.push(format!("not a gate: {e}")),
            Ok(g) => {
                named(&g.name, &mut out);
                let mut g = g;
                // A bundle's gate runs the bundle's script (in Conix: docs/design/layout.md §2).
                if let (Some((b, _)), Some(sc)) = (crate::bundle::split(name), g.script.as_mut()) {
                    *sc = crate::bundle::in_bundle(b, sc);
                }
                out.extend(gate_problems(&g, packs));
            }
        },
        PartKind::Hook => match super::def::parse_part::<HookDef>(text, "hook") {
            Err(e) => out.push(format!("not a hook: {e}")),
            Ok(h) => {
                named(&h.name, &mut out);
                out.extend(hook_problems(&h));
            }
        },
        PartKind::Script => {
            if text.trim().is_empty() {
                out.push("it is empty".to_string());
            }
            #[cfg(feature = "script")]
            out.extend(crate::runner::script::problems(text));
        }
    }
    out
}

fn gate_problems(g: &GateDef, packs: &dyn Packs) -> Vec<String> {
    use super::parts::MAX_SCRIPT_SECONDS;
    let catalog = packs.catalog();
    let mut out = vec![];
    // Which fields go with which kind (and, for `check`, with which checker): one that is set on
    // a gate it does not go with is a mistake the author should hear about, not a silent no-op.
    let tool_fields = g.tool.is_some() || !g.args.is_null() || !g.assert.is_empty();
    let command_only = g.command.is_some() || g.cwd.is_some() || !g.env.is_empty() || g.result.is_some();
    let check_fields = g.check.is_some()
        || !g.paths.is_empty()
        || g.non_empty.is_some()
        || !g.not_containing.is_empty()
        || g.file.is_some()
        || g.read_as.is_some()
        || g.schema.is_some()
        || !g.matches.is_empty()
        || !g.not_matches.is_empty()
        || !g.value.is_null();
    let script_fields = g.script.is_some() || g.function.is_some() || !g.params.is_null();
    let others = |out: &mut Vec<String>, kind: &str, tool: bool, command: bool, check: bool, script: bool, timeout: bool| {
        let mut extra = vec![];
        if tool && tool_fields {
            extra.push("tool, args and assert go with a tool gate");
        }
        if command && command_only {
            extra.push("command, cwd, env and result go with a command gate");
        }
        if check && check_fields {
            extra.push("check and its fields go with a check gate");
        }
        if script && script_fields {
            extra.push("script, function and params go with a script gate");
        }
        if timeout && g.timeout_seconds.is_some() {
            extra.push("timeout_seconds goes with a command or script gate");
        }
        for e in extra {
            out.push(format!("a {kind} gate: {e}"));
        }
    };
    match g.kind {
        GateKind::Tool => {
            match &g.tool {
                None => out.push("a tool gate names its tool".to_string()),
                Some(t) if !catalog.tools.contains(t) => out.push(format!("tool {t} is not in the catalog")),
                Some(_) => {}
            }
            if g.assert.is_empty() {
                out.push("a tool gate needs an assertion".to_string());
            }
            for a in &g.assert {
                if a.path.trim().is_empty() {
                    out.push("an assertion needs a path".to_string());
                }
                if a.op_count() != 1 {
                    out.push(format!(
                        "assertion on {:?} needs exactly one of eq, ne, len, min_len, exists",
                        a.path
                    ));
                }
            }
            others(&mut out, "tool", false, true, true, true, true);
        }
        GateKind::Command => {
            if !catalog.command_gates {
                out.push("command gates are not turned on in this application".to_string());
            }
            if g.command.as_deref().is_none_or(|c| c.trim().is_empty()) {
                out.push("a command gate needs its command".to_string());
            }
            if g.timeout_seconds.is_some_and(|t| t == 0 || t > MAX_GATE_SECONDS) {
                out.push(format!("timeout_seconds must be between 1 and {MAX_GATE_SECONDS}"));
            }
            for e in &g.env {
                let name = e.strip_suffix('*').unwrap_or(e);
                let ok = !name.is_empty()
                    && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
                    && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
                if !ok {
                    out.push(format!("env {e:?}: a variable name, which may end in *"));
                }
            }
            others(&mut out, "command", true, false, true, true, false);
        }
        GateKind::Check => {
            check_problems(g, &mut out);
            others(&mut out, "check", true, true, false, true, true);
        }
        GateKind::Script => {
            match (&g.script, &g.function) {
                (Some(_), Some(_)) => out.push("a script gate runs a script or a function, not both".to_string()),
                (None, None) => out.push("a script gate names its script (or a function of the application)".to_string()),
                (Some(sc), None) => {
                    if packs.part(PartKind::Script, sc).is_none() {
                        out.push(format!("there is no script {sc} (scripts/{sc}.js)"));
                    }
                }
                (None, Some(f)) => {
                    if !catalog.gates.contains(f) {
                        out.push(format!("the application has no function {f} for gates"));
                    }
                }
            }
            if g.timeout_seconds.is_some_and(|t| t == 0 || t > MAX_SCRIPT_SECONDS) {
                out.push(format!("timeout_seconds must be between 1 and {MAX_SCRIPT_SECONDS}"));
            }
            others(&mut out, "script", true, true, true, false, false);
        }
        GateKind::Host => {
            if !catalog.gates.contains(&g.name) {
                out.push(format!("the application has no check named {}", g.name));
            }
            if tool_fields || command_only || check_fields || script_fields || g.timeout_seconds.is_some() {
                out.push("a host gate has nothing but its name, title and description".to_string());
            }
        }
    }
    let mut refs = refs_in_value(&g.args);
    refs.extend(refs_in_value(&g.value));
    refs.extend(refs_in_value(&g.params));
    for s in g.cwd.iter().chain(g.file.iter()).chain(g.paths.iter()) {
        refs.extend(refs_in(s).into_iter().map(String::from));
    }
    for r in refs {
        let root = r.split(['.', '[']).next().unwrap_or("");
        if !ROOTS.contains(&root) {
            out.push(format!("reference {r}: unknown root {root:?}"));
        }
    }
    out
}

/// A `check` gate's own problems: its checker, the fields that checker needs, and nothing it
/// does not take.
fn check_problems(g: &GateDef, out: &mut Vec<String>) {
    use super::parts::CheckKind as K;
    let Some(kind) = g.check else {
        out.push("a check gate says which check: files, schema, text, output or sealed".to_string());
        return;
    };
    // (field, set?, the checkers it goes with)
    let fields: [(&str, bool, &[K]); 9] = [
        ("paths", !g.paths.is_empty(), &[K::Files]),
        ("non_empty", g.non_empty.is_some(), &[K::Files]),
        ("not_containing", !g.not_containing.is_empty(), &[K::Files]),
        ("file", g.file.is_some(), &[K::Schema, K::Text]),
        ("as", g.read_as.is_some(), &[K::Schema]),
        ("schema", g.schema.is_some(), &[K::Schema, K::Output]),
        ("matches", !g.matches.is_empty(), &[K::Text]),
        ("not_matches", !g.not_matches.is_empty(), &[K::Text]),
        ("value", !g.value.is_null(), &[K::Output]),
    ];
    let name = serde_json::to_value(kind).ok().and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    for (field, set, with) in fields {
        if set && !with.contains(&kind) {
            out.push(format!("check {name} takes no {field}"));
        }
    }
    match kind {
        K::Files => {
            if g.paths.is_empty() {
                out.push("check files needs paths".to_string());
            }
        }
        K::Schema | K::Text => {
            if g.file.as_deref().is_none_or(|f| f.trim().is_empty()) {
                out.push(format!("check {name} needs a file"));
            }
        }
        K::Output | K::Sealed => {}
    }
    if matches!(kind, K::Schema | K::Output) {
        match &g.schema {
            None => out.push(format!("check {name} needs a schema")),
            Some(s) => {
                if let Err(e) = jsonschema::meta::validate(s) {
                    out.push(format!("schema: {e}"));
                } else if let Err(e) = jsonschema::validator_for(s) {
                    out.push(format!("schema: {e}"));
                }
            }
        }
    }
    if kind == K::Text {
        if g.matches.is_empty() && g.not_matches.is_empty() {
            out.push("check text needs matches or not_matches".to_string());
        }
        for re in g.matches.iter().chain(&g.not_matches) {
            if let Err(e) = regex::Regex::new(re) {
                out.push(format!("{re:?} is not a regular expression: {e}"));
            }
        }
    }
}

fn hook_problems(h: &HookDef) -> Vec<String> {
    let mut out = vec![];
    for on in &h.on {
        let known = match on.strip_suffix('*') {
            Some(prefix) => EVENT_KINDS.iter().any(|k| k.starts_with(prefix)),
            None => EVENT_KINDS.contains(&on.as_str()),
        };
        if !known {
            out.push(format!("on {on:?}: not an event kind"));
        }
    }
    for f in &h.flows {
        if !is_name(f, &[]) {
            out.push(format!("flows: {f:?} is not a flow name"));
        }
    }
    let a = h.action.trim();
    if !(a.starts_with("http://") || a.starts_with("https://")) || a.len() < 10 {
        out.push(format!("action {a:?}: a webhook's action is an http(s) URL"));
    }
    if h.secret_env.as_deref().is_some_and(|v| v.trim().is_empty()) {
        out.push("secret_env is empty".to_string());
    }
    out
}

/// A path relative to a directory that stays in it: not absolute, no `..`, not empty.
fn relative_inside(p: &str) -> bool {
    !p.trim().is_empty() && !p.starts_with('/') && !p.starts_with('\\') && !p.split(['/', '\\']).any(|s| s == "..")
}
