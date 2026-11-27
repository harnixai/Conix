//! `conix flow …`：流程包的管理，只在命令行（docs/design/flows.md §7.12）。全局库在 `~/.conix/flows/`，
//! 项目跑的是装进项目 `.conix/flows/` 的；规矩在 `host::flows::manage`，这里只管参数、给人看计划、确认、
//! 做完后告诉在跑的实例重读流程。不需要实例。

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use host::flows::doctor::{self, Level};
use host::flows::library::{BootNote, BuiltinState, Library};
use host::flows::manage::{self, Action, Ctx, Plan, Status, Where};
use host::flows::package;

#[derive(clap::Args)]
pub struct FlowArgs {
    #[command(subcommand)]
    pub cmd: FlowCmd,
    /// 项目目录；省略则是当前目录所在的项目。
    #[arg(short = 'C', long, global = true, value_name = "DIR")]
    pub project_dir: Option<PathBuf>,
    /// 不问，直接做。
    #[arg(short, long, global = true)]
    pub yes: bool,
    /// 只列出要做什么，不做。
    #[arg(long, global = true)]
    pub dry_run: bool,
}

#[derive(clap::Subcommand)]
pub enum FlowCmd {
    /// 列出流程包与状态：项目的（缺省）、全局的、两边对照。
    List {
        #[arg(long, conflicts_with = "all")]
        global: bool,
        #[arg(long)]
        all: bool,
    },
    /// 一个包的说明、流程、命令、版本、要求的 Conix、来源。
    Show {
        bundle: String,
        #[arg(long)]
        global: bool,
    },
    /// 从全局库装进项目。
    Install {
        bundle: String,
        #[arg(long)]
        force: bool,
        #[arg(long, value_enum, value_name = "ask|yes|no")]
        jobs: Option<JobsChoice>,
    },
    /// 用全局库里的更新项目里装的。
    Update {
        bundle: Option<String>,
        #[arg(long, conflicts_with = "bundle")]
        all: bool,
        #[arg(long)]
        force: bool,
        #[arg(long, value_enum, value_name = "ask|yes|no")]
        jobs: Option<JobsChoice>,
    },
    /// 删掉项目的（缺省）或全局库的，都先备份。
    Remove {
        bundle: String,
        #[arg(long)]
        global: bool,
        #[arg(long)]
        force: bool,
    },
    /// 把全局库里的内置包恢复成这个 Conix 带的那版。
    Reset {
        bundle: Option<String>,
        #[arg(long, conflicts_with = "bundle")]
        all: bool,
    },
    /// 把流程包导出成 zip。
    Export {
        bundle: String,
        #[arg(long)]
        global: bool,
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// 把包文件导入全局库（缺省）或项目。
    Import {
        file: PathBuf,
        #[arg(long)]
        project: bool,
        #[arg(long)]
        force: bool,
    },
    /// 把项目里的包回填到全局库。
    Backfill {
        bundle: String,
        #[arg(long)]
        force: bool,
    },
    /// 项目里的与全局库里的差在哪。
    Diff { bundle: String },
    /// 清掉备份、暂存的残留、对不上目录的记录。
    Clean {
        #[arg(long)]
        global: bool,
    },
    /// 看包里的 MANIFEST 并核对：不装。
    Inspect { file: PathBuf },
    /// 体检：全局库（--global）或项目的流程包。
    Doctor {
        #[arg(long)]
        global: bool,
        #[arg(long)]
        fix: bool,
    },
    /// 核对一份归档：文件与 record.json 的清单、证据与确认的摘要、record.json 与运行账本。
    Verify {
        #[arg(required_unless_present = "all")]
        dir: Option<PathBuf>,
        #[arg(long, conflicts_with = "dir")]
        all: bool,
    },
}

/// 包建议的作业加不加（flows.md §7.12）。不写是问；`--yes` 而不写 `--jobs` 是不加：不问就替人多加了会定时跑的东西，
/// 哪怕是停着的，也该是人明说要的。
#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum JobsChoice {
    Ask,
    Yes,
    No,
}

