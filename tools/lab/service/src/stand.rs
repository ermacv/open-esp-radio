//! The service's drivers of the real stand: its boards through
//! `oer-lab-board`, the router over its SSH and the commands it runs under
//! a lease.
//!
//! Each board has one owner here ([`BoardOwner`]): its operations take
//! turns on it in the order they asked, and it holds the board's port while
//! the board is parked. A board's end state follows the ways its stand file
//! lets the stand reset it:
//!
//! - one that resets over its USB Serial/JTAG (`usb-jtag-rts`) rests in its
//!   ROM's download mode, reached by a reset into it and proven by a reset
//!   line the ROM printed after that reset naming a download boot over USB;
//!   its port stays open, since closing a USB Serial/JTAG port can move its
//!   reset lines and start the image: passive console reads use it and hand
//!   it back, and the next park resets into download through it;
//! - one that resets only by JTAG and power rests with its power off: its
//!   switchable hub port, found under it on the host and allowed by the
//!   stand's hub policy, is recorded before it is switched off, then the
//!   port must report off and the board leave USB; the next lease powers
//!   it on again and waits for it to return ([`Drivers::prepare`]).
//!
//! A board whose end state is not confirmed is unavailable until a person
//! returns it; it counts as quiet only when its ROM was seen waiting for a
//! download.

use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use oer_lab_arbiter::{
    claims::{AIR, Claim, covers},
    journal::{ImageIdentity, Journal},
    readiness::{Confirmation, Emission, ROUTER, Recovery},
};
use oer_lab_board::{
    port::Port,
    power::{HubPort, HubPower},
    reset,
    stand::{Board, Via},
};
use oer_lab_client::protocol::{FlashVia, Operation};
use oer_lab_model::{ResetStep, Stand};

use crate::service::{Drivers, OperationIo};

/// How long the ROM's banner is read after a reset.
const BANNER: Duration = Duration::from_secs(2);
/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How long a board may take to leave USB once its port is off.
const POWER_LEAVE: Duration = Duration::from_secs(5);
/// How often a running operation looks at its stop flag.
const POLL: Duration = Duration::from_millis(50);
/// The record of the boards parked with their power off, in the stand's
/// directory: the hub port each was switched off at.
const POWERED_OFF: &str = "powered-off.json";

/// The stand's drivers.
pub struct StandDrivers {
    /// The repository the service was built from: the chip profiles.
    root: PathBuf,
    stand_file: PathBuf,
    /// The stand's directory, for the driver's own records.
    directory: PathBuf,
    journal: Journal,
    /// Each board's owner, by MAC.
    boards: Mutex<BTreeMap<String, Arc<BoardOwner>>>,
    /// Held while the record of boards parked with their power off changes.
    powered_off: Mutex<()>,
}

/// The one owner of a board in the service: its operations take turns in
/// the order they asked, so two operations of one lease (a flash and a
/// power cycle) never drive it at once, and it keeps the board's port while
/// the board rests parked in its ROM.
#[derive(Default)]
pub(crate) struct BoardOwner {
    queue: Mutex<(u64, VecDeque<u64>)>,
    changed: Condvar,
    parked: Mutex<Option<Port>>,
}

/// One operation's turn on a board, until dropped.
pub(crate) struct Turn {
    owner: Arc<BoardOwner>,
    ticket: u64,
}

impl BoardOwner {
    /// Wait for this operation's turn, after every operation that asked
    /// before; refused once `stop` is set while waiting.
    pub(crate) fn take(self: &Arc<Self>, stop: &AtomicBool) -> Result<Turn> {
        let mut queue = lock(&self.queue);
        let ticket = queue.0;
        queue.0 += 1;
        queue.1.push_back(ticket);
        loop {
            if queue.1.front() == Some(&ticket) {
                return Ok(Turn {
                    owner: self.clone(),
                    ticket,
                });
            }
            if stop.load(Ordering::SeqCst) {
                queue.1.retain(|waiting| *waiting != ticket);
                self.changed.notify_all();
                return Err("stopped while waiting for the board".to_owned());
            }
            queue = self
                .changed
                .wait_timeout(queue, POLL)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }
}

impl Turn {
    /// The port the parked board rests on, taken from its owner: closing it
    /// lets the chip leave its ROM.
    fn parked(&self) -> Option<Port> {
        lock(&self.owner.parked).take()
    }

