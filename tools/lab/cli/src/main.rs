//! `cargo stand`: the shared stand's own commands — the queue and leases,
//! boards and their service, owners, preemption, the stand file against the
//! host, the host fixtures and their software installation. HIL test
//! commands are `cargo hil`, which builds on the stand's libraries; this
//! binary links none of HIL.
#![forbid(unsafe_code)]

mod board;
mod command;
mod discover;
mod fixture_install;
mod init;
mod install;
mod local_fixtures;
mod service;
mod stand_file;

use std::process::ExitCode;

use oer_os_process::Checkout;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The owner selected by this invocation.
#[derive(Clone, Debug, Default, PartialEq, clap::Args)]
pub(crate) struct Options {
    #[arg(long, global = true)]
    pub(crate) owner: Option<String>,
}

impl Options {
    pub(crate) fn owner(&self, ctx: &Checkout) -> Result<String> {
        Ok(oer_lab_arbiter::owner::resolve(self.owner.as_deref(), &ctx.root)?.to_string())
    }
}

#[derive(clap::Parser)]
#[command(name = "cargo stand", no_binary_name = true, args_override_self = true)]
struct Cli {
    #[command(flatten)]
    options: Options,
    #[command(flatten)]
    checkout: oer_os_process::CheckoutArgs,
    #[command(subcommand)]
    command: Option<StandCommand>,
}

#[derive(clap::Subcommand)]
enum StandCommand {
    Queue(command::QueueCli),
    Wait(command::WaitCli),
    Lease(command::LeaseCli),
    Board(command::BoardCli),
    Devices(command::DevicesCli),
    Owner(command::OwnerCli),
    Preempt(command::PreemptCli),
    #[command(flatten)]
    Discovery(discover::StandCli),
    Init(init::InitCli),
    Install,
    Fixtures,
    #[command(subcommand)]
    Fixture(FixtureCli),
    #[command(subcommand, name = "stand-file")]
    StandFile(StandFileCli),
    #[command(name = "__command-tree", hide = true)]
    CommandTree,
}

/// `cargo stand stand-file convert`.
#[derive(clap::Parser)]
#[command(name = "cargo stand stand-file", no_binary_name = true)]
enum StandFileCli {
    /// Rewrite a schema 1 stand file to schema 2, keeping its comments:
    /// the lab router becomes `[router]`, the run's channel an allowed one.
    Convert {
        /// Write the schema 2 file here (mode 0600) and leave the stand
        /// file as it is.
        #[arg(long, conflicts_with = "in_place")]
        output: Option<std::path::PathBuf>,
        /// Replace the stand file, keeping the schema 1 file beside it.
        #[arg(long)]
        in_place: bool,
    },
}

/// `cargo stand fixture install|build-hostapd|build-udhcpc`.
#[derive(clap::Parser)]
#[command(name = "cargo stand fixture", no_binary_name = true)]
enum FixtureCli {
    /// Prepare and install one versioned Linux fixture software bundle.
    Install {
        /// Finite Linux fixture provider to provision.
        #[arg(long, value_enum)]
        provider: oer_lab_fixture::Provider,
        /// Print the offline plan without executing any installer step.
        #[arg(long)]
        dry_run: bool,
        /// Bluetooth adapter admitted by the installed policy; defaults to hci0.
        #[arg(long, value_name = "hciN")]
        adapter: Vec<String>,
    },
    /// Build the pinned hostapd with explicit HIL coexistence policy support.
    BuildHostapd,
    /// Build the pinned DHCP client without root or radio access.
    BuildUdhcpc,
}

/// `cargo stand __command-tree`: every `cargo stand` command path with its
/// subcommands and long flags, as JSON, for the documentation check.
fn command_tree() -> Vec<oer_os_command_tree::CommandNode> {
    use clap::CommandFactory as _;
    oer_os_command_tree::command_tree(&Cli::command(), &[String::from("stand")])
}

