//! One board of the stand as the host reaches it. [`Board`] describes it:
//! its stand-file reference, where it was last seen, its chip, its reset
//! ladder and its hub power. Every operation on it (a write, a start, a
//! reset, a console, its power) is a [`LeasedBoard`]'s, which holds the
//! board's [`DeviceAccess`]: no I/O without proof that this process owns
//! the board or was delegated it.

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use oer_chip_profile::Start;
use oer_device_lock::{DeviceAccess, DeviceId};
use oer_devices::image::{Receipt, Store, Transport};
use oer_stand_file::{BoardRef, ResetStep, StandFile};

use oer_devices::console;
use oer_devices::openocd::Openocd;
use oer_devices::port::{Port, retrying};
use oer_devices::reset;
use oer_devices::reset::{RecoveryStep, ResetPath, Rung};

use oer_stand_power::{HubPower, PowerCycle, PowerLoss, ResetCauseField};

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

/// A board of the stand file: what it is and where it was last seen. It
/// does no I/O; [`Board::lease`] makes the [`LeasedBoard`] that does.
#[derive(Clone, Debug)]
pub struct Board {
    pub reference: BoardRef,
    /// Where the board was last seen; a reset or power cycle can move it.
    port: PathBuf,
    /// The chip as espflash names it.
    espflash_chip: String,
    reset: Vec<ResetStep>,
    power: Option<HubPower>,
    /// Where the chip holds why it last reset, which a power cycle reads.
    reset_cause: Option<ResetCauseField>,
}

impl Board {
    /// `board` of `stand`, last seen at `port`, with its chip's profile from
    /// the repository at `root`.
    pub fn new(
        root: &Path,
        stand: &StandFile,
        board: &oer_stand_file::Board,
        port: PathBuf,
    ) -> crate::Result<Self> {
        let profile = oer_chip_profile::Profile::load(root, &board.chip)?;
        let reset_cause = profile
            .reset_cause
            .as_ref()
            .map(|field| ResetCauseField::resolve(root, &profile.id, field))
            .transpose()?;
        Ok(Self {
            reference: board.reference()?,
            port,
            espflash_chip: profile.espflash_chip,
            reset: board.reset.clone(),
            power: stand.hub_port(board).map(HubPower::new),
            reset_cause,
        })
    }

    /// The attached board of `stand` that `query` names (an id, a chip with
    /// one board, or a MAC), at its `/dev/serial/by-id` port.
    pub fn attached(root: &Path, stand: &StandFile, query: &str) -> crate::Result<Self> {
        let board = stand.resolve(query)?;
        let port = oer_devices::discovery::port_of(&board.mac()?)?;
        Self::new(root, stand, board, port)
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

    /// The board under `access`, which must be this board's and still
    /// exclude every other process: the only way to its I/O. The leased
    /// board keeps `access` (an owning guard's clone, or the delegation)
    /// for as long as it, or a console it opened, lives.
    pub fn lease(self, access: &DeviceAccess) -> crate::Result<LeasedBoard> {
        access.ensure_covers(self.mac())?;
        let _operation = access.operation()?;
        Ok(LeasedBoard {
            board: self,
            access: access.clone(),
        })
    }
}

/// A board this process owns or was delegated: every operation on it.
#[derive(Debug)]
pub struct LeasedBoard {
    board: Board,
    access: DeviceAccess,
}

/// A leased board's open console: the port with the board's access, which
/// lives as long as the port.
///
/// It reads and writes the port ([`std::io::Read`], [`std::io::Write`]) but
/// never lends it out: a `&mut Port` could be swapped for another port and
/// outlive the access. The port leaves only with the access, through
/// [`BoardConsole::lines`].
#[derive(Debug)]
pub struct BoardConsole {
    // Declared first: the port closes before the access is released.
    port: Port,
    operation: oer_device_lock::DeviceOperation,
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
    /// The lines the console receives; they keep the board's access until
    /// their reader has closed the port.
    pub fn lines(self) -> BoardLines {
        BoardLines {
            lines: console::lines(self.port),
            _operation: self.operation,
        }
    }
}

/// A leased board's console lines with the board's access.
pub struct BoardLines {
    // Declared first: the reader closes the port before the access goes.
    lines: console::Lines,
    _operation: oer_device_lock::DeviceOperation,
}

impl std::ops::Deref for BoardLines {
    type Target = console::Lines;

