//! 右栏与左边对话区的对应（交互 §4.1、§5）：启动即草稿、选中按对象记、各标签的落点、
//! 流程与作业没有自己的会话、`n` `e` `d` 的话头、`#n` 序号、失焦块的选中底色。

mod common;

use common::Harness;
use model::{Focus, Msg, SessRow, Tab};
use port::{Command, Event, Origin, SessionId};
use tui::Size;

const SIZE: Size = Size::new(160, 45);

fn selected_upper(h: &Harness, tab: Tab) -> usize {
    h.core.state.ui.upper[tab.index()].selected
}

fn selected_session_row(h: &Harness) -> SessRow {
    h.core.state.session_rows()[selected_upper(h, Tab::Sessions)].clone()
}

fn session_id(h: &Harness, title: &str) -> SessionId {
    h.core
        .state
        .domain
        .sessions
        .iter()
        .find(|x| x.title == title)
        .unwrap_or_else(|| panic!("没有会话 {title}"))
        .id
        .clone()
}

fn no_eager_sessions(h: &Harness) {
    assert!(
        !h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::CreateSession { .. })),
        "选中流程、作业，按 n e d，都不建会话"
    );
}

#[tokio::test(start_paused = true)]
async fn the_workbench_starts_on_a_draft() {
    let mut h = Harness::new("default", SIZE).await;
    assert!(h.core.state.ui.chat.is_none(), "启动即草稿");
    assert_eq!(selected_session_row(&h), SessRow::New, "右栏选中＋新建");
    assert!(h.text().contains("会话 › 新会话"));
    assert!(
        !h.calls("dev")
            .iter()
            .any(|c| matches!(c, Command::CreateSession { .. })),
        "没输入就不建会话"
    );
}

#[tokio::test(start_paused = true)]
async fn the_selection_stays_on_the_same_session_when_the_list_changes() {
    let mut h = Harness::new("default", SIZE).await;
    h.select_session("昨天的调研").await;
    let picked = session_id(&h, "昨天的调研");
    assert_eq!(h.core.state.ui.chat, Some(picked.clone()));
    // 别处建了一个新会话：它排到最前面，选中仍是原来那个，左边也不动。
    let mut fresh = h
        .core
        .state
        .domain
        .sessions
        .iter()
        .find(|x| x.id == picked)
        .unwrap()
        .clone();
    fresh.id = SessionId("s-elsewhere".into());
    fresh.title = "别处建的".into();
    fresh.provenance.origin = Origin::User;
    fresh.created_ms = h.core.now_ms() + 1;
    let s = h
        .core
        .handle(Msg::Event(Box::new(Event::SessionUpserted(fresh))));
    h.shell.extend(s);
    h.settle().await;
    assert_eq!(
        h.core.state.session_rows()[1],
        SessRow::Session(SessionId("s-elsewhere".into())),
        "新会话排在最前（＋新建之后）"
    );
    assert_eq!(selected_session_row(&h), SessRow::Session(picked.clone()));
    assert_eq!(h.core.state.ui.chat, Some(picked.clone()), "左边不动");
    // 上面的会话展开 goal / todo，选中也不漂。
    let above = session_id(&h, "重构认证模块");
    h.core.state.ui.sess_expanded.insert(above);
    let s = h.core.tick();
    h.shell.extend(s);
    h.settle().await;
    assert_eq!(selected_session_row(&h), SessRow::Session(picked.clone()));
    // 选中的会话没了：落在原来的位置上，不报错。
    let s = h.core.handle(Msg::Event(Box::new(Event::SessionRemoved {
        session: picked.clone(),
    })));
    h.shell.extend(s);
    h.settle().await;
    assert_ne!(selected_session_row(&h), SessRow::Session(picked));
}

