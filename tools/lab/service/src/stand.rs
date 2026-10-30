//! The service's drivers of the real stand: its boards through
//! `oer-lab-board` and the commands it runs under a lease.
//!
//! A board's end state is the ROM's download mode, reached by a reset into
//! it and proven by a reset line the ROM printed after that reset, naming a
//! download boot over USB. Its port stays open while the board is parked:
//! closing a USB Serial/JTAG port can move its reset lines. A board whose
//! ROM does not wait for a download over USB, even after a power cycle into
//! download, is unavailable until a person returns it.

use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, PoisonError, atomic::Ordering, mpsc},
    time::{Duration, Instant},
};

use oer_lab_arbiter::{
    claims::{Claim, covers},
    journal::{ImageIdentity, Journal},
    readiness::{Confirmation, ROUTER, Recovery},
};
use oer_lab_board::{
    port::Port,
    reset::{self, RecoveryStep},
    stand::{Board, Via},
};
use oer_lab_model::Stand;

use crate::protocol::{FlashVia, Operation, TerminalSize};
use crate::service::{Drivers, OperationIo};

/// How long the ROM's banner is read after a reset.
const BANNER: Duration = Duration::from_secs(2);
/// How long a board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);
/// How often a running operation looks at its stop flag.
const POLL: Duration = Duration::from_millis(50);
/// How long a stopped command has to exit before it is killed.
const COMMAND_GRACE: Duration = Duration::from_secs(5);