/// 帮助文字按当前语言重写（见 `main::localized_command`）。
pub fn localize(c: clap::Command) -> clap::Command {
    let force = |c: clap::Command| c.mut_arg("force", |a| a.help(i18n::t!("app.cli.flow_force")));
    let bundle =
        |c: clap::Command| c.mut_arg("bundle", |a| a.help(i18n::t!("app.cli.flow_bundle")));
    let global = |c: clap::Command, key: &str| c.mut_arg("global", |a| a.help(i18n::tr(key)));
    let jobs = |c: clap::Command| c.mut_arg("jobs", |a| a.help(i18n::t!("app.cli.flow_jobs")));
    c.about(i18n::t!("app.cli.flow"))
        .mut_arg("project_dir", |a| a.help(i18n::t!("app.cli.flow_project")))
        .mut_arg("yes", |a| a.help(i18n::t!("app.cli.flow_yes")))
        .mut_arg("dry_run", |a| a.help(i18n::t!("app.cli.flow_dry_run")))
        .mut_subcommand("list", |c| {
            c.about(i18n::t!("app.cli.flow_list"))
                .mut_arg("global", |a| a.help(i18n::t!("app.cli.flow_list_global")))
                .mut_arg("all", |a| a.help(i18n::t!("app.cli.flow_list_all")))
        })
        .mut_subcommand("show", |c| {
            global(
                bundle(c.about(i18n::t!("app.cli.flow_show"))),
                "app.cli.flow_in_global",
            )
        })
        .mut_subcommand("install", |c| {
            jobs(force(bundle(c.about(i18n::t!("app.cli.flow_install")))))
        })
        .mut_subcommand("update", |c| {
            jobs(force(bundle(c.about(i18n::t!("app.cli.flow_update")))))
                .mut_arg("all", |a| a.help(i18n::t!("app.cli.flow_update_all")))
        })
        .mut_subcommand("remove", |c| {
            global(
                force(bundle(c.about(i18n::t!("app.cli.flow_remove")))),
                "app.cli.flow_in_global",
            )
            .mut_arg("force", |a| a.help(i18n::t!("app.cli.flow_remove_force")))
        })
        .mut_subcommand("reset", |c| {
            bundle(c.about(i18n::t!("app.cli.flow_reset")))
                .mut_arg("all", |a| a.help(i18n::t!("app.cli.flow_reset_all")))
        })
        .mut_subcommand("export", |c| {
            global(
                bundle(c.about(i18n::t!("app.cli.flow_export"))),
                "app.cli.flow_in_global",
            )
            .mut_arg("output", |a| a.help(i18n::t!("app.cli.flow_output")))
        })
        .mut_subcommand("import", |c| {
            force(c.about(i18n::t!("app.cli.flow_import")))
                .mut_arg("file", |a| a.help(i18n::t!("app.cli.flow_file")))
                .mut_arg("project", |a| {
                    a.help(i18n::t!("app.cli.flow_import_project"))
                })
        })
        .mut_subcommand("backfill", |c| {
            force(bundle(c.about(i18n::t!("app.cli.flow_backfill"))))
        })
        .mut_subcommand("diff", |c| bundle(c.about(i18n::t!("app.cli.flow_diff"))))
        .mut_subcommand("clean", |c| {
            global(
                c.about(i18n::t!("app.cli.flow_clean")),
                "app.cli.flow_in_global",
            )
        })
        .mut_subcommand("inspect", |c| {
            c.about(i18n::t!("app.cli.flow_inspect"))
                .mut_arg("file", |a| a.help(i18n::t!("app.cli.flow_file")))
        })
        .mut_subcommand("doctor", |c| {
            global(
                c.about(i18n::t!("app.cli.flow_doctor")),
                "app.cli.flow_in_global",
            )
            .mut_arg("fix", |a| a.help(i18n::t!("app.cli.flow_doctor_fix")))
        })
        .mut_subcommand("verify", |c| {
            c.about(i18n::t!("app.cli.flow_verify"))
                .mut_arg("dir", |a| a.help(i18n::t!("app.cli.flow_archive_dir")))
                .mut_arg("all", |a| a.help(i18n::t!("app.cli.flow_verify_all")))
        })
}

/// 起来时把内置包解进全局库、升级没改过的（flows.md §7.12）；每件事一行提示，写到 stderr。
pub fn bootstrap(home: &Path) {
    for n in Library::new(home).bootstrap() {
        let line = match n {
            BootNote::Extracted { name, version } => {
                i18n::t!("flowlib.boot_extracted", name = name, version = version)
            }
            BootNote::Upgraded { name, from, to } => {
                i18n::t!("flowlib.boot_upgraded", name = name, from = from, to = to)
            }
            BootNote::Failed { name, error } => {
                i18n::t!("flowlib.boot_failed", name = name, error = error)
            }
        };
        eprintln!("{line}");
    }
}

fn project_of(given: Option<PathBuf>, here: impl FnOnce() -> Result<PathBuf>) -> Result<PathBuf> {
    match given {
        Some(p) => p.canonicalize().with_context(|| format!("{}", p.display())),
        None => here(),
    }
}

fn join(v: &[String]) -> String {
    if v.is_empty() {
        "-".into()
    } else {
        v.join(", ")
    }
}

fn err(e: String) -> anyhow::Error {
    anyhow::Error::msg(e)
}

