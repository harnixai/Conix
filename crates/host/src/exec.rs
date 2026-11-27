//! 执行服务的 Conix 实现（flows.md §7.14）：实例里替项目、包、用户自己的配置起的进程都经这里——
//! 命令门禁、`plan` 的 run、包 JS 的 `flow.exec`（流程库经 `RunnerConfig::exec` 调）、`conix flow doctor`
//! 里包的自检、通知出口。
//!
//! 后端是 Agix 的沙箱（协议 §6.9 `exec.run`，装配时 `EmbedConfig.host_exec` 打开）：`read` 档只写得了这次
//! 调用自己的临时目录，`workspace` 档另加它点名的目录，网络按包的声明，环境不继承，禁读名单照传。沙箱
//! 兜不住（Linux 上没有 bwrap）时 Agix 拒绝，除非全局 `conix.yaml` 的 `flows.exec_unconfined: allow`。
//! 没有引擎的地方（单元测试）退回普通子进程，结果的 `enforced` 照实说什么都没守。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use harnix_flow::runner::{PlainExec, run_plain};
use harnix_flow::{
    Enforcement, Exec, ExecError, ExecOutcome, ExecProfile, ExecRequest, OutputKeep,
};
use serde_json::{Value, json};
use wire::RpcError;

use crate::engine::Engine;

/// Agix 的上限（协议 §6.9）：时限至多 30 分钟，每路输出至多 4 MiB。
const MAX_TIMEOUT_MS: u128 = 1_800_000;
const MAX_OUTPUT: usize = 4 << 20;
/// Agix 拒绝的错误码：沙箱兜不住、managed 层的规则点了它。
const EXEC_REFUSED: i64 = -32030;
const INVALID_PARAMS: i64 = -32602;

/// Conix 的 [`Exec`]：`host` 档位（用户自己在全局配置里写的命令）不进沙箱，别的经 Agix。
pub struct ConixExec {
    engine: Option<Arc<dyn Engine>>,
    allow_unconfined: bool,
}

impl ConixExec {
    /// 经这条引擎连接（要在装配时打开了 `host_exec`）。`allow_unconfined`：沙箱兜不住时照样跑。
    pub fn agix(engine: Arc<dyn Engine>, allow_unconfined: bool) -> Self {
        ConixExec {
            engine: Some(engine),
            allow_unconfined,
        }
    }

    /// 没有引擎：普通子进程，什么都不守。
    pub fn plain() -> Self {
        ConixExec {
            engine: None,
            allow_unconfined: false,
        }
    }
}

#[async_trait]
impl Exec for ConixExec {
    async fn run(&self, req: ExecRequest) -> Result<ExecOutcome, ExecError> {
        match &self.engine {
            // 用户自己配的出口就是他自己的命令，凭据也在他的环境里：隔离它挡不住谁，只会让出口用不了。
            _ if req.profile == ExecProfile::Host => {
                run_plain(req, Enforcement::none("plain")).await
            }
            None => PlainExec.run(req).await,
            Some(engine) => run_agix(engine, req, self.allow_unconfined).await,
        }
    }
}

static NEXT_RUN: AtomicU64 = AtomicU64::new(1);

/// 调用被丢下（运行被停、门禁超出自己的时限）时让 Agix 停掉那个进程组，而不是等它跑完。
struct CancelOnDrop {
    engine: Option<Arc<dyn Engine>>,
    run_id: String,
}

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let (Some(engine), Ok(rt)) = (self.engine.take(), tokio::runtime::Handle::try_current())
        else {
            return;
        };
        let params = json!({"run_id": std::mem::take(&mut self.run_id)});
        rt.spawn(async move {
            let _ = engine.call("exec.cancel", params).await;
        });
    }
}

