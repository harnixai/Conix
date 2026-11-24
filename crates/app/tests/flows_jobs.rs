//! 场景 m11、m12、m13：流程标签与作业标签；右栏选中即切换左边的对话区（需求 F、J）。

mod common;

use common::Harness;
use model::{FlowLower, FlowRow, JobLower};
use port::{Command, Event};
use tui::Size;

const SIZE: Size = Size::new(160, 45);

async fn flows(scenario: &str) -> Harness {
    let mut h = Harness::new(scenario, SIZE).await;
    h.tab(model::Tab::Flows).await;
    h.settle().await;
    h
}

fn flow_index(h: &Harness, name: &str) -> usize {
    h.core
        .state
        .flow_rows()
        .iter()
        .position(|r| r == &FlowRow::Flow(name.into()))
        .unwrap()
}

fn run_index(h: &Harness, run: &str) -> usize {
    h.core
        .state
        .flow_rows()
        .iter()
        .position(|r| matches!(r, FlowRow::Run(_, id) if id.as_str() == run))
        .unwrap_or_else(|| panic!("没有运行 {run}"))
}

#[tokio::test(start_paused = true)]
async fn selecting_a_flow_keeps_the_chat_and_shows_its_design_below() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.settle().await;
    assert!(h.core.state.ui.chat.is_none(), "流程定义没有会话：左边不动");
    let lower = h.lower();
    for want in [
        "收集需求",
        "代码评审 ∥ 文档评审",
        "← 等 代码评审、文档评审",
        "skill implement",
        "出口 output、gate: ci",
    ] {
        assert!(lower.contains(want), "右下缺 {want}\n{lower}");
    }
    assert!(h.text().contains("流程设计：需求到上线"), "右下框的标题");
    assert!(h.panel().is_empty(), "左上不再有流程设计");
    h.snapshot("flow-design");
    h.keys("v").await;
    assert!(h.text().contains("feature-to-ship 的定义"), "查看 YAML");
}

#[tokio::test(start_paused = true)]
async fn selecting_a_run_follows_its_main_agent_and_shows_stations() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r1")).await;
    h.settle().await;
    assert_eq!(
        h.chat_title(),
        "支付重试（主会话）",
        "左边跟着切到运行的主 Agent"
    );
    let text = h.text();
    for want in [
        "主 Agent",
        "收集需求",
        "实现",
        "后端",
        "前端",
        "← 等 代码评审、文档评审",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    h.snapshot("flow-run");
    let rows = h.core.state.flow_lower_rows();
    let backend = rows
        .iter()
        .position(|r| matches!(r, FlowLower::StationTask(st, 0) if st.as_str() == "impl"))
        .unwrap();
    h.pick_lower(backend).await;
    h.settle().await;
    assert_eq!(h.chat_title(), "后端", "站里的子 Agent 有会话：左边跟着切");
    assert!(
        h.text().contains("流程 › 支付重试 › 后端"),
        "面包屑从「流程」起"
    );
}

/// 运行与站的操作不放键：`m` 把选中的运行或站作为话头放进用户会话，操作在对话里说（交互 §5）。以前的
/// 操作键都不再发运行操作。
#[tokio::test(start_paused = true)]
async fn run_and_station_ops_are_talked_about_not_keyed() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r2")).await;
    h.settle().await;
    let sign = station_row(&h, "sign");
    h.pick_lower(sign).await;
    let before = h.calls("dev").len();
    h.keys("a c f t p x R").await;
    h.settle().await;
    assert!(
        !h.calls("dev")[before..]
            .iter()
            .any(|c| matches!(c, Command::RunOp { .. })),
        "操作键都没了"
    );
    assert!(h.core.state.ui.confirm.is_none());
    h.tab(model::Tab::Flows).await;
    h.pick_lower(station_row(&h, "sign")).await;
    h.keys("m").await;
    let text = h.core.state.ui.composer.text.clone();
    assert!(
        text.contains("的站「评审汇总」") && text.contains("r2") && text.ends_with("："),
        "话头写明运行与站：{text}"
    );
    assert!(h.core.state.chat_is_user_session(), "回到了用户会话");
    h.tab(model::Tab::Flows).await;
    h.pick_upper(run_index(&h, "r2")).await;
    h.keys("m").await;
    let text = h.core.state.ui.composer.text.clone();
    assert!(
        text.starts_with("运行「") && text.contains("（流程「需求到上线」，r2）："),
        "{text}"
    );
}

#[tokio::test(start_paused = true)]
async fn m11_run_progresses_live() {
    let mut h = flows("m11-flow-run").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r1")).await;
    h.advance(14_000).await;
    let v = h.core.state.selected_run().unwrap();
    let sign = v.stations.iter().find(|s| s.id.as_str() == "sign").unwrap();
    assert_eq!(sign.status, port::StationStatus::Held, "走到评审汇总并搁置");
    assert!(h.text().contains("搁置"));
    assert!(
        h.core
            .state
            .attention()
            .iter()
            .any(|a| matches!(a, model::Attention::Run(..)))
    );
}

