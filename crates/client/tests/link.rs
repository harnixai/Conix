//! `SocketBackend` / `SocketInstances` 对着真的 `host::server::serve`（配假的 Service）跑。

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use client::layout::write_json_atomic;
use client::{InstanceFile, InstanceMeta, Layout, SocketBackend, SocketInstances};
use host::server::{ServeEnd, ServeOptions, serve};
use host::{Service, Sink};
use port::link::ServerFrame;
use port::{
    Attach, Backend, ClientInfo, Command, ConnState, Event, InstanceInfo, InstanceState, Instances,
    NoticeLevel, PortError, Reply,
};
use tokio::net::UnixListener;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

#[derive(Default)]
struct Fake {
    sink: Mutex<Option<Sink>>,
}

#[async_trait]
impl Service for Fake {
    fn attach(&self, sink: Option<Sink>) {
        *self.sink.lock().unwrap() = sink;
    }

    fn hello(&self) -> (InstanceInfo, bool, bool) {
        (
            InstanceInfo {
                name: "t".into(),
                host: "h".into(),
                pid: None,
                project: None,
                state: InstanceState::Running,
                started_ms: None,
                stopped_ms: None,
                version: String::new(),
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

    /// `ListJobs` 先推一条事件再回；`ListFlows` 不回。
    async fn handle(&self, cmd: Command) -> Result<Reply, PortError> {
        match cmd {
            Command::ListJobs => {
                if let Some(s) = self.sink.lock().unwrap().as_ref() {
                    let _ = s.send(ServerFrame::Event {
                        event: Event::Notice {
                            level: NoticeLevel::Info,
                            text: "n".into(),
                        },
                    });
                }
                Ok(Reply::Jobs(vec![]))
            }
            Command::ListFlows => std::future::pending().await,
            _ => Ok(Reply::Ok),
        }
    }
}

struct Server {
    task: JoinHandle<ServeEnd>,
    _shutdown: watch::Sender<Option<String>>,
}

fn serve_at(path: &Path) -> Server {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).unwrap();
    let (tx, rx) = watch::channel(None);
    Server {
        task: tokio::spawn(serve(
            listener,
            Arc::new(Fake::default()),
            ServeOptions::new(rx),
            |_| {},
        )),
        _shutdown: tx,
    }
}

fn me(pid: u32) -> ClientInfo {
    ClientInfo {
        pid,
        ..client::this_client()
    }
}

async fn next(rx: &mut mpsc::UnboundedReceiver<Event>) -> Event {
    tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("no event within 5s")
        .expect("event stream closed")
}

fn sock(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("h.sock")
}

#[tokio::test]
async fn calls_are_matched_and_events_come_first() {
    let dir = tempfile::tempdir().unwrap();
    let _s = serve_at(&sock(&dir));
    let b = Arc::new(
        SocketBackend::connect(sock(&dir), None, me(7), false)
            .await
            .unwrap(),
    );
    assert_eq!(b.instance().holder.as_ref().unwrap().pid, 7);
    let mut ev = b.events();
    assert!(b.events().try_recv().is_err());
    // 慢命令挂着，不影响后面的命令配上自己的回复。
    let b2 = b.clone();
    let slow = tokio::spawn(async move { b2.call(Command::ListFlows).await });
    assert_eq!(b.call(Command::ListJobs).await, Ok(Reply::Jobs(vec![])));
    assert!(matches!(ev.try_recv(), Ok(Event::Notice { .. })));
    assert_eq!(
        b.call(Command::Heartbeat { last_input_ms: 1 }).await,
        Ok(Reply::Ok)
    );
    assert!(!slow.is_finished());
}

#[tokio::test]
async fn reconnects_after_the_instance_restarts() {
    let dir = tempfile::tempdir().unwrap();
    let s = serve_at(&sock(&dir));
    let b = Arc::new(
        SocketBackend::connect(sock(&dir), None, me(7), false)
            .await
            .unwrap(),
    );
    let mut ev = b.events();
    let b2 = b.clone();
    let pending = tokio::spawn(async move { b2.call(Command::ListFlows).await });
    tokio::task::yield_now().await;

    // 模拟实例进程没了：连接直接断，不推 `InstanceStopped`。
    s.task.abort();
    let _ = s.task.await;
    assert_eq!(pending.await.unwrap(), Err(PortError::Disconnected));
    assert_eq!(next(&mut ev).await, Event::Connection(ConnState::Lost));
    assert_eq!(
        next(&mut ev).await,
        Event::Connection(ConnState::Reconnecting { attempt: 1 })
    );
    assert_eq!(
        b.call(Command::ListJobs).await,
        Err(PortError::Disconnected)
    );

    let _s = serve_at(&sock(&dir));
    assert_eq!(next(&mut ev).await, Event::Connection(ConnState::Resynced));
    assert_eq!(b.call(Command::ListJobs).await, Ok(Reply::Jobs(vec![])));
}

#[tokio::test]
async fn kicked_client_does_not_reconnect() {
    let dir = tempfile::tempdir().unwrap();
    let _s = serve_at(&sock(&dir));
    let a = SocketBackend::connect(sock(&dir), None, me(7), false)
        .await
        .unwrap();
    let mut ev = a.events();
    match SocketBackend::connect(sock(&dir), None, me(8), false).await {
        Err(PortError::Occupied { holder, .. }) => assert_eq!(holder.pid, 7),
        other => panic!("{:?}", other.map(|b| b.instance().clone())),
    }
    let b = SocketBackend::connect(sock(&dir), None, me(8), true)
        .await
        .unwrap();
    match next(&mut ev).await {
        Event::Kicked { by, .. } => assert_eq!(by.pid, 8),
        other => panic!("{other:?}"),
    }
    tokio::time::sleep(Duration::from_millis(800)).await;
    assert!(
        ev.try_recv().is_err(),
        "no Lost / Reconnecting after Kicked"
    );
    assert_eq!(
        a.call(Command::ListJobs).await,
        Err(PortError::Disconnected)
    );
    assert_eq!(b.call(Command::ListJobs).await, Ok(Reply::Jobs(vec![])));
}

fn dead_pid() -> u32 {
    let mut c = std::process::Command::new("true").spawn().unwrap();
    c.wait().unwrap();
    c.id()
}

fn discovery(name: &str, pid: u32) -> InstanceFile {
    InstanceFile {
        name: name.into(),
        pid,
        host: "h".into(),
        project: Some(format!("/p/{name}")),
        started_ms: 1,
        version: "0".into(),
        holder: None,
        supervisor_pid: None,
    }
}

#[tokio::test]
async fn gives_up_when_the_instance_pid_is_dead() {
    let dir = tempfile::tempdir().unwrap();
    let disc = dir.path().join("instance.json");
    let s = serve_at(&sock(&dir));
    let b = SocketBackend::connect(sock(&dir), Some(disc.clone()), me(7), false)
        .await
        .unwrap();
    let mut ev = b.events();
    write_json_atomic(&disc, &discovery("t", dead_pid())).unwrap();
    s.task.abort();
    assert_eq!(next(&mut ev).await, Event::Connection(ConnState::Lost));
    next(&mut ev).await;
    assert!(matches!(next(&mut ev).await, Event::InstanceStopped { .. }));
}

#[tokio::test]
async fn list_marks_and_cleans_stale_instances() {
    let dir = tempfile::tempdir().unwrap();
    let layout = Layout::new(dir.path());
    for n in ["a", "b", "c"] {
        std::fs::create_dir_all(layout.instance(n)).unwrap();
        let meta = InstanceMeta {
            name: n.into(),
            project: Some(format!("/p/{n}")),
            stopped_ms: Some(5),
            ..Default::default()
        };
        write_json_atomic(&layout.meta(n), &meta).unwrap();
    }
    // a：崩了，留下发现文件与 socket；b：正常停了；c：在跑。
    write_json_atomic(&layout.discovery("a"), &discovery("a", dead_pid())).unwrap();
    std::fs::write(layout.socket("a"), b"").unwrap();
    let _c = serve_at(&layout.socket("c"));
    write_json_atomic(&layout.discovery("c"), &discovery("c", std::process::id())).unwrap();

    let insts = SocketInstances::new(layout.clone(), "/nonexistent");
    let states = |l: Vec<InstanceInfo>| {
        l.into_iter()
            .map(|i| (i.name.0, i.state))
            .collect::<Vec<_>>()
    };
    assert_eq!(
        states(insts.list(false).await.unwrap()),
        [
            ("a".into(), InstanceState::Stale),
            ("c".into(), InstanceState::Running)
        ]
    );
    assert!(!layout.discovery("a").exists() && !layout.socket("a").exists());
    assert_eq!(
        states(insts.list(true).await.unwrap()),
        [
            ("a".into(), InstanceState::Stopped),
            ("b".into(), InstanceState::Stopped),
            ("c".into(), InstanceState::Running)
        ]
    );

    let list = insts.list(true).await.unwrap();
    let c = client::for_project(&list, Path::new("/p/c")).unwrap();
    assert_eq!((c.name.as_str(), c.state), ("c", InstanceState::Running));

    // 接入：探测连接不占位；第二个不带 force 的被告知占用。
    let b = insts.attach("c", Attach::default()).await.unwrap();
    assert_eq!(b.call(Command::ListJobs).await, Ok(Reply::Jobs(vec![])));
    assert!(matches!(
        insts.attach("c", Attach::default()).await,
        Err(PortError::Occupied { .. })
    ));
    assert!(matches!(
        insts.attach("zz", Attach::default()).await,
        Err(PortError::NoSuchInstance(_))
    ));
    assert!(matches!(
        insts.attach("b", Attach::default()).await,
        Err(PortError::Failed(_))
    ));
}
