//! The service's drivers of the real stand: its boards through
//! `oer-lab-board` and the router over its SSH.
//!
//! Each board has one owner here ([`BoardOwner`]): its operations take
//! turns on it in the order they asked. A board's radio works only on its
//! image's command, so a board has no end state to prove: after a lease
//! one that resets over its USB Serial/JTAG (`usb-jtag-rts`) is reset into
//! its image, which stops whatever the lease left running, and is ready
//! again whatever the reset did.

use std::{
    collections::{BTreeMap, VecDeque},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use oer_lab_arbiter::{
    claims::{AIR, Claim, covers},
    journal::{ImageIdentity, Journal},
    readiness::{Confirmation, Emission, ROUTER, Recovery},
};
use oer_lab_board::stand::{Board, BoardConsole, ProfiledBoard, Via};
use oer_lab_client::protocol::{FlashVia, Operation};
use oer_lab_model::{ResetStep, Stand, rom::ResetPath};

use crate::service::{Drivers, OperationIo};

/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How often a running operation looks at its stop flag.
const POLL: Duration = Duration::from_millis(50);

/// The stand's drivers.
pub struct StandDrivers {
    /// The repository the service was built from: the chip profiles.
    root: PathBuf,
    stand_file: PathBuf,
    journal: Journal,
    /// Each board's owner, by MAC.
    boards: Mutex<BTreeMap<String, Arc<BoardOwner>>>,
}

/// The one owner of a board in the service: its operations take turns in
/// the order they asked, so two operations of one lease (a flash and a
/// power cycle) never drive it at once.
#[derive(Default)]
pub(crate) struct BoardOwner {
    queue: Mutex<(u64, VecDeque<u64>)>,
    changed: Condvar,
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
    pub fn new(root: PathBuf, stand_file: PathBuf, journal: Journal) -> Self {
        Self {
            root,
            stand_file,
            journal,
            boards: Mutex::new(BTreeMap::new()),
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
            Board::new(&stand, entry, port).map_err(text)?,
            entry.mac.clone(),
        ))
    }

    /// `board` with its chip's profile, which every chip-dependent operation
    /// needs; refused for a chip without one.
    fn profiled(&self, board: Board) -> Result<ProfiledBoard> {
        board.profiled(&self.root).map_err(text)
    }

    /// The board `query` names, which `claims` must hold, on this
    /// operation's turn (refused when `stop` is set first).
    fn board(&self, query: &str, claims: &[Claim], stop: &AtomicBool) -> Result<(Board, Turn)> {
        let (_, mac) = self.described(query, claims)?;
        let turn = self.owner(&mac).take(stop)?;
        // The board's port may have moved while this operation waited.
        let (board, _) = self.described(query, claims)?;
        Ok((board, turn))
    }

    /// After a lease: reset `mac`'s board into its image when it resets
    /// over its USB Serial/JTAG, on its turn. Nothing is proven: it is
    /// ready whatever the reset did, and a failure is only reported.
    fn settle(&self, mac: &str) -> Confirmation {
        let resets_over_usb = self.stand().ok().is_some_and(|stand| {
            stand
                .resolve(mac)
                .is_ok_and(|entry| entry.reset.contains(&ResetStep::UsbJtagRts))
        });
        if resets_over_usb {
            let reset = self
                .owner(mac)
                .take(&AtomicBool::new(false))
                .and_then(|_turn| {
                    let (board, _) = self.described(mac, &[Claim::board(mac)])?;
                    self.profiled(board)?.restart().map_err(text)
                });
            if let Err(error) = reset {
                eprintln!("lab-service: board {mac} was not reset after its lease: {error}");
            }
        }
        Confirmation::Ready
    }

    /// The chip of every attached board, its ROM asked when no earlier
    /// answer is known, on each board's turn.
    fn detect(&self, stop: &AtomicBool) -> Result<serde_json::Value> {
        let chips = oer_chip_profile::Profile::all(&self.root).map_err(text)?;
        let mut found = Vec::new();
        for device in oer_lab_board::devices() {
            let mac = device.mac.to_string();
            let _turn = self.owner(&mac).take(stop)?;
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

    fn flash(
        &self,
        board: &ProfiledBoard,
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

    /// Stream `console` both ways until `io.stop`.
    fn console(&self, console: BoardConsole, io: &OperationIo) -> Result<()> {
        let mut port = console.into_port();
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
        Ok(())
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

impl Drivers for StandDrivers {
    fn recover(&self, recovery: &Recovery) -> Confirmation {
        if let Some(mac) = recovery.resource.strip_prefix("board:") {
            return self.settle(mac);
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

    fn prepare(&self, _: &str, _: &[Claim]) -> std::result::Result<(), String> {
        Ok(())
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
                let board = self.profiled(board)?;
                self.flash(&board, bundle, image, owner, *via, *skip_carried)
            }
            Operation::Reset { board, path } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                // A hub port's power cycle needs no chip profile.
                Ok(serde_json::json!(
                    match path {
                        Some(ResetPath::Power) => board.power_reset(),
                        Some(path) => self.profiled(board)?.reset(*path),
                        None => self.profiled(board)?.restart(),
                    }
                    .map_err(text)?
                ))
            }
            Operation::Console { board, reset } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                // A start resets into the image, which needs the chip profile.
                let console = if *reset {
                    self.profiled(board)?.open_console_from_reset()
                } else {
                    board.open_console()
                }
                .map_err(text)?;
                self.console(console, &io)?;
                Ok(serde_json::Value::Null)
            }
            Operation::ReadConsole { board, watch_ms } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                Ok(serde_json::json!(
                    board.console(Duration::from_millis(*watch_ms))
                ))
            }
            Operation::PowerCycle { board } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                let returned = board.power_cycle_observed().map_err(text)?.verdict()?;
                Ok(serde_json::json!(returned.as_millis() as u64))
            }
            Operation::ProvePowerLoss { board } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                let returned = self
                    .profiled(board)?
                    .prove_power_loss()
                    .map_err(text)?
                    .verdict()?;
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
                let image = carried(&board)?;
                // Why the chip-dependent operations are refused, if they are.
                let without_profile = self.profiled(board).err();
                Ok(serde_json::json!({
                    "port": port,
                    "image": image,
                    "without_profile": without_profile,
                }))
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

/// Why a lease holding `claims` may not run `operation`. A board's radio
/// works only on its image's command, which the scenario that gives it
/// claims the air for; a router operation turns the router's own radio on,
/// so its lease must hold the air.
fn air_refusal(operation: &Operation, claims: &[Claim]) -> Option<String> {
    (matches!(operation, Operation::Router { .. }) && !covers(claims, &[Claim::shared(AIR)]))
        .then(|| "a router operation radiates: the lease must hold the air".to_owned())
}

#[cfg(test)]
mod tests;