/// 跑一条子命令；`here` 给出没写 `-C` 时的项目（和直接运行 `conix` 一样），`home` 是全局目录，
/// `artifacts_dir` 是全局配置里的制品目录名（项目自己的 `.conix/conix.yaml` 写了以项目的为准），
/// `commands` 是全局配置的 `flows.command_gates` 没关（项目与包定义的执行的总开关）。
pub async fn run(
    args: FlowArgs,
    home: &Path,
    artifacts_dir: Option<String>,
    commands: bool,
    here: impl FnOnce() -> Result<PathBuf>,
) -> Result<()> {
    let project = project_of(args.project_dir, here)?;
    let artifacts = app::config::ProjectConfig::load(&project)
        .flows
        .artifacts_dir
        .or(artifacts_dir)
        .filter(|a| host::flows::artifacts_dir_ok(a))
        .unwrap_or_else(|| host::flows::ARTIFACTS_DIR.to_string());
    let ctx = Ctx::new(&project, home);
    let confirm = Confirm {
        yes: args.yes,
        dry_run: args.dry_run,
        jobs: None,
    };
    match args.cmd {
        FlowCmd::List { global, all } => list(&ctx, global, all),
        FlowCmd::Show { bundle, global } => show(&ctx, &bundle, global),
        FlowCmd::Install {
            bundle,
            force,
            jobs,
        } => {
            let plan = manage::plan_install(&ctx, &bundle, force).map_err(err)?;
            let confirm = Confirm { jobs, ..confirm };
            change(&ctx, home, vec![plan], vec![], &confirm)
        }
        FlowCmd::Update {
            bundle,
            all,
            force,
            jobs,
        } => {
            let confirm = Confirm { jobs, ..confirm };
            let names = match (bundle, all) {
                (Some(b), _) => vec![b],
                (None, true) => {
                    let lock = manage::read_lock(&project).map_err(err)?;
                    lock.bundles
                        .keys()
                        .filter(|n| ctx.project_dir(n).is_some())
                        .cloned()
                        .collect()
                }
                (None, false) => bail!("{}", i18n::t!("flowlib.cli_update_which")),
            };
            let (mut plans, mut errors) = (vec![], vec![]);
            for n in names {
                match manage::plan_update(&ctx, &n, force) {
                    Ok(p) => plans.push(p),
                    Err(e) => errors.push(e),
                }
            }
            change(&ctx, home, plans, errors, &confirm)
        }
        FlowCmd::Remove {
            bundle,
            global,
            force,
        } => {
            let plan = manage::plan_remove(&ctx, &bundle, global, force).map_err(err)?;
            change(&ctx, home, vec![plan], vec![], &confirm)
        }
        FlowCmd::Reset { bundle, all } => {
            let names: Vec<String> = match (bundle, all) {
                (Some(b), _) => vec![b],
                (None, true) => host::flows::builtin::bundles()
                    .into_iter()
                    .map(String::from)
                    .collect(),
                (None, false) => bail!("{}", i18n::t!("flowlib.cli_reset_which")),
            };
            let (mut plans, mut errors) = (vec![], vec![]);
            for n in names {
                match manage::plan_reset(&ctx, &n) {
                    Ok(p) => plans.push(p),
                    Err(e) => errors.push(e),
                }
            }
            change(&ctx, home, plans, errors, &confirm)
        }
        FlowCmd::Export {
            bundle,
            global,
            output,
        } => {
            let dir = if global {
                ctx.library.dir(&bundle)
            } else {
                ctx.project_dir(&bundle)
            };
            let Some(dir) = dir else {
                bail!(
                    "{}",
                    if global {
                        i18n::t!("flowlib.no_global", name = bundle)
                    } else {
                        i18n::t!("flowlib.not_in_project", name = bundle)
                    }
                );
            };
            let cwd = std::env::current_dir()?;
            let e = package::export(&dir, output.as_deref(), &cwd).map_err(err)?;
            println!(
                "{}",
                i18n::t!(
                    "app.flow_exported",
                    name = e.package.name,
                    version = e.package.version,
                    files = e.package.files,
                    conix = e.package.conix_version,
                    file = e.file
                )
            );
            Ok(())
        }
        FlowCmd::Import {
            file,
            project,
            force,
        } => {
            let plan = manage::plan_import(&ctx, &file, project, force).map_err(err)?;
            change(&ctx, home, vec![plan], vec![], &confirm)
        }
        FlowCmd::Backfill { bundle, force } => {
            let plan = manage::plan_backfill(&ctx, &bundle, force).map_err(err)?;
            change(&ctx, home, vec![plan], vec![], &confirm)
        }
        FlowCmd::Diff { bundle } => diff(&ctx, &bundle),
        FlowCmd::Clean { global } => {
            let plan = manage::plan_clean(&ctx, global).map_err(err)?;
            change(&ctx, home, vec![plan], vec![], &confirm)
        }
        FlowCmd::Inspect { file } => {
            let seen = package::inspect(Path::new(&file));
            match &seen.manifest {
                Some(m) => print!("{m}"),
                None => println!("{}", i18n::t!("app.flow_inspect_no_manifest")),
            }
            match seen.verified {
                Ok(_) => println!("{}", i18n::t!("app.flow_inspect_ok")),
                Err(reason) => {
                    bail!("{}", i18n::t!("app.flow_inspect_failed", reason = reason))
                }
            }
            Ok(())
        }
        FlowCmd::Doctor { global, fix } => {
            doctor(&ctx, home, &artifacts, commands, global, fix).await
        }
        FlowCmd::Verify { dir, all } => {
            let runs = client::layout::ProjectState::new(&project).flow_runs();
            let dirs = match dir {
                Some(d) => vec![d],
                None if all => {
                    let found = doctor::archives(&project, &artifacts);
                    if found.is_empty() {
                        println!(
                            "{}",
                            i18n::t!(
                                "flowlib.verify_none",
                                dir = project.join(&artifacts).display()
                            )
                        );
                    }
                    found
                }
                None => bail!("{}", i18n::t!("flowlib.cli_verify_which")),
            };
            let several = dirs.len() > 1 || all;
            let mut bad = 0;
            for d in &dirs {
                if several {
                    println!("{}", d.strip_prefix(&project).unwrap_or(d).display());
                }
                let r = package::verify_archive(&runs, d);
                print_report(&r);
                if r.ok {
                    println!("{}", i18n::t!("app.flow_verify_ok", n = r.checked));
                } else {
                    bad += 1;
                }
            }
            if bad > 0 {
                if several {
                    bail!(
                        "{}",
                        i18n::t!("flowlib.verify_some_failed", n = bad, total = dirs.len())
                    );
                }
                bail!("{}", i18n::t!("app.flow_verify_failed"));
            }
            Ok(())
        }
    }
}

