//! 场景 m22：流程包 xforge 的地址（design/flows.md §4.3–§4.4）。全局 `/flow:<包>…`，流程上下文里
//! `/<包>:<站|流程|命令>`；站操作做什么由实例定（mock 照 §4.4 的表脚本化），这里验 TUI 发什么、显示什么。

mod common;

use common::Harness;
use model::{FlowRow, Focus};
use port::{Command, RunOp, StationId, StationStatus};
use tui::Size;

const SIZE: Size = Size::new(160, 45);

async fn xforge() -> Harness {
    let mut h = Harness::new("m22-xforge", SIZE).await;
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
        .unwrap_or_else(|| panic!("没有流程 {name}"))
}

fn run_index(h: &Harness, run: &str) -> usize {
    h.core
        .state
        .flow_rows()
        .iter()
        .position(|r| matches!(r, FlowRow::Run(_, id) if id.as_str() == run))
        .unwrap_or_else(|| panic!("没有运行 {run}"))
}

/// 展开流程、选中运行、进到它的主会话：左边的对话就是这个运行的会话（流程上下文）。
async fn open_run(h: &mut Harness, flow: &str, run: &str) {
    h.pick_upper(flow_index(h, flow)).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(h, run)).await;
    h.settle().await;
    h.keys("Enter").await;
    h.core.state.ui.composer.clear();
}

/// 不进任何运行：在「随便聊聊」里（全局的对话）。
async fn global_chat(h: &mut Harness) {
    h.open_session("随便聊聊").await;
    h.core.state.ui.focus = Focus::Composer;
    h.core.state.ui.composer.clear();
}

async fn say(h: &mut Harness, text: &str) {
    h.type_str(text).await;
    h.keys("Enter").await;
    h.settle().await;
}

fn notice(h: &Harness) -> String {
    h.core
        .state
        .ui
        .notice
        .as_ref()
        .map(|n| n.text.clone())
        .unwrap_or_default()
}

fn calls_since(h: &Harness, before: usize) -> Vec<Command> {
    h.calls("dev")[before..].to_vec()
}

fn run_ops(cmds: &[Command]) -> Vec<(String, RunOp)> {
    cmds.iter()
        .filter_map(|c| match c {
            Command::RunOp { run, op } => Some((run.as_str().to_string(), op.clone())),
            _ => None,
        })
        .collect()
}

fn station(h: &Harness, run: &str, id: &str) -> port::StationView {
    h.core
        .state
        .domain
        .run_views
        .get(&port::RunId(run.into()))
        .unwrap_or_else(|| panic!("没有运行 {run}"))
        .stations
        .iter()
        .find(|s| s.id.0 == id)
        .cloned()
        .unwrap_or_else(|| panic!("没有站 {id}"))
}

fn goto(station: &str, reason: Option<&str>) -> RunOp {
    RunOp::Goto {
        station: StationId(station.into()),
        reason: reason.map(String::from),
    }
}

#[tokio::test(start_paused = true)]
async fn the_bundle_catalog_is_fetched_at_start() {
    let h = xforge().await;
    let b = &h.core.state.domain.bundles;
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].name, "xforge");
    assert_eq!(b[0].flows, ["quick", "solid"]);
    assert!(b[0].triage);
}

