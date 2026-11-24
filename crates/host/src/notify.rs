//! 通知出口（architecture §8.10）：用户在全局 `conix.yaml` 里配的命令。Conix 不内建网络客户端：
//! 桌面、ntfy、webhook、邮件都是一条命令，凭据留在用户自己的环境里。只认全局配置——项目里的
//! 文件来自仓库，不能让一个仓库在别人的机器上执行命令、把数据往外发。

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use port::{Alert, NoticeLevel, SinkResult};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

const SINK_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
#[serde(default)]
pub struct NotifyConfig {
    pub when: When,
    pub min_level: Level,
    pub sinks: Vec<SinkConfig>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum When {
    /// 没有 TUI 接着时才发：人在屏幕前就不打扰。
    #[default]
    Detached,
    Always,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Info,
    #[default]
    Warn,
    Error,
}

impl Level {
    pub fn of(l: NoticeLevel) -> Level {
        match l {
            NoticeLevel::Info => Level::Info,
            NoticeLevel::Warn => Level::Warn,
            NoticeLevel::Error => Level::Error,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct SinkConfig {
    pub name: String,
    /// 程序与参数；不经 shell（要 shell 就自己写 `["sh", "-c", "…"]`）。
    pub command: Vec<String>,
    /// `json`：把这条通知（几条合成的就是全部）以 JSON 写进命令的 stdin。
    #[serde(default)]
    pub stdin: Option<String>,
    #[serde(default)]
    pub min_level: Option<Level>,
}

/// 占位符要用到的、这个实例的事实。
#[derive(Debug, Clone)]
pub struct Origin {
    pub instance: String,
    pub host: String,
    pub project: String,
}

pub struct Notifier {
    pub cfg: NotifyConfig,
    origin: Origin,
    log: Option<PathBuf>,
    /// 一阵里来的合成一条送（缺省 60 秒）。
    pub window_ms: u64,
}

impl Notifier {
    pub fn new(cfg: NotifyConfig, origin: Origin, log: Option<PathBuf>) -> Notifier {
        Notifier {
            cfg,
            origin,
            log,
            window_ms: 60_000,
        }
    }

    /// 这条够不够级别发出去（全局门槛）。
    pub fn wants(&self, a: &Alert) -> bool {
        !self.cfg.sinks.is_empty() && Level::of(a.level) >= self.cfg.min_level
    }

    /// 把一批合成一条，发给每个够级别的出口；回每个出口的结果。
    pub async fn send(&self, alerts: &[Alert]) -> Vec<SinkResult> {
        let Some(top) = alerts.iter().map(|a| Level::of(a.level)).max() else {
            return vec![];
        };
        let (title, text) = match alerts {
            [one] => (one.title.clone(), one.text.clone()),
            many => (
                format!("{} things need you", many.len()),
                many.iter()
                    .map(|a| format!("· {}", a.title))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
        };
        let kind = match alerts {
            [one] => serde_json::to_value(one.kind)
                .ok()
                .and_then(|v| v.as_str().map(str::to_string))
                .unwrap_or_default(),
            _ => "many".into(),
        };
        let level = match top {
            Level::Info => "info",
            Level::Warn => "warn",
            Level::Error => "error",
        };
        let attach = format!(
            "ssh {} -t conix attach {}",
            self.origin.host, self.origin.instance
        );
        let vars = [
            ("{instance}", self.origin.instance.as_str()),
            ("{project}", self.origin.project.as_str()),
            ("{kind}", kind.as_str()),
            ("{level}", level),
            ("{title}", title.as_str()),
            ("{text}", text.as_str()),
            ("{attach}", attach.as_str()),
        ];
        let payload = serde_json::json!({
            "instance": self.origin.instance,
            "host": self.origin.host,
            "project": self.origin.project,
            "level": level,
            "title": title,
            "text": text,
            "attach": attach,
            "alerts": alerts,
        });
        let mut out = vec![];
        for sink in &self.cfg.sinks {
            if top < sink.min_level.unwrap_or(Level::Info) {
                continue;
            }
            let args: Vec<String> = sink
                .command
                .iter()
                .map(|a| vars.iter().fold(a.clone(), |acc, (k, v)| acc.replace(k, v)))
                .collect();
            let stdin = (sink.stdin.as_deref() == Some("json")).then(|| payload.to_string());
            let mut res = run(&args, stdin.as_deref()).await;
            if res.is_err() {
                res = run(&args, stdin.as_deref()).await;
            }
            let r = SinkResult {
                name: sink.name.clone(),
                ok: res.is_ok(),
                detail: res.err().unwrap_or_default(),
            };
            self.record(&kind, &r);
            out.push(r);
        }
        out
    }

    fn record(&self, kind: &str, r: &SinkResult) {
        let Some(path) = &self.log else { return };
        let line = serde_json::json!({
            "at_ms": crate::instance::now_ms(),
            "kind": kind,
            "sink": r.name,
            "ok": r.ok,
            "detail": r.detail,
        });
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            let _ = std::io::Write::write_all(&mut f, format!("{line}\n").as_bytes());
        }
    }
}

async fn run(args: &[String], stdin: Option<&str>) -> Result<(), String> {
    let (prog, rest) = args.split_first().ok_or("the command is empty")?;
    let mut cmd = tokio::process::Command::new(prog);
    cmd.args(rest)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| format!("{prog}: {e}"))?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        let _ = pipe.write_all(text.as_bytes()).await;
    }
    match tokio::time::timeout(SINK_TIMEOUT, child.wait_with_output()).await {
        Err(_) => Err(format!("{prog}: no answer in 10s")),
        Ok(Err(e)) => Err(format!("{prog}: {e}")),
        Ok(Ok(o)) if o.status.success() => Ok(()),
        Ok(Ok(o)) => Err(format!(
            "{prog}: {} {}",
            o.status,
            String::from_utf8_lossy(&o.stderr).trim()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alert(level: NoticeLevel, title: &str) -> Alert {
        Alert {
            kind: port::AlertKind::JobFailed,
            level,
            title: title.into(),
            text: "provider 429".into(),
            at_ms: 1,
            target: None,
        }
    }

    fn notifier(dir: &std::path::Path, sinks: Vec<SinkConfig>) -> Notifier {
        Notifier::new(
            NotifyConfig {
                when: When::Detached,
                min_level: Level::Warn,
                sinks,
            },
            Origin {
                instance: "app".into(),
                host: "box".into(),
                project: "/work/app".into(),
            },
            Some(dir.join("notify.jsonl")),
        )
    }

    /// 参数级替换、整条 JSON 进 stdin、每个出口自己的门槛、失败的照实回、都记进 notify.jsonl。
    #[tokio::test]
    async fn sinks_get_their_arguments_and_stdin_and_failures_are_reported() {
        let d = tempfile::tempdir().unwrap();
        let args = d.path().join("args");
        let body = d.path().join("body");
        let n = notifier(
            d.path(),
            vec![
                SinkConfig {
                    name: "args".into(),
                    command: vec![
                        "sh".into(),
                        "-c".into(),
                        format!("printf '%s|%s' \"$0\" \"$1\" > {}", args.display()),
                        "{title}".into(),
                        "{attach}".into(),
                    ],
                    stdin: None,
                    min_level: None,
                },
                SinkConfig {
                    name: "body".into(),
                    command: vec![
                        "sh".into(),
                        "-c".into(),
                        format!("cat > {}", body.display()),
                    ],
                    stdin: Some("json".into()),
                    min_level: Some(Level::Error),
                },
                SinkConfig {
                    name: "broken".into(),
                    command: vec!["false".into()],
                    stdin: None,
                    min_level: None,
                },
            ],
        );
        let r = n
            .send(&[alert(NoticeLevel::Error, "job \"x\" failed")])
            .await;
        assert_eq!(
            r.iter()
                .map(|r| (r.name.as_str(), r.ok))
                .collect::<Vec<_>>(),
            [("args", true), ("body", true), ("broken", false)]
        );
        assert_eq!(
            std::fs::read_to_string(&args).unwrap(),
            "job \"x\" failed|ssh box -t conix attach app"
        );
        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&body).unwrap()).unwrap();
        assert_eq!(v["alerts"][0]["text"], "provider 429");
        let log = std::fs::read_to_string(d.path().join("notify.jsonl")).unwrap();
        assert_eq!(log.lines().count(), 3);

        // 只够 warn 的不发给只收 error 的出口；几条合成一条。
        std::fs::remove_file(&body).unwrap();
        let r = n
            .send(&[alert(NoticeLevel::Warn, "a"), alert(NoticeLevel::Warn, "b")])
            .await;
        assert!(r.iter().all(|r| r.name != "body"));
        assert!(!body.exists());
        assert_eq!(
            std::fs::read_to_string(&args).unwrap().split('|').next(),
            Some("2 things need you")
        );
    }
}
