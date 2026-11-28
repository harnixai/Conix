use std::collections::BTreeMap;

use port::{
    FlowDetail, FlowOrigin, FlowSummary, Goal, HistoryItem, InstanceInfo, InstanceName,
    InstanceState, Job, JobEntry, Origin, Prompt, Provenance, RunView, SessionId, SessionState,
    SessionSummary, SlashCommand, StationView, Task, TaskId, Todo, Usage,
};
use serde::{Deserialize, Serialize};

use crate::MockError;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Seed {
    pub commands: Vec<SlashCommand>,
    pub sessions: Vec<SessionSeed>,
    pub tasks: Vec<Task>,
    pub task_output: BTreeMap<TaskId, String>,
    pub flows: Vec<FlowSeed>,
    /// 流程包的目录（`ListBundles`）；不在包里的流程不进来。
    pub bundles: Vec<port::BundleInfo>,
    /// 全局流程库里有、项目没装的包（`ListLibrary`）。
    pub library: Vec<String>,
    /// 全局库里内置的包与项目装没装（`ListSamples`，帮助浮层的「样例」）。
    pub samples: Vec<port::SampleBundle>,
    pub runs: Vec<RunView>,
    pub jobs: Vec<JobSeed>,
    /// 可装的插件包；装好的不进种子，从 `nothing installed` 起步。
    pub plugin_packages: Vec<port::PluginPackage>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionSeed {
    pub summary: SessionSummary,
    #[serde(default)]
    pub history: Vec<HistoryItem>,
    #[serde(default)]
    pub goal: Option<Goal>,
    #[serde(default)]
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub prompts: Vec<Prompt>,
    #[serde(default)]
    pub running: bool,
    /// 种子里就是已关闭的（交互 §5.2「已关闭」）。
    #[serde(default)]
    pub closed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FlowSeed {
    pub summary: FlowSummary,
    #[serde(default)]
    pub stations: Vec<StationView>,
    #[serde(default)]
    pub yaml: String,
}

impl FlowSeed {
    pub fn detail(&self) -> FlowDetail {
        FlowDetail {
            summary: self.summary.clone(),
            stations: self.stations.clone(),
            yaml: self.yaml.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobSeed {
    pub job: Job,
    #[serde(default)]
    pub history: Vec<JobEntry>,
}

impl Seed {
    pub fn parse(name: &str, text: &str) -> Result<Seed, MockError> {
        let value: serde_yaml::Value = serde_yaml::from_str(text)
            .map_err(|e| MockError::Parse(name.to_string(), e.to_string()))?;
        crate::from_yaml_value(value).map_err(|e| MockError::Parse(name.to_string(), e))
    }
}

/// 长列表压力用：100 个会话、一个会话下 50 个任务、一个流程 30 次运行（场景 m19）。
pub fn busy() -> Seed {
    let provenance = |i: usize| Provenance {
        instance: InstanceName::from("busy"),
        host: "ops-01".into(),
        project: Some(format!("~/work/p{:02}", i % 7)),
        scene: if i.is_multiple_of(3) { "ops" } else { "code" }.into(),
        model: "claude-sonnet-5-5".into(),
        origin: Origin::User,
    };
    let sessions = (0..100)
        .map(|i| SessionSeed {
            summary: SessionSummary {
                id: SessionId(format!("b{i:03}")),
                title: format!("会话 {i:03}"),
                state: match i % 5 {
                    0 => SessionState::Running,
                    1 => SessionState::Waiting,
                    _ => SessionState::Idle,
                },
                waiting: u32::from(i % 5 == 1),
                usage: Usage {
                    input: 1000 * i as u64,
                    ..Usage::default()
                },
                provenance: provenance(i),
                updated_ms: 1_763_000_000_000 - i as u64 * 60_000,
                created_ms: 1_763_000_000_000 - i as u64 * 60_000,
                held_by: None,
                permission: Default::default(),
            },
            history: vec![],
            goal: None,
            todos: vec![],
            prompts: vec![],
            running: i % 5 == 0,
            closed: false,
        })
        .collect();
    let tasks = (0..50)
        .map(|i| Task {
            id: TaskId(format!("bt{i:02}")),
            owner: SessionId::from("b000"),
            kind: if i % 2 == 0 {
                port::TaskKind::Shell
            } else {
                port::TaskKind::Monitor
            },
            status: if i % 4 == 0 {
                port::TaskStatus::Done
            } else {
                port::TaskStatus::Running
            },
            label: format!("job-{i:02}.sh"),
            agent_type: None,
            agent_session: None,
            model: None,
            provider: None,
            started_ms: 1_763_000_000_000,
            ended_ms: None,
            usage: Usage::default(),
            waiting_for: vec![],
            error: None,
        })
        .collect();
    let runs = (0..30)
        .map(|i| RunView {
            summary: port::RunSummary {
                id: port::RunId(format!("br{i:02}")),
                flow: "feature-to-ship".into(),
                subject: format!("主题 {i:02}"),
                status: if i % 3 == 0 {
                    port::RunStatus::Finished
                } else {
                    port::RunStatus::Running
                },
                current: Some("实现".into()),
                stations_done: 2,
                stations_total: 5,
                started_ms: 1_763_000_000_000,
                ended_ms: None,
                usage: Usage::default(),
                bundle: None,
                pace: port::Pace::Auto,
                parent: None,
            },
            main_session: SessionId::from("b001"),
            stations: vec![],
            def_digest: String::new(),
            def_state: port::DefState::Current,
            proposals: vec![],
            worktree: None,
            warnings: vec![],
        })
        .collect();
    Seed {
        sessions,
        tasks,
        runs,
        flows: vec![FlowSeed {
            summary: FlowSummary {
                name: "feature-to-ship".into(),
                title: "需求到上线".into(),
                origin: FlowOrigin::Builtin,
                active_runs: 20,
                source: None,
                bundle: None,
            },
            stations: vec![],
            yaml: String::new(),
        }],
        ..Seed::default()
    }
}

/// `MockInstances` 列出的实例与各自加载的种子。
pub fn default_instances() -> Vec<(InstanceInfo, &'static str)> {
    let now = 1_763_280_000_000;
    vec![
        (
            InstanceInfo {
                name: "dev".into(),
                host: "ops-01".into(),
                pid: Some(41_207),
                project: Some("~/work/auth".into()),
                state: InstanceState::Running,
                started_ms: Some(now - 3 * 3_600_000 - 12 * 60_000),
                stopped_ms: None,
                version: "0.1.0".into(),
                sessions: 3,
                waiting: 2,
                jobs: 3,
                holder: None,
                restarts: 0,
            },
            "default",
        ),
        (
            InstanceInfo {
                name: "ops".into(),
                host: "ops-01".into(),
                pid: Some(9_311),
                project: Some("/srv/ops".into()),
                state: InstanceState::Running,
                started_ms: Some(now - 6 * 86_400_000),
                stopped_ms: None,
                version: "0.1.0".into(),
                sessions: 100,
                waiting: 20,
                jobs: 0,
                holder: Some(port::ClientInfo {
                    user: "me".into(),
                    pid: 7_734,
                    tty: Some("/dev/pts/3".into()),
                    ssh_from: Some("10.0.0.7".into()),
                    attached_ms: now - 2 * 3_600_000,
                    last_input_ms: now - 41 * 60_000,
                }),
                restarts: 0,
            },
            "busy",
        ),
        (
            InstanceInfo {
                name: "research".into(),
                host: "ops-01".into(),
                pid: None,
                project: Some("~/notes".into()),
                state: InstanceState::Stopped,
                started_ms: None,
                stopped_ms: Some(now - 2 * 86_400_000),
                version: "0.1.0".into(),
                sessions: 0,
                waiting: 0,
                jobs: 0,
                holder: None,
                restarts: 0,
            },
            "empty",
        ),
        (
            InstanceInfo {
                name: "old".into(),
                host: "ops-01".into(),
                pid: Some(1_402),
                project: Some("~/tmp".into()),
                state: InstanceState::Stale,
                started_ms: None,
                stopped_ms: None,
                version: "0.1.0".into(),
                sessions: 0,
                waiting: 0,
                jobs: 0,
                holder: None,
                restarts: 0,
            },
            "empty",
        ),
    ]
}
