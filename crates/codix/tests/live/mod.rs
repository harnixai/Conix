//! LIVE 测试的共用部分（testing.md §8）：真实的 `codix host` 进程、真实的 Agix 与模型，
//! 从 TUI 走的那条路接入——`SocketInstances` 拉起与接入，`SocketBackend` 收发 `port` 的命令与
//! 事件。只测 Codix 自己做的事（实例、接入、会话代理、作业……），Agix 的特性它自己测。
//!
//! 每个用例一个 [`Live`]：独立的 `CODIX_HOME`（放在 /tmp 下的短路径，socket 路径有 104 字节的
//! 上限）、一个 git 示例项目、写好提问超时的 Agix `settings.json`。凭据从仓库根的 `.env` 读，只交给
//! 实例进程的环境，不进测试进程、不打印。用例结束不论成败都停掉它起的实例。

#![allow(dead_code)]

pub mod tui;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use client::{Layout, ProjectState, SocketInstances};
use port::{
    Answer, Attach, Backend, Command, Event, Instances, JobEntry, NewInstance, PortError, Prompt,
    PromptKind, Reply, SessionId, SessionSummary, StopReason, SubmitMode, Usage, Watched,
};
use tokio::sync::Notify;

pub const NAME: &str = "lv";
/// 等一轮对话的上限：模型偶尔慢，但超过它多半是卡住了。
pub const TURN: Duration = Duration::from_secs(180);
/// 等不靠模型的事件（列表变化、连接事件）的上限。
pub const QUICK: Duration = Duration::from_secs(30);

/// 仓库根 `.env` 里的变量（`CODIX_LIVE_ENV` 可指定别的文件）。worktree 里没有 `.env` 时往上找，
/// 所以 `.claude/worktrees/*` 里也能跑。
pub fn credentials() -> &'static BTreeMap<String, String> {
    static ENV: OnceLock<BTreeMap<String, String>> = OnceLock::new();
    ENV.get_or_init(|| {
        let path = std::env::var_os("CODIX_LIVE_ENV")
            .map(PathBuf::from)
            .or_else(|| {
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .ancestors()
                    .map(|d| d.join(".env"))
                    .find(|p| p.is_file())
            })
            .unwrap_or_else(|| {
                panic!("LIVE 测试要凭据：仓库根放一个 .env（或用 CODIX_LIVE_ENV 指定文件）")
            });
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("读不了 {}：{e}", path.display()));
        let vars: BTreeMap<String, String> = text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .filter_map(|l| l.strip_prefix("export ").unwrap_or(l).split_once('='))
            .map(|(k, v)| {
                (
                    k.trim().to_string(),
                    v.trim().trim_matches(['"', '\'']).to_string(),
                )
            })
            .collect();
        let has_key = ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY"]
            .iter()
            .any(|k| vars.get(*k).is_some_and(|v| !v.is_empty()));
        assert!(
            has_key && vars.contains_key("ANTHROPIC_MODEL"),
            "{} 里要有 ANTHROPIC_AUTH_TOKEN（或 ANTHROPIC_API_KEY）与 ANTHROPIC_MODEL",
            path.display()
        );
        vars
    })
}

pub fn model() -> String {
    credentials()["ANTHROPIC_MODEL"].clone()
}

fn codix() -> &'static str {
    env!("CARGO_BIN_EXE_codix")
}

/// 刚链接出的二进制第一次执行要被 macOS 扫描，先跑一次，免得算进实例的启动时间。
fn prewarm() {
    static DONE: OnceLock<()> = OnceLock::new();
    DONE.get_or_init(|| {
        let _ = std::process::Command::new(codix())
            .arg("--version")
            .output();
    });
}

pub fn now_ms() -> u64 {
    client::now_ms()
}

/// 一个用例的世界：家目录、示例项目、拉起实例用的 `SocketInstances`。
pub struct Live {
    pub home: tempfile::TempDir,
    pub project: tempfile::TempDir,
    pub instances: Arc<SocketInstances>,
    extra_env: Vec<(String, String)>,
}

#[derive(Default)]
pub struct Opts {
    /// 写进 `codix.yaml` 的内容（实例的配置，比如空闲超时）。
    pub codix_yaml: Option<String>,
    /// 只给实例进程的额外环境变量（比如缩短心跳超时）。
    pub env: Vec<(String, String)>,
}

