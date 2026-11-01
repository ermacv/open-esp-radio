//! One board of the stand as the host reaches it: its stand-file reference,
//! where it was last seen, its chip, its reset ladder and its hub power,
//! and every operation on it (a write, a start, a reset, a console, its
//! power). Only the LAB service, the one owner of the stand's hardware,
//! runs them.
//!
//! A [`Board`] needs only the stand file and the host: its hub port's
//! power, its console without a reset and where it is. Every operation that
//! depends on its chip (a write, an RTS, JTAG or download reset, a console
//! opened by a reset, the reset ladder, the proof of power loss) belongs to
//! a [`ProfiledBoard`], which only a chip with a profile yields.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use crate::image::{Receipt, Store, Transport};
use oer_espressif_facts::Start;
use oer_lab_model::device_id::DeviceId;
use oer_lab_model::{BoardRef, ResetStep, Stand};

use crate::console;
use crate::openocd::Openocd;
use crate::port::{Port, retrying};
use crate::reset;
use oer_lab_model::rom::{RecoveryStep, ResetPath, Rung};

use crate::power::{HubPower, PowerCycle, PowerLoss, ResetCauseField};

/// How often, how long and how far apart the reset cause is read after a
/// power cycle.
const RESET_CAUSE_ATTEMPTS: u32 = 5;
const RESET_CAUSE_READ: std::time::Duration = std::time::Duration::from_secs(20);
const RESET_CAUSE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

/// How long a reset's console is read for the ROM's line.
const BANNER: Duration = Duration::from_secs(2);
/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How long a returned port may take until the stand's user may open it.
const PORT_ACCESS: Duration = Duration::from_secs(3);

/// How a write reaches the board's flash.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Via {
    /// espflash and the USB Serial/JTAG reset lines.
    Usb,
    /// OpenOCD and the chip's debug module, which works over any running
    /// image; it resets into the written image.
    Jtag,
}

/// A board of the stand file: what it is, where it was last seen, its reset
/// ladder and its hub power; the operations that need no chip profile.
#[derive(Clone, Debug)]
pub struct Board {
    pub reference: BoardRef,
    /// Where the board was last seen; a reset or power cycle can move it.
    port: PathBuf,
    reset: Vec<ResetStep>,
    power: Option<HubPower>,
}

/// A board whose chip has a profile: the operations of its [`Board`] and
/// those that depend on its chip.
#[derive(Clone, Debug)]
pub struct ProfiledBoard {
    board: Board,
    /// The chip as espflash names it.
    espflash_chip: String,
    /// Where the chip holds why it last reset, which a power cycle reads.
    reset_cause: Option<ResetCauseField>,
}

impl std::ops::Deref for ProfiledBoard {
    type Target = Board;

    fn deref(&self) -> &Board {
        &self.board
    }
}

impl Board {
    /// `board` of `stand`, last seen at `port`, with the hub port that
    /// powers it when its reset ladder includes `power`. No chip profile is
    /// read; [`Self::profiled`] reads it.
    pub fn new(
        stand: &Stand,
        board: &oer_lab_model::Board,
        port: PathBuf,
    ) -> crate::stand::Result<Self> {
        Self::with_power(
            board,
            port,
            crate::power::hub_port(stand, board).map(HubPower::new),
        )
    }

    fn with_power(
        board: &oer_lab_model::Board,
        port: PathBuf,
        power: Option<HubPower>,
    ) -> crate::stand::Result<Self> {
        Ok(Self {
            reference: board.reference()?,
            port,
            reset: board.reset.clone(),
            power,
        })
    }

    /// The attached board of `stand` that `query` names (an id, a chip with
    /// one board, or a MAC), at its `/dev/serial/by-id` port.
    pub fn attached(stand: &Stand, query: &str) -> crate::stand::Result<Self> {
        let board = stand.resolve(query)?;
        let port = crate::discovery::port_of(&board.device_id()?)?;
        Self::new(stand, board, port)
    }

    /// The board with its chip's profile from the repository at `root`;
    /// refused for a chip without one, naming what still works.
    pub fn profiled(self, root: &Path) -> crate::stand::Result<ProfiledBoard> {
        let profile =
            oer_chip_profile::Profile::load(root, &self.reference.chip).map_err(|error| {
                format!(
                    "board `{}`: {error}; without a chip profile only its hub port's power, \
                     its console without a reset and its description work",
                    self.reference.id
                )
            })?;
        let reset_cause = profile
            .facts
            .reset_cause
            .as_ref()
            .map(|field| ResetCauseField::resolve(root, &profile.facts.id, field))
            .transpose()?;
        Ok(ProfiledBoard {
            board: self,
            espflash_chip: profile.facts.espflash_chip,
            reset_cause,
        })
    }

