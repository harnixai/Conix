//! 场景 m09、m10、m25：会话标签（需求 S-01 至 S-09、C-12、C-16，交互 §4.1、§5.2）。

mod common;

use common::Harness;
use port::Command;
use tui::Size;

const SIZE: Size = Size::new(160, 45);

async fn wb(scenario: &str) -> Harness {
    Harness::new(scenario, SIZE).await
}

#[tokio::test(start_paused = true)]
async fn upper_lists_main_agents_with_origin_and_expands_goal_and_todo() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    let rows = h.core.state.session_rows();
    assert_eq!(rows[0], model::SessRow::New, "第一行是新建会话");
    assert!(
        rows.iter().filter_map(|r| r.session()).all(|id| {
            let x = h
                .core
                .state
                .domain
                .sessions
                .iter()
                .find(|x| &x.id == id)
                .unwrap();
            matches!(
                x.provenance.origin,
                port::Origin::User | port::Origin::Fork { .. }
            )
        }),
        "只列人发起的会话：子 Agent、流程、作业开出来的都不在"
    );
    let text = h.text();
    assert!(text.contains("＋ 新建会话"));
    assert!(text.contains("⑂"), "分叉的标出来");
    assert!(!text.contains("⏰") && !text.contains("⇶"));
    h.keys("Right").await;
    let text = h.text();
    assert!(
        text.contains("goal") && text.contains("迁到 OAuth"),
        "{text}"
    );
    assert!(text.contains("第 2/256 轮"));
    assert!(text.contains("todo ▸ 2/7"));
    h.keys("Down Down Enter").await;
    assert!(h.text().contains("todo ▾"), "todo 展开明细");
    h.snapshot("sessions-upper-expanded");
    h.keys("Left").await;
    assert!(
        !h.core
            .state
            .session_rows()
            .iter()
            .any(|r| matches!(r, model::SessRow::Goal(_))),
        "收起"
    );
}

#[tokio::test(start_paused = true)]
async fn lower_tree_three_levels_and_finished_group() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    let text = h.text();
    for want in [
        "主 Agent",
        "审查认证改动",
        "cargo test",
        "检查依赖许可",
        // 子 Agent 写类型再跟它实际用的模型：它与父会话不是同一家。
        "general · deepseek",
        "cargo deny check licenses",
        "已结束 (",
        "├ ",
        "└ ",
    ] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    assert!(text.contains("等你批准"), "在等人的子 Agent 高亮");
    let in_tree = |h: &Harness, label: &str| {
        h.core.state.tree_rows().iter().any(|r| match r {
            model::TreeRow::Task { id, .. } => h
                .core
                .state
                .conv_tasks()
                .iter()
                .any(|t| &t.id == id && t.label == label),
            _ => false,
        })
    };
    assert!(!in_tree(&h, "调研 OAuth 库"), "已结束的缺省收起");
    assert!(text.contains("◆ 审查认证改动"), "子 Agent 有图标");
    assert!(text.contains("$ cargo test"), "shell 任务有自己的图标");
    h.keys("Tab").await;
    let finished = h
        .core
        .state
        .tree_rows()
        .iter()
        .position(|r| matches!(r, model::TreeRow::Finished { .. }))
        .unwrap();
    for _ in 0..finished {
        h.keys("Down").await;
    }
    h.keys("Enter").await;
    assert!(in_tree(&h, "调研 OAuth 库"), "展开已结束");
    h.snapshot("sessions-tree");
}

#[tokio::test(start_paused = true)]
async fn drill_into_sub_agent_message_it_and_come_back() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.select_task("审查认证改动").await;
    h.keys("Enter").await;
    h.settle().await;
    let text = h.text();
    assert!(
        text.contains("会话 › 重构认证模块 › 审查认证改动"),
        "面包屑\n{text}"
    );
    assert!(text.contains("子 Agent ← 重构认证模块"), "归属行");
    h.type_str("先只看 auth 目录").await;
    h.keys("Enter").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::MessageTask { task, text } if task.as_str() == "t1" && text == "先只看 auth 目录"
    )));
    h.keys("Esc").await;
    assert!(h.text().contains("会话 › 重构认证模块"));
    assert!(!h.text().contains("› 审查认证改动 ─"));
}

#[tokio::test(start_paused = true)]
async fn task_output_is_read_only_and_follows() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.select_task("tail -f app.log").await;
    h.keys("Enter").await;
    h.settle().await;
    let text = h.text();
    assert!(text.contains("› tail -f app.log"), "{text}");
    h.type_str("abc").await;
    assert!(h.core.state.ui.composer.text.is_empty(), "只读");
    h.keys("Esc").await;
    assert!(h.core.state.ui.task_view.is_none());
}

