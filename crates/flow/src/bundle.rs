//! Flow bundles (in Conix; its docs/design/layout.md §2): a directory with a `bundle.yaml`, holding
//! one or more flows — paths of one piece of work, `solid.yaml` and `quick.yaml` say — and every part
//! they use. A bundle is self-contained: what its flows name (`skill: propose`, `gate: spec-delta`,
//! `policies: [x]`, a subflow) is the bundle's, offered as `<bundle>/<name>`; nothing is looked up in
//! another bundle or below. So a bundle can be copied, packed and moved as one directory.
//!
//! ```text
//! <bundle>/
//!   bundle.yaml                 name (the directory's), title, description, version
//!   <flow>.yaml …               offered as <bundle>/<flow>; flow.yaml as <bundle>
//!   skills/<name>/SKILL.md      local.md beside it: a section a user adds (read after the skill)
//!   rules/ policies/ gates/ hooks/ scripts/
//!   templates/                  skeletons of the deliverables stations declare
//!   commands/<name>.js          the bundle's own commands (`flow_command`)
//! ```
//!
//! A bundle's flow file says its own name in `name:` (the file's stem; `flow.yaml` says the bundle's);
//! read, it is offered under its full name and every name it says is made the bundle's. Its gates,
//! policies and hooks say their own names too, and are offered under full names the same way
//! ([`crate::Packs::gate`] takes either).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::kernel::def::{Executor, ExitCond, FlowDef, Pace};
use crate::kernel::parts::PartKind;

/// The file that makes a directory a bundle.
pub const BUNDLE_FILE: &str = "bundle.yaml";
/// The flow file offered under the bundle's own name.
pub const MAIN_FLOW: &str = "flow";
/// What marks a deliverable's skeleton as not written yet: a station with deliverables is not met
/// while one still holds it.
pub const TEMPLATE_MARK: &str = "<!-- conix:template -->";
/// A skill's section a user adds, beside its `SKILL.md`.
pub const LOCAL_SKILL: &str = "local.md";
/// The gate a station with deliverables is judged by besides its own, made from what it declares:
/// `<bundle>/deliverables@<flow file>.<station>`.
pub const DELIVERABLES_GATE: &str = "deliverables@";

/// What `bundle.yaml` says.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleMeta {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub version: String,
    /// The bundle's commands, each `commands/<name>.js`: what sessions, people and stations may run.
    #[serde(default, skip_serializing_if = "Commands::is_empty")]
    pub commands: Commands,
    /// Its flows by file stem, in the order people are offered them (and a start is routed by,
    /// unless `start.route` says otherwise). Absent: every flow file, by name.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<String>,
    /// How a start that names the bundle alone is routed to one of its flows
    /// ([`crate::kernel::route`]).
    #[serde(default, skip_serializing_if = "StartDef::is_empty")]
    pub start: StartDef,
    /// How its runs go from station to station when a flow says nothing ([`Pace`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    /// What running it needs of the host (checked when a package is imported).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires: Option<BundleRequires>,
}

/// What `bundle.yaml` says of one command.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct CommandDecl {
    /// The first arguments that only read (`list`, `get`, `help`): a call with one of them gets no
    /// writer, so it cannot change anything whoever runs it. Every other call counts as writing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub read_only: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A bundle's commands in the order `bundle.yaml` gives them: written as a list of names (every
/// subcommand writes) or as a map from a name to its [`CommandDecl`]. The list stays a form so
/// bundles written before the map still read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Commands(pub Vec<(String, CommandDecl)>);

impl Commands {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(|(n, _)| n.as_str())
    }
    pub fn get(&self, name: &str) -> Option<&CommandDecl> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, d)| d)
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CommandsForm {
    List(Vec<String>),
    // A `Mapping`, not a `BTreeMap`: the order the file gives is the order commands are listed in.
    Map(serde_yaml::Mapping),
}