impl Live {
    pub fn new() -> Live {
        Live::with(Opts::default())
    }

    pub fn with(opts: Opts) -> Live {
        prewarm();
        let home = tempfile::Builder::new()
            .prefix("cxl")
            .tempdir_in("/tmp")
            .unwrap();
        // 提问超时放长，等人回答的不会自己撤掉。模型不写设置：0.4 里没定义 providers 时，
        // 自带的 anthropic 用进程参数取模型，由 CODIX_MODEL 交给实例进程（见下面的 env）。
        std::fs::write(
            home.path().join("settings.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "permissions": {"prompt_timeout_secs": 600},
            }))
            .unwrap(),
        )
        .unwrap();
        if let Some(y) = &opts.codix_yaml {
            std::fs::write(home.path().join("codix.yaml"), y).unwrap();
        }
        let project = new_project("app");
        let mut env: Vec<(String, String)> = credentials()
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env.push(("CODIX_MODEL".to_string(), model()));
        env.extend(opts.env.iter().cloned());
        let instances =
            Arc::new(SocketInstances::new(Layout::new(home.path()), codix()).with_env(env.clone()));
        Live {
            home,
            project,
            instances,
            extra_env: env,
        }
    }

    pub fn layout(&self) -> Layout {
        Layout::new(self.home.path())
    }

    pub fn project_path(&self) -> String {
        self.project
            .path()
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    /// 新建实例 `lv`（项目是这个用例的示例项目）并接入。
    pub async fn start(&self) -> Client {
        self.create(NAME, &self.project_path()).await
    }

    /// 起实例并信任它的项目：作业定义随项目提交，项目受信才读、才写（architecture §8.5）。
    /// 信任由 Agix 记着，重启之后照旧。
    pub async fn start_trusted(&self) -> Client {
        let c = self.start().await;
        c.ok(Command::TrustProject { trusted: true }).await;
        c
    }

    pub async fn create(&self, name: &str, project: &str) -> Client {
        let (_, b) = self
            .instances
            .create(NewInstance {
                name: Some(name.into()),
                project: Some(project.into()),
            })
            .await
            .unwrap_or_else(|e| panic!("新建实例 {name}：{e}"));
        Client::new(b, self.layout(), name)
    }

    pub async fn attach(&self, opts: Attach) -> Result<Client, PortError> {
        self.attach_to(NAME, opts).await
    }

    pub async fn attach_to(&self, name: &str, opts: Attach) -> Result<Client, PortError> {
        let b = self.instances.attach(name, opts).await?;
        Ok(Client::new(b, self.layout(), name))
    }

    pub async fn stop(&self) {
        self.instances.stop(NAME).await.expect("停实例");
    }

    /// 停掉再照 `meta.json` 拉起（`codix start`），返回新的接入。
    pub async fn restart(&self) -> Client {
        self.stop().await;
        self.start_again().await
    }

    pub async fn start_again(&self) -> Client {
        let b = self.instances.start(NAME).await.expect("启动实例");
        Client::new(b, self.layout(), NAME)
    }

    pub fn instance_dir(&self) -> PathBuf {
        self.layout().instance(NAME)
    }

    /// 项目里 Codix 的东西：`state/codix/`、`.codix/jobs.json`。
    pub fn state(&self) -> ProjectState {
        ProjectState::new(self.project_path())
    }

    /// 实例目录下的 JSON 文件（`instance.json`、`meta.json`）。
    pub fn read(&self, file: &str) -> serde_json::Value {
        read_json_file(&self.instance_dir().join(file))
    }

    /// 项目状态里的 JSON 文件（`sessions.json`、`jobs-state.json`）。
    pub fn read_state(&self, file: &str) -> serde_json::Value {
        read_json_file(&self.state().dir().join(file))
    }

    /// 作业定义：`.codix/jobs.json`。
    pub fn read_jobs(&self) -> serde_json::Value {
        read_json_file(&self.state().jobs())
    }

    /// `jobs/<id>.jsonl`：同一 `fired_ms` 以最后一行为准。
    pub fn job_history_file(&self, job: &str) -> Vec<JobEntry> {
        let p = self.state().job_history(job);
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let mut by: BTreeMap<u64, JobEntry> = BTreeMap::new();
        for l in text.lines().filter(|l| !l.trim().is_empty()) {
            let e: JobEntry = serde_json::from_str(l).unwrap();
            by.insert(e.fired_ms, e);
        }
        by.into_values().collect()
    }

    /// `kill -9` 实例进程（模拟崩溃：不走任何收尾），等它真没了才返回。
    /// `kill -9` 实例进程（模拟崩溃）：看护进程还在，会把它重新拉起来。
    pub async fn kill_host(&self) {
        let pid = self.read("instance.json")["pid"].as_u64().unwrap();
        kill_hard(pid).await;
    }

    /// 连看护进程一起 `kill -9`：没人拉起，留下残留。
    pub async fn kill_host_and_supervisor(&self) {
        let f = self.read("instance.json");
        if let Some(s) = f["supervisor_pid"].as_u64() {
            kill_hard(s).await;
        }
        kill_hard(f["pid"].as_u64().unwrap()).await;
    }

    /// 等看护进程把实例重新拉起来（发现文件里换了 pid、socket 连得上），再接入。
    pub async fn wait_restarted(&self, old_pid: u64) -> Client {
        let deadline = std::time::Instant::now() + Duration::from_secs(60);
        loop {
            let pid = std::fs::read_to_string(self.instance_dir().join("instance.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
                .and_then(|v| v["pid"].as_u64());
            if pid.is_some_and(|p| p != old_pid)
                && let Ok(c) = self.attach(Attach::default()).await
            {
                return c;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "看护进程没把实例拉起来\n{}",
                self.host_log()
            );
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

async fn kill_hard(pid: u64) {
    let pid = pid.to_string();
    {
        let kill = |sig: &str| {
            std::process::Command::new("kill")
                .args([sig, &pid])
                .output()
                .unwrap()
                .status
                .success()
        };
        assert!(kill("-9"), "kill -9 {pid}");
        // `kill` 返回时进程可能还没被回收。
        for _ in 0..100 {
            if !kill("-0") {
                return;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        panic!("实例进程 {pid} 杀不掉");
    }
}

impl Live {
    pub fn host_log(&self) -> String {
        std::fs::read_to_string(self.layout().log(NAME)).unwrap_or_default()
    }

    /// 直接跑 `codix host`（不经 `SocketInstances`）：测项目锁。
    pub fn run_host_directly(&self, name: &str) -> std::process::Output {
        self.run_host_directly_in(name, &self.project_path())
    }

    /// 同上，指定项目：测同名实例的锁（同一个项目会先撞上项目锁）。
    pub fn run_host_directly_in(&self, name: &str, project: &str) -> std::process::Output {
        std::process::Command::new(codix())
            .args(["host", "--name", name, "--project", project])
            .env("CODIX_HOME", self.home.path())
            .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
            .envs(self.extra_env.iter().map(|(k, v)| (k, v)))
            .output()
            .unwrap()
    }
}

impl Drop for Live {
    /// 用例中途失败也要停掉实例：否则留下常驻的 `codix host` 进程。`Drop` 里不能 await，
    /// 所以走命令行的 `codix stop`。
    fn drop(&mut self) {
        let Ok(dir) = std::fs::read_dir(self.layout().instances()) else {
            return;
        };
        for e in dir.flatten() {
            let _ = std::process::Command::new(codix())
                .arg("stop")
                .arg(e.file_name())
                .env("CODIX_HOME", self.home.path())
                .env_remove("DYLD_FALLBACK_LIBRARY_PATH")
                .output();
        }
    }
}

/// 一个 git 示例项目：有一个文件、提交过一次。
fn read_json_file(p: &std::path::Path) -> serde_json::Value {
    let text = std::fs::read_to_string(p).unwrap_or_else(|e| panic!("{}：{e}", p.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}：{e}", p.display()))
}

pub fn new_project(name: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new().prefix(name).tempdir().unwrap();
    std::fs::write(
        dir.path().join("README.md"),
        "# app\n\nA tiny sample project for Codix live tests.\n",
    )
    .unwrap();
    let git = |args: &[&str]| {
        std::process::Command::new("git")
            .args(args)
            .current_dir(dir.path())
            .output()
            .unwrap()
    };
    git(&["init", "-q"]);
    git(&[
        "-c",
        "user.name=live",
        "-c",
        "user.email=live@codix",
        "add",
        ".",
    ]);
    git(&[
        "-c",
        "user.name=live",
        "-c",
        "user.email=live@codix",
        "commit",
        "-q",
        "-m",
        "init",
    ]);
    dir
}

/// 收到提问时怎么办。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answering {
    /// 不答：留给用例自己答。
    Manual,
    /// 权限一律允许，问题选第一个选项（没有选项的答 "ok"）。
    Auto,
    /// 权限一律拒绝（问题同 `Auto`）：用例要确认某件事没被做成时用，比如计划模式里写文件。
    Deny,
}

/// 心跳：像一个开着的 TUI 那样每 10 秒报一次"刚有输入"，长的一轮也不会被实例当成僵死或空闲。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Beat {
    Active,
    None,
}

struct Shared {
    events: Mutex<Vec<(Instant, Event)>>,
    more: Notify,
    answering: Mutex<Answering>,
}

/// 一个接入（相当于一个 TUI）：命令与事件都经它。
pub struct Client {
    pub backend: Arc<dyn Backend>,
    pub name: String,
    layout: Layout,
    shared: Arc<Shared>,
    /// `wait_event` 从这里往后找；找到之后挪到匹配的那条后面。
    cursor: usize,
    beat: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Client {
    pub fn new(backend: Arc<dyn Backend>, layout: Layout, name: &str) -> Client {
        let shared = Arc::new(Shared {
            events: Mutex::default(),
            more: Notify::new(),
            answering: Mutex::new(Answering::Manual),
        });
        let mut rx = backend.events();
        {
            let (shared, backend) = (shared.clone(), Arc::downgrade(&backend));
            tokio::spawn(async move {
                while let Some(ev) = rx.recv().await {
                    let how = *shared.answering.lock().unwrap();
                    if let Event::PromptPending { session, prompt } = &ev
                        && how != Answering::Manual
                        && let Some(b) = backend.upgrade()
                    {
                        let (session, prompt) = (session.clone(), prompt.clone());
                        tokio::spawn(async move {
                            let _ = b
                                .call(Command::Answer {
                                    session,
                                    answer: auto_answer(&prompt, how),
                                    prompt: prompt.id,
                                })
                                .await;
                        });
                    }
                    shared.events.lock().unwrap().push((Instant::now(), ev));
                    shared.more.notify_waiters();
                }
            });
        }
        let c = Client {
            backend,
            name: name.into(),
            layout,
            shared,
            cursor: 0,
            beat: Mutex::new(None),
        };
        c.heartbeat(Beat::Active);
        c
    }

    pub fn answering(&self, a: Answering) {
        *self.shared.answering.lock().unwrap() = a;
    }

    pub fn heartbeat(&self, b: Beat) {
        if let Some(h) = self.beat.lock().unwrap().take() {
            h.abort();
        }
        if b == Beat::Active {
            let backend = Arc::downgrade(&self.backend);
            *self.beat.lock().unwrap() = Some(tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    let Some(b) = backend.upgrade() else { return };
                    let _ = b
                        .call(Command::Heartbeat {
                            last_input_ms: now_ms(),
                        })
                        .await;
                }
            }));
        }
    }

    pub async fn call(&self, cmd: Command) -> Result<Reply, PortError> {
        self.backend.call(cmd).await
    }

    /// 应当成功的命令：失败就让用例失败，并说是哪条命令。
    pub async fn ok(&self, cmd: Command) -> Reply {
        let name = cmd.name();
        self.call(cmd)
            .await
            .unwrap_or_else(|e| panic!("{name} 失败：{e}\n{}", self.recent(20)))
    }

    /// 所有收到的事件（含已经被 `wait_event` 走过的）。
    pub fn events(&self) -> Vec<Event> {
        self.shared
            .events
            .lock()
            .unwrap()
            .iter()
            .map(|(_, e)| e.clone())
            .collect()
    }

    pub fn mark(&self) -> usize {
        self.shared.events.lock().unwrap().len()
    }

    /// 把游标挪回 `mark`：先后不定的两件事（任务帧可能比这一轮的结束晚到，也可能早到）
    /// 分别从同一处等。
    pub fn rewind_to(&mut self, mark: usize) {
        self.cursor = mark;
    }

    /// 把游标挪到现在：之后的 `wait_event` 只看从这一刻起到的事件。
    pub fn skip_past(&mut self) {
        self.cursor = self.mark();
    }

    /// 从游标往后等第一个满足 `want` 的事件，返回它并把游标挪到它后面。等不到就失败，
    /// 附上最近收到的事件。
    pub async fn wait_event<T>(
        &mut self,
        what: &str,
        timeout: Duration,
        mut want: impl FnMut(&Event) -> Option<T>,
    ) -> T {
        let deadline = Instant::now() + timeout;
        // 每个事件只交给 `want` 一次：`finish_turn` 的闭包会往 `TurnOutcome` 里累加，重复看会重复累加。
        let mut seen = self.cursor;
        loop {
            let notified = self.shared.more.notified();
            {
                let evs = self.shared.events.lock().unwrap();
                for (i, (_, e)) in evs.iter().enumerate().skip(seen) {
                    seen = i + 1;
                    if let Some(t) = want(e) {
                        self.cursor = i + 1;
                        return t;
                    }
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() || tokio::time::timeout(left, notified).await.is_err() {
                panic!(
                    "{}s 内等不到{what}。最近的事件：\n{}",
                    timeout.as_secs(),
                    self.recent(30)
                );
            }
        }
    }

    /// 在一段时间里不该出现满足 `bad` 的事件（只用在确有必要的地方：它总要等满这段时间）。
    pub async fn expect_none(&self, what: &str, during: Duration, bad: impl Fn(&Event) -> bool) {
        let from = self.mark();
        tokio::time::sleep(during).await;
        let evs = self.shared.events.lock().unwrap();
        if let Some((_, e)) = evs.iter().skip(from).find(|(_, e)| bad(e)) {
            panic!("不该出现{what}：{e:?}");
        }
    }

    /// 最近 `n` 条事件，给失败信息用。连续的文字 / 思考增量并成一行，免得几十个字的碎片
    /// 把真正有用的事件挤出去。
    pub fn recent(&self, n: usize) -> String {
        let evs = self.shared.events.lock().unwrap();
        let t0 = evs.first().map(|(t, _)| *t);
        let mut lines: Vec<(u128, String)> = vec![];
        let mut run: Option<(u128, &'static str, SessionId, String)> = None;
        let flush = |run: &mut Option<(u128, &'static str, SessionId, String)>,
                     lines: &mut Vec<(u128, String)>| {
            if let Some((at, kind, s, text)) = run.take() {
                lines.push((
                    at,
                    format!("{kind}×{{{}}} {s}: {text:?}", text.chars().count()),
                ));
            }
        };
        for (t, e) in evs.iter() {
            let at = t0.map(|t0| t.duration_since(t0).as_millis()).unwrap_or(0);
            let delta = match e {
                Event::TextDelta { session, text } => Some(("TextDelta", session, text)),
                Event::ThinkingDelta { session, text } => Some(("ThinkingDelta", session, text)),
                _ => None,
            };
            match (delta, &mut run) {
                (Some((k, s, text)), Some((_, rk, rs, acc))) if *rk == k && rs == s => {
                    acc.push_str(text)
                }
                (Some((k, s, text)), _) => {
                    flush(&mut run, &mut lines);
                    run = Some((at, k, s.clone(), text.clone()));
                }
                (None, _) => {
                    flush(&mut run, &mut lines);
                    lines.push((at, format!("{e:?}")));
                }
            }
        }
        flush(&mut run, &mut lines);
        lines[lines.len().saturating_sub(n)..]
            .iter()
            .map(|(at, s)| {
                let mut s = s.clone();
                if s.len() > 400 {
                    let cut = (0..=400).rev().find(|i| s.is_char_boundary(*i)).unwrap();
                    s.truncate(cut);
                    s.push('…');
                }
                format!("  +{at}ms {s}")
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ---- 会话 ----

    pub async fn new_session(&mut self) -> SessionId {
        let Reply::Session(s) = self
            .ok(Command::CreateSession {
                project: None,
                scene: None,
                model: None,
                permission: None,
            })
            .await
        else {
            panic!("create_session 回的不是 Session")
        };
        self.watch(&s.id).await;
        s.id
    }

    /// Watch 一个会话。自动作答时，快照里已经悬着的提问也一并答掉（它们不会再作为事件来）。
    pub async fn watch(&self, id: &SessionId) -> Watched {
        let w = match self
            .ok(Command::Watch {
                session: id.clone(),
            })
            .await
        {
            Reply::Watched(w) => *w,
            r => panic!("watch 回的是 {r:?}"),
        };
        let how = *self.shared.answering.lock().unwrap();
        if how != Answering::Manual {
            for p in &w.prompts {
                let _ = self
                    .call(Command::Answer {
                        session: id.clone(),
                        prompt: p.id.clone(),
                        answer: auto_answer(p, how),
                    })
                    .await;
            }
        }
        w
    }

    pub async fn sessions(&self) -> Vec<SessionSummary> {
        match self
            .ok(Command::ListSessions {
                filter: Default::default(),
            })
            .await
        {
            Reply::Sessions(v) => v,
            r => panic!("list_sessions 回的是 {r:?}"),
        }
    }

    pub async fn submit(&self, id: &SessionId, text: &str, mode: SubmitMode) -> Reply {
        self.ok(Command::Submit {
            session: id.clone(),
            text: text.into(),
            mode,
        })
        .await
    }

    /// 说一句话，等这一轮结束，收集它的文字、工具与用量。
    pub async fn turn(&mut self, id: &SessionId, text: &str) -> TurnOutcome {
        self.skip_past();
        self.submit(id, text, SubmitMode::Turn).await;
        self.finish_turn(id).await
    }

    /// 等会话 `id` 的这一轮结束（从游标往后看）。
    pub async fn finish_turn(&mut self, id: &SessionId) -> TurnOutcome {
        let mut out = TurnOutcome::default();
        let id = id.clone();
        self.wait_event("这一轮的结局", TURN, |e| {
            match e {
                Event::TextDelta { session, text } if *session == id => out.text.push_str(text),
                Event::StepRestarted { session, .. } if *session == id => out.text.clear(),
                Event::ToolStarted { session, name, .. } if *session == id => {
                    out.tools.push(name.clone())
                }
                Event::ToolFinished {
                    session,
                    content,
                    is_error,
                    ..
                } if *session == id => out.results.push((content.clone(), *is_error)),
                Event::PromptPending { session, prompt } if *session == id => {
                    out.prompts.push(prompt.clone())
                }
                Event::TurnEnded {
                    session,
                    stop,
                    usage,
                } if *session == id => {
                    out.end = Some(Ok(stop.clone()));
                    out.usage = *usage;
                    return Some(());
                }
                Event::TurnFailed { session, message } if *session == id => {
                    out.end = Some(Err(message.clone()));
                    return Some(());
                }
                _ => {}
            }
            None
        })
        .await;
        out
    }

    pub async fn wait_prompt(&mut self, id: &SessionId) -> Prompt {
        let id = id.clone();
        self.wait_event("提问", TURN, |e| match e {
            Event::PromptPending { session, prompt } if *session == id => Some(prompt.clone()),
            _ => None,
        })
        .await
    }

    pub async fn wait_upsert(
        &mut self,
        what: &str,
        want: impl Fn(&SessionSummary) -> bool,
    ) -> SessionSummary {
        self.wait_event(what, TURN, |e| match e {
            Event::SessionUpserted(s) if want(s) => Some(s.clone()),
            _ => None,
        })
        .await
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Some(h) = self.beat.lock().unwrap().take() {
            h.abort();
        }
    }
}

fn auto_answer(p: &Prompt, how: Answering) -> Answer {
    match &p.kind {
        PromptKind::Permission { .. } if how == Answering::Deny => Answer::Deny {
            reason: Some("the test denies every permission".into()),
        },
        PromptKind::Permission { .. } => Answer::Permit,
        PromptKind::Question { options, .. } if !options.is_empty() => Answer::Choice { index: 0 },
        PromptKind::Question { .. } => Answer::Text { text: "ok".into() },
    }
}

#[derive(Debug, Default)]
pub struct TurnOutcome {
    pub text: String,
    pub tools: Vec<String>,
    /// 每个工具的结果与是否出错，按完成的先后。
    pub results: Vec<(String, bool)>,
    pub prompts: Vec<Prompt>,
    pub usage: Usage,
    /// `Ok(停止原因)` 或 `Err(失败信息)`。
    pub end: Option<Result<StopReason, String>>,
}

impl TurnOutcome {
    /// 这一轮正常结束了（模型说完了），否则把失败信息带出来。
    pub fn ended(&self) -> &Self {
        match &self.end {
            Some(Ok(StopReason::EndTurn)) => self,
            other => panic!("这一轮没有正常结束：{other:?}\n文字：{}", self.text),
        }
    }
}
