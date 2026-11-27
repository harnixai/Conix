//! 执行服务的 Conix 实现（flows.md §7.14）：实例里替项目、包、用户自己的配置起的进程都经这里——
//! 命令门禁、`plan` 的 run、包 JS 的 `flow.exec`（流程库经 `RunnerConfig::exec` 调）、`conix flow doctor`
//! 里包的自检、通知出口。
//!
//! 现在只有普通子进程这一个后端：档位与网络照传不守，结果的 `enforced` 照实说什么都没守。OS 级的隔离
//! （seatbelt、bwrap，或 Agix 将来给的）以后接在 [`Confine`] 上：它把一个请求包成在隔离里跑的请求（改
//! `argv`、加环境）并说守住了什么，起进程仍是 `run_plain`。调用方、档位规则、记录都不用动。

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use harnix_flow::runner::run_plain;
use harnix_flow::{Enforcement, Exec, ExecError, ExecOutcome, ExecProfile, ExecRequest};

/// 一个请求包进隔离之后的样子：真正要起的请求，与它守住了什么。
pub struct Wrapped {
    pub req: ExecRequest,
    pub enforced: Enforcement,
}

/// 把请求包进 OS 级的隔离。实现按 `req.profile`（`read` 不许写、`workspace` 只许写那几个目录）与
/// `req.network` 给出真正要起的请求；给不了的（平台没有、档位不认识）回 `Err`，进程就不起——宁可不跑，
/// 也不在说要隔离的时候悄悄不隔离。`host` 档位不经它（[`ConixExec`]）。
pub trait Confine: Send + Sync {
    fn wrap(&self, req: ExecRequest) -> Result<Wrapped, String>;
}

/// 不隔离：原样起，说什么都没守。
pub struct Plain;

impl Confine for Plain {
    fn wrap(&self, req: ExecRequest) -> Result<Wrapped, String> {
        Ok(Wrapped {
            req,
            enforced: Enforcement::none("plain"),
        })
    }
}

/// Conix 的 [`Exec`]：`host` 档位（用户自己在全局配置里写的命令）不隔离，别的交给 [`Confine`]。
pub struct ConixExec {
    confine: Arc<dyn Confine>,
}

impl ConixExec {
    pub fn new(confine: Arc<dyn Confine>) -> Self {
        ConixExec { confine }
    }

    pub fn plain() -> Self {
        Self::new(Arc::new(Plain))
    }
}

#[async_trait]
impl Exec for ConixExec {
    async fn run(&self, req: ExecRequest) -> Result<ExecOutcome, ExecError> {
        // 用户自己配的出口就是他自己的命令，凭据也在他的环境里：隔离它挡不住谁，只会让出口用不了。
        let wrapped = if req.profile == ExecProfile::Host {
            Plain.wrap(req)
        } else {
            self.confine.wrap(req)
        }
        .map_err(ExecError::Refused)?;
        run_plain(wrapped.req, wrapped.enforced).await
    }
}

/// 这个进程里的执行服务：流程、体检、通知用的是同一个。以后按配置选隔离后端也在这里选。
pub fn service() -> Arc<dyn Exec> {
    static ONE: OnceLock<Arc<dyn Exec>> = OnceLock::new();
    ONE.get_or_init(|| Arc::new(ConixExec::plain())).clone()
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
    use std::time::Duration;

    struct Refuses;

    impl Confine for Refuses {
        fn wrap(&self, _: ExecRequest) -> Result<Wrapped, String> {
            Err("no sandbox here".into())
        }
    }

    fn req(profile: ExecProfile) -> ExecRequest {
        let mut r = ExecRequest::new(
            vec!["true".into()],
            std::env::temp_dir(),
            Duration::from_secs(5),
        );
        r.env = vec![("PATH".into(), std::env::var_os("PATH").unwrap_or_default())];
        r.profile = profile;
        r
    }

    /// 缺省的后端照实说什么都没守；隔离给不了就不起；用户自己的出口不经隔离。
    #[tokio::test]
    async fn the_plain_backend_says_so_and_a_confinement_that_cannot_refuses() {
        let o = service().run(req(ExecProfile::Read)).await.unwrap();
        assert_eq!((o.code, o.enforced), (Some(0), Enforcement::none("plain")));
        let strict = ConixExec::new(Arc::new(Refuses));
        assert!(matches!(
            strict.run(req(ExecProfile::Read)).await,
            Err(ExecError::Refused(_))
        ));
        assert_eq!(
            strict.run(req(ExecProfile::Host)).await.unwrap().code,
            Some(0)
        );
    }
}
