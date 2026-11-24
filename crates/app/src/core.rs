//! 不碰终端的事件循环核心（architecture §5）：持有 State、键位解析器、实例目录与当前后端，
//! 把输入翻译成 Msg、跑 update、执行 Effect。真实终端的读写在 `main.rs`，测试直接驱动它。

use std::sync::Arc;
use std::time::Duration;

use keys::{Key, Keymap, Resolution, Resolver};
use model::{Effect, InstOk, InstOp, Mouse, Msg, ReqId, State};
use port::{Attach, Backend, Event, Instances, NewInstance, PortError, Reply};
use tokio::sync::mpsc;
use tui::{FrameState, Size};

/// `Core` 自己处理不了、要交给终端外壳的事。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellEffect {
    Quit { detach: bool },
    SetMouseCapture(bool),
    Redraw,
    Bell,
    Clipboard(String),
}

/// 启动后先做什么（需求 I-01、I-02、I-04）。
#[derive(Debug, Clone)]
pub enum Start {
    Picker,
    Op(InstOp),
}

type ReplyMsg = (ReqId, Result<Reply, PortError>);
type InstMsg = (
    InstOp,
    Result<(InstOk, Option<Arc<dyn Backend>>), PortError>,
);

pub struct Core {
    pub state: State,
    resolver: Resolver,
    instances: Arc<dyn Instances>,
    backend: Option<Arc<dyn Backend>>,
    events: Option<mpsc::UnboundedReceiver<Event>>,
    replies_tx: mpsc::UnboundedSender<ReplyMsg>,
    replies: mpsc::UnboundedReceiver<ReplyMsg>,
    inst_tx: mpsc::UnboundedSender<InstMsg>,
    inst: mpsc::UnboundedReceiver<InstMsg>,
    clock: Box<dyn Fn() -> u64 + Send>,
    dirty: bool,
    /// 测试用计数：画了几帧（testing.md §1「一次排空最多画一帧」）。
    pub frames: u64,
}

impl Core {
    pub fn new(
        instances: Arc<dyn Instances>,
        keymap: Arc<Keymap>,
        size: Size,
        clock: Box<dyn Fn() -> u64 + Send>,
        start: Start,
        settings: model::Settings,
    ) -> (Core, Vec<ShellEffect>) {
        let now = clock();
        let (replies_tx, replies) = mpsc::unbounded_channel();
        let (inst_tx, inst) = mpsc::unbounded_channel();
        let state = State::with_settings(keymap, size, now, settings);
        let mut core = Core {
            state,
            resolver: Resolver::default(),
            instances,
            backend: None,
            events: None,
            replies_tx,
            replies,
            inst_tx,
            inst,
            clock,
            dirty: true,
            frames: 0,
        };
        let effects = match start {
            Start::Picker => core.state.open_picker(None),
            Start::Op(op) => {
                core.state.open_picker(None);
                vec![Effect::Instances(op)]
            }
        };
        let shell = core.run(effects);
        (core, shell)
    }

    fn now(&self) -> u64 {
        (self.clock)()
    }

    pub fn handle(&mut self, msg: Msg) -> Vec<ShellEffect> {
        self.state.set_now(self.now());
        let effects = model::update(&mut self.state, msg);
        self.dirty = true;
        self.run(effects)
    }

    pub fn key(&mut self, key: Key) -> Vec<ShellEffect> {
        let now = self.now();
        let contexts = self.state.key_contexts();
        let keymap = self.state.keymap.clone();
        match self.resolver.resolve(&keymap, &contexts, key, now) {
            Resolution::Action(a) => self.handle(Msg::Action(a)),
            Resolution::Text(c) => self.handle(Msg::Text(c)),
            // 二次确认与"已脱离"里，没绑定的键也算一次按键（放弃确认 / 重新接入）。
            Resolution::Unbound => match key.as_text() {
                Some(c) => self.handle(Msg::Text(c)),
                None => vec![],
            },
            Resolution::Pending => vec![],
        }
    }

    pub fn mouse(&mut self, m: Mouse) -> Vec<ShellEffect> {
        self.handle(Msg::Mouse(m))
    }

    pub fn resize(&mut self, size: Size) -> Vec<ShellEffect> {
        self.handle(Msg::Resize(size))
    }

    /// 到了 `deadline()` 的时刻调用。
    pub fn tick(&mut self) -> Vec<ShellEffect> {
        self.handle(Msg::Tick)
    }

    /// 下一次需要 Tick 的时刻（毫秒时间戳）；`None` 表示不用唤醒。
    pub fn deadline(&self) -> Option<u64> {
        self.state.next_deadline()
    }

    pub fn now_ms(&self) -> u64 {
        self.now()
    }

