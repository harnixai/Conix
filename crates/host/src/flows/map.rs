//! 流程库的类型 → 端口的类型。状态与库一致，"等待 / 暂停中 / 已暂停"取库算出的 `Phase`。

use harnix_flow::wire::DefRow;
use harnix_flow::{FlowDef, Phase, RunRecord, StationStatus, TaskStatus};
use port::{RunId, SessionId, StationId, TaskId};
use serde_json::Value;

pub(crate) fn usage(u: &harnix_flow::Usage) -> port::Usage {
    port::Usage {
        input: u.input_tokens,
        output: u.output_tokens,
        cache_read: u.cache_read_input_tokens,
        cache_write: u.cache_creation_input_tokens,
    }
}

pub(crate) fn origin(o: harnix_flow::Origin) -> port::FlowOrigin {
    match o {
        harnix_flow::Origin::Builtin => port::FlowOrigin::Builtin,
        // 没有全局可写层，User / Override 不会出现；万一有，它就是存下来的那一份，不是只读的。
        harnix_flow::Origin::Project
        | harnix_flow::Origin::User
        | harnix_flow::Origin::Override => port::FlowOrigin::Project,
        harnix_flow::Origin::Plugin => port::FlowOrigin::Plugin,
        // `Origin` 是 non_exhaustive：库以后加的来源先按只读的全局层显示，不让编译挡住重新拷贝。
        _ => port::FlowOrigin::Plugin,
    }
}

pub(crate) fn flow_summary(d: &DefRow, active_runs: u32) -> port::FlowSummary {
    port::FlowSummary {
        name: d.name.clone(),
        title: if d.title.is_empty() {
            d.name.clone()
        } else {
            d.title.clone()
        },
        origin: origin(d.origin),
        active_runs,
        source: d.source.clone(),
        // 流程包的归属等宿主接上流程库的包之后再填。
        bundle: None,
    }
}

fn status(s: StationStatus) -> port::StationStatus {
    match s {
        StationStatus::Pending => port::StationStatus::Pending,
        StationStatus::Held => port::StationStatus::Held,
        StationStatus::Working => port::StationStatus::Working,
        StationStatus::Unmet => port::StationStatus::Unmet,
        StationStatus::Passed => port::StationStatus::Passed,
        StationStatus::Skipped => port::StationStatus::Skipped,
        StationStatus::Stale => port::StationStatus::Stale,
    }
}

pub(crate) fn run_status(p: &Phase) -> port::RunStatus {
    match p.state.as_str() {
        "waiting" => port::RunStatus::Waiting {
            reason: p.reason.clone().unwrap_or_default(),
        },
        "pausing" => port::RunStatus::Pausing,
        "paused" => port::RunStatus::Paused,
        "finished" => port::RunStatus::Finished,
        "cancelled" => port::RunStatus::Cancelled,
        _ => port::RunStatus::Running,
    }
}

/// 给人看的执行方式：`session`、`session  code`、`agent × 2`、`confirm`……
fn work(def: &FlowDef, st: &harnix_flow::Station) -> String {
    let kind_of = |v: Value| {
        let kind = v["kind"].as_str().unwrap_or("session").to_string();
        match v["scene"].as_str().filter(|s| !s.is_empty()) {
            Some(scene) => format!("{kind}  {scene}"),
            None => kind,
        }
    };
    let _ = def;
    match (&st.executor, &st.tasks) {
        (_, Some(tasks)) if tasks.len() > 1 => {
            let first = tasks
                .first()
                .and_then(|t| serde_json::to_value(&t.executor).ok())
                .map(kind_of)
                .unwrap_or_else(|| "session".into());
            format!("{first} × {}", tasks.len())
        }
        (Some(e), _) => serde_json::to_value(e)
            .map(kind_of)
            .unwrap_or_else(|_| "session".into()),
        (None, _) if st.exit.iter().all(|c| c.kind() == "confirm") && !st.exit.is_empty() => {
            "confirm".into()
        }
        (None, _) => "session".into(),
    }
}

fn exits(st: &harnix_flow::Station) -> Vec<String> {
    st.exit
        .iter()
        .map(|c| match c {
            harnix_flow::ExitCond::Gate { gate, .. } => format!("gate: {gate}"),
            other => other.kind().to_string(),
        })
        .collect()
}

/// 定义里的各站（还没有运行时的样子：都没轮到）。
pub(crate) fn def_stations(def: &FlowDef) -> Vec<port::StationView> {
    def.stations
        .iter()
        .map(|s| port::StationView {
            id: StationId(s.id.clone()),
            title: s.title.clone(),
            needs: def.needs_of(&s.id).into_iter().map(StationId).collect(),
            work: work(def, s),
            skill: s.skill.clone(),
            exit: exits(s),
            status: port::StationStatus::Pending,
            tasks: vec![],
            elapsed_ms: 0,
            usage: port::Usage::default(),
            unmet: None,
            session: None,
            votes: None,
        })
        .collect()
}