    /// Keep `port`, on which the board rests parked.
    fn park(&self, port: Port) {
        *lock(&self.owner.parked) = Some(port);
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut queue = lock(&self.owner.queue);
        let ticket = self.ticket;
        queue.1.retain(|waiting| *waiting != ticket);
        self.owner.changed.notify_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type Result<T> = std::result::Result<T, String>;

fn text(error: impl std::fmt::Display) -> String {
    error.to_string()
}

impl StandDrivers {
    pub fn new(root: PathBuf, stand_file: PathBuf, directory: PathBuf, journal: Journal) -> Self {
        Self {
            root,
            stand_file,
            directory,
            journal,
            boards: Mutex::new(BTreeMap::new()),
            powered_off: Mutex::new(()),
        }
    }

    /// The stand's stateful resources: every board and, when the stand file
    /// names one, the router.
    pub fn resources(stand: &Stand) -> Vec<String> {
        let mut resources = stand
            .boards
            .iter()
            .map(|board| format!("board:{}", board.mac))
            .collect::<Vec<_>>();
        if stand.router.is_some() {
            resources.push(ROUTER.to_owned());
        }
        resources
    }

    fn stand(&self) -> Result<Stand> {
        Stand::load(&self.stand_file).map_err(text)
    }

    /// The owner of the board with `mac`.
    fn owner(&self, mac: &str) -> Arc<BoardOwner> {
        lock(&self.boards)
            .entry(mac.to_owned())
            .or_default()
            .clone()
    }

    /// The board `query` names, which `claims` must hold, and its MAC as the
    /// stand file writes it; without its turn: for what reads only the stand
    /// file.
    fn described(&self, query: &str, claims: &[Claim]) -> Result<(Board, String)> {
        let stand = self.stand()?;
        let entry = stand.resolve(query).map_err(text)?;
        if !covers(claims, &[Claim::board(&entry.mac)]) {
            return Err(format!(
                "the lease does not hold board `{}`; claim it",
                entry.id
            ));
        }
        // A board that is re-enumerating returns within this wait; one that
        // does not is an error, never a stand-in path a reset would open.
        let port = oer_lab_board::discovery::wait_for(&entry.device_id().map_err(text)?, REATTACH)
            .ok_or_else(|| format!("board `{}` is not attached", entry.id))?;
        Ok((
            Board::new(&self.root, &stand, entry, port).map_err(text)?,
            entry.mac.clone(),
        ))
    }

    /// The board `query` names, which `claims` must hold, on this
    /// operation's turn (refused when `stop` is set first), with the port
    /// it rested parked on: a console keeps using it.
    fn board_and_port(
        &self,
        query: &str,
        claims: &[Claim],
        stop: &AtomicBool,
    ) -> Result<(Board, Option<Port>, Turn)> {
        let (_, mac) = self.described(query, claims)?;
        let turn = self.owner(&mac).take(stop)?;
        // The board's port may have moved while this operation waited.
        let (board, _) = self.described(query, claims)?;
        let parked = turn.parked();
        Ok((board, parked, turn))
    }

    /// The board `query` names, which `claims` must hold, on this
    /// operation's turn; the port it rested parked on is closed first.
    fn board(&self, query: &str, claims: &[Claim], stop: &AtomicBool) -> Result<(Board, Turn)> {
        self.board_and_port(query, claims, stop)
            .map(|(board, _, turn)| (board, turn))
    }

    /// Bring `mac`'s board to the end state its stand file allows, and
    /// prove it.
    fn park(&self, mac: &str) -> Confirmation {
        let unavailable = |reason: String| Confirmation::Unavailable {
            reason,
            emission: Emission::Unknown,
        };
        let stand = match self.stand() {
            Ok(stand) => stand,
            Err(error) => return unavailable(error),
        };
        let Ok(entry) = stand.resolve(mac) else {
            return unavailable("the stand file no longer names it".to_owned());
        };
        let turn = match self.owner(mac).take(&AtomicBool::new(false)) {
            Ok(turn) => turn,
            Err(error) => return unavailable(error),
        };
        // The port it rested on, still open when nothing of the lease
        // closed it: the reset into download goes through it, since closing
        // it could start the image.
        let held = turn.parked();
        if entry.reset.contains(&ResetStep::UsbJtagRts) {
            match self.described(mac, &[Claim::board(mac)]) {
                Ok((board, _)) => self.park_in_download(mac, &board, &turn, held),
                Err(error) => unavailable(error),
            }
        } else if entry.reset.contains(&ResetStep::Power) {
            match self.park_powered_off(mac, &stand, entry) {
                Ok(()) => Confirmation::Ready,
                Err(error) => unavailable(format!("its power is not confirmed off: {error}")),
            }
        } else {
            unavailable(
                "its stand file names no reset that leads to an end state: neither \
                 `usb-jtag-rts` nor `power`"
                    .to_owned(),
            )
        }
    }

    /// Park the board in its ROM's download mode over USB, by a reset
    /// into it (through its `held` port first, when it rested on one) and,
    /// when it has power, a power cycle into download.
    fn park_in_download(
        &self,
        mac: &str,
        board: &Board,
        turn: &Turn,
        held: Option<Port>,
    ) -> Confirmation {
        let mut tried = Vec::new();
        // A ROM seen waiting for any download does not radiate; only what
        // the last attempt saw counts, since every attempt moves the board.
        let mut quiet = false;
        if let Some(mut serial) = held {
            match reset::download_on(&mut serial)
                .map_err(text)
                .and_then(|()| banner(serial))
            {
                Ok((serial, line)) if serves_usb_download(&line) => {
                    eprintln!("lab-service: board {mac} parked on its open port: {line}");
                    turn.park(serial);
                    return Confirmation::Ready;
                }
                Ok((_, line)) => {
                    quiet = line.contains("DOWNLOAD(");
                    tried.push(format!(
                        "a reset into download on its open port read `{line}`"
                    ));
                }
                Err(error) => {
                    tried.push(format!(
                        "a reset into download on its open port failed: {error}"
                    ));
                }
            }
        }
        let mut attempts = vec![("", board.current_port(REATTACH))];
        if attempts.first().is_some_and(|(_, port)| port.is_none()) {
            tried.push("its port is gone".to_owned());
            attempts.clear();
        }
        let mut powered = false;
        loop {
            for (after, port) in attempts.drain(..) {
                let Some(port) = port else {
                    quiet = false;
                    tried.push(format!("its port did not return{after}"));
                    continue;
                };
                match download(&port) {
                    Ok((serial, line)) if serves_usb_download(&line) => {
                        eprintln!("lab-service: board {mac} parked{after}: {line}");
                        turn.park(serial);
                        return Confirmation::Ready;
                    }
                    Ok((_, line)) => {
                        quiet = line.contains("DOWNLOAD(");
                        tried.push(format!("a reset into download{after} read `{line}`"));
                    }
                    Err(error) => {
                        quiet = false;
                        tried.push(format!("a reset into download{after} failed: {error}"));
                    }
                }
            }
            if powered || !board.has_power() {
                break;
            }
            powered = true;
            // The entry's own banner may predate the port's return: the
            // mode is proven with a fresh reset.
            match board.download_entry() {
                Ok(_) => attempts.push((" after a power cycle", board.current_port(REATTACH))),
                Err(error) => {
                    quiet = false;
                    tried.push(format!("the power cycle into download failed: {error}"));
                }
            }
        }
        Confirmation::Unavailable {
            reason: format!(
                "its ROM did not wait for a download over USB: {}",
                tried.join("; ")
            ),
            emission: if quiet {
                Emission::Quiet
            } else {
                Emission::Unknown
            },
        }
    }

    /// Switch the board's power off and prove it: its port is recorded
    /// first, then it must report off and the board leave USB. A board this
    /// record already holds off stays off when its port still reports so.
    fn park_powered_off(
        &self,
        mac: &str,
        stand: &Stand,
        board: &oer_lab_model::Board,
    ) -> Result<()> {
        // The port is recorded before it is switched: a restart then still
        // knows where the board's power is.
        let port = self.powered_off(|records| {
            if let Some(port) = records.get(mac) {
                return Ok(port.clone());
            }
            let port = oer_lab_board::power::hub_port(stand, board)
                .ok_or("no switchable hub port the stand's hub policy allows is found under it")?;
            records.insert(mac.to_owned(), port.clone());
            Ok(port)
        })?;
        let power = HubPower::new(port);
        if power.is_on().map_err(text)? {
            power.off().map_err(text)?;
        }
        if power.is_on().map_err(text)? {
            return Err(format!(
                "port {} of hub {} still reports power after it was switched off",
                power.port.port, power.port.location
            ));
        }
        let device = board.device_id().map_err(text)?;
        if !oer_lab_board::power::wait_until(POWER_LEAVE, &|| {
            !oer_lab_board::discovery::is_attached(&device)
        }) {
            return Err("it stayed on USB after its port was switched off".to_owned());
        }
        eprintln!(
            "lab-service: board {mac} parked with port {} of hub {} off",
            power.port.port, power.port.location
        );
        Ok(())
    }

    /// Read the record of the boards parked with their power off, with
    /// their hub ports, let `change` change it and keep what it left: one
    /// change at a time, so two boards' parkings never lose each other's
    /// record.
    fn powered_off<T>(
        &self,
        change: impl FnOnce(&mut BTreeMap<String, HubPort>) -> Result<T>,
    ) -> Result<T> {
        let _one = lock(&self.powered_off);
        let path = self.directory.join(POWERED_OFF);
        let mut records = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(text)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeMap::new(),
            Err(error) => return Err(text(error)),
        };
        let before = records.clone();
        let result = change(&mut records)?;
        if records != before {
            oer_os_durable::atomic_json(&path, &records).map_err(text)?;
        }
        Ok(result)
    }

    /// The chip of every attached board, its ROM asked when no earlier
    /// answer is known, on each board's turn.
    fn detect(&self, stop: &AtomicBool) -> Result<serde_json::Value> {
        let chips = oer_chip_profile::Profile::all(&self.root).map_err(text)?;
        let mut found = Vec::new();
        for device in oer_lab_board::devices() {
            let mac = device.mac.to_string();
            let turn = self.owner(&mac).take(stop)?;
            // The ROM is asked on the board's own port.
            drop(turn.parked());
            let chip = match device.chip.clone() {
                Some(chip) => chip,
                None => {
                    let name = oer_lab_board::flash::detect(&device.port).map_err(text)?;
                    let profile = chips
                        .iter()
                        .find(|profile| profile.facts.espflash_chip == name)
                        .ok_or_else(|| {
                            format!("board {mac} is an {name}, which no chip profile names")
                        })?;
                    oer_lab_board::device::remember_chip(&device.mac, &profile.facts.id)
                        .map_err(text)?;
                    profile.facts.id.clone()
                }
            };
            found.push(serde_json::json!({ "mac": mac, "chip": chip }));
        }
        Ok(serde_json::Value::Array(found))
    }

    /// Power on `mac`'s board parked with its power off, and wait until it
    /// is back on USB.
    fn power_on(&self, mac: &str) -> Result<()> {
        let Some(port) = self.powered_off(|records| Ok(records.get(mac).cloned()))? else {
            return Ok(());
        };
        let owner = self.owner(mac);
        let _turn = owner.take(&AtomicBool::new(false))?;
        let power = HubPower::new(port);
        power.on().map_err(text)?;
        if !power.is_on().map_err(text)? {
            return Err(format!(
                "port {} of hub {} reports no power after it was switched on",
                power.port.port, power.port.location
            ));
        }
        // Ready for the lease once its port is there as the stand opens it:
        // udev names the port (and hands it to the stand's group) a moment
        // after the kernel attached the board.
        let device = oer_lab_model::DeviceId::parse(mac).map_err(text)?;
        if !oer_lab_board::power::wait_until(REATTACH, &|| {
            oer_lab_board::discovery::port_of(&device)
                .is_ok_and(|port| port.starts_with("/dev/serial/by-id") && port.exists())
        }) {
            return Err(format!(
                "its port did not return after port {} of hub {} was switched on",
                power.port.port, power.port.location
            ));
        }
        eprintln!(
            "lab-service: board {mac} powered on at port {} of hub {}",
            power.port.port, power.port.location
        );
        self.powered_off(|records| {
            records.remove(mac);
            Ok(())
        })
    }

    fn flash(
        &self,
        board: &Board,
        bundle: &str,
        image: &ImageIdentity,
        owner: &str,
        via: FlashVia,
        skip_carried: bool,
    ) -> Result<serde_json::Value> {
        let bundle = oer_image_bundle::ImageBundle::load(Path::new(bundle)).map_err(text)?;
        let store = oer_lab_board::image::Store::open().map_err(text)?;
        if skip_carried && let Some(receipt) = board.carries(&store, &bundle).map_err(text)? {
            return Ok(serde_json::json!({ "written": false, "receipt": receipt }));
        }
        let via = match via {
            FlashVia::Usb => Via::Usb,
            FlashVia::Jtag => Via::Jtag,
        };
        let receipt = board
            .flash(&store, &bundle, &image.name, via, &image.origin)
            .map_err(text)?;
        let application = receipt
            .segment("application")
            .ok_or("the receipt names no application segment")?;
        self.journal
            .record_flash(
                owner.to_owned(),
                board.mac(),
                &ImageIdentity {
                    sha256: application.sha256.clone(),
                    ..image.clone()
                },
            )
            .map_err(text)?;
        Ok(serde_json::json!({ "written": true, "receipt": receipt }))
    }

    /// Stream `board`'s console both ways until `io.stop`: on its `parked`
    /// port when it has one and is not started, which it returns open; when
    /// `start`, after a reset into its image that discards what the port
    /// held before.
    fn console(
        &self,
        board: &Board,
        parked: Option<Port>,
        start: bool,
        io: &OperationIo,
    ) -> Result<Option<Port>> {
        let resting = parked.is_some() && !start;
        let mut port = match parked {
            // A reset from the ROM's download mode re-enumerates the port:
            // the console opens anew, and the reset below discards what the
            // boot that closing the parked port started had printed.
            Some(_) if start => board.open_console().map_err(text)?.into_port(),
            Some(port) => port,
            None => board.open_console().map_err(text)?.into_port(),
        };
        if start {
            reset::reset_usb_serial_jtag(&mut port).map_err(text)?;
        }
        port.set_timeout(POLL).map_err(text)?;
        let mut buffer = vec![0u8; 4096];
        while !io.stop.load(Ordering::SeqCst) {
            match port.read(&mut buffer) {
                Ok(0) => {}
                Ok(read) => io.output.write(buffer.get(..read).unwrap_or_default()),
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) => return Err(format!("the console failed: {error}")),
            }
            while let Ok(bytes) = io.input.try_recv() {
                port.write_all(&bytes).map_err(text)?;
            }
        }
        Ok(resting.then_some(port))
    }