// ── 看 ──────────────────────────────────────────────────────────────────────

fn status_text(s: &Status) -> String {
    match s {
        Status::Installed => i18n::t!("flowlib.st_installed"),
        Status::Modified => i18n::t!("flowlib.st_modified"),
        Status::Update(v) => i18n::t!("flowlib.st_update", version = v),
        Status::ProjectOnly => i18n::t!("flowlib.st_project_only"),
        Status::NotInstalled => i18n::t!("flowlib.st_not_installed"),
    }
}

fn builtin_text(b: Option<BuiltinState>, newer: Option<&str>) -> Option<String> {
    let s = match b? {
        BuiltinState::Pristine => i18n::t!("flowlib.bi_pristine"),
        BuiltinState::Modified => i18n::t!("flowlib.bi_modified"),
        BuiltinState::Removed => i18n::t!("flowlib.bi_removed"),
        BuiltinState::Missing => i18n::t!("flowlib.bi_missing"),
    };
    Some(match newer {
        Some(v) => format!("{s}{}", i18n::t!("flowlib.bi_newer", version = v)),
        None => s,
    })
}

/// 这个 Conix 带的内置版比库里那份的底子新：回新的版本。
fn newer_shipped(row: &manage::Row) -> Option<String> {
    let b = host::flows::builtin::package(&row.name)?;
    let base = row.entry.as_ref()?.shipped.as_ref()?;
    let newer = host::flows::library::version_key(&b.version)
        > host::flows::library::version_key(&base.version);
    (newer && row.builtin == Some(BuiltinState::Modified)).then_some(b.version)
}

fn global_text(row: &manage::Row) -> String {
    let mut parts = vec![];
    if let Some(g) = &row.global {
        parts.push(g.version.clone());
    }
    if let Some(e) = &row.entry {
        parts.push(e.source.as_str().to_string());
    }
    if let Some(b) = builtin_text(row.builtin, newer_shipped(row).as_deref()) {
        parts.push(b);
    }
    if parts.is_empty() {
        "-".into()
    } else {
        parts.join("  ")
    }
}

