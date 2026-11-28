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

pub(crate) fn pace(p: harnix_flow::kernel::def::Pace) -> port::Pace {
    match p {
        harnix_flow::kernel::def::Pace::Auto => port::Pace::Auto,
        harnix_flow::kernel::def::Pace::Step => port::Pace::Step,
    }
}

/// 站里的会话提的建议。端口只有"建议打回"一种：`cancel` 的建议界面还没有位置放，先不推
/// （库的记录里还在，模型的 `flow_cancel` 与人的 `/cancel` 都照旧）。
pub(crate) fn proposal(p: &harnix_flow::kernel::run::ProposalRec) -> Option<port::ProposalView> {
    use harnix_flow::kernel::run::{ProposalKind, ProposalStatus};
    if p.kind != ProposalKind::Rework {
        return None;
    }
    Some(port::ProposalView {
        id: p.id.clone(),
        station: StationId(p.station.clone()),
        kind: port::ProposalKind::Rework,
        to: StationId(p.to.clone()?),
        reason: p.reason.clone(),
        by: p.by.id.clone(),
        status: match p.status {
            ProposalStatus::Open => port::ProposalStatus::Open,
            ProposalStatus::Accepted => port::ProposalStatus::Accepted,
            ProposalStatus::Dismissed => port::ProposalStatus::Dismissed,
        },
    })
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
            conditional: vec![],
        })
        .collect()
}

/// 带 `when` 的出口这一轮的判定，按出口的顺序：不要的取 `not_required`（记着读到的值），要的与还不知道的取
/// 检查记录里的 `when`。要的那条的理由本来只在没达标时经 `explain()` 进 `unmet`；放在这里，过了也看得到为什么要过它。
fn conditional(r: &harnix_flow::kernel::run::StationRec) -> Vec<port::ConditionalExit> {
    let mut out: Vec<(usize, port::ConditionalExit)> = r
        .not_required
        .iter()
        .map(|n| {
            (
                n.cond,
                port::ConditionalExit {
                    label: n.label.clone(),
                    required: Some(false),
                    why: when_read(&n.when, &n.value),
                },
            )
        })
        .collect();
    for c in &r.checks {
        let (Some(cond), Some(w)) = (c.cond, &c.when) else {
            continue;
        };
        if w.required == Some(false) || out.iter().any(|(k, _)| *k == cond) {
            continue;
        }
        out.push((
            cond,
            port::ConditionalExit {
                label: c.label.clone(),
                required: w.required,
                why: w.why.clone(),
            },
        ));
    }
    out.sort_by_key(|(k, _)| *k);
    out.into_iter().map(|(_, c)| c).collect()
}

/// `when` 读到的值，给人看：`interface-compat: breaking = false`、`migrate-plan: summary.destructive = false`。
/// 门禁与站写本地名：同一个包里的，包名前缀只是噪音。
fn when_read(w: &harnix_flow::kernel::def::ExitWhen, value: &serde_json::Value) -> String {
    let local = |n: &str| {
        harnix_flow::bundle::split(n)
            .map(|(_, l)| l.to_string())
            .unwrap_or_else(|| n.to_string())
    };
    let (src, key) = match (&w.gate, &w.fact, &w.step, &w.path) {
        (Some(g), Some(f), _, _) => (local(g), f.clone()),
        (_, _, Some(s), Some(p)) => (local(s), p.clone()),
        _ => (String::new(), String::new()),
    };
    let v = match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if src.is_empty() {
        v
    } else {
        format!("{src}: {key} = {v}")
    }
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
        bundle: rec.def.bundle.clone(),
        pace: pace(rec.pace),
        parent: rec.parent.as_ref().map(|p| RunId(p.run.clone())),
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
        // 列表摘要不带运行记录里的步调，取名字里的包；读得到记录时用的是上面那个。
        bundle: harnix_flow::bundle::split(&s.flow).map(|(b, _)| b.to_string()),
        pace: port::Pace::Auto,
        parent: s.parent.clone().map(RunId),
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
                // 一站通常只有一道确认出口；有几道时界面显示第一道。
                votes: v
                    .votes
                    .get(&s.id)
                    .and_then(|vs| vs.first())
                    .map(|c| port::VoteView {
                        approvals: c.approvals,
                        rejections: c.rejections,
                        min: c.min,
                    }),
                conditional: r.map(conditional).unwrap_or_default(),
            }
        })
        .collect();
    port::RunView {
        summary: summary_of(rec, &phase, now),
        main_session: SessionId(main),
        stations,
        def_digest: rec.def_digest.clone(),
        def_state: port::DefState::Current,
        proposals: rec.proposals.iter().filter_map(proposal).collect(),
        worktree: worktree(rec),
    }
}