impl<'de> Deserialize<'de> for Commands {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match CommandsForm::deserialize(d)? {
            CommandsForm::List(l) => Ok(Commands(l.into_iter().map(|n| (n, CommandDecl::default())).collect())),
            CommandsForm::Map(m) => m
                .into_iter()
                .map(|(k, v)| {
                    let name = k.as_str().ok_or_else(|| D::Error::custom("a command's name is a string"))?.to_string();
                    let decl = if v.is_null() { CommandDecl::default() } else { serde_yaml::from_value(v).map_err(D::Error::custom)? };
                    Ok((name, decl))
                })
                .collect::<Result<_, _>>()
                .map(Commands),
        }
    }
}

impl Serialize for Commands {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        if self.0.iter().all(|(_, d)| *d == CommandDecl::default()) {
            let mut q = s.serialize_seq(Some(self.0.len()))?;
            for (n, _) in &self.0 {
                q.serialize_element(n)?;
            }
            q.end()
        } else {
            let mut m = s.serialize_map(Some(self.0.len()))?;
            for (n, d) in &self.0 {
                m.serialize_entry(n, d)?;
            }
            m.end()
        }
    }
}

/// Whether a call of a command only reads: its first argument is one its declaration lists. `help`
/// and no argument at all read too: they answer from the file's comment without running it.
pub fn is_read_only(decl: Option<&CommandDecl>, args: &[String]) -> bool {
    match args.first().map(String::as_str) {
        None | Some("help") => true,
        Some(a) => decl.is_some_and(|d| d.read_only.iter().any(|r| r == a)),
    }
}