    fn hub_port(&self, hub: &str, port: u8, off: Duration, claims: &[Claim]) -> Result<()> {
        let stand = self.stand()?;
        let found = stand
            .hub(hub)
            .ok_or_else(|| format!("the stand file has no hub `{hub}`"))?;
        let location = found.usb2.clone();
        if !covers(claims, &[Claim::hub_port(&location, port)]) {
            return Err(format!(
                "switching port {port} of hub `{hub}` needs a lease that holds it"
            ));
        }
        oer_lab_board::power::HubPower::new(oer_lab_board::power::HubPort { location, port })
            .cycle_holding(off)
            .map_err(text)
    }
}

/// What `board`'s flash holds, as its receipt says; the journal is history.
fn carried(board: &Board) -> Result<String> {
    use oer_lab_board::image::State;
    Ok(
        match oer_lab_board::image::Store::open()
            .map_err(text)?
            .state(board.mac())
            .map_err(text)?
        {
            Some(State::Started { receipt }) => format!(
                "{} (bundle {}), written by {}",
                receipt.image,
                receipt.digest.get(..12).unwrap_or(&receipt.digest),
                receipt.by
            ),
            Some(State::Written { receipt }) => format!(
                "{} written by {}, its start unconfirmed",
                receipt.image, receipt.by
            ),
            Some(State::Writing { by, .. }) => format!("unknown, a write by {by} did not finish"),
            None => "unknown".to_owned(),
        },
    )
}

/// Reset the board at `port` into its ROM's download mode and read the
/// ROM's reset line from the fresh console: the reset sequence discards
/// what the port held before it.
fn download(port: &Path) -> Result<(Port, String)> {
    banner(reset::reset_into_download(port).map_err(text)?)
}

/// The ROM's reset line, read from `serial` just after a reset.
fn banner(mut serial: Port) -> Result<(Port, String)> {
    serial.set_timeout(POLL).map_err(text)?;
    let started = Instant::now();
    let mut banner = Vec::new();
    let mut buffer = [0u8; 1024];
    while started.elapsed() < BANNER {
        match serial.read(&mut buffer) {
            Ok(read) => banner.extend_from_slice(buffer.get(..read).unwrap_or_default()),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => return Err(text(error)),
        }
    }
    let banner = String::from_utf8_lossy(&banner);
    let line = oer_lab_model::rom::reset_line(&banner).ok_or("the ROM printed no reset line")?;
    Ok((serial, line))
}

/// Whether the ROM's reset line names a download boot that serves the USB
/// Serial/JTAG: `boot:0x.. (DOWNLOAD(USB/UART0))`. A download through UART
/// or SDIO alone leaves the stand no way in over USB.
pub(crate) fn serves_usb_download(line: &str) -> bool {
    line.split_once("boot:")
        .and_then(|(_, boot)| boot.split_once("DOWNLOAD("))
        .is_some_and(|(_, modes)| {
            modes
                .split(')')
                .next()
                .is_some_and(|modes| modes.split('/').any(|mode| mode == "USB"))
        })
}

impl Drivers for StandDrivers {
    fn recover(&self, recovery: &Recovery) -> Confirmation {
        if let Some(mac) = recovery.resource.strip_prefix("board:") {
            return self.park(mac);
        }
        if recovery.resource == ROUTER {
            // Its end state: its AP section disabled and no access point
            // running, its configuration kept.
            return match self.stand().and_then(|stand| crate::router::park(&stand)) {
                Ok(()) => Confirmation::Ready,
                Err(error) => Confirmation::Unavailable {
                    reason: format!("the router's end state is not confirmed: {error}"),
                    emission: Emission::Unknown,
                },
            };
        }
        Confirmation::Unavailable {
            reason: format!("no driver recovers `{}`", recovery.resource),
            emission: Emission::Unknown,
        }
    }

