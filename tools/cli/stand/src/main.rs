//! `cargo stand`: the shared stand's own commands — the queue and leases,
//! boards and their service, owners, preemption, the stand file against the
//! host, the host fixtures and their software installation. HIL test
//! commands are `cargo hil`, which builds on the stand's libraries; this
//! binary links none of HIL.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod board;
mod command;
mod discover;
mod fixture_install;
mod init;
mod install;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

use oer_process::Checkout;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub(crate) const HELP: &str = "\
Stand commands (shared by every checkout of this user):
  cargo stand queue [--json]          holders, balances, queue with expected starts, boards, recent leases
  cargo stand wait [BOARD...]         block until the boards (all when none) and the stand are in service
  cargo stand lease [OPTIONS] -- CMD  run CMD under one lease; nested cargo stand and cargo hil join it
      --board NAME|MAC                boards CMD uses (repeatable)
      --stand                         claim the whole stand instead; blocks every other owner
      --air shared|exclusive|none     radio environment; exclusive for RF measurements, none without radio
      --flashed IMAGE (--application FILE | --sha256 HASH) --device NAME|MAC
      [--commit REV]                  journal CMD's flash when it succeeds
  cargo stand board reset BOARD [--via rts|jtag|download|power]   reset under a lease; prints the ROM reset line
  cargo stand board check BOARD       attached, firmware, maintenance, reset paths, whether it answers; no reset
  cargo stand board console BOARD [--for 10s] [--until TEXT]     the console without a reset, under a lease
  cargo stand board soak BOARD --cycles N|--for 8h [--via rts,jtag,power]   reset again and again; journal the result
  cargo stand devices [--json]        the stand file's boards: name, chip, port, health, last firmware
  cargo stand [--owner NAME] devices maintenance BOARD|--stand --reason TEXT   only NAME may claim BOARD (or the stand) until release
  cargo stand devices release BOARD [--confirm reset|power-cycle|rom-answers]
  cargo stand owner [set NAME | merge OLD NEW | forget NAME]   this checkout's owner: any name
  cargo stand preempt ID --reason TEXT   stop another owner's lease: SIGTERM with cleanup, SIGKILL after 5m
  cargo stand init [--force]          write the stand file (0600) from the attached boards: chip and MAC each
  cargo stand discover [--blink HUB:PORT | --verify-power BOARD]   attached boards against the stand file
  cargo stand doctor                  the stand file, uhubctl without sudo, NetworkManager leaving wlan0
  cargo stand fixtures                host Wi-Fi radios, Bluetooth adapter and OpenWrt hosts
  cargo stand fixture install --provider linux-net [--dry-run]
  cargo stand fixture build-hostapd   build the pinned hostapd of the linux-net provider
  cargo stand install                 install the oer-stand of origin/main as `cargo stand`, built once

Options, before the command or after `lease`:
  --owner NAME     default: an enclosing lease's owner, else the checkout's registered owner

Every lease holds the device lock of each of its boards for its whole length:
`cargo fw` and other tools see such a board busy, and a board another
process holds is busy for the queue until it lets go.";

/// The options before a command.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Options {
    pub(crate) owner: Option<String>,
}

impl Options {
    /// Split a leading `--owner NAME` (or `--owner=NAME`) from the rest.
    pub(crate) fn split(args: &[OsString]) -> Result<(Self, Vec<OsString>)> {
        let mut options = Self::default();
        let mut rest = args.iter();
        let mut remaining = Vec::new();
        while let Some(argument) = rest.next() {
            let text = argument.to_str().unwrap_or_default();
            if let Some(value) = text.strip_prefix("--owner=") {
                options.owner = Some(value.to_owned());
            } else if text == "--owner" {
                options.owner = Some(
                    rest.next()
                        .and_then(|value| value.to_str())
                        .ok_or("--owner requires a value")?
                        .to_owned(),
                );
            } else {
                remaining.push(argument.clone());
                remaining.extend(rest.cloned());
                break;
            }
        }
        Ok((options, remaining))
    }

    /// The owner: `--owner`, else an enclosing lease's, else the
    /// checkout's registered one.
    pub(crate) fn owner(&self, ctx: &Checkout) -> Result<String> {
        Ok(oer_stand_owners::resolve(self.owner.as_deref(), &ctx.root)?.to_string())
    }
}

/// A leading `--root PATH` (or `--root=PATH`) names the checkout to act on,
/// as the installed `oer-stand` passes it; every other argument is the
/// command.
fn split_root(mut args: Vec<OsString>) -> Result<(Option<PathBuf>, Vec<OsString>)> {
    let Some(first) = args.first().and_then(|first| first.to_str()) else {
        return Ok((None, args));
    };
    if first == "--root" {
        if args.len() < 2 {
            return Err("--root requires a value".into());
        }
        let rest = args.split_off(2);
        return Ok((Some(PathBuf::from(&args[1])), rest));
    }
    if let Some(root) = first.strip_prefix("--root=") {
        let root = PathBuf::from(root);
        return Ok((Some(root), args.split_off(1)));
    }
    Ok((None, args))
}