async fn run_agix(
    engine: &Arc<dyn Engine>,
    req: ExecRequest,
    allow_unconfined: bool,
) -> Result<ExecOutcome, ExecError> {
    let run_id = format!("conix-exec-{}", NEXT_RUN.fetch_add(1, Ordering::Relaxed));
    let params = agix_params(&req, allow_unconfined, &run_id)?;
    let mut armed = CancelOnDrop {
        engine: Some(engine.clone()),
        run_id,
    };
    let got = engine.call("exec.run", params).await;
    armed.engine = None;
    match got {
        Ok(v) => outcome(v, req.keep),
        Err(RpcError::Remote { code, message, .. }) => Err(match code {
            EXEC_REFUSED => ExecError::Refused(message),
            INVALID_PARAMS => ExecError::NotStarted(message),
            _ => ExecError::Lost(message),
        }),
        Err(RpcError::Closed) => Err(ExecError::Lost("the engine's connection closed".into())),
    }
}

/// 一个请求在 `exec.run` 上的样子。
fn agix_params(
    req: &ExecRequest,
    allow_unconfined: bool,
    run_id: &str,
) -> Result<Value, ExecError> {
    let (profile, writable) = match &req.profile {
        ExecProfile::Read => ("read", vec![]),
        ExecProfile::Workspace { writable } => ("write", granted(&req.cwd, writable)),
        ExecProfile::Host => unreachable!("the host profile never goes into the sandbox"),
    };
    let env: serde_json::Map<String, Value> = req
        .env
        .iter()
        .filter_map(|(k, v)| Some((k.to_str()?.to_string(), v.to_string_lossy().into_owned())))
        // `TMPDIR` 是 Agix 给这次调用建的那个：`read` 档里别处都写不了，传宿主的进去门禁就写不了临时文件。
        .filter(|(k, _)| k != "TMPDIR")
        .map(|(k, v)| (k, Value::String(v)))
        .collect();
    let max_output = match req.keep {
        OutputKeep::Head(n) => n.clamp(1, MAX_OUTPUT),
        // Agix 留的是开头；要结尾的（门禁的结论在最后）取满上限，再在这边截出结尾。
        OutputKeep::Tail(_) => MAX_OUTPUT,
    };
    let mut p = json!({
        "argv": req.argv,
        "cwd": req.cwd,
        "profile": profile,
        "network": req.network,
        "env": env,
        "timeout_ms": req.timeout.as_millis().clamp(1, MAX_TIMEOUT_MS) as u64,
        "max_output_bytes": max_output,
        "allow_unconfined": allow_unconfined,
        "run_id": run_id,
    });
    if profile == "write" {
        p["writable"] = json!(writable);
    }
    if !req.deny_read.is_empty() {
        p["deny_read"] = json!(req.deny_read);
    }
    if let Some(s) = &req.stdin {
        p["stdin"] = json!(String::from_utf8_lossy(s));
    }
    Ok(p)
}

/// 档位点名的目录里 Agix 收得下的：它要每一项都已存在。还没有的、在工作目录里的先建出来（站本来就可以写
/// 那里，空目录不进 git）；工作目录之外还没有的不给——不能为了给权限去别处建目录。
fn granted(cwd: &Path, writable: &[PathBuf]) -> Vec<PathBuf> {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    writable
        .iter()
        .filter(|w| w.is_dir() || (w.starts_with(&cwd) && std::fs::create_dir_all(w).is_ok()))
        .cloned()
        .collect()
}

