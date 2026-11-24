//! 场景 m14、m15、m18、m19、m20、m22，以及过滤与事件日志（需求 L-04、L-05、M、I-09，交互 §6、§9）。

mod common;

use app::core::Start;
use common::Harness;
use model::{InstOp, Screen, Tab};
use tui::Size;

const SIZE: Size = Size::new(160, 45);

#[tokio::test(start_paused = true)]
async fn m14_ctrl_r_walks_every_waiting_place_across_tabs() {
    let mut h = Harness::new("m14-attention", SIZE).await;
    h.advance(2_500).await;
    let items = h.core.state.attention();
    assert!(
        items
            .iter()
            .any(|a| matches!(a, model::Attention::Session(_)))
    );
    assert!(items.iter().any(|a| matches!(a, model::Attention::Job(_))));
    assert!(
        h.last_line()
            .contains(&format!("下一处待处理({})", items.len())),
        "{}",
        h.last_line()
    );
    let mut tabs = std::collections::HashSet::new();
    for _ in 0..items.len() {
        h.keys("Ctrl+R").await;
        tabs.insert(format!("{:?}", h.core.state.ui.tab));
        assert!(h.core.state.ui.focus.in_side());
    }
    assert!(tabs.len() >= 2, "跨标签：{tabs:?}");
}

#[tokio::test(start_paused = true)]
async fn m15_reconnect_shows_state_and_refreshes() {
    let mut h = Harness::new("m15-reconnect", SIZE).await;
    h.advance(2_100).await;
    assert!(h.text().contains("⚠ 已断开"));
    h.advance(1_000).await;
    assert!(h.text().contains("重连中 (第 1 次)"));
    h.advance(4_500).await;
    let text = h.text();
    assert!(text.contains("● 已连接"), "{text}");
    let lists = h
        .calls("dev")
        .iter()
        .filter(|c| matches!(c, port::Command::ListSessions { .. }))
        .count();
    assert!(lists >= 2, "重连后重新列一遍");
}

#[tokio::test(start_paused = true)]
async fn m18_errors_show_where_they_happen_and_can_retry() {
    let mut h = Harness::new("m18-errors", SIZE).await;
    h.tab(model::Tab::Flows).await;
    assert!(h.text().contains("✗ 加载失败：流程服务暂时不可用 · r 重试"));
    h.keys("r").await;
    h.settle().await;
    assert!(h.text().contains("需求到上线"), "重试成功");
    h.open_session("昨天的调研").await;
    h.type_str("你好").await;
    h.keys("Enter").await;
    h.advance(2_000).await;
    assert!(
        h.transcript().contains("Error: overloaded"),
        "{}",
        h.transcript()
    );
}

#[tokio::test(start_paused = true)]
async fn m19_long_lists_scroll_and_filter() {
    let mut h = Harness::start(
        "default",
        SIZE,
        Start::Op(InstOp::Attach {
            name: "ops".into(),
            force: true,
        }),
    )
    .await;
    h.tab(Tab::Sessions).await;
    h.keys("PageDown PageDown PageDown").await;
    let sel = h.core.state.ui.upper().selected;
    assert!(sel >= 30, "{sel}");
    assert!(h.core.state.ui.upper().scroll > 0, "滚动了");
    h.keys("/").await;
    h.type_str("099").await;
    h.keys("Enter").await;
    assert_eq!(h.core.state.session_rows().len(), 1, "过滤到一条");
    assert!(h.text().contains("会话 099"));
    h.keys("/ Esc").await;
    assert!(h.core.state.session_rows().len() > 50, "Esc 清除过滤");
}

#[tokio::test(start_paused = true)]
async fn m20_first_run_has_guiding_empty_states() {
    let mut h = Harness::new("m20-first-run", SIZE).await;
    let text = h.text();
    assert!(
        text.contains(&format!("Conix {}", model::short_version())),
        "欢迎块"
    );
    assert!(
        h.panel().is_empty() && !text.contains("Ctrl+T"),
        "没有 goal / todo 的会话没有面板，连标题线也没有"
    );
    assert!(text.contains("还没有会话"), "{text}");
    h.tab(model::Tab::Flows).await;
    assert!(h.text().contains("还没有流程"));
    h.tab(model::Tab::Jobs).await;
    assert!(h.text().contains("还没有作业"));
}

#[tokio::test(start_paused = true)]
async fn m22_mouse_on_picker_exit_dialog_tabs_and_ask() {
    let mut h = Harness::start("default", SIZE, Start::Picker).await;
    let (col, row) = h.find("research").unwrap();
    h.click(col, row);
    h.click(col, row);
    h.settle().await;
    assert_eq!(
        h.core.state.ui.attached.as_deref(),
        Some("research"),
        "双击启动"
    );
    let (col, row) = h.find("作业").unwrap();
    h.click(col + 1, row);
    assert_eq!(h.core.state.ui.tab, Tab::Jobs);
    h.keys("Ctrl+X").await;
    let (col, row) = h.find("返回").unwrap();
    h.click(col, row);
    assert!(h.core.state.ui.overlay.is_none(), "点返回关掉退出框");
    h.keys("F1").await;
    h.click(0, 1);
    assert!(h.core.state.ui.overlay.is_none(), "点框外关掉浮层");
    h.mouse(model::MouseKind::ScrollDown, 140, 10);
}