/// A command's usage: the `//` comment block its file opens with, markers taken off. `None` when
/// it opens with none.
pub fn command_help(source: &str) -> Option<String> {
    let mut lines: Vec<String> = vec![];
    for l in source.lines() {
        let t = l.trim_start();
        if let Some(rest) = t.strip_prefix("//") {
            lines.push(rest.strip_prefix(' ').unwrap_or(rest).trim_end().to_string());
        } else if t.is_empty() && lines.is_empty() {
            continue;
        } else {
            break;
        }
    }
    let text = lines.join("\n").trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Names a bundle keeps for itself: `help` and `status` are the address's own, so no flow, command
/// or station may take them.
pub const RESERVED: [&str; 2] = ["help", "status"];

/// The problems in a bundle's one namespace: its flows' local names, its commands' names and the
/// station ids of its flows must not collide across these kinds (a station id repeats across
/// flows: they are one station at different lengths), and none is reserved. `flows` is each flow's
/// local name with its station ids. One name, one meaning: `/<bundle>:<name>` cannot be two things.
pub fn namespace_problems(commands: &[&str], flows: &[(String, Vec<String>)]) -> Vec<String> {
    let mut out = vec![];
    let mut seen: std::collections::BTreeMap<String, &'static str> = Default::default();
    let mut claim = |name: &str, kind: &'static str, out: &mut Vec<String>| {
        if RESERVED.contains(&name) {
            out.push(format!("{kind} {name:?}: `{name}` is reserved"));
        }
        match seen.get(name) {
            Some(&k) if k != kind => out.push(format!("{name:?} is both a {k} and a {kind}: a bundle's names are one space")),
            Some(_) if kind != "station" => out.push(format!("{kind} {name:?} is named twice")),
            _ => {}
        }
        seen.entry(name.to_string()).or_insert(kind);
    };
    for (local, _) in flows {
        claim(local, "flow", &mut out);
    }
    for c in commands {
        claim(c, "command", &mut out);
    }
    for (_, stations) in flows {
        for s in stations {
            claim(s, "station", &mut out);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// A bundle's `start`: the order its flows are tried in, and the skill the main session judges by
/// when the inputs given cannot decide.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartDef {
    /// Flow stems, tried in order: the first whose inputs accept the start's is the one. Absent:
    /// the bundle's `flows`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub route: Vec<String>,
    /// A skill of the bundle (`skills/<name>/SKILL.md`) a model follows to judge the inputs the
    /// route needs, when the start did not give them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub triage: Option<String>,
}

impl StartDef {
    pub fn is_empty(&self) -> bool {
        self.route.is_empty() && self.triage.is_none()
    }
}

impl BundleMeta {
    /// Read from a `bundle.yaml`'s text (its name is not checked against a directory here).
    pub fn parse(text: &str) -> Result<Self, String> {
        serde_yaml::from_str(text).map_err(|e| format!("{BUNDLE_FILE}: {e}"))
    }

    /// The bundle's flows by stem, in order: `flows` as written, else `stems` by name.
    pub fn flow_order(&self, stems: &BTreeSet<String>) -> Vec<String> {
        if self.flows.is_empty() {
            stems.iter().filter(|s| *s != MAIN_FLOW).cloned().collect()
        } else {
            self.flows.clone()
        }
    }

    /// The stems a start naming the bundle is routed through, in order.
    pub fn route_order(&self, stems: &BTreeSet<String>) -> Vec<String> {
        if self.start.route.is_empty() {
            self.flow_order(stems)
        } else {
            self.start.route.clone()
        }
    }

    /// What is wrong with its `flows`, `start` and `pace`, given the stems of the flows it has
    /// and whether it has a skill of a name.
    pub fn start_problems(&self, stems: &BTreeSet<String>, has_skill: &dyn Fn(&str) -> bool) -> Vec<String> {
        let mut out = vec![];
        let mut seen = BTreeSet::new();
        for f in &self.flows {
            if !stems.contains(f) {
                out.push(format!("{BUNDLE_FILE}: flows names {f}, which is not a flow of the bundle"));
            }
            if !seen.insert(f) {
                out.push(format!("{BUNDLE_FILE}: flows names {f} twice"));
            }
        }
        let order = self.flow_order(stems);
        for f in &self.start.route {
            if !order.contains(f) {
                out.push(format!("{BUNDLE_FILE}: start.route names {f}, which is not one of its flows ({})", order.join(", ")));
            }
        }
        if let Some(t) = &self.start.triage {
            if !has_skill(t) {
                out.push(format!("{BUNDLE_FILE}: start.triage names the skill {t}, which the bundle does not have (skills/{t}/SKILL.md)"));
            }
        }
        out
    }
}

/// A bundle's `bundle.yaml` as packs offer it, if they offer the bundle and it reads.
pub fn meta_of(packs: &dyn crate::Packs, bundle: &str) -> Option<BundleMeta> {
    BundleMeta::parse(&packs.bundle_file(bundle, BUNDLE_FILE)?).ok()
}

/// The stems of a bundle's flows as packs offer them (`flow` for the bundle's own).
pub fn offered_stems(packs: &dyn crate::Packs, bundle: &str) -> BTreeSet<String> {
    packs
        .flows()
        .iter()
        .filter_map(|f| match split(f) {
            Some((b, s)) if b == bundle => Some(s.to_string()),
            None if f == bundle => Some(MAIN_FLOW.to_string()),
            _ => None,
        })
        .collect()
}

/// What a bundle needs of the host that runs it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BundleRequires {
    /// The oldest Conix that runs it, `>=x.y.z`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conix: Option<String>,
}

/// Names a bundle cannot have: they are the words of `conix flow export` / `import`, which a flow
/// address (`/flow:export`) is told apart from a bundle by.
pub const RESERVED_BUNDLES: [&str; 2] = ["export", "import"];

/// Whether a name may be a bundle's: `[a-z][a-z0-9-]*`, not a reserved one.
pub fn is_bundle_name(s: &str) -> bool {
    is_name(s) && !RESERVED_BUNDLES.contains(&s)
}

/// A requirement `>=x.y.z`, as the version it names. Only this form: a bundle says the oldest
/// host it runs on, nothing cleverer to disagree about.
pub fn parse_requirement(s: &str) -> Result<(u64, u64, u64), String> {
    let v = s.trim().strip_prefix(">=").ok_or_else(|| format!("requirement {s:?} must be written >=x.y.z"))?;
    parse_version(v.trim()).ok_or_else(|| format!("requirement {s:?} must be written >=x.y.z"))
}

/// `x.y.z` (a build suffix `-…` is ignored) as numbers.
pub fn parse_version(v: &str) -> Option<(u64, u64, u64)> {
    let core = v.split(['-', '+']).next()?;
    let mut it = core.split('.').map(|n| n.parse::<u64>().ok());
    let r = (it.next()??, it.next()??, it.next()??);
    it.next().is_none().then_some(r)
}

/// A bundle's name and the local name in a full name: `xforge/solid` → `("xforge", "solid")`.
pub fn split(name: &str) -> Option<(&str, &str)> {
    let (b, l) = name.split_once('/')?;
    (segment(b) && segment(l)).then_some((b, l))
}

/// A name a directory or file may be called by: no separator, not hidden, not empty.
pub(crate) fn segment(s: &str) -> bool {
    !s.is_empty() && !s.contains(['/', '\\']) && !s.starts_with('.')
}

/// The full name of a bundle's part.
pub fn qualified(bundle: &str, local: &str) -> String {
    format!("{bundle}/{local}")
}

/// The name a bundle's flow file is offered under.
pub fn flow_name(bundle: &str, stem: &str) -> String {
    if stem == MAIN_FLOW {
        bundle.to_string()
    } else {
        qualified(bundle, stem)
    }
}

/// The bundle and the file stem a flow name may be of: `b/x` → `(b, x)`, `b` → `(b, flow)`.
pub fn flow_file_of(name: &str) -> Option<(&str, &str)> {
    match split(name) {
        Some(x) => Some(x),
        None if segment(name) => Some((name, MAIN_FLOW)),
        None => None,
    }
}

/// Whether a name is a bundle's (a full name), and whether two names are one part's: the one a part
/// file declares and the one it is offered under.
pub fn names_match(declared: &str, offered: &str) -> bool {
    declared == offered || split(offered).is_some_and(|(_, local)| local == declared)
}

/// A name a part of a bundle refers to, made the bundle's (a full name is left as it is).
pub fn in_bundle(bundle: &str, name: &str) -> String {
    if name.contains('/') {
        name.to_string()
    } else {
        qualified(bundle, name)
    }
}

/// Whether a directory is a bundle.
pub fn is_bundle(dir: &Path) -> bool {
    dir.join(BUNDLE_FILE).is_file()
}

/// A bundle's `bundle.yaml`, parsed; `Err` says what is wrong with it.
pub fn meta(dir: &Path) -> Result<BundleMeta, String> {
    let text = std::fs::read_to_string(dir.join(BUNDLE_FILE)).map_err(|e| format!("{BUNDLE_FILE}: {e}"))?;
    let m: BundleMeta = serde_yaml::from_str(&text).map_err(|e| format!("{BUNDLE_FILE}: {e}"))?;
    let named = dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    if m.name != named {
        return Err(format!("{BUNDLE_FILE}: name {:?} is not the directory's ({named:?})", m.name));
    }
    if RESERVED_BUNDLES.contains(&m.name.as_str()) {
        return Err(format!("{BUNDLE_FILE}: name {:?} is reserved (it is a word of flow export / import)", m.name));
    }
    if let Some(c) = m.requires.as_ref().and_then(|r| r.conix.as_deref()) {
        parse_requirement(c).map_err(|e| format!("{BUNDLE_FILE}: requires.conix: {e}"))?;
    }
    Ok(m)
}

/// The bundles in a pack directory: its subdirectories with a `bundle.yaml`, by name.
pub fn bundles_in(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else { return vec![] };
    let mut out: Vec<(String, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && is_bundle(p))
        .filter_map(|p| {
            let n = p.file_name()?.to_str()?.to_string();
            (segment(&n) && is_name(&n) && !RESERVED_BUNDLES.contains(&n.as_str())).then_some((n, p))
        })
        .collect();
    out.sort();
    out
}

/// The bundle of that name in a pack directory.
pub fn bundle_in(dir: &Path, name: &str) -> Option<PathBuf> {
    if !segment(name) || RESERVED_BUNDLES.contains(&name) {
        return None;
    }
    let d = dir.join(name);
    is_bundle(&d).then_some(d)
}

/// `[a-z][a-z0-9-]*`: what a bundle and a flow file may be called.
fn is_name(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b[0].is_ascii_lowercase() && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

/// The stems of a bundle's flow files.
pub fn flow_stems(dir: &Path) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return BTreeSet::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.file_name().and_then(|n| n.to_str()) != Some(BUNDLE_FILE))
        .filter(|p| matches!(p.extension().and_then(|x| x.to_str()), Some("yaml" | "yml")))
        .filter_map(|p| p.file_stem().and_then(|s| s.to_str()).map(String::from))
        .filter(|n| is_name(n))
        .collect()
}

