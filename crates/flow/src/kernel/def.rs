//! The flow definition: a YAML file shipped in a pack.
//!
//! The types mirror the YAML surface one to one, so the JSON Schema derived
//! from them ([`definition_schema`]) is the schema of the file an author
//! writes. A station states its work as one of `executor` (one task), `tasks`
//! (several, each with an `id`) or `foreach` + `executor` (one task per item);
//! none of them makes it a pure checkpoint. [`Station::work`] gives the
//! normalised form. Combinations the types cannot rule out are structural
//! problems reported by [`crate::validate`].

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::parts::Permission;

/// Rounds a station may run when the definition does not say.
pub const DEFAULT_REWORK_LIMIT: u32 = 10;
/// The most `rework_limit` may be.
pub const MAX_REWORK_LIMIT: u32 = 20;

/// A flow: a directed acyclic graph of stations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct FlowDef {
    /// The definition format it is written in (`DEFINITION_FORMAT`); 1 when absent. A library
    /// that reads an older format refuses it whole.
    #[serde(default = "first_format", skip_serializing_if = "is_first_format")]
    #[schemars(range(min = 1, max = 1))]
    pub format: u32,
    /// `[a-z][a-z0-9-]*`; run ids are `<name>-<n>`.
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Parameters a run is started with, by name.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub inputs: BTreeMap<String, InputDef>,
    /// How many rounds one station may run (first entry included). Default 10, at most 20.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rework_limit: Option<u32>,
    /// The run's main session — the flow's main Agent (xops-03 §1): where it goes and which
    /// model it talks to. Absent: the module's main scene in the module's space.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub main: Option<MainSession>,
    /// Rules every station works under (`rules/<name>.md`): read by its sessions, never enforced.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    /// Policies every station works under (`policies/<name>.yaml`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<String>,
    /// Permissions every station works under, written here.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<Permission>,
    /// How a call no permission speaks for is judged: `auto` (the default) lets it through inside
    /// the run's project and refuses it outside; `strict` refuses a guarded tool and asks a person
    /// about a path outside.
    #[serde(default, skip_serializing_if = "Guard::is_auto")]
    pub guard: Guard,
    /// Where a run's deliverables go: a path relative to the project (a run of no project: to its
    /// own directory) that may say `${flow}`, `${date}` (`YYYYMMDD`), `${seq}` (two digits: one
    /// more than the highest already there), `${title}` and `${inputs.<k>}`. Absent:
    /// [`DEFAULT_OUTPUT_DIR`]. A start may give its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_dir: Option<String>,
    /// Where a task of it works: `project` (the default: the project itself) or `worktree` (a
    /// worktree of the project's git repository, on a branch of the run's). A start may say
    /// otherwise; a run of no project works in its own directory whatever this says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace: Option<super::run::Workspace>,
    /// What a run of it as a child hands its parent task; absent: `{run, stations: {<id>:
    /// output}}`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<FlowResult>,
    /// What is kept once a run is over, and under what name (in Conix: docs/design/layout.md §3).
    /// A bundle's flow is archived when this is absent too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub archive: Option<ArchiveDef>,
    /// How a run of it goes from station to station: `auto` (the default) enters each as soon as
    /// it can, `step` holds every station on entry for a person to release. A bundle's flow takes
    /// its bundle's `pace` when it says none; a start may say otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pace: Option<Pace>,
    /// The bundle the flow was read from: set when it is read (`crate::bundle`), not written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    pub stations: Vec<Station>,
}

/// How a run goes from station to station.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Pace {
    /// Each station is entered as soon as its needs are done.
    #[default]
    Auto,
    /// Each station is held on entry, as `hold: true` holds one: a person releases it.
    Step,
}

impl Pace {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Step => "step",
        }
    }
}

/// What a run leaves in its bundle's archive (in Conix: docs/design/layout.md §3).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ArchiveDef {
    /// The archive's directory name: a template as `output_dir`'s; absent [`DEFAULT_ARCHIVE_NAME`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Paths or globs in the run's directory kept; absent: the stations' deliverables, or all.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keep: Vec<String>,
    /// When a run is archived; absent: when it finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<ArchiveWhen>,
}

/// When a run is archived.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ArchiveWhen {
    /// Once it finished.
    #[default]
    Finished,
    /// Once it is over, finished or cancelled.
    Always,
}

