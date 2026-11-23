//! `codix host`：实例进程的入口（architecture §8.7）。由 `codix new` / `start` 拉起，stdout 与
//! stderr 已接到 `host.log`。认领项目状态 → 认领实例目录 → 起引擎 → 起会话代理 → 监听 socket →
//! 写发现文件 → 服务到被停 → 依次收尾。

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use app::config;
use client::{InstanceFile, Layout, ProjectState};
use host::server::{ServeEnd, ServeOptions};
use host::{Hub, HubConfig};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::watch;

pub async fn run(name: String, project: PathBuf) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(engine::log_filter(
            std::env::var("RUST_LOG").ok().as_deref(),
        ))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();
    let (cfg, problems) = config::load();
    for p in problems {
        tracing::warn!("{p}");
    }
    let (settings, _) = cfg.settings();
    let layout = Layout::new(config::home());
    let project = project
        .canonicalize()
        .with_context(|| format!("project {}", project.display()))?;
    let project_str = project.to_string_lossy().into_owned();
    let started_ms = host::instance::now_ms();
    // 先认领项目：项目已有实例时这个名字不该留下 `meta.json`，否则 `list --all` 多出一个停着的实例。
    let project_claim =
        host::project::claim(&layout, ProjectState::new(&project), &name, started_ms)?;
    let state = project_claim.state().clone();
    let claim = host::instance::claim(&layout, &name, Some(&project_str))?;
    let hostname = host::instance::hostname();
    let version = env!("CARGO_PKG_VERSION").to_string();

    let host_lib: Arc<std::sync::OnceLock<host::plugins::hostlib::HostLibHandler>> =
        Default::default();
    let (embedded, client, notes) = engine::Embedded::start(engine::EngineConfig {
        data_root: layout.home().to_path_buf(),
        project: project.clone(),
        groups_file: layout.groups(&name),
        settings: host::hub::cli_settings(),
        // 引擎自带 `anthropic` 的模型，只在设置里没定义 `providers` 时用：codix.yaml 的 engine.model，CODIX_MODEL 覆盖。
        model: cfg.engine_model(),
        host_lib: host_lib.clone(),
    })
    .await?;
    let clock: host::hub::Clock = Arc::new(host::instance::now_ms);
    let hub = Hub::start(
        Arc::new(client),
        notes,
        HubConfig {
            instance: name.clone(),
            host: hostname.clone(),
            project: project.clone(),
            version: version.clone(),
            started_ms,
            registry: host::hub::Registry::open(state.sessions()),
            jobs: Some(host::jobs::JobStore::open_in(&state)),
            clock,
            engine_model: embedded.model().to_string(),
        },
    )
    .await
    .context("connect to the engine")?;
    hub.set_plugin_dirs(host::plugins::package_dirs(layout.home()));
    let lib_hub = hub.clone();
    let _ = host_lib.set(Arc::new(move |call| {
        Box::pin(host::plugins::hostlib::handle(lib_hub.clone(), call))
    }));
    let j = &cfg.jobs;
    let text = |a: &Option<app::config::Amount>| a.as_ref().map(app::config::Amount::text);
    let (guard, problems) = host::jobs::guard::GuardConfig::parse(&host::jobs::guard::GuardText {
        timeout: text(&j.defaults.timeout),
        max_tokens: text(&j.defaults.max_tokens),
        overlap: j.defaults.overlap.clone(),
        disable_after: j.defaults.disable_after,
        network: j.defaults.network.clone(),
        limit_timeout: text(&j.limits.timeout),
        limit_max_tokens: text(&j.limits.max_tokens),
        daily_tokens: text(&j.daily_tokens),
        max_concurrent: j.max_concurrent,
    });
    for p in problems {
        tracing::warn!("codix.yaml: {p}");
        hub.config_problem(p);
    }
    hub.set_guard_config(guard);
    if !cfg.notify.is_null() {
        match serde_yaml::from_value::<host::notify::NotifyConfig>(cfg.notify.clone()) {
            Ok(n) => {
                let mut n = host::notify::Notifier::new(
                    n,
                    host::notify::Origin {
                        instance: name.clone(),
                        host: hostname.clone(),
                        project: project_str.clone(),
                    },
                    Some(state.dir().join("notify.jsonl")),
                );
                // 只给 LIVE 测试用：合并窗口缩短，免得每条用例干等一分钟。不进 codix.yaml。
                if let Some(ms) = std::env::var("CODIX_TEST_NOTIFY_WINDOW_MS")
                    .ok()
                    .and_then(|v| v.parse::<u64>().ok())
                {
                    n.window_ms = ms;
                }
                hub.set_notifier(n);
            }
            Err(e) => {
                let p = format!("notify is not understood, nothing is sent: {e}");
                tracing::warn!("codix.yaml: {p}");
                hub.config_problem(p);
            }
        }
    }
    if let Some(q) = project_claim.quarantined() {
        tracing::warn!(to = %q.display(), "project state not written on this machine; quarantined");
        let text = format!(
            "Codix state in this project was not written on this machine: moved to {}, starting empty",
            q.display()
        );
        hub.alert(
            port::AlertKind::Quarantined,
            port::NoticeLevel::Warn,
            format!("quarantined:{}", q.display()),
            "project state quarantined".into(),
            text.clone(),
        );
        hub.notice_on_attach(text);
    }
    let flows = host::flows::start(
        &hub,
        host::flows::FlowsConfig {
            flows_dir: layout.flows(),
            runs_dir: state.flow_runs(),
            worktrees_dir: host::flows::worktrees_root(&project),
            workspaces_dir: state.flow_workspaces(),
            policies_file: Some(state.run_policies()),
        },
    )
    .await;
    // 流程起不来不拖累实例：会话与作业照常，流程标签说明没有接上。
    let flows = match flows {
        Ok(f) => Some(f),
        Err(e) => {
            tracing::error!("flows not started: {e:#}");
            hub.notice_on_attach(format!("flows are unavailable in this instance: {e:#}"));
            None
        }
    };

    let listener = claim.bind()?;
    let base = InstanceFile {
        name: name.clone(),
        pid: std::process::id(),
        host: hostname,
        project: Some(project_str),
        started_ms,
        version,
        holder: None,
        supervisor_pid: std::env::var_os(crate::supervise::CHILD_ENV)
            .map(|_| nix::unistd::getppid().as_raw() as u32),
    };
    claim.write_discovery(&base)?;
    tracing::info!(instance = %name, "serving");

    // 常驻实例的日志只追加：每小时看一次，超了就轮转，并把 stdout / stderr 换到新文件上
    // （它们是拉起方接好的 fd，换了文件名还写在旧文件里）。
    let log_path = layout.log(&name);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            if let Ok(true) = client::layout::rotate_log(
                &log_path,
                client::layout::LOG_MAX_BYTES,
                client::layout::LOG_KEEP,
            ) {
                reopen_log(&log_path);
            }
        }
    });

    let (stop_tx, stop_rx) = watch::channel(None);
    let mut term = signal(SignalKind::terminate())?;
    let mut int = signal(SignalKind::interrupt())?;
    // 接住 SIGHUP 就不会被它的缺省动作结束：终端关掉不该带走实例。
    let mut hup = signal(SignalKind::hangup())?;
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = term.recv() => break,
                _ = int.recv() => break,
                _ = hup.recv() => continue,
            }
        }
        let _ = stop_tx.send(Some("stopped".into()));
    });

    let mut opts = ServeOptions::new(stop_rx);
    opts.idle_timeout_ms = settings.idle_timeout_ms;
    // 只给 LIVE 测试用：不缩短的话，测心跳超时断开就得干等 35 秒。不进 codix.yaml，用户不该调它。
    if let Some(ms) = std::env::var("CODIX_TEST_HEARTBEAT_TIMEOUT_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        opts.heartbeat_timeout = std::time::Duration::from_millis(ms);
    }
    let disc_layout = layout.clone();
    let disc_name = name.clone();
    let end = host::server::serve(listener, Arc::new(hub.clone()), opts, move |holder| {
        let f = InstanceFile {
            holder: holder.cloned(),
            ..base.clone()
        };
        let _ = client::layout::write_json_atomic(&disc_layout.discovery(&disc_name), &f);
    })
    .await;
    match end {
        ServeEnd::StopRequested => tracing::info!("stop requested by the attached client"),
        ServeEnd::Shutdown(why) => tracing::info!("shutting down: {why}"),
    }
    if let Some(f) = &flows {
        f.stop().await;
    }
    hub.shutdown();
    embedded.shutdown().await;
    claim.release()?;
    drop(project_claim);
    Ok(())
}

fn reopen_log(path: &std::path::Path) {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    else {
        return;
    };
    for fd in [1, 2] {
        let _ = nix::unistd::dup2(f.as_raw_fd(), fd);
    }
}
