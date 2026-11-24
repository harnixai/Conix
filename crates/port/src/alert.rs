//! 等人处理的事（architecture §8.10）：实例里产生，经通知出口送出去，接入时作为"离开期间"摆出来。

use serde::{Deserialize, Serialize};

use crate::{JobId, Millis, NoticeLevel, RunId, SessionId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Alert {
    pub kind: AlertKind,
    pub level: NoticeLevel,
    pub title: String,
    pub text: String,
    pub at_ms: Millis,
    #[serde(default)]
    pub target: Option<AlertTarget>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    /// 有人值守的会话在等回答。
    Prompt,
    /// 运行的站等确认、搁置、未达标。
    Station,
    /// 运行结束了。
    RunEnded,
    /// 作业的一次触发失败或被护栏中止。
    JobFailed,
    /// 作业连续失败，在这台机器上停用了。
    JobDisabled,
    /// 今日无人值守的额度用完了。
    DailyBudget,
    /// 项目状态不是这台机器写的，挪进了 quarantine。
    Quarantined,
    /// `TestNotify` 发的。
    Test,
}

/// 那件事在哪：接入后浮层里 Enter 跳过去。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertTarget {
    Session(SessionId),
    Run { flow: String, run: RunId },
    Job(JobId),
}

/// `TestNotify` 的回答：每个出口发没发成。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SinkResult {
    pub name: String,
    pub ok: bool,
    #[serde(default)]
    pub detail: String,
}

/// `/doctor` 的一项检查（Agix 的 `daemon.doctor` 加上 Conix 自己的几项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthCheck {
    pub name: String,
    pub status: HealthStatus,
    #[serde(default)]
    pub summary: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthStatus {
    Ok,
    Degraded,
    Failing,
}