/// The stand's drivers.
pub struct StandDrivers {
    /// The repository the service was built from: the chip profiles.
    root: PathBuf,
    stand_file: PathBuf,
    journal: Journal,
    /// The open ports of parked boards, by MAC.
    parked: Mutex<BTreeMap<String, Port>>,
    /// Resources a command under a lease may have changed beyond what
    /// their driver can confirm.
    touched: Mutex<BTreeSet<String>>,
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
            parked: Mutex::new(BTreeMap::new()),
            touched: Mutex::new(BTreeSet::new()),
        }
    }

    /// The stand's stateful resources: its boards and, when the stand file
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

    /// The board `query` names, which `claims` must hold; its parked port
    /// is closed first.
    fn board(&self, query: &str, claims: &[Claim]) -> Result<Board> {
        let stand = self.stand()?;
        let entry = stand.resolve(query).map_err(text)?;
        if !covers(claims, &[Claim::board(&entry.mac)]) {
            return Err(format!(
                "the lease does not hold board `{}`; claim it",
                entry.id
            ));
        }
        lock(&self.parked).remove(&entry.mac);
        let port = oer_lab_board::discovery::port_of(&entry.device_id().map_err(text)?)
            .unwrap_or_else(|_| PathBuf::from("/dev/null"));
        Board::new(&self.root, &stand, entry, port).map_err(text)
    }

    /// Park `mac`'s board in its ROM's download mode and prove it.
    fn park(&self, mac: &str) -> Confirmation {
        let unavailable = |reason: String, quiet| Confirmation::Unavailable { reason, quiet };
        let board = match self.board(mac, &[Claim::board(mac)]) {
            Ok(board) => board,
            Err(error) => return unavailable(error, false),
        };
        let mut tried = Vec::new();
        if let Some(port) = board.current_port(REATTACH) {
            match download(&port) {
                Ok((serial, line)) if serves_usb_download(&line) => {
                    lock(&self.parked).insert(mac.to_owned(), serial);
                    return Confirmation::Ready;
                }
                Ok((_, line)) => tried.push(format!("a reset into download read `{line}`")),
                Err(error) => tried.push(format!("a reset into download failed: {error}")),
            }
        } else {
            tried.push("its port is gone".to_owned());
        }
        if board.has_power() {
            match board.download_entry() {
                Ok(_) => {
                    if let Some(port) = board.current_port(REATTACH) {
                        // The entry's own banner may predate the port's
                        // return: prove the mode with a fresh reset.
                        match download(&port) {
                            Ok((serial, line)) if serves_usb_download(&line) => {
                                lock(&self.parked).insert(mac.to_owned(), serial);
                                return Confirmation::Ready;
                            }
                            Ok((_, line)) => tried.push(format!(
                                "after a power cycle into download the ROM read `{line}`"
                            )),
                            Err(error) => tried.push(format!(
                                "after a power cycle into download the reset failed: {error}"
                            )),
                        }
                    }
                }
                Err(error) => tried.push(format!("the power cycle into download failed: {error}")),
            }
        }
        unavailable(
            format!(
                "its ROM did not wait for a download over USB: {}",
                tried.join("; ")
            ),
            false,
        )
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
    fn console(&self, board: &Board, start: bool, io: &OperationIo) -> Result<()> {
        let mut port = if start {
            let serial = board.reset_into_console().map_err(text)?;
            serial.into_port()
        } else {
            board.open_console().map_err(text)?.into_port()
        };
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

    /// Run `argv` as a child of the service until it exits or `io.stop`.
    fn command(
        &self,
        argv: &[String],
        directory: &str,
        environment: &[(String, String)],
        terminal: Option<TerminalSize>,
        claims: &[Claim],
        io: &OperationIo,
    ) -> Result<serde_json::Value> {
        let (program, arguments) = argv.split_first().ok_or("the command is empty")?;
        // The router is reached by commands of a lease that holds it; what
        // such a command left on it no driver confirms.
        if covers(claims, &[Claim::exclusive(ROUTER)]) {
            lock(&self.touched).insert(ROUTER.to_owned());
        }
        // On a terminal the command runs under `setsid --ctty`, which makes
        // the pseudo-terminal its controlling terminal in a session of its
        // own: closing the terminal hangs that session up.
        let pty = terminal.map(pseudo_terminal).transpose()?;
        let mut command = match &pty {
            Some(_) => {
                let mut command = oer_os_process::command("setsid");
                command.args(["--ctty", "--wait", "--"]).arg(program);
                command
            }
            None => oer_os_process::command(program),
        };
        command
            .args(arguments)
            .current_dir(directory)
            .env_clear()
            .envs(environment.iter().map(|(key, value)| (key, value)));
        let master = match pty {
            Some((master, slave)) => {
                let stdio = |fd: &std::os::fd::OwnedFd| -> Result<std::process::Stdio> {
                    Ok(std::process::Stdio::from(fd.try_clone().map_err(text)?))
                };
                command
                    .stdin(stdio(&slave)?)
                    .stdout(stdio(&slave)?)
                    .stderr(stdio(&slave)?);
                Some(std::fs::File::from(master))
            }
            None => {
                command
                    .stdin(std::process::Stdio::piped())
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped());
                None
            }
        };
        let mut child =
            oer_os_process::owned::Child::spawn_with_shutdown_grace(&mut command, COMMAND_GRACE)
                .map_err(text)?;
        // The child holds the terminal's slave side now.
        drop(command);
        let mut streams = vec![
            child
                .take_stdout()
                .map(|out| Box::new(out) as Box<dyn Read + Send>),
            child
                .take_stderr()
                .map(|err| Box::new(err) as Box<dyn Read + Send>),
        ];
        let mut stdin: Option<Box<dyn Write + Send>> = child
            .stdin
            .take()
            .map(|input| Box::new(input) as Box<dyn Write + Send>);
        if let Some(master) = &master {
            streams.push(Some(Box::new(master.try_clone().map_err(text)?)));
            stdin = Some(Box::new(master.try_clone().map_err(text)?));
        }
        let (chunks, received) = mpsc::sync_channel::<Vec<u8>>(64);
        let readers = streams
            .into_iter()
            .flatten()
            .map(|mut stream| {
                let chunks = chunks.clone();
                std::thread::spawn(move || {
                    let mut buffer = vec![0u8; 4096];
                    while let Ok(read) = stream.read(&mut buffer) {
                        let Some(chunk) = buffer.get(..read).filter(|chunk| !chunk.is_empty())
                        else {
                            break;
                        };
                        if chunks.send(chunk.to_vec()).is_err() {
                            break;
                        }
                    }
                })
            })
            .collect::<Vec<_>>();
        drop(chunks);
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
                            stdin = None;
                            break;
                        }
                    }
                }
            }
            if io.stop.load(Ordering::SeqCst) {
                // Hang up the terminal's session, then stop the group.
                drop(stdin.take());
                let _ = child.kill();
                break child.wait().map_err(text)?;
            }
            if let Some(status) = child.try_wait().map_err(text)? {
                break status;
            }
            if let Ok(chunk) = received.recv_timeout(POLL) {
                io.output.write(&chunk);
            }
        };
        // The terminal's reader ends once every holder of its slave side
        // closed it; the master closes with the operation.
        drop(stdin);
        drop(master);
        for reader in readers {
            let _ = reader.join();
        }
        while let Ok(chunk) = received.try_recv() {
            io.output.write(&chunk);
        }
        Ok(serde_json::json!(status.code()))
    }

    fn hub_port(&self, hub: &str, port: u8, off: Duration, claims: &[Claim]) -> Result<()> {
        let stand = self.stand()?;
        let found = stand
            .hub(hub)
            .ok_or_else(|| format!("the stand file has no hub `{hub}`"))?;
        let location = found.usb2.clone();
        if !covers(
            claims,
            &[Claim::exclusive(hub_port_resource(&location, port))],
        ) {
            return Err(format!(
                "switching port {port} of hub `{hub}` needs a lease that holds it"
            ));
        }
        oer_lab_board::power::HubPower::new(oer_lab_board::power::HubPort { location, port })
            .cycle_holding(off)
            .map_err(text)
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

/// The claim resource of port `port` of the hub at USB 2 location
/// `location`.
pub fn hub_port_resource(location: &str, port: u8) -> String {
    format!("hub-port:{location}:{port}")
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
    let line = reset::reset_line(&banner).ok_or("the ROM printed no reset line")?;
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
            if lock(&self.touched).remove(ROUTER) {
                return Confirmation::Unavailable {
                    reason: "a command under the lease could change the router; a person \
                             checks it and returns it"
                        .to_owned(),
                    quiet: true,
                };
            }
            return match self
                .stand()
                .and_then(|stand| crate::router::answers(&stand))
            {
                Ok(()) => Confirmation::Ready,
                Err(error) => Confirmation::Unavailable {
                    reason: format!("the router does not answer over SSH: {error}"),
                    quiet: true,
                },
            };
        }
        Confirmation::Unavailable {
            reason: format!("no driver recovers `{}`", recovery.resource),
            quiet: false,
        }
    }

    fn run(
        &self,
        operation: &Operation,
        claims: &[Claim],
        io: OperationIo,
    ) -> std::result::Result<serde_json::Value, String> {
        match operation {
            Operation::Command {
                argv,
                directory,
                environment,
                terminal,
            } => self.command(argv, directory, environment, *terminal, claims, &io),
            Operation::Flash {
                board,
                bundle,
                image,
                owner,
                via,
                skip_carried,
            } => {
                let board = self.board(board, claims)?;
                self.flash(&board, bundle, image, owner, *via, *skip_carried)
            }
            Operation::Reset { board, path } => {
                let board = self.board(board, claims)?;
                Ok(serde_json::json!(board.reset(*path).map_err(text)?))
            }
            Operation::Ladder { board } => {
                let board = self.board(board, claims)?;
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
                let board = self.board(board, claims)?;
                Ok(serde_json::json!(self.rung(&board, *step)?))
            }
            Operation::Console { board, reset } => {
                let board = self.board(board, claims)?;
                self.console(&board, *reset, &io)?;
                Ok(serde_json::Value::Null)
            }
            Operation::ReadConsole { board, watch_ms } => {
                let board = self.board(board, claims)?;
                Ok(serde_json::json!(
                    board.console(Duration::from_millis(*watch_ms))
                ))
            }
            Operation::PowerCycle { board } => {
                let board = self.board(board, claims)?;
                let returned = board.power_cycle_observed().map_err(text)?.verdict()?;
                Ok(serde_json::json!(returned.as_millis() as u64))
            }
            Operation::ProvePowerLoss { board } => {
                let board = self.board(board, claims)?;
                let returned = board.prove_power_loss().map_err(text)?.verdict()?;
                Ok(serde_json::json!(returned.as_millis() as u64))
            }
            Operation::Router { request } => {
                if !covers(claims, &[Claim::shared(ROUTER)]) {
                    return Err("a router operation needs a lease that holds the router".to_owned());
                }
                crate::router::run(request, &self.stand()?, &io)
            }
            Operation::ProbeFixtures => {
                if !covers(claims, &[Claim::shared(ROUTER)]) {
                    return Err("probing the router needs a lease that holds it".to_owned());
                }
                Ok(serde_json::json!(oer_lab_router::fixtures::describe(
                    &oer_lab_router::fixtures::probe(&self.stand_file)
                )))
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
