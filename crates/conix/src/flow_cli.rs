//! `conix flow export | import | inspect`：流程包与包文件互转（docs/design/flows.md §7.12）。不需要实例：
//! 项目的与内置的流程包在这里直接读；插件的流程包要在实例里导（`/flow:export`）。

use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, bail};
use host::flows::package::{self, ImportOptions};

#[derive(clap::Subcommand)]
pub enum FlowCmd {
    /// 把流程包导出成 zip。
    Export {
        /// 流程包的名字（项目里的或内置的）。
        bundle: String,
        /// 写到哪：文件，或已有的目录；省略则写在当前目录。
        #[arg(short, long)]
        output: Option<PathBuf>,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// 把包文件装进项目的 `.conix/flows/`。
    Import {
        file: PathBuf,
        /// 已有同名的流程包就覆盖（旧的先备份）。
        #[arg(long)]
        force: bool,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// 看包里的 MANIFEST 并核对：不装。
    Inspect { file: PathBuf },
}

/// 帮助文字按当前语言重写（见 `main::localized_command`）。
pub fn localize(c: clap::Command) -> clap::Command {
    c.about(i18n::t!("app.cli.flow"))
        .mut_subcommand("export", |c| {
            c.about(i18n::t!("app.cli.flow_export"))
                .mut_arg("bundle", |a| a.help(i18n::t!("app.cli.flow_bundle")))
                .mut_arg("output", |a| a.help(i18n::t!("app.cli.flow_output")))
                .mut_arg("project", |a| a.help(i18n::t!("app.cli.flow_project")))
        })
        .mut_subcommand("import", |c| {
            c.about(i18n::t!("app.cli.flow_import"))
                .mut_arg("file", |a| a.help(i18n::t!("app.cli.flow_file")))
                .mut_arg("force", |a| a.help(i18n::t!("app.cli.flow_force")))
                .mut_arg("project", |a| a.help(i18n::t!("app.cli.flow_project")))
        })
        .mut_subcommand("inspect", |c| {
            c.about(i18n::t!("app.cli.flow_inspect"))
                .mut_arg("file", |a| a.help(i18n::t!("app.cli.flow_file")))
        })
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

/// 跑一条子命令；`here` 给出没写 `--project` 时的项目（和直接运行 `conix` 一样）。
pub fn run(cmd: FlowCmd, here: impl FnOnce() -> Result<PathBuf>) -> Result<()> {
    match cmd {
        FlowCmd::Export {
            bundle,
            output,
            project,
        } => {
            let project = project_of(project, here)?;
            let cwd = std::env::current_dir()?;
            let e = package::export_offline(&project, &bundle, output.as_deref(), &cwd)
                .map_err(anyhow::Error::msg)?;
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
        }
        FlowCmd::Import {
            file,
            force,
            project,
        } => {
            let project = project_of(project, here)?;
            let r = package::import(
                &file,
                &project,
                &ImportOptions {
                    force,
                    catalog: package::default_catalog(),
                    plugin_dirs: &[],
                },
            )
            .map_err(anyhow::Error::msg)?;
            let p = &r.package;
            println!(
                "{}",
                i18n::t!(
                    "app.flow_imported",
                    name = p.name,
                    version = p.version,
                    conix = p.conix_version
                )
            );
            println!(
                "{}",
                i18n::t!("app.flow_imported_flows", flows = join(&p.flows))
            );
            println!(
                "{}",
                i18n::t!("app.flow_imported_commands", commands = join(&p.commands))
            );
            println!(
                "{}",
                i18n::t!("app.flow_imported_installed", dir = r.installed)
            );
            if let Some(b) = r.backup {
                eprintln!("{}", i18n::t!("app.flow_warn_backup", dir = b));
            }
            if let Some(by) = r.shadows {
                eprintln!(
                    "{}",
                    i18n::t!("app.flow_warn_shadows", name = p.name, by = by)
                );
            }
            if p.command_gates > 0 {
                eprintln!("{}", i18n::t!("app.flow_warn_gates", n = p.command_gates));
            }
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
        }
    }
    Ok(())
}
