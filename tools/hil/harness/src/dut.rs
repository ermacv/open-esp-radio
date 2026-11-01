//! The device under test during the cases: its link and the board that
//! resets it. A reset closes the link, takes the first rung of the board's
//! ladder and opens the console again, so the cases see a new boot of the
//! same image. The new console answers also a device the rung did not
//! restart, so the reset counts only when another boot answers than the
//! last one seen, which outlives a link a failed reset dropped.

use std::time::Duration;

use oer_hil_link::{Boot, Link};

use crate::finish::Board;

/// How long a reset boot may take to say Hello.
const RESET_HELLO: Duration = Duration::from_secs(10);

pub(crate) struct Dut<'a> {
    link: Option<Link>,
    /// The last boot seen while no link shows one.
    seen: Option<u64>,
    board: &'a dyn Board,
    image_crc32: u32,
    hello: Duration,
}

impl<'a> Dut<'a> {
    pub(crate) fn new(link: Option<Link>, board: &'a dyn Board, image_crc32: u32) -> Self {
        Self {
            link,
            seen: None,
            board,
            image_crc32,
            hello: RESET_HELLO,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_hello(mut self, hello: Duration) -> Self {
        self.hello = hello;
        self
    }

    pub(crate) fn image_crc32(&self) -> u32 {
        self.image_crc32
    }

    /// The last boot seen, also after a failed reset dropped its link.
    pub(crate) fn last_boot(&self) -> Option<u64> {
        self.link
            .as_ref()
            .and_then(Link::running_boot_id)
            .or(self.seen)
    }

    pub(crate) fn into_link(self) -> Option<Link> {
        self.link
    }
}

/// What a case's context needs of the device under test.
pub(crate) trait Device {
    /// The link, while the device answers on one.
    fn link(&self) -> Option<&Link>;
    /// Reset the device and return its new boot of the built image.
    fn reset(&mut self) -> Result<Boot, String>;
    /// Return the new boot of the built image the device started by itself,
    /// without a reset of the board.
    fn follow_restart(&mut self) -> Result<Boot, String>;
    /// Run `request` on the lab router under the run's lease.
    fn router(
        &self,
        request: oer_lab_client::protocol::RouterOperation,
    ) -> Result<serde_json::Value, String>;
}

impl Device for Dut<'_> {
    fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    fn reset(&mut self) -> Result<Boot, String> {
        let previous = self.last_boot();
        self.seen = previous;
        // The rung needs the console the link holds.
        self.link = None;
        self.board.reset()?;
        self.reconnect(previous, "the reset")
    }

    fn follow_restart(&mut self) -> Result<Boot, String> {
        let previous = self.last_boot();
        self.seen = previous;
        // The console opens again on the device's new boot.
        self.link = None;
        self.reconnect(previous, "the device's restart")
    }

    fn router(
        &self,
        request: oer_lab_client::protocol::RouterOperation,
    ) -> Result<serde_json::Value, String> {
        self.board.router(request)
    }
}

impl Dut<'_> {
    /// Open the console again and take the boot that answers: it counts only
    /// when another boot answers than `previous` and runs the built image.
    fn reconnect(&mut self, previous: Option<u64>, what: &str) -> Result<Boot, String> {
        let link = self.board.connect()?;
        let answered = link.boot(self.hello);
        // A frame names its boot also when the boot's Hello does not come.
        self.seen = link.running_boot_id().or(previous);
        let boot = answered.map_err(|error| format!("after {what}: {error}"))?;
        if previous == Some(boot.boot_id) {
            // The device runs on: its link serves the cases that follow.
            self.link = Some(link);
            return Err(format!(
                "{what} did not restart the device: boot {:016x} answered again",
                boot.boot_id
            ));
        }
        if boot.hello.image_crc32 != self.image_crc32 {
            return Err(format!(
                "after {what} the board runs image {:08x}, not the built {:08x}",
                boot.hello.image_crc32, self.image_crc32
            ));
        }
        self.link = Some(link);
        Ok(boot)
    }
}
#[cfg(test)]
mod tests;
