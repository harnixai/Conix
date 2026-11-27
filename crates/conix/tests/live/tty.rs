//! LIVE 的真终端一层（testing.md §8）：伪终端里起真的 `conix` 二进制，像人那样敲键盘、看屏幕。
//! `live/tui.rs` 在进程里驱动 `app::Core`，看得到 model；这一层只有终端：按键编成终端发的字节写进去，
//! 屏幕由 `vt100` 照 `conix` 写出来的 VT 序列还原。实例由这个 `conix` 照用户的路子拉起（`conix new --name lv`）
//! 或接入（`conix attach lv`），家目录、项目、凭据与端口驱动的 harness 是同一个 [`Live`]。
//!
//! 写 TTY 用例的规矩：
//!
//! - **动作只走键盘**：说话、斜杠命令（`/confirm`、`/reject`、`/rework`、`/worktree`、`/trust`、
//!   `/flow:<包> …`）、键（`^R` 找到等人的地方、提问框里 `Enter` 批准、`^X` 退出……）。不发端口命令。
//! - **观察主要看屏幕**：[`Tty::wait`] 等屏幕上出现某样东西。屏幕上没有稳定的字可等时，给界面加一个
//!   小标记（只改界面、不改协议，文字进 i18n），不去读 model。
//! - 要做决定时可以读项目里的文件（人也会去看仓库）：报告写成什么样、改了哪些文件；用例最后的断言落在
//!   项目的最终状态上（交付物、归档），断言粗一些——细的机制断言留在端口驱动的 LIVE 里。
//! - 人在 Conix 之外做的事（改治理文件、在仓库里提交）就是普通的文件读写与命令。
//!
//! 等待用墙上时间，一次可以等很久（xforge 的一站能跑几十分钟）：每过一分钟往 stderr 打一行进度。等不到、
//! `conix` 自己退了、或者用例 panic，都把屏幕、做过的动作、实例 `host.log` 的末尾、运行记录的状态打出来。
//! 用例结束时 [`Tty::quit`] 经界面退出并停掉实例；没走到那一步，`Drop` 杀掉 `conix`，[`Live`] 的 `Drop`
//! 停实例，不留进程。

use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use super::{Live, NAME};

pub const COLS: u16 = 160;
pub const ROWS: u16 = 45;
/// 两次进度之间：长的等待每过这么久报一次在等什么、屏幕底下是什么。
const PROGRESS: Duration = Duration::from_secs(60);
/// 屏幕这么久没有新输出才算画完一帧：ratatui 一帧分几次写，半帧的屏幕不拿来判断。
const QUIET: Duration = Duration::from_millis(40);
/// 两个按键之间：Esc 后面紧跟别的字节会被当成 Alt 组合，界面也要时间处理上一个键。
const KEY_GAP: Duration = Duration::from_millis(60);

// 屏幕上可等的字（i18n 的 zh，`CONIX_LANG=zh`）。界面改了这几处的字，这里跟着改。
/// 「离开期间」浮层的标题（`view.away.title`）。
pub const AWAY: &str = "离开期间";
/// 提问框拿着键盘时的键位行（`view.ask.keys_active`）：Enter 就是批准（第一项「是」）。
pub const ASK_ACTIVE: &str = "Esc 拒绝 · ↑↓ / 数字选择 · Enter 确认";
/// 左边会话的状态（`view.badge.*`），在对话区标题线的右端。
pub const RUNNING: &str = "● 运行中";
pub const WAITING: &str = "◐ 等你";
pub const IDLE: &str = "○ 空闲";
/// 底栏的待处理计数（^R 那一项，`keys.*.next_attention`）：后面跟着 `(n)`。
pub const ATTENTION: &str = "下一处待处理(";
/// 流程标签里运行在等人确认（`flows.run.waiting_confirm`）。
pub const WAITS_CONFIRM: &str = "等你确认";

