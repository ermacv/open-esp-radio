//! `cargo stand board reset|check|console|soak`: the stand's own ways to
//! reach a board's port.
//!
//! Opening a USB serial port with the default modem lines resets an
//! Espressif chip, and a UART bridge's lines drive EN and BOOT, so only the
//! LAB service opens board ports: these commands lease the board from it
//! and ask it for each operation.

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::Result;
use oer_lab_client::{
    client::{Client, Input},
    protocol::{Operation, Request},
};
use oer_lab_model::rom::ResetPath;
use oer_os_process::Checkout;

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum Via {
    /// Pulse RTS on the chip's USB Serial/JTAG port, the boot strap released.
    Rts,
    /// Reset the CPU through the chip's JTAG with OpenOCD.
    Jtag,
    /// Cycle the power of the board's hub port, then, as soon as its USB
    /// returns, reset it into the ROM's download mode: the stand's way into a
    /// board whose image switches its USB Serial/JTAG off.
    Download,
    /// Cycle the power of the board's hub port: off, then on. The
    /// stand's only way to switch a hub port; a lease runs no `uhubctl`.
    Power,
}

/// `cargo stand board reset|check|console`.
#[derive(Debug, clap::Subcommand)]
pub(crate) enum BoardCommand {
    /// Reset a board and print the reset reason its ROM reports.
    Reset {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        #[arg(long, value_enum, default_value = "rts")]
        via: Via,
    },
    /// Report whether a board is attached and answers, what it runs and how
    /// the stand reaches it, without resetting it.
    Check {
        #[arg(value_name = "NAME|MAC")]
        board: String,
    },
    /// Print a board's console for a bounded time without resetting it.
    Console {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// How long, e.g. 10s or 2m.
        #[arg(long = "for", default_value = "10s", value_parser = parse_duration)]
        duration: Duration,
        /// End at the first line containing this text; fail when none does.
        #[arg(long)]
        until: Option<String>,
    },
    /// Reset a board again and again through each path, stopping at the
    /// first reset after which it does not boot; the result is journaled.
    #[command(group(clap::ArgGroup::new("length").required(true).args(["cycles", "duration"])))]
    Soak {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// Cycles to run; one cycle resets through each path once.
        #[arg(long)]
        cycles: Option<u32>,
        /// How long to run, e.g. 8h.
        #[arg(long = "for", value_parser = parse_duration)]
        duration: Option<Duration>,
        /// Reset paths, each once per cycle.
        #[arg(long, value_enum, value_delimiter = ',', default_value = "rts")]
        via: Vec<Via>,
        /// Cycles per lease; other owners may use the board between them.
        #[arg(long, default_value_t = 10)]
        batch: u32,
    },
}

impl Via {
    fn path(self) -> ResetPath {
        match self {
            Self::Rts => ResetPath::Rts,
            Self::Jtag => ResetPath::Jtag,
            Self::Power => ResetPath::Power,
            Self::Download => ResetPath::Download,
        }
    }
}

fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    oer_lab_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// A board of the stand file, resolved for a command.
struct Target {
    /// Its stand-file entry.
    board: oer_lab_model::Board,
    mac: oer_lab_model::DeviceId,
}

fn target(board: &str) -> Result<Target> {
    let stand = oer_lab_model::Stand::load(&oer_lab_model::paths::stand_file()?)?;
    let entry = stand.resolve(board)?.clone();
    Ok(Target {
        mac: entry.device_id()?,
        board: entry,
    })
}

/// A lease of `target`'s board.
fn lease(target: &Target, owner: String, work: String) -> Result<Client> {
    crate::service::lease(
        owner,
        work,
        vec![oer_lab_arbiter::claims::Claim::board(&target.board.mac)],
    )
}

/// Reset `target`'s board through `path`; the ROM's reset line.
fn reset(client: &mut Client, target: &Target, path: ResetPath) -> Result<Option<String>> {
    Ok(serde_json::from_value(crate::service::run(
        client,
        Operation::Reset {
            board: target.board.mac.clone(),
            path,
        },
    )?)?)
}

