//! socket 服务的接入流程：假的 Service、临时目录里的 socket、手写的帧。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use host::server::{ServeEnd, ServeOptions, serve};
use host::{Service, Sink};
use port::link::{ClientFrame, ServerFrame};
use port::{
    ClientInfo, Command, Event, InstanceInfo, InstanceState, NoticeLevel, PortError, Reply,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::sync::watch;
use tokio::task::JoinHandle;

#[derive(Default)]
struct FakeService {
    sink: Mutex<Option<Sink>>,
    attaches: Mutex<Vec<bool>>,
}

#[async_trait]
impl Service for FakeService {
    fn attach(&self, sink: Option<Sink>) {
        self.attaches.lock().unwrap().push(sink.is_some());
        *self.sink.lock().unwrap() = sink;
    }

    fn hello(&self) -> (InstanceInfo, bool, bool) {
        (
            InstanceInfo {
                name: "t".into(),
                host: "h".into(),
                pid: Some(1),
                project: None,
                state: InstanceState::Running,
                started_ms: None,
                stopped_ms: None,
                version: "0".into(),
                sessions: 0,
                waiting: 0,
                jobs: 0,
                holder: None,
                restarts: 0,
            },
            false,
            true,
        )
    }

    /// `ListJobs` 先推三条事件再回；`ListFlows` 永远不回（慢命令）。
    async fn handle(&self, cmd: Command) -> Result<Reply, PortError> {
        match cmd {
            Command::ListJobs => {
                let sink = self.sink.lock().unwrap().clone().unwrap();
                for i in 0..3 {
                    let _ = sink.send(ServerFrame::Event {
                        event: Event::Notice {
                            level: NoticeLevel::Info,
                            text: i.to_string(),
                        },
                    });
                }
                tokio::task::yield_now().await;
                Ok(Reply::Jobs(vec![]))
            }
            Command::ListFlows => std::future::pending().await,
            _ => Err(PortError::Unsupported("fake".into())),
        }
    }
}

struct Server {
    _dir: tempfile::TempDir,
    path: PathBuf,
    svc: Arc<FakeService>,
    holders: Arc<Mutex<Vec<Option<ClientInfo>>>>,
    shutdown: watch::Sender<Option<String>>,
    task: JoinHandle<ServeEnd>,
}

fn start() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("h.sock");
    let listener = tokio::net::UnixListener::bind(&path).unwrap();
    let svc = Arc::new(FakeService::default());
    let holders = Arc::new(Mutex::new(vec![]));
    let (shutdown, rx) = watch::channel(None);
    let opts = ServeOptions::new(rx);
    let h = holders.clone();
    let task = tokio::spawn(serve(listener, svc.clone(), opts, move |c| {
        h.lock().unwrap().push(c.cloned())
    }));
    Server {
        _dir: dir,
        path,
        svc,
        holders,
        shutdown,
        task,
    }
}

fn now_ms() -> u64 {
    host::instance::now_ms()
}

fn client(pid: u32) -> ClientInfo {
    ClientInfo {
        user: "u".into(),
        pid,
        tty: None,
        ssh_from: None,
        attached_ms: now_ms(),
        last_input_ms: now_ms(),
    }
}

struct Raw {
    lines: Lines<BufReader<OwnedReadHalf>>,
    wr: OwnedWriteHalf,
}

impl Raw {
    async fn connect(path: &Path) -> Raw {
        let (rd, wr) = UnixStream::connect(path).await.unwrap().into_split();
        Raw {
            lines: BufReader::new(rd).lines(),
            wr,
        }
    }

    async fn send(&mut self, id: u64, cmd: Command) {
        let mut line = serde_json::to_vec(&ClientFrame { id, cmd }).unwrap();
        line.push(b'\n');
        self.wr.write_all(&line).await.unwrap();
    }

    /// `None` 是对方关了连接。
    async fn recv(&mut self) -> Option<ServerFrame> {
        let line = tokio::time::timeout(Duration::from_secs(5), self.lines.next_line())
            .await
            .expect("no frame within 5s");
        line.ok()
            .flatten()
            .map(|l| serde_json::from_str(&l).unwrap())
    }

    async fn reply(&mut self) -> (u64, Result<Reply, PortError>) {
        match self.recv().await {
            Some(ServerFrame::Reply { id, reply }) => (id, reply.into()),
            other => panic!("expected reply, got {other:?}"),
        }
    }

    async fn hello(path: &Path, pid: u32, force: bool) -> (Raw, Result<Reply, PortError>) {
        let mut raw = Raw::connect(path).await;
        raw.send(
            1,
            Command::Hello {
                client: client(pid),
                force,
            },
        )
        .await;
        let (id, r) = raw.reply().await;
        assert_eq!(id, 1);
        (raw, r)
    }
}

fn holder_pid(r: &Result<Reply, PortError>) -> u32 {
    match r {
        Ok(Reply::Hello { instance, .. }) => instance.holder.as_ref().unwrap().pid,
        other => panic!("expected hello, got {other:?}"),
    }
}

#[tokio::test]
async fn handshake_attaches_and_repeated_hello_is_idempotent() {
    let s = start();
    let (mut a, r) = Raw::hello(&s.path, 7, false).await;
    assert_eq!(holder_pid(&r), 7);
    assert_eq!(*s.svc.attaches.lock().unwrap(), [true]);
    assert_eq!(
        s.holders
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .as_ref()
            .unwrap()
            .pid,
        7
    );

    a.send(
        2,
        Command::Hello {
            client: client(7),
            force: false,
        },
    )
    .await;
    let (id, r) = a.reply().await;
    assert_eq!((id, holder_pid(&r)), (2, 7));
    assert_eq!(*s.svc.attaches.lock().unwrap(), [true]);
}