/// An archive's name when the flow says none: the date, a number, the flow's own name.
pub const DEFAULT_ARCHIVE_NAME: &str = "${date}${seq}-${flow}";

/// A file a station hands in (in Conix: docs/design/layout.md §4), in the run's directory.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Deliverable {
    /// Relative to the run's directory (`${output_dir}`).
    pub path: String,
    /// A file of the bundle's `templates/` the deliverable starts from, put there when the station
    /// is entered (not over one already there).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

/// A definition's format when it says none: the first.
pub(crate) fn first_format() -> u32 {
    1
}

pub(crate) fn is_first_format(n: &u32) -> bool {
    *n == 1
}

/// Whether this library reads a definition format: why not, said.
pub(crate) fn format_problem(n: u64, what: &str) -> Option<String> {
    let reads = crate::DEFINITION_FORMAT;
    if n > u64::from(reads) {
        Some(format!(
            "this {what} is written in definition format {n}; this library reads format {reads}: it needs a newer one"
        ))
    } else if n == 0 {
        Some(format!("format: a {what}'s definition format is a whole number from 1 (this library reads {reads})"))
    } else {
        None
    }
}

/// A definition or part text checked for its `format` before anything else is read: one written
/// for a newer library is refused as that, not as fields this one does not know.
pub(crate) fn check_format(text: &str, what: &str) -> Result<(), String> {
    let Ok(serde_yaml::Value::Mapping(m)) = serde_yaml::from_str::<serde_yaml::Value>(text) else {
        return Ok(());
    };
    match m.get("format") {
        None => Ok(()),
        Some(v) => match v.as_u64() {
            Some(n) => format_problem(n, what).map_or(Ok(()), Err),
            None => Err(format_problem(0, what).unwrap_or_default()),
        },
    }
}

/// A part's text (a policy, a gate, a hook) read, its format checked first.
pub(crate) fn parse_part<T: serde::de::DeserializeOwned>(text: &str, what: &str) -> Result<T, String> {
    check_format(text, what)?;
    serde_yaml::from_str(text).map_err(|e| e.to_string())
}

/// The reasoning efforts a session may be opened with (`effort`), least first. A sub-agent
/// (`kind: agent`) takes the host's settings: it has none of its own.
pub const EFFORTS: &[&str] = &["low", "medium", "high", "xhigh", "max"];

/// Where a run's deliverables go when neither its flow nor its start says.
pub const DEFAULT_OUTPUT_DIR: &str = "flows/${flow}-${date}${seq}";

/// How a flow's tool calls are judged where no permission speaks for them. An explicit `ask`, a
/// `confirm` and a `hold` wait for a person under both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum Guard {
    /// No approvals: a guarded tool no permission names is allowed inside the project (or
    /// worktree); a path outside it is refused, with a reason the session reads, unless an `allow`
    /// whose own `paths` cover it matched.
    #[default]
    Auto,
    /// A guarded tool no permission allows is refused; a path outside the project is asked of a
    /// person.
    Strict,
}

impl Guard {
    pub fn is_auto(&self) -> bool {
        *self == Self::Auto
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Strict => "strict",
        }
    }
}

/// The run's main session: what session tasks bound to `main` and sub-agent tasks work in.
/// The fields are a session station's placement (xops-02 §1).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct MainSession {
    /// A wrapped `flow-*` scene (its tasks hand in with `flow_submit`); the module's main
    /// scene when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    /// How hard the model reasons ([`EFFORTS`]); the host's settings when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub params: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub space: Option<String>,
}

/// A run parameter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct InputDef {
    /// A JSON Schema the value must satisfy.
    #[serde(default = "empty_object")]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
    pub schema: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

fn empty_object() -> Value {
    Value::Object(Default::default())
}