fn list(ctx: &Ctx, global: bool, all: bool) -> Result<()> {
    let rows = manage::rows(ctx).map_err(err)?;
    let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
    let pv = |r: &manage::Row| {
        r.project
            .as_ref()
            .map(|p| p.version.clone())
            .unwrap_or_else(|| "-".into())
    };
    let st = |r: &manage::Row| {
        r.statuses()
            .iter()
            .map(status_text)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut any = false;
    for r in &rows {
        if global {
            if r.global.is_none() && r.entry.is_none() {
                continue;
            }
            println!("{:<width$}  {}", r.name, global_text(r));
        } else if all {
            println!(
                "{:<width$}  {}  {:<10}  {}  {}",
                r.name,
                i18n::t!("flowlib.col_project"),
                format!("{} {}", pv(r), st(r)),
                i18n::t!("flowlib.col_global"),
                global_text(r)
            );
        } else {
            if r.project.is_none() && r.lock.is_none() {
                continue;
            }
            let s = if r.project.is_none() {
                i18n::t!("flowlib.st_gone")
            } else {
                st(r)
            };
            println!("{:<width$}  {:<8}  {s}", r.name, pv(r));
        }
        any = true;
    }
    if !any {
        println!(
            "{}",
            if global {
                i18n::t!("flowlib.list_none_global")
            } else {
                i18n::t!("flowlib.list_none_project")
            }
        );
    }
    Ok(())
}

fn show(ctx: &Ctx, name: &str, global: bool) -> Result<()> {
    let row = manage::row(ctx, name).map_err(err)?;
    let side = if global { &row.global } else { &row.project };
    let Some(side) = side else {
        bail!(
            "{}",
            if global {
                i18n::t!("flowlib.no_global", name = name)
            } else {
                i18n::t!("flowlib.not_in_project", name = name)
            }
        );
    };
    let info = package::describe(&side.dir).map_err(err)?;
    println!("{} {}  {}", info.name, info.version, info.title);
    if !info.description.is_empty() {
        println!("{}", info.description);
    }
    println!(
        "{}",
        i18n::t!("flowlib.show_requires", requires = info.requires_conix)
    );
    println!(
        "{}",
        i18n::t!("app.flow_imported_flows", flows = join(&info.flows))
    );
    println!(
        "{}",
        i18n::t!(
            "app.flow_imported_commands",
            commands = join(&info.commands)
        )
    );
    if info.command_gates > 0 {
        println!("{}", i18n::t!("flowlib.note_gates", n = info.command_gates));
    }
    let source = if global {
        global_text(&row)
    } else {
        match &row.lock {
            Some(l) => format!(
                "{}  {}",
                i18n::t!("flowlib.show_from_global", version = l.version, at = l.at),
                row.statuses()
                    .iter()
                    .map(status_text)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            None => i18n::t!("flowlib.st_project_only"),
        }
    };
    println!("{}", i18n::t!("flowlib.show_source", source = source));
    println!("{}", i18n::t!("flowlib.show_dir", dir = side.dir.display()));
    Ok(())
}

fn diff(ctx: &Ctx, name: &str) -> Result<()> {
    let row = manage::row(ctx, name).map_err(err)?;
    let (Some(p), Some(g)) = (&row.project, &row.global) else {
        bail!(
            "{}",
            if row.project.is_none() {
                i18n::t!("flowlib.not_in_project", name = name)
            } else {
                i18n::t!("flowlib.no_global", name = name)
            }
        );
    };
    let pf = package::collect(&p.dir).map_err(err)?;
    let gf = package::collect(&g.dir).map_err(err)?;
    // 从全局库看项目：项目多出来的是"增"。
    let c = manage::diff(Some(&gf), &pf);
    println!(
        "{}",
        i18n::t!(
            "flowlib.diff_head",
            name = name,
            project = p.version,
            global = g.version
        )
    );
    if c.is_empty() {
        println!("  {}", i18n::t!("flowlib.diff_same"));
        return Ok(());
    }
    let all = |mark: &str, v: &[String]| {
        for f in v {
            println!("  {mark} {f}");
        }
    };
    all("+", &c.added);
    all("~", &c.changed);
    all("-", &c.removed);
    Ok(())
}

// ── 体检 ────────────────────────────────────────────────────────────────────

/// 装着的插件的流程目录（按插件名），不经实例：`<家目录>/plugins/` 下带 `plugin.toml` 的包（至多三层）。
/// 启没启用要问引擎，这里不知道，所以只用来提醒"可能遮住"。
fn plugin_flow_dirs(home: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, depth: u32, out: &mut Vec<(String, PathBuf)>) {
        if dir.join("plugin.toml").is_file() {
            if let Some(section) = host::plugins::conix_section(dir) {
                let m = host::plugins::Mounted {
                    name: dir
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    root: dir.to_path_buf(),
                    section,
                };
                if let Some(d) = m.flows_dir() {
                    out.push((m.name, d));
                }
            }
            return;
        }
        if depth == 0 {
            return;
        }
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                walk(&e.path(), depth - 1, out);
            }
        }
    }
    let mut out = vec![];
    walk(&home.join("plugins"), 3, &mut out);
    out
}

