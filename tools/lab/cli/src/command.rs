//! The stand's commands: the queue, leases, boards, devices and their
//! service, owners and preemption.

use std::{ffi::OsString, path::Path};

use crate::Result;
use oer_lab_service::protocol::{Operation, Request};
use oer_os_process::Checkout;

#[derive(clap::Args)]
pub(crate) struct OwnerCli {
    #[command(subcommand)]
    command: Option<OwnerCommand>,
}

#[derive(clap::Subcommand)]
enum OwnerCommand {
    Set { name: String },
    Merge { old: String, new: String },
    Forget { name: String },
}

/// `cargo stand owner [set NAME | merge OLD NEW | forget NAME]`.
pub(crate) fn owner(ctx: &Checkout, cli: OwnerCli) -> Result<std::process::ExitCode> {
    let owners = oer_lab_arbiter::owner::Owners::open()?;
    let parse = |name: &str| oer_lab_arbiter::owner::Owner::new(name);
    match cli.command {
        None => match owners.of(&ctx.root)? {
            Some(owner) => println!("{owner}"),
            None => return Err(oer_lab_arbiter::owner::NoOwner(ctx.root.clone()).into()),
        },
        Some(OwnerCommand::Set { name }) => {
            let owner = parse(&name)?;
            owners.set(&ctx.root, owner.clone())?;
            println!("{} is owned by {owner}", ctx.root.display());
        }
        Some(OwnerCommand::Merge { old, new }) => {
            let new = parse(&new)?;
            crate::service::client()?.administer(Request::MergeOwner {
                old: old.clone(),
                new: new.to_string(),
            })?;
            println!("{old}'s balance and history are {new}'s");
        }
        Some(OwnerCommand::Forget { name }) => {
            let forgotten = crate::service::client()?
                .administer(Request::ForgetOwner { name: name.clone() })?;
            println!(
                "{name}: {}",
                if forgotten {
                    "balance dropped"
                } else {
                    "no balance"
                }
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[derive(clap::Args)]
pub(crate) struct PreemptCli {
    #[arg(value_parser = parse_lease_id)]
    id: u64,
    #[arg(long)]
    reason: String,
}

fn parse_lease_id(text: &str) -> std::result::Result<u64, String> {
    text.trim_start_matches('#')
        .parse()
        .map_err(|_| format!("{text} is not a lease number"))
}

/// `cargo stand preempt ID --reason TEXT`: the service stops the lease's
/// operations, recovers its resources and tells its holder why.
pub(crate) fn preempt(owner: &str, cli: PreemptCli) -> Result<std::process::ExitCode> {
    crate::service::client()?.preempt(cli.id, owner, &cli.reason)?;
    println!("lease #{} is being stopped", cli.id);
    Ok(std::process::ExitCode::SUCCESS)
}

#[derive(clap::Args)]
#[command(after_help = BALANCE_RULE)]
pub(crate) struct QueueCli {
    #[arg(long)]
    json: bool,
}

/// Print the stand's holder, queue, board state and recent leases.
pub(crate) fn queue(cli: QueueCli) -> Result<std::process::ExitCode> {
    let status = crate::service::client()?.report()?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
    } else {
        println!("{status}");
    }
    Ok(std::process::ExitCode::SUCCESS)
}

#[derive(clap::Args)]
pub(crate) struct WaitCli {
    #[arg(long, required = true)]
    service: bool,
    boards: Vec<String>,
}

/// `cargo stand wait --service [BOARD...]`: until no maintenance keeps the
/// boards (every board when none is named) or the stand out of service and
/// none of them is unavailable.
pub(crate) fn wait_for_service(cli: WaitCli) -> Result<std::process::ExitCode> {
    let stand = oer_lab_model::Stand::load(&oer_lab_model::paths::stand_file()?)?;
    let macs = cli
        .boards
        .iter()
        .map(|board| Ok(stand.resolve(board)?.device_id()?.to_string()))
        .collect::<Result<Vec<_>>>()?;
    let mut client = crate::service::client()?;
    let mut reported = String::new();
    loop {
        let status = client.report()?;
        let mut out = status
            .maintenance
            .iter()
            .filter(|entry| {
                macs.is_empty()
                    || entry.mac == oer_lab_arbiter::STAND_SERVICE
                    || macs.contains(&entry.mac)
            })
            .map(|entry| {
                format!(
                    "{} is under maintenance by {}: {}",
                    if entry.mac == oer_lab_arbiter::STAND_SERVICE {
                        "the stand"
                    } else {
                        entry.mac.as_str()
                    },
                    entry.owner,
                    entry.reason
                )
            })
            .collect::<Vec<_>>();
        out.extend(status.resources.iter().filter_map(|(resource, readiness)| {
            let mac = resource.strip_prefix("board:")?;
            let oer_lab_arbiter::readiness::Readiness::Unavailable { reason, .. } = readiness
            else {
                return None;
            };
            (macs.is_empty() || macs.iter().any(|wanted| wanted == mac))
                .then(|| format!("{mac} is unavailable: {reason}"))
        }));
        if out.is_empty() {
            break;
        }
        let report = out.join("; ");
        if report != reported {
            eprintln!("stand: waiting: {report}");
            reported = report;
        }
        oer_os_process::sleep(SERVICE_POLL)?;
    }
    println!("in service");
    Ok(std::process::ExitCode::SUCCESS)
}

/// How often `cargo stand wait` asks the service again.
const SERVICE_POLL: std::time::Duration = std::time::Duration::from_secs(5);

/// How a lease command uses the radio environment; `None` claims no air.
#[derive(Clone, Copy, Debug, PartialEq)]
struct AirArg(Option<oer_lab_arbiter::claims::Mode>);

fn parse_air(text: &str) -> std::result::Result<AirArg, String> {
    match text {
        "shared" => Ok(AirArg(Some(oer_lab_arbiter::claims::Mode::Shared))),
        "exclusive" => Ok(AirArg(Some(oer_lab_arbiter::claims::Mode::Exclusive))),
        "none" => Ok(AirArg(None)),
        _ => Err(String::from("use `shared`, `exclusive` or `none`")),
    }
}

/// The resources of a lease command: its boards and the air, or the whole
/// stand when it names neither.
fn lease_claims(
    boards: &[String],
    air: Option<AirArg>,
    stand: bool,
    file: &oer_lab_model::Stand,
) -> Result<Vec<oer_lab_arbiter::claims::Claim>> {
    match (stand, boards.is_empty() && air.is_none()) {
        (true, true) => return Ok(vec![oer_lab_arbiter::claims::Claim::stand()]),
        (true, false) => return Err("--stand claims everything; drop --board and --air".into()),
        // A whole-stand lease blocks every other owner, so it is never the
        // accidental result of naming nothing.
        (false, true) => {
            return Err(
                "name what the command uses with --board NAME|MAC (repeatable) and \
                        --air, or claim the whole stand with --stand"
                    .into(),
            );
        }
        (false, false) => {}
    }
    let mut claims = boards
        .iter()
        .map(|board| {
            Ok(oer_lab_arbiter::claims::Claim::board(
                &file.resolve(board)?.device_id()?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    // Boards use the air shared unless told otherwise.
    if let Some(mode) = air.map_or(
        Some(oer_lab_arbiter::claims::Mode::Shared),
        |AirArg(mode)| mode,
    ) {
        claims.push(oer_lab_arbiter::claims::Claim {
            resource: oer_lab_arbiter::claims::AIR.to_owned(),
            mode,
        });
    }
    if claims.is_empty() {
        return Err("--air none needs --board: the lease would claim nothing".into());
    }
    Ok(claims)
}

/// The `cargo stand` arguments of a leased command that is itself a stand
/// command, through `cargo stand` or the installed `oer-tool-lab-cli`.
fn nested_stand(program: &OsString, arguments: &[OsString]) -> Option<Vec<OsString>> {
    let name = Path::new(program).file_name()?.to_str()?;
    match (
        name,
        arguments.first().and_then(|argument| argument.to_str()),
    ) {
        ("cargo", Some("stand")) => Some(arguments[1..].to_vec()),
        ("oer-tool-lab-cli", _) => Some(arguments.to_vec()),
        _ => None,
    }
}

/// Parse `cargo stand` arguments as the command they name will, without
/// running it.
fn check_stand_arguments(args: &[OsString]) -> Result<()> {
    use clap::Parser as _;
    crate::Cli::try_parse_from(args)
        .map(drop)
        .map_err(|error| format!("`cargo stand`: {}", error.render().to_string().trim()).into())
}

/// `cargo stand lease [OPTIONS] -- COMMAND...`
#[derive(Debug, clap::Parser)]
#[command(name = "cargo stand lease", no_binary_name = true)]
pub(crate) struct LeaseCli {
    /// A board the command uses, by registered name, chip or MAC;
    /// repeatable.
    #[arg(long = "board", value_name = "NAME|MAC")]
    boards: Vec<String>,
    /// Claim the whole stand: every board, fixture and the air.
    #[arg(long)]
    stand: bool,
    /// How the command uses the radio environment: `shared` (default with
    /// boards), `exclusive` for RF measurements, or `none` for work that
    /// never enables a radio, which then runs beside an exclusive air lease.
    #[arg(long, value_parser = parse_air)]
    air: Option<AirArg>,
    /// Journal a flash that COMMAND performs when it succeeds.
    #[command(flatten)]
    flashed: FlashedArgs,
    #[arg(last = true, required = true)]
    pub(crate) command: Vec<OsString>,
}

/// A flash a leased command performed outside the flash operation,
/// recorded in the board journal when the command succeeds.
#[derive(Clone, Debug, Default, clap::Args)]
struct FlashedArgs {
    /// Name of the flashed image, e.g. `ieee802154-peer`.
    #[arg(long = "flashed")]
    image: Option<String>,
    /// The flashed application binary, whose SHA-256 is recorded.
    #[arg(long, requires = "image", conflicts_with = "sha256")]
    application: Option<std::path::PathBuf>,
    /// SHA-256 of the flashed application instead of `--application`.
    #[arg(long, requires = "image")]
    sha256: Option<String>,
    /// The flashed board: its stand-file id, its chip or its MAC.
    #[arg(long, value_name = "NAME|MAC", requires = "image")]
    device: Option<String>,
    /// Source commit of the image.
    #[arg(long, requires = "image")]
    commit: Option<String>,
}

impl FlashedArgs {
    fn record(
        &self,
        client: &mut oer_lab_service::client::Client,
        stand: &oer_lab_model::Stand,
        owner: String,
        origin: String,
    ) -> Result<()> {
        let Some(request) = self.request(stand, owner, origin)? else {
            return Ok(());
        };
        client.administer(request)?;
        if let Some(image) = &self.image {
            eprintln!("stand: recorded {image}");
        }
        Ok(())
    }

    /// The service's journal request for the flash, when one is described.
    fn request(
        &self,
        stand: &oer_lab_model::Stand,
        owner: String,
        origin: String,
    ) -> Result<Option<Request>> {
        let Some(image) = &self.image else {
            return Ok(None);
        };
        let sha256 = match (&self.application, &self.sha256) {
            (Some(application), None) => oer_os_durable::sha256_file(application)?,
            (None, Some(hash)) => hash.clone(),
            _ => return Err("a recorded flash needs --application or --sha256".into()),
        };
        let device = self
            .device
            .as_deref()
            .ok_or("a recorded flash needs --device NAME|MAC")?;
        let mac = stand.resolve(device)?.device_id()?;
        Ok(Some(Request::RecordFlash {
            owner,
            mac: mac.to_string(),
            image: oer_lab_arbiter::journal::ImageIdentity {
                name: image.clone(),
                sha256,
                commit: self.commit.clone(),
                dirty: None,
                origin,
            },
        }))
    }
}

/// Why a lease refuses `command`: a hub port is switched only by a power
/// cycle of a board of the stand file (`cargo stand board reset BOARD --via
/// power`), never by `uhubctl` in a leased command, which could leave a port
/// off. The program is matched, directly or as a command of a shell's `-c`
/// script; an argument that only names it (`grep uhubctl`) passes.
fn switches_hub_power(command: &[OsString]) -> Option<String> {
    let is_uhubctl = |word: &str| {
        Path::new(word)
            .file_name()
            .is_some_and(|name| name == "uhubctl")
    };
    let (program, arguments) = command.split_first()?;
    let program = program.to_string_lossy();
    let shell = matches!(
        Path::new(program.as_ref())
            .file_name()
            .and_then(|name| name.to_str()),
        Some("sh" | "bash" | "zsh" | "fish" | "dash")
    );
    let script_runs_it = || {
        arguments
            .iter()
            .skip_while(|argument| *argument != "-c")
            .nth(1)
            .is_some_and(|script| {
                script
                    .to_string_lossy()
                    .split([';', '&', '|', '\n', '(', ')', '`'])
                    .filter_map(|segment| {
                        segment.split_whitespace().find(|word| {
                            !word.contains('=')
                                && !matches!(*word, "sudo" | "env" | "exec" | "command")
                        })
                    })
                    .any(is_uhubctl)
            })
    };
    (is_uhubctl(&program) || (shell && script_runs_it())).then(|| {
        "a lease runs no `uhubctl`: cycle a board's hub port with \
         `cargo stand board reset BOARD --via power`"
            .to_owned()
    })
}

/// Run one command, typically a series of HIL commands, under one lease.
/// The LAB service runs it as its own child, with this command's arguments,
/// directory and environment; its output and input pass through. Nested
/// `cargo stand`, `cargo fw` and `cargo hil` commands join the lease instead
/// of queueing. The service ends it at the hard limit every lease has.
pub(crate) fn lease(
    ctx: &Checkout,
    outer: crate::Options,
    cli: LeaseCli,
) -> Result<std::process::ExitCode> {
    let options = outer;
    let (program, arguments) = cli
        .command
        .split_first()
        .ok_or("cargo stand lease needs a COMMAND after --")?;
    if let Some(refusal) = switches_hub_power(&cli.command) {
        return Err(refusal.into());
    }
    // A mistake in a nested stand command fails now, not after the wait.
    if let Some(nested) = nested_stand(program, arguments) {
        check_stand_arguments(&nested).map_err(|error| format!("not leased: {error}"))?;
    }
    let work = cli
        .command
        .iter()
        .map(|argument| argument.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ");
    let owner = options.owner(ctx)?;
    let stand = oer_lab_model::Stand::load(&oer_lab_model::paths::stand_file()?)?;
    let mut client = crate::service::lease(
        owner.clone(),
        work.clone(),
        lease_claims(&cli.boards, cli.air, cli.stand, &stand)?,
    )?;
    let mut environment = std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect::<Vec<_>>();
    environment.retain(|(key, _)| key != oer_lab_arbiter::owner::OWNER_ENV);
    environment.push((oer_lab_arbiter::owner::OWNER_ENV.to_owned(), owner.clone()));
    // On a terminal the command gets a pseudo-terminal of its size, and this
    // terminal passes every key through until the command ends.
    let terminal = terminal_size();
    let _raw = terminal.and_then(|_| RawTerminal::enter());
    let running = client.start(Operation::Command {
        argv: cli
            .command
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect(),
        directory: std::env::current_dir()?.display().to_string(),
        environment,
        terminal,
    })?;
    // A cancelled lease command stops its command through the service.
    let stop = running.input();
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watching = done.clone();
    std::thread::spawn(move || {
        while !watching.load(std::sync::atomic::Ordering::SeqCst) {
            if oer_os_process::cancellation_requested() {
                let _ = stop.cancel();
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
    });
    let input = running.input();
    std::thread::spawn(move || {
        use std::io::Read as _;
        let mut stdin = std::io::stdin();
        let mut buffer = vec![0u8; 4096];
        while let Ok(read) = stdin.read(&mut buffer) {
            let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
                break;
            };
            if input.send(chunk).is_err() {
                return;
            }
        }
        let _ = input.close();
    });
    let result = running.finish(&mut std::io::stdout());
    done.store(true, std::sync::atomic::Ordering::SeqCst);
    if let Some(reason) = client.revoked() {
        eprintln!("stand: the lease ended: {reason}");
        return Ok(std::process::ExitCode::from(HARD_LIMIT_EXIT));
    }
    let code = result?;
    let succeeded = code == serde_json::json!(0);
    if cli.flashed.image.is_some() {
        if succeeded {
            cli.flashed
                .record(&mut client, &stand, owner, format!("lease `{work}`"))?;
        } else {
            eprintln!("stand: the command failed; its flash is not recorded");
        }
    }
    if oer_os_process::cancellation_requested() {
        return Ok(std::process::ExitCode::from(130));
    }
    Ok(std::process::ExitCode::from(
        code.as_u64()
            .and_then(|code| u8::try_from(code).ok())
            .unwrap_or(1),
    ))
}

/// The size of the terminal this command runs on, when its input and output
/// are one.
fn terminal_size() -> Option<oer_lab_service::protocol::TerminalSize> {
    use std::io::IsTerminal as _;
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        return None;
    }
    let size = rustix::termios::tcgetwinsize(std::io::stdout()).ok()?;
    Some(oer_lab_service::protocol::TerminalSize {
        rows: size.ws_row,
        columns: size.ws_col,
    })
}

/// This terminal in raw mode until dropped, so the leased command's own
/// terminal handles every key.
struct RawTerminal(rustix::termios::Termios);

impl RawTerminal {
    fn enter() -> Option<Self> {
        let saved = rustix::termios::tcgetattr(std::io::stdin()).ok()?;
        let mut raw = saved.clone();
        raw.make_raw();
        rustix::termios::tcsetattr(
            std::io::stdin(),
            rustix::termios::OptionalActions::Now,
            &raw,
        )
        .ok()?;
        Some(Self(saved))
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = rustix::termios::tcsetattr(
            std::io::stdin(),
            rustix::termios::OptionalActions::Now,
            &self.0,
        );
    }
}

/// `cargo stand board reset|check|console|soak`.
pub(crate) fn board(
    ctx: &Checkout,
    options: &crate::Options,
    cli: BoardCli,
) -> Result<std::process::ExitCode> {
    let BoardCli { command } = cli;
    crate::board::board(ctx, options.owner(ctx)?, command)
}

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
enum ConfirmArg {
    Reset,
    PowerCycle,
    /// No person acted: the release check's RTS reset shows the ROM
    /// answering, so the board never needed one.
    RomAnswers,
}

/// `cargo stand devices [--json]` and its board commands.
pub(crate) fn devices(
    ctx: &Checkout,
    options: &crate::Options,
    cli: DevicesCli,
) -> Result<std::process::ExitCode> {
    let stand = || -> Result<oer_lab_model::Stand> {
        oer_lab_model::Stand::load(&oer_lab_model::paths::stand_file()?)
    };
    let mut client = crate::service::client()?;
    match cli.command {
        Some(DevicesCommand::Maintenance {
            board,
            stand: whole,
            reason,
        }) => {
            let (board, mac) = match (board, whole) {
                (_, true) => (
                    String::from("the stand"),
                    String::from(oer_lab_arbiter::STAND_SERVICE),
                ),
                (Some(board), false) => {
                    let mac = stand()?.resolve(&board)?.device_id()?.to_string();
                    (board, mac)
                }
                (None, false) => return Err("name a board or pass --stand".into()),
            };
            let owner = options.owner(ctx)?;
            client.administer(Request::Maintain {
                mac: mac.clone(),
                owner: owner.clone(),
                reason,
            })?;
            println!("{board} ({mac}) is under maintenance by {owner}");
            let claim = if mac == oer_lab_arbiter::STAND_SERVICE {
                oer_lab_arbiter::claims::Claim::stand()
            } else {
                oer_lab_arbiter::claims::Claim::board(&mac)
            };
            let claims = oer_lab_arbiter::describe_claims(&[claim]);
            for holder in client.report()?.holders {
                if holder.claims == claims || claims == "the whole stand" {
                    println!(
                        "still held by #{} {} `{}` until it ends",
                        holder.id, holder.owner, holder.work
                    );
                }
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(DevicesCommand::Release {
            board: None,
            stand: true,
            ..
        }) => {
            if client.administer(Request::EndMaintenance {
                mac: oer_lab_arbiter::STAND_SERVICE.to_owned(),
            })? {
                println!("the stand is back in service");
            } else {
                println!("the stand was not under maintenance");
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        Some(DevicesCommand::Release { board, confirm, .. }) => {
            let board = board.ok_or("name a board or pass --stand")?;
            let mac = stand()?.resolve(&board)?.device_id()?.to_string();
            let resource = format!("board:{mac}");
            let unavailable = client
                .report()?
                .resources
                .into_iter()
                .any(|(name, readiness)| {
                    name == resource
                        && matches!(
                            readiness,
                            oer_lab_arbiter::readiness::Readiness::Unavailable { .. }
                        )
                });
            if unavailable {
                let confirmation = match confirm.ok_or(
                    "the board is unavailable: reset or power-cycle it, then pass --confirm \
                     reset|power-cycle; --confirm rom-answers tries the stand's own reset first",
                )? {
                    ConfirmArg::Reset => oer_lab_arbiter::journal::Confirmation::Reset,
                    ConfirmArg::PowerCycle => oer_lab_arbiter::journal::Confirmation::PowerCycle,
                    ConfirmArg::RomAnswers => oer_lab_arbiter::journal::Confirmation::RomAnswers,
                };
                client.return_resource(&resource)?;
                let readiness = loop {
                    let readiness = client
                        .report()?
                        .resources
                        .into_iter()
                        .find(|(name, _)| *name == resource)
                        .map(|(_, readiness)| readiness);
                    if !matches!(
                        readiness,
                        Some(oer_lab_arbiter::readiness::Readiness::Recovering { .. })
                    ) {
                        break readiness;
                    }
                    oer_os_process::sleep(std::time::Duration::from_secs(1))?;
                };
                let check = match &readiness {
                    Some(oer_lab_arbiter::readiness::Readiness::Ready { .. }) => {
                        String::from("its ROM waits for a download over USB")
                    }
                    Some(oer_lab_arbiter::readiness::Readiness::Unavailable { reason, .. }) => {
                        return Err(
                            format!("{board} ({mac}) is still unavailable: {reason}").into()
                        );
                    }
                    other => format!("{other:?}"),
                };
                client.administer(Request::Record {
                    owner: options.owner(ctx)?,
                    device: Some(mac.clone()),
                    event: oer_lab_arbiter::journal::BoardEventKind::QuarantineReleased {
                        confirmation,
                        check: check.clone(),
                    },
                })?;
                println!("{board} ({mac}) is back in service: {check}");
                return Ok(std::process::ExitCode::SUCCESS);
            }
            if client.administer(Request::EndMaintenance { mac: mac.clone() })? {
                println!("{board} ({mac}) is back in service");
            } else {
                println!("{board} ({mac}) was not under maintenance");
            }
            return Ok(std::process::ExitCode::SUCCESS);
        }
        None => {}
    }
    let status = client.report()?;
    if cli.json {
        println!("{}", serde_json::to_string_pretty(&status.devices)?);
    } else {
        for device in &status.devices {
            println!(
                "{} [{}]{}: {}",
                device.label,
                device.port.as_deref().unwrap_or("not attached"),
                device
                    .health
                    .as_ref()
                    .map(|health| format!(" (health: {health})"))
                    .unwrap_or_default(),
                device
                    .firmware
                    .as_ref()
                    .map_or_else(|| String::from("firmware unknown"), ToString::to_string)
            );
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

// The clap parsers of the stand commands, walked by `__command-tree`.
#[derive(clap::Parser)]
#[command(name = "cargo stand board", no_binary_name = true)]
pub(crate) struct BoardCli {
    #[command(subcommand)]
    command: crate::board::BoardCommand,
}

#[derive(clap::Parser)]
#[command(name = "cargo stand devices", no_binary_name = true)]
pub(crate) struct DevicesCli {
    #[arg(long)]
    json: bool,
    #[command(subcommand)]
    command: Option<DevicesCommand>,
}
#[derive(clap::Subcommand)]
enum DevicesCommand {
    /// Take a board out of service: until `release`, only this owner
    /// (`--owner`, else the checkout) may claim it. Leases already held
    /// run on.
    #[command(group(clap::ArgGroup::new("what").required(true).args(["board", "stand"])))]
    Maintenance {
        #[arg(value_name = "NAME|MAC")]
        board: Option<String>,
        /// The whole stand: only this owner's requests are served, other
        /// runs wait until `release --stand`.
        #[arg(long)]
        stand: bool,
        #[arg(long)]
        reason: String,
    },
    /// Return a board, or the whole stand, to service.
    #[command(group(clap::ArgGroup::new("what").required(true).args(["board", "stand"])))]
    Release {
        #[arg(value_name = "NAME|MAC")]
        board: Option<String>,
        #[arg(long)]
        stand: bool,
        /// What returns a quarantined board: a person pressed its reset
        /// button or power-cycled it, or its ROM answers the stand's own
        /// reset. It returns only if it then boots.
        #[arg(long, value_enum)]
        confirm: Option<ConfirmArg>,
    },
}

/// How the stand orders conflicting requests, for `cargo stand queue --help`.
const BALANCE_RULE: &str = "\
Who goes next: every lease charges its owner's balance the time it holds, and
waiting behind a conflicting lease credits the time waited. The owner with the
highest balance is served first, the earlier request on a tie; balances halve
every 2h and stay within 1h either way. A run of several scenarios that has
held 10m yields after its current scenario to a waiter with a higher balance.
Every lease ends at 1h. There is no budget to request.";

/// Exit status of a lease command terminated at the hard limit.
const HARD_LIMIT_EXIT: u8 = 124;

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_STAND: &str = "schema = 1\n[stand]\nid = \"t\"\nair = \"exclusive\"\n\
             [[hub]]\nid = \"h\"\nusb2 = \"1-1\"\n\
             [[board]]\nid = \"peer-a\"\nusb-serial = \"02:00:00:00:25:64\"\nchip = \"chip-b\"\n\
             radios = [\"ble\"]\nroles = [\"peer\"]\nport = { hub = \"h\", port = 1 }\nreset = [\"jtag\"]\n";

    #[test]
    fn a_lease_runs_no_hub_power_switch() {
        let command = |words: &[&str]| words.iter().map(OsString::from).collect::<Vec<_>>();
        for refused in [
            command(&["uhubctl", "-l", "3-8.3", "-p", "2", "-a", "off"]),
            command(&["sh", "-c", "uhubctl -l 3-8.3 -p 2 -a off; sleep 5"]),
            command(&["/usr/sbin/uhubctl", "-a", "cycle"]),
            command(&["bash", "-c", "sleep 1 && sudo uhubctl -a on"]),
        ] {
            assert!(super::switches_hub_power(&refused).is_some(), "{refused:?}");
        }
        for allowed in [
            command(&["cargo", "hil", "peer", "send"]),
            command(&[
                "cargo",
                "hil",
                "devices",
                "set",
                "AA",
                "--power-uhubctl",
                "3-8.3",
                "--power-port",
                "2",
            ]),
            command(&["sh", "-c", "echo uhubctl is refused"]),
        ] {
            assert!(super::switches_hub_power(&allowed).is_none(), "{allowed:?}");
        }
    }

    #[test]
    fn lease_records_a_described_flash_on_the_named_board() {
        use clap::Parser as _;
        let cli = LeaseCli::try_parse_from([
            "--flashed",
            "ieee802154-peer",
            "--sha256",
            &"AB".repeat(32),
            "--device",
            "02:00:00:00:25:64",
            "--",
            "idf.py",
            "flash",
        ])
        .unwrap();
        assert_eq!(cli.command, ["idf.py", "flash"]);
        assert!(
            LeaseCli::try_parse_from(["idf.py"]).is_err(),
            "COMMAND follows --"
        );
        assert!(
            LeaseCli::try_parse_from(["--sha256", "ab", "--", "x"]).is_err(),
            "flash details need --flashed"
        );
        let stand = oer_lab_model::Stand::parse(TEST_STAND).unwrap();
        let Some(Request::RecordFlash { owner, mac, image }) = cli
            .flashed
            .request(&stand, "802154".into(), "test".into())
            .unwrap()
        else {
            panic!("no flash record");
        };
        assert_eq!(mac, "02:00:00:00:25:64");
        assert_eq!(owner, "802154");
        assert!(image.sha256.eq_ignore_ascii_case(&"ab".repeat(32)));
        let incomplete = FlashedArgs {
            image: Some("x".into()),
            sha256: Some("ab".repeat(32)),
            ..FlashedArgs::default()
        };
        assert!(
            incomplete
                .request(&stand, "802154".into(), "test".into())
                .is_err()
        );
    }

    #[test]
    fn a_lease_claims_its_boards_and_the_air_or_explicitly_the_whole_stand() {
        use oer_lab_arbiter::claims::{AIR, Claim, Mode};
        let devices = oer_lab_model::Stand::parse(TEST_STAND).unwrap();
        assert_eq!(
            lease_claims(&[], None, true, &devices).unwrap(),
            [Claim::stand()]
        );
        // Naming nothing never claims the whole stand by accident.
        assert!(lease_claims(&[], None, false, &devices).is_err());
        assert!(lease_claims(&["chip-b".into()], None, true, &devices).is_err());
        assert_eq!(
            lease_claims(&["chip-b".into()], None, false, &devices).unwrap(),
            [Claim::board("02:00:00:00:25:64"), Claim::shared(AIR)]
        );
        assert_eq!(
            lease_claims(&[], Some(AirArg(Some(Mode::Exclusive))), false, &devices).unwrap(),
            [Claim::exclusive(AIR)]
        );
        assert!(lease_claims(&["s3".into()], None, false, &devices).is_err());
        assert_eq!(
            lease_claims(&["chip-b".into()], Some(AirArg(None)), false, &devices).unwrap(),
            [Claim::board("02:00:00:00:25:64")]
        );
        assert!(lease_claims(&[], Some(AirArg(None)), false, &devices).is_err());
    }
}