    pub fn mac(&self) -> &DeviceId {
        &self.reference.mac
    }

    pub fn chip(&self) -> &str {
        &self.reference.chip
    }

    /// Where the board was last seen.
    pub fn port(&self) -> &Path {
        &self.port
    }

    /// Whether the board resets by its hub port's power.
    pub fn has_power(&self) -> bool {
        self.power.is_some()
    }
}

/// A board's open console: it reads and writes the port
/// ([`std::io::Read`], [`std::io::Write`]), or becomes its lines
/// ([`BoardConsole::lines`]).
#[derive(Debug)]
pub struct BoardConsole {
    port: Port,
}

impl std::io::Read for BoardConsole {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        std::io::Read::read(&mut self.port, buffer)
    }
}

impl std::io::Write for BoardConsole {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        std::io::Write::write(&mut self.port, bytes)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.port)
    }
}

impl BoardConsole {
    /// The open port.
    pub fn into_port(self) -> Port {
        self.port
    }

    /// The lines the console receives.
    pub fn lines(self) -> BoardLines {
        BoardLines {
            lines: console::lines(self.port),
        }
    }
}

/// A board's console lines.
pub struct BoardLines {
    lines: console::Lines,
}

impl std::ops::Deref for BoardLines {
    type Target = console::Lines;

    fn deref(&self) -> &console::Lines {
        &self.lines
    }
}

impl Board {
    /// The board's port now: where it was while that exists, else its port
    /// once it is attached again within `within`.
    pub fn current_port(&self, within: Duration) -> Option<PathBuf> {
        if self.port.exists() {
            return Some(self.port.clone());
        }
        crate::discovery::wait_for(self.mac(), within)
    }

    /// The receipt of `bundle` when the board runs it, by the devices
    /// layer's receipts in `store` (never by the stand's journal), read
    /// once: the basis of a skipped flash.
    pub fn carries(
        &self,
        store: &Store,
        bundle: &oer_image_bundle::ImageBundle,
    ) -> crate::stand::Result<Option<Receipt>> {
        crate::image::carries(store, self.mac(), bundle)
    }

    /// The board's console read for `watch` without resetting it, once its
    /// port is back.
    pub fn console(&self, watch: Duration) -> String {
        self.current_port(REATTACH)
            .and_then(|port| reset::open_without_reset(&port).ok())
            .map(|serial| console::read_for(serial, watch))
            .unwrap_or_default()
    }

    /// Open the board's console without a reset, retrying while the port is
    /// still held by the reader of a previous reset or returning after one.
    pub fn open_console(&self) -> crate::stand::Result<BoardConsole> {
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port is gone")?;
        let port = retrying(PORT_ACCESS, || reset::open_without_reset(&port))?;
        Ok(BoardConsole { port })
    }

    fn require_power(&self) -> crate::stand::Result<&HubPower> {
        self.power.as_ref().ok_or_else(|| {
            "the board does not reset by power; add `power` to its `reset` in the stand file".into()
        })
    }

    /// Power the board off and on, watching it leave USB and return.
    pub fn power_cycle_observed(&self) -> crate::stand::Result<PowerCycle> {
        self.require_power()?.cycle_observed(self.mac())
    }

    /// Power the board off and on and wait until its port is back and open
    /// to the stand's user: a power-on reset into the image in its flash.
    pub fn power_on(&self) -> crate::stand::Result<()> {
        self.power_cycle_observed()?.verdict()?;
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port did not return after its power")?;
        // udev hands the returned port to the stand's group a moment after
        // it appears.
        retrying(PORT_ACCESS, || reset::open_without_reset(&port))
            .map(drop)
            .map_err(|_| format!("{} stayed closed after its power", port.display()).into())
    }

    /// Cycle the power of the board's hub port: the one reset that needs no
    /// chip profile. A power-on reset re-enumerates the port, so the ROM's
    /// reset line is read from the console once the port is back.
    pub fn power_reset(&self) -> crate::stand::Result<Option<String>> {
        Power(self).reset()?;
        Ok(oer_lab_model::rom::reset_line(&self.console(BANNER)))
    }
}

