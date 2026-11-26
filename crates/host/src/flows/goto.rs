//! 站操作（`RunOp::Goto`，flows.md §4.4）：`/<包>:<站>` 不说要做什么，由这里按运行此刻与这一站的关系定。
//! 判断是纯函数 [`plan`]（吃库给的 `RunView`），执行走库已有的操作，所以授权、记录与单独的 `/release`、
//! `/rework`、`/retry` 完全一样：放行只认人，由 `ConixAuthorizer` 把关。

use harnix_flow::{CheckStatus, RunView, StationStatus, TaskStatus};

/// 对这一站要做的事。`Say` 是什么都不动、只告诉人此刻的情形。
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Plan {
    Release,
    /// 重派失败的任务（所有，不像 `/retry` 只取第一个）。
    Retry(Vec<String>),
    RerunGates,
    Rework,
    Say(String),
}

pub(crate) fn plan(v: &RunView, station: &str) -> Result<Plan, String> {
    let rec = &v.record;
    let Some(def) = rec.def.station(station) else {
        return Err(format!("no station {station} in this run's flow"));
    };
    if !rec.is_active() {
        return Err(format!("run {} is not active", rec.id));
    }
    let st = rec.stations.get(station);
    let status = st.map_or(StationStatus::Pending, |s| s.status);
    Ok(match status {
        StationStatus::Skipped => {
            let when: Vec<&str> = def.when.iter().map(|a| a.path.as_str()).collect();
            Plan::Say(format!(
                "station {station} was skipped: its `when` did not hold{}",
                if when.is_empty() {
                    String::new()
                } else {
                    format!(" (it looks at {})", when.join(", "))
                }
            ))
        }
        StationStatus::Passed => Plan::Rework,
        // 还没轮到（含被打回扫掉、等上游重新过的）：不跳。
        StationStatus::Pending | StationStatus::Stale => Plan::Say(format!(
            "station {station} is not reached yet and is not jumped to; {}",
            waiting_on(v)
        )),
        StationStatus::Held => Plan::Release,
        StationStatus::Working | StationStatus::Unmet => {
            let Some(st) = st else {
                return Err(format!("station {station} has no record"));
            };
            let failed: Vec<String> = st
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Failed)
                .map(|t| t.id.clone())
                .collect();
            if !failed.is_empty() {
                Plan::Retry(failed)
            } else if status == StationStatus::Unmet {
                Plan::RerunGates
            } else if st.tasks_settled() && confirm_waiting(v, station) {
                Plan::Say(format!(
                    "station {station} waits for a confirmation: `/confirm` or `/reject <reason>`{}",
                    votes_line(v, station)
                ))
            } else {
                Plan::Say(format!(
                    "station {station} is working: {}",
                    describe(v, station)
                ))
            }
        }
    })
}

fn confirm_waiting(v: &RunView, station: &str) -> bool {
    v.unmet.get(station).is_some_and(|cs| {
        cs.iter()
            .any(|c| c.kind == "confirm" && c.status == CheckStatus::Wait)
    })
}

fn votes_line(v: &RunView, station: &str) -> String {
    match v.votes.get(station).map(Vec::as_slice) {
        Some([first, ..]) => format!(" ({} of {} approvals)", first.approvals, first.min),
        _ => String::new(),
    }
}

/// 一站此刻的任务与还欠的出口。
fn describe(v: &RunView, station: &str) -> String {
    let mut parts = vec![];
    if let Some(st) = v.record.stations.get(station)
        && !st.tasks.is_empty()
    {
        let done = st
            .tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Submitted)
            .count();
        let running = st.tasks.iter().filter(|t| t.in_flight()).count();
        parts.push(format!(
            "{} task(s), {done} handed in, {running} running",
            st.tasks.len()
        ));
    }
    match v.unmet.get(station).filter(|c| !c.is_empty()) {
        Some(cs) => parts.push(format!(
            "exit not met: {}",
            cs.iter()
                .map(|c| format!("{} ({})", c.label, c.explain()))
                .collect::<Vec<_>>()
                .join("; ")
        )),
        None => parts.push("no exit condition is failing yet".into()),
    }
    parts.join("; ")
}

/// 现在是哪几站在等什么（拒绝跳到后面的站时说）。
fn waiting_on(v: &RunView) -> String {
    let open = v.record.open_stations();
    if open.is_empty() {
        return "no station is open right now".into();
    }
    open.iter()
        .map(|id| {
            let held = v
                .record
                .stations
                .get(id)
                .is_some_and(|s| s.status == StationStatus::Held);
            if held {
                format!("the current station {id} is held: `/release` it first")
            } else if confirm_waiting(v, id) {
                format!(
                    "the current station {id} waits for a confirmation (`/confirm` or `/reject <reason>`){}",
                    votes_line(v, id)
                )
            } else {
                format!(
                    "the current station {id} is still going: {}",
                    describe(v, id)
                )
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}