#[tokio::test(start_paused = true)]
async fn stop_rename_close_with_confirmation() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.select_task("tail -f app.log").await;
    h.keys("s").await;
    assert!(h.last_line().contains("停止「tail -f app.log」"));
    h.keys("y").await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::StopTask { task } if task.as_str() == "t4"))
    );
    h.select_session("昨天的调研").await;
    h.keys("r").await;
    assert_eq!(h.core.state.ui.composer.text, "昨天的调研");
    h.type_str("（已归档）").await;
    h.keys("Enter").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::Rename { title, .. } if title == "昨天的调研（已归档）"
    )));
    h.select_session("翻译文档").await;
    h.keys("x y").await;
    assert!(
        h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::Close { .. }))
    );
}

#[tokio::test(start_paused = true)]
async fn goal_pause_and_clear() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    h.keys("Right Down p").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::UpdateGoal {
            action: port::GoalAction::Pause,
            ..
        }
    )));
    h.keys("x y").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::UpdateGoal {
            action: port::GoalAction::Clear,
            ..
        }
    )));
}

#[tokio::test(start_paused = true)]
async fn m09_tree_changes_live() {
    let mut h = wb("m09-session-tree").await;
    h.select_session("重构认证模块").await;
    h.advance(21_000).await;
    let tasks = h.core.state.conv_tasks();
    assert!(
        tasks.iter().any(|t| t.label == "补测试"),
        "新出现的子 Agent"
    );
    let t9 = tasks.iter().find(|t| t.id.as_str() == "t9").unwrap();
    assert_eq!(t9.status, port::TaskStatus::Done);
    assert!(tasks.iter().any(|t| t.id.as_str() == "t14"));
}

#[tokio::test(start_paused = true)]
async fn m10_goal_rounds_and_blocked() {
    let mut h = wb("m10-goal-todo").await;
    h.select_session("重构认证模块").await;
    h.keys("Right").await;
    h.advance(10_000).await;
    assert!(
        h.text().contains("第 4/256 轮") || h.text().contains("第 5/256 轮"),
        "{}",
        h.text()
    );
    h.advance(8_000).await;
    assert!(h.text().contains("受阻：需要你确认数据库迁移方案"));
}

#[tokio::test(start_paused = true)]
async fn m25_ownership_line_for_every_origin() {
    for (title, want) in [
        ("昨天的调研", "分叉自 重构认证模块"),
        (
            "重构认证模块",
            "[CONIX] dev@ops-01  ~/work/auth  code  claude-sonnet-5-5",
        ),
        ("老项目会话", "⚠ 被实例 research 占用"),
    ] {
        let mut h = wb("default").await;
        h.open_session(title).await;
        let text = h.text();
        assert!(text.contains(want), "{title}：缺 {want}\n{text}");
    }
    // 流程与作业开出来的会话不在会话列表里，从流程、作业标签进去。
    let mut h = wb("default").await;
    h.tab(model::Tab::Flows).await;
    let r1 = h
        .core
        .state
        .flow_rows()
        .iter()
        .position(|r| matches!(r, model::FlowRow::Run(_, id) if id.as_str() == "r1"))
        .unwrap();
    h.pick_upper(r1).await;
    assert!(h.text().contains("流程「需求到上线」› 支付重试 › 实现"));
    h.tab(model::Tab::Jobs).await;
    h.pick_upper(0).await;
    let entry = h
        .core
        .state
        .job_lower_rows()
        .iter()
        .position(|r| matches!(r, model::JobLower::Entry(0)))
        .unwrap();
    h.pick_lower(entry).await;
    assert!(h.text().contains("作业「"), "{}", h.text());
}