    fn deref(&self) -> &console::Lines {
        &self.lines
    }
}

impl LeasedBoard {
    pub fn board(&self) -> &Board {
        &self.board
    }

    pub fn access(&self) -> &DeviceAccess {
        &self.access
    }

    pub fn mac(&self) -> &DeviceId {
        self.board.mac()
    }

    pub fn chip(&self) -> &str {
        self.board.chip()
    }

    pub fn has_power(&self) -> bool {
        self.board.has_power()
    }

    /// Where the board was last seen.
    pub fn port(&self) -> &Path {
        self.board.port()
    }

    /// The board's port now: where it was while that exists, else its port
    /// once it is attached again within `within`.
    pub fn current_port(&self, within: Duration) -> Option<PathBuf> {
        if self.board.port.exists() {
            return Some(self.board.port.clone());
        }
        oer_devices::discovery::wait_for(self.mac(), within)
    }

    /// The receipt of `bundle` when the board runs it, by the devices
    /// layer's receipts in `store` (never by the stand's journal), read
    /// once: the basis of a skipped flash.
    pub fn carries(
        &self,
        store: &Store,
        bundle: &oer_image_bundle::ImageBundle,
    ) -> crate::Result<Option<Receipt>> {
        oer_devices::image::carries(store, &self.access, bundle)
    }