#[tokio::test(start_paused = true)]
async fn m12_change_the_flow_by_talking_and_the_design_updates() {
    let mut h = flows("m12-flow-author").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.settle().await;
    assert!(!h.lower().contains("安全评审"));
    // 在用户会话里说（草稿：第一句话建出会话），右下的流程设计跟着变。
    h.keys("Ctrl+G Ctrl+G").await;
    assert_eq!(
        h.core.state.ui.focus,
        model::Focus::Composer,
        "右上 → 右下 → 输入框"
    );
    h.type_str("加一个安全评审站").await;
    h.keys("Enter").await;
    h.advance(3_000).await;
    let t = h.lower();
    assert!(t.contains("安全评审"), "流程设计跟着变\n{t}");
    assert!(t.contains("← 等 代码评审、文档评审、安全评审"));
    let talked = h.core.state.ui.chat.clone().expect("建出了会话");
    h.tab(model::Tab::Flows).await;
    h.keys("m").await;
    assert_eq!(
        h.core.state.ui.composer.text, "流程 #1「需求到上线」：",
        "m 在当前的用户会话里起个话头"
    );
    assert_eq!(h.core.state.ui.chat, Some(talked));
    h.type_str("启动一次运行，主题新功能演示").await;
    h.keys("Enter").await;
    h.advance(1_000).await;
    assert!(
        h.core.state.domain.runs["feature-to-ship"]
            .iter()
            .any(|r| r.id.as_str() == "r5"),
        "启动了运行"
    );
}

#[tokio::test(start_paused = true)]
async fn jobs_list_details_history_and_follow() {
    let mut h = Harness::new("default", SIZE).await;
    h.tab(model::Tab::Jobs).await;
    h.settle().await;
    h.pick_upper(0).await;
    h.settle().await;
    assert!(h.core.state.ui.chat.is_none(), "作业没有会话：左边不动");
    let lower = h.lower();
    assert!(
        lower.contains("执行 3 次 · 成功 2 · 失败 1"),
        "执行统计在右下\n{lower}"
    );
    let text = h.text();
    assert!(text.contains("每日巡检 的详情"));
    assert!(text.contains("触发记录"));
    assert!(text.contains("0 9 * * *"));
    h.snapshot("jobs");
    let rows = h.core.state.job_lower_rows();
    let first = rows
        .iter()
        .position(|r| matches!(r, JobLower::Entry(0)))
        .unwrap();
    h.pick_lower(first).await;
    h.settle().await;
    assert!(
        !h.chat_title().starts_with("作业："),
        "跳到那一次的会话：{}",
        h.chat_title()
    );
    let before = h.calls("dev").len();
    h.keys("Tab Space R s t e d").await;
    assert!(
        !h.calls("dev")[before..].iter().any(|c| matches!(
            c,
            Command::SetJobEnabled { .. } | Command::RunJobNow { .. } | Command::StopFiring { .. }
        )),
        "启停、立即运行、停下都不放键"
    );
    h.tab(model::Tab::Jobs).await;
    h.keys("m").await;
    assert_eq!(
        h.core.state.ui.composer.text, "作业 #1「每日巡检」：",
        "作业的操作在用户会话里说"
    );
    assert!(h.core.state.chat_is_user_session(), "回到了用户会话");
    h.tab(model::Tab::Jobs).await;
    h.keys("v").await;
    let text = h.text();
    assert!(
        text.contains("作业 #1「每日巡检」") && text.contains("触发记录（3 次）"),
        "v 看全文\n{text}"
    );
}

#[tokio::test(start_paused = true)]
async fn m13_jobs_fire_and_fail() {
    let mut h = Harness::new("m13-jobs", SIZE).await;
    h.tab(model::Tab::Jobs).await;
    h.advance(7_000).await;
    let j4 = h
        .core
        .state
        .domain
        .jobs
        .iter()
        .find(|j| j.id.as_str() == "j4")
        .unwrap();
    assert!(matches!(j4.last, Some(port::JobOutcome::Failed { .. })));
    assert!(h.core.state.attention_count(model::Tab::Jobs) >= 1);
}

fn station_row(h: &Harness, id: &str) -> usize {
    h.core
        .state
        .flow_lower_rows()
        .iter()
        .position(|r| matches!(r, FlowLower::Station(s) if s.as_str() == id))
        .unwrap_or_else(|| panic!("没有站 {id}"))
}

