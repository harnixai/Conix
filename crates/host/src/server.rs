//! socket 服务与单一接入（architecture §8.3）：握手、踢人、心跳与空闲兜底、脱离与停实例。
//! 其余端口命令交给 [`Service`]。

use crate::sync::Locked;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use client::backend::write_frame;
use client::local::now_ms;
use port::link::{ClientFrame, LinkReply, ServerFrame};
use port::{ClientInfo, Command, Event, PortError, Reply};
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::OwnedWriteHalf;
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, mpsc, watch};
use tokio::task::JoinSet;
use tokio::time::{Instant, sleep, sleep_until, timeout};

use crate::service::{Service, Sink};

const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(35);
/// 关停时等各条连接把排着的帧写完；对端不读也不会让关停卡住。
const FLUSH_TIMEOUT: Duration = Duration::from_secs(3);

pub struct ServeOptions {
    /// 这么久一帧都没收到就断开（TUI 僵死、ssh 半开）。
    pub heartbeat_timeout: Duration,
    /// 置为 `Some(原因)` 即关停：先给接入者推 `InstanceStopped{原因}`。发送端丢掉不算关停。
    pub shutdown: watch::Receiver<Option<String>>,
}

impl ServeOptions {
    pub fn new(shutdown: watch::Receiver<Option<String>>) -> ServeOptions {
        ServeOptions {
            heartbeat_timeout: HEARTBEAT_TIMEOUT,
            shutdown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServeEnd {
    /// 接入者发了 `StopInstance`（已回 `Ok`），由调用方关停实例。
    StopRequested,
    /// `ServeOptions::shutdown` 触发。
    Shutdown(String),
}

type OnHolder = Box<dyn Fn(Option<&ClientInfo>) + Send + Sync>;

struct Holder {
    conn: u64,
    info: ClientInfo,
    sink: Sink,
    close: watch::Sender<bool>,
}

struct Shared {
    svc: Arc<dyn Service>,
    on_holder: OnHolder,
    /// 接入者的登记与 `svc.attach` / `on_holder` 在同一把锁下做：踢人与旧连接收尾交错时，
    /// 旧连接晚到的 `attach(None)` 不会盖掉新接入者。
    holder: Mutex<Option<Holder>>,
    stop: Notify,
    closing: watch::Sender<bool>,
    heartbeat_timeout: Duration,
    uid: u32,
}

/// 跑到 `StopInstance` 或 `opts.shutdown` 为止；返回前所有连接都已关掉、`svc.attach(None)`
/// 已调过。`on_holder` 在接入者变化（含心跳带来的 `last_input_ms` 变化）时调用，用来重写发现文件。
pub async fn serve(
    listener: UnixListener,
    svc: Arc<dyn Service>,
    mut opts: ServeOptions,
    on_holder: impl Fn(Option<&ClientInfo>) + Send + Sync + 'static,
) -> ServeEnd {
    let sh = Arc::new(Shared {
        svc,
        on_holder: Box::new(on_holder),
        holder: Mutex::new(None),
        stop: Notify::new(),
        closing: watch::channel(false).0,
        heartbeat_timeout: opts.heartbeat_timeout,
        uid: nix::unistd::geteuid().as_raw(),
    });
    let mut conns = JoinSet::new();
    let mut next = 0u64;
    let end = loop {
        tokio::select! {
            r = listener.accept() => match r {
                Ok((stream, _)) => {
                    next += 1;
                    conns.spawn(connection(sh.clone(), stream, next));
                }
                // 多半是 fd 用完之类的暂时状况；退出就等于把实例停了，不值得。
                Err(e) => {
                    tracing::warn!("accept: {e}");
                    sleep(Duration::from_millis(100)).await;
                }
            },
            reason = shutdown_reason(&mut opts.shutdown) => break ServeEnd::Shutdown(reason),
            _ = sh.stop.notified() => break ServeEnd::StopRequested,
            Some(_) = conns.join_next(), if !conns.is_empty() => {}
        }
    };
    if let ServeEnd::Shutdown(reason) = &end
        && let Some(h) = sh.holder.locked().as_ref()
    {
        let _ = h.sink.send(event(Event::InstanceStopped {
            reason: reason.clone(),
        }));
    }
    let _ = sh.closing.send(true);
    if timeout(FLUSH_TIMEOUT, async {
        while conns.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        conns.shutdown().await;
        release(&sh, None);
    }
    end
}

async fn shutdown_reason(rx: &mut watch::Receiver<Option<String>>) -> String {
    // `watch::Ref` 不是 Send，不能活过下面的 await，所以先克隆出来。
    let reason = rx.wait_for(Option::is_some).await.ok().map(|r| r.clone());
    match reason {
        Some(r) => r.unwrap_or_default(),
        None => std::future::pending().await,
    }
}

fn event(event: Event) -> ServerFrame {
    ServerFrame::Event { event }
}

fn reply(id: u64, r: Result<Reply, PortError>) -> ServerFrame {
    ServerFrame::Reply {
        id,
        reply: LinkReply::from(r),
    }
}

/// 发送端丢掉也算关：等的一方不能因此永远挂着。
async fn closed(rx: &mut watch::Receiver<bool>) {
    let _ = rx.wait_for(|c| *c).await;
}

/// 放开接入。`conn` 为 `Some` 时只放自己那一份：已经被踢的连接收尾时什么也不做。
fn release(sh: &Shared, conn: Option<u64>) {
    let mut h = sh.holder.locked();
    if h.as_ref().is_some_and(|x| conn.is_none_or(|c| c == x.conn)) {
        *h = None;
        sh.svc.attach(None);
        (sh.on_holder)(None);
    }
}

/// 帧解析不了但带着 `id`（多半是更新的 TUI 发来的新命令）时回 `Unsupported`，不断开。
#[derive(Deserialize)]
struct IdOnly {
    id: u64,
}

enum Admit {
    Ok,
    Busy(PortError),
}

async fn connection(sh: Arc<Shared>, stream: UnixStream, conn: u64) {
    if !stream.peer_cred().is_ok_and(|c| c.uid() == sh.uid) {
        return;
    }
    let (rd, mut wr) = stream.into_split();
    let mut lines = BufReader::new(rd).lines();
    let mut closing = sh.closing.subscribe();

    let first = tokio::select! {
        l = timeout(sh.heartbeat_timeout, lines.next_line()) => l,
        _ = closed(&mut closing) => return,
    };
    // `list` 探测 socket 时只连不发，走的就是这里。
    let Ok(Ok(Some(line))) = first else { return };
    let (id, mut client, force) = match serde_json::from_str::<ClientFrame>(&line) {
        Ok(ClientFrame {
            id,
            cmd: Command::Hello { client, force },
        }) => (id, client, force),
        // `conix flow …` 改了项目的流程包、加了作业：不接入、不踢人，做完回一句就断开。
        Ok(ClientFrame {
            id,
            cmd: cmd @ (Command::ReloadFlows | Command::ReloadJobs),
        }) => {
            let r = sh.svc.handle(cmd).await;
            let _ = write_frame(&mut wr, &reply(id, r)).await;
            let _ = wr.shutdown().await;
            return;
        }
        _ => {
            if let Ok(IdOnly { id }) = serde_json::from_str(&line) {
                let err = PortError::Failed("first frame must be hello".into());
                let _ = write_frame(&mut wr, &reply(id, Err(err))).await;
            }
            return;
        }
    };

    let (sink, rx) = mpsc::unbounded_channel();
    let (close_tx, close_rx) = watch::channel(false);
    let admit = {
        let mut h = sh.holder.locked();
        match h.take() {
            Some(old) if !force => {
                let idle_ms = now_ms().saturating_sub(old.info.last_input_ms);
                let holder = old.info.clone();
                *h = Some(old);
                Admit::Busy(PortError::Occupied { holder, idle_ms })
            }
            old => {
                if let Some(old) = old {
                    let _ = old.sink.send(event(Event::Kicked {
                        by: client.clone(),
                        at_ms: now_ms(),
                    }));
                    let _ = old.close.send(true);
                }
                let _ = sink.send(reply(id, Ok(hello(&sh, &client))));
                // 先排回复再交出 sink：Service 一拿到就可能推事件，`Hello` 的回复应在最前。
                sh.svc.attach(Some(sink.clone()));
                (sh.on_holder)(Some(&client));
                *h = Some(Holder {
                    conn,
                    info: client.clone(),
                    sink: sink.clone(),
                    close: close_tx.clone(),
                });
                Admit::Ok
            }
        }
    };
    if let Admit::Busy(err) = admit {
        let _ = write_frame(&mut wr, &reply(id, Err(err))).await;
        let _ = wr.shutdown().await;
        return;
    }
    // 写端不另起任务：读写都在这条连接的任务里，`serve` 被中止时整条连接一起关，
    // 不会剩一个写任务攥着 socket（Service 手里的 sink 会让它一直等下去）。
    let writer = write_loop(wr, rx, close_rx.clone());
    let reader = async {
        let mut seen = Instant::now();
        let mut kicked = close_rx;
        loop {
            tokio::select! {
                _ = closed(&mut kicked) => break,
                _ = closed(&mut closing) => break,
                _ = sleep_until(seen + sh.heartbeat_timeout) => break,
                line = lines.next_line() => {
                    let Ok(Some(line)) = line else { break };
                    seen = Instant::now();
                    let (id, cmd) = match serde_json::from_str::<ClientFrame>(&line) {
                        Ok(f) => (f.id, f.cmd),
                        Err(e) => {
                            if let Ok(IdOnly { id }) = serde_json::from_str(&line) {
                                let _ = sink.send(reply(id, Err(PortError::Unsupported(e.to_string()))));
                            }
                            continue;
                        }
                    };
                    match cmd {
                        // model 接入后自己还会发一次 `Hello`：连接已经是它的，照常回快照。
                        Command::Hello { .. } => {
                            let _ = sink.send(reply(id, Ok(hello(&sh, &client))));
                        }
                        Command::Heartbeat { last_input_ms } => {
                            if last_input_ms != client.last_input_ms {
                                client.last_input_ms = last_input_ms;
                                let mut h = sh.holder.locked();
                                if let Some(x) = h.as_mut().filter(|x| x.conn == conn) {
                                    x.info.last_input_ms = last_input_ms;
                                    (sh.on_holder)(Some(&x.info));
                                }
                            }
                            let _ = sink.send(reply(id, Ok(Reply::Ok)));
                        }
                        Command::Detach => {
                            let _ = sink.send(reply(id, Ok(Reply::Ok)));
                            break;
                        }
                        // 连接留着，等 `serve` 关停时统一关：回复要先写出去。
                        Command::StopInstance => {
                            let _ = sink.send(reply(id, Ok(Reply::Ok)));
                            sh.stop.notify_one();
                        }
                        // 各起一个任务：慢命令不能挡住心跳与别的命令。回复与 Service 推的事件
                        // 走同一个 sink，所以这条命令处理期间的事件排在回复前面。
                        cmd => {
                            let (svc, sink) = (sh.svc.clone(), sink.clone());
                            tokio::spawn(async move {
                                let r = svc.handle(cmd).await;
                                let _ = sink.send(reply(id, r));
                            });
                        }
                    }
                }
            }
        }
        release(&sh, Some(conn));
        let _ = close_tx.send(true);
    };
    tokio::join!(reader, writer);
}

fn hello(sh: &Shared, client: &ClientInfo) -> Reply {
    let (mut instance, flows, jobs) = sh.svc.hello();
    instance.holder = Some(client.clone());
    Reply::Hello {
        instance,
        flows,
        jobs,
    }
}

/// 收到关闭信号时把已排队的帧写完再关：`Kicked`、`InstanceStopped` 与
/// `Detach` 的回复都是在关之前排进去的，丢了对方就不知道自己为什么断了。
async fn write_loop(
    mut wr: OwnedWriteHalf,
    mut rx: mpsc::UnboundedReceiver<ServerFrame>,
    mut close: watch::Receiver<bool>,
) {
    loop {
        tokio::select! {
            biased;
            f = rx.recv() => match f {
                Some(f) => if write_frame(&mut wr, &f).await.is_err() { return },
                None => break,
            },
            _ = closed(&mut close) => {
                while let Ok(f) = rx.try_recv() {
                    if write_frame(&mut wr, &f).await.is_err() {
                        return;
                    }
                }
                break;
            }
        }
    }
    let _ = wr.shutdown().await;
}
