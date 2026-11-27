use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use app::core::{ShellEffect, Start};
use app::term::{self, CrosstermTerm, Term};
use app::{Core, config};
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use crossterm::event::{
    Event as CtEvent, EventStream, KeyEventKind, MouseButton as CtButton, MouseEventKind,
};
use futures::StreamExt;
use model::{InstOp, Mouse, MouseButton, MouseKind, Msg};
use port::Instances;

mod flow_cli;
mod host_main;
mod instance_config;
mod supervise;

/// 跑在终端里的 agent 工作台
#[derive(Parser)]
#[command(name = "conix", version)]
struct Cli {
    /// 用 mock 后端，不接真实实例：不给值打开演示 `demo`，也可以给场景名或场景文件。
    #[arg(long, global = true, num_args = 0..=1, default_missing_value = "demo")]
    mock: Option<String>,
    /// mock 时间线的加速倍数。
    #[arg(long, default_value_t = 1.0, global = true)]
    speed: f64,
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// 新建实例并接入。
    New {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        project: Option<String>,
    },
    /// 列出本机的实例。
    List {
        #[arg(long)]
        all: bool,
    },
    /// 接入指定实例。
    Attach {
        name: String,
        /// 踢掉当前接入者。
        #[arg(long)]
        force: bool,
    },
    /// 启动一个已停止的实例。
    Start { name: String },
    /// 停止实例。
    Stop { name: String },
    /// 流程包：全局库与项目里的装、更新、回填、导入导出、体检。
    Flow(flow_cli::FlowArgs),
    /// 实例进程的入口，由 new / start 拉起，用户不直接调用。
    #[command(hide = true)]
    Host {
        #[arg(long)]
        name: String,
        #[arg(long)]
        project: std::path::PathBuf,
    },
}