    fn inventory(&self) -> Vec<oer_lab_client::protocol::AttachedBoard> {
        let stand = self.stand().ok();
        let report = oer_lab_router::hub::status().unwrap_or_default();
        let devices = Path::new(oer_lab_router::hub::SYSFS_USB_DEVICES);
        oer_lab_router::boards::attached()
            .into_iter()
            .map(|attached| {
                let board = stand
                    .as_ref()
                    .and_then(|stand| stand.board_by_mac(&attached.mac));
                let hub_port = oer_lab_router::hub::parent(&attached.location)
                    .map(|(hub, port)| format!("{hub}:{port}"));
                oer_lab_client::protocol::AttachedBoard {
                    id: board.map(|board| board.id.clone()),
                    mac: attached.mac.to_string(),
                    location: attached.location.clone(),
                    tty: attached.tty.as_ref().map(|tty| tty.display().to_string()),
                    power: oer_lab_router::hub::power_port(
                        devices,
                        &report,
                        stand.as_ref(),
                        &attached.location,
                    )
                    .map(drop)
                    .map_err(|why| why.to_string()),
                    warning: board
                        .and_then(|board| board.ignored_port(hub_port.as_deref().unwrap_or("-"))),
                    hub_port,
                }
            })
            .collect()
    }

    fn hubs(&self) -> std::result::Result<String, String> {
        oer_lab_board::power::report_text().map_err(text)
    }