impl ProfiledBoard {
    /// Write `bundle`, known as `image`, into the board's flash through
    /// `via` by the one write operation (`crate::image`, which publishes
    /// the receipt into `store`), start it and confirm the start; `by` is
    /// the run or command that writes. A USB write starts the image as the
    /// bundle's start policy says; OpenOCD's program resets into it. A board
    /// that is not on USB, because its image switched its USB Serial/JTAG
    /// off, is put into its ROM's download mode first through its hub
    /// port's power.
    pub fn flash(
        &self,
        store: &Store,
        bundle: &oer_image_bundle::ImageBundle,
        image: &str,
        via: Via,
        by: &str,
    ) -> crate::stand::Result<Receipt> {
        let reference = &self.reference;
        if bundle.chip != reference.chip {
            return Err(format!(
                "board `{}` is an {}; the bundle is an {} image",
                reference.id, reference.chip, bundle.chip
            )
            .into());
        }
        let written = match via {
            Via::Usb => {
                let port = self.flashable_port()?;
                crate::image::write(
                    store,
                    self.mac(),
                    bundle,
                    image,
                    Transport::Usb {
                        port: &port,
                        espflash_chip: &self.espflash_chip,
                    },
                    by,
                )?
            }
            Via::Jtag => crate::image::write(
                store,
                self.mac(),
                bundle,
                image,
                Transport::Jtag { chip: self.chip() },
                by,
            )?,
        };
        self.start(written.pending_start())?;
        written.started()
    }

    /// Start the image a write left as `start` needs: nothing after the
    /// writer's own reset, else a power-on reset where the board resets by
    /// power and an RTS reset where it does not.
    fn start(&self, start: Start) -> crate::stand::Result<()> {
        match (start, &self.power) {
            (Start::Reset, _) => Ok(()),
            (Start::PowerOn, Some(_)) => self.power_on(),
            (Start::PowerOn, None) => {
                let port = self
                    .current_port(REATTACH)
                    .ok_or("the board's port is gone")?;
                drop(reset::reset_into_application(&port)?);
                Ok(())
            }
        }
    }

    /// The port a write goes through: the board's port, or, when the board
    /// is not on USB, its port after a download entry.
    fn flashable_port(&self) -> crate::stand::Result<PathBuf> {
        if let Some(port) = self.current_port(Duration::ZERO) {
            return Ok(port);
        }
        if self.power.is_none() {
            return Err(format!(
                "board `{}` is not on USB and does not reset by power; a person must reset it",
                self.reference.id
            )
            .into());
        }
        let banner = self.download_entry()?;
        eprintln!(
            "hil: board `{}` was not on USB; its ROM now waits for the flash: {}",
            self.reference.id,
            oer_lab_model::rom::reset_line(&banner)
                .as_deref()
                .unwrap_or("no reset line")
        );
        self.current_port(REATTACH)
            .ok_or_else(|| "the board's port did not return after its download entry".into())
    }

