//! 无终端的驱动器（testing.md §2）：mock 后端 + tokio 暂停时钟 + 按键 / 鼠标脚本 + 快照。

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use app::Core;
use app::core::{ShellEffect, Start};
use app::snapshot::{assert_snapshot, render_text};
use model::{InstOp, Mouse, MouseButton, MouseKind, Msg};
use tui::Size;

pub const EPOCH_MS: u64 = 1_763_280_000_000;

pub struct Harness {
    pub core: Core,
    pub size: Size,
    pub instances: Arc<mock::MockInstances>,
    pub shell: Vec<ShellEffect>,
}

impl Harness {
    /// 直接接入 mock 实例 `dev`。
    pub async fn new(scenario: &str, size: Size) -> Harness {
        Harness::start(
            scenario,
            size,
            Start::Op(InstOp::Attach {
                name: "dev".into(),
                force: false,
            }),
        )
        .await
    }

    pub async fn start(scenario: &str, size: Size, start: Start) -> Harness {
        let settings = model::Settings::default();
        let sc =
            mock::load_scenario(scenario).unwrap_or_else(|_| mock::Scenario::seed_only(scenario));
        let instances = Arc::new(mock::MockInstances::new(sc, mock::MockOptions::default()));
        let begin = tokio::time::Instant::now();
        let clock = Box::new(move || EPOCH_MS + begin.elapsed().as_millis() as u64);
        let (core, shell) = Core::new(
            instances.clone(),
            Arc::new(keys::Keymap::default()),
            size,
            clock,
            start,
            settings,
        );
        let mut h = Harness {
            core,
            size,
            instances,
            shell,
        };
        h.settle().await;
        h
    }

    /// 让已 spawn 的后端调用跑完、结果进来。
    pub async fn settle(&mut self) {
        for _ in 0..4 {
            for _ in 0..20 {
                tokio::task::yield_now().await;
            }
            let s = self.core.pump();
            self.shell.extend(s);
        }
    }

    /// 让时间走 `ms`：途中到期的定时（model 的 deadline）都按时 Tick。
    pub async fn advance(&mut self, ms: u64) {
        let end = self.core.now_ms() + ms;
        loop {
            let now = self.core.now_ms();
            let next = self.core.deadline().filter(|d| *d <= end).unwrap_or(end);
            let step = next.saturating_sub(now).max(1).min(end.saturating_sub(now));
            if step > 0 {
                tokio::time::advance(Duration::from_millis(step)).await;
            }
            self.settle().await;
            if self
                .core
                .deadline()
                .is_some_and(|d| d <= self.core.now_ms())
            {
                let s = self.core.tick();
                self.shell.extend(s);
                self.settle().await;
            }
            if self.core.now_ms() >= end {
                return;
            }
        }
    }

    /// 按 Ctrl+N 换到标签 `t`。已经在它上面就等于点一下标签头（会话标签会重新落点），不绕一圈。
    pub async fn tab(&mut self, t: model::Tab) {
        if self.core.state.ui.tab == t {
            let a = match t {
                model::Tab::Sessions => keys::Action::TabSessions,
                model::Tab::Flows => keys::Action::TabFlows,
                model::Tab::Jobs => keys::Action::TabJobs,
            };
            let s = self.core.handle(Msg::Action(a));
            self.shell.extend(s);
            self.settle().await;
            return;
        }
        while self.core.state.ui.tab != t {
            self.keys("Ctrl+N").await;
        }
    }

    /// 空格分隔的按键：`"Ctrl+B Ctrl+N Down"`；字面空格写 `Space`。
    pub async fn keys(&mut self, seq: &str) {
        for k in seq.split_whitespace() {
            let key = keys::Key::parse(k).unwrap_or_else(|| panic!("按键写错了：{k}"));
            let s = self.core.key(key);
            self.shell.extend(s);
        }
        self.settle().await;
    }

    pub fn mouse(&mut self, kind: MouseKind, col: u16, row: u16) {
        let s = self.core.mouse(Mouse { kind, col, row });
        self.shell.extend(s);
    }