#[tokio::test(start_paused = true)]
async fn new_session_row_opens_a_draft_and_the_first_words_create_it() {
    let mut h = wb("default").await;
    h.open_session("重构认证模块").await;
    h.tab(model::Tab::Sessions).await;
    h.pick_upper(0).await;
    h.keys("Enter").await;
    h.settle().await;
    assert!(h.core.state.ui.chat.is_none(), "＋ 新建会话是草稿");
    assert_eq!(h.core.state.ui.focus, model::Focus::Composer);
    let created = |h: &common::Harness| {
        h.calls("dev")
            .iter()
            .filter(|c| matches!(c, Command::CreateSession { .. }))
            .count()
    };
    assert_eq!(created(&h), 0, "没输入就不建会话");
    h.type_str("帮我看看日志").await;
    h.keys("Enter").await;
    h.settle().await;
    assert_eq!(created(&h), 1, "第一句话建出会话");
    let id = h.core.state.ui.chat.clone().expect("左边是新会话");
    assert_eq!(
        h.core.state.session_rows()[h.core.state.ui.upper[0].selected],
        model::SessRow::Session(id),
        "右栏选中跟到新会话"
    );
    h.tab(model::Tab::Sessions).await;
    h.keys("n").await;
    assert!(h.core.state.ui.chat.is_none(), "n 也是回到草稿");
    h.open_session("重构认证模块").await;
    h.type_str("/new").await;
    h.keys("Enter").await;
    assert!(h.core.state.ui.chat.is_none(), "/new 也是回到草稿");
    assert_eq!(created(&h), 1);
}

#[tokio::test(start_paused = true)]
async fn todos_are_numbered_open_first_done_last() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
    let p = h.panel();
    let lines: Vec<&str> = p.lines().filter(|l| l.contains(". ")).collect();
    assert!(lines[0].contains("▶ 3. 实现 provider"), "{p}");
    assert!(lines.last().unwrap().contains("✓ 2. 调研 OAuth 库"), "{p}");
    assert!(!p.contains("☒"), "做完的是 ✓，不是 ☒");
}

#[tokio::test(start_paused = true)]
async fn tree_rows_end_with_time_and_tokens_not_status_words() {
    let mut h = wb("default").await;
    h.select_session("重构认证模块").await;
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
    for word in ["运行中", "等你批准", "完成", "失败", "已停止", "空闲"] {
        assert!(!lower.contains(word), "右下不写状态字：{word}\n{lower}");
    }
    assert!(lower.contains("8m00s, "), "{lower}");
}

#[tokio::test(start_paused = true)]
async fn a_long_panel_scrolls_under_the_wheel_and_never_takes_the_focus() {
    // 矮一点的终端：面板最多占对话区的 1/3，goal + 7 条 todo 放不下，要滚。
    let mut h = Harness::new("default", Size::new(160, 24)).await;
    h.open_session("重构认证模块").await;
    assert!(h.text().contains("滚轮滚动"), "放不下时下沿标出可以滚动");
    let (col, row) = h.find("实现 provider").unwrap();
    h.click(col, row);
    assert_eq!(
        h.core.state.ui.focus,
        model::Focus::Composer,
        "点面板内容不拿焦点"
    );
    let before = h.text();
    h.mouse(model::MouseKind::ScrollDown, col, row);
    h.mouse(model::MouseKind::ScrollDown, col, row);
    h.settle().await;
    assert_ne!(before, h.text(), "滚轮滚面板");
    assert!(h.text().contains("↑2 "));
    let (col, row) = h.find("^T 收起").unwrap();
    h.click(col, row);
    assert!(!h.core.state.panel_open(), "点下沿收起");
}

/// 关掉的会话在末尾的「已关闭」里，选中只读地看，`o` 重新打开回到上面（交互 §5.2）。
#[tokio::test(start_paused = true)]
async fn a_closed_session_waits_under_closed_and_can_be_reopened() {
    let mut h = Harness::new("default", Size::new(160, 45)).await;
    h.tab(model::Tab::Sessions).await;
    h.settle().await;
    let first = h.core.state.user_sessions()[0].clone();
    let at = h
        .core
        .state
        .session_rows()
        .iter()
        .position(|r| r == &model::SessRow::Session(first.id.clone()))
        .unwrap();
    h.pick_upper(at).await;
    h.keys("x y").await;
    h.settle().await;
    let rows = h.core.state.session_rows();
    let closed = rows
        .iter()
        .position(|r| r == &model::SessRow::Closed)
        .expect("出现「已关闭」");
    assert!(h.text().contains("已关闭 (1)"));
    h.pick_upper(closed).await;
    h.keys("Right").await;
    h.settle().await;
    let row = h
        .core
        .state
        .session_rows()
        .iter()
        .position(|r| r == &model::SessRow::ClosedSession(first.id.clone()))
        .unwrap();
    h.pick_upper(row).await;
    h.settle().await;
    assert_eq!(h.chat_title(), first.title, "左边只读地看它");
    h.keys("o").await;
    h.settle().await;
    assert!(
        h.core
            .state
            .user_sessions()
            .iter()
            .any(|s| s.id == first.id),
        "重新打开回到上面"
    );
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        port::Command::ReopenSession { session } if *session == first.id
    )));
}