/// A station: the unit of judgement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Station {
    pub id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Stations that must have finished (passed or skipped) first. Absent = the previous
    /// station in the list; `[]` = none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub needs: Option<Vec<String>>,
    /// Checked on entry; any assertion failing skips the station.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Assert>,
    /// One task, or the task template of `foreach`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<Executor>,
    /// Several tasks, each with its own id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tasks: Option<Vec<TaskSpec>>,
    /// The list of items, one task each: a reference (`${stations.<id>.output.<list>}`), or a
    /// bundle's script that gives it (`{script: <name>}`), evaluated on entry. An item that is an
    /// object may say `after: [<id>…]`: other items' `id`s whose tasks must be done first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub foreach: Option<Foreach>,
    /// With `foreach`: how many tasks may be in flight at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_parallel: Option<u32>,
    /// With `executor`: the skill a session task works from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    /// With `executor`: how many times a failed task is dispatched again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
    /// JSON Schema every task's output must satisfy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "Record<string, unknown>"))]
    pub output: Option<Value>,
    /// All must hold for the station to pass.
    #[serde(default)]
    pub exit: Vec<ExitCond>,
    /// Gates each task's hand-in must pass before it is taken (only `gate` conditions), judged with
    /// that task's context (`${item}`, `${task}`). One that does not pass sends the hand-in back to
    /// the task to fix; the station's own `exit` is judged only over hand-ins taken.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub task_exit: Vec<ExitCond>,
    /// Stations (this one or an ancestor) a rework from here may go back to.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rework_to: Vec<String>,
    /// Rework automatically when the exit conditions fail, at most `max` times.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_unmet: Option<OnUnmet>,
    /// Stop on entry until someone releases the station.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hold: bool,
    /// Rules this station works under besides the flow's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<String>,
    /// Policies this station works under besides the flow's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policies: Vec<String>,
    /// Permissions this station works under besides the flow's.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub permissions: Vec<Permission>,
    /// Where its tasks may write inside the project: globs (references allowed; a relative one
    /// is taken under the workspace). An entry that is exactly a reference to an array
    /// (`${item.paths}`) expands to its elements, per task. A Write/Edit/NotebookEdit to a project
    /// path matching none is refused; the run's own directory is always writable. Bash is not
    /// path-checked. Absent: no narrowing.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub writes: Vec<String>,
    /// The files it hands in (in Conix): told to its sessions, and checked on exit — each there, not
    /// empty, its template's mark gone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deliverables: Vec<Deliverable>,
}

/// One task of a static `tasks` list.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct TaskSpec {
    pub id: String,
    pub executor: Executor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<Retry>,
}

impl TaskSpec {
    /// Attempts a task gets before it counts as failed.
    pub fn attempts(&self) -> u32 {
        1 + self.retry.as_ref().map_or(0, |r| r.max)
    }
}

/// Who does a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Executor {
    /// A session task, fed the protocol layer and the skill. Without a `scene` (and `session`
    /// absent or `main`): a turn of the run's main session, which the flow's `main` places — the
    /// placement fields are not written then (xops-03 §1). With one: a session of its own (`new`,
    /// the default then), or its key's; where it runs and on which model are optional — absent,
    /// the module's space and that space's instance, and the instance's model.
    Session {
        #[serde(default, skip_serializing_if = "String::is_empty")]
        scene: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<SessionBinding>,
        /// The model the session talks to (`SessionSpec.model`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// The provider of that model (`SessionSpec.provider`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        provider: Option<String>,
        /// How hard the model reasons ([`EFFORTS`]); the host's settings when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// An instance that exists already, by name. Not with `template`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        instance: Option<String>,
        /// A template's full name (`<module>.<template>`, or a profile's): the module makes sure
        /// an instance of it is ready, then opens the session there. Not with `instance`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        /// With `template`: its parameters. Values may hold references (`${run}`,
        /// `${inputs.k}`, …); `name` is the instance's name (`flow-${run}` when absent).
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        params: BTreeMap<String, String>,
        /// The space the session belongs to; the module's when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        space: Option<String>,
    },
    /// A sub-agent of the run's main session (xops-03 §2): the main Agent hands the task to an
    /// Agix sub-agent (its `Agent` tool) and hands the result in with `flow_submit`.
    Agent {
        /// A type in Agix's agent catalog (its model, tools, skills); the default agent when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent_type: Option<String>,
        /// The model the sub-agent talks to (the `Agent` tool's `model`).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        /// One line for people about what the sub-agent does.
        #[serde(default, skip_serializing_if = "String::is_empty")]
        description: String,
    },
    /// The application's own code (`Programs`).
    Program { step: String },
    /// A command of the run's bundle (`commands/<name>.js`), run by the flow with no model: what
    /// it returns is the task's output (a value that is not an object as `{result: <value>}`), a
    /// throw fails the attempt. `args` may hold references (`${item.id}`), resolved at dispatch.
    Command {
        command: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    /// Waits to be claimed from outside: a person or an external agent. `who` is an opaque
    /// label handed to the authorizer.
    External {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        who: Option<String>,
    },
    /// A child run; finishing it submits the task.
    Subflow {
        flow: String,
        #[serde(default, skip_serializing_if = "Value::is_null")]
        #[cfg_attr(feature = "ts", ts(type = "unknown"))]
        inputs: Value,
    },
}

