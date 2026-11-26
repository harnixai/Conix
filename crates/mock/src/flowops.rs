//! 流程包的脚本化行为（docs/design 的流程地址与站操作）：选路、站操作（§4.4）、换路、包命令的输出。
//! 都是纯函数，真实实例里由流程库决定；mock 只要 TUI 能验证它对这几种回答的处理。

use port::{
    BundleInfo, ProposalStatus, RunOp, RunStatus, RunView, StationId, StationStatus, StationView,
};

use crate::backend::{advance, apply_run_op};

pub(crate) enum Pick {
    /// 流程的全名（`包/流程`，或不在包里的流程名）。
    Flow(String),
    /// 输入定不了路，要分诊；带包名。
    Triage(String),
}

/// 脚本化的各流程接受的 `risk`：quick 只收 low，solid 收 low 与 medium，major 都收。
fn accepts(local: &str, risk: &str) -> bool {
    match local {
        "quick" => risk == "low",
        "solid" => matches!(risk, "low" | "medium"),
        _ => true,
    }
}

/// `flow` 是包名（按输入选路，选不出就分诊）、`包/流程`或不在包里的流程名。
pub(crate) fn pick(
    bundle: Option<&BundleInfo>,
    flow: &str,
    inputs: &serde_json::Value,
    all_flows: &[String],
) -> Result<Pick, String> {
    let risk = inputs.get("risk").and_then(|v| v.as_str());
    let Some(b) = bundle else {
        // 点名流程：输入不合它的 schema 就拒。
        if let (Some((_, local)), Some(r)) = (flow.split_once('/'), risk)
            && !accepts(local, r)
        {
            return Err(format!("{local} 不收 risk={r}"));
        }
        return if all_flows.iter().any(|f| f == flow) {
            Ok(Pick::Flow(flow.to_string()))
        } else {
            Err(format!("没有流程 {flow}"))
        };
    };
    match risk {
        Some(r) => b
            .flows
            .iter()
            .find(|f| accepts(f, r))
            .map(|f| Pick::Flow(format!("{}/{f}", b.name)))
            .ok_or_else(|| format!("{} 里没有流程接受 risk={r}", b.name)),
        None if b.triage => Ok(Pick::Triage(b.name.clone())),
        None => b
            .flows
            .first()
            .map(|f| Pick::Flow(format!("{}/{f}", b.name)))
            .ok_or_else(|| format!("{} 里没有流程", b.name)),
    }
}

pub(crate) fn triage_reply(bundle: &str, subject: &str) -> String {
    format!(
        "（mock 分诊）{bundle}：「{subject}」我先读一下代码、判断复杂度，再选 quick 还是 solid。"
    )
}

/// 换路的目标：流程本地名补上运行所在的包。
pub(crate) fn full_name(view: &RunView, to: &str) -> String {
    match view.summary.bundle.as_deref() {
        Some(b) if !to.contains('/') => format!("{b}/{to}"),
        _ => to.to_string(),
    }
}

fn done(s: &StationView) -> bool {
    matches!(s.status, StationStatus::Passed | StationStatus::Skipped)
}

/// 站操作（§4.4）：按这一站与运行当前站的关系做事。回做了什么 / 在等什么；不能做就回理由。
pub(crate) fn goto(view: &mut RunView, station: &StationId) -> Result<String, String> {
    let ti = view
        .stations
        .iter()
        .position(|s| &s.id == station)
        .ok_or_else(|| format!("没有站 {station}"))?;
    let Some(ci) = view.stations.iter().position(|s| !done(s)) else {
        return Err("运行已经走完了".into());
    };
    let title = view.stations[ti].title.clone();
    if ti > ci {
        let cur = &view.stations[ci];
        return Err(format!(
            "当前站是「{}」（{}），不能直接跳到「{title}」",
            cur.title,
            wait_text(cur)
        ));
    }
    if ti < ci {
        return match view.stations[ti].status {
            StationStatus::Skipped => Ok(format!("「{title}」被 when 跳过了")),
            _ => {
                apply_run_op(
                    view,
                    &RunOp::Rework {
                        to: station.clone(),
                        reason: None,
                    },
                );
                Ok(format!("已打回到「{title}」"))
            }
        };
    }
    let st = &view.stations[ti];
    match st.status {
        StationStatus::Held => {
            apply_run_op(
                view,
                &RunOp::Release {
                    station: station.clone(),
                },
            );
            Ok(format!("已放行「{title}」，开始派工"))
        }
        StationStatus::Unmet => {
            apply_run_op(
                view,
                &RunOp::Retry {
                    station: station.clone(),
                },
            );
            Ok(format!("重跑「{title}」"))
        }
        _ => Ok(format!("「{title}」{}", wait_text(st))),
    }
}