/// The full names of a bundle's flows.
pub fn flow_names(bundle: &str, dir: &Path) -> BTreeSet<String> {
    flow_stems(dir).iter().map(|s| flow_name(bundle, s)).collect()
}

/// The text of a bundle's flow file.
pub fn flow_text(dir: &Path, stem: &str) -> Option<String> {
    if !segment(stem) {
        return None;
    }
    ["yaml", "yml"].iter().find_map(|ext| std::fs::read_to_string(dir.join(format!("{stem}.{ext}"))).ok())
}

/// A bundle's flow, read: the definition under its full name with every name it says made the
/// bundle's, and the text it was read from. `None` when it is not there, does not parse, or says
/// another name than its file's.
pub fn read_flow(bundle: &str, dir: &Path, stem: &str) -> Option<(FlowDef, String)> {
    let text = flow_text(dir, stem)?;
    let mut def = FlowDef::parse(&text).ok()?;
    let own = if stem == MAIN_FLOW { bundle } else { stem };
    if def.name != own {
        return None;
    }
    let stems = flow_stems(dir);
    qualify(&mut def, bundle, &|s: &str| stems.contains(s));
    for st in def.stations.iter_mut().filter(|s| !s.deliverables.is_empty()) {
        let gate = qualified(bundle, &format!("{DELIVERABLES_GATE}{stem}.{}", st.id));
        st.exit.insert(0, ExitCond::Gate { gate, label: Some("deliverables".into()), rework_to: None });
    }
    Some((def, text))
}