impl Executor {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Session { .. } => "session",
            Self::Agent { .. } => "agent",
            Self::Program { .. } => "program",
            Self::Command { .. } => "command",
            Self::External { .. } => "external",
            Self::Subflow { .. } => "subflow",
        }
    }

    /// Session, agent and program tasks are dispatched (auto-claimed) by the kernel.
    pub fn is_push(&self) -> bool {
        matches!(
            self,
            Self::Session { .. } | Self::Agent { .. } | Self::Program { .. } | Self::Command { .. }
        )
    }

    /// A turn of the run's main session: a session task bound to `main`, or a sub-agent task.
    pub fn in_main(&self) -> bool {
        match self {
            Self::Session { .. } => self.binding().is_some_and(|b| b.is_main()),
            Self::Agent { .. } => true,
            _ => false,
        }
    }

    /// A session task's binding as it applies: the one written, else `main` without a scene
    /// and `new` with one.
    pub fn binding(&self) -> Option<SessionBinding> {
        match self {
            Self::Session { session, scene, .. } => Some(session.clone().unwrap_or(
                if scene.is_empty() {
                    SessionBinding::Main
                } else {
                    SessionBinding::New
                },
            )),
            _ => None,
        }
    }
}

/// Which session a session task runs in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum SessionBinding {
    /// The run's main session (xops-03 §1).
    Main,
    /// A new session per task.
    New,
    /// One session for every task of the run.
    PerRun,
    /// Tasks whose key resolves to the same string share a session.
    Key(String),
}

impl SessionBinding {
    pub fn is_new(&self) -> bool {
        matches!(self, Self::New)
    }

    pub fn is_main(&self) -> bool {
        matches!(self, Self::Main)
    }
}

/// The session key of the run's main session in `RunRecord::session_keys`: no key a station
/// writes can be it (keys resolve to what a definition writes, and `@` is not a reference).
pub const MAIN_SESSION_KEY: &str = "@main";

/// Retry policy of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Retry {
    pub max: u32,
}

/// Automatic rework when a station's exit conditions fail.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct OnUnmet {
    pub rework_to: String,
    pub max: u32,
}

/// A condition for leaving a station.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExitCond {
    /// Every task has submitted an output satisfying the station's `output` contract.
    Output {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// A gate of the pack (`gates/<name>.yaml`) passes. `rework_to`: where an automatic rework
    /// goes when this gate fails, over the station's `on_unmet.rework_to` (it counts against
    /// `on_unmet.max` all the same).
    Gate {
        gate: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rework_to: Option<String>,
    },
    /// Someone confirms. `who` is an opaque label handed to the authorizer (a role, in Conix).
    /// `via: mcp` asks an outside approval system through MCP tools instead of a person here;
    /// `timeout` (`3d`, `8h`, `90m`) makes no decision by then a failure.
    Confirm {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        who: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        via: Option<ConfirmVia>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        mcp: Option<Box<McpApproval>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<String>,
        /// Approvals needed to pass (1 when absent); each decision is one vote of the round.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        min: Option<OrSetting<u32>>,
        /// One vote per actor, a later vote replacing an earlier one (true when absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        distinct: Option<bool>,
        /// When rejections make the exit not met (`any` when absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reject: Option<OrSetting<RejectRule>>,
        /// Who may not vote: `author`, the actors that delivered in this run and whoever started it.
        /// An item may be `${settings.<path>}`, standing for a word or a list of words.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        not: Vec<String>,
        /// `true` is `not: [author]` (in Conix): an on/off a governance file can hold as it is.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        separate: Option<OrSetting<bool>>,
        /// What an approval is of: files whose digests are recorded when the exit passes, checked
        /// again before every later station passes (`deliverables` when absent).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        binds: Option<Binds>,
    },
    /// The station's child runs finished, enough of them delivering (`require`, default all).
    Children {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        require: Option<Require>,
    },
}

/// How many of a station's child runs must deliver (docs/design/flows.md §6.2, in Conix):
/// `all`, `any`, or at least a number. Short of `all`, a child that failed is no failure of the
/// station by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum Require {
    AtLeast(u32),
    Word(RequireWord),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum RequireWord {
    All,
    Any,
}

