//! Which flow a start runs, and changing it once started (in Conix: its docs/design/flows.md):
//! a start naming a bundle that has no flow of its own name is routed through the bundle's
//! `start.route` — the first flow whose inputs accept the start's — or, when the inputs given
//! cannot decide, handed to a model to judge by the bundle's `start.triage` skill. A run may
//! switch to another flow of its bundle while its first station has not passed. Also here: the
//! station an automatic rework goes back to when gates name their own, and what a station's
//! session proposes for a person to decide.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::def::{ExitCond, FlowDef, Pace, Station};
use super::engine::CreateIn;
use super::host::Packs;
use super::run::{CheckRec, CheckStatus, ProposalStatus, RoutedBy, RoutedRec, RunRecord, StationStatus};
use super::txn::M;
use super::{FlowError, Result};
use crate::bundle;

/// Where a start goes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Route {
    /// Start this flow (a full name): named, or the first of the bundle's route that accepts
    /// the inputs. `reason` says why, for the run's record and for whoever started it.
    Flow { flow: String, by: RoutedBy, reason: String },
    /// The inputs given do not decide: a model judges them by `skill` (a full name) and starts
    /// again with what it judged, or names one of `candidates`.
    Triage { bundle: String, skill: String, candidates: Vec<RouteCandidate> },
}

/// A flow a start naming its bundle may go to, as triage is shown it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RouteCandidate {
    /// Its full name.
    pub flow: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// What it is started with ([`FlowDef::inputs_schema`]).
    pub inputs: Value,
}

/// Whether more inputs could make `def` take `given`: nothing given it does not know, and each
/// value given satisfies its schema — only something required is missing.
fn only_missing(def: &FlowDef, given: &Value) -> bool {
    let empty = serde_json::Map::new();
    let given = match given {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return false,
    };
    given.iter().all(|(k, v)| {
        def.inputs
            .get(k)
            .is_some_and(|d| jsonschema::validator_for(&d.schema).is_ok_and(|s| s.is_valid(v)))
    })
}

/// Where a start of `name` with `inputs` goes, in `packs`: a flow of that name is the one; a
/// bundle's name is routed. `NotFound` for neither; `Invalid` when no flow of the bundle takes
/// the inputs and none could with more (or there is nobody to judge them), saying what each
/// accepts.
pub fn route(packs: &dyn Packs, name: &str, inputs: &Value) -> Result<Route> {
    if packs.flow(name).is_some() {
        return Ok(Route::Flow { flow: name.to_string(), by: RoutedBy::Named, reason: format!("{name} was named") });
    }
    let Some(meta) = bundle::meta_of(packs, name).filter(|_| bundle::segment(name)) else {
        return Err(FlowError::NotFound(format!("flow {name}")));
    };
    let stems = bundle::offered_stems(packs, name);
    let order = meta.route_order(&stems);
    let mut tried: Vec<(String, String)> = vec![];
    let mut candidates = vec![];
    let mut accepts: Vec<String> = vec![];
    let mut could = false;
    for stem in &order {
        let flow = bundle::flow_name(name, stem);
        let Some((def, _)) = packs.flow(&flow) else {
            tried.push((flow, "not offered".into()));
            continue;
        };
        match def.apply_inputs(inputs) {
            Ok(_) => {
                let before = if tried.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", tried.iter().map(|(f, why)| format!("{f}: {why}")).collect::<Vec<_>>().join("; "))
                };
                return Ok(Route::Flow {
                    reason: format!("{flow} is the first in {name}'s route [{}] that takes these inputs{before}", order.join(", ")),
                    flow,
                    by: RoutedBy::Route,
                });
            }
            Err(e) => {
                could |= only_missing(&def, inputs);
                candidates.push(RouteCandidate {
                    flow: flow.clone(),
                    title: def.title.clone(),
                    description: def.description.clone(),
                    inputs: def.inputs_schema(),
                });
                accepts.push(format!("{flow} takes {} ({e})", accepts_text(&def.inputs_schema())));
                tried.push((flow, e));
            }
        }
    }
    let triage = meta.start.triage.as_deref().map(|t| bundle::in_bundle(name, t)).filter(|t| packs.skill(t).is_some());
    match triage {
        Some(skill) if could => Ok(Route::Triage { bundle: name.to_string(), skill, candidates }),
        _ => {
            let how = if could {
                "give the inputs that decide, or name the flow (`<bundle>/<flow>`)"
            } else {
                "change the inputs, or name the flow (`<bundle>/<flow>`)"
            };
            Err(FlowError::Invalid(format!(
                "no flow of bundle {name} takes these inputs: {}; {how}",
                if accepts.is_empty() { "it offers none".to_string() } else { accepts.join("; ") }
            )))
        }
    }
}

