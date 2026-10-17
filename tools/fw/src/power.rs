//! Power-cycling a board's hub port with `uhubctl`.
//!
//! Only a port the stand file lists as `switchable`, and not `protected`, is
//! switched, and only after the board's USB location in sysfs proves that it
//! hangs on that very port: a stand file that disagrees with the cabling
//! would otherwise cut the power of whatever else the port feeds.

use std::{
    process::Command,
    thread,
    time::{Duration, Instant},
};

use crate::{Result, boards::Attached, stand};

/// How long the port stays off.
const OFF: Duration = Duration::from_secs(2);
/// How long the board may take to leave and to return to USB.
const DETACH: Duration = Duration::from_secs(5);
const RETURN: Duration = Duration::from_secs(15);

/// The hub location and port to switch for `board`, which is `attached`.
pub fn port_of<'a>(
    stand: &'a stand::Stand,
    board: &stand::Board,
    attached: Option<&Attached>,
) -> std::result::Result<(&'a stand::Hub, u32), String> {
    let (hub, port) = stand.switchable_port(board)?;
    let attached = attached.ok_or_else(|| {
        format!(
            "board {} is not attached, so its hub port cannot be confirmed; not switching",
            board.id
        )
    })?;
    if attached.hub_port() != Some((hub.usb2.as_str(), port)) {
        return Err(format!(
            "board {} is attached at USB {}, but the stand file puts it on {} port {port} \
             ({}.{port}); fix the stand file, not switching",
            board.id, attached.location, hub.id, hub.usb2
        ));
    }
    Ok((hub, port))
}

fn uhubctl(location: &str, port: u32, action: &str) -> Result<()> {
    let output = Command::new("uhubctl")
        .args(["-l", location, "-p", &port.to_string(), "-a", action])
        .output()
        .map_err(|error| format!("run uhubctl: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "uhubctl -l {location} -p {port} -a {action} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(())
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

/// Switch `hub`'s `port` off and on again and wait for the board `mac` to
/// return to USB.
pub fn cycle(hub: &stand::Hub, port: u32, mac: &str) -> Result<Attached> {
    uhubctl(&hub.usb2, port, "off")?;
    let left = wait_until(DETACH, || crate::boards::find(mac).is_none());
    thread::sleep(OFF);
    uhubctl(&hub.usb2, port, "on")?;
    if !left {
        return Err(format!(
            "board {mac} stayed on USB while {} port {port} was off: the port does not \
             switch its power",
            hub.id
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

    const STAND: &str = r#"
schema = 1
[[hub]]
id = "mid"
usb2 = "3-8.3"
protected = [4]
switchable = [1, 2, 3]
[[hub]]
id = "bottom"
usb2 = "3-8.3.4"
switchable = [1, 2, 3, 4]
[[board]]
id = "s31-a"
usb-serial = "30:ED:A0:F3:F6:D0"
chip = "esp32s31"
port = { hub = "mid", port = 3 }
"#;

    fn attached(location: &str) -> Attached {
        Attached {
            location: location.to_owned(),
            mac: "30:ED:A0:F3:F6:D0".to_owned(),
            tty: None,
        }
    }

    #[test]
    fn switches_only_the_port_the_board_is_attached_to() {
        let stand = stand::Stand::parse(STAND).unwrap();
        let board = stand.board("s31-a").unwrap();
        let (hub, port) = port_of(&stand, board, Some(&attached("3-8.3.3"))).unwrap();
        assert_eq!((hub.id.as_str(), port), ("mid", 3));
        let error = port_of(&stand, board, Some(&attached("3-8.3.4.4"))).unwrap_err();
        assert!(error.contains("attached at USB 3-8.3.4.4"), "{error}");
        let error = port_of(&stand, board, None).unwrap_err();
        assert!(error.contains("not attached"), "{error}");
    }
}
