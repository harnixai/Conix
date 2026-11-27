use serde::{Deserialize, Serialize};

use crate::{Millis, RunId, SessionId, StationId, TaskId, Usage};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowSummary {
    pub name: String,
    pub title: String,
    pub origin: FlowOrigin,
    #[serde(default)]
    pub active_runs: u32,
    /// 插件带来的（或覆盖了插件那份的）：插件名。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// 流程所在的流程包；不在包里的是 `None`，这时包名就是流程名（flows.md §7 地址）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
}

/// 流程包的目录项（`ListBundles`）：补全与地址解析用它认名字属于流程、命令还是站。不在包里的
/// 流程没有项，TUI 把流程名当包名。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BundleInfo {
    pub name: String,
    #[serde(default)]
    pub title: String,
    /// 包里各流程的本地名（不带包名前缀），按选路的顺序。
    #[serde(default)]
    pub flows: Vec<String>,
    #[serde(default)]
    pub commands: Vec<CommandInfo>,
    /// 包里各流程的站 id（去重，同一个站在不同长度的流程里算一个）。
    #[serde(default)]
    pub stations: Vec<StationId>,
    /// 只给包、输入定不了路时会分诊（包里有 `start.triage`）。
    #[serde(default)]
    pub triage: bool,
    /// 包定的缺省步调。
    #[serde(default)]
    pub pace: Pace,
}

/// 包的一个命令：`read_only` 是不会写的子命令（命令名后的第一个词）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandInfo {
    pub name: String,
    #[serde(default)]
    pub read_only: Vec<String>,
    #[serde(default)]
    pub description: String,
}

/// 运行的步调：`Step` 每一站进站都搁置，人用站操作一站站推。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Pace {
    #[default]
    Auto,
    Step,
}

/// 站里的会话建议（`flow_propose`）：目前只有建议打回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposalView {
    pub id: String,
    pub station: StationId,
    pub kind: ProposalKind,
    /// 建议退到哪一站。
    pub to: StationId,
    #[serde(default)]
    pub reason: String,
    /// 谁提的（会话或子 Agent 的标签）。
    #[serde(default)]
    pub by: String,
    #[serde(default)]
    pub status: ProposalStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalKind {
    Rework,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProposalStatus {
    #[default]
    Open,
    Accepted,
    Dismissed,
}

/// 确认站的票数：`approvals` 够 `min` 才过。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoteView {
    #[serde(default)]
    pub approvals: u32,
    #[serde(default)]
    pub rejections: u32,
    pub min: u32,
}

/// 流程从哪来。会话、流程、作业都是项目级的：项目里建的流程只在这个项目可见，
/// 转成全局后所有项目都能用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlowOrigin {
    /// 随 Conix 发布的：只读，每个项目都有。
    Builtin,
    /// 这个项目的流程（与内置、插件的同名时取代它们）。没有全局的流程（architecture §8.1）。
    #[serde(alias = "user")]
    Project,
    /// 插件带来的：只读，每个项目都有。
    Plugin,
}

impl FlowOrigin {
    /// 不是这个项目的、各项目共用的那些（界面上标 `◎`）。
    pub fn is_shared(self) -> bool {
        matches!(self, FlowOrigin::Builtin | FlowOrigin::Plugin)
    }
}

/// 查看定义（需求 F-08）：站的概要与原始 YAML。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlowDetail {
    pub summary: FlowSummary,
    pub stations: Vec<StationView>,
    pub yaml: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: RunId,
    pub flow: String,
    pub subject: String,
    pub status: RunStatus,
    /// 当前所在的站（标题）。
    #[serde(default)]
    pub current: Option<String>,
    #[serde(default)]
    pub stations_done: u32,
    #[serde(default)]
    pub stations_total: u32,
    #[serde(default)]
    pub started_ms: Millis,
    #[serde(default)]
    pub ended_ms: Option<Millis>,
    #[serde(default)]
    pub usage: Usage,
    /// 运行所在的流程包（`flow` 带着包名前缀时同名）；不在包里是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<String>,
    #[serde(default)]
    pub pace: Pace,
}