#[tokio::test(start_paused = true)]
async fn the_sessions_tab_keeps_a_user_session_or_lands_on_the_most_recent_one() {
    let mut h = Harness::new("default", SIZE).await;
    // 左边是用户会话：到会话标签留着它，右栏选中对上。
    h.open_session("昨天的调研").await;
    let mine = session_id(&h, "昨天的调研");
    h.tab(model::Tab::Jobs).await;
    h.tab(model::Tab::Sessions).await;
    assert_eq!(h.core.state.ui.chat, Some(mine.clone()));
    assert_eq!(selected_session_row(&h), SessRow::Session(mine));
    // 左边是作业触发出来的会话：到会话标签换到最近活跃的用户会话。
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
    assert!(!h.core.state.chat_is_user_session(), "在作业的会话里");
    h.tab(model::Tab::Sessions).await;
    let recent = h.core.state.recent_user_session().unwrap();
    assert_eq!(h.core.state.ui.chat, Some(recent.clone()));
    assert_eq!(selected_session_row(&h), SessRow::Session(recent));
    // 草稿：到会话标签选中＋新建。
    h.keys("n").await;
    assert!(h.core.state.ui.chat.is_none());
    h.tab(model::Tab::Sessions).await;
    assert_eq!(selected_session_row(&h), SessRow::New);
    assert!(h.core.state.ui.chat.is_none(), "草稿还是草稿");
}

#[tokio::test(start_paused = true)]
async fn the_flows_and_jobs_tabs_leave_the_chat_alone() {
    let mut h = Harness::new("default", SIZE).await;
    h.open_session("昨天的调研").await;
    let mine = session_id(&h, "昨天的调研");
    h.tab(model::Tab::Flows).await;
    let r1 = h
        .core
        .state
        .flow_rows()
        .iter()
        .position(|r| matches!(r, model::FlowRow::Run(_, id) if id.as_str() == "r1"))
        .unwrap();
    h.pick_upper(r1).await;
    assert_eq!(
        h.chat_title(),
        "支付重试（主会话）",
        "选中运行切到它的主会话"
    );
    h.tab(model::Tab::Sessions).await;
    let recent = h.core.state.recent_user_session();
    assert_eq!(
        h.core.state.ui.chat, recent,
        "到会话标签回到最近活跃的用户会话"
    );
    assert_ne!(recent, Some(mine));
    h.tab(model::Tab::Flows).await;
    assert_eq!(
        h.core.state.ui.chat, recent,
        "到流程标签时右上停在运行上，左边也不动"
    );
    h.tab(model::Tab::Jobs).await;
    assert_eq!(h.core.state.ui.chat, recent);
    h.keys("Ctrl+G Ctrl+G Ctrl+G").await;
    assert_eq!(h.core.state.ui.chat, recent, "焦点回到右栏也不算换了选中");
}

#[tokio::test(start_paused = true)]
async fn selecting_a_flow_or_a_job_keeps_the_chat() {
    let mut h = Harness::new("default", SIZE).await;
    h.open_session("昨天的调研").await;
    let mine = session_id(&h, "昨天的调研");
    h.tab(model::Tab::Jobs).await;
    for i in 0..h.core.state.job_rows().len() {
        h.pick_upper(i).await;
        assert_eq!(h.core.state.ui.chat, Some(mine.clone()), "作业 {i}");
    }
    h.tab(model::Tab::Flows).await;
    h.pick_upper(0).await;
    assert_eq!(h.core.state.ui.chat, Some(mine));
    no_eager_sessions(&h);
}

#[tokio::test(start_paused = true)]
async fn n_and_m_put_a_lead_in_into_the_user_session() {
    let mut h = Harness::new("default", SIZE).await;
    // 草稿上按 m：留在草稿，话头发出去就是一个新会话。
    h.tab(model::Tab::Jobs).await;
    h.pick_upper(0).await;
    h.keys("m").await;
    assert!(h.core.state.ui.chat.is_none(), "草稿留在草稿");
    assert_eq!(h.core.state.ui.composer.text, "作业 #1「每日巡检」：");
    h.keys("Ctrl+U").await;
    h.tab(model::Tab::Jobs).await;
    // 左边是作业触发出来的会话时按 m（Tab 回到上半，说的是作业）：回到最近活跃的用户会话。
    let entry = h
        .core
        .state
        .job_lower_rows()
        .iter()
        .position(|r| matches!(r, model::JobLower::Entry(0)))
        .unwrap();
    h.pick_lower(entry).await;
    h.keys("Tab m").await;
    assert_eq!(h.core.state.ui.composer.text, "作业 #1「每日巡检」：");
    assert!(h.core.state.chat_is_user_session());
    let mine = h.core.state.recent_user_session().unwrap();
    assert_eq!(h.core.state.ui.focus, Focus::Composer);
    h.keys("Ctrl+U").await;
    h.tab(model::Tab::Jobs).await;
    h.keys("n").await;
    assert_eq!(h.core.state.ui.composer.text, "新建一个作业：");
    h.keys("Ctrl+U").await;
    h.tab(model::Tab::Flows).await;
    h.pick_upper(0).await;
    h.keys("m").await;
    assert_eq!(h.core.state.ui.composer.text, "流程 #1「需求到上线」：");
    h.keys("Ctrl+U").await;
    h.tab(model::Tab::Flows).await;
    h.keys("n").await;
    assert_eq!(h.core.state.ui.composer.text, "新建一个流程：");
    assert_eq!(h.core.state.ui.chat, Some(mine), "一直在同一个用户会话里");
    no_eager_sessions(&h);
}