    /// Write `bundle`, known as `image`, into the board's flash through
    /// `via` by the one write operation (`oer_devices::image`, which publishes
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
    ) -> crate::Result<Receipt> {
        let _operation = self.access.operation()?;
        let reference = &self.board.reference;
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
                oer_devices::image::write(
                    store,
                    &self.access,
                    bundle,
                    image,
                    Transport::Usb {
                        port: &port,
                        espflash_chip: &self.board.espflash_chip,
                    },
                    by,
                )?
            }
            Via::Jtag => oer_devices::image::write(
                store,
                &self.access,
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
    fn start(&self, start: Start) -> crate::Result<()> {
        match (start, &self.board.power) {
            (Start::Reset, _) => Ok(()),
            (Start::PowerOn, Some(_)) => self.power_on(),
            (Start::PowerOn, None) => {
                let _operation = self.access.operation()?;
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
    fn flashable_port(&self) -> crate::Result<PathBuf> {
        if let Some(port) = self.current_port(Duration::ZERO) {
            return Ok(port);
        }
        if self.board.power.is_none() {
            return Err(format!(
                "board `{}` is not on USB and does not reset by power; a person must reset it",
                self.board.reference.id
            )
            .into());
        }
        let banner = self.download_entry()?;
        eprintln!(
            "hil: board `{}` was not on USB; its ROM now waits for the flash: {}",
            self.board.reference.id,
            reset::reset_line(&banner)
                .as_deref()
                .unwrap_or("no reset line")
        );
        self.current_port(REATTACH)
            .ok_or_else(|| "the board's port did not return after its download entry".into())
    }

    /// The rungs of the board's reset ladder, in the stand file's order.
    pub fn rungs(&self) -> Vec<Box<dyn Rung + '_>> {
        self.board
            .reset
            .iter()
            .filter_map(|step| -> Option<Box<dyn Rung + '_>> {
                match step {
                    ResetStep::UsbJtagRts => Some(Box::new(Rts(self))),
                    ResetStep::Jtag => Some(Box::new(Jtag(self))),
                    ResetStep::Power => self
                        .board
                        .power
                        .as_ref()
                        .map(|_| Box::new(Power(self)) as _),
                }
            })
            .collect()
    }

    /// Reset the board through `path` and return the ROM's reset line it
    /// printed: the rung of that kind, whether or not the board's ladder
    /// lists it.
    pub fn reset(&self, path: ResetPath) -> crate::Result<Option<String>> {
        let banner = match path {
            ResetPath::Rts => Rts(self).reset()?,
            ResetPath::Jtag => Jtag(self).reset()?,
            ResetPath::Power => {
                self.require_power()?;
                Power(self).reset()?
            }
            ResetPath::Download => Some(self.download_entry()?),
        };
        // A chip whose USB Serial/JTAG port re-enumerates prints its banner
        // before the port returns: read the console again once it is back.
        Ok(banner
            .as_deref()
            .and_then(reset::reset_line)
            .or_else(|| reset::reset_line(&self.console(BANNER))))
    }

    /// The board's console read for `watch` without resetting it, once its
    /// port is back; nothing when the access no longer holds.
    pub fn console(&self, watch: Duration) -> String {
        let Ok(_operation) = self.access.operation() else {
            return String::new();
        };
        self.current_port(REATTACH)
            .and_then(|port| reset::open_without_reset(&port).ok())
            .map(|serial| console::read_for(serial, watch))
            .unwrap_or_default()
    }

    /// Open the board's console without a reset, retrying while the port is
    /// still held by the reader of a previous reset or returning after one.
    /// The console keeps the board's access while it is open.
    pub fn open_console(&self) -> crate::Result<BoardConsole> {
        let _operation = self.access.operation()?;
        let port = self
            .current_port(REATTACH)
            .ok_or("the board's port is gone")?;
        let port = retrying(PORT_ACCESS, || reset::open_without_reset(&port))?;
        Ok(BoardConsole {
            port,
            operation: _operation,
        })
    }

    fn require_power(&self) -> crate::Result<&HubPower> {
        self.board.power.as_ref().ok_or_else(|| {
            "the board does not reset by power; add `power` to its `reset` in the stand file".into()
        })
    }

    /// Power the board off and on, watching it leave USB and return.
    pub fn power_cycle_observed(&self) -> crate::Result<PowerCycle> {
        let _operation = self.access.operation()?;
        self.require_power()?
            .cycle_observed(self.mac(), _operation.lifetime())
    }

    /// Power the board off and on, watching it leave USB and return, then
    /// read through its JTAG why it last reset: a power-on reset proves that
    /// its port cuts its power. The read halts the core for a moment and
    /// lets it run on; the image in flash must leave the chip's JTAG on and
    /// not reset again, as the verification of a port arranges.
    pub fn prove_power_loss(&self) -> crate::Result<PowerLoss> {
        let _operation = self.access.operation()?;
        let field = self.board.reset_cause.as_ref().ok_or_else(|| {
            format!(
                "the {} profile names no reset cause, so a power cycle cannot be proven",
                self.chip()
            )
        })?;
        let lifetime = _operation.lifetime();
        self.require_power()?
            .prove_power_loss(self.mac(), lifetime, || {
                let openocd = Openocd::locate()?;
                let mut last = None;
                // The USB-JTAG answers a moment after the board's USB returns.
                for _ in 0..RESET_CAUSE_ATTEMPTS {
                    match openocd.read_word(
                        self.chip(),
                        self.mac().as_ref(),
                        field.address,
                        RESET_CAUSE_READ,
                        lifetime,
                    ) {
                        Ok(word) => return Ok(field.cause(word)),
                        Err(error) => last = Some(error),
                    }
                    oer_process::sleep(RESET_CAUSE_RETRY)?;
                }
                Err(last.unwrap_or_else(|| "no attempt was made".into()))
            })
    }

    /// Power the board off and on and wait until its port is back and open
    /// to the stand's user: a power-on reset into the image in its flash.
    pub fn power_on(&self) -> crate::Result<()> {
        let _operation = self.access.operation()?;
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

    /// Power the board off and on and, as soon as its USB returns, reset it
    /// into its ROM's download mode through the USB Serial/JTAG; the console
    /// the ROM printed. Only a board that resets by power has it.
    pub fn download_entry(&self) -> crate::Result<String> {
        let _operation = self.access.operation()?;
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
    pub fn download_entry_if_powered(&self) -> Option<impl Fn() -> crate::Result<String> + '_> {
        self.board
            .power
            .as_ref()
            .map(|_| move || self.download_entry())
    }
}

struct Rts<'a>(&'a LeasedBoard);

impl Rung for Rts<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::RtsReset
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        let _operation = self.0.access.operation()?;
        let port = self
            .0
            .current_port(Duration::from_secs(5))
            .ok_or("the board's port is gone")?;
        let serial = retrying(PORT_ACCESS, || reset::reset_into_application(&port))?;
        Ok(Some(console::read_for(serial, BANNER)))
    }
}