#[tokio::test(start_paused = true)]
async fn run_stations_are_flat_colored_and_switch_to_their_context() {
    let mut h = flows("demo").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    assert!(
        h.core
            .state
            .flow_rows()
            .iter()
            .any(|r| matches!(r, FlowRow::Run(f, _) if f == "feature-to-ship")),
        "选中流程即展开它的运行"
    );
    h.pick_upper(run_index(&h, "r1")).await;
    let p = h.panel();
    assert!(
        h.text().contains("─ 运行：支付重试 ─ │") && p.contains("✓ 收集需求 → ✓ 设计 → ● 实现"),
        "运行里的会话，面板是运行进度\n{p}"
    );
    let text = h.text();
    assert!(
        text.contains("│   ✓ 收集需求") && text.contains("│ ▾ ● 实现"),
        "站不缩进\n{text}"
    );
    h.pick_lower(station_row(&h, "design")).await;
    assert_eq!(h.chat_title(), "设计（独立会话）", "有独立会话的站切到它");
    h.pick_lower(station_row(&h, "collect")).await;
    assert_eq!(h.chat_title(), "支付重试（主会话）", "其余的站在主会话里");
    let impl_at = station_row(&h, "impl");
    h.pick_lower(impl_at).await;
    let n = h.core.state.flow_lower_rows().len();
    h.keys("Left").await;
    assert_eq!(
        h.core.state.flow_lower_rows().len(),
        n - 2,
        "← 收起站里的子 Agent"
    );
    h.keys("Right").await;
    assert_eq!(h.core.state.flow_lower_rows().len(), n);
    h.keys("Down").await;
    assert_eq!(h.chat_title(), "后端", "选中站里的子 Agent 切到它的会话");
    h.snapshot("flow-run-demo");
}

#[tokio::test(start_paused = true)]
async fn a_station_of_a_definition_opens_its_config_in_an_overlay() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    let imp = h
        .core
        .state
        .flow_lower_rows()
        .iter()
        .position(|r| matches!(r, FlowLower::DefStation(2)))
        .unwrap();
    h.pick_lower(imp).await;
    assert!(h.core.state.ui.chat.is_none(), "左边不动");
    h.keys("Enter").await;
    let text = h.text();
    assert!(text.contains("需求到上线 · 第 3 站「实现」"), "{text}");
    for want in [
        "skill",
        "implement",
        "前一站",
        "设计",
        "下一站",
        "代码评审、文档评审",
        "- id: impl",
        "tasks:",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    h.snapshot("flow-station-config");
    h.keys("Esc").await;
    assert!(h.core.state.ui.overlay.is_none());
}

#[tokio::test(start_paused = true)]
async fn global_flows_are_marked_and_flow_jobs_jump_to_their_flow() {
    let mut h = flows("default").await;
    let text = h.text();
    assert!(text.contains("◎ 需求到上线"), "全局流程标 ◎\n{text}");
    assert!(!text.contains("◎ 周报"), "项目级的不标");
    h.tab(model::Tab::Jobs).await;
    let weekly = h
        .core
        .state
        .job_rows()
        .iter()
        .position(|id| {
            h.core
                .state
                .domain
                .jobs
                .iter()
                .any(|j| &j.id == id && matches!(j.kind, port::JobKind::Flow { .. }))
        })
        .unwrap();
    h.pick_upper(weekly).await;
    let link = h
        .core
        .state
        .job_lower_rows()
        .iter()
        .position(|r| matches!(r, JobLower::Flow(_)))
        .unwrap();
    h.pick_lower(link).await;
    h.keys("Enter").await;
    h.settle().await;
    assert_eq!(h.core.state.ui.tab, model::Tab::Flows, "跳到流程标签");
    assert!(
        matches!(h.core.state.selected_flow_row(), Some(FlowRow::Flow(_))),
        "选中它用的流程"
    );
    assert!(h.core.state.ui.chat.is_none(), "跳过去左边也不动");
}

#[tokio::test(start_paused = true)]
async fn run_lower_shows_time_and_tokens_but_no_status_words() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.pick_upper(run_index(&h, "r1")).await;
    let tui::Body::Workbench(w) = h.core.frame().body else {
        panic!("不在工作台")
    };
    let lower: String = w
        .side
        .unwrap()
        .lower
        .rows
        .iter()
        .map(|l| l.text() + "\n")
        .collect();
    for word in ["已通过", "进行中", "待进入", "运行中", "完成"] {
        assert!(!lower.contains(word), "右下不写状态字：{word}\n{lower}");
    }
    assert!(
        lower.contains("7m00s, 3.0k/900"),
        "用时, 输入/输出\n{lower}"
    );
    assert!(!lower.contains("入") && !lower.contains("出 "), "{lower}");
    assert!(lower.contains("7m00s"), "{lower}");
}

