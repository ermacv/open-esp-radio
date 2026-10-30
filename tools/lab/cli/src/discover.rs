//! `cargo stand discover|doctor`: the stand file against the host
//! (`discovery`, `doctor`), and the hub port switches
//! of `discover --blink` and `--verify-power` under a lease.

use oer_lab_model::Stand;
mod discovery;
mod doctor;

use discovery as discover;

use crate::Result;
use oer_os_process::Checkout;

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
    cli: StandCli,
) -> Result<std::process::ExitCode> {
    let path = oer_lab_model::paths::stand_file()?;
    match cli {
        StandCli::Discover {
            blink: Some(target),
            ..
        } => {
            let stand = Stand::load(&path)?;
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
            let stand = Stand::load(&path)?;
            let after =
                verify_power(&stand, &board, owner()?).map_err(|why| format!("{board}: {why}"))?;
            println!(
                "{board} lost its power: it left USB, returned {:.1} s after its port's power \
                 and read a power-on reset",
                after.as_secs_f64()
            );
        }
        StandCli::Discover { .. } => {
            let stand = Stand::load(&path)?;
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
    stand: &'a oer_lab_model::Stand,
    target: &str,
    owner: String,
) -> Result<(&'a oer_lab_model::Hub, u8)> {
    let (hub, port) = discover::blink_target(stand, target)?;
    let mut claims = vec![oer_lab_arbiter::claims::Claim::exclusive(
        oer_lab_service::stand::hub_port_resource(&hub.usb2, port),
    )];
    if let Some(board) = stand.boards.iter().find(|board| {
        board
            .port
            .as_ref()
            .is_some_and(|place| place.hub == hub.id && place.port == u32::from(port))
    }) {
        claims.push(oer_lab_arbiter::claims::Claim::board(&board.device_id()?));
    }
    let mut lease = crate::service::lease(
        owner,
        format!("stand discover --blink {}:{port}", hub.id),
        claims,
    )?;
    crate::service::run(
        &mut lease,
        oer_lab_service::protocol::Operation::HubPort {
            hub: hub.id.clone(),
            port,
            off_secs: discover::BLINK.as_secs(),
        },
    )?;
    Ok((hub, port))
}

/// Cycle the hub port of the board `query` names under `owner`'s lease of
/// the board, proving the board lost its power.
fn verify_power(
    stand: &oer_lab_model::Stand,
    query: &str,
    owner: String,
) -> Result<std::time::Duration> {
    let board = stand.resolve(query)?;
    let mut lease = crate::service::lease(
        owner,
        format!("stand discover --verify-power {query}"),
        vec![oer_lab_arbiter::claims::Claim::board(&board.device_id()?)],
    )?;
    let returned = crate::service::run(
        &mut lease,
        oer_lab_service::protocol::Operation::ProvePowerLoss {
            board: board.mac.clone(),
        },
    )?;
    Ok(std::time::Duration::from_millis(
        returned
            .as_u64()
            .ok_or("the service returned no duration")?,
    ))
}
