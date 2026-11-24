//! Agix 协议（`agix/docs/daemon_rpc_protocol.md`）的客户端一侧：NDJSON 分帧的 JSON-RPC 2.0，
//! 按 `id` 配对回复，通知（`session.event`、`daemon.event`）进一个 channel；以及 Conix 用到的
//! 那几种帧载荷。载荷一律宽松地读：不认识的 `kind`、多出来的字段都跳过，协议只会往下长。

pub mod frames;
pub mod hostlib;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// 一次调用的失败：对端的错误响应，或连接已经没了。
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum RpcError {
    #[error("{message} ({code})")]
    Remote {
        code: i64,
        message: String,
        data: Value,
    },
    #[error("connection closed")]
    Closed,
}

impl RpcError {
    pub fn code(&self) -> Option<i64> {
        match self {
            RpcError::Remote { code, .. } => Some(*code),
            RpcError::Closed => None,
        }
    }
}

/// 不带 `id` 的帧：会话的流式事件或实例级的异步通知。
#[derive(Debug, Clone, PartialEq)]
pub enum Notification {
    Session {
        session_id: String,
        turn_id: Option<String>,
        event: Value,
    },
    Daemon(Value),
}

#[derive(Serialize)]
struct Request<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: Value,
}

#[derive(Deserialize)]
struct Incoming {
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    code: i64,
    message: String,
    #[serde(default)]
    data: Value,
}

type Waiters = Arc<Mutex<Option<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>>;

/// 一条连接。可以并发调用：每个请求各等各的回复，读循环从不等任何一个（协议 §5.2）。
#[derive(Clone)]
pub struct Client {
    next: Arc<AtomicU64>,
    out: mpsc::UnboundedSender<String>,
    waiters: Waiters,
}

impl Client {
    /// 在一对读写端上起读写两个任务；返回客户端与通知流。读端结束时所有在等的调用以
    /// [`RpcError::Closed`] 失败，通知流随之关闭。
    pub fn spawn<R, W>(read: R, write: W) -> (Client, mpsc::UnboundedReceiver<Notification>)
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (out, mut out_rx) = mpsc::unbounded_channel::<String>();
        let (note_tx, note_rx) = mpsc::unbounded_channel();
        let waiters: Waiters = Arc::new(Mutex::new(Some(HashMap::new())));
        tokio::spawn(async move {
            let mut write = write;
            while let Some(line) = out_rx.recv().await {
                if write.write_all(line.as_bytes()).await.is_err()
                    || write.write_all(b"\n").await.is_err()
                    || write.flush().await.is_err()
                {
                    break;
                }
            }
        });
        let w = waiters.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(msg) = serde_json::from_str::<Incoming>(&line) else {
                    continue;
                };
                match (msg.id.as_ref().and_then(Value::as_u64), msg.method) {
                    (Some(id), None) => {
                        let tx = w.lock().unwrap().as_mut().and_then(|m| m.remove(&id));
                        if let Some(tx) = tx {
                            let r = match (msg.result, msg.error) {
                                (_, Some(e)) => Err(RpcError::Remote {
                                    code: e.code,
                                    message: e.message,
                                    data: e.data,
                                }),
                                (r, None) => Ok(r.unwrap_or(Value::Null)),
                            };
                            let _ = tx.send(r);
                        }
                    }
                    (None, Some(m)) => {
                        let n = match m.as_str() {
                            "session.event" => {
                                let p = msg.params;
                                Notification::Session {
                                    session_id: p["session_id"].as_str().unwrap_or("").into(),
                                    turn_id: p["turn_id"].as_str().map(String::from),
                                    event: p.get("event").cloned().unwrap_or(Value::Null),
                                }
                            }
                            "daemon.event" => Notification::Daemon(msg.params),
                            _ => continue,
                        };
                        if note_tx.send(n).is_err() {
                            // 没人收通知也照样配对回复。
                        }
                    }
                    _ => {}
                }
            }
            // 连接没了：在等的一律失败，之后的调用也立刻失败。
            if let Some(m) = w.lock().unwrap().take() {
                for (_, tx) in m {
                    let _ = tx.send(Err(RpcError::Closed));
                }
            }
        });
        (
            Client {
                next: Arc::new(AtomicU64::new(1)),
                out,
                waiters,
            },
            note_rx,
        )
    }

    pub async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        match self.waiters.lock().unwrap().as_mut() {
            Some(m) => m.insert(id, tx),
            None => return Err(RpcError::Closed),
        };
        let line = serde_json::to_string(&Request {
            jsonrpc: "2.0",
            id,
            method,
            params,
        })
        .expect("a request always serializes");
        if self.out.send(line).is_err() {
            self.waiters.lock().unwrap().as_mut().map(|m| m.remove(&id));
            return Err(RpcError::Closed);
        }
        rx.await.unwrap_or(Err(RpcError::Closed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// 一个在 duplex 另一端答话的假服务端：先推一帧事件，再乱序回两个请求。
    #[tokio::test]
    async fn replies_pair_by_id_and_notifications_flow() {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (r, w) = tokio::io::split(ours);
        let (client, mut notes) = Client::spawn(r, w);
        tokio::spawn(async move {
            let (r, mut w) = tokio::io::split(theirs);
            let mut lines = BufReader::new(r).lines();
            let a: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let b: Value =
                serde_json::from_str(&lines.next_line().await.unwrap().unwrap()).unwrap();
            let ev = json!({"jsonrpc":"2.0","method":"session.event","params":{"session_id":"s1","turn_id":null,"event":{"kind":"turn_start"}}});
            let rb = json!({"jsonrpc":"2.0","id":b["id"],"error":{"code":-32004,"message":"nope"}});
            let ra = json!({"jsonrpc":"2.0","id":a["id"],"result":{"m":a["method"]}});
            for v in [ev, rb, ra] {
                w.write_all(format!("{v}\nnot json\n").as_bytes())
                    .await
                    .unwrap();
            }
            drop(w);
            let _ = lines.next_line().await;
        });
        let (x, y) = tokio::join!(
            client.call("a.one", json!({})),
            client.call("b.two", json!({}))
        );
        assert_eq!(x.unwrap(), json!({"m":"a.one"}));
        assert_eq!(y.unwrap_err().code(), Some(-32004));
        assert_eq!(
            notes.recv().await.unwrap(),
            Notification::Session {
                session_id: "s1".into(),
                turn_id: None,
                event: json!({"kind":"turn_start"})
            }
        );
    }

    #[tokio::test]
    async fn closed_connection_fails_waiting_and_later_calls() {
        let (ours, theirs) = tokio::io::duplex(1 << 16);
        let (r, w) = tokio::io::split(ours);
        let (client, _notes) = Client::spawn(r, w);
        let pending = tokio::spawn({
            let c = client.clone();
            async move { c.call("x", json!({})).await }
        });
        tokio::task::yield_now().await;
        drop(theirs);
        assert_eq!(pending.await.unwrap(), Err(RpcError::Closed));
        assert_eq!(client.call("y", json!({})).await, Err(RpcError::Closed));
    }
}