fn task_status(t: &harnix_flow::TaskRec) -> port::TaskStatus {
    match t.status {
        TaskStatus::Ready => port::TaskStatus::Waiting,
        TaskStatus::Claimed if t.asking.is_some() => port::TaskStatus::Waiting,
        TaskStatus::Claimed => port::TaskStatus::Running,
        TaskStatus::Submitted => port::TaskStatus::Done,
        TaskStatus::Failed => port::TaskStatus::Failed,
        TaskStatus::Cancelled => port::TaskStatus::Stopped,
    }
}

fn summary_of(rec: &RunRecord, phase: &Phase, now: u64) -> port::RunSummary {
    let total = rec.def.stations.len() as u32;
    let done = rec
        .stations
        .values()
        .filter(|s| matches!(s.status, StationStatus::Passed | StationStatus::Skipped))
        .count() as u32;
    let current = phase
        .stations
        .first()
        .and_then(|id| rec.def.stations.iter().find(|s| &s.id == id))
        .map(|s| s.title.clone());
    let _ = now;
    port::RunSummary {
        id: RunId(rec.id.clone()),
        flow: rec.flow.clone(),
        subject: rec.subject.clone(),
        status: run_status(phase),
        current,
        stations_done: done,
        stations_total: total,
        started_ms: rec.created_at,
        ended_ms: rec.finished_at,
        usage: usage(&rec.usage()),
        bundle: None,
        pace: port::Pace::Auto,
    }
}

/// 读不到运行记录时的退路：库的列表摘要没有各站，站数取 0。
pub(crate) fn run_summary(s: &harnix_flow::RunSummary) -> port::RunSummary {
    port::RunSummary {
        id: RunId(s.id.clone()),
        flow: s.flow.clone(),
        subject: s.subject.clone(),
        status: s.phase.as_ref().map(run_status).unwrap_or(match s.status {
            harnix_flow::RunStatus::Active => port::RunStatus::Running,
            harnix_flow::RunStatus::Finished => port::RunStatus::Finished,
            harnix_flow::RunStatus::Cancelled => port::RunStatus::Cancelled,
        }),
        current: s.open.first().cloned(),
        stations_done: 0,
        stations_total: 0,
        started_ms: s.created_at,
        ended_ms: s.finished_at,
        usage: usage(&s.usage),
        bundle: None,
        pace: port::Pace::Auto,
    }
}

pub(crate) fn run_view(v: &harnix_flow::RunView) -> port::RunView {
    let rec = &v.record;
    let phase = v.phase.clone().unwrap_or_else(|| rec.phase());
    let now = crate::instance::now_ms();
    let main = rec.main_session().unwrap_or_default().to_string();
    let stations = rec
        .def
        .stations
        .iter()
        .map(|s| {
            let r = rec.stations.get(&s.id);
            let tasks: Vec<port::StationTask> = r
                .map(|r| {
                    r.tasks
                        .iter()
                        .map(|t| port::StationTask {
                            id: TaskId(t.id.clone()),
                            label: t.id.clone(),
                            status: task_status(t),
                            session: t
                                .session
                                .clone()
                                .filter(|x| *x != main)
                                .map(SessionId),
                        })
                        .collect()
                })
                .unwrap_or_default();
            let own = tasks.iter().find_map(|t| t.session.clone()).filter(|_| {
                matches!(&s.executor, Some(harnix_flow::Executor::Session { scene, .. }) if !scene.is_empty())
            });
            let elapsed = r
                .and_then(|r| {
                    let from = r.entered_at?;
                    Some(r.left_at.unwrap_or(now).saturating_sub(from))
                })
                .unwrap_or(0);
            let unmet = v.unmet.get(&s.id).map(|checks| {
                checks
                    .iter()
                    .map(|c| c.explain())
                    .collect::<Vec<_>>()
                    .join("; ")
            });
            port::StationView {
                id: StationId(s.id.clone()),
                title: s.title.clone(),
                needs: rec.def.needs_of(&s.id).into_iter().map(StationId).collect(),
                work: work(&rec.def, s),
                skill: s.skill.clone(),
                exit: exits(s),
                status: r
                    .map(|r| status(r.status))
                    .unwrap_or(port::StationStatus::Pending),
                tasks: if own.is_some() { vec![] } else { tasks },
                elapsed_ms: elapsed,
                usage: r.map(|r| usage(&r.usage())).unwrap_or_default(),
                unmet: unmet.filter(|u| !u.is_empty()),
                session: own,
                votes: None,
            }
        })
        .collect();
    port::RunView {
        summary: summary_of(rec, &phase, now),
        main_session: SessionId(main),
        stations,
        def_digest: rec.def_digest.clone(),
        def_state: port::DefState::Current,
        proposals: vec![],
    }
}