/// 人在另一个终端里敲 `conix list --all` 看到的实例 `lv` 的状态（`Running`、`Stopped`……）；列表里没有它是
/// `None`。
pub fn listed_state(lv: &Live) -> Option<String> {
    let o = std::process::Command::new(env!("CARGO_BIN_EXE_conix"))
        .args(["list", "--all"])
        .current_dir(lv.project_path())
        .env("CONIX_HOME", lv.home.path())
        .env("CONIX_LANG", "en")
        .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
        .output()
        .unwrap();
    String::from_utf8_lossy(&o.stdout).lines().find_map(|l| {
        let mut w = l.split_whitespace();
        (w.next() == Some(NAME)).then(|| w.next().unwrap_or_default().to_string())
    })
}

/// 主模型用 Claude 的 [`Live`]（`.env` 没有 Claude 的 key 就是 `None`，用例跳过）：provider 排在前面的是
/// Claude，DeepSeek 在后面给要换一家模型的站用（`model: <DeepSeek 的模型>`）。
pub fn on_claude(mut opts: super::Opts) -> Option<Live> {
    let cl = super::claude()?;
    let ds = super::deepseek();
    let one = |id: &str, e: &super::Endpoint| {
        serde_json::json!({
            "id": id, "vendor": "anthropic", "api": "anthropic",
            "base_url": e.base_url, "api_key_env": e.key_env, "models": [e.model],
            "tiers": {"strong": e.model, "balanced": e.model, "fast": e.model},
        })
    };
    let mut settings = serde_json::json!({
        "providers": [one("claude", &cl), one("deepseek", &ds)],
        "model": {"order": ["claude", "deepseek"]},
    });
    if let Some(serde_json::Value::Object(extra)) = opts.settings.take() {
        for (k, v) in extra {
            settings[k] = v;
        }
    }
    opts.settings = Some(settings);
    Some(Live::with(opts))
}

/// 终端里的一个 `conix`。
pub struct Tty<'a> {
    pub lv: &'a Live,
    child: Box<dyn Child + Send + Sync>,
    _master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    term: Arc<Mutex<Term>>,
    begun: Instant,
    /// 做过的动作（带时刻），失败时打出来。
    trail: Vec<String>,
    /// 已经打过一次现场：panic 时 `Drop` 不再重复。
    dumped: bool,
    exited: bool,
}

struct Term {
    parser: vt100::Parser,
    last_output: Instant,
    eof: bool,
}

/// [`Tty::drive`] 的一步：接着等，或者等到了。
pub enum Step<T> {
    Wait,
    Done(T),
}