/// 帮助文字按当前语言重写：clap 的 derive 只认字面量，`///` 里的中文只作源码里的说明，
/// 运行时用 `Command` 的构建接口覆盖，所以必须在语言初始化之后、解析参数之前调用。
fn localized_command() -> clap::Command {
    Cli::command()
        .about(i18n::t!("app.cli.about"))
        .mut_arg("mock", |a| a.help(i18n::t!("app.cli.mock")))
        .mut_arg("speed", |a| a.help(i18n::t!("app.cli.speed")))
        .mut_subcommand("new", |c| c.about(i18n::t!("app.cli.new")))
        .mut_subcommand("list", |c| c.about(i18n::t!("app.cli.list")))
        .mut_subcommand("attach", |c| {
            c.about(i18n::t!("app.cli.attach"))
                .mut_arg("force", |a| a.help(i18n::t!("app.cli.attach_force")))
        })
        .mut_subcommand("start", |c| c.about(i18n::t!("app.cli.start")))
        .mut_subcommand("stop", |c| c.about(i18n::t!("app.cli.stop")))
        .mut_subcommand("flow", flow_cli::localize)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[tokio::main]
async fn main() -> Result<()> {
    // 先读配置、定语言：`conix list` 的输出、启动错误、`--help` 都要按语言显示。
    let (cfg, mut problems) = config::load();
    problems.extend(i18n::init(
        &config::language(&cfg),
        Some(&config::home().join("locales")),
    ));
    let cli = Cli::from_arg_matches(&localized_command().get_matches())?;
    // 作业的"下次触发"、触发记录的时刻按本地时间显示；调度器本身就按本地时区算。
    model::set_utc_offset(chrono::Local::now().offset().local_minus_utc());
    // 任何一条 conix 起来时把内置的流程包解进全局库、升级没改过的（flows.md §7.12）；mock 不碰真实的目录。
    if cli.mock.is_none() {
        flow_cli::bootstrap(&config::home());
    }
    if let Some(Cmd::Host { name, project }) = cli.command {
        if std::env::var_os(supervise::CHILD_ENV).is_some() {
            return host_main::run(name, project).await;
        }
        return supervise::run(name, project).await;
    }
    if let Some(Cmd::Flow(args)) = cli.command {
        let artifacts = cfg.flows.artifacts_dir.clone();
        return flow_cli::run(args, &config::home(), artifacts, || project_here(&cfg)).await;
    }
    let (settings, more) = cfg.settings();
    problems.extend(more);
    let mock_mode = cli.mock.is_some();
    let instances: Arc<dyn Instances> = match &cli.mock {
        Some(scenario) => {
            let sc = mock::load_scenario(scenario)
                .or_else(|_| mock::load_seed(scenario).map(|_| mock::Scenario::seed_only(scenario)))
                .with_context(|| i18n::t!("app.no_scenario", name = scenario))?;
            Arc::new(mock::MockInstances::new(
                sc,
                mock::MockOptions {
                    speed: cli.speed,
                    ..mock::MockOptions::default()
                },
            ))
        }
        None => Arc::new(client::SocketInstances::new(
            client::Layout::new(config::home()),
            std::env::current_exe()?,
        )),
    };
    if let Some(Cmd::List { all }) = cli.command {
        for i in instances.list(all).await? {
            // 列宽在这里补，文字里只留占位：换语言时词序可以变，列还是对齐的。
            let now = client::now_ms();
            let since = |ms: u64| model::fmt_elapsed(now.saturating_sub(ms));
            println!(
                "{}",
                i18n::t!(
                    "app.list_row",
                    name = format!("{:<10}", i.name),
                    state = format!("{:<8?}", i.state),
                    project = format!("{:<16}", i.project.unwrap_or_default()),
                    pid = format!("{:<7}", i.pid.map(|p| p.to_string()).unwrap_or_default()),
                    up = format!("{:<8}", i.started_ms.map(since).unwrap_or_default()),
                    sessions = format!("{:<4}", i.sessions),
                    waiting = format!("{:<3}", i.waiting),
                    restarts = if i.restarts > 0 {
                        i18n::t!("app.restarts", n = i.restarts)
                    } else {
                        String::new()
                    },
                    holder = i
                        .holder
                        .map(|h| i18n::t!(
                            "app.held_by_idle",
                            holder = h.label(),
                            idle = since(h.last_input_ms)
                        ))
                        .unwrap_or_default()
                )
            );
        }
        return Ok(());
    }
    let start = match cli.command {
        // 没给项目、又在家目录这类地方：和直接运行 `conix` 一样进个人项目；给了就照给的，不合适由实例目录拒。
        Some(Cmd::New {
            name,
            project: None,
        }) if !mock_mode => Start::Op(InstOp::Create {
            name,
            project: Some(project_here(&cfg)?.to_string_lossy().into_owned()),
        }),
        Some(Cmd::New { name, project }) => Start::Op(InstOp::Create { name, project }),
        Some(Cmd::Attach { name, force, .. }) => Start::Op(InstOp::Attach { name, force }),
        Some(Cmd::Start { name }) => Start::Op(InstOp::Start { name }),
        Some(Cmd::Stop { name }) => {
            instances.stop(&name).await?;
            println!("{}", i18n::t!("app.stopped", name = name));
            return Ok(());
        }
        Some(Cmd::List { .. } | Cmd::Host { .. } | Cmd::Flow(_)) => unreachable!(),
        None => {
            let all = instances.list(true).await?;
            let running: Vec<_> = all
                .iter()
                .filter(|i| i.state == port::InstanceState::Running)
                .cloned()
                .collect();
            let cwd = if mock_mode {
                std::env::current_dir()?.canonicalize()?
            } else {
                project_here(&cfg)?
            };
            // 在项目目录里运行 conix 就接入这个项目的实例（需求 §1 项目级）；停着的就拉起来，
            // 不另建一个——会话归属、作业都记在原来那个实例的目录里。
            match (mock_mode, client::for_project(&all, &cwd)) {
                (false, Some(i)) if i.state == port::InstanceState::Running => {
                    Start::Op(InstOp::Attach {
                        name: i.name.to_string(),
                        force: false,
                    })
                }
                (false, Some(i)) => Start::Op(InstOp::Start {
                    name: i.name.to_string(),
                }),
                _ if running.is_empty() => Start::Op(InstOp::Create {
                    name: None,
                    project: (!mock_mode).then(|| cwd.to_string_lossy().into_owned()),
                }),
                _ => Start::Picker,
            }
        }
    };
    let cfg_err = (!problems.is_empty()).then(|| problems.join(&i18n::t!("app.config.joiner")));
    run(instances, start, cfg, cfg_err, settings, mock_mode).await
}

/// 在这里运行 `conix` 是哪个项目：当前目录；家目录、它的上级、全局目录不能当项目，进个人项目
/// （architecture §8.1），没有就建。
fn project_here(cfg: &config::Config) -> Result<std::path::PathBuf> {
    let cwd = std::env::current_dir()?.canonicalize()?;
    if !client::unfit_project(&cwd, client::user_home().as_deref(), &config::home()) {
        return Ok(cwd);
    }
    let personal = client::personal_project(cfg.instance.personal_project.as_deref());
    std::fs::create_dir_all(&personal).with_context(|| format!("{}", personal.display()))?;
    Ok(personal.canonicalize()?)
}

async fn run(
    instances: Arc<dyn Instances>,
    start: Start,
    cfg: config::Config,
    cfg_err: Option<String>,
    settings: model::Settings,
    mock_mode: bool,
) -> Result<()> {
    let mut t = CrosstermTerm::new()?;
    let negotiated = term::negotiate(
        &mut t,
        cfg.ui.initial_size(),
        term::in_multiplexer(),
        Duration::from_millis(300),
    );
    install_panic_hook();
    t.enter(true)?;
    let result = event_loop(&mut t, instances, start, cfg_err, settings, mock_mode).await;
    t.leave()?;
    term::restore(&mut t, negotiated);
    result
}

fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = crossterm::execute!(
            std::io::stdout(),
            crossterm::event::DisableMouseCapture,
            crossterm::event::DisableBracketedPaste,
            crossterm::terminal::LeaveAlternateScreen
        );
        let _ = crossterm::terminal::disable_raw_mode();
        default(info);
    }));
}