/// The gate made from a station's deliverables (`deliverables@<flow file>.<station>`), as a part's
/// text: each file there, not empty, its template's mark gone.
fn deliverables_gate(dir: &Path, local: &str) -> Option<String> {
    let (stem, station) = local.strip_prefix(DELIVERABLES_GATE)?.split_once('.')?;
    let def = FlowDef::parse(&flow_text(dir, stem)?).ok()?;
    let st = def.stations.iter().find(|s| s.id == station)?;
    let paths: Vec<String> = st.deliverables.iter().map(|d| format!("${{output_dir}}/{}", d.path)).collect();
    if paths.is_empty() {
        return None;
    }
    let g = serde_json::json!({
        "name": local, "title": "交付物都写好了", "kind": "check", "check": "files",
        "description": "这一站声明的每个交付物都在、不空，骨架的标记已去掉。",
        "paths": paths, "not_containing": [TEMPLATE_MARK],
    });
    serde_yaml::to_string(&g).ok()
}

/// Whether a part's local name is one made, not written: a station's deliverables gate.
pub fn is_made(kind: PartKind, local: &str) -> bool {
    kind == PartKind::Gate && local.starts_with(DELIVERABLES_GATE)
}

/// Make every name a bundle's flow says the bundle's: its own name, skills, rules, policies, gates,
/// and the flows of its subflow stations (a file of the bundle by its stem, or the bundle itself).
pub fn qualify(def: &mut FlowDef, bundle: &str, has_flow: &dyn Fn(&str) -> bool) {
    let name = std::mem::take(&mut def.name);
    def.name = if name == bundle { bundle.to_string() } else { flow_name(bundle, &name) };
    def.bundle = Some(bundle.to_string());
    let q = |v: &mut Vec<String>| v.iter_mut().for_each(|n| *n = in_bundle(bundle, n));
    q(&mut def.rules);
    q(&mut def.policies);
    for s in &mut def.stations {
        q(&mut s.rules);
        q(&mut s.policies);
        if let Some(k) = &mut s.skill {
            *k = in_bundle(bundle, k);
        }
        for t in s.tasks.iter_mut().flatten() {
            if let Some(k) = &mut t.skill {
                *k = in_bundle(bundle, k);
            }
            if let Executor::Subflow { flow, .. } = &mut t.executor {
                *flow = subflow(bundle, flow, has_flow);
            }
        }
        if let Some(Executor::Subflow { flow, .. }) = &mut s.executor {
            *flow = subflow(bundle, flow, has_flow);
        }
        for e in s.exit.iter_mut().chain(s.task_exit.iter_mut()) {
            if let ExitCond::Gate { gate, .. } = e {
                *gate = in_bundle(bundle, gate);
            }
        }
        if let Some(crate::kernel::def::Foreach::Script(f)) = &mut s.foreach {
            f.script = in_bundle(bundle, &f.script);
        }
    }
}