impl<'a> Tty<'a> {
    /// 像用户在项目目录里敲 `conix new --name lv`：新建实例 `lv` 并接入，等到连上。
    pub fn new(lv: &'a Live) -> Tty<'a> {
        let mut t = Tty::spawn(lv, &["new", "--name", NAME]);
        t.wait_attached();
        t
    }

    /// 像 `conix attach lv`：接入已有的实例。
    pub fn attach(lv: &'a Live) -> Tty<'a> {
        let mut t = Tty::spawn(lv, &["attach", NAME]);
        t.wait_attached();
        t
    }

    /// 在项目目录里跑 `conix <args>`，不等任何东西（`conix` 不带参数时自己挑实例）。
    pub fn spawn(lv: &'a Live, args: &[&str]) -> Tty<'a> {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: ROWS,
                cols: COLS,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("开伪终端");
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_conix"));
        cmd.args(args);
        cmd.cwd(lv.project_path());
        cmd.env("CONIX_HOME", lv.home.path());
        cmd.env("CONIX_LANG", "zh");
        cmd.env("TERM", "xterm-256color");
        // 同 `SocketInstances`：开发时的加载器回退路径让 macOS 上每次 exec 慢十几秒，实例会继承它。
        cmd.env_remove("DYLD_FALLBACK_LIBRARY_PATH");
        // 凭据与模型交给 `conix`：它拉起的实例继承它的环境，和用户在自己的 shell 里跑一样。
        for (k, v) in lv.instance_env() {
            cmd.env(k, v);
        }
        let child = pty.slave.spawn_command(cmd).expect("起 conix");
        drop(pty.slave);
        let mut reader = pty.master.try_clone_reader().unwrap();
        let writer = pty.master.take_writer().unwrap();
        let term = Arc::new(Mutex::new(Term {
            parser: vt100::Parser::new(ROWS, COLS, 0),
            last_output: Instant::now(),
            eof: false,
        }));
        {
            let term = term.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 8192];
                loop {
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let mut t = term.lock().unwrap();
                            t.parser.process(&buf[..n]);
                            t.last_output = Instant::now();
                        }
                    }
                }
                term.lock().unwrap().eof = true;
            });
        }
        let mut t = Tty {
            lv,
            child,
            _master: pty.master,
            writer,
            term,
            begun: Instant::now(),
            trail: vec![],
            dumped: false,
            exited: false,
        };
        t.record(format!("conix {}", args.join(" ")));
        t
    }

    fn wait_attached(&mut self) {
        // 连上之后右上角是"已连接"、底栏是基本键位；实例第一次起来要几秒。
        self.wait("接入实例", Duration::from_secs(60), |s| {
            s.contains("● 已连接") && s.contains("^X 退出")
        });
        // 接入之后实例马上推来的（「离开期间」浮层、会话列表）先画出来：浮层没画出来就敲键，键会落进
        // 一个人看不见的浮层里（Enter 是「去处理」），而人总是先看见它再动手。
        std::thread::sleep(Duration::from_millis(1500));
        self.settle();
    }

    /// 「离开期间」浮层开着就按 Esc 关掉，回关没关。实例在没人接入时攒下了要说的事（作业失败、项目状态
    /// 挪走了……），接入时先弹这个。
    pub fn close_away(&mut self) -> bool {
        if !self.contains(AWAY) {
            return false;
        }
        self.keys("Esc");
        self.wait("关掉「离开期间」", Duration::from_secs(10), |s| {
            !s.contains(AWAY)
        });
        true
    }

    fn elapsed(&self) -> String {
        let s = self.begun.elapsed().as_secs();
        format!("+{:02}:{:02}", s / 60, s % 60)
    }

    fn record(&mut self, what: String) {
        let at = self.elapsed();
        self.trail.push(format!("{at} {what}"));
    }

    fn send(&mut self, bytes: &[u8]) {
        if let Err(e) = self
            .writer
            .write_all(bytes)
            .and_then(|_| self.writer.flush())
        {
            self.fail(&format!("写不进终端：{e}"));
        }
    }

    /// 空格分隔的按键，写法同键位配置（`keys::Key::parse`）：`"^G ^G Enter"`、`"Esc"`、`"Up"`、
    /// `"Shift+Tab"`、`"Alt+Enter"`；字面空格写 `Space`。终端分不出的几对照终端的样子发：`^M` 就是 Enter、
    /// `^I` 就是 Tab。
    pub fn keys(&mut self, seq: &str) {
        self.record(format!("keys {seq}"));
        for k in seq.split_whitespace() {
            let key = keys::Key::parse(k).unwrap_or_else(|| panic!("按键写错了：{k}"));
            let bytes = encode(key);
            self.send(&bytes);
            std::thread::sleep(KEY_GAP);
        }
        self.settle();
    }

    /// 在输入框里打字（UTF-8 原样写进去，没有输入法这一层）。换行写成 `Alt+Enter`：直接的 `\r`
    /// 是 Enter，会把这一行发出去。
    pub fn type_text(&mut self, text: &str) {
        self.record(format!("type {text:?}"));
        for (i, part) in text.split('\n').enumerate() {
            if i > 0 {
                self.send(&encode(keys::Key::parse("Alt+Enter").unwrap()));
                std::thread::sleep(KEY_GAP);
            }
            self.send(part.as_bytes());
        }
        self.settle();
    }

    /// 打一行字并回车：说话、斜杠命令都是它。
    pub fn line(&mut self, text: &str) {
        self.type_text(text);
        self.keys("Enter");
    }

    /// 等终端安静下来（没有新输出）一会儿，至多一秒：刚做的动作画到屏幕上了再往下看。
    pub fn settle(&mut self) {
        let end = Instant::now() + Duration::from_secs(1);
        while Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
            if self.term.lock().unwrap().last_output.elapsed() >= Duration::from_millis(150) {
                break;
            }
        }
    }

    /// 现在的屏幕：45 行，每行去掉行尾空白。
    pub fn screen(&self) -> String {
        self.term.lock().unwrap().parser.screen().contents()
    }

    /// 屏幕的一行（从 0 数）。
    pub fn row(&self, n: u16) -> String {
        let t = self.term.lock().unwrap();
        t.parser
            .screen()
            .rows(0, COLS)
            .nth(n as usize)
            .unwrap_or_default()
    }

    /// 屏幕最底下那一行（底栏：键位提示、提示语）。
    pub fn last_line(&self) -> String {
        self.row(ROWS - 1)
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.screen().contains(needle)
    }

    /// 左边会话的状态：[`RUNNING`]、[`WAITING`]、[`IDLE`] 之一；草稿（还没有会话）是 `None`。
    pub fn chat_state(&self) -> Option<&'static str> {
        let title = self.row(1);
        // 第 1 行是左右两个框的上边：只看左边那个（右边会话列表里也有同样的圆点）。
        let left = title.split("╮╭").next().unwrap_or_default();
        [RUNNING, WAITING, IDLE]
            .into_iter()
            .find(|b| left.contains(b))
    }

    /// 底栏上「^R 下一处待处理(n)」的 n（焦点在输入框时才有这一项）。
    pub fn attention(&self) -> Option<u32> {
        let l = self.last_line();
        let rest = l.split(ATTENTION).nth(1)?;
        rest.split(')').next()?.parse().ok()
    }

    /// 屏幕上 `before` 与 `after` 之间的字：从提示里取运行的 id 这类（`已启动「…」（xreport-1）`）。
    pub fn between(&self, before: &str, after: &str) -> Option<String> {
        let s = self.screen();
        let rest = s.split(before).nth(1)?;
        Some(rest.split(after).next()?.to_string())
    }

    /// 提问框开着、拿着键盘（输入框空着、焦点在左边）。
    pub fn asking(&self) -> bool {
        self.contains(ASK_ACTIVE)
    }

    /// 说一句话（或者发一条要模型干活的斜杠命令），等左边的会话动起来、再停下来，回那时的屏幕。
    /// 中途冒出来的权限询问一律批准（提问框里 Enter，第一项是「是」）；要自己答的用 [`Tty::line`] 加
    /// [`Tty::drive`]。
    pub fn say(&mut self, text: &str, limit: Duration) -> String {
        self.line(text);
        // 一轮至少一两秒，100ms 一看不会错过"在跑"；草稿上的第一句话要先建出会话。
        self.drive("会话开始跑", Duration::from_secs(60), |t, _| {
            match t.chat_state() {
                Some(IDLE) | None => Step::Wait,
                Some(_) => Step::Done(()),
            }
        });
        self.wait_idle(&format!("「{text}」这一轮跑完"), limit, true)
    }

    /// 等左边的会话停下来（[`IDLE`]）。`approve` 时冒出来的权限询问按 Enter 批准，否则留着等（等到超时）。
    pub fn wait_idle(&mut self, what: &str, limit: Duration, approve: bool) -> String {
        self.drive(what, limit, |t, s| {
            if approve && s.contains(ASK_ACTIVE) {
                t.record("提问框：Enter 批准".into());
                t.keys("Enter");
                return Step::Wait;
            }
            match t.chat_state() {
                Some(IDLE) => Step::Done(s.to_string()),
                _ => Step::Wait,
            }
        })
    }

    /// `conix` 退出了（终端那头读到了结尾）。
    pub fn has_exited(&self) -> bool {
        self.term.lock().unwrap().eof
    }

    /// 等屏幕满足 `ok`，返回那时的屏幕。`limit` 内没等到、或者 `conix` 退了，就带着现场失败。
    pub fn wait(
        &mut self,
        what: &str,
        limit: Duration,
        mut ok: impl FnMut(&str) -> bool,
    ) -> String {
        self.drive(what, limit, |_, s| {
            if ok(s) {
                Step::Done(s.to_string())
            } else {
                Step::Wait
            }
        })
    }

    /// 等屏幕上出现 `needle`。
    pub fn wait_text(&mut self, what: &str, limit: Duration, needle: &str) -> String {
        self.wait(what, limit, |s| s.contains(needle))
    }

    /// 等屏幕上出现几样里的某一样，回是第几样：一个动作的结果有几种可能时（批准提问、或者已经跑完）。
    pub fn wait_any(&mut self, what: &str, limit: Duration, needles: &[&str]) -> usize {
        self.drive(what, limit, |_, s| {
            match needles.iter().position(|n| s.contains(n)) {
                Some(i) => Step::Done(i),
                None => Step::Wait,
            }
        })
    }

    /// 长的、要中途动手的等待：每画完一帧把屏幕交给 `step`，它可以按键（批准冒出来的权限询问、答问题），
    /// 回 `Step::Done` 就结束。`limit` 可以是一个钟头：每分钟打一行进度。
    pub fn drive<T>(
        &mut self,
        what: &str,
        limit: Duration,
        mut step: impl FnMut(&mut Self, &str) -> Step<T>,
    ) -> T {
        let start = Instant::now();
        let mut next_report = start + PROGRESS;
        let mut last_look = Instant::now() - Duration::from_secs(1);
        loop {
            let (quiet, eof) = {
                let t = self.term.lock().unwrap();
                (t.last_output.elapsed() >= QUIET, t.eof)
            };
            // 一直在画（转圈、流式的字）也至少每秒看一次。
            if quiet || last_look.elapsed() >= Duration::from_secs(1) {
                last_look = Instant::now();
                let s = self.screen();
                if let Step::Done(v) = step(self, &s) {
                    return v;
                }
            }
            if eof {
                self.fail(&format!("等「{what}」时 conix 退出了"));
            }
            let now = Instant::now();
            if now.duration_since(start) >= limit {
                self.fail(&format!("「{what}」：{limit:?} 内没等到"));
            }
            if now >= next_report {
                next_report = now + PROGRESS;
                let waited = now.duration_since(start).as_secs();
                eprintln!(
                    "[tty {}] 在等「{what}」：已 {}m{:02}s / {}m · 底栏：{}",
                    self.elapsed(),
                    waited / 60,
                    waited % 60,
                    limit.as_secs() / 60,
                    self.last_line().trim()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// 带着现场失败：屏幕、做过的动作、实例日志的末尾、运行记录的状态。
    pub fn fail(&mut self, why: &str) -> ! {
        self.dumped = true;
        panic!("{why}\n{}", self.dump());
    }

    pub fn dump(&self) -> String {
        let mut out = format!(
            "==== 屏幕（{}）====\n{}\n==== 做过的动作 ====\n{}\n",
            self.elapsed(),
            frame(&self.screen()),
            self.trail.join("\n")
        );
        let log = self.lv.host_log();
        let lines: Vec<&str> = log.lines().collect();
        out.push_str(&format!(
            "==== host.log（末尾 {} 行，共 {} 行）====\n{}\n",
            lines.len().min(60),
            lines.len(),
            lines[lines.len().saturating_sub(60)..].join("\n")
        ));
        let runs = self.runs();
        if !runs.is_empty() {
            out.push_str(&format!("==== 运行 ====\n{}\n", runs.join("\n")));
        }
        out
    }

    /// 项目里每次运行的记录：运行、状态、各站的状态（失败时看卡在哪一站）。
    fn runs(&self) -> Vec<String> {
        let dir: PathBuf = self.lv.state().flow_runs();
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return vec![];
        };
        let mut out = vec![];
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "json") {
                continue;
            }
            let Some(v) = std::fs::read_to_string(&p)
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
            else {
                continue;
            };
            let stations: Vec<String> = v["stations"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .map(|(k, st)| format!("{k}={}", st["status"].as_str().unwrap_or("?")))
                        .collect()
                })
                .unwrap_or_default();
            out.push(format!(
                "{} {} [{}]",
                p.file_stem().unwrap_or_default().to_string_lossy(),
                v["status"],
                stations.join(" ")
            ));
        }
        out.sort();
        out
    }

    /// 经界面退出并停掉实例：^X，选「直接退出」，等 `conix` 退出。
    pub fn quit(mut self) {
        self.keys("^X");
        self.wait_text("退出的选择", Duration::from_secs(10), "要怎么退出？");
        self.keys("Down Enter");
        self.wait_exit();
    }

    /// 经界面脱离：^X，选「后台运行」，实例接着跑。
    pub fn detach(mut self) {
        self.keys("^X");
        self.wait_text("退出的选择", Duration::from_secs(10), "要怎么退出？");
        self.keys("Enter");
        self.wait_exit();
    }

    fn wait_exit(&mut self) {
        let end = Instant::now() + Duration::from_secs(30);
        while Instant::now() < end {
            if let Ok(Some(_)) = self.child.try_wait() {
                self.exited = true;
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        self.fail("conix 30 秒内没有退出");
    }
}

impl Drop for Tty<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() && !self.dumped {
            eprintln!("{}", self.dump());
        }
        if !self.exited {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// 屏幕加上框和行号，空白行也看得出来。
fn frame(s: &str) -> String {
    let mut lines: Vec<&str> = s.split('\n').collect();
    lines.resize(ROWS as usize, "");
    lines
        .iter()
        .enumerate()
        .map(|(i, l)| format!("{i:2}│{l}"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// 一个键在 xterm 里发出的字节（crossterm 照这个解析）。
fn encode(k: keys::Key) -> Vec<u8> {
    use keys::KeyCode as K;
    let m = k.mods;
    // CSI 的修饰参数：1 + Shift(1) + Alt(2) + Ctrl(4)。
    let param = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let csi = |last: char| -> Vec<u8> {
        if param == 1 {
            format!("\x1b[{last}").into_bytes()
        } else {
            format!("\x1b[1;{param}{last}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if param == 1 {
            format!("\x1b[{n}~").into_bytes()
        } else {
            format!("\x1b[{n};{param}~").into_bytes()
        }
    };
    let alt = |mut b: Vec<u8>| -> Vec<u8> {
        if m.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    match k.code {
        K::Char(c) if m.ctrl => {
            let b = match c.to_ascii_lowercase() {
                c @ 'a'..='z' => c as u8 - b'a' + 1,
                ' ' | '@' | '2' => 0,
                '[' | '3' => 0x1b,
                '\\' | '4' => 0x1c,
                ']' | '5' => 0x1d,
                '^' | '6' => 0x1e,
                '_' | '7' | '/' => 0x1f,
                other => panic!("终端发不出 Ctrl+{other}"),
            };
            alt(vec![b])
        }
        K::Char(c) => alt(c.to_string().into_bytes()),
        K::Enter => alt(b"\r".to_vec()),
        K::Esc => alt(vec![0x1b]),
        K::Tab if m.shift => b"\x1b[Z".to_vec(),
        K::Tab => alt(b"\t".to_vec()),
        K::BackTab => b"\x1b[Z".to_vec(),
        K::Backspace => alt(vec![0x7f]),
        K::Up => csi('A'),
        K::Down => csi('B'),
        K::Right => csi('C'),
        K::Left => csi('D'),
        K::Home => csi('H'),
        K::End => csi('F'),
        K::Insert => tilde(2),
        K::Delete => tilde(3),
        K::PageUp => tilde(5),
        K::PageDown => tilde(6),
        K::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        K::F(n) => {
            let code = match n {
                5 => 15,
                6 => 17,
                7 => 18,
                8 => 19,
                9 => 20,
                10 => 21,
                11 => 23,
                12 => 24,
                _ => panic!("没有 F{n}"),
            };
            tilde(code)
        }
    }
}