/// 与 Harnix flow 的运行状态一致，"等待 / 暂停中 / 已暂停"是它派生出来的。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    /// 卡在人或外部上：待审批、待确认、搁置、未达标、任务失败……
    Waiting {
        reason: String,
    },
    Pausing,
    Paused,
    Finished,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    pub summary: RunSummary,
    pub main_session: SessionId,
    pub stations: Vec<StationView>,
    /// 运行启动时钉住的定义的摘要（Harnix 的 `def_digest`），界面写它的前 7 位。
    #[serde(default)]
    pub def_digest: String,
    #[serde(default)]
    pub def_state: DefState,
    /// 站里的会话提的建议（在等的与处理过的）。
    #[serde(default)]
    pub proposals: Vec<ProposalView>,
}

/// 钉住的定义与流程现在的定义比：已结束的运行看得出它跑的是不是现在这份。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefState {
    #[default]
    Current,
    Changed,
    Deleted,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationView {
    pub id: StationId,
    pub title: String,
    #[serde(default)]
    pub needs: Vec<StationId>,
    /// `session`、`agent × 2`、`confirm`、`gate: ci` 这种给人看的执行方式。
    #[serde(default)]
    pub work: String,
    /// 这一站照哪个 skill 做（流程设计卡里显示）。
    #[serde(default)]
    pub skill: Option<String>,
    /// 出口条件，给人看的写法：`output`、`gate: ci`、`confirm`。
    #[serde(default)]
    pub exit: Vec<String>,
    pub status: StationStatus,
    #[serde(default)]
    pub tasks: Vec<StationTask>,
    #[serde(default)]
    pub elapsed_ms: Millis,
    #[serde(default)]
    pub usage: Usage,
    /// 未达标时的说明。
    #[serde(default)]
    pub unmet: Option<String>,
    /// 这一站有自己的会话（独立会话的站）；没有就在运行的主会话里做。
    #[serde(default)]
    pub session: Option<crate::SessionId>,
    /// 确认站的票数；不是确认站是 `None`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub votes: Option<VoteView>,
    /// 带 `when` 的出口这一轮怎么判的（flows.md §4.2）：不要的也列出来，审计看得到本该有谁批、为什么这次没批。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub conditional: Vec<ConditionalExit>,
}

/// 一条带 `when` 的出口这一轮的判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConditionalExit {
    pub label: String,
    /// `Some(true)` 要（读到"要"，或说不清按要算）；`Some(false)` 不要；`None` 还不知道（读的门禁还在跑）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required: Option<bool>,
    /// 判的依据：不要的是读到了什么（`interface-compat: breaking = false`），要的是为什么要。
    #[serde(default)]
    pub why: String,
}

/// 与 Harnix flow 的 `StationStatus` 一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StationStatus {
    Pending,
    Held,
    Working,
    Unmet,
    Passed,
    Skipped,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StationTask {
    pub id: TaskId,
    pub label: String,
    pub status: crate::TaskStatus,
    #[serde(default)]
    pub session: Option<SessionId>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunOp {
    /// 人确认一站：`reject` 是驳回，`reason` 是理由（驳回要写）。
    Confirm {
        station: StationId,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        reject: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Release {
        station: StationId,
    },
    Force {
        station: StationId,
        reason: String,
    },
    /// 打回到 `to`（它和后面的站重做）；`reason` 记进运行的日志，给重做的站看。
    Rework {
        to: StationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Retry {
        station: StationId,
    },
    /// 站操作（`/<包>:<站> [理由]`，flows.md §4.4）：做什么由实例按运行此刻与这一站的关系定
    /// （放行、显示在等什么、重跑、打回、拒绝），TUI 不判断。回 `Reply::Text`（做了什么 / 在等什么），
    /// 不能做就报 `PortError::Failed`（理由给人看）。
    Goto {
        station: StationId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// 换路（`/<包>:<流程> [理由]`）：`to` 是包里的流程本地名。只在第一站还没过时允许。回 `Reply::Text`。
    Switch {
        to: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Pause,
    Resume,
    Cancel,
}
