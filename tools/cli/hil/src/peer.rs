//! `cargo hil peer send`: one command of a reference peer's text protocol
//! and its answer, under a lease of the peer board and the air (a peer
//! command may transmit).

use std::{
    ffi::OsString,
    io::Write as _,
    sync::mpsc,
    time::{Duration, Instant},
};

use crate::Result;
use oer_process::Checkout;

fn parse_duration(text: &str) -> std::result::Result<Duration, String> {
    oer_stand_arbiter::parse_duration(text).map_err(|error| error.to_string())
}

/// `cargo hil peer send BOARD LINE`.
#[derive(Debug, clap::Parser)]
#[command(name = "cargo hil peer", no_binary_name = true)]
pub(crate) enum PeerCli {
    /// Send one line to a peer's text protocol and print its answer, up to
    /// its `@OK` or `@ERR` line.
    Send {
        #[arg(value_name = "NAME|MAC")]
        board: String,
        /// The command line, e.g. `SYNC` or `CFG 15 4f45 ...`.
        line: Vec<String>,
        /// How long to wait for the answer.
        #[arg(long = "for", default_value = "5s", value_parser = parse_duration)]
        duration: Duration,
    },
}

pub(crate) fn peer(
    ctx: &Checkout,
    owner: String,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let PeerCli::Send {
        board,
        line,
        duration,
    } = PeerCli::try_parse_from(args)?;
    let line = line.join(" ");
    if line.is_empty() {
        return Err("cargo hil peer send needs a command line".into());
    }
    let arbiter = oer_stand_arbiter::Arbiter::open()?;
    let target = oer_stand_board::Board::attached(&ctx.root, &arbiter.stand()?, &board)?;
    // The grant holds the board's device access until it is dropped; the
    // leased board and its console keep it while they live.
    let grant = arbiter.acquire(&oer_stand_arbiter::Request {
        owner,
        work: format!("peer send {board} {line}"),
        scenarios: Vec::new(),
        run: None,
        claims: vec![
            oer_stand_claims::Claim::board(target.mac()),
            oer_stand_claims::Claim::shared(oer_stand_claims::AIR),
        ],
    })?;
    let access = grant
        .device(target.mac())
        .ok_or("the lease holds no access to the board")?;
    let mut serial = target.lease(access)?.open_console()?;
    // A fresh line ends whatever the peer's parser held.
    serial.write_all(format!("\n{line}\n").as_bytes())?;
    let lines = serial.lines();
    let command = line.split_whitespace().next().unwrap_or_default();
    let answered = answer(&lines, command, duration, &mut |text| println!("{text}"));
    Ok(match answered {
        Some(true) => std::process::ExitCode::SUCCESS,
        Some(false) | None => std::process::ExitCode::FAILURE,
    })
}

/// Print `lines` until the `@OK` (true) or `@ERR` (false) answer to
/// `command` in the peers' one grammar (`oer-device-peer-line`), or `None`
/// after `duration`. An answer naming another command, such as noise a peer
/// read while it booted, is only printed.
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
                match oer_device_peer_line::parse(text.trim()) {
                    Some(oer_device_peer_line::Line::Ok { command: about })
                        if about.eq_ignore_ascii_case(command) =>
                    {
                        return Some(true);
                    }
                    Some(oer_device_peer_line::Line::Err { command: about, .. })
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
