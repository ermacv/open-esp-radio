use std::path::PathBuf;

use clap::{Parser, Subcommand};
use oer_process as process;
use oer_process::Checkout;
use oer_xtask::{Result, checks};

#[derive(Parser)]
#[command(about = "Repository build and test commands")]
struct Cli {
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    /// Stream the output of every command a check runs instead of logging
    /// it under target/xtask/logs and showing only a failure's diagnostics.
    #[arg(long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Build API documentation from each package's `[package.metadata.docs.rs]`
    /// with `RUSTDOCFLAGS=-D warnings`, then run host doctests.
    Doc,
    /// Push this branch with `git push` and open its pull request as a draft
    /// when it has none; a person marks it ready and merges it. Refused on
    /// main.
    Push,
    /// Update every workspace's Cargo.lock to its manifests after a
    /// dependency or pin change.
    Lock {
        /// Only verify, offline, that every lock matches its manifests.
        #[arg(long)]
        check: bool,
    },
    /// Add or remove a Git worktree whose target/ starts as a copy-on-write
    /// clone of this checkout's build outputs.
    Worktree {
        #[command(subcommand)]
        worktree: Worktree,
    },
    Check {
        #[command(subcommand)]
        check: Check,
    },
}

#[derive(Subcommand)]
enum Worktree {
    /// Create PATH on a new BRANCH from FROM and seed its target/.
    Add {
        path: std::path::PathBuf,
        #[arg(long)]
        branch: String,
        #[arg(long, default_value = "origin/main")]
        from: String,
    },
    /// Remove a worktree and its private target/.
    Remove { path: std::path::PathBuf },
    /// Turn this checkout's target/ into a btrfs subvolume once, so `add`
    /// snapshots it instantly; run it while no build uses target/.
    Prepare,
}

#[derive(Subcommand)]
enum Check {
    /// Test every root-workspace package with the feature sets its
    /// `open-radio.test-feature-sets` declares.
    FeatureSets,
    /// Compile contract misuses and their positive twins, checking exact rustc diagnostics.
    Ui,
}

fn run() -> Result<std::process::ExitCode> {
    let cli = Cli::parse();
    let ctx = match cli.root {
        Some(root) => Checkout::new(root)?,
        None => Checkout::discover("xtask")?,
    };
    let _signals = process::install_signal_handlers()?;
    let log = match log_name(&cli.command) {
        Some(name) if !cli.verbose => Some(oer_xtask::report::Log::start(&ctx.root, &name)?),
        _ => None,
    };
    let result = dispatch(&ctx, cli.command);
    if let Some(log) = &log {
        match &result {
            Ok(_) => println!("log: {}", log.path.display()),
            Err(_) => {
                let digest = log.digest();
                if !digest.is_empty() {
                    eprintln!("{digest}");
                }
                eprintln!("full log: {}", log.path.display());
            }
        }
    }
    result
}

/// The log of a command whose child output is logged by default: the
/// checks, `doc` and `lock`.
fn log_name(command: &Task) -> Option<String> {
    let name = match command {
        Task::Check {
            check: Check::FeatureSets,
        } => String::from("check-feature-sets"),
        Task::Check { check: Check::Ui } => String::from("check-ui"),
        Task::Doc => String::from("doc"),
        Task::Lock { .. } => String::from("lock"),
        _ => return None,
    };
    Some(name)
}

fn dispatch(ctx: &Checkout, command: Task) -> Result<std::process::ExitCode> {
    let ctx = ctx.clone();
    match command {
        Task::Doc => oer_xtask::doc::run(&ctx),
        Task::Push => oer_xtask::push::run(&ctx),
        Task::Lock { check: false } => checks::metadata::update_locks(&ctx),
        Task::Lock { check: true } => checks::metadata::check_locks(
            &ctx,
            &checks::metadata::workspaces(&ctx)?
                .into_iter()
                .collect::<Vec<_>>(),
        ),
        Task::Worktree { worktree } => match worktree {
            Worktree::Add { path, branch, from } => {
                oer_xtask::worktree::add(&ctx, &path, &branch, &from)
            }
            Worktree::Remove { path } => oer_xtask::worktree::remove(&ctx, &path),
            Worktree::Prepare => oer_xtask::worktree::prepare(&ctx),
        },
        Task::Check { check } => match check {
            Check::FeatureSets => checks::feature_sets::run(&ctx),
            Check::Ui => checks::ui::run(&ctx),
        },
    }?;
    Ok(std::process::ExitCode::SUCCESS)
}

fn main() -> std::process::ExitCode {
    if oer_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree = oer_command_tree::command_tree(&Cli::command(), &[String::from("xtask")]);
        println!("{}", oer_command_tree::json(&tree));
        return std::process::ExitCode::SUCCESS;
    }
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("xtask: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_and_tests_log_by_default_and_stream_with_verbose() {
        let cli = Cli::try_parse_from(["xtask", "check", "ui"]).unwrap();
        assert!(!cli.verbose);
        assert_eq!(log_name(&cli.command).as_deref(), Some("check-ui"));
        let cli = Cli::try_parse_from(["xtask", "--verbose", "push"]).unwrap();
        assert!(cli.verbose);
        assert_eq!(log_name(&cli.command), None);
        let cli = Cli::try_parse_from(["xtask", "lock", "--check"]).unwrap();
        assert!(matches!(cli.command, Task::Lock { check: true }));
        // The gate, CI and review commands are gone.
        for removed in [
            &["xtask", "check", "changed"][..],
            &["xtask", "check", "tier", "--list"],
            &["xtask", "review"],
            &["xtask", "ci-status"],
            &["xtask", "hooks"],
        ] {
            assert!(Cli::try_parse_from(removed).is_err(), "{removed:?}");
        }
    }
}
