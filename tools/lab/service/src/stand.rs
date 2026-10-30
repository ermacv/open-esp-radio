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
//!   reset lines;
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
        mpsc,
    },
    time::{Duration, Instant},
};

use oer_lab_arbiter::{
    claims::{Claim, covers},
    journal::{ImageIdentity, Journal},
    readiness::{Confirmation, Emission, ROUTER, Recovery},
};
use oer_lab_board::{
    port::Port,
    power::{HubPort, HubPower},
    reset,
    stand::{Board, Via},
};
use oer_lab_client::protocol::{FlashVia, Operation, TerminalSize};
use oer_lab_model::{ResetStep, Stand, rom::RecoveryStep};

use crate::service::{Drivers, OperationIo};

/// How long the ROM's banner is read after a reset.
const BANNER: Duration = Duration::from_secs(2);
/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How long a board may take to leave USB once its port is off.
const POWER_LEAVE: Duration = Duration::from_secs(5);
/// How often a running operation looks at its stop flag.
const POLL: Duration = Duration::from_millis(50);
/// How long a stopped command has to exit before it is killed.
const COMMAND_GRACE: Duration = Duration::from_secs(5);
/// How long a killed command's control group may take to empty.
const GROUP_EMPTY: Duration = Duration::from_secs(10);
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
    /// The control groups commands run in; none when the service owns no
    /// group, and runs no command.
    commands: Option<oer_os_process::cgroup::Delegated>,
    next_group: std::sync::atomic::AtomicU64,
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
    pub fn new(
        root: PathBuf,
        stand_file: PathBuf,
        directory: PathBuf,
        journal: Journal,
        commands: Option<oer_os_process::cgroup::Delegated>,
    ) -> Self {
        Self {
            root,
            stand_file,
            directory,
            journal,
            boards: Mutex::new(BTreeMap::new()),
            commands,
            next_group: std::sync::atomic::AtomicU64::new(1),
        }
    }

    /// The stand's stateful resources: every board and, when the stand file
    /// names a station fixture, the router.
    pub fn resources(stand: &Stand) -> Vec<String> {
        let mut resources = stand
            .boards
            .iter()
            .map(|board| format!("board:{}", board.mac))
            .collect::<Vec<_>>();
        if stand.station_fixture.ssh_target.is_some() {
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
        let port = oer_lab_board::discovery::port_of(&entry.device_id().map_err(text)?)
            .unwrap_or_else(|_| PathBuf::from("/dev/null"));
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
        let (board, turn) = match self.board(mac, &[Claim::board(mac)], &AtomicBool::new(false)) {
            Ok(board) => board,
            Err(error) => return unavailable(error),
        };
        if board.resets().contains(&ResetStep::UsbJtagRts) {
            self.park_in_download(mac, &board, &turn)
        } else if board.resets().contains(&ResetStep::Power) {
            match self.park_powered_off(mac, &board) {
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
    /// into it and, when it has power, a power cycle into download.
    fn park_in_download(&self, mac: &str, board: &Board, turn: &Turn) -> Confirmation {
        let mut tried = Vec::new();
        // A ROM seen waiting for any download does not radiate.
        let mut quiet = false;
        let mut attempts = vec![("", board.current_port(REATTACH))];
        if attempts.first().is_some_and(|(_, port)| port.is_none()) {
            tried.push("its port is gone".to_owned());
            attempts.clear();
        }
        let mut powered = false;
        loop {
            for (after, port) in attempts.drain(..) {
                let Some(port) = port else {
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
                        quiet |= line.contains("DOWNLOAD(");
                        tried.push(format!("a reset into download{after} read `{line}`"));
                    }
                    Err(error) => {
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
                Err(error) => tried.push(format!("the power cycle into download failed: {error}")),
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
    fn park_powered_off(&self, mac: &str, board: &Board) -> Result<()> {
        let mut records = self.powered_off()?;
        let power = match records.get(mac) {
            Some(port) => HubPower::new(port.clone()),
            None => {
                let power = board.hub_power().cloned().ok_or(
                    "no switchable hub port the stand's hub policy allows is found under it",
                )?;
                records.insert(mac.to_owned(), power.port.clone());
                self.record_powered_off(&records)?;
                power
            }
        };
        if power.is_on().map_err(text)? {
            power.off().map_err(text)?;
        }
        if power.is_on().map_err(text)? {
            return Err(format!(
                "port {} of hub {} still reports power after it was switched off",
                power.port.port, power.port.location
            ));
        }
        let device = board.mac();
        if !oer_lab_board::power::wait_until(POWER_LEAVE, &|| {
            !oer_lab_board::discovery::is_attached(device)
        }) {
            return Err("it stayed on USB after its port was switched off".to_owned());
        }
        eprintln!(
            "lab-service: board {mac} parked with port {} of hub {} off",
            power.port.port, power.port.location
        );
        Ok(())
    }

    /// The boards parked with their power off, with their hub ports.
    fn powered_off(&self) -> Result<BTreeMap<String, HubPort>> {
        match std::fs::read(self.directory.join(POWERED_OFF)) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(text),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(BTreeMap::new()),
            Err(error) => Err(text(error)),
        }
    }

    fn record_powered_off(&self, records: &BTreeMap<String, HubPort>) -> Result<()> {
        oer_os_durable::atomic_json(&self.directory.join(POWERED_OFF), records).map_err(text)
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
        let mut records = self.powered_off()?;
        let Some(port) = records.get(mac).cloned() else {
            return Ok(());
        };
        let owner = self.owner(mac);
        let _turn = owner.take(&AtomicBool::new(false))?;
        let power = HubPower::new(port);
        power.on().map_err(text)?;
        let device = oer_lab_model::DeviceId::parse(mac).map_err(text)?;
        if !oer_lab_board::power::wait_until(REATTACH, &|| {
            oer_lab_board::discovery::is_attached(&device)
        }) {
            return Err(format!(
                "it did not return on USB after port {} of hub {} was switched on",
                power.port.port, power.port.location
            ));
        }
        records.remove(mac);
        self.record_powered_off(&records)
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

    /// Take `step` of `board`'s ladder; the console the ROM printed.
    fn rung(&self, board: &Board, step: RecoveryStep) -> Result<String> {
        let banner = match step {
            RecoveryStep::DownloadEntry => Some(board.download_entry().map_err(text)?),
            step => {
                let rungs = board.rungs();
                let rung = rungs
                    .iter()
                    .find(|rung| rung.step() == step)
                    .ok_or_else(|| format!("the board's ladder has no {step:?}"))?;
                rung.reset().map_err(text)?
            }
        };
        // A chip whose port re-enumerates prints its banner before the port
        // returns: read the console again once it is back.
        Ok(banner.unwrap_or_else(|| board.console(BANNER)))
    }

    /// Stream `board`'s console both ways until `io.stop`.
    /// Stream `board`'s console both ways until `io.stop`: on its `parked`
    /// port when it has one and is not started; when `start`, after a reset
    /// into its image that discards what the port held before.
    fn console(
        &self,
        board: &Board,
        parked: Option<Port>,
        start: bool,
        io: &OperationIo,
    ) -> Result<()> {
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
        Ok(())
    }

    /// Run `argv` as a child of the service, in a control group of its own,
    /// until it exits or `io.stop`; `run_to_end` keeps it running after
    /// `io.stop` until it ends by itself. The operation ends once the group
    /// is empty and its output read: a group that does not empty leaves the
    /// lease's resources unavailable.
    fn command(
        &self,
        argv: &[String],
        directory: &str,
        environment: &[(String, String)],
        terminal: Option<TerminalSize>,
        run_to_end: bool,
        io: &OperationIo,
    ) -> Result<serde_json::Value> {
        let (program, arguments) = argv.split_first().ok_or("the command is empty")?;
        let groups = self
            .commands
            .as_ref()
            .ok_or("this service owns no control group to run commands in")?;
        let group = groups
            .group(&format!(
                "command-{}",
                self.next_group.fetch_add(1, Ordering::SeqCst)
            ))
            .map_err(text)?;
        let mut command = oer_os_process::command(program);
        command
            .args(arguments)
            .current_dir(directory)
            .env_clear()
            .envs(environment.iter().map(|(key, value)| (key, value)));
        // On a terminal the command leads a session with the pseudo-terminal
        // as its controlling terminal; its streams are the terminal.
        let pty = terminal.map(pseudo_terminal).transpose()?;
        let (controlling, master) = match pty {
            Some((master, slave)) => {
                let stdio = |fd: &std::os::fd::OwnedFd| -> Result<std::process::Stdio> {
                    Ok(std::process::Stdio::from(fd.try_clone().map_err(text)?))
                };
                command
                    .stdin(stdio(&slave)?)
                    .stdout(stdio(&slave)?)
                    .stderr(stdio(&slave)?);
                (Some(slave), Some(std::fs::File::from(master)))
            }
            None => {
                command
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                (None, None)
            }
        };
        let mut child = group.spawn(&mut command, controlling).map_err(text)?;
        // The child holds the terminal's slave side now.
        drop(command);
        let mut streams = Vec::new();
        if let Some(out) = child.stdout.take() {
            streams.push(std::fs::File::from(std::os::fd::OwnedFd::from(out)));
        }
        if let Some(err) = child.stderr.take() {
            streams.push(std::fs::File::from(std::os::fd::OwnedFd::from(err)));
        }
        let mut stdin: Option<Box<dyn Write + Send>> = child
            .stdin
            .take()
            .map(|input| Box::new(input) as Box<dyn Write + Send>);
        if let Some(master) = &master {
            streams.push(master.try_clone().map_err(text)?);
            stdin = Some(Box::new(master.try_clone().map_err(text)?));
        }
        // The readers end at the end of their stream, or once the group is
        // empty and the stream stayed quiet: a descriptor passed outside the
        // group may still hold it open.
        let ended = std::sync::Arc::new(AtomicBool::new(false));
        let (chunks, received) = mpsc::sync_channel::<Vec<u8>>(64);
        let readers = streams
            .into_iter()
            .map(|stream| {
                let chunks = chunks.clone();
                let ended = ended.clone();
                std::thread::spawn(move || read_until_quiet(stream, &chunks, &ended))
            })
            .collect::<Vec<_>>();
        drop(chunks);
        let mut stopping = false;
        let status = loop {
            while let Ok(chunk) = received.try_recv() {
                io.output.write(&chunk);
            }
            if let Some(input) = &mut stdin {
                loop {
                    match io.input.try_recv() {
                        Ok(bytes) => {
                            if input.write_all(&bytes).is_err() {
                                break;
                            }
                        }
                        Err(mpsc::TryRecvError::Empty) => break,
                        Err(mpsc::TryRecvError::Disconnected) => {
                            // A terminal's input ends with its master: a
                            // command on one keeps it until it ends.
                            if master.is_none() {
                                stdin = None;
                            }
                            break;
                        }
                    }
                }
            }
            if io.stop.load(Ordering::SeqCst) && !run_to_end && !stopping {
                // Admit nothing more, ask the command's session to end,
                // and kill the group if it has not after the grace.
                stopping = true;
                group.close();
                drop(stdin.take());
                if let Some(leader) =
                    rustix::process::Pid::from_raw(i32::try_from(child.id()).unwrap_or(i32::MAX))
                {
                    let _ =
                        rustix::process::kill_process_group(leader, rustix::process::Signal::TERM);
                }
                if !group.wait_empty(COMMAND_GRACE).map_err(text)? {
                    group.kill().map_err(text)?;
                }
            }
            if let Some(status) = child.try_wait().map_err(text)? {
                break status;
            }
            if let Ok(chunk) = received.recv_timeout(POLL) {
                io.output.write(&chunk);
            }
        };
        // What the command left in its group ends with it.
        if group.populated().map_err(text)? {
            group.kill().map_err(text)?;
        }
        group.close();
        let emptied = group.wait_empty(GROUP_EMPTY).map_err(text)?;
        ended.store(true, Ordering::SeqCst);
        drop(stdin);
        while readers.iter().any(|reader| !reader.is_finished()) {
            if let Ok(chunk) = received.recv_timeout(POLL) {
                io.output.write(&chunk);
            }
        }
        drop(master);
        while let Ok(chunk) = received.try_recv() {
            io.output.write(&chunk);
        }
        if !emptied {
            let reason = format!(
                "the command's control group {} kept a process after it was killed",
                group.name()
            );
            io.unconfirmed(reason.clone());
            return Err(reason);
        }
        Ok(serde_json::json!(status.code()))
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

/// Send what `stream` yields to `chunks` until its end, or until `ended` is
/// set and the stream stays quiet for one poll: a process that left the
/// command may still hold it open, and the operation ends anyway.
fn read_until_quiet(
    mut stream: std::fs::File,
    chunks: &mpsc::SyncSender<Vec<u8>>,
    ended: &std::sync::atomic::AtomicBool,
) {
    use rustix::event::{PollFd, PollFlags, Timespec, poll};
    let timeout = Timespec {
        tv_sec: 0,
        tv_nsec: i64::try_from(POLL.as_nanos()).unwrap_or(50_000_000),
    };
    let mut buffer = vec![0u8; 4096];
    let mut quiet_by = None;
    loop {
        if ended.load(Ordering::SeqCst) {
            let by = *quiet_by.get_or_insert_with(|| Instant::now() + COMMAND_GRACE);
            if Instant::now() >= by {
                return;
            }
        }
        let ready = {
            let mut fds = [PollFd::new(&stream, PollFlags::IN)];
            match poll(&mut fds, Some(&timeout)) {
                Ok(ready) => ready > 0,
                Err(rustix::io::Errno::INTR) => false,
                Err(_) => return,
            }
        };
        if !ready {
            if ended.load(Ordering::SeqCst) {
                return;
            }
            continue;
        }
        // A terminal whose every slave closed reads as an error, a pipe as
        // its end.
        let Ok(read) = stream.read(&mut buffer) else {
            return;
        };
        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty()) else {
            return;
        };
        if chunks.send(chunk.to_vec()).is_err() {
            return;
        }
    }
}

/// A pseudo-terminal of `size`: its master and its slave side.
fn pseudo_terminal(size: TerminalSize) -> Result<(std::os::fd::OwnedFd, std::os::fd::OwnedFd)> {
    use rustix::{fs::OFlags, pty};
    let master = pty::openpt(pty::OpenptFlags::RDWR | pty::OpenptFlags::NOCTTY).map_err(text)?;
    pty::grantpt(&master).map_err(text)?;
    pty::unlockpt(&master).map_err(text)?;
    let name = pty::ptsname(&master, Vec::new()).map_err(text)?;
    rustix::termios::tcsetwinsize(
        &master,
        rustix::termios::Winsize {
            ws_row: size.rows,
            ws_col: size.columns,
            ws_xpixel: 0,
            ws_ypixel: 0,
        },
    )
    .map_err(text)?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(text)?;
    Ok((master, slave))
}

/// Reset the board at `port` into its ROM's download mode and read the
/// ROM's reset line from the fresh console: the reset sequence discards
/// what the port held before it.
fn download(port: &Path) -> Result<(Port, String)> {
    let mut serial = reset::reset_into_download(port).map_err(text)?;
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
            // Its end state: it answers over SSH and runs its access point.
            // A command that could change it more was marked before it ran.
            return match self
                .stand()
                .and_then(|stand| crate::router::end_state(&stand))
            {
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

    fn prepare(&self, resource: &str) -> std::result::Result<(), String> {
        match resource.strip_prefix("board:") {
            Some(mac) => self
                .power_on(mac)
                .map_err(|error| format!("its power did not come back: {error}")),
            None => Ok(()),
        }
    }

    fn marks(&self, operation: &Operation, claims: &[Claim]) -> Vec<(String, String)> {
        // A command of a lease that holds the router may change it over its
        // SSH beyond what an answer confirms.
        match operation {
            Operation::Command { .. } if covers(claims, &[Claim::shared(ROUTER)]) => vec![(
                ROUTER.to_owned(),
                "a command under the lease could change the router; a person checks it and \
                 returns it"
                    .to_owned(),
            )],
            _ => Vec::new(),
        }
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
        let stop = io.stop.clone();
        match operation {
            Operation::Command {
                argv,
                directory,
                environment,
                terminal,
                run_to_end,
            } => self.command(argv, directory, environment, *terminal, *run_to_end, &io),
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
                Ok(serde_json::json!(board.reset(*path).map_err(text)?))
            }
            Operation::Ladder { board } => {
                let (board, _) = self.described(board, claims)?;
                let mut steps = board
                    .rungs()
                    .iter()
                    .map(|rung| rung.step())
                    .collect::<Vec<_>>();
                if board.has_power() {
                    steps.push(RecoveryStep::DownloadEntry);
                }
                Ok(serde_json::json!(steps))
            }
            Operation::Rung { board, step } => {
                let (board, _turn) = self.board(board, claims, &stop)?;
                Ok(serde_json::json!(self.rung(&board, *step)?))
            }
            Operation::Console { board, reset } => {
                let (board, parked, _turn) = self.board_and_port(board, claims, &stop)?;
                self.console(&board, parked, *reset, &io)?;
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

#[cfg(test)]
mod tests;