#[tokio::test]
async fn first_frame_must_be_hello() {
    let s = start();
    let mut a = Raw::connect(&s.path).await;
    a.send(5, Command::ListJobs).await;
    assert!(matches!(a.reply().await, (5, Err(PortError::Failed(_)))));
    assert!(a.recv().await.is_none());
    assert!(s.svc.attaches.lock().unwrap().is_empty());
}

#[tokio::test]
async fn second_client_is_told_who_holds_it() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    let (mut b, r) = Raw::hello(&s.path, 8, false).await;
    match r {
        Err(PortError::Occupied { holder, .. }) => assert_eq!(holder.pid, 7),
        other => panic!("{other:?}"),
    }
    assert!(b.recv().await.is_none());
    a.send(2, Command::ListJobs).await;
    for _ in 0..3 {
        assert!(matches!(a.recv().await, Some(ServerFrame::Event { .. })));
    }
    assert!(matches!(a.reply().await, (2, Ok(Reply::Jobs(_)))));
}

#[tokio::test]
async fn force_kicks_the_old_client() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    let (mut b, r) = Raw::hello(&s.path, 8, true).await;
    assert_eq!(holder_pid(&r), 8);
    match a.recv().await {
        Some(ServerFrame::Event {
            event: Event::Kicked { by, .. },
        }) => assert_eq!(by.pid, 8),
        other => panic!("{other:?}"),
    }
    assert!(a.recv().await.is_none());
    b.send(2, Command::ListJobs).await;
    for _ in 0..3 {
        b.recv().await.unwrap();
    }
    assert!(matches!(b.reply().await, (2, Ok(Reply::Jobs(_)))));
    // 旧连接收尾时不能把新接入者放掉。
    assert_eq!(*s.svc.attaches.lock().unwrap(), [true, true]);
    assert!(s.holders.lock().unwrap().last().unwrap().is_some());
}

#[tokio::test]
async fn detach_frees_the_slot() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    a.send(2, Command::Detach).await;
    assert!(matches!(a.reply().await, (2, Ok(Reply::Ok))));
    assert!(a.recv().await.is_none());
    assert_eq!(*s.holders.lock().unwrap().last().unwrap(), None);
    let (_b, r) = Raw::hello(&s.path, 8, false).await;
    assert_eq!(holder_pid(&r), 8);
    assert_eq!(*s.svc.attaches.lock().unwrap(), [true, false, true]);
}

#[tokio::test]
async fn stop_instance_replies_then_ends_serving() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    a.send(2, Command::StopInstance).await;
    assert!(matches!(a.reply().await, (2, Ok(Reply::Ok))));
    assert!(a.recv().await.is_none());
    assert_eq!(s.task.await.unwrap(), ServeEnd::StopRequested);
    assert_eq!(*s.svc.attaches.lock().unwrap(), [true, false]);
}

#[tokio::test]
async fn shutdown_tells_the_client_why() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    s.shutdown.send(Some("sigterm".into())).unwrap();
    match a.recv().await {
        Some(ServerFrame::Event {
            event: Event::InstanceStopped { reason },
        }) => assert_eq!(reason, "sigterm"),
        other => panic!("{other:?}"),
    }
    assert!(a.recv().await.is_none());
    assert_eq!(s.task.await.unwrap(), ServeEnd::Shutdown("sigterm".into()));
}

#[tokio::test]
async fn silent_client_is_dropped_after_heartbeat_timeout() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    // 握手用真实时间，之后暂停：只有推进时钟才会超时。
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(30)).await;
    a.send(
        2,
        Command::Heartbeat {
            last_input_ms: now_ms(),
        },
    )
    .await;
    assert!(matches!(a.reply().await, (2, Ok(Reply::Ok))));
    tokio::time::advance(Duration::from_secs(34)).await;
    a.send(
        3,
        Command::Hello {
            client: client(7),
            force: false,
        },
    )
    .await;
    assert!(matches!(a.reply().await, (3, Ok(_))));
    tokio::time::advance(Duration::from_secs(36)).await;
    assert!(a.recv().await.is_none());
    assert_eq!(*s.holders.lock().unwrap().last().unwrap(), None);
}

/// 没有空闲脱离：接入者很久没输入，只要心跳在，连接就留着。
#[tokio::test]
async fn a_long_idle_holder_is_not_detached() {
    let s = start();
    let mut a = Raw::connect(&s.path).await;
    let mut c = client(7);
    c.last_input_ms = now_ms() - 31 * 60_000;
    a.send(
        1,
        Command::Hello {
            client: c,
            force: false,
        },
    )
    .await;
    assert!(matches!(a.reply().await, (1, Ok(_))));
    a.send(
        2,
        Command::Heartbeat {
            last_input_ms: now_ms() - 31 * 60_000,
        },
    )
    .await;
    assert!(matches!(a.reply().await, (2, Ok(Reply::Ok))));
}

#[tokio::test]
async fn slow_command_does_not_block_heartbeats() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    a.send(2, Command::ListFlows).await;
    let t = now_ms() + 5;
    a.send(3, Command::Heartbeat { last_input_ms: t }).await;
    assert!(matches!(a.reply().await, (3, Ok(Reply::Ok))));
    let last = s.holders.lock().unwrap().last().cloned().flatten().unwrap();
    assert_eq!(last.last_input_ms, t);
}

#[tokio::test]
async fn unknown_command_gets_unsupported() {
    let s = start();
    let (mut a, _) = Raw::hello(&s.path, 7, false).await;
    a.wr.write_all(b"{\"id\":9,\"cmd\":\"from_the_future\"}\n")
        .await
        .unwrap();
    assert!(matches!(
        a.reply().await,
        (9, Err(PortError::Unsupported(_)))
    ));
}