/// 项目受不受信，照 Agix 记信任的地方读（全局 `settings.json` / `settings.local.json` 的 `app.trusted_projects`，
/// 路径或以 `/*`、`/**` 结尾的前缀）：读不出来的当没信任，看不出来（两个文件都不是 JSON）是 `None`。
fn trusted(home: &Path, project: &Path) -> Option<bool> {
    let mut read_any = false;
    let mut missing = 0;
    for f in ["settings.json", "settings.local.json"] {
        let Ok(text) = std::fs::read_to_string(home.join(f)) else {
            missing += 1;
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        read_any = true;
        if v["app"]["trust_all_projects"].as_bool() == Some(true) {
            return Some(true);
        }
        let entries = v["app"]["trusted_projects"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for e in entries.iter().filter_map(|e| e.as_str()) {
            let hit = match e.strip_suffix("/**").or_else(|| e.strip_suffix("/*")) {
                Some(prefix) => project.starts_with(prefix),
                None => Path::new(e) == project,
            };
            if hit || e == "*" || e == "**" {
                return Some(true);
            }
        }
    }
    (read_any || missing == 2).then_some(false)
}

fn level_mark(l: Level) -> &'static str {
    match l {
        Level::Info => "·",
        Level::Warn => "◐",
        Level::Error => "✗",
    }
}

async fn check(
    ctx: &Ctx,
    home: &Path,
    artifacts: &str,
    commands: bool,
    global: bool,
) -> doctor::Report {
    if global {
        doctor::global(ctx)
    } else {
        let opts = doctor::ProjectOpts {
            artifacts_dir: artifacts.to_string(),
            plugin_flow_dirs: plugin_flow_dirs(home),
            trusted: trusted(home, &ctx.project),
            commands,
        };
        doctor::project(ctx, &opts).await
    }
}

async fn doctor(
    ctx: &Ctx,
    home: &Path,
    artifacts: &str,
    commands: bool,
    global: bool,
    fix: bool,
) -> Result<()> {
    let report = check(ctx, home, artifacts, commands, global).await;
    println!(
        "{}",
        if global {
            i18n::t!(
                "flowlib.dr_title_global",
                dir = ctx.library.root().display()
            )
        } else {
            i18n::t!("flowlib.dr_title_project", dir = ctx.project.display())
        }
    );
    if report.findings.is_empty() {
        println!("  {}", i18n::t!("flowlib.dr_nothing"));
    }
    print_findings(report.findings.iter());
    if fix {
        if report.fixes.is_empty() {
            println!("{}", i18n::t!("flowlib.dr_no_fixes"));
        }
        let mut installed = false;
        for (f, r) in report.fixes.iter().zip(doctor::fix(ctx, &report.fixes)) {
            match r {
                Ok(line) => {
                    installed |= matches!(f, doctor::Fix::Install(_) | doctor::Fix::Lock(_));
                    println!("{}", i18n::t!("flowlib.dr_fixed", what = line));
                }
                Err(e) => eprintln!("{}", i18n::t!("flowlib.dr_fix_failed", why = e)),
            }
        }
        if installed
            && let Some(name) = notify_instance(home, &ctx.project, port::Command::ReloadFlows)
        {
            println!("{}", i18n::t!("flowlib.notified", name = name));
        }
    } else if !report.fixes.is_empty() {
        println!(
            "{}",
            i18n::t!(
                "flowlib.dr_can_fix",
                n = report.fixes.len(),
                cmd = if global {
                    "conix flow doctor --global --fix"
                } else {
                    "conix flow doctor --fix"
                }
            )
        );
    }
    // 修过了就按修完的样子再查一遍定退出码：修好的错误不该让脚本以为还坏着。修出来的新错误（装上的包自己的自检报的）
    // 要列出来：只说"还有 1 个错误"，人不知道是哪个。
    let errors = if fix && !report.fixes.is_empty() {
        let after = check(ctx, home, artifacts, commands, global).await;
        let left: Vec<_> = after
            .findings
            .iter()
            .filter(|f| f.level == doctor::Level::Error)
            .collect();
        if !left.is_empty() {
            println!("{}", i18n::t!("flowlib.dr_after_fix"));
            print_findings(left.into_iter());
        }
        after.errors()
    } else {
        report.errors()
    };
    if errors > 0 {
        bail!("{}", i18n::t!("flowlib.dr_errors", n = errors));
    }
    Ok(())
}

fn print_findings<'a>(findings: impl Iterator<Item = &'a doctor::Finding>) {
    for f in findings {
        let who = f
            .bundle
            .as_deref()
            .map(|b| format!("{b}: "))
            .unwrap_or_default();
        println!("  {} {who}{}", level_mark(f.level), f.text);
        if let Some(h) = &f.hint {
            println!("      → {h}");
        }
    }
}

// ── 改 ──────────────────────────────────────────────────────────────────────

struct Confirm {
    yes: bool,
    dry_run: bool,
    /// 包建议的作业（install、update）：`None` 是没写 `--jobs`。
    jobs: Option<JobsChoice>,
}