fn outcome(v: Value, keep: OutputKeep) -> Result<ExecOutcome, ExecError> {
    if v["cancelled"].as_bool() == Some(true) {
        return Err(ExecError::Lost("cancelled: the engine stopped it".into()));
    }
    let text = |k: &str| v[k].as_str().unwrap_or_default().as_bytes().to_vec();
    let (mut stdout, mut stderr) = (text("stdout"), text("stderr"));
    let mut truncated = v["truncated"].as_bool().unwrap_or(false);
    if let OutputKeep::Tail(n) = keep {
        for s in [&mut stdout, &mut stderr] {
            if s.len() > n {
                s.drain(..s.len() - n);
                truncated = true;
            }
        }
    }
    let level = v["enforcement"].as_str().unwrap_or("none");
    let full = level == "full";
    let enforced = Enforcement {
        backend: "agix".into(),
        // `partial` 时 Agix 只给一行行的文字，说不清是哪一面没守住：两面都不算守住，差在哪看 `unmet`。
        filesystem: full,
        network: full,
        unmet: v["unmet"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
    };
    Ok(ExecOutcome {
        code: v["code"].as_i64().map(|c| c as i32),
        signal: v["signal"].as_i64().map(|s| s as i32),
        stdout,
        stderr,
        timed_out: v["timed_out"].as_bool().unwrap_or(false),
        truncated,
        ms: v["ms"].as_u64().unwrap_or(0),
        enforced,
    })
}

/// 要一直留着的东西（起来的引擎本身）。
pub type Keep = Box<dyn std::any::Any + Send + Sync>;

/// 在这个目录（项目）上起一个引擎，交出连接与 [`Keep`]。
pub type StartEngine = Box<
    dyn FnOnce(
            PathBuf,
        )
            -> futures::future::BoxFuture<'static, Result<(Arc<dyn Engine>, Keep), String>>
        + Send,
>;

/// 第一次要起进程时才起引擎的执行服务：命令行里（`conix flow doctor` 跑包的自检）没有实例，进程也要进沙箱，
/// 而大多数命令一个进程都不起，不该为它们起引擎。
pub struct LazyExec {
    start: std::sync::Mutex<Option<StartEngine>>,
    allow_unconfined: bool,
    ready: tokio::sync::OnceCell<Result<(ConixExec, Keep), String>>,
}

impl LazyExec {
    pub fn new(start: StartEngine, allow_unconfined: bool) -> Self {
        LazyExec {
            start: std::sync::Mutex::new(Some(start)),
            allow_unconfined,
            ready: tokio::sync::OnceCell::new(),
        }
    }
}

#[async_trait]
impl Exec for LazyExec {
    async fn run(&self, req: ExecRequest) -> Result<ExecOutcome, ExecError> {
        if req.profile == ExecProfile::Host {
            return ConixExec::plain().run(req).await;
        }
        let ready = self
            .ready
            .get_or_init(|| async {
                let start = self.start.lock().unwrap().take();
                let start = start.ok_or_else(|| "already tried".to_string())?;
                // 第一个进程的工作目录就是项目：引擎在它的 cwd 里补 `.conix/.gitignore`，不能是命令行恰好站着的地方。
                let (engine, keep) = start(req.cwd.clone()).await?;
                Ok((ConixExec::agix(engine, self.allow_unconfined), keep))
            })
            .await;
        match ready {
            Ok((exec, _)) => exec.run(req).await,
            Err(e) => Err(ExecError::Refused(format!(
                "the engine's sandbox did not start: {e}"
            ))),
        }
    }
}

static SERVICE: OnceLock<Arc<dyn Exec>> = OnceLock::new();

/// 实例起来、引擎连上之后装上它的执行服务；要在任何人取 [`service`] 之前。
pub fn install(exec: Arc<dyn Exec>) {
    if SERVICE.set(exec).is_err() {
        tracing::error!("the exec service was taken before the engine's was installed");
    }
}

/// 这个进程里的执行服务：流程、体检、通知用的是同一个。没装（单元测试）是普通子进程。
pub fn service() -> Arc<dyn Exec> {
    SERVICE.get_or_init(|| Arc::new(ConixExec::plain())).clone()
}

/// 宿主自己起的进程（不在哪个运行里）：经 `exec` 起，记一行宿主日志，与流程库运行之外的那行同一个样子，
/// 不记输出。
pub async fn run_logged(
    exec: &dyn Exec,
    what: &str,
    req: ExecRequest,
) -> Result<ExecOutcome, ExecError> {
    let got = exec.run(req.clone()).await;
    let line = harnix_flow::runner::exec_record(what, &req, &got);
    tracing::info!(target: "flow::exec", exec = %line, "process run");
    got
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::Duration;

    fn remote(code: i64, message: &str) -> RpcError {
        RpcError::Remote {
            code,
            message: message.into(),
            data: Value::Null,
        }
    }

    /// 记下请求、按给的回答的引擎。
    struct Fake {
        calls: Mutex<Vec<(String, Value)>>,
        answer: Result<Value, RpcError>,
    }

    #[async_trait]
    impl Engine for Fake {
        async fn call(&self, method: &str, params: Value) -> Result<Value, RpcError> {
            self.calls.lock().unwrap().push((method.into(), params));
            self.answer.clone()
        }
    }

    fn fake(answer: Result<Value, RpcError>) -> Arc<Fake> {
        Arc::new(Fake {
            calls: Mutex::default(),
            answer,
        })
    }

    fn req(profile: ExecProfile) -> ExecRequest {
        let mut r = ExecRequest::new(
            vec!["true".into()],
            std::env::temp_dir(),
            Duration::from_secs(5),
        );
        r.env = vec![
            ("PATH".into(), std::env::var_os("PATH").unwrap_or_default()),
            ("TMPDIR".into(), "/tmp".into()),
        ];
        r.profile = profile;
        r
    }

    /// 没有引擎时是普通子进程，照实说什么都没守；用户自己的出口在有引擎时也不进沙箱。
    #[tokio::test]
    async fn without_an_engine_it_is_a_plain_process_and_the_hosts_own_never_goes_in() {
        let o = service().run(req(ExecProfile::Read)).await.unwrap();
        assert_eq!((o.code, o.enforced), (Some(0), Enforcement::none("plain")));
        let e = fake(Err(remote(-1, "never called")));
        let exec = ConixExec::agix(e.clone(), false);
        assert_eq!(
            exec.run(req(ExecProfile::Host)).await.unwrap().code,
            Some(0)
        );
        assert!(e.calls.lock().unwrap().is_empty());
    }

    /// 请求原样到 `exec.run`：档位、可写目录（工作目录里没有的先建）、禁读、环境去掉宿主的 `TMPDIR`、时限封顶；
    /// 回答换回结果，要结尾的截出结尾。
    #[tokio::test]
    async fn a_request_goes_to_exec_run_as_it_is_and_the_answer_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = std::fs::canonicalize(dir.path()).unwrap();
        let e = fake(Ok(
            json!({"code": 1, "signal": null, "stdout": "0123456789", "stderr": "",
            "truncated": false, "timed_out": false, "cancelled": false, "ms": 7,
            "enforcement": "partial", "unmet": ["deny_write on paths made later"]}),
        ));
        let exec = ConixExec::agix(e.clone(), true);
        let mut r = req(ExecProfile::Workspace {
            writable: vec![cwd.join("out/new"), PathBuf::from("/nowhere/at/all")],
        });
        r.cwd = cwd.clone();
        r.deny_read = vec![PathBuf::from("/other/side")];
        r.timeout = Duration::from_secs(7200);
        r.keep = OutputKeep::Tail(4);
        let o = exec.run(r).await.unwrap();
        assert_eq!(
            (o.code, o.stdout.as_slice(), o.truncated, o.ms),
            (Some(1), &b"6789"[..], true, 7)
        );
        assert_eq!(
            (o.enforced.backend.as_str(), o.enforced.filesystem),
            ("agix", false)
        );
        assert_eq!(o.enforced.unmet, ["deny_write on paths made later"]);
        let calls = e.calls.lock().unwrap();
        let (method, p) = &calls[0];
        assert_eq!(method, "exec.run");
        assert_eq!(p["profile"], "write");
        assert_eq!(p["writable"], json!([cwd.join("out/new")]));
        assert!(cwd.join("out/new").is_dir());
        assert_eq!(p["deny_read"], json!(["/other/side"]));
        assert!(p["env"].get("TMPDIR").is_none() && p["env"].get("PATH").is_some());
        assert_eq!(
            (p["timeout_ms"].as_u64(), p["allow_unconfined"].as_bool()),
            (Some(1_800_000), Some(true))
        );
        assert_eq!(p["max_output_bytes"], json!(MAX_OUTPUT));
    }

    /// 沙箱兜不住时 Agix 拒绝，进程没起：是 `Refused`，不是一个失败的进程。
    #[tokio::test]
    async fn a_sandbox_that_cannot_hold_refuses() {
        let e = fake(Err(remote(EXEC_REFUSED, "bwrap is not installed")));
        let got = ConixExec::agix(e, false).run(req(ExecProfile::Read)).await;
        assert!(matches!(got, Err(ExecError::Refused(m)) if m.contains("bwrap")));
    }
}