/// 运行自己的 worktree：子运行借父运行的或自己分叉出来的，由流程库合回父运行，人不收尾，但也列出来
/// （`decidable` 为假）。
fn worktree(rec: &harnix_flow::RunRecord) -> Option<port::WorktreeView> {
    let w = rec.worktree.as_ref()?;
    let outcome = w.outcome.as_ref();
    Some(port::WorktreeView {
        branch: w.branch.clone(),
        path: w.path.clone(),
        outcome: outcome.map(|o| match o.kind {
            harnix_flow::WorktreeEnd::Merged => port::WorktreeEnd::Merged,
            harnix_flow::WorktreeEnd::Kept => port::WorktreeEnd::Kept,
            harnix_flow::WorktreeEnd::Discarded => port::WorktreeEnd::Discarded,
        }),
        into: outcome.and_then(|o| o.into.clone()),
        decidable: harnix_flow::worktree_may_end(rec).is_ok(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use harnix_flow::kernel::run::{ProposalKind, ProposalRec, ProposalStatus};

    fn rec(kind: ProposalKind, to: Option<&str>, status: ProposalStatus) -> ProposalRec {
        ProposalRec {
            id: "p1".into(),
            station: "apply".into(),
            round: 1,
            task: "t".into(),
            kind,
            to: to.map(String::from),
            reason: "spec is wrong".into(),
            by: harnix_flow::Actor::new("agent", "s1"),
            at: 1,
            status,
            decided_by: None,
            decided_at: None,
        }
    }

    /// 不要的出口写读到了什么，要的写为什么要；按出口的顺序，与门禁、确认的结果分开列。
    #[test]
    fn conditional_exits_say_why_they_were_or_were_not_required() {
        let r: harnix_flow::kernel::run::StationRec = serde_json::from_value(serde_json::json!({
            "status": "passed",
            "round": 1,
            "checks": [
                {"cond": 0, "kind": "gate", "label": "interface-compat", "round": 1, "status": "pass", "at": 1},
                {"cond": 1, "kind": "confirm", "label": "批准破坏性改动", "round": 1, "status": "pass", "at": 1,
                 "when": {"required": false, "why": "x", "value": false}},
                {"cond": 2, "kind": "confirm", "label": "安全评审", "round": 1, "status": "pass", "at": 1,
                 "when": {"required": true, "why": "gate impact-scan has no result", "value": null}}
            ],
            "not_required": [
                {"cond": 1, "label": "批准破坏性改动", "round": 1,
                 "when": {"gate": "xforge/interface-compat", "fact": "breaking"}, "value": false}
            ]
        }))
        .unwrap();
        let c = conditional(&r);
        assert_eq!(c.len(), 2, "{c:?}");
        assert_eq!(
            (c[0].label.as_str(), c[0].required, c[0].why.as_str()),
            (
                "批准破坏性改动",
                Some(false),
                "interface-compat: breaking = false"
            )
        );
        assert_eq!(c[1].required, Some(true));
        assert_eq!(c[1].why, "gate impact-scan has no result");
    }

    #[test]
    fn a_rework_proposal_is_shown_and_a_cancel_one_is_not() {
        let v = proposal(&rec(
            ProposalKind::Rework,
            Some("propose"),
            ProposalStatus::Open,
        ))
        .unwrap();
        assert_eq!(
            (
                v.id.as_str(),
                v.station.0.as_str(),
                v.to.0.as_str(),
                v.by.as_str()
            ),
            ("p1", "apply", "propose", "s1")
        );
        assert_eq!(v.status, port::ProposalStatus::Open);
        let done = proposal(&rec(
            ProposalKind::Rework,
            Some("propose"),
            ProposalStatus::Accepted,
        ))
        .unwrap();
        assert_eq!(done.status, port::ProposalStatus::Accepted);
        assert!(proposal(&rec(ProposalKind::Cancel, None, ProposalStatus::Open)).is_none());
    }
}