#[tokio::test(start_paused = true)]
async fn global_start_by_bundle_by_flow_and_with_step() {
    let mut h = xforge().await;
    h.keys("Ctrl+N Ctrl+N Ctrl+N").await; // 回到会话标签，把焦点还给输入框
    h.core.state.ui.focus = Focus::Composer;
    let before = h.calls("dev").len();
    // 只给包，输入定得了路：风险低 → 第一个接受它的流程 quick。
    say(&mut h, r#"/flow:xforge 修 "缓存 抖动" --risk low"#).await;
    // 点名流程，步进，--title 是运行的标题。
    say(
        &mut h,
        "/flow:xforge:solid 做导出 --risk medium --step --title 导出报表二期",
    )
    .await;
    let starts: Vec<_> = calls_since(&h, before)
        .into_iter()
        .filter_map(|c| match c {
            Command::StartRun {
                flow,
                subject,
                inputs,
                step,
                session,
            } => Some((flow, subject, inputs, step, session)),
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 2, "{starts:?}");
    assert_eq!(starts[0].0, "xforge");
    assert_eq!(starts[0].1, "修 缓存 抖动", "引号里的算一个词");
    assert_eq!(starts[0].2, serde_json::json!({"risk": "low"}));
    assert!(!starts[0].3);
    assert_eq!(starts[1].0, "xforge/solid");
    assert_eq!(starts[1].1, "导出报表二期");
    assert_eq!(starts[1].2, serde_json::json!({"risk": "medium"}));
    assert!(starts[1].3, "--step");
    let runs = &h.core.state.domain.run_views;
    let quick = runs
        .values()
        .find(|v| v.summary.subject == "修 缓存 抖动")
        .expect("quick 的运行");
    assert_eq!(quick.summary.flow, "xforge/quick");
    assert_eq!(quick.summary.bundle.as_deref(), Some("xforge"));
    let solid = runs
        .values()
        .find(|v| v.summary.subject == "导出报表二期")
        .expect("solid 的运行");
    assert_eq!(solid.summary.pace, port::Pace::Step);
    assert_eq!(
        solid.stations[0].status,
        StationStatus::Held,
        "步进：第一站也搁置"
    );
    assert!(notice(&h).contains("已启动"), "{}", notice(&h));
}

#[tokio::test(start_paused = true)]
async fn a_start_without_a_subject_is_refused_locally() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    let before = h.calls("dev").len();
    say(&mut h, "/flow:xforge:solid --risk low").await;
    assert!(
        !calls_since(&h, before)
            .iter()
            .any(|c| matches!(c, Command::StartRun { .. } | Command::Submit { .. })),
        "{:?}",
        calls_since(&h, before)
    );
    assert!(notice(&h).contains("要写主题"), "{}", notice(&h));
    say(&mut h, "/flow:nosuch hi").await;
    assert!(notice(&h).contains("没有流程包"), "{}", notice(&h));
}

#[tokio::test(start_paused = true)]
async fn a_start_that_cannot_choose_a_route_triages_in_the_chat() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    let before = h.calls("dev").len();
    say(&mut h, "/flow:xforge 重构鉴权").await;
    let start = calls_since(&h, before)
        .into_iter()
        .find_map(|c| match c {
            Command::StartRun { session, .. } => Some(session),
            _ => None,
        })
        .expect("StartRun");
    assert_eq!(
        start.as_ref().map(|s| s.as_str()),
        Some("ux"),
        "分诊发到敲命令的对话"
    );
    h.advance(2_000).await;
    let text = h.text();
    assert!(text.contains("mock 分诊"), "{text}");
    assert!(notice(&h).contains("分诊"), "{}", notice(&h));
    assert!(
        h.core
            .state
            .domain
            .run_views
            .values()
            .all(|v| v.summary.subject != "重构鉴权"),
        "运行由模型选完路后自己起，不是这里"
    );
}

/// 当前站在 hold（步进）：`/xforge:apply` 放行。
#[tokio::test(start_paused = true)]
async fn in_context_the_current_held_station_is_released() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x1").await;
    assert_eq!(station(&h, "x1", "apply").status, StationStatus::Held);
    let before = h.calls("dev").len();
    say(&mut h, "/xforge:apply").await;
    let cmds = calls_since(&h, before);
    assert_eq!(run_ops(&cmds), [("x1".to_string(), goto("apply", None))]);
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::Submit { .. })),
        "本地路由，不进模型"
    );
    assert_eq!(station(&h, "x1", "apply").status, StationStatus::Working);
    assert!(notice(&h).contains("已放行"), "{}", notice(&h));
}

/// 前面已经过了的站：打回到它，理由给重做的站。
#[tokio::test(start_paused = true)]
async fn in_context_an_earlier_station_sends_the_run_back() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x1").await;
    let before = h.calls("dev").len();
    say(&mut h, "/xforge:design 接口要能取消").await;
    assert_eq!(
        run_ops(&calls_since(&h, before)),
        [("x1".to_string(), goto("design", Some("接口要能取消")))]
    );
    assert_eq!(station(&h, "x1", "design").status, StationStatus::Working);
    assert_eq!(station(&h, "x1", "apply").status, StationStatus::Stale);
    assert!(notice(&h).contains("已打回"), "{}", notice(&h));
}