impl Require {
    /// How many of `of` must deliver.
    pub fn needed(self, of: usize) -> usize {
        match self {
            Require::Word(RequireWord::All) => of,
            Require::Word(RequireWord::Any) => 1.min(of),
            Require::AtLeast(n) => n as usize,
        }
    }

    pub fn is_all(self) -> bool {
        self == Require::Word(RequireWord::All)
    }
}

/// What a flow run as a child hands its parent task (docs/design/flows.md §6.1, in Conix):
/// `from` with its references resolved, checked against `schema`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
pub struct FlowResult {
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub from: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub schema: Option<Value>,
}

/// A field written as its value or as `${settings.<path>}` (in Conix; docs/design/flows.md §4.1):
/// read from the settings the run pinned when it started, so a governance file decides it and an
/// edit to that file mid-run does not. Only `confirm`'s counting fields take one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum OrSetting<T> {
    Value(T),
    Setting(String),
}

/// The settings path a string refers to when it is exactly one `${settings.<path>}`.
pub fn settings_ref(s: &str) -> Option<&str> {
    let inner = s.trim().strip_prefix("${")?.strip_suffix('}')?.trim();
    let path = inner.strip_prefix("settings.")?.trim();
    (!path.is_empty() && !path.contains(['$', '{', '}'])).then_some(path)
}

/// When rejections make a `confirm` exit not met: one is enough (`any`), or as many as the
/// approvals it needs (`majority`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum RejectRule {
    #[default]
    Any,
    Majority,
}

/// What a `confirm` exit's approval binds: `deliverables` (those of the station and of the
/// stations it needs, in the run's directory), or a list of paths and globs, where the word
/// `deliverables` may stand among them. `[]` binds nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum Binds {
    Word(BindsWord),
    List(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum BindsWord {
    Deliverables,
}

/// The word in a `binds` list that stands for the deliverables.
pub const BINDS_DELIVERABLES: &str = "deliverables";

/// Who decides a confirmation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(rename_all = "snake_case")]
pub enum ConfirmVia {
    /// A person, through the application.
    #[default]
    Person,
    /// An outside approval system, through MCP tools the host calls (`Approvals`).
    Mcp,
}

/// A confirmation asked of an outside approval system: `request` once, then `status` every
/// `poll` until `decision` reads approved or rejected (docs/design/flows.md §4, in Conix).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct McpApproval {
    pub server: String,
    /// Opens the request. Its arguments may say `${run}`, `${station}`, `${inputs.k}`…
    pub request: McpCall,
    /// Reads where it stands. Its arguments may also say `${request.<path>}`: what `request` gave.
    pub status: McpCall,
    pub decision: DecisionMap,
    /// Where in the status result the decider's name is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub by: Option<String>,
    /// Where in the request's or the status' result a link to the request is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    /// How often `status` is called (`5m` when absent, `30s` at least).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poll: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct McpCall {
    pub tool: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub arguments: Value,
}

/// How a status result reads as a decision: the value at `path`, one of `approved` or of
/// `rejected`; anything else is still waiting.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct DecisionMap {
    pub path: String,
    pub approved: Vec<String>,
    pub rejected: Vec<String>,
}

/// The poll interval of an MCP approval when it says none, and the shortest it may say.
pub const DEFAULT_POLL_MS: u64 = 5 * 60 * 1000;
pub const MIN_POLL_MS: u64 = 30 * 1000;

impl McpApproval {
    pub fn poll_ms(&self) -> u64 {
        self.poll.as_deref().and_then(duration_ms).unwrap_or(DEFAULT_POLL_MS).max(MIN_POLL_MS)
    }
}

/// A duration as definitions write it: `90s`, `30m`, `8h`, `3d`, or parts of them together
/// (`1h30m`); `None` for anything else, zero included.
pub fn duration_ms(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut total: u64 = 0;
    let mut num = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: u64 = num.parse().ok()?;
        num.clear();
        let unit = match c {
            's' => 1_000,
            'm' => 60_000,
            'h' => 3_600_000,
            'd' => 86_400_000,
            _ => return None,
        };
        total = total.checked_add(n.checked_mul(unit)?)?;
    }
    if !num.is_empty() {
        return None;
    }
    (total > 0).then_some(total)
}

