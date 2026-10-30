//! The device under test during the cases: its link and the board that
//! resets it. A reset closes the link, takes the first rung of the board's
//! ladder and opens the console again, so the cases see a new boot of the
//! same image. The new console answers also a device the rung did not
//! restart, so the reset counts only when another boot answers.

use std::time::Duration;

use oer_hil_link::{Boot, Link};

use crate::finish::Board;

/// How long a reset boot may take to say Hello.
const RESET_HELLO: Duration = Duration::from_secs(10);

pub(crate) struct Dut<'a> {
    link: Option<Link>,
    board: &'a dyn Board,
    image_crc32: u32,
    hello: Duration,
}

impl<'a> Dut<'a> {
    pub(crate) fn new(link: Option<Link>, board: &'a dyn Board, image_crc32: u32) -> Self {
        Self {
            link,
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
}

impl Device for Dut<'_> {
    fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    fn reset(&mut self) -> Result<Boot, String> {
        let previous = self.link.as_ref().and_then(Link::running_boot_id);
        // The rung needs the console the link holds.
        self.link = None;
        self.board.reset()?;
        let link = self.board.connect()?;
        let boot = link
            .boot(self.hello)
            .map_err(|error| format!("after the reset: {error}"))?;
        if previous == Some(boot.boot_id) {
            // The device runs on: its link serves the cases that follow.
            self.link = Some(link);
            return Err(format!(
                "the reset did not restart the device: boot {:016x} answered again",
                boot.boot_id
            ));
        }
        if boot.hello.image_crc32 != self.image_crc32 {
            return Err(format!(
                "after the reset the board runs image {:08x}, not the built {:08x}",
                boot.hello.image_crc32, self.image_crc32
            ));
        }
        self.link = Some(link);
        Ok(boot)
    }
}
#[cfg(test)]
mod tests;
