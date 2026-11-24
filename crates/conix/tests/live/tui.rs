//! LIVE 的界面一层（testing.md §8）：真的 `app::Core`——键位、model、视图——经 `SocketInstances`
//! 接真实实例。mock 测试证明界面对 mock 做得对，这一层证明同样的按键在真实后端上走得通：
//! 回复、事件、命令的结果是真实例给的。
//!
//! 时钟是墙上时间：不像 mock 测试那样拨表，而是等——[`Tui::until`] 一边收实例的消息、一边
//! 按 model 给的时刻 Tick，直到条件成立或超时。

use std::sync::Arc;
use std::time::{Duration, Instant};

use app::Core;
use app::core::{ShellEffect, Start};
use app::snapshot::render_text;
use model::{InstOp, Msg};
use tui::Size;

use super::{Live, NAME};

pub const SIZE: Size = Size::new(160, 45);

pub struct Tui {
    pub core: Core,
    pub size: Size,
    pub shell: Vec<ShellEffect>,
}

impl Tui {
    /// 像 `conix new lv` 那样：经界面新建实例 `lv`（项目是这个用例的示例项目）并接入。
    pub async fn create(lv: &Live) -> Tui {
        Tui::start(
            lv,
            Start::Op(InstOp::Create {
                name: Some(NAME.into()),
                project: Some(lv.project_path()),
            }),
        )
        .await
    }

    /// 像 `conix attach lv` 那样接入已有的实例。
    pub async fn attach(lv: &Live) -> Tui {
        Tui::start(
            lv,
            Start::Op(InstOp::Attach {
                name: NAME.into(),
                force: false,
            }),
        )
        .await
    }

    async fn start(lv: &Live, start: Start) -> Tui {
        let (core, shell) = Core::new(
            lv.instances.clone(),
            Arc::new(keys::Keymap::default()),
            SIZE,
            Box::new(super::now_ms),
            start,
            model::Settings::default(),
        );
        let mut t = Tui {
            core,
            size: SIZE,
            shell,
        };
        t.until("接入实例", super::QUICK, |t| {
            t.core.attached() && t.core.state.domain.instance.is_some()
        })
        .await;
        t
    }

    /// 处理已经到了的消息，到期的定时 Tick 一下。
    fn drive(&mut self) {
        let s = self.core.pump();
        self.shell.extend(s);
        if self
            .core
            .deadline()
            .is_some_and(|d| d <= self.core.now_ms())
        {
            let s = self.core.tick();
            self.shell.extend(s);
        }
    }

    /// 等到 `ok` 成立；`limit` 内没等到就带着当前屏幕失败。
    pub async fn until(
        &mut self,
        what: &str,
        limit: Duration,
        mut ok: impl FnMut(&mut Tui) -> bool,
    ) {
        let end = Instant::now() + limit;
        loop {
            self.drive();
            if ok(self) {
                return;
            }
            if Instant::now() >= end {
                panic!("{what}：{limit:?} 内没等到\n{}", self.text());
            }
            if let Ok(s) =
                tokio::time::timeout(Duration::from_millis(100), self.core.next_msg()).await
            {
                self.shell.extend(s);
            }
        }
    }

    /// 让实例的消息进来一会儿（不等条件）：发出去的命令有了回复再看下一步。
    pub async fn settle(&mut self) {
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            self.drive();
            if let Ok(s) =
                tokio::time::timeout(Duration::from_millis(50), self.core.next_msg()).await
            {
                self.shell.extend(s);
            }
        }
        self.drive();
    }

    /// 空格分隔的按键：`"^G ^G Enter"`；字面空格写 `Space`。
    pub async fn keys(&mut self, seq: &str) {
        for k in seq.split_whitespace() {
            let key = keys::Key::parse(k).unwrap_or_else(|| panic!("按键写错了：{k}"));
            let s = self.core.key(key);
            self.shell.extend(s);
            self.drive();
        }
        self.settle().await;
    }

    /// 在输入框里打字（逐字，等同键盘输入）。
    pub async fn type_str(&mut self, text: &str) {
        for c in text.chars() {
            let s = self.core.handle(Msg::Text(c));
            self.shell.extend(s);
        }
        self.drive();
    }

    /// 打一行字并回车。
    pub async fn say(&mut self, text: &str) {
        self.type_str(text).await;
        self.keys("Enter").await;
    }

    pub fn text(&mut self) -> String {
        let f = self.core.frame();
        render_text(&f, self.size)
    }

    pub fn last_line(&mut self) -> String {
        self.text().lines().last().unwrap_or_default().to_string()
    }

    /// 左边会话的转录（屏幕上看得见的那段之外也算：取 model 里的全部行）。
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

    /// 左边那个会话这一轮跑完了。
    pub fn idle(&self) -> bool {
        self.core.state.conv().is_some_and(|c| !c.running)
    }
}