fn subflow(bundle: &str, flow: &str, has_flow: &dyn Fn(&str) -> bool) -> String {
    if flow.contains('/') {
        flow.to_string()
    } else if has_flow(flow) {
        flow_name(bundle, flow)
    } else if flow == bundle && has_flow(MAIN_FLOW) {
        bundle.to_string()
    } else {
        // Not the bundle's: left as written, so validation says which flow is missing.
        qualified(bundle, flow)
    }
}

/// A part's text in a bundle directory, by its local name; a skill's with its `local.md` after it.
pub fn part_text(dir: &Path, kind: PartKind, local: &str) -> Option<String> {
    if !segment(local) {
        return None;
    }
    if is_made(kind, local) {
        return deliverables_gate(dir, local);
    }
    let text = std::fs::read_to_string(dir.join(kind.dir()).join(kind.file(local))).ok()?;
    Some(match kind {
        PartKind::Skill => with_local(text, local_skill(dir, local).as_deref()),
        _ => text,
    })
}

/// The `local.md` of a skill in a directory laid out as a bundle (a bundle, or a project's
/// directory of that name that only adds to one).
pub fn local_skill(dir: &Path, local: &str) -> Option<String> {
    if !segment(local) {
        return None;
    }
    std::fs::read_to_string(dir.join(PartKind::Skill.dir()).join(local).join(LOCAL_SKILL))
        .ok()
        .filter(|t| !t.trim().is_empty())
}

/// A skill's text with a user's section after it.
pub fn with_local(skill: String, local: Option<&str>) -> String {
    match local {
        None => skill,
        Some(l) => format!("{}\n\n## 本项目的补充\n\n{}\n", skill.trim_end(), l.trim()),
    }
}

/// The full names of a bundle's parts of a kind.
pub fn part_names(bundle: &str, dir: &Path, kind: PartKind) -> BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(dir.join(kind.dir())) else { return BTreeSet::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_str().map(String::from)?;
            let local = match kind {
                PartKind::Skill => e.path().join("SKILL.md").is_file().then_some(n)?,
                _ => n.strip_suffix(kind.file("").as_str()).filter(|s| segment(s))?.to_string(),
            };
            Some(qualified(bundle, &local))
        })
        .collect()
}

/// A file of a bundle by its path in it (`templates/brief.md`, `commands/xspec.js`): only inside it.
pub fn file(dir: &Path, rel: &str) -> Option<String> {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.is_empty() || !parts.iter().all(|p| segment(p)) {
        return None;
    }
    std::fs::read_to_string(parts.iter().fold(dir.to_path_buf(), |p, s| p.join(s))).ok()
}

// ── imports between a bundle's scripts ───────────────────────────────────────