/// `cargo stand fixture install|build-hostapd`.
#[derive(clap::Parser)]
#[command(name = "cargo stand fixture", no_binary_name = true)]
enum FixtureCli {
    /// Prepare and install one versioned Linux fixture software bundle.
    Install {
        /// Finite Linux fixture provider to provision.
        #[arg(long, value_enum)]
        provider: oer_stand_fixture_install::Provider,
        /// Print the offline plan without executing any installer step.
        #[arg(long)]
        dry_run: bool,
    },
    /// Build the pinned hostapd with explicit HIL coexistence policy support.
    BuildHostapd,
}

/// `cargo stand __command-tree`: every `cargo stand` command path with its
/// subcommands and long flags, as JSON, for the documentation check.
fn command_tree() -> Vec<oer_command_tree::CommandNode> {
    use clap::CommandFactory as _;
    use oer_command_tree::{CommandNode, command_tree as walk};
    let words = |words: &[&str]| {
        words
            .iter()
            .map(|word| (*word).to_owned())
            .collect::<Vec<_>>()
    };
    let mut nodes = vec![CommandNode {
        path: words(&["stand"]),
        subcommands: words(&[
            "queue", "wait", "lease", "board", "devices", "owner", "preempt", "discover", "doctor",
            "init", "install", "fixtures", "fixture",
        ]),
        flags: words(&["--owner", "--root"]),
        forwards: false,
    }];
    nodes.extend(command::command_nodes());
    // `discover` and `doctor` are the subcommands of one parser.
    nodes.extend(
        walk(&discover::StandCli::command(), &words(&["stand"]))
            .into_iter()
            .filter(|node| node.path.len() > 1),
    );
    nodes.extend(walk(&init::InitCli::command(), &words(&["stand", "init"])));
    for name in ["install", "fixtures"] {
        nodes.push(CommandNode {
            path: words(&["stand", name]),
            subcommands: Vec::new(),
            flags: Vec::new(),
            forwards: false,
        });
    }
    nodes.extend(walk(&FixtureCli::command(), &words(&["stand", "fixture"])));
    nodes
}

fn run() -> Result<ExitCode> {
    use clap::Parser as _;
    if oer_command_tree::requested() {
        println!("{}", oer_command_tree::json(&command_tree()));
        return Ok(ExitCode::SUCCESS);
    }
    let (root, args) = split_root(std::env::args_os().skip(1).collect())?;
    let ctx = match root {
        Some(root) => Checkout::new(root)?,
        None => Checkout::discover("cargo stand")?,
    };
    let _signals = oer_process::install_signal_handlers()?;
    let (options, args) = Options::split(&args)?;
    let Some((first, rest)) = args.split_first() else {
        println!("{HELP}");
        return Ok(ExitCode::SUCCESS);
    };
    match first.to_str().unwrap_or_default() {
        "help" | "--help" | "-h" => {
            println!("{HELP}");
            Ok(ExitCode::SUCCESS)
        }
        "queue" => command::queue(rest),
        "wait" => command::wait_for_service(rest),
        "lease" => command::lease(&ctx, options, rest),
        "board" => command::board(&ctx, &options, rest),
        "devices" => command::devices(&ctx, &options, rest),
        "owner" => command::owner(&ctx, rest),
        "preempt" => command::preempt(&options.owner(&ctx)?, rest),
        "discover" | "doctor" => discover::stand(&ctx, || options.owner(&ctx), &args),
        "init" => init::init(&ctx.root, rest),
        "install" => {
            install::run(&ctx)?;
            Ok(ExitCode::SUCCESS)
        }
        "fixtures" => {
            let stand_file = oer_stand_file::paths::stand_file()?;
            print!(
                "{}",
                oer_stand_fixtures::describe(&oer_stand_fixtures::probe(&stand_file))
            );
            Ok(ExitCode::SUCCESS)
        }
        "fixture" => match FixtureCli::try_parse_from(rest)? {
            FixtureCli::Install { provider, dry_run } => {
                fixture_install::run(&ctx.root, provider, dry_run)?;
                Ok(ExitCode::SUCCESS)
            }
            FixtureCli::BuildHostapd => {
                oer_stand_hostapd::build(&ctx.root)?;
                Ok(ExitCode::SUCCESS)
            }
        },
        other => Err(format!("`{other}` is no stand command\n\n{HELP}").into()),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(status) => status,
        Err(error) => {
            // A command line clap refused, or help it was asked for, exits
            // as clap does: usage errors with 2, help with 0.
            if let Some(usage) = error.downcast_ref::<clap::Error>() {
                usage.exit();
            }
            eprintln!("stand: {error}");
            if oer_process::is_cancelled(&*error) {
                ExitCode::from(130)
            } else {
                ExitCode::FAILURE
            }
        }
    }
}