/// An inputs schema in brief: each input and its schema.
fn accepts_text(schema: &Value) -> String {
    let Some(props) = schema.get("properties").and_then(Value::as_object).filter(|p| !p.is_empty()) else {
        return "no inputs".into();
    };
    props
        .iter()
        .map(|(k, v)| {
            let mut v = v.clone();
            if let Some(o) = v.as_object_mut() {
                o.remove("description");
            }
            format!("{k} {v}")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// A start as it is carried out: a bundle's name made the flow it routes to, and how that was
/// chosen recorded in `routed` (what the starter said about it kept: `triage`, and its reason
/// before the route's). A start that needs triage is refused, saying so.
pub fn resolve(packs: &dyn Packs, mut input: CreateIn) -> Result<CreateIn> {
    let given = input.routed.take();
    match route(packs, &input.flow, &input.inputs)? {
        Route::Flow { flow, by, reason } => {
            let (by, reason) = match (by, given) {
                (RoutedBy::Named, Some(g)) => (g.by, g.reason),
                (RoutedBy::Named, None) => (RoutedBy::Named, None),
                (b, g) => {
                    let triaged = g.as_ref().is_some_and(|g| g.by == RoutedBy::Triage);
                    let said = g.and_then(|g| g.reason).filter(|r| !r.trim().is_empty());
                    (
                        if triaged { RoutedBy::Triage } else { b },
                        Some(match said {
                            Some(s) => format!("{s}; {reason}"),
                            None => reason,
                        }),
                    )
                }
            };
            input.flow = flow;
            input.routed = Some(RoutedRec { by, reason });
            Ok(input)
        }
        Route::Triage { bundle, skill, candidates } => Err(FlowError::Invalid(format!(
            "the inputs given do not decide which flow of bundle {bundle} to start ({}): judge them by the skill {skill} and \
             start {bundle} again with them, or name the flow",
            candidates.iter().map(|c| format!("{} takes {}", c.flow, accepts_text(&c.inputs))).collect::<Vec<_>>().join("; ")
        ))),
    }
}

/// How a run of `def` goes: what its start says, else the flow, else its bundle, else `auto`.
pub fn pace_of(def: &FlowDef, packs: &dyn Packs, given: Option<Pace>) -> Pace {
    given
        .or(def.pace)
        .or_else(|| def.bundle.as_deref().and_then(|b| bundle::meta_of(packs, b)).and_then(|m| m.pace))
        .unwrap_or_default()
}

/// The problems of a bundle's `bundle.yaml` a flow of it is judged with: what its `flows` and
/// `start` name that the bundle does not have.
pub(crate) fn bundle_problems(def: &FlowDef, packs: &dyn Packs) -> Vec<String> {
    let Some(b) = def.bundle.as_deref() else { return vec![] };
    let Some(meta) = bundle::meta_of(packs, b) else { return vec![] };
    let stems = bundle::offered_stems(packs, b);
    meta.start_problems(&stems, &|s| packs.skill(&bundle::in_bundle(b, s)).is_some())
}

// ── switching ────────────────────────────────────────────────────────────────

/// The station a run may switch flows at: its flow's first, open, with nothing else entered.
/// Why not, said.
pub(crate) fn switch_point(rec: &RunRecord) -> std::result::Result<String, String> {
    if !rec.is_active() {
        return Err(format!("run {} is not active", rec.id));
    }
    let Some(first) = rec.def.stations.first().map(|s| s.id.clone()) else {
        return Err(format!("flow {} has no stations", rec.flow));
    };
    if !rec.stations.get(&first).is_some_and(|s| s.status.is_open()) {
        return Err(format!(
            "run {} is past its first station {first}: a flow is switched only before that passes — cancel the run and start the other flow",
            rec.id
        ));
    }
    if let Some((id, _)) = rec.stations.iter().find(|(id, s)| **id != first && s.status != StationStatus::Pending) {
        return Err(format!("run {}'s station {id} has been entered: a flow is switched only while its first station is the one open", rec.id));
    }
    Ok(first)
}

/// The full name `to` means for a run of bundle `bundle`: as written when it is a full name,
/// else that bundle's flow of that stem.
pub(crate) fn switch_target(bundle: &str, to: &str) -> String {
    if to.contains('/') || to == bundle {
        to.to_string()
    } else {
        bundle::flow_name(bundle, to)
    }
}

/// What a switch to `to` is started with: the run's inputs the new flow knows, `over` on them.
pub(crate) fn switch_inputs(to: &FlowDef, had: &Value, over: Option<&Value>) -> std::result::Result<Value, String> {
    let mut m = serde_json::Map::new();
    if let Some(o) = had.as_object() {
        for (k, v) in o.iter().filter(|(k, _)| to.inputs.contains_key(*k)) {
            m.insert(k.clone(), v.clone());
        }
    }
    match over {
        None | Some(Value::Null) => {}
        Some(Value::Object(o)) => m.extend(o.clone()),
        Some(_) => return Err("inputs must be an object".into()),
    }
    to.apply_inputs(&Value::Object(m))
}

// ── rework points ────────────────────────────────────────────────────────────

/// Where `on_unmet` sends a station left unmet: each failed check's point — a gate's own
/// `rework_to`, else the station's `on_unmet.rework_to` — and of those the earliest station of
/// the flow, since reworking it runs the later ones again too.
pub(crate) fn unmet_target(def: &FlowDef, st: &Station, unmet: &[CheckRec], default: &str) -> String {
    let points = unmet.iter().filter(|c| matches!(c.status, CheckStatus::Fail | CheckStatus::Error)).map(|c| {
        match c.cond.and_then(|i| st.exit.get(i)) {
            Some(ExitCond::Gate { rework_to: Some(r), .. }) => r.as_str(),
            _ => default,
        }
    });
    let index = |id: &str| def.stations.iter().position(|s| s.id == id).unwrap_or(usize::MAX);
    points.min_by_key(|p| index(p)).unwrap_or(default).to_string()
}

// ── proposals ────────────────────────────────────────────────────────────────

impl M<'_> {
    /// A rework to `to` happens: open rework proposals to `to` are accepted by whoever reworked,
    /// and the others of every station it starts over (it, and those it makes stale) are
    /// dismissed — what they proposed about is being done again anyway.
    pub(crate) fn proposals_on_rework(&mut self, to: &str) {
        let mut again = self.rec.def.descendants(to);
        again.push(to.to_string());
        let by = self.by.clone();
        self.settle_proposals(|p| {
            if p.kind == super::run::ProposalKind::Rework && p.to.as_deref() == Some(to) {
                Some(ProposalStatus::Accepted)
            } else if again.contains(&p.station) {
                Some(ProposalStatus::Dismissed)
            } else {
                None
            }
        }, &by);
    }

    /// A station passed, or was entered again otherwise: what its session proposed is not taken.
    pub(crate) fn proposals_dismissed(&mut self, station: &str) {
        let by = self.by.clone();
        self.settle_proposals(|p| (p.station == station).then_some(ProposalStatus::Dismissed), &by);
    }

    /// The run is over: a cancel proposed is accepted when it was cancelled, every other is not taken.
    pub(crate) fn proposals_closed(&mut self, cancelled: bool) {
        let by = self.by.clone();
        self.settle_proposals(
            |p| Some(if cancelled && p.kind == super::run::ProposalKind::Cancel { ProposalStatus::Accepted } else { ProposalStatus::Dismissed }),
            &by,
        );
    }

    fn settle_proposals(&mut self, decide: impl Fn(&super::run::ProposalRec) -> Option<ProposalStatus>, by: &super::run::Actor) {
        let now = self.fx.now;
        let mut decided = vec![];
        for p in self.rec.proposals.iter_mut().filter(|p| p.is_open()) {
            if let Some(s) = decide(p) {
                p.status = s;
                p.decided_by = Some(by.clone());
                p.decided_at = Some(now);
                decided.push((p.id.clone(), p.station.clone(), s));
            }
        }
        for (id, station, s) in decided {
            let kind = if s == ProposalStatus::Accepted { "proposal.accepted" } else { "proposal.dismissed" };
            self.emit(kind, Some(&station), None, json!({"id": id, "by": by}));
        }
    }
}