/// 在后面的站：不跳，说当前站在等什么（实例拒绝，理由当错误显示）。
#[tokio::test(start_paused = true)]
async fn in_context_a_later_station_is_refused_with_the_reason() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x1").await;
    let before = h.calls("dev").len();
    say(&mut h, "/xforge:archive").await;
    assert_eq!(
        run_ops(&calls_since(&h, before)),
        [("x1".to_string(), goto("archive", None))]
    );
    let n = h.core.state.ui.notice.clone().expect("提示");
    assert!(n.error, "拒绝是错误");
    assert!(
        n.text.contains("当前站是「实施」") && n.text.contains("搁置着"),
        "{}",
        n.text
    );
    assert_eq!(
        station(&h, "x1", "apply").status,
        StationStatus::Held,
        "没动"
    );
}

#[tokio::test(start_paused = true)]
async fn in_context_a_flow_name_switches_the_route() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/quick", "x2").await;
    let before = h.calls("dev").len();
    say(&mut h, "/xforge:solid 风险比想的高").await;
    assert_eq!(
        run_ops(&calls_since(&h, before)),
        [(
            "x2".to_string(),
            RunOp::Switch {
                to: "solid".into(),
                reason: Some("风险比想的高".into()),
            }
        )]
    );
    let v = &h.core.state.domain.run_views[&port::RunId("x2".into())];
    assert_eq!(v.summary.flow, "xforge/solid");
    assert_eq!(v.stations.len(), 5);
    assert!(notice(&h).contains("已换到"), "{}", notice(&h));
    // 第一站还没过：再换回去也行；已经是的那条就拒。
    say(&mut h, "/xforge:solid").await;
    assert!(notice(&h).contains("已经是"), "{}", notice(&h));
}

/// 第一站过了就不能换路：实例拒，理由显示出来。
#[tokio::test(start_paused = true)]
async fn switching_after_the_first_station_is_refused() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x1").await;
    say(&mut h, "/xforge:quick").await;
    let n = h.core.state.ui.notice.clone().expect("提示");
    assert!(n.error && n.text.contains("第一站已经过了"), "{}", n.text);
    assert_eq!(
        h.core.state.domain.run_views[&port::RunId("x1".into())]
            .summary
            .flow,
        "xforge/solid"
    );
}

#[tokio::test(start_paused = true)]
async fn in_context_a_command_runs_locally_and_its_output_is_shown() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x1").await;
    let before = h.calls("dev").len();
    say(&mut h, "/xforge:xspec list").await;
    let cmds = calls_since(&h, before);
    assert_eq!(
        cmds,
        [Command::FlowCommand {
            bundle: "xforge".into(),
            command: "xspec".into(),
            args: vec!["list".into()],
            run: Some(port::RunId("x1".into())),
        }]
    );
    let text = h.text();
    assert!(
        text.contains("add-oauth") && text.contains("fix-cache"),
        "{text}"
    );
    assert!(
        !cmds.iter().any(|c| matches!(c, Command::Submit { .. })),
        "输出是本地的，不进模型"
    );
}

#[tokio::test(start_paused = true)]
async fn globally_a_command_needs_the_full_address() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    let before = h.calls("dev").len();
    say(&mut h, "/flow:xforge:xspec list").await;
    assert_eq!(
        calls_since(&h, before),
        [Command::FlowCommand {
            bundle: "xforge".into(),
            command: "xspec".into(),
            args: vec!["list".into()],
            run: None,
        }]
    );
    assert!(h.text().contains("add-oauth"));
    h.keys("Esc").await;
    // 用到运行目录的子命令：没有运行就报"要在运行里"。
    say(&mut h, "/flow:xforge:xpkg plan").await;
    let n = h.core.state.ui.notice.clone().expect("提示");
    assert!(n.error && n.text.contains("要在运行里"), "{}", n.text);
    // 给了 --run 就带上运行。
    let before = h.calls("dev").len();
    say(&mut h, "/flow:xforge:xpkg plan --run x1 --after a").await;
    assert_eq!(
        calls_since(&h, before),
        [Command::FlowCommand {
            bundle: "xforge".into(),
            command: "xpkg".into(),
            args: ["plan", "--after", "a"].map(String::from).to_vec(),
            run: Some(port::RunId("x1".into())),
        }]
    );
}

