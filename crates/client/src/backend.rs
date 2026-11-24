//! 经 `host.sock` 接入的 `port::Backend`（architecture §8.3）：按 `id` 配对回复，事件进一条
//! channel，连接意外断开时自己重连。

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use async_trait::async_trait;
use port::link::{ClientFrame, LinkReply, ServerFrame};
use port::{Backend, ClientInfo, Command, ConnState, Event, InstanceInfo, PortError, Reply};
use serde::Serialize;
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};

use crate::layout::{InstanceFile, read_json};
use crate::local::{now_ms, pid_alive};

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const FIRST_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(8);
const GIVE_UP_AFTER: Duration = Duration::from_secs(30);

type Reader = Lines<BufReader<OwnedReadHalf>>;

pub struct SocketBackend {
    inner: Arc<Inner>,
    instance: InstanceInfo,
    events: Mutex<Option<mpsc::UnboundedReceiver<Event>>>,
}

struct Inner {
    socket: PathBuf,
    discovery: Option<PathBuf>,
    client: ClientInfo,
    next_id: AtomicU64,
    /// 最近一次心跳带的 `last_input_ms`：重连的 `Hello` 用它，否则实例拿握手时的旧值算空闲，
    /// 一接回来就可能判成超时。
    last_input_ms: AtomicU64,
    idle_timeout_ms: Option<u64>,
    link: Mutex<Link>,
    /// 被踢、空闲脱离、实例停了，或者是自己要脱离 / 停实例：之后断开不再重连。
    done: AtomicBool,
    events: mpsc::UnboundedSender<Event>,
}

/// 写端与在等的回复放在同一把锁下：断开时清空 `pending` 与新的 `call` 登记不会交错，
/// 否则一个 `call` 可能登记在已经清过的表里，永远等不到回复。
#[derive(Default)]
struct Link {
    tx: Option<mpsc::UnboundedSender<ClientFrame>>,
    pending: HashMap<u64, oneshot::Sender<LinkReply>>,
}

impl SocketBackend {
    /// 连上并握手。`discovery` 是实例的发现文件：重连时凭它的 pid 判断实例是否还在。
    pub async fn connect(
        socket: impl Into<PathBuf>,
        discovery: Option<PathBuf>,
        client: ClientInfo,
        force: bool,
        idle_timeout_ms: Option<u64>,
    ) -> Result<SocketBackend, PortError> {
        let socket = socket.into();
        let (lines, wr, reply) = handshake(&socket, &client, force, idle_timeout_ms).await?;
        let Reply::Hello { instance, .. } = reply else {
            return Err(PortError::Failed("unexpected handshake reply".into()));
        };
        let (events, rx) = mpsc::unbounded_channel();
        let inner = Arc::new(Inner {
            socket,
            discovery,
            last_input_ms: AtomicU64::new(client.last_input_ms),
            idle_timeout_ms,
            client,
            next_id: AtomicU64::new(1),
            link: Mutex::new(Link::default()),
            done: AtomicBool::new(false),
            events,
        });
        install(&inner, lines, wr, false);
        Ok(SocketBackend {
            inner,
            instance,
            events: Mutex::new(Some(rx)),
        })
    }

    /// 握手时实例回的信息（`holder` 是自己）。
    pub fn instance(&self) -> &InstanceInfo {
        &self.instance
    }
}

#[async_trait]
impl Backend for SocketBackend {
    async fn call(&self, cmd: Command) -> Result<Reply, PortError> {
        match cmd {
            Command::Detach | Command::StopInstance => {
                self.inner.done.store(true, Ordering::SeqCst)
            }
            Command::Heartbeat { last_input_ms } => self
                .inner
                .last_input_ms
                .store(last_input_ms, Ordering::Relaxed),
            _ => {}
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let rx = {
            let mut link = self.inner.link.lock().unwrap();
            let tx = link.tx.as_ref().ok_or(PortError::Disconnected)?;
            tx.send(ClientFrame { id, cmd })
                .map_err(|_| PortError::Disconnected)?;
            let (otx, orx) = oneshot::channel();
            link.pending.insert(id, otx);
            orx
        };
        rx.await.map_err(|_| PortError::Disconnected)?.into()
    }

    fn events(&self) -> mpsc::UnboundedReceiver<Event> {
        // 第二次取给一条已关闭的空 channel，而不是 panic：调用方读到 None 就知道没有了。
        self.events
            .lock()
            .unwrap()
            .take()
            .unwrap_or_else(|| mpsc::unbounded_channel().1)
    }
}

impl Inner {
    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn dispatch(&self, line: &str) {
        // 认不出的帧（更新的实例多出来的事件）跳过，不断开。
        match serde_json::from_str::<ServerFrame>(line) {
            Ok(ServerFrame::Reply { id, reply }) => {
                if let Some(tx) = self.link.lock().unwrap().pending.remove(&id) {
                    let _ = tx.send(reply);
                }
            }
            Ok(ServerFrame::Event { event }) => {
                if matches!(
                    event,
                    Event::Kicked { .. }
                        | Event::IdleDetached { .. }
                        | Event::InstanceStopped { .. }
                ) {
                    self.done.store(true, Ordering::SeqCst);
                }
                self.emit(event);
            }
            Err(_) => {}
        }
    }

    fn drop_link(&self) {
        let mut link = self.link.lock().unwrap();
        link.tx = None;
        // 丢掉 oneshot 的发送端，等着的 `call` 就拿到 Disconnected。
        link.pending.clear();
    }

