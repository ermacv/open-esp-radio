//! `cargo stand discover|doctor`: the stand file against the host
//! (`oer-stand-discover`, `oer-stand-doctor`), and the hub port switches
//! of `discover --blink` and `--verify-power` under a lease.

use std::ffi::OsString;

use oer_stand_discover as discover;
use oer_stand_doctor as doctor;
use oer_stand_file::StandFile;

use crate::Result;
use oer_process::Checkout;

#[derive(clap::Parser)]
#[command(name = "cargo stand", no_binary_name = true)]
pub(crate) enum StandCli {
    /// Map each attached board to its hub port and button and compare the
    /// result with the stand file, which it never changes.
    Discover {
        /// Switch HUB:PORT (a stand-file hub id and port) off for five
        /// seconds under a lease, to see which button it is.
        #[arg(long, value_name = "HUB:PORT", conflicts_with = "verify_power")]
        blink: Option<String>,
        /// Cycle BOARD's hub port under its lease and require the board to
        /// leave USB while the port is off and to return once it is on.
        #[arg(long, value_name = "BOARD")]
        verify_power: Option<String>,
    },
    /// Check the host around the stand file: the file itself, `uhubctl`
    /// without sudo and NetworkManager leaving `wlan0` alone.
    Doctor,
}

pub(crate) fn stand(
    ctx: &Checkout,
    owner: impl FnOnce() -> Result<String>,
    args: &[OsString],
) -> Result<std::process::ExitCode> {
    use clap::Parser as _;
    let path = oer_stand_file::paths::stand_file()?;
    match StandCli::try_parse_from(args)? {
        StandCli::Discover {
            blink: Some(target),
            ..
        } => {
            let stand = StandFile::load(&path)?;
            let (hub, port) = blink(&stand, &target, owner()?)?;
            println!(
                "{}:{port} was off for {} s",
                hub.id,
                discover::BLINK.as_secs()
            );
        }
        StandCli::Discover {
            verify_power: Some(board),
            ..
        } => {
            // The ROM prints its reset reason before the board's own USB
            // enumerates; the board leaving USB, returning and reading a
            // power-on reset through its JTAG is the proof.
            let stand = StandFile::load(&path)?;
            let after = verify_power(&ctx.root, &stand, &board, owner()?)?
                .verdict()
                .map_err(|why| format!("{board}: {why}"))?;
            println!(
                "{board} lost its power: it left USB, returned {:.1} s after its port's power \
                 and read a power-on reset",
                after.as_secs_f64()
            );
        }
        StandCli::Discover { .. } => {
            let stand = StandFile::load(&path)?;
            print!("{}", discover::describe(&stand, &discover::report(&stand)?));
        }
        StandCli::Doctor => {
            let mut failed = false;
            for check in doctor::doctor(&ctx.root, &path) {
                match &check.failure {
                    None => println!("PASS {}", check.name),
                    Some(failure) => {
                        failed = true;
                        println!("FAIL {}: {failure}", check.name);
                    }
                }
            }
            if failed {
                return Ok(std::process::ExitCode::FAILURE);
            }
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Switch the port `target` (`HUB:PORT`) of `stand` off for [`discover::BLINK`]
/// under `owner`'s lease of the port and of the board on it, to see which
/// button it is.
fn blink<'a>(
    stand: &'a oer_stand_file::StandFile,
    target: &str,
    owner: String,
) -> Result<(&'a oer_stand_file::Hub, u8)> {
    let (hub, port) = discover::blink_target(stand, target)?;
    let arbiter = oer_stand_arbiter::Arbiter::open()?;
    let mut claims = vec![oer_stand_claims::Claim::exclusive(format!(
        "hub-port:{}:{port}",
        hub.usb2
    ))];
    if let Some(board) = stand.board.iter().find(|board| {
        board
            .port
            .as_ref()
            .is_some_and(|place| place.hub == hub.id && place.port == port)
    }) {
        claims.push(oer_stand_claims::Claim::board(&board.mac()?));
    }
    let grant = arbiter.acquire(&oer_stand_arbiter::Request {
        owner,
        work: format!("stand discover --blink {}:{port}", hub.id),
        scenarios: Vec::new(),
        run: None,
        claims,
    })?;
    let operation = stand
        .board
        .iter()
        .find_map(|board| {
            let mac = board.mac().ok()?;
            grant
                .device(&mac)
                .map(oer_device_lock::DeviceAccess::operation)
        })
        .transpose()?;
    // A newly discovered port may have no board identity yet.
    let no_device = oer_process::IoLifetime::default();
    let lifetime = operation
        .as_ref()
        .map_or(&no_device, oer_device_lock::DeviceOperation::lifetime);
    oer_stand_power::HubPower::new(oer_stand_file::HubPort {
        location: hub.usb2.clone(),
        port,
    })
    .cycle_holding(discover::BLINK, lifetime)?;
    Ok((hub, port))
}

/// Cycle the hub port of the board `query` names under `owner`'s lease of
/// the board, proving the board lost its power.
fn verify_power(
    root: &std::path::Path,
    stand: &oer_stand_file::StandFile,
    query: &str,
    owner: String,
) -> Result<oer_stand_power::PowerLoss> {
    let board = oer_stand_board::Board::attached(root, stand, query)?;
    let arbiter = oer_stand_arbiter::Arbiter::open()?;
    // The grant holds the board's device access through the power cycle.
    let grant = arbiter.acquire(&oer_stand_arbiter::Request {
        owner,
        work: format!("stand discover --verify-power {query}"),
        scenarios: Vec::new(),
        run: None,
        claims: vec![oer_stand_claims::Claim::board(board.mac())],
    })?;
    let access = grant
        .device(board.mac())
        .ok_or("the lease holds no access to the board")?;
    board.lease(access)?.prove_power_loss()
}