fn where_text(w: Where) -> String {
    match w {
        Where::Project => i18n::t!("flowlib.where_project"),
        Where::Global => i18n::t!("flowlib.where_global"),
    }
}

const SHOWN: usize = 12;

fn print_files(mark: &str, files: &[String]) {
    for f in files.iter().take(SHOWN) {
        println!("    {mark} {f}");
    }
    if files.len() > SHOWN {
        println!(
            "    {}",
            i18n::t!("flowlib.and_more", n = files.len() - SHOWN).trim()
        );
    }
}

fn print_plan(p: &Plan) {
    if p.action == Action::Clean {
        println!(
            "{}",
            i18n::t!("flowlib.plan_clean", place = where_text(p.target))
        );
        for path in &p.paths {
            println!("    - {}", path.display());
        }
        for o in &p.orphans {
            println!("    {}", i18n::t!("flowlib.plan_orphan", name = o));
        }
        if let Some(n) = &p.noop {
            println!("  {n}");
        }
        return;
    }
    let versions = match (&p.from, &p.to) {
        (Some(a), Some(b)) => format!("{a} → {b}"),
        (None, Some(b)) => b.clone(),
        (Some(a), None) => a.clone(),
        (None, None) => String::new(),
    };
    println!(
        "{}",
        i18n::t!(
            "flowlib.plan_head",
            action = p.action.as_str(),
            name = p.bundle,
            versions = versions,
            place = where_text(p.target)
        )
    );
    if let Some(n) = &p.noop {
        println!("  {n}");
        return;
    }
    let c = &p.changes;
    if !c.is_empty() {
        println!(
            "  {}",
            i18n::t!(
                "flowlib.plan_files",
                added = c.added.len(),
                changed = c.changed.len(),
                removed = c.removed.len()
            )
        );
        print_files("+", &c.added);
        print_files("~", &c.changed);
        print_files("-", &c.removed);
    }
    if let Some(b) = &p.backup {
        println!("  {}", i18n::t!("flowlib.plan_backup", dir = b.display()));
    }
    for n in &p.notes {
        println!("  {n}");
    }
    print_jobs(&p.jobs);
}

/// 作业做什么，一句：简单作业是那句话的开头，流程作业是起哪条流程。
fn job_what(j: &port::Job) -> String {
    match &j.kind {
        port::JobKind::Simple { prompt, .. } => {
            let first = prompt.lines().next().unwrap_or_default();
            let mut s: String = first.chars().take(40).collect();
            if s.chars().count() < prompt.chars().count() {
                s.push('…');
            }
            i18n::t!("flowlib.job_simple", prompt = s)
        }
        port::JobKind::Flow { flow, .. } => i18n::t!("flowlib.job_flow", flow = flow),
    }
}

fn print_jobs(o: &host::jobs::suggest::Offer) {
    if o.is_empty() {
        return;
    }
    println!("  {}", i18n::t!("flowlib.plan_jobs", n = o.new.len()));
    for j in &o.new {
        println!("    + {}  {}  {}  {}", j.id, j.name, j.cron, job_what(j));
    }
    for j in &o.existing {
        println!(
            "    = {}",
            i18n::t!("flowlib.plan_job_exists", id = j.id, name = j.name)
        );
    }
}

/// 人在终端里答 y 才算。
fn ask(question: &str) -> Result<bool> {
    print!("{question} ");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(matches!(answer.trim(), "y" | "Y" | "yes" | "YES" | "Yes"))
}