impl ExitCond {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Output { .. } => "output",
            Self::Gate { .. } => "gate",
            Self::Confirm { .. } => "confirm",
            Self::Children { .. } => "children",
        }
    }

    /// A name for people: the `label`, or one made from the kind.
    pub fn label(&self) -> String {
        let (label, made) = match self {
            Self::Output { label } => (label, "output".to_string()),
            Self::Gate { label, gate, .. } => (label, format!("gate {gate}")),
            Self::Confirm { label, who, .. } => (
                label,
                who.as_ref()
                    .map_or("confirm".to_string(), |w| format!("confirm by {w}")),
            ),
            Self::Children { label, .. } => (label, "children finished".to_string()),
        };
        label.clone().unwrap_or(made)
    }
}

/// An assertion on a JSON value: `path` selects, exactly one of the operators compares.
///
/// Shared by `when` (evaluated against the run context: `inputs.mode`,
/// `stations.collect.output.modules`, …) and `tool` gates (against the tool's result).
/// A path that selects nothing reads as `null`. `eq: null` cannot be written — use
/// `exists: false`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct Assert {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub eq: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts", ts(type = "unknown"))]
    pub ne: Option<Value>,
    /// Element count (array, object; a non-empty string or scalar counts 1, null 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub len: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_len: Option<usize>,
    /// `true`: present and not null; `false`: absent or null.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exists: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

impl Assert {
    /// How many operators are set; a well-formed assertion has exactly one.
    pub fn op_count(&self) -> usize {
        [
            self.eq.is_some(),
            self.ne.is_some(),
            self.len.is_some(),
            self.min_len.is_some(),
            self.exists.is_some(),
        ]
        .iter()
        .filter(|b| **b)
        .count()
    }
}

/// A station's work, normalised.
#[derive(Debug, Clone, PartialEq)]
pub enum Work {
    /// A pure checkpoint: no tasks, the exit is judged on entry.
    None,
    /// One task, whose id is the station's id.
    One(TaskSpec),
    Tasks(Vec<TaskSpec>),
    Foreach {
        items: Foreach,
        max_parallel: Option<u32>,
        task: TaskSpec,
    },
}

/// Where a `foreach` station's items come from.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(untagged)]
pub enum Foreach {
    /// One reference to a list (`${stations.<id>.output.<list>}`).
    Ref(String),
    /// A script of the bundle that returns the list.
    Script(ForeachScript),
}

/// `foreach: {script: <name>}`: `scripts/<name>.js`, read-only as a gate script is, given the run
/// (`inputs`, the stations' `outputs`, `run`, `output_dir`, `data_dir`) and returning the items.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS))]
#[serde(deny_unknown_fields)]
pub struct ForeachScript {
    pub script: String,
}

impl Foreach {
    /// The script that gives the items, when a script does.
    pub fn script(&self) -> Option<&str> {
        match self {
            Self::Script(s) => Some(&s.script),
            Self::Ref(_) => None,
        }
    }
}

impl std::fmt::Display for Foreach {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Ref(r) => f.write_str(r),
            Self::Script(s) => write!(f, "script {}", s.script),
        }
    }
}

impl Work {
    /// The task specs it is made of: a `foreach`'s one template, a checkpoint's none.
    pub fn into_specs(self) -> Vec<TaskSpec> {
        match self {
            Work::None => vec![],
            Work::One(t) | Work::Foreach { task: t, .. } => vec![t],
            Work::Tasks(ts) => ts,
        }
    }
}

impl Station {
    /// The work in normalised form. When the fields are combined in a way validation
    /// rejects, `tasks` wins over `executor`.
    pub fn work(&self) -> Work {
        let one = |e: &Executor| TaskSpec {
            id: self.id.clone(),
            executor: e.clone(),
            skill: self.skill.clone(),
            retry: self.retry,
        };
        match (&self.tasks, &self.executor, &self.foreach) {
            (Some(t), _, _) => Work::Tasks(t.clone()),
            (None, Some(e), Some(items)) => Work::Foreach {
                items: items.clone(),
                max_parallel: self.max_parallel,
                task: one(e),
            },
            (None, Some(e), None) => Work::One(one(e)),
            (None, None, _) => Work::None,
        }
    }

    /// Every task spec of its work: a `foreach` station's one template, a checkpoint's none.
    pub fn task_specs(&self) -> Vec<TaskSpec> {
        self.work().into_specs()
    }

