//! The scenario's session on the stand: its image built, its board leased
//! from the LAB service and written, and the link on the board's console.
//! [`Session::finish`] leaves the board running the image or recovers it
//! under the same lease; dropping the session then releases the link and
//! the lease, also when a stage of opening failed. The service owns the
//! board throughout: every write, reset and console goes through it.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use oer_hil_link::Link;
use oer_image_build::images;
use oer_image_bundle::ImageBundle;
use oer_lab_arbiter::{
    claims::Claim,
    journal::{BoardEventKind, ImageIdentity},
};
use oer_lab_client::{
    client::{Client, Running},
    protocol::{FlashVia, LeaseRequest, Operation, Request},
};
use oer_lab_model::rom::{Ladder, RecoveryStep, Rung};
use oer_lab_model::{BoardRole, Stand};

use crate::Scenario;
use crate::finish::{self, Cleanup, Ending, Waits};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How finishing waits for the device under test.
const WAITS: Waits = Waits {
    probe: Duration::from_secs(2),
    hello: Duration::from_secs(10),
};

/// Declared in drop order: the link closes its console before the lease
/// ends with the client.
pub(crate) struct Session {
    link: Option<Link>,
    image_crc32: u32,
    board: StandBoard,
}

impl Session {
    /// Build `scenario`'s image, lease the board `selector` names (or the
    /// one device under test of the image's chip), write the image unless
    /// the board runs it, and open the link with a reset.
    pub(crate) fn open(scenario: &Scenario, selector: Option<&str>) -> Result<Self> {
        let root = oer_os_process::Checkout::discover("HIL scenario")?.root;
        let bundle = build(&root, scenario.image)?;
        let image_crc32 = bundle
            .staged
            .as_ref()
            .map(|staged| staged.runtime_crc32)
            .ok_or("the image is not staged: it has no identity to check")?;
        let stand = Stand::load(&oer_lab_model::path()?)?;
        let board = device_under_test(&stand, selector)?.clone();
        let owner = oer_lab_arbiter::owner::from_environment()?.to_string();
        let mut client = Client::open(&format!("hil {}", scenario.name))?;
        eprintln!("hil: waiting for board {}", board.id);
        client.acquire(
            LeaseRequest {
                owner: owner.clone(),
                work: format!("hil {}", scenario.name),
                scenarios: vec![scenario.name.to_owned()],
                run: None,
                claims: vec![Claim::board(&board.mac)],
                maintenance: None,
                divisible: false,
            },
            &mut |message| eprintln!("hil: {message}"),
        )?;
        let stand_board = StandBoard {
            client: Mutex::new(client),
            board: board.mac.clone(),
            owner,
        };
        stand_board.write(&root, scenario.image, &bundle)?;
        let link = stand_board.link(true)?;
        Ok(Self {
            link: Some(link),
            image_crc32,
            board: stand_board,
        })
    }

    /// The link the session opened, for the cases.
    pub(crate) fn take_link(&mut self) -> Option<Link> {
        self.link.take()
    }

    /// The board, for resets during the cases.
    pub(crate) fn board(&self) -> &StandBoard {
        &self.board
    }

    /// Leave the board running the built image on `link` after the cases
    /// `ending` describes, recovering it when needed; what that took.
    pub(crate) fn finish(&mut self, link: Option<Link>, ending: Ending) -> Cleanup {
        let (link, cleanup) = finish::finish(&self.board, link, self.image_crc32, ending, WAITS);
        self.link = link;
        cleanup
    }

    /// The built image's identity, which the board must report.
    pub(crate) fn image_crc32(&self) -> u32 {
        self.image_crc32
    }

    /// Close the link and finish the lease: the LAB service stops its
    /// operations and confirms the board's end state, or says why not.
    pub(crate) fn release(&mut self) -> std::result::Result<(), String> {
        drop(self.link.take());
        self.board.client().release()
    }
}

/// The leased board, reached through the service.
pub(crate) struct StandBoard {
    client: Mutex<Client>,
    /// The board's MAC.
    board: String,
    owner: String,
}

impl StandBoard {
    fn client(&self) -> std::sync::MutexGuard<'_, Client> {
        self.client.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `operation` to its result, its output discarded.
    fn run(&self, operation: Operation) -> std::result::Result<serde_json::Value, String> {
        self.client().run(operation, &mut std::io::sink(), None)
    }

    /// Write `bundle` unless the board runs it; the service journals it.
    fn write(&self, root: &Path, image: &str, bundle: &ImageBundle) -> Result<()> {
        eprintln!("hil: writing {image} unless the board runs it");
        let git = |arguments: &[&str]| oer_os_process::git::text(root, arguments).ok();
        let written = self.run(Operation::Flash {
            board: self.board.clone(),
            bundle: bundle.directory().display().to_string(),
            image: ImageIdentity {
                name: image.to_owned(),
                sha256: String::new(),
                commit: git(&["rev-parse", "HEAD"]),
                dirty: git(&["status", "--porcelain"]).map(|status| !status.is_empty()),
                origin: std::env::args().collect::<Vec<_>>().join(" "),
            },
            owner: self.owner.clone(),
            via: FlashVia::Usb,
            skip_carried: true,
        })?;
        if written.get("written") == Some(&serde_json::Value::Bool(false)) {
            eprintln!("hil: the board runs {image} already");
        }
        Ok(())
    }