/// 列出计划、确认、照做；做完了项目的告诉在跑的实例。`errors` 是计划不出来的那几个，最后报错退出。
fn change(
    ctx: &Ctx,
    home: &Path,
    plans: Vec<Plan>,
    errors: Vec<String>,
    c: &Confirm,
) -> Result<()> {
    for e in &errors {
        eprintln!("{e}");
    }
    for p in &plans {
        print_plan(p);
    }
    let todo: Vec<&Plan> = plans.iter().filter(|p| !p.is_noop()).collect();
    let finish = |errors: &[String]| -> Result<()> {
        if errors.is_empty() {
            Ok(())
        } else {
            bail!("{}", i18n::t!("flowlib.cli_some_failed", n = errors.len()))
        }
    };
    if todo.is_empty() || c.dry_run {
        if c.dry_run && !todo.is_empty() {
            println!("{}", i18n::t!("flowlib.cli_dry_run"));
        }
        return finish(&errors);
    }
    if !c.yes {
        if !std::io::stdin().is_terminal() {
            bail!("{}", i18n::t!("flowlib.cli_need_yes"));
        }
        if !ask(&i18n::t!("flowlib.cli_ask"))? {
            println!("{}", i18n::t!("flowlib.cli_cancelled"));
            return Ok(());
        }
    }
    // 作业另问一次：包装不装与要不要它定时做事是两件事，前一句的 y 不替后一句答。
    let offered: usize = todo.iter().map(|p| p.jobs.new.len()).sum();
    let add_jobs = offered > 0
        && match (c.jobs, c.yes) {
            (Some(JobsChoice::Yes), _) => true,
            (Some(JobsChoice::No), _) => false,
            (_, true) => {
                println!("{}", i18n::t!("flowlib.cli_jobs_not_added"));
                false
            }
            (_, false) => ask(&i18n::t!("flowlib.cli_jobs_ask", n = offered))?,
        };
    let mut errors = errors;
    let mut touched_project = false;
    let mut added_jobs = false;
    for p in todo {
        match manage::apply(ctx, p) {
            Ok(done) => {
                touched_project |= p.target == Where::Project;
                println!(
                    "{}",
                    i18n::t!("flowlib.done", action = p.action.as_str(), name = p.bundle)
                );
                if let Some(b) = done.backup {
                    println!("  {}", i18n::t!("flowlib.done_backup", dir = b.display()));
                }
                if add_jobs && !p.jobs.new.is_empty() {
                    match host::jobs::suggest::add(&ctx.project, &p.jobs.new, now_ms()) {
                        Ok(ids) => {
                            added_jobs |= !ids.is_empty();
                            for id in ids {
                                println!("  {}", i18n::t!("flowlib.job_added", id = id));
                            }
                            println!("  {}", i18n::t!("flowlib.jobs_disabled_hint"));
                        }
                        Err(e) => {
                            let e = i18n::t!("flowlib.jobs_failed", why = e);
                            eprintln!("{e}");
                            errors.push(e);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("{e}");
                errors.push(e);
            }
        }
    }
    if touched_project
        && let Some(name) = notify_instance(home, &ctx.project, port::Command::ReloadFlows)
    {
        println!("{}", i18n::t!("flowlib.notified", name = name));
    }
    if added_jobs && let Some(name) = notify_instance(home, &ctx.project, port::Command::ReloadJobs)
    {
        println!("{}", i18n::t!("flowlib.notified_jobs", name = name));
    }
    finish(&errors)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 这个项目若有实例在跑，经它的 socket 让它重读流程或作业（`cmd`；尽力而为：不在、连不上都不算错）。回实例名。
fn notify_instance(home: &Path, project: &Path, cmd: port::Command) -> Option<String> {
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    let layout = client::Layout::new(home);
    let want = project
        .canonicalize()
        .unwrap_or_else(|_| project.to_path_buf());
    for e in std::fs::read_dir(layout.instances()).ok()?.flatten() {
        let Some(name) = e.file_name().into_string().ok() else {
            continue;
        };
        let Some(f) = client::layout::read_json::<client::InstanceFile>(&layout.discovery(&name))
        else {
            continue;
        };
        if f.project.as_deref().map(Path::new) != Some(want.as_path()) || !client::pid_alive(f.pid)
        {
            continue;
        }
        let Ok(mut s) = UnixStream::connect(layout.socket(&name)) else {
            continue;
        };
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        let frame = port::link::ClientFrame {
            id: 1,
            cmd: cmd.clone(),
        };
        let Ok(mut line) = serde_json::to_string(&frame) else {
            continue;
        };
        line.push('\n');
        if s.write_all(line.as_bytes()).is_err() {
            continue;
        }
        let mut reply = String::new();
        if std::io::BufReader::new(&s).read_line(&mut reply).is_ok() && !reply.is_empty() {
            return Some(name);
        }
    }
    None
}

fn print_report(r: &package::archive::ArchiveReport) {
    for p in &r.missing {
        println!("{}", i18n::t!("app.flow_verify_missing", path = p));
    }
    for p in &r.extra {
        println!("{}", i18n::t!("app.flow_verify_extra", path = p));
    }
    for p in &r.changed {
        println!("{}", i18n::t!("app.flow_verify_changed", path = p));
    }
    for e in &r.evidence {
        println!(
            "{}",
            i18n::t!(
                "app.flow_verify_evidence",
                path = e.path,
                by = e.by,
                problem = e.problem
            )
        );
    }
    for p in &r.problems {
        println!("{}", i18n::t!("app.flow_verify_problem", problem = p));
    }
    if r.unlisted {
        println!("{}", i18n::t!("app.flow_verify_unlisted"));
    }
    match r.record {
        package::archive::RecordState::Matches => {
            println!("{}", i18n::t!("app.flow_verify_record_matches"))
        }
        package::archive::RecordState::Differs => {
            println!("{}", i18n::t!("app.flow_verify_record_differs"))
        }
        package::archive::RecordState::NoLedger => {
            println!("{}", i18n::t!("app.flow_verify_no_ledger"))
        }
    }
}