const FRAME_BUDGET: Duration = Duration::from_millis(16);

async fn event_loop(
    t: &mut CrosstermTerm,
    instances: Arc<dyn Instances>,
    start: Start,
    cfg_err: Option<String>,
    settings: model::Settings,
    mock_mode: bool,
) -> Result<()> {
    // 用户的键位覆盖：读不出来就当没有（原因进 /doctor）；写错的那几条不生效，别的照用。
    let keymap = Arc::new(keys::Keymap::load(&config::home().join("keybindings.json")));
    // mock 的数据以一个固定时刻为"现在"：交互运行时让时钟从那一刻起走，耗时、"下次触发"
    // 这些才对得上。真实实例用墙上时间。
    let begin = std::time::Instant::now();
    let epoch = mock::MockOptions::default().epoch_ms;
    let clock: Box<dyn Fn() -> u64 + Send> = if mock_mode {
        Box::new(move || epoch + begin.elapsed().as_millis() as u64)
    } else {
        Box::new(now_ms)
    };
    let (mut core, effects) = Core::new(instances, keymap, t.size(), clock, start, settings);
    if let Some(e) = cfg_err {
        core.handle(Msg::Event(Box::new(port::Event::Notice {
            level: port::NoticeLevel::Error,
            text: e,
        })));
    }
    let mut input = EventStream::new();
    let mut last_draw = Instant::now() - FRAME_BUDGET;
    let mut pending = effects;
    let mut dirty = false;
    loop {
        for e in pending.drain(..) {
            match e {
                ShellEffect::Quit { .. } => {
                    core.detach().await;
                    return Ok(());
                }
                ShellEffect::SetMouseCapture(on) => t.set_mouse(on)?,
                ShellEffect::Redraw => t.clear()?,
                ShellEffect::Bell => print!("\x07"),
                ShellEffect::Clipboard(text) => print!("{}", app::osc52(&text)),
            }
        }
        dirty |= core.take_dirty();
        let due = last_draw + FRAME_BUDGET;
        if dirty && Instant::now() >= due {
            t.draw(&core.frame())?;
            last_draw = Instant::now();
            dirty = false;
        }
        // 帧预算没到就记着脏、到点再画（合帧，architecture §5）；定时只按 model 给的最早时刻。
        let redraw_at = tokio::time::Instant::from_std(due);
        let tick_at = core.deadline().map(|d| {
            let wait = d.saturating_sub(core.now_ms());
            tokio::time::Instant::now() + Duration::from_millis(wait)
        });
        tokio::select! {
            ev = input.next() => match ev {
                Some(Ok(ev)) => pending.extend(on_terminal(&mut core, ev)),
                Some(Err(e)) => return Err(e.into()),
                None => return Ok(()),
            },
            effects = core.next_msg() => pending.extend(effects),
            _ = tokio::time::sleep_until(tick_at.unwrap_or(redraw_at)), if tick_at.is_some() => {
                pending.extend(core.tick());
            }
            _ = tokio::time::sleep_until(redraw_at), if dirty => {}
        }
        pending.extend(core.pump());
    }
}

fn on_terminal(core: &mut Core, ev: CtEvent) -> Vec<ShellEffect> {
    match ev {
        CtEvent::Key(k) if k.kind != KeyEventKind::Release => match keys::Key::from_crossterm(k) {
            Some(key) => core.key(key),
            None => vec![],
        },
        CtEvent::Mouse(m) => {
            let button = |b: CtButton| match b {
                CtButton::Left => MouseButton::Left,
                CtButton::Right => MouseButton::Right,
                CtButton::Middle => MouseButton::Middle,
            };
            let kind = match m.kind {
                MouseEventKind::Down(b) => MouseKind::Down(button(b)),
                MouseEventKind::Up(b) => MouseKind::Up(button(b)),
                MouseEventKind::Drag(b) => MouseKind::Drag(button(b)),
                MouseEventKind::ScrollUp => MouseKind::ScrollUp,
                MouseEventKind::ScrollDown => MouseKind::ScrollDown,
                _ => return vec![],
            };
            core.mouse(Mouse {
                kind,
                col: m.column,
                row: m.row,
            })
        }
        CtEvent::Resize(c, r) => core.resize(tui::Size::new(c, r)),
        CtEvent::Paste(s) => core.handle(Msg::Paste(s)),
        _ => vec![],
    }
}
