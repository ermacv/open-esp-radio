//! The scenario's session on the stand: its image built, its board leased
//! and written, and the link on the board's console. Dropping the session
//! releases them in that reverse order: the link, the board, the lease.

use std::{path::Path, time::Duration};

use oer_hil_link::Link;
use oer_image_build::images;
use oer_image_bundle::ImageBundle;
use oer_lab_arbiter::{Arbiter, Grant, Request, claims::Claim, journal::ImageIdentity};
use oer_lab_board::lock::DeviceId;
use oer_lab_board::port::{Lines, Port, Settings};
use oer_lab_board::stand::{LeasedBoard, Via};
use oer_lab_model::{BoardRole, Stand};

use crate::Scenario;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The device under test's console: its lines as they are, retried while a
/// previous reader still holds the port.
const DUT_CONSOLE: Settings = Settings::CONSOLE
    .lines(Lines::Kept)
    .timeout(Duration::ZERO)
    .busy(Duration::from_secs(2));

/// Declared in drop order.
pub(crate) struct Session {
    link: Link,
    _board: LeasedBoard,
    _grant: Grant,
}

impl Session {
    /// Build `scenario`'s image, lease the board `selector` names (or the
    /// one device under test of the image's chip), write the image unless
    /// the board runs it, and open the link with a reset.
    pub(crate) fn open(scenario: &Scenario, selector: Option<&str>) -> Result<Self> {
        let root = oer_os_process::Checkout::discover("HIL scenario")?.root;
        let bundle = build(&root, scenario.image)?;
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
        let leased = oer_lab_board::stand::Board::attached(&root, &stand, &board.id)?.lease(access)?;
        write(&root, scenario.image, &bundle, &leased, &request.owner)?;
        let mut port = Port::open(leased.port(), DUT_CONSOLE)?;
        // Every supported chip restarts through its USB Serial/JTAG console.
        oer_lab_board::reset::reset_usb_serial_jtag(&mut port)?;
        let link = Link::start(Box::new(port))?;
        Ok(Self {
            link,
            _board: leased,
            _grant: grant,
        })
    }

    pub(crate) fn link(&self) -> &Link {
        &self.link
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
fn device_under_test<'a>(stand: &'a Stand, selector: Option<&str>) -> Result<&'a oer_lab_model::Board> {
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
        [] => Err(format!("the stand file has no {} board with the dut role", images::CHIP).into()),
        _ => Err(format!(
            "the stand file has several {} boards with the dut role; name one with --board",
            images::CHIP
        )
        .into()),
    }
}

/// Write `bundle` into `board` unless its receipt says it runs it, and
/// journal the write.
fn write(root: &Path, image: &str, bundle: &ImageBundle, board: &LeasedBoard, owner: &str) -> Result<()> {
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