/// `target`'s console lines, opened without a reset.
fn console_lines(client: &mut Client, target: &Target) -> Result<(mpsc::Receiver<Vec<u8>>, Input)> {
    let console = client.start(Operation::Console {
        board: target.board.mac.clone(),
        reset: false,
    })?;
    let input = console.input();
    Ok((crate::service::lines(console), input))
}

pub(crate) fn board(
    ctx: &Checkout,
    owner: String,
    command: BoardCommand,
) -> Result<std::process::ExitCode> {
    match command {
        BoardCommand::Reset { board, via } => {
            let target = target(&board)?;
            let mut lease = lease(&target, owner, format!("board reset {board} --via {via:?}"))?;
            let line = reset(&mut lease, &target, via.path())?;
            println!(
                "{board} ({}) reset via {}: {}",
                target.mac,
                format!("{via:?}").to_lowercase(),
                line.as_deref()
                    .unwrap_or("no ROM reset line on its console")
            );
        }
        BoardCommand::Check { board } => {
            let target = target(&board)?;
            let mut lease = lease(&target, owner, format!("board check {board}"))?;
            check(&target, &mut lease, &board)?;
        }
        BoardCommand::Console {
            board,
            duration,
            until,
        } => {
            let target = target(&board)?;
            let mut lease = lease(&target, owner, format!("board console {board}"))?;
            let log = ctx
                .root
                .join("target/hil/console")
                .join(target.mac.compact());
            std::fs::create_dir_all(&log)?;
            let log = log.join(format!("console-{}.log", oer_os_durable::unix_seconds()));
            let (lines, input) = console_lines(&mut lease, &target)?;
            let seen = capture(&lines, duration, until.as_deref(), &log)?;
            let _ = input.cancel();
            eprintln!("stand: console of {board} written to {}", log.display());
            if until.is_some() && !seen {
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
        BoardCommand::Soak {
            board,
            cycles,
            duration,
            via,
            batch,
        } => return soak(ctx, owner, &board, cycles, duration, &via, batch.max(1)),
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Whether a reset's ROM line shows the board booting from flash, or why not.
fn booted(line: Option<&str>) -> std::result::Result<(), String> {
    match line {
        None => Err(String::from("no ROM reset line on its console")),
        Some(line) if line.contains("DOWNLOAD") => {
            Err(format!("its ROM waits for a download: {line}"))
        }
        Some(_) => Ok(()),
    }
}

/// `cargo stand board soak`: batches of cycles under their own leases, ending
/// at `cycles`, after `duration`, or at the first reset that did not boot.
fn soak(
    ctx: &Checkout,
    owner: String,
    board: &str,
    cycles: Option<u32>,
    duration: Option<Duration>,
    via: &[Via],
    batch: u32,
) -> Result<std::process::ExitCode> {
    let target = target(board)?;
    let started = Instant::now();
    let done = |cycle: u32| {
        cycles.is_some_and(|cycles| cycle >= cycles)
            || duration.is_some_and(|duration| started.elapsed() >= duration)
    };
    let (mut cycle, mut resets, mut failure) = (0_u32, 0_u32, None);
    'soak: while !done(cycle) {
        let mut leased = lease(
            &target,
            owner.clone(),
            format!("board soak {board} cycles {}..", cycle + 1),
        )?;
        for _ in 0..batch {
            if done(cycle) {
                break;
            }
            cycle += 1;
            for &path in via {
                resets += 1;
                let line = reset(&mut leased, &target, path.path())?;
                let name = format!("{path:?}").to_lowercase();
                match booted(line.as_deref()) {
                    Ok(()) => println!("{cycle} {name}: {}", line.unwrap_or_default()),
                    Err(why) => {
                        println!("{cycle} {name}: FAILED: {why}");
                        // What the board prints next is the evidence.
                        let log = ctx
                            .root
                            .join("target/hil/console")
                            .join(target.mac.compact())
                            .join(format!(
                                "soak-failure-{}.log",
                                oer_os_durable::unix_seconds()
                            ));
                        std::fs::create_dir_all(log.parent().ok_or("no parent")?)?;
                        if let Ok((lines, input)) = console_lines(&mut leased, &target) {
                            let _ = capture(&lines, Duration::from_secs(5), None, &log);
                            let _ = input.cancel();
                            eprintln!("stand: console after the failure in {}", log.display());
                        }
                        failure = Some(format!("cycle {cycle} via {name}: {why}"));
                        break 'soak;
                    }
                }
            }
        }
    }
    crate::service::client()?.administer(Request::Record {
        owner,
        device: Some(target.mac.to_string()),
        event: oer_lab_arbiter::journal::BoardEventKind::Soaked {
            paths: via.iter().map(|via| via.path()).collect(),
            cycles: cycle,
            resets,
            failure: failure.clone(),
        },
    })?;
    println!(
        "{board}: {resets} resets in {cycle} cycles: {}",
        failure.as_deref().unwrap_or("every reset booted")
    );
    Ok(if failure.is_some() {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    })
}

/// Print `lines` until the `@OK` (true) or `@ERR` (false) answer to
/// `command` in the peers' one grammar ([`oer_lab_model::peer_line::parse`]), or
/// `None` after `duration`. An answer naming another command, such as noise
/// a peer read while it booted, is only printed.
fn answer(
    lines: &mpsc::Receiver<Vec<u8>>,
    command: &str,
    duration: Duration,
    print: &mut dyn FnMut(&str),
) -> Option<bool> {
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                let text = String::from_utf8_lossy(&line);
                print(&text);
                match oer_lab_model::peer_line::parse(text.trim()) {
                    Some(oer_lab_model::peer_line::Line::Ok { command: about })
                        if about.eq_ignore_ascii_case(command) =>
                    {
                        return Some(true);
                    }
                    Some(oer_lab_model::peer_line::Line::Err { command: about, .. })
                        if about.eq_ignore_ascii_case(command) =>
                    {
                        return Some(false);
                    }
                    _ => {}
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// Copy `lines` to the terminal and `log` until `duration` passes or a line
/// contains `until`; returns whether one did.
fn capture(
    lines: &mpsc::Receiver<Vec<u8>>,
    duration: Duration,
    until: Option<&str>,
    log: &std::path::Path,
) -> Result<bool> {
    use std::io::Write as _;
    let mut file = std::fs::File::create(log)?;
    let deadline = Instant::now() + duration;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match lines.recv_timeout(remaining) {
            Ok(line) if line.is_empty() => {}
            Ok(line) => {
                file.write_all(&line)?;
                file.write_all(b"\n")?;
                let text = String::from_utf8_lossy(&line);
                println!("{text}");
                if until.is_some_and(|until| text.contains(until)) {
                    return Ok(true);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    Ok(false)
}

fn check(target: &Target, lease: &mut Client, board: &str) -> Result<()> {
    let mac = &target.mac;
    let described = crate::service::run(
        lease,
        Operation::Describe {
            board: mac.to_string(),
        },
    )?;
    let text = |key: &str| {
        described
            .get(key)
            .and_then(serde_json::Value::as_str)
            .unwrap_or("?")
            .to_owned()
    };
    println!(
        "{board} ({mac}): attached at {}, chip {}",
        text("port"),
        target.board.chip
    );
    // What the board runs is its receipt's; the journal is history.
    println!("  image: {}", text("image"));
    let arbiter = oer_lab_arbiter::Arbiter::open()?;
    match arbiter.journal().latest_flash(mac)? {
        Some(event) => println!("  last journaled flash: {event}"),
        None => println!("  last journaled flash: none"),
    }
    if let Some(maintenance) = crate::service::client()?
        .report()?
        .maintenance
        .into_iter()
        .find(|entry| *mac == entry.mac)
    {
        println!(
            "  maintenance by {}: {}",
            maintenance.owner, maintenance.reason
        );
    }
    println!(
        "  reset paths: rts, jtag{}",
        if target
            .board
            .reset
            .contains(&oer_lab_model::ResetStep::Power)
        {
            ", power (hub port)"
        } else {
            ""
        }
    );
    // A text-protocol peer answers SYNC; a HIL runtime answers only its binary
    // protocol, which the runner speaks; a board between leases waits in its
    // ROM's download mode and answers nothing.
    let (lines, input) = console_lines(lease, target)?;
    input.send(b"\nSYNC\n")?;
    let mut heard = Vec::new();
    let answered = answer(&lines, "SYNC", Duration::from_secs(2), &mut |text| {
        heard.push(text.to_owned())
    });
    match answered {
        Some(true) => println!("  answers: the peer text protocol (@OK SYNC)"),
        _ if !heard.is_empty() => println!(
            "  answers: console output, no peer text protocol; last line: {}",
            heard.last().map(String::as_str).unwrap_or_default()
        ),
        _ => println!(
            "  answers: nothing within 2s (a HIL runtime speaks only the runner's protocol; \
             a board between leases waits in its ROM's download mode; a hung or halted chip \
             says nothing)"
        ),
    }
    let _ = input.cancel();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_soak_counts_only_resets_that_boot_from_flash() {
        assert!(
            booted(Some(
                "rst:0x15 (USB_UART_HPSYS),boot:0x18 (SPI_FAST_FLASH_BOOT)"
            ))
            .is_ok()
        );
        assert!(
            booted(Some("rst:0x1 (POWERON),boot:0x4 (DOWNLOAD(USB/UART0))"))
                .unwrap_err()
                .contains("download")
        );
        assert!(booted(None).is_err());
    }

    fn lines(text: &[&str]) -> mpsc::Receiver<Vec<u8>> {
        let (sender, received) = mpsc::channel();
        for line in text {
            sender.send(line.as_bytes().to_vec()).unwrap();
        }
        std::mem::forget(sender);
        received
    }

    #[test]
    fn a_peer_answer_ends_at_its_ok_or_err_line() {
        let mut printed = Vec::new();
        let ok = answer(
            &lines(&["@READY", "@ERR n unknown", "@OK SYNC", "later"]),
            "SYNC",
            Duration::from_secs(1),
            &mut |text| printed.push(text.to_owned()),
        );
        assert_eq!(ok, Some(true));
        assert_eq!(printed, ["@READY", "@ERR n unknown", "@OK SYNC"]);
        let err = answer(
            &lines(&["@ERR BURST busy"]),
            "BURST",
            Duration::from_secs(1),
            &mut |_| {},
        );
        assert_eq!(err, Some(false));
        let silent = answer(
            &lines(&["boot"]),
            "SYNC",
            Duration::from_millis(200),
            &mut |_| {},
        );
        assert_eq!(silent, None);
    }

    #[test]
    fn a_reset_names_its_path() {
        use clap::Parser as _;
        #[derive(clap::Parser)]
        struct Wrap {
            #[command(subcommand)]
            command: BoardCommand,
        }
        let parsed = Wrap::try_parse_from(["x", "reset", "chip-b", "--via", "jtag"]).unwrap();
        assert!(matches!(
            parsed.command,
            BoardCommand::Reset { via: Via::Jtag, .. }
        ));
        assert!(Wrap::try_parse_from(["x", "reset", "chip-b", "--via", "en"]).is_err());
        for via in ["rts", "jtag", "download", "power"] {
            assert!(
                Wrap::try_parse_from(["x", "reset", "chip-b", "--via", via]).is_ok(),
                "{via}"
            );
        }
        assert_eq!(4, <Via as clap::ValueEnum>::value_variants().len());
        assert!(Wrap::try_parse_from(["x", "reset", "chip-b", "--download"]).is_err());
    }
}