    /// 把这一刻已经到达的后端事件、命令结果与实例操作结果全部处理掉（不等待）。
    pub fn pump(&mut self) -> Vec<ShellEffect> {
        let mut out = vec![];
        loop {
            let mut progressed = false;
            while let Some(ev) = self.events.as_mut().and_then(|e| e.try_recv().ok()) {
                out.extend(self.handle(Msg::Event(Box::new(ev))));
                progressed = true;
            }
            while let Ok((id, r)) = self.replies.try_recv() {
                out.extend(self.handle(Msg::Reply(id, Box::new(r))));
                progressed = true;
            }
            while let Ok(m) = self.inst.try_recv() {
                out.extend(self.instances_done(m));
                progressed = true;
            }
            if !progressed {
                return out;
            }
        }
    }

    /// 等下一件事：后端事件、命令结果或实例操作结果。给真实循环的 `select!` 用。
    pub async fn next_msg(&mut self) -> Vec<ShellEffect> {
        let ev = async {
            match self.events.as_mut() {
                Some(e) => e.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            Some(ev) = ev => self.handle(Msg::Event(Box::new(ev))),
            Some((id, r)) = self.replies.recv() => self.handle(Msg::Reply(id, Box::new(r))),
            Some(m) = self.inst.recv() => self.instances_done(m),
        }
    }

    /// 退出前告诉实例"我脱离了"，最多等一小会儿；实例不在也不要紧。
    pub async fn detach(&mut self) {
        if let Some(b) = self.backend.take() {
            let _ = tokio::time::timeout(Duration::from_millis(300), b.call(port::Command::Detach))
                .await;
        }
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn frame(&mut self) -> FrameState {
        self.frames += 1;
        model::view(&self.state)
    }

    pub fn attached(&self) -> bool {
        self.backend.is_some()
    }

    fn instances_done(&mut self, (op, r): InstMsg) -> Vec<ShellEffect> {
        let r = r.map(|(ok, backend)| {
            if let Some(b) = backend {
                self.events = Some(b.events());
                self.backend = Some(b);
            }
            ok
        });
        self.handle(Msg::Instances(op, r))
    }

    fn run(&mut self, effects: Vec<Effect>) -> Vec<ShellEffect> {
        let mut shell = vec![];
        for e in effects {
            match e {
                Effect::Call(id, cmd) => {
                    let Some(backend) = self.backend.clone() else {
                        let _ = self.replies_tx.send((id, Err(PortError::Disconnected)));
                        continue;
                    };
                    let tx = self.replies_tx.clone();
                    tokio::spawn(async move {
                        let r = backend.call(*cmd).await;
                        let _ = tx.send((id, r));
                    });
                }
                Effect::Instances(op) => {
                    if !matches!(op, InstOp::List | InstOp::Stop { .. }) {
                        self.backend = None;
                        self.events = None;
                    }
                    let inst = self.instances.clone();
                    let tx = self.inst_tx.clone();
                    tokio::spawn(async move {
                        let r = run_inst(inst.as_ref(), &op).await;
                        let _ = tx.send((op, r));
                    });
                }
                Effect::Release => {
                    self.backend = None;
                    self.events = None;
                }
                Effect::Quit { detach } => shell.push(ShellEffect::Quit { detach }),
                Effect::SetMouseCapture(on) => shell.push(ShellEffect::SetMouseCapture(on)),
                Effect::Redraw => shell.push(ShellEffect::Redraw),
                Effect::Bell => shell.push(ShellEffect::Bell),
                Effect::Clipboard(t) => shell.push(ShellEffect::Clipboard(t)),
            }
        }
        shell
    }
}

async fn run_inst(
    inst: &dyn Instances,
    op: &InstOp,
) -> Result<(InstOk, Option<Arc<dyn Backend>>), PortError> {
    let attached = |name: &str, b| Ok((InstOk::Attached(name.to_string()), Some(b)));
    match op {
        InstOp::List => Ok((InstOk::Listed(inst.list(true).await?), None)),
        InstOp::Attach { name, force } => {
            let b = inst.attach(name, Attach { force: *force }).await?;
            attached(name, b)
        }
        InstOp::Start { name } => {
            let b = inst.start(name).await?;
            attached(name, b)
        }
        InstOp::Create { name, project } => {
            let project = project.clone().or_else(|| {
                std::env::current_dir()
                    .ok()
                    .map(|p| p.display().to_string())
            });
            let (info, b) = inst
                .create(NewInstance {
                    name: name.clone().map(Into::into),
                    project,
                })
                .await?;
            attached(info.name.as_str(), b)
        }
        InstOp::Stop { name } => {
            inst.stop(name).await?;
            Ok((InstOk::Stopped, None))
        }
    }
}