/// The module a script's `import` names, as a path in the bundle: `spec` is relative (`./`, `../`)
/// and taken from the directory of `from`, itself a path in the bundle (`scripts/spec-delta.js`).
/// Never a bare name (`fs`: there is no package to find it in) and never out of the bundle.
pub fn resolve_import(from: &str, spec: &str) -> Result<String, String> {
    if !(spec.starts_with("./") || spec.starts_with("../")) {
        return Err(format!(
            "cannot import {spec:?}: only the bundle's own files are imported, by a relative path (`./x.js`, `../lib/x.js`)"
        ));
    }
    let mut at: Vec<&str> = from.split('/').collect();
    at.pop();
    for seg in spec.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                if at.pop().is_none() {
                    return Err(format!("cannot import {spec:?} from {from}: it leaves the bundle"));
                }
            }
            s => at.push(s),
        }
    }
    if at.is_empty() || !at.iter().all(|p| segment(p)) {
        return Err(format!("cannot import {spec:?} from {from}: not a file of the bundle"));
    }
    Ok(at.join("/"))
}

/// The specifiers a module's text imports (`import … from "x"`, `import "x"`, `export … from "x"`).
/// A scan, not a parse: a quoted `from "x"` in a comment is found too, which only costs a file looked
/// for and not found.
pub fn imports_of(source: &str) -> Vec<String> {
    use std::sync::OnceLock;
    static RE: OnceLock<regex::Regex> = OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(r#"(?:\b(?:import|export)\b[^;"'`]*?\bfrom\s*|\bimport\s*)["']([^"'\n]+)["']"#).expect("the import pattern")
    });
    re.captures_iter(source).map(|c| c[1].to_string()).collect()
}