fn wait_text(st: &StationView) -> String {
    if st.exit.iter().any(|e| e == "confirm") {
        let votes = st
            .votes
            .map(|v| format!("，{}/{} 票", v.approvals, v.min))
            .unwrap_or_default();
        return format!("在等确认{votes}：/confirm 或 /reject");
    }
    match st.status {
        StationStatus::Working => format!("正在干活，还欠 {}", st.exit.join("、")),
        StationStatus::Held => "搁置着，等放行".into(),
        StationStatus::Unmet => format!("没达标：{}", st.unmet.clone().unwrap_or_default()),
        _ => "还没轮到".into(),
    }
}

/// 换路（§4.2）：只在第一站还没过时；同一个运行换钉新流程，同 id 的站保留状态。
pub(crate) fn switch(
    view: &mut RunView,
    to_full: &str,
    mut stations: Vec<StationView>,
) -> Result<String, String> {
    if view.summary.flow == to_full {
        return Err(format!("已经是 {to_full}"));
    }
    if view.stations.first().is_none_or(done) {
        return Err("第一站已经过了，不能换路；取消再开".into());
    }
    for s in &mut stations {
        s.status = StationStatus::Pending;
        s.tasks.clear();
        if let Some(old) = view.stations.iter().find(|o| o.id == s.id) {
            s.status = old.status;
        }
    }
    view.summary.flow = to_full.to_string();
    view.summary.stations_total = stations.len() as u32;
    view.stations = stations;
    for p in &mut view.proposals {
        if p.status == ProposalStatus::Open {
            p.status = ProposalStatus::Dismissed;
        }
    }
    if matches!(view.summary.status, RunStatus::Waiting { .. }) {
        view.summary.status = RunStatus::Running;
    }
    advance(view);
    Ok(format!("已换到 {to_full}"))
}

/// 包命令的输出（脚本化）。`has_run` 为假时用到运行目录的子命令报"要在运行里"。
pub(crate) fn command_text(
    bundle: Option<&BundleInfo>,
    command: &str,
    args: &[String],
    has_run: bool,
) -> Result<String, String> {
    let Some(b) = bundle else {
        return Err("没有这个包".into());
    };
    let Some(info) = b.commands.iter().find(|c| c.name == command) else {
        return Err(format!("{} 没有命令 {command}", b.name));
    };
    let sub = args.first().map(String::as_str).unwrap_or("help");
    match (command, sub) {
        (_, "help") => Ok(format!(
            "{command}: {}\n  子命令：{}",
            info.description,
            info.read_only.join(" ")
        )),
        ("xspec", "list") => Ok(
            "规格增量 3 个\n  add-oauth  proposed\n  fix-cache  applied\n  export-report  proposed"
                .into(),
        ),
        ("xledger", "list") => Ok("台账 2 条\n  Q1  answered\n  D1  open".into()),
        ("xpkg", "list") => Ok("工作包 2 个\n  pkg-api  done\n  pkg-ui  open".into()),
        ("xpkg", "plan") if !has_run => Err("要在运行里：xpkg plan 用到运行目录".into()),
        _ => Ok(format!("{command} {}: ok", args.join(" "))),
    }
}