    /// 发现文件里的 pid 已经不在：实例崩了或被杀了，没必要再等。发现文件不在不算——
    /// 正常退出会先推 `InstanceStopped`，走不到这里；`conix start` 重写之前也可能短暂没有。
    fn instance_gone(&self) -> bool {
        self.discovery
            .as_deref()
            .and_then(read_json::<InstanceFile>)
            .is_some_and(|f| !pid_alive(f.pid))
    }
}

async fn handshake(
    socket: &Path,
    client: &ClientInfo,
    force: bool,
    idle_timeout_ms: Option<u64>,
) -> Result<(Reader, OwnedWriteHalf, Reply), PortError> {
    let stream = UnixStream::connect(socket)
        .await
        .map_err(|e| PortError::Failed(format!("connect {}: {e}", socket.display())))?;
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    let hello = ClientFrame {
        id: 0,
        cmd: Command::Hello {
            client: client.clone(),
            force,
            idle_timeout_ms,
        },
    };
    write_frame(&mut wr, &hello)
        .await
        .map_err(|_| PortError::Disconnected)?;
    let line = timeout(HANDSHAKE_TIMEOUT, lines.next_line())
        .await
        .map_err(|_| PortError::Failed("handshake timed out".into()))?
        .map_err(|_| PortError::Disconnected)?
        .ok_or(PortError::Disconnected)?;
    match serde_json::from_str::<ServerFrame>(&line) {
        Ok(ServerFrame::Reply { id: 0, reply }) => {
            Result::<Reply, PortError>::from(reply).map(|r| (lines, wr, r))
        }
        _ => Err(PortError::Failed("unexpected handshake reply".into())),
    }
}

/// 写端装好之后才推 `Resynced`、再起读任务：model 收到 `Resynced` 立刻重新拉列表，那时
/// 写端必须已经在；读任务后起，服务端的事件就不会抢在 `Resynced` 前面。
fn install(inner: &Arc<Inner>, lines: Reader, wr: OwnedWriteHalf, resynced: bool) {
    let (tx, rx) = mpsc::unbounded_channel();
    inner.link.lock().unwrap().tx = Some(tx);
    if resynced {
        inner.emit(Event::Connection(ConnState::Resynced));
    }
    tokio::spawn(write_loop(wr, rx));
    tokio::spawn(read_loop(Arc::downgrade(inner), lines));
}

async fn write_loop(mut wr: OwnedWriteHalf, mut rx: mpsc::UnboundedReceiver<ClientFrame>) {
    while let Some(f) = rx.recv().await {
        if write_frame(&mut wr, &f).await.is_err() {
            break;
        }
    }
    let _ = wr.shutdown().await;
}

/// 读任务只拿弱引用：`SocketBackend` 被丢掉时写端随之关闭，实例那边看到 EOF 就放开接入，
/// 而不是被后台任务吊着一条没人用的连接。
async fn read_loop(inner: Weak<Inner>, mut lines: Reader) {
    while let Ok(Some(line)) = lines.next_line().await {
        let Some(inner) = inner.upgrade() else { return };
        inner.dispatch(&line);
    }
    let Some(strong) = inner.upgrade() else {
        return;
    };
    strong.drop_link();
    if strong.done.load(Ordering::SeqCst) {
        return;
    }
    strong.emit(Event::Connection(ConnState::Lost));
    drop(strong);
    reconnect(inner).await;
}

async fn reconnect(weak: Weak<Inner>) {
    let started = Instant::now();
    let mut delay = FIRST_BACKOFF;
    for attempt in 1.. {
        let Some(inner) = weak.upgrade() else { return };
        inner.emit(Event::Connection(ConnState::Reconnecting { attempt }));
        let socket = inner.socket.clone();
        let client = ClientInfo {
            last_input_ms: inner.last_input_ms.load(Ordering::Relaxed),
            ..inner.client.clone()
        };
        drop(inner);
        sleep(delay).await;
        delay = (delay * 2).min(MAX_BACKOFF);

        let Some(inner) = weak.upgrade() else { return };
        if inner.instance_gone() {
            return give_up(&inner, "instance exited");
        }
        match handshake(&socket, &client, false, inner.idle_timeout_ms).await {
            Ok((lines, wr, _)) => return install(&inner, lines, wr, true),
            // 断开期间有人正常接入了（比如服务端因心跳超时放掉了我们）。实例还在跑，说
            // "实例停了"是错的；对用户来说这就是被别人占了，按被踢处理，同样不再重连。
            Err(PortError::Occupied { holder, .. }) => {
                inner.done.store(true, Ordering::SeqCst);
                return inner.emit(Event::Kicked {
                    by: holder,
                    at_ms: now_ms(),
                });
            }
            Err(_) if started.elapsed() >= GIVE_UP_AFTER => {
                return give_up(&inner, "reconnect timed out");
            }
            Err(_) => {}
        }
    }
}

fn give_up(inner: &Inner, reason: &str) {
    inner.done.store(true, Ordering::SeqCst);
    inner.emit(Event::InstanceStopped {
        reason: reason.into(),
    });
}

/// 一帧一行。实例与 TUI 两边共用。
pub async fn write_frame(
    w: &mut (impl AsyncWrite + Unpin),
    frame: &impl Serialize,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(frame)?;
    line.push(b'\n');
    w.write_all(&line).await?;
    w.flush().await
}