#[tokio::test(start_paused = true)]
async fn globally_a_station_operation_picks_the_run_it_can() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    // 包里有两个 solid 在跑（x1、x3）加一个 quick（x2）：不写运行又没有选中的，列出候选。
    let before = h.calls("dev").len();
    say(&mut h, "/flow:xforge:apply").await;
    assert!(run_ops(&calls_since(&h, before)).is_empty());
    assert!(
        notice(&h).contains("x1") && notice(&h).contains("x3"),
        "{}",
        notice(&h)
    );
    // 写运行 id：第一个词是运行，后面是理由。
    say(&mut h, "/flow:xforge:apply x1 看过了").await;
    assert_eq!(
        run_ops(&calls_since(&h, before)),
        [("x1".to_string(), goto("apply", Some("看过了")))]
    );
}

/// 全局里不认简写：交给 Agix（照旧当一句话发出去），只多一句提示。
#[tokio::test(start_paused = true)]
async fn globally_the_short_form_passes_through_with_a_hint() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    let before = h.calls("dev").len();
    say(&mut h, "/xforge:apply").await;
    let cmds = calls_since(&h, before);
    assert!(run_ops(&cmds).is_empty());
    assert!(
        cmds.iter()
            .any(|c| matches!(c, Command::Submit { text, .. } if text == "/xforge:apply")),
        "{cmds:?}"
    );
    assert!(notice(&h).contains("/flow:xforge"), "{}", notice(&h));
}

/// 流程上下文里，别的包的 `/<x>:<y>` 照旧交给 Agix。
#[tokio::test(start_paused = true)]
async fn in_context_other_prefixes_and_unknown_names_pass_through() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x1").await;
    let before = h.calls("dev").len();
    say(&mut h, "/other:thing a").await;
    say(&mut h, "/xforge:nonesuch a").await;
    let texts: Vec<String> = calls_since(&h, before)
        .into_iter()
        .filter_map(|c| match c {
            Command::Submit { text, .. } => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(texts, ["/other:thing a", "/xforge:nonesuch a"]);
}

#[tokio::test(start_paused = true)]
async fn the_bundle_and_the_run_can_be_described() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    say(&mut h, "/flow:xforge").await;
    let t = h.text();
    for want in [
        "流程：quick、solid",
        "命令：xspec、xledger、xpkg",
        "登录加 OAuth",
        "用法",
    ] {
        assert!(t.contains(want), "缺 {want}\n{t}");
    }
    h.keys("Esc").await;
    h.tab(model::Tab::Flows).await;
    open_run(&mut h, "xforge/solid", "x3").await;
    say(&mut h, "/xforge").await;
    let t = h.text();
    for want in ["当前站：检查", "1/2 票", "建议打回到「设计」"] {
        assert!(t.contains(want), "缺 {want}\n{t}");
    }
}

#[tokio::test(start_paused = true)]
async fn completion_knows_bundles_flows_commands_and_stations() {
    let mut h = xforge().await;
    global_chat(&mut h).await;
    let names = |h: &Harness| -> Vec<(String, String)> {
        h.core
            .state
            .picker_candidates()
            .into_iter()
            .map(|c| (c.name, c.source))
            .collect()
    };
    h.type_str("/flow:").await;
    assert_eq!(
        names(&h),
        [("flow:xforge".to_string(), "flow-bundle".to_string())]
    );
    h.core.state.ui.composer.clear();
    h.type_str("/flow:xforge:").await;
    let got = names(&h);
    for (n, k) in [
        ("flow:xforge:quick", "flow-flow"),
        ("flow:xforge:solid", "flow-flow"),
        ("flow:xforge:xspec", "flow-command"),
        ("flow:xforge:xledger", "flow-command"),
        ("flow:xforge:xpkg", "flow-command"),
        ("flow:xforge:spec", "flow-station"),
        ("flow:xforge:apply", "flow-station"),
    ] {
        assert!(
            got.contains(&(n.to_string(), k.to_string())),
            "缺 {n}：{got:?}"
        );
    }
    h.core.state.ui.composer.clear();
    h.type_str("/flow:xforge:x").await;
    assert_eq!(names(&h).len(), 3, "按前缀过滤");
    // 候选带种类标签画在补全列表里。
    assert!(h.text().contains("命令"), "{}", h.text());
    // 全局里 /xforge: 不补（不是流程上下文）。
    h.core.state.ui.composer.clear();
    h.type_str("/xforge:").await;
    assert!(names(&h).is_empty());

    h.tab(model::Tab::Flows).await;
    open_run(&mut h, "xforge/quick", "x2").await;
    h.type_str("/xforge:").await;
    let got = names(&h);
    for (n, k) in [
        ("xforge:solid", "flow-flow"),
        ("xforge:xspec", "flow-command"),
        ("xforge:apply", "flow-station"),
    ] {
        assert!(
            got.contains(&(n.to_string(), k.to_string())),
            "缺 {n}：{got:?}"
        );
    }
    h.core.state.ui.composer.clear();
    h.type_str("/xforge:ap").await;
    h.keys("Tab").await;
    assert_eq!(h.core.state.ui.composer.text, "/xforge:apply ");
}