struct Jtag<'a>(&'a LeasedBoard);

impl Rung for Jtag<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::JtagReset
    }

    /// The console is opened first, without touching the reset lines, so
    /// the read starts before the reset's banner.
    fn reset(&self) -> crate::Result<Option<String>> {
        let _operation = self.0.access.operation()?;
        let openocd = Openocd::locate()?;
        let serial = self
            .0
            .current_port(Duration::ZERO)
            .and_then(|port| reset::open_without_reset(&port).ok());
        openocd.reset(self.0.chip(), self.0.mac(), _operation.lifetime())?;
        Ok(serial.map(|serial| console::read_for(serial, BANNER)))
    }
}

struct Power<'a>(&'a LeasedBoard);

impl Rung for Power<'_> {
    fn step(&self) -> RecoveryStep {
        RecoveryStep::PowerCycle
    }

    fn reset(&self) -> crate::Result<Option<String>> {
        let _operation = self.0.access.operation()?;
        self.0.require_power()?.cycle(_operation.lifetime())?;
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stand() -> StandFile {
        StandFile::parse(include_str!("../../../stand/stand.example.toml")).unwrap()
    }

    fn board(id: &str) -> Board {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let stand = stand();
        Board::new(
            &root,
            &stand,
            stand.board(id).unwrap(),
            "/dev/absent".into(),
        )
        .unwrap()
    }

    /// `board` under a device lock in a directory of the test's own.
    fn leased(board: Board) -> LeasedBoard {
        let locks = tempfile::tempdir().unwrap().keep();
        let access = DeviceAccess::try_acquire_in(&locks, board.mac(), "test")
            .unwrap()
            .unwrap();
        board.lease(&access).unwrap()
    }

    #[test]
    fn the_rungs_follow_the_stand_file_s_ladder() {
        let s31 = leased(board("s31-a"));
        assert_eq!(
            s31.rungs()
                .iter()
                .map(|rung| rung.step())
                .collect::<Vec<_>>(),
            [
                RecoveryStep::RtsReset,
                RecoveryStep::JtagReset,
                RecoveryStep::PowerCycle
            ]
        );
        assert!(s31.has_power());
        assert_eq!(s31.board().espflash_chip, "esp32s31");
        let c5 = leased(board("c5-a"));
        assert_eq!(
            c5.rungs()
                .iter()
                .map(|rung| rung.step())
                .collect::<Vec<_>>(),
            [RecoveryStep::JtagReset, RecoveryStep::PowerCycle]
        );
    }

    #[test]
    fn a_bundle_of_another_chip_is_refused_before_the_port_is_touched() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let profile = oer_chip_profile::Profile::load(&root, "esp32c5").unwrap();
        let directory = tempfile::tempdir().unwrap();
        let bundle = oer_image_bundle::ImageBundle::new(
            directory.path(),
            &profile,
            profile.flash.clone().unwrap(),
        );
        let store = oer_devices::image::Store::at(directory.path().join("receipts"));
        let error = leased(board("s31-a"))
            .flash(&store, &bundle, "test", Via::Usb, "test")
            .unwrap_err()
            .to_string();
        assert!(error.contains("is an esp32s31"), "{error}");
    }

    #[test]
    fn a_board_without_power_cannot_be_power_cycled() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut stand = stand();
        stand.board[1].reset = vec![ResetStep::Jtag];
        let c5 = leased(Board::new(&root, &stand, &stand.board[1], "/dev/absent".into()).unwrap());
        assert!(!c5.has_power());
        assert!(c5.download_entry_if_powered().is_none());
        let error = c5.reset(ResetPath::Power).unwrap_err().to_string();
        assert!(error.contains("does not reset by power"), "{error}");
        // A flash of a board that is neither on USB nor powerable needs a person.
        let error = c5.flashable_port().unwrap_err().to_string();
        assert!(error.contains("a person must reset it"), "{error}");
    }
}