#[tokio::test(start_paused = true)]
async fn mouse_click_ask_option_answers() {
    let mut h = Harness::new("m04-prompts", SIZE).await;
    h.open_session("昨天的调研").await;
    h.type_str("跑测试").await;
    h.keys("Enter").await;
    h.advance(600).await;
    let (col, row) = h.find("2. 是，且不再询问").unwrap();
    h.click(col, row);
    h.click(col, row);
    h.settle().await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        port::Command::Answer {
            answer: port::Answer::PermitSession,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn events_command_lists_events_and_commands() {
    let mut h = Harness::new("m01-chat-basic", SIZE).await;
    h.open_session("昨天的调研").await;
    h.type_str("你好").await;
    h.keys("Enter").await;
    h.advance(2_000).await;
    h.type_str("/events").await;
    h.keys("Enter").await;
    let text = h.text();
    assert!(text.contains("事件日志"));
    assert!(
        text.contains("← text_delta") && text.contains("→ submit"),
        "{text}"
    );
    h.keys("Esc").await;
    assert!(h.core.state.ui.overlay.is_none());
}

#[tokio::test(start_paused = true)]
async fn selection_follows_in_sessions_tab_without_enter() {
    let mut h = Harness::new("default", SIZE).await;
    h.select_session("昨天的调研").await;
    assert_eq!(h.chat_title(), "昨天的调研", "选中即切换");
    h.select_session("重构认证模块").await;
    assert_eq!(h.chat_title(), "重构认证模块");
    h.select_task("审查认证改动").await;
    assert_eq!(h.chat_title(), "审查认证改动", "下半选中子 Agent 也跟着切");
    h.select_task("tail -f app.log").await;
    assert!(
        h.core.state.ui.task_view.is_some(),
        "选中没有会话的任务：看它的输出"
    );
    assert!(h.text().contains("› tail -f app.log"));
    assert!(matches!(h.core.state.ui.screen, Screen::Workbench));
}

#[tokio::test(start_paused = true)]
async fn the_one_demo_plays_through_every_tab() {
    let mut h = Harness::new("demo", SIZE).await;
    h.select_session("重构认证模块").await;
    h.advance(40_000).await;
    assert!(
        h.panel().contains("受阻：需要你确认数据库迁移方案"),
        "goal 轮次走到受阻\n{}",
        h.panel()
    );
    let items = h.core.state.attention();
    for kind in ["Session", "Run", "Job"] {
        assert!(
            items.iter().any(|a| format!("{a:?}").starts_with(kind)),
            "演示里有 {kind} 类待处理：{items:?}"
        );
    }
    h.tab(model::Tab::Flows).await;
    h.settle().await;
    assert!(h.text().contains("需求到上线"));
    h.tab(model::Tab::Jobs).await;
    h.settle().await;
    assert!(h.text().contains("依赖巡检"));
    h.select_session("Conix 展示").await;
    h.keys("Enter").await;
    h.type_str("看看效果").await;
    h.keys("Enter").await;
    h.advance(8_000).await;
    assert!(h.transcript().contains("看看效果"));
}

#[tokio::test(start_paused = true)]
async fn flows_and_jobs_have_no_sessions_of_their_own() {
    let mut h = Harness::new("default", SIZE).await;
    h.tab(model::Tab::Flows).await;
    h.pick_upper(0).await;
    h.tab(model::Tab::Jobs).await;
    h.pick_upper(0).await;
    h.keys("n").await;
    h.settle().await;
    assert_eq!(h.core.state.ui.composer.text, "新建一个作业：");
    assert!(
        h.core.state.ui.chat.is_none() || h.core.state.chat_is_user_session(),
        "话头放在用户会话（或草稿）里"
    );
    assert!(
        !h.calls("dev")
            .iter()
            .any(|c| matches!(c, port::Command::CreateSession { .. })),
        "不预先建会话"
    );
    h.tab(model::Tab::Sessions).await;
    for r in h.core.state.session_rows() {
        if let Some(id) = r.session() {
            let x = h
                .core
                .state
                .domain
                .sessions
                .iter()
                .find(|x| &x.id == id)
                .unwrap();
            assert!(
                port::order::is_user_session(x),
                "{} 不该在会话列表里",
                x.title
            );
        }
    }
}

#[tokio::test(start_paused = true)]
async fn tabs_with_waiting_items_get_a_mark_not_the_selected_color() {
    let mut h = Harness::new("default", SIZE).await;
    h.tab(model::Tab::Flows).await;
    h.settle().await;
    h.tab(model::Tab::Sessions).await;
    let line = h.text().lines().nth(1).unwrap().to_string();
    assert!(line.contains(" 流程 ◐ "), "有待处理的标签后面跟 ◐\n{line}");
}

#[tokio::test(start_paused = true)]
async fn scroll_keys_act_on_the_focused_area() {
    let mut h = Harness::new("demo", Size::new(160, 30)).await;
    h.open_session("Conix 展示").await;
    for i in 0..20 {
        h.type_str(&format!("第 {i} 行")).await;
        h.keys("Shift+Enter").await;
    }
    let (len, rows) = h.core.state.composer_overflow().expect("输入超出了框");
    assert!(len > rows);
    assert!(h.text().contains("第 19 行"), "缺省跟着光标走");
    h.keys("PageUp PageUp PageUp").await;
    let text = h.text();
    assert!(
        text.contains("第 0 行") && !text.contains("第 19 行"),
        "输入超出时滚输入区\n{text}"
    );
    h.type_str("x").await;
    assert!(h.text().contains("第 19 行"), "一编辑就回到光标处");
    h.keys("Ctrl+U").await;
    assert!(h.core.state.composer_overflow().is_none());
    assert!(h.core.state.conv().unwrap().follow);
    h.keys("Shift+Up Shift+Up").await;
    assert!(!h.core.state.conv().unwrap().follow, "输入没超出时滚转录");
}