    fn resources(&self) -> std::result::Result<Vec<String>, String> {
        if !self.stand_file.exists() {
            return Ok(Vec::new());
        }
        Ok(Self::resources(&self.stand()?))
    }

    fn prepare(&self, resource: &str, claims: &[Claim]) -> std::result::Result<(), String> {
        let Some(mac) = resource.strip_prefix("board:") else {
            return Ok(());
        };
        let parked_off = self.powered_off(|records| Ok(records.contains_key(mac)))?;
        if parked_off && !covers(claims, &[Claim::shared(AIR)]) {
            return Err(format!(
                "board {mac} rests with its power off, and powering it on starts its \
                 image, which may radiate: the lease must hold the air"
            ));
        }
        self.power_on(mac)
            .map_err(|error| format!("its power did not come back: {error}"))
    }

    fn attached(&self) -> Vec<(String, String)> {
        oer_lab_board::discovery::attached()
            .into_iter()
            .filter_map(|port| Some((port.port, port.mac?.to_string())))
            .collect()
    }

    fn run(
        &self,
        operation: &Operation,
        claims: &[Claim],
        io: OperationIo,
    ) -> std::result::Result<serde_json::Value, String> {
        if let Some(refusal) = air_refusal(operation, claims) {
            return Err(refusal);
        }
        let stop = io.stop.clone();
        match operation {
            Operation::Flash {
                board,
                bundle,
                image,
                owner,
                via,
                skip_carried,
            } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                self.flash(&board, bundle, image, owner, *via, *skip_carried)
            }
            Operation::Reset { board, path } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                Ok(serde_json::json!(
                    match path {
                        Some(path) => board.reset(*path),
                        None => board.restart(),
                    }
                    .map_err(text)?
                ))
            }
            Operation::Console { board, reset } => {
                let (board, parked, turn) = self.board_and_port(board, claims, &stop)?;
                if parked.is_none() && !covers(claims, &[Claim::shared(AIR)]) {
                    return Err(NOT_PARKED.to_owned());
                }
                // A console that did not start the image hands the port it
                // rests on back open.
                if let Some(port) = self.console(&board, parked, *reset, &io)? {
                    turn.park(port);
                }
                Ok(serde_json::Value::Null)
            }
            Operation::ReadConsole { board, watch_ms } => {
                let watch = Duration::from_millis(*watch_ms);
                let (board, parked, turn) = self.board_and_port(board, claims, &stop)?;
                Ok(serde_json::json!(match parked {
                    // Read on the port it rests on, which stays open.
                    Some(mut port) => {
                        let text = oer_lab_board::console::read_for(&mut port, watch);
                        turn.park(port);
                        text
                    }
                    None if covers(claims, &[Claim::shared(AIR)]) => board.console(watch),
                    None => return Err(NOT_PARKED.to_owned()),
                }))
            }
            Operation::PowerCycle { board } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                let returned = board.power_cycle_observed().map_err(text)?.verdict()?;
                Ok(serde_json::json!(returned.as_millis() as u64))
            }
            Operation::ProvePowerLoss { board } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                let returned = board.prove_power_loss().map_err(text)?.verdict()?;
                Ok(serde_json::json!(returned.as_millis() as u64))
            }
            Operation::Router { request } => {
                if !covers(claims, &[Claim::shared(ROUTER)]) {
                    return Err("a router operation needs a lease that holds the router".to_owned());
                }
                crate::router::run(request, &self.stand()?, &io)
            }
            Operation::ProbeRouter => {
                if !covers(claims, &[Claim::shared(ROUTER)]) {
                    return Err("probing the router needs a lease that holds it".to_owned());
                }
                serde_json::to_value(oer_lab_router::fixtures::probe_remote(&self.stand_file))
                    .map_err(text)
            }
            Operation::Detect => {
                if !covers(claims, &[Claim::stand()]) {
                    return Err("detecting the attached boards needs the whole stand".to_owned());
                }
                self.detect(&stop)
            }
            Operation::Describe { board } => {
                let (board, _) = self.described(board, claims)?;
                let port = oer_lab_board::discovery::port_of(board.mac())
                    .map_or_else(|_| "no port".to_owned(), |port| port.display().to_string());
                Ok(serde_json::json!({ "port": port, "image": carried(&board)? }))
            }
            Operation::HubPort {
                hub,
                port,
                off_secs,
            } => {
                self.hub_port(hub, *port, Duration::from_secs(*off_secs), claims)?;
                Ok(serde_json::Value::Null)
            }
        }
    }
}

/// Why a lease without the air may not read a board's console: the board
/// no longer rests on its open port, and opening and closing one could
/// start its image.
const NOT_PARKED: &str = "the board does not rest on its open port; reading its console \
                          then needs the air";

/// Why a lease holding `claims` may not run `operation`: one that starts an
/// image, powers a board or drives the router may put it on the air, so the
/// lease must hold the air. A resource in use radiates only under its
/// lease's air claims.
fn air_refusal(operation: &Operation, claims: &[Claim]) -> Option<String> {
    let radiates = match operation {
        Operation::Flash { .. }
        | Operation::PowerCycle { .. }
        | Operation::ProvePowerLoss { .. }
        | Operation::HubPort { .. }
        | Operation::Router { .. }
        // Even into download: the way there may boot the image (a power
        // cycle, or a closed port).
        | Operation::Reset { .. } => true,
        Operation::Console { reset, .. } => *reset,
        Operation::ReadConsole { .. }
        | Operation::ProbeRouter
        | Operation::Detect
        | Operation::Describe { .. } => false,
    };
    (radiates && !covers(claims, &[Claim::shared(AIR)]))
        .then(|| "this operation may start what radiates: the lease must hold the air".to_owned())
}

#[cfg(test)]
mod tests;
