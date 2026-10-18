//! Power-cycling a board's hub port with `uhubctl`.
//!
//! The port is found on the host ([`oer_tool_service_lab::hub`]): the board's USB location
//! names its hub and port, `uhubctl` confirms the hub switches power per
//! port, and the port is switched only when the board is the one device
//! below it. After switching off, the board must have left USB, or the port
//! did not cut its power.

use std::{
    path::Path,
    thread,
    time::{Duration, Instant},
};

use oer_tool_data_stand_file as stand;
use oer_tool_service_lab::hub;

use crate::{Result, boards::Attached};

/// How long the port stays off.
const OFF: Duration = Duration::from_secs(2);
/// How long the board may take to leave and to return to USB.
const DETACH: Duration = Duration::from_secs(5);
const RETURN: Duration = Duration::from_secs(15);

/// The hub port that powers `board`, `attached` at its USB location, and may
/// be switched off, or why not.
pub fn port_of(
    stand: Option<&stand::Stand>,
    board_id: &str,
    attached: Option<&Attached>,
    report: &[hub::HubStatus],
    devices: &Path,
) -> std::result::Result<hub::PowerPort, String> {
    let attached = attached.ok_or_else(|| {
        format!("board {board_id} is not attached, so its hub port cannot be found; not switching")
    })?;
    hub::power_port(devices, report, stand, &attached.location)
}

fn wait_until(within: Duration, mut done: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        if done() {
            return true;
        }
        thread::sleep(Duration::from_millis(100));
    }
    done()
}

/// Switch `port` off and on again and wait for the board `mac` to leave and
/// return to USB.
pub fn cycle(port: &hub::PowerPort, mac: &str) -> Result<Attached> {
    hub::switch(&port.hub, port.port, false)?;
    let left = wait_until(DETACH, || crate::boards::find(mac).is_none());
    thread::sleep(OFF);
    hub::switch(&port.hub, port.port, true)?;
    if !left {
        return Err(format!(
            "{port} did not cut power: the board stayed on USB while the port was off"
        )
        .into());
    }
    let mut returned = None;
    wait_until(RETURN, || {
        returned = crate::boards::find(mac).filter(|board| board.tty.is_some());
        returned.is_some()
    });
    returned.ok_or_else(|| {
        format!(
            "board {mac} did not return to USB within {}s after power-on",
            RETURN.as_secs()
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_board_that_is_not_attached_is_not_switched() {
        let error = port_of(None, "s31-a", None, &[], Path::new("/nonexistent")).unwrap_err();
        assert!(error.contains("not attached"), "{error}");
    }
}