    /// The task template a task id belongs to (foreach tasks share one).
    pub fn task_spec(&self, task: &str) -> Option<TaskSpec> {
        match self.work() {
            Work::None => None,
            Work::One(t) => Some(t),
            Work::Tasks(ts) => ts.into_iter().find(|t| t.id == task),
            Work::Foreach { task: t, .. } => Some(t),
        }
    }

    /// Every station a rework from here may name: its `rework_to`, its `on_unmet.rework_to`, and
    /// its gates' `rework_to`, each once, in that order.
    pub fn rework_points(&self) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        let gates = self.exit.iter().filter_map(|e| match e {
            ExitCond::Gate { rework_to: Some(r), .. } => Some(r),
            _ => None,
        });
        for r in self.rework_to.iter().chain(self.on_unmet.as_ref().map(|u| &u.rework_to)).chain(gates) {
            if !out.contains(r) {
                out.push(r.clone());
            }
        }
        out
    }

    /// The `who` label of the first `confirm` exit, if any.
    pub fn confirm_who(&self) -> Option<String> {
        self.exit.iter().find_map(|c| match c {
            ExitCond::Confirm { who, .. } => who.clone(),
            _ => None,
        })
    }
}

impl FlowDef {
    /// Parse a definition from YAML (shape only; see [`crate::validate`] for the rest).
    pub fn parse(yaml: &str) -> Result<Self, String> {
        check_format(yaml, "flow")?;
        serde_yaml::from_str(yaml).map_err(|e| e.to_string())
    }

    /// What a run is started with, as one JSON Schema of the `inputs` object, for a form: each
    /// input a property — its schema, with its `description` and `default` — and those without a
    /// default required. Nothing else may be given.
    pub fn inputs_schema(&self) -> Value {
        let mut properties = serde_json::Map::new();
        let mut required = vec![];
        for (k, d) in &self.inputs {
            let mut p = match &d.schema {
                Value::Object(o) => o.clone(),
                _ => serde_json::Map::new(),
            };
            if !d.description.is_empty() {
                p.insert("description".into(), Value::String(d.description.clone()));
            }
            match &d.default {
                Some(v) => {
                    p.insert("default".into(), v.clone());
                }
                None => required.push(Value::String(k.clone())),
            }
            properties.insert(k.clone(), Value::Object(p));
        }
        serde_json::json!({
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false,
        })
    }

    /// `given` checked against the inputs and their defaults filled in: what a run of it is
    /// started with. Why not, said.
    pub fn apply_inputs(&self, given: &Value) -> Result<Value, String> {
        crate::kernel::txn::apply_inputs(self, given)
    }

    pub fn rework_limit(&self) -> u32 {
        self.rework_limit.unwrap_or(DEFAULT_REWORK_LIMIT)
    }

    pub fn station(&self, id: &str) -> Option<&Station> {
        self.stations.iter().find(|s| s.id == id)
    }

    /// A station's needs with the default applied (the previous station in the list).
    pub fn needs_of(&self, id: &str) -> Vec<String> {
        let Some(i) = self.stations.iter().position(|s| s.id == id) else {
            return vec![];
        };
        match &self.stations[i].needs {
            Some(n) => n.clone(),
            None if i == 0 => vec![],
            None => vec![self.stations[i - 1].id.clone()],
        }
    }

    /// Every station reachable from `id` along `needs` edges downstream, not including `id`,
    /// in definition order.
    pub fn descendants(&self, id: &str) -> Vec<String> {
        let mut found: Vec<String> = vec![id.to_string()];
        let mut changed = true;
        while changed {
            changed = false;
            for s in &self.stations {
                if found.contains(&s.id) {
                    continue;
                }
                if self.needs_of(&s.id).iter().any(|n| found.contains(n)) {
                    found.push(s.id.clone());
                    changed = true;
                }
            }
        }
        self.stations
            .iter()
            .map(|s| s.id.clone())
            .filter(|s| s != id && found.contains(s))
            .collect()
    }

    /// Whether `anc` is `id` itself or one of its ancestors.
    pub fn is_self_or_ancestor(&self, anc: &str, id: &str) -> bool {
        anc == id || self.descendants(anc).iter().any(|d| d == id)
    }
}

/// The JSON Schema of a flow definition file.
pub fn definition_schema() -> Value {
    serde_json::to_value(schemars::schema_for!(FlowDef)).unwrap_or(Value::Null)
}
