//! The scenario's session on the stand: its image built, its board leased
//! and written, and the link on the board's console. [`Session::finish`]
//! leaves the board running the image or recovers it under the same lease;
//! dropping the session then releases the link, the board and the lease in
//! that order, also when a stage of opening failed.

use std::{path::Path, time::Duration};

use oer_hil_link::Link;
use oer_image_build::images;
use oer_image_bundle::ImageBundle;
use oer_lab_arbiter::{
    Arbiter, Grant, QuarantineTrigger, Request,
    claims::Claim,
    journal::{BoardEventKind, ImageIdentity},
};
use oer_lab_board::lock::DeviceId;
use oer_lab_board::port::{Lines, Port, Settings};
use oer_lab_board::reset::{Ladder, RecoveryStep};
use oer_lab_board::stand::{LeasedBoard, Via};
use oer_lab_model::{BoardRole, Stand};

use crate::Scenario;
use crate::finish::{self, Cleanup, Ending, Waits};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The device under test's console: its lines as they are, retried while a
/// previous reader still holds the port.
const DUT_CONSOLE: Settings = Settings::CONSOLE
    .lines(Lines::Kept)
    .timeout(Duration::ZERO)
    .busy(Duration::from_secs(2));

/// How long the board's port may take to return after a reset.
const REATTACH: Duration = Duration::from_secs(10);

/// How long the console is read for the ROM's line after a rung that does
/// not return one itself.
const BANNER_WATCH: Duration = Duration::from_secs(2);

/// How long finishing waits for the device under test.
const WAITS: Waits = Waits {
    probe: Duration::from_secs(2),
    hello: Duration::from_secs(10),
};

/// Declared in drop order.
pub(crate) struct Session {
    link: Option<Link>,
    image_crc32: u32,
    board: LeasedBoard,
    _grant: Grant,
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
        let stand_file = oer_lab_model::path()?;
        let stand = Stand::load(&stand_file)?;
        let board = device_under_test(&stand, selector)?.clone();
        let mut request = Request::from_environment(format!("hil {}", scenario.name))?;
        request.scenarios = vec![scenario.name.to_owned()];
        request.claims = vec![Claim::board(&board.mac)];
        eprintln!("hil: waiting for board {}", board.id);
        let mut grant = Arbiter::open()?
            .with_stand_file(stand_file)
            .acquire(&request)?;
        grant.supervise_self(false);
        let mac = DeviceId::parse(&board.mac)?;
        let access = grant
            .device(&mac)
            .ok_or("the lease holds no access to the board")?;
        let leased =
            oer_lab_board::stand::Board::attached(&root, &stand, &board.id)?.lease(access)?;
        write(&root, scenario.image, &bundle, &leased, &request.owner)?;
        let mut port = Port::open(leased.port(), DUT_CONSOLE)?;
        // Every supported chip restarts through its USB Serial/JTAG console.
        oer_lab_board::reset::reset_usb_serial_jtag(&mut port)?;
        let link = Link::start(Box::new(port))?;
        Ok(Self {
            link: Some(link),
            image_crc32,
            board: leased,
            _grant: grant,
        })
    }

    /// The link, while the device answers on one.
    pub(crate) fn link(&self) -> Option<&Link> {
        self.link.as_ref()
    }

    /// Leave the board running the built image after the cases `ending`
    /// describes, recovering it when needed; what that took.
    pub(crate) fn finish(&mut self, ending: Ending) -> Cleanup {
        let (link, cleanup) = finish::finish(
            &StandBoard(&self.board),
            self.link.take(),
            self.image_crc32,
            ending,
            WAITS,
        );
        self.link = link;
        cleanup
    }

    /// The built image's identity, which the board must report.
    pub(crate) fn image_crc32(&self) -> u32 {
        self.image_crc32
    }
}

/// The leased board as finishing sees it.
struct StandBoard<'a>(&'a LeasedBoard);

impl finish::Board for StandBoard<'_> {
    fn climb(&self, answers: &mut dyn FnMut() -> bool) -> Ladder {
        let rungs = self.0.rungs();
        let download_entry = self.0.download_entry_if_powered();
        oer_lab_board::reset::climb(
            &rungs.iter().map(Box::as_ref).collect::<Vec<_>>(),
            download_entry
                .as_ref()
                .map(|entry| entry as &dyn Fn() -> oer_lab_board::Result<String>),
            &|| self.0.console(BANNER_WATCH),
            answers,
        )
    }

    fn connect(&self) -> std::result::Result<Link, String> {
        let port = self
            .0
            .current_port(REATTACH)
            .ok_or("the board's port did not return")?;
        let port = Port::open(&port, DUT_CONSOLE).map_err(|error| error.to_string())?;
        Link::start(Box::new(port)).map_err(|error| error.to_string())
    }

    fn quarantine(&self, reason: String) -> std::result::Result<(), String> {
        Arbiter::open()
            .and_then(|arbiter| {
                arbiter.quarantine(
                    self.0.mac().as_ref(),
                    QuarantineTrigger::Unreachable,
                    reason,
                    None,
                )
            })
            .map_err(|error| error.to_string())
    }

    fn recovered(&self, step: RecoveryStep, reset_line: Option<String>) {
        let recorded = Arbiter::open().and_then(|arbiter| {
            arbiter.journal().record_by(
                String::from("stand"),
                Some(self.0.mac().to_string()),
                BoardEventKind::Recovered {
                    step,
                    hardware: reset_line
                        .as_deref()
                        .is_some_and(|line| line.contains("DOWNLOAD")),
                    reset_line,
                    origin: std::env::args().collect::<Vec<_>>().join(" "),
                },
            )
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

/// Write `bundle` into `board` unless its receipt says it runs it, and
/// journal the write.
fn write(
    root: &Path,
    image: &str,
    bundle: &ImageBundle,
    board: &LeasedBoard,
    owner: &str,
) -> Result<()> {
    let store = oer_lab_board::image::Store::open()?;
    if board.carries(&store, bundle)?.is_some() {
        eprintln!("hil: the board runs {image} already");
        return Ok(());
    }
    eprintln!("hil: writing {image}");
    let origin = std::env::args().collect::<Vec<_>>().join(" ");
    let receipt = board.flash(&store, bundle, image, Via::Usb, &origin)?;
    let application = receipt
        .segment("application")
        .ok_or("the receipt names no application segment")?;
    let git = |arguments: &[&str]| oer_os_process::git::text(root, arguments).ok();
    oer_lab_arbiter::journal::Journal::open()?.record_flash(
        owner.to_owned(),
        board.mac(),
        &ImageIdentity {
            name: image.to_owned(),
            sha256: application.sha256.clone(),
            commit: git(&["rev-parse", "HEAD"]),
            dirty: git(&["status", "--porcelain"]).map(|status| !status.is_empty()),
            origin,
        },
    )?;
    Ok(())
}