fn run() -> Result<ExitCode> {
    use clap::{CommandFactory as _, Parser as _};
    let cli = Cli::try_parse_from(std::env::args_os().skip(1))?;
    if matches!(cli.command, Some(StandCommand::CommandTree)) {
        println!("{}", oer_os_command_tree::json(&command_tree()));
        return Ok(ExitCode::SUCCESS);
    }
    let ctx = cli.checkout.checkout("cargo stand")?;
    let _signals = oer_os_process::install_signal_handlers()?;
    let options = cli.options;
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::Queue(cli)) => command::queue(cli),
        Some(StandCommand::Wait(cli)) => command::wait_for_service(cli),
        Some(StandCommand::Lease(cli)) => command::lease(&ctx, options, cli),
        Some(StandCommand::Board(cli)) => command::board(&ctx, &options, cli),
        Some(StandCommand::Devices(cli)) => command::devices(&ctx, &options, cli),
        Some(StandCommand::Owner(cli)) => command::owner(&ctx, cli),
        Some(StandCommand::Preempt(cli)) => command::preempt(&options.owner(&ctx)?, cli),
        Some(StandCommand::Discovery(cli)) => discover::stand(&ctx, || options.owner(&ctx), cli),
        Some(StandCommand::Init(cli)) => init::init(&ctx.root, cli),
        Some(StandCommand::Install) => {
            install::run(&ctx)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::Fixtures) => {
            // The equipment reached over SSH is the service's: its probe
            // holds the router shared. The host's own radios are fixtures.
            let mut lease = service::lease(
                options.owner(&ctx)?,
                String::from("stand fixtures"),
                vec![oer_lab_arbiter::claims::Claim::shared(
                    oer_lab_arbiter::readiness::ROUTER,
                )],
            )?;
            let remote: Vec<oer_lab_model::equipment::Fixture> = serde_json::from_value(
                service::run(&mut lease, oer_lab_client::protocol::Operation::ProbeRouter)?,
            )?;
            lease.release()?;
            let local = local_fixtures::probe_local(&oer_lab_model::paths::stand_file()?);
            println!("Equipment over SSH (the LAB service):");
            print!("{}", oer_lab_model::equipment::describe(&remote));
            println!("Host fixtures:");
            print!("{}", oer_lab_model::equipment::describe(&local));
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::Fixture(FixtureCli::Install {
            provider,
            dry_run,
            adapter,
        })) => {
            fixture_install::run(&ctx.root, provider, dry_run, &adapter)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::StandFile(StandFileCli::Convert { output, in_place })) => {
            let path = oer_lab_model::paths::stand_file()?;
            let output = match (output, in_place) {
                (Some(output), _) => output,
                (None, true) => path.clone(),
                (None, false) => return Err("name --output PATH or --in-place".into()),
            };
            stand_file::convert(&path, &output)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::Fixture(FixtureCli::BuildHostapd)) => {
            oer_lab_fixture::hostapd::build(&ctx.root)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::Fixture(FixtureCli::BuildUdhcpc)) => {
            oer_lab_fixture::udhcpc::build(&ctx.root)?;
            Ok(ExitCode::SUCCESS)
        }
        Some(StandCommand::CommandTree) => {
            unreachable!("command tree returned before checkout resolution")
        }
    }
}

fn usage_status(error: &clap::Error, args: &[std::ffi::OsString]) -> u8 {
    use clap::CommandFactory as _;
    let partial = Cli::command()
        .ignore_errors(true)
        .try_get_matches_from(args)
        .ok();
    match partial.as_ref().and_then(clap::ArgMatches::subcommand_name) {
        Some("queue") if !error.use_stderr() => 0,
        Some("queue" | "wait" | "owner" | "preempt") => 1,
        Some(_) => error.exit_code() as u8,
        None if !error.use_stderr() => 0,
        None => 1,
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            // A command line clap refused, or help it was asked for, exits
            // as clap does: usage errors with 2, help with 0.
            if let Some(usage) = error.downcast_ref::<clap::Error>() {
                let _ = usage.print();
                return ExitCode::from(usage_status(
                    usage,
                    &std::env::args_os().skip(1).collect::<Vec<_>>(),
                ));
            }
            eprintln!("stand: {error}");
            if oer_os_process::is_cancelled(&*error) {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}

#[cfg(test)]
mod tests;