/// 已结束的运行在末尾的「已结束」里，不在它的流程下面；选中它左边是主会话，标题写出归属，
/// 停不了也取消不了（交互 §5.3）。
#[tokio::test(start_paused = true)]
async fn ended_runs_sit_under_ended_and_show_where_they_came_from() {
    let mut h = flows("default").await;
    h.pick_upper(flow_index(&h, "feature-to-ship")).await;
    h.keys("Right").await;
    h.settle().await;
    let rows = h.core.state.flow_rows();
    assert!(
        !rows
            .iter()
            .any(|r| matches!(r, FlowRow::Run(_, id) if id.as_str() == "r3")),
        "已完成的运行不在流程下面：{rows:?}"
    );
    let ended = rows.iter().position(|r| r == &FlowRow::Ended).unwrap();
    h.pick_upper(ended).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "r3")).await;
    h.settle().await;
    assert_eq!(
        h.chat_title(),
        "昨天的调研",
        "左边是它的主会话（mock 里是 s3）"
    );
    let text = h.text();
    for want in [
        "登录限流  需求到上线",
        "登录限流 · 流程「需求到上线」",
        "定义 3f2a91c · 已改过",
        "运行已结束",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    h.keys("Left").await;
    h.settle().await;
    assert_eq!(
        h.core.state.selected_flow_row(),
        Some(FlowRow::Ended),
        "← 收起「已结束」，选中回到它"
    );
}

/// 护栏写在定义里；自动停用的、正在跑的在列表里看得出来；下半选中一次触发按 `m`，话头写明是哪一次；
/// 简单作业的一次触发 → 展开是那个会话的任务树（交互 §5.4）。
#[tokio::test(start_paused = true)]
async fn jobs_show_guards_and_a_firing_is_talked_about_and_unfolds_its_tree() {
    let mut h = Harness::new("default", SIZE).await;
    h.tab(model::Tab::Jobs).await;
    h.settle().await;
    let upper = h.text();
    assert!(upper.contains("自动停用：连续 3 次失败"), "{upper}");
    assert!(upper.contains("正在跑"), "{upper}");
    h.pick_upper(0).await;
    h.settle().await;
    assert!(
        h.lower()
            .contains("超时 30m · 单次 2.0M · 重叠时跳过 · 连续失败 3 次停用"),
        "{}",
        h.lower()
    );
    let first = h
        .core
        .state
        .job_lower_rows()
        .iter()
        .position(|r| matches!(r, JobLower::Entry(0)))
        .unwrap();
    h.pick_lower(first).await;
    h.settle().await;
    let n = h.core.state.job_lower_rows().len();
    h.keys("Right").await;
    h.settle().await;
    let rows = h.core.state.job_lower_rows();
    assert!(rows.len() >= n, "展开那一次");
    assert!(h.lower().contains("▾ "), "{}", h.lower());
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::SessionTasks { .. })),
        "取那个会话的任务树"
    );
    h.keys("Left").await;
    assert_eq!(h.core.state.job_lower_rows().len(), n, "收起");
    h.keys("m").await;
    let text = h.core.state.ui.composer.text.clone();
    assert!(
        text.starts_with("作业 #1「每日巡检」") && text.contains("那一次（fired_ms "),
        "{text}"
    );
}

/// 接入时实例推来"离开期间"：浮层列出每件事，Enter 跳到那里（与 ^R 同一个落点），Esc 关掉（交互 §5.5）。
#[tokio::test(start_paused = true)]
async fn the_away_summary_lists_what_happened_and_enter_goes_there() {
    let mut h = Harness::new("default", SIZE).await;
    let alert = |kind, title: &str, target| port::Alert {
        kind,
        level: port::NoticeLevel::Error,
        title: title.into(),
        text: "provider 返回 429".into(),
        at_ms: 0,
        target: Some(target),
    };
    h.core
        .handle(model::Msg::Event(Box::new(Event::AwaySummary {
            since_ms: 0,
            alerts: vec![
                alert(
                    port::AlertKind::JobFailed,
                    "job \"依赖巡检\" failed",
                    port::AlertTarget::Job(port::JobId("j4".into())),
                ),
                alert(
                    port::AlertKind::Station,
                    "run \"导出 CSV\" is waiting for you",
                    port::AlertTarget::Run {
                        flow: "feature-to-ship".into(),
                        run: port::RunId("r2".into()),
                    },
                ),
            ],
        })));
    h.settle().await;
    let text = h.text();
    assert!(text.contains("job \"依赖巡检\" failed"), "{text}");
    assert!(text.contains("run \"导出 CSV\" is waiting for you"));
    h.keys("Down Enter").await;
    h.settle().await;
    assert!(h.core.state.ui.overlay.is_none(), "Enter 关掉浮层");
    assert_eq!(h.core.state.ui.tab, model::Tab::Flows);
    assert!(
        matches!(h.core.state.selected_flow_row(), Some(FlowRow::Run(_, id)) if id.as_str() == "r2"),
        "跳到那次运行"
    );
}