    /// The rungs of the board's reset ladder, in the stand file's order.
    pub fn rungs(&self) -> Vec<Box<dyn Rung + '_>> {
        self.reset
            .iter()
            .filter_map(|step| -> Option<Box<dyn Rung + '_>> {
                match step {
                    ResetStep::UsbJtagRts => Some(Box::new(Rts(self))),
                    ResetStep::Jtag => Some(Box::new(Jtag(self))),
                    ResetStep::Power => self.power.as_ref().map(|_| Box::new(Power(self)) as _),
                }
            })
            .collect()
    }

    /// Reset the board through `path` and return the ROM's reset line it
    /// printed: the rung of that kind, whether or not the board's ladder
    /// lists it.
    pub fn reset(&self, path: ResetPath) -> crate::stand::Result<Option<String>> {
        let banner = match path {
            ResetPath::Rts => Rts(self).reset()?,
            ResetPath::Jtag => Jtag(self).reset()?,
            ResetPath::Power => return self.power_reset(),
            ResetPath::Download => Some(self.download_entry()?),
        };
        // A chip whose USB Serial/JTAG port re-enumerates prints its banner
        // before the port returns: read the console again once it is back.
        Ok(banner
            .as_deref()
            .and_then(oer_lab_model::rom::reset_line)
            .or_else(|| oer_lab_model::rom::reset_line(&self.console(BANNER))))
    }

    /// Reset the board through the first reset its stand file lists, into
    /// its image, and return the ROM's reset line it printed.
    pub fn restart(&self) -> crate::stand::Result<Option<String>> {
        let rungs = self.rungs();
        let rung = rungs
            .first()
            .ok_or("the board's stand file lists no reset")?;
        let banner = rung.reset()?;
        Ok(banner
            .as_deref()
            .and_then(oer_lab_model::rom::reset_line)
            .or_else(|| oer_lab_model::rom::reset_line(&self.console(BANNER))))
    }

    /// Reset the board into its flashed application over RTS and return
    /// its console, open from the reset on, so no line of the boot is
    /// missed.
    pub fn reset_into_console(&self) -> crate::stand::Result<BoardConsole> {
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port is gone")?;
        let port = retrying(PORT_ACCESS, || reset::reset_into_application(&port))?;
        Ok(BoardConsole { port })
    }

    /// Open the board's console and reset it into its image over its USB
    /// Serial/JTAG, which discards what the port held before.
    pub fn open_console_from_reset(&self) -> crate::stand::Result<BoardConsole> {
        let mut port = self.open_console()?.into_port();
        reset::reset_usb_serial_jtag(&mut port)?;
        Ok(BoardConsole { port })
    }

    /// Power the board off and on, watching it leave USB and return, then
    /// read through its JTAG why it last reset: a power-on reset proves that
    /// its port cuts its power. The read halts the core for a moment and
    /// lets it run on; the image in flash must leave the chip's JTAG on and
    /// not reset again, as the verification of a port arranges.
    pub fn prove_power_loss(&self) -> crate::stand::Result<PowerLoss> {
        let field = self.reset_cause.as_ref().ok_or_else(|| {
            format!(
                "the {} profile names no reset cause, so a power cycle cannot be proven",
                self.chip()
            )
        })?;
        self.require_power()?.prove_power_loss(self.mac(), || {
            let openocd = Openocd::locate()?;
            let mut last = None;
            // The USB-JTAG answers a moment after the board's USB returns.
            for _ in 0..RESET_CAUSE_ATTEMPTS {
                match openocd.read_word(
                    self.chip(),
                    self.mac().as_ref(),
                    field.address,
                    RESET_CAUSE_READ,
                ) {
                    Ok(word) => return Ok(field.cause(word)),
                    Err(error) => last = Some(error),
                }
                oer_os_process::sleep(RESET_CAUSE_RETRY)?;
            }
            Err(last.unwrap_or_else(|| "no attempt was made".into()))
        })
    }

    /// Power the board off and on and, as soon as its USB returns, reset it
    /// into its ROM's download mode through the USB Serial/JTAG; the console
    /// the ROM printed. Only a board that resets by power has it.
    pub fn download_entry(&self) -> crate::stand::Result<String> {
        self.power_cycle_observed()?.verdict()?;
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port did not return after its power")?;
        // udev hands the returned port to the stand's group a moment after
        // it appears: retry the open briefly, then reset at once.
        let serial = retrying(PORT_ACCESS, || reset::reset_into_download(&port))?;
        Ok(console::read_for(serial, BANNER))
    }

    /// [`Self::download_entry`] when the board resets by power.
    pub fn download_entry_if_powered(
        &self,
    ) -> Option<impl Fn() -> crate::stand::Result<String> + '_> {
        self.power.as_ref().map(|_| move || self.download_entry())
    }
}

struct Rts<'a>(&'a Board);

impl Rung for Rts<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::RtsReset
    }

    fn reset(&self) -> crate::stand::Result<Option<String>> {
        let port = self
            .0
            .current_port(Duration::from_secs(5))
            .ok_or("the board's port is gone")?;
        let serial = retrying(PORT_ACCESS, || reset::reset_into_application(&port))?;
        Ok(Some(console::read_for(serial, BANNER)))
    }
}

struct Jtag<'a>(&'a Board);

impl Rung for Jtag<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::JtagReset
    }

    /// The console is opened first, without touching the reset lines, so
    /// the read starts before the reset's banner.
    fn reset(&self) -> crate::stand::Result<Option<String>> {
        let openocd = Openocd::locate()?;
        let serial = self
            .0
            .current_port(Duration::ZERO)
            .and_then(|port| reset::open_without_reset(&port).ok());
        openocd.reset(self.0.chip(), self.0.mac())?;
        Ok(serial.map(|serial| console::read_for(serial, BANNER)))
    }
}

struct Power<'a>(&'a Board);

impl Rung for Power<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::PowerCycle
    }

    fn reset(&self) -> crate::stand::Result<Option<String>> {
        self.0.require_power()?.cycle()?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests;