    /// A link on the board's console, opened with a reset into its image
    /// when `reset`.
    fn link(&self, reset: bool) -> std::result::Result<Link, String> {
        let console = self.client().start(Operation::Console {
            board: self.board.clone(),
            reset,
        })?;
        let line = bridge(console).map_err(|error| error.to_string())?;
        Link::start(Box::new(line)).map_err(|error| error.to_string())
    }

    fn steps(&self) -> std::result::Result<Vec<RecoveryStep>, String> {
        serde_json::from_value(self.run(Operation::Ladder {
            board: self.board.clone(),
        })?)
        .map_err(|error| error.to_string())
    }

    fn rung(&self, step: RecoveryStep) -> std::result::Result<Option<String>, String> {
        serde_json::from_value(self.run(Operation::Rung {
            board: self.board.clone(),
            step,
        })?)
        .map_err(|error| error.to_string())
    }
}

/// A local socket the link reads and writes as its serial line, joined to
/// the service's console operation: closing the link's end cancels it.
fn bridge(console: Running) -> std::io::Result<UnixStream> {
    let (line, served) = UnixStream::pair()?;
    let mut to_link = served.try_clone()?;
    let mut from_link = served;
    let input = console.input();
    let mut output = console;
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 4096];
        loop {
            match output.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let Some(chunk) = buffer.get(..read) else {
                        break;
                    };
                    if to_link.write_all(chunk).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = to_link.shutdown(std::net::Shutdown::Both);
    });
    std::thread::spawn(move || {
        let mut buffer = vec![0u8; 4096];
        loop {
            match from_link.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    let Some(chunk) = buffer.get(..read) else {
                        break;
                    };
                    if input.send(chunk).is_err() {
                        break;
                    }
                }
            }
        }
        // The link is gone: its console stops.
        let _ = input.cancel();
    });
    Ok(line)
}

/// One step of the ladder, taken by the service.
struct ServiceRung<'a> {
    board: &'a StandBoard,
    step: RecoveryStep,
}

impl Rung for ServiceRung<'_> {
    fn step(&self) -> RecoveryStep {
        self.step
    }

    fn reset(&self) -> oer_lab_model::Result<Option<String>> {
        Ok(self.board.rung(self.step)?)
    }
}

impl finish::Board for StandBoard {
    fn climb(&self, answers: &mut dyn FnMut() -> bool) -> Ladder {
        let steps = match self.steps() {
            Ok(steps) => steps,
            Err(error) => {
                eprintln!("hil: the service gave no reset ladder: {error}");
                Vec::new()
            }
        };
        let rungs = steps
            .iter()
            .filter(|step| **step != RecoveryStep::DownloadEntry)
            .map(|step| ServiceRung {
                board: self,
                step: *step,
            })
            .collect::<Vec<_>>();
        let download = steps.contains(&RecoveryStep::DownloadEntry).then_some(
            || -> oer_lab_model::Result<String> {
                Ok(self.rung(RecoveryStep::DownloadEntry)?.unwrap_or_default())
            },
        );
        oer_lab_model::rom::climb(
            &rungs
                .iter()
                .map(|rung| rung as &dyn Rung)
                .collect::<Vec<_>>(),
            download
                .as_ref()
                .map(|entry| entry as &dyn Fn() -> oer_lab_model::Result<String>),
            &String::new,
            answers,
        )
    }

    fn reset(&self) -> std::result::Result<(), String> {
        let first = self
            .steps()?
            .into_iter()
            .find(|step| *step != RecoveryStep::DownloadEntry)
            .ok_or("the board's stand file lists no reset")?;
        self.rung(first).map(drop)
    }

    fn connect(&self) -> std::result::Result<Link, String> {
        self.link(false)
    }

    fn recovered(&self, step: RecoveryStep, reset_line: Option<String>) {
        let recorded = self.client().administer(Request::Record {
            owner: String::from("stand"),
            device: Some(self.board.clone()),
            event: BoardEventKind::Recovered {
                step,
                hardware: reset_line
                    .as_deref()
                    .is_some_and(|line| line.contains("DOWNLOAD")),
                reset_line,
                origin: std::env::args().collect::<Vec<_>>().join(" "),
            },
        });
        if let Err(error) = recorded {
            eprintln!("hil: cannot journal the recovery: {error}");
        }
    }
}

/// Build `image` through the image pipeline into its bundle.
fn build(root: &Path, image: &str) -> Result<ImageBundle> {
    eprintln!("hil: building {image}");
    let spec = images::spec(root, image, oer_image_checks::Enforced::Reported)?;
    let bundle = oer_image_build::build(&spec)?;
    for warning in &bundle.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(bundle)
}

/// The board `selector` names, or the one board of the image's chip that
/// may be a device under test.
fn device_under_test<'a>(
    stand: &'a Stand,
    selector: Option<&str>,
) -> Result<&'a oer_lab_model::Board> {
    if let Some(selector) = selector {
        return stand
            .board(selector)
            .ok_or_else(|| format!("the stand file has no board `{selector}`").into());
    }
    let candidates = stand
        .boards
        .iter()
        .filter(|board| board.chip == images::CHIP && board.has_role(BoardRole::Dut))
        .collect::<Vec<_>>();
    match candidates.as_slice() {
        [board] => Ok(board),
        [] => Err(format!(
            "the stand file has no {} board with the dut role",
            images::CHIP
        )
        .into()),
        _ => Err(format!(
            "the stand file has several {} boards with the dut role; name one with --board",
            images::CHIP
        )
        .into()),
    }
}