/// What `roots` (path in the bundle → source) import, followed to the end: path → source, the
/// roots themselves left out. `read` gives a bundle file by its path. What cannot be read or
/// resolved is left for the loader to say when the script runs.
pub fn import_closure(roots: &[(&str, &str)], read: &dyn Fn(&str) -> Option<String>) -> std::collections::BTreeMap<String, String> {
    let mut found = std::collections::BTreeMap::new();
    let known: BTreeSet<&str> = roots.iter().map(|(p, _)| *p).collect();
    let mut todo: Vec<(String, String)> = roots.iter().map(|(p, s)| (p.to_string(), s.to_string())).collect();
    while let Some((from, text)) = todo.pop() {
        for spec in imports_of(&text) {
            let Ok(path) = resolve_import(&from, &spec) else { continue };
            if known.contains(path.as_str()) || found.contains_key(&path) {
                continue;
            }
            if let Some(src) = read(&path) {
                todo.push((path.clone(), src.clone()));
                found.insert(path, src);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(files: &[(&str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        let b = d.path().join("dev");
        for (p, t) in files {
            let f = b.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, t).unwrap();
        }
        d
    }

    #[test]
    fn a_bundles_flow_is_offered_under_its_full_name_and_says_only_its_bundles_names() {
        let d = bundle(&[
            ("bundle.yaml", "name: dev\n"),
            ("solid.yaml", "name: solid\nrules: [r]\nstations:\n  - {id: a, executor: {kind: session}, skill: s, policies: [p], exit: [{kind: gate, gate: g}]}\n  - {id: b, executor: {kind: subflow, flow: quick}, exit: [{kind: children}]}\n  - {id: c, executor: {kind: subflow, flow: other/x}, exit: [{kind: children}]}\n"),
            ("quick.yaml", "name: quick\nstations: [{id: a, exit: [{kind: confirm}]}]\n"),
        ]);
        let dir = d.path().join("dev");
        assert_eq!(bundles_in(d.path()), vec![("dev".to_string(), dir.clone())]);
        assert_eq!(flow_names("dev", &dir), ["dev/quick".to_string(), "dev/solid".to_string()].into());
        let (def, _) = read_flow("dev", &dir, "solid").unwrap();
        assert_eq!(def.name, "dev/solid");
        assert_eq!(def.rules, vec!["dev/r"]);
        let a = &def.stations[0];
        assert_eq!((a.skill.as_deref(), a.policies[0].as_str()), (Some("dev/s"), "dev/p"));
        assert!(matches!(&a.exit[0], ExitCond::Gate { gate, .. } if gate == "dev/g"));
        assert!(matches!(&def.stations[1].executor, Some(Executor::Subflow { flow, .. }) if flow == "dev/quick"));
        assert!(matches!(&def.stations[2].executor, Some(Executor::Subflow { flow, .. }) if flow == "other/x"));
        assert!(read_flow("dev", &dir, "nope").is_none());
    }

    #[test]
    fn a_flow_file_says_its_own_name_and_flow_yaml_is_the_bundles() {
        let d = bundle(&[("bundle.yaml", "name: dev\n"), ("flow.yaml", "name: dev\nstations: [{id: a, exit: [{kind: confirm}]}]\n"), ("x.yaml", "name: y\nstations: []\n")]);
        let dir = d.path().join("dev");
        assert_eq!(read_flow("dev", &dir, "flow").unwrap().0.name, "dev");
        assert!(read_flow("dev", &dir, "x").is_none(), "names another flow than its file's");
        assert_eq!(flow_file_of("dev"), Some(("dev", "flow")));
        assert_eq!(flow_file_of("dev/x"), Some(("dev", "x")));
    }

    #[test]
    fn a_skill_is_read_with_the_section_a_user_added() {
        let d = bundle(&[("bundle.yaml", "name: dev\n"), ("skills/s/SKILL.md", "---\nname: s\n---\n做事。\n"), ("skills/s/local.md", "我们这里还要写英文摘要。\n")]);
        let dir = d.path().join("dev");
        let t = part_text(&dir, PartKind::Skill, "s").unwrap();
        assert!(t.contains("做事。") && t.ends_with("## 本项目的补充\n\n我们这里还要写英文摘要。\n"), "{t}");
        assert_eq!(part_names("dev", &dir, PartKind::Skill), ["dev/s".to_string()].into());
        assert!(names_match("s", "dev/s") && names_match("dev/s", "dev/s") && !names_match("t", "dev/s"));
    }

    #[test]
    fn a_bundle_cannot_take_a_reserved_name_or_an_odd_requirement() {
        let d = tempfile::tempdir().unwrap();
        for n in RESERVED_BUNDLES {
            let b = d.path().join(n);
            std::fs::create_dir_all(&b).unwrap();
            std::fs::write(b.join("bundle.yaml"), format!("name: {n}\n")).unwrap();
            assert!(meta(&b).unwrap_err().contains("reserved"));
        }
        assert!(bundles_in(d.path()).is_empty() && bundle_in(d.path(), "export").is_none());
        let b = d.path().join("dev");
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("bundle.yaml"), "name: dev\nrequires: {conix: \"^0.1\"}\n").unwrap();
        assert!(meta(&b).unwrap_err().contains(">=x.y.z"));
        std::fs::write(b.join("bundle.yaml"), "name: dev\nrequires: {conix: \">=0.1.10\"}\n").unwrap();
        assert_eq!(meta(&b).unwrap().requires.unwrap().conix.as_deref(), Some(">=0.1.10"));
        assert_eq!(parse_version("0.1.10-2026112502"), Some((0, 1, 10)));
        assert_eq!(parse_version("0.1"), None);
    }

    #[test]
    fn a_bundle_is_a_directory_with_a_bundle_yaml_named_as_it_is() {
        let d = bundle(&[("bundle.yaml", "name: other\n")]);
        assert!(meta(&d.path().join("dev")).unwrap_err().contains("not the directory's"));
        std::fs::create_dir_all(d.path().join("skills/x")).unwrap();
        assert_eq!(bundles_in(d.path()).len(), 1, "a pack's skills/ is not a bundle");
        assert_eq!(file(&d.path().join("dev"), "bundle.yaml").as_deref(), Some("name: other\n"));
        assert!(file(&d.path().join("dev"), "../dev/bundle.yaml").is_none());
    }
}