#[tokio::test(start_paused = true)]
async fn proposals_wait_in_the_run_detail_and_arrive_in_the_chat() {
    let mut h = xforge().await;
    // 流程标签：x3 带着一条在等的建议，列在运行详情里。
    h.pick_upper(flow_index(&h, "xforge/solid")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "x3")).await;
    h.settle().await;
    let lower = h.lower();
    assert!(
        lower.contains("建议打回到「设计」")
            && lower.contains("接口缺取消语义")
            && lower.contains("reviewer"),
        "{lower}"
    );
    // x1 的会话里 5 秒时来了一条：在它的对话里放一条提示。
    h.pick_upper(run_index(&h, "x1")).await;
    h.settle().await;
    assert!(!h.text().contains("worker"));
    h.advance(6_000).await;
    let t = h.text();
    assert!(
        t.contains("建议打回到「设计」") && t.contains("接受：/xforge:design"),
        "{t}"
    );
    // 接受它就是 /xforge:design：打回，建议标成已接受。
    h.keys("Enter").await;
    h.core.state.ui.composer.clear();
    say(&mut h, "/xforge:design").await;
    let v = &h.core.state.domain.run_views[&port::RunId("x1".into())];
    assert_eq!(v.proposals[0].status, port::ProposalStatus::Accepted);
}

#[tokio::test(start_paused = true)]
async fn pace_step_shows_on_the_run_and_votes_on_the_confirm_station() {
    let mut h = xforge().await;
    h.pick_upper(flow_index(&h, "xforge/solid")).await;
    h.keys("Right").await;
    h.settle().await;
    let upper = h.panel();
    let _ = upper;
    let t = h.text();
    let x1_line = t.lines().find(|l| l.contains("实施") && l.contains("步进"));
    assert!(x1_line.is_some(), "步进的运行标着步进\n{t}");
    h.pick_upper(run_index(&h, "x3")).await;
    h.settle().await;
    assert!(h.lower().contains("1/2 票"), "{}", h.lower());
    // 第二票：够了，站过，票数 2/2。
    h.keys("m").await;
    h.core.state.ui.composer.clear();
    say(&mut h, "/confirm check").await;
    assert_eq!(station(&h, "x3", "check").status, StationStatus::Passed);
    h.pick_upper(run_index(&h, "x3")).await;
    assert!(h.lower().contains("2/2 票"), "{}", h.lower());
}

/// 多人确认：一票不够，站还在等；驳回一票就不过，票面上看得到。
#[tokio::test(start_paused = true)]
async fn one_vote_of_two_does_not_pass_and_a_rejection_shows() {
    let mut h = xforge().await;
    open_run(&mut h, "xforge/solid", "x3").await;
    say(&mut h, "/reject check 缺取消").await;
    let st = station(&h, "x3", "check");
    assert_eq!(st.status, StationStatus::Unmet);
    assert_eq!(st.votes.map(|v| v.rejections), Some(1));
    h.pick_upper(run_index(&h, "x3")).await;
    assert!(h.lower().contains("1 票驳回"), "{}", h.lower());
}

#[tokio::test(start_paused = true)]
async fn snapshot_of_a_stepped_run_with_votes_and_a_proposal() {
    let mut h = xforge().await;
    h.pick_upper(flow_index(&h, "xforge/solid")).await;
    h.keys("Right").await;
    h.settle().await;
    h.pick_upper(run_index(&h, "x3")).await;
    h.settle().await;
    h.snapshot("flow-xforge-run");
}