#[tokio::test(start_paused = true)]
async fn numbers_follow_the_canonical_order_and_ignore_the_filter() {
    let mut h = Harness::new("default", SIZE).await;
    h.tab(model::Tab::Flows).await;
    let text = h.text();
    for want in ["#1 ◎ 需求到上线", "#2 ◎ 发布检查", "#3   周报"] {
        assert!(text.contains(want), "缺 {want}\n{text}");
    }
    h.keys("/").await;
    h.type_str("周报").await;
    h.keys("Enter").await;
    assert!(h.text().contains("#3   周报"), "过滤时不重编");
    h.tab(model::Tab::Jobs).await;
    let text = h.text();
    assert!(text.contains("#1 ○ 每日巡检") && text.contains("#4 ⏸ 依赖巡检"));
    h.tab(model::Tab::Sessions).await;
    let n = h.core.state.user_sessions().len();
    let newest = h.core.state.user_sessions()[0].title.clone();
    assert!(h.text().contains("#1 "), "会话也编号");
    assert!(
        h.text()
            .lines()
            .any(|l| l.contains("#1 ") && l.contains(&newest)),
        "#1 是最新建的"
    );
    assert!(n > 1);
}

#[tokio::test(start_paused = true)]
async fn rename_sets_the_current_sessions_title() {
    let mut h = Harness::new("default", SIZE).await;
    h.type_str("/rename").await;
    h.keys("Enter").await;
    assert!(
        h.last_line().contains("先打开"),
        "草稿上没有会话可改：{}",
        h.last_line()
    );
    h.open_session("昨天的调研").await;
    let mine = session_id(&h, "昨天的调研");
    h.type_str("/rename 新的名字").await;
    h.keys("Enter").await;
    assert!(h.calls("dev").iter().any(|c| matches!(
        c,
        Command::Rename { session, title } if session == &mine && title == "新的名字"
    )));
    h.type_str("/rename").await;
    h.keys("Enter").await;
    assert_eq!(h.core.state.ui.rename, Some(mine), "不带名字：进改名");
    let picker: Vec<String> = {
        h.keys("Ctrl+U").await;
        h.type_str("/ren").await;
        h.core
            .state
            .picker_candidates()
            .into_iter()
            .map(|c| c.name)
            .collect()
    };
    assert!(picker.contains(&"rename".to_string()), "补全里有它");
}

#[tokio::test(start_paused = true)]
async fn the_unfocused_block_keeps_its_selection() {
    let mut h = Harness::new("default", SIZE).await;
    h.select_session("重构认证模块").await;
    h.keys("Tab").await;
    let side = |h: &mut Harness| {
        let tui::Body::Workbench(w) = h.core.frame().body else {
            panic!("不在工作台")
        };
        w.side.unwrap()
    };
    let s = side(&mut h);
    assert!(
        !s.upper.focused && s.upper.selected.is_some(),
        "右上失焦仍有选中"
    );
    assert!(s.lower.focused && s.lower.selected.is_some());
    h.keys("Ctrl+G").await;
    let s = side(&mut h);
    assert!(
        !s.upper.focused && s.upper.selected.is_some(),
        "焦点在左边时右栏的选中也留着"
    );
}