    pub fn click(&mut self, col: u16, row: u16) {
        self.mouse(MouseKind::Down(MouseButton::Left), col, row);
        self.mouse(MouseKind::Up(MouseButton::Left), col, row);
    }

    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.mouse(MouseKind::Down(MouseButton::Left), from.0, from.1);
        self.mouse(MouseKind::Drag(MouseButton::Left), to.0, to.1);
        self.mouse(MouseKind::Up(MouseButton::Left), to.0, to.1);
    }

    pub fn resize(&mut self, size: Size) {
        self.size = size;
        let s = self.core.handle(Msg::Resize(size));
        self.shell.extend(s);
    }

    pub fn text(&mut self) -> String {
        let f = self.core.frame();
        render_text(&f, self.size)
    }

    pub fn snapshot(&mut self, name: &str) {
        let t = self.text();
        assert_snapshot(name, &t);
    }

    /// 屏幕上某段文字第一次出现的位置（列、行），按显示宽度计列。
    pub fn find(&mut self, needle: &str) -> Option<(u16, u16)> {
        let text = self.text();
        for (row, line) in text.lines().enumerate() {
            if let Some(byte) = line.find(needle) {
                let col = tui::display_width(&line[..byte]);
                return Some((col as u16, row as u16));
            }
        }
        None
    }

    /// 在输入框里打字（逐字，等同键盘输入）。
    pub async fn type_str(&mut self, text: &str) {
        for c in text.chars() {
            let s = self.core.handle(Msg::Text(c));
            self.shell.extend(s);
        }
        self.settle().await;
    }

    /// 在会话标签里选中标题为 `title` 的主 Agent 并打开它。
    pub async fn open_session(&mut self, title: &str) {
        self.tab(model::Tab::Sessions).await;
        let i = self
            .core
            .state
            .session_rows()
            .iter()
            .position(|r| {
                matches!(r, model::SessRow::Session(id)
                    if self.core.state.domain.sessions.iter().any(|x| &x.id == id && x.title == title))
            })
            .unwrap_or_else(|| panic!("会话列表里没有 {title}"));
        let cur = self.core.state.ui.upper().selected;
        let (key, n) = if i >= cur {
            ("Down", i - cur)
        } else {
            ("Up", cur - i)
        };
        for _ in 0..n {
            self.keys(key).await;
        }
        self.keys("Enter").await;
        self.settle().await;
    }

    /// 会话标签上半选中标题为 `title` 的主 Agent（不打开）。
    pub async fn select_session(&mut self, title: &str) {
        self.tab(model::Tab::Sessions).await;
        let target = self
            .core
            .state
            .session_rows()
            .iter()
            .position(|r| {
                matches!(r, model::SessRow::Session(id)
                    if self.core.state.domain.sessions.iter().any(|x| &x.id == id && x.title == title))
            })
            .unwrap_or_else(|| panic!("没有会话 {title}"));
        self.step_to(target, true).await;
        self.settle().await;
    }

    /// 会话标签下半选中标签为 `label` 的任务（先切到下半）。
    pub async fn select_task(&mut self, label: &str) {
        if self.core.state.ui.focus != model::Focus::SideLower {
            self.keys("Tab").await;
        }
        let target = self
            .core
            .state
            .tree_rows()
            .iter()
            .position(|r| match r {
                model::TreeRow::Task { id, .. } => self
                    .core
                    .state
                    .conv_tasks()
                    .iter()
                    .any(|t| &t.id == id && t.label == label),
                _ => false,
            })
            .unwrap_or_else(|| panic!("树里没有 {label}"));
        self.step_to(target, false).await;
    }

    /// 右栏上半 / 下半选中第 `i` 行（用上下键走过去，和人操作一样）。
    pub async fn pick_upper(&mut self, i: usize) {
        if self.core.state.ui.focus != model::Focus::SideUpper {
            self.keys("Tab").await;
        }
        self.step_to(i, true).await;
        self.settle().await;
    }

    pub async fn pick_lower(&mut self, i: usize) {
        if self.core.state.ui.focus != model::Focus::SideLower {
            self.keys("Tab").await;
        }
        self.step_to(i, false).await;
        self.settle().await;
    }

    /// 当前对话区显示的会话标题。
    pub fn chat_title(&self) -> String {
        self.core
            .state
            .conv()
            .map(|c| c.summary.title.clone())
            .unwrap_or_default()
    }

    async fn step_to(&mut self, target: usize, upper: bool) {
        for step in 0.. {
            assert!(
                step < 500,
                "走不到第 {target} 行：上半 {:?} 下半 {:?} 焦点 {:?} 下半行数 {}",
                self.core.state.ui.upper(),
                self.core.state.ui.lower(),
                self.core.state.ui.focus,
                self.core.state.lower_len_pub()
            );
            let cur = if upper {
                self.core.state.ui.upper().selected
            } else {
                self.core.state.ui.lower().selected
            };
            if cur == target {
                return;
            }
            self.keys(if cur < target { "Down" } else { "Up" }).await;
        }
    }

    /// 当前会话转录的全部文字（已排好的行，不受视口限制）。
    pub fn transcript(&self) -> String {
        self.core
            .state
            .conv()
            .map(|c| {
                c.lines()
                    .iter()
                    .map(|l| l.text())
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .unwrap_or_default()
    }

    /// 对话区顶部面板的全部内容（不受面板高度限制）；收起时为空。
    /// 右下框里看得见的行。
    pub fn lower(&mut self) -> String {
        let tui::Body::Workbench(w) = self.core.frame().body else {
            panic!("不在工作台")
        };
        w.side
            .map(|s| s.lower.rows.iter().map(|l| l.text() + "\n").collect())
            .unwrap_or_default()
    }

    pub fn panel(&self) -> String {
        self.core
            .state
            .panel_content(200)
            .iter()
            .map(|l| l.text())
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn calls(&self, instance: &str) -> Vec<port::Command> {
        self.instances
            .backend(instance)
            .map(|b| b.calls().lock().unwrap().clone())
            .unwrap_or_default()
    }

    pub fn last_line(&mut self) -> String {
        self.text().lines().last().unwrap_or_default().to_string()
    }
}
