//! The scenario's session on the stand: its image built, its board leased
//! from the LAB service and written, and the link on the board's console.
//! [`Session::finish`] checks the device under test at the end of the
//! cases and [`Session::release`] ends the lease with the LAB service's
//! report of the board's end state; dropping the session releases the link
//! and the lease without that report, also when a stage of opening failed.
//! The service owns the board throughout: every write, reset and console
//! goes through it, and it alone brings the board to its end state.

use std::{
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::Path,
    path::PathBuf,
    sync::{Mutex, PoisonError},
    time::Duration,
};

use oer_tools_hil_link::Link;
use oer_tools_image_build::images;
use oer_tools_image_bundle::ImageBundle;
use oer_tools_lab_arbiter::{
    claims::{AIR, Claim},
    journal::ImageIdentity,
    readiness::ROUTER,
};
use oer_tools_lab_client::{
    client::{Client, Running},
    protocol::{FlashVia, LeaseRequest, Operation},
};
use oer_tools_lab_model::{BoardRole, Stand};

use crate::Scenario;
use crate::finish::{self, Cleanup, Ending, Waits};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// How finishing waits for the device under test.
const WAITS: Waits = Waits {
    probe: Duration::from_secs(2),
};

/// The features a scenario asks of its image, in Cargo's words.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct Choice {
    pub(crate) features: Vec<String>,
    pub(crate) default_features: bool,
}

/// What the command line names of the stand: each role's board, and the
/// injector's.
#[derive(Clone, Debug, Default)]
pub(crate) struct Selection {
    /// `role=board`, or a bare board for a scenario of one device.
    pub(crate) boards: Vec<String>,
    pub(crate) injector: Option<String>,
}

/// One device under test of the session. Declared in drop order: the link
/// closes its console before the lease ends with the client.
pub(crate) struct SessionDevice {
    link: Option<Link>,
    image_crc32: u32,
    /// What the image was built with, as its bundle records it.
    built: oer_tools_image_bundle::BuildChoice,
    board: StandBoard,
    /// The board's stand-file id.
    board_id: String,
}

impl SessionDevice {
    /// What the image was built with.
    pub(crate) fn built(&self) -> &oer_tools_image_bundle::BuildChoice {
        &self.built
    }

    /// The board's stand-file id.
    pub(crate) fn board_id(&self) -> &str {
        &self.board_id
    }

    /// The link the session opened, for the cases.
    pub(crate) fn take_link(&mut self) -> Option<Link> {
        self.link.take()
    }

    /// The board, for resets during the cases.
    pub(crate) fn board(&self) -> &StandBoard {
        &self.board
    }

    /// The built image's identity, which the board must report.
    pub(crate) fn image_crc32(&self) -> u32 {
        self.image_crc32
    }
}

pub(crate) struct Session {
    devices: Vec<SessionDevice>,
    equipment: std::rc::Rc<Equipment>,
    /// The checkout the scenario runs in.
    root: PathBuf,
}

/// What every board of the session shares: the lease's client and the
/// injector's line, while the lease holds one.
struct Equipment {
    client: Mutex<Client>,
    injector: Option<oer_tools_lab_client::peer::PeerLine>,
}

/// The choice a scenario's image was built with: a HIL image is a
/// package of this workspace.
///
/// # Errors
///
/// The bundle is an ESP-IDF catalog build, which has no choice of features.
pub(crate) fn package_choice(
    build: &oer_tools_image_bundle::Build,
) -> Result<oer_tools_image_bundle::BuildChoice> {
    match build {
        oer_tools_image_bundle::Build::Package(choice) => Ok(choice.clone()),
        oer_tools_image_bundle::Build::EspIdf => Err(
            "the scenario's image is an ESP-IDF catalog build, not a package of this workspace"
                .into(),
        ),
    }
}

impl Session {
    /// Resolve each role's board (the one device under test of the image's
    /// chip for a scenario of one), the injector's and the stand owner, then
    /// build each role's image, lease the boards, write each image unless its
    /// board runs it, and open each link with a reset. A selection or an
    /// owner that cannot be resolved fails before the build.
    pub(crate) fn open(
        scenario: &Scenario,
        selection: &Selection,
        choice: &Choice,
    ) -> Result<Self> {
        let root = oer_tools_os_process::Checkout::discover("HIL scenario")?.root;
        let stand = Stand::load(&oer_tools_lab_model::path()?)?;
        let (boards, injector, owner) = resolve(&stand, scenario, selection, || {
            Ok(oer_tools_lab_arbiter::owner::from_environment()?.to_string())
        })?;
        let mut bundles = Vec::with_capacity(boards.len());
        for role in scenario.roles() {
            let bundle = build(&root, role.image, choice)?;
            let built = package_choice(&bundle.build)?;
            let image_crc32 = bundle
                .staged
                .as_ref()
                .map(|staged| staged.runtime_crc32)
                .ok_or("the image is not staged: it has no identity to check")?;
            bundles.push((bundle, built, image_crc32));
        }
        let mut client = Client::open(&format!("hil {}", scenario.name))?;
        let names = boards
            .iter()
            .map(|board| board.id.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        eprintln!("hil: waiting for {names}");
        let macs = boards
            .iter()
            .map(|board| board.mac.clone())
            .collect::<Vec<_>>();
        client.acquire(
            LeaseRequest {
                owner: owner.clone(),
                work: format!("hil {}", scenario.name),
                scenarios: vec![scenario.name.to_owned()],
                run: None,
                claims: claims(
                    &macs,
                    injector.as_ref().map(|board| board.mac.as_str()),
                    scenario,
                ),
                maintenance: None,
                divisible: false,
            },
            &mut |message| eprintln!("hil: {message}"),
        )?;
        let injector = match &injector {
            Some(board) => Some(oer_tools_lab_client::peer::PeerLine::open(client.start(
                Operation::Console {
                    board: board.mac.clone(),
                    reset: false,
                },
            )?)),
            None => None,
        };
        let equipment = std::rc::Rc::new(Equipment {
            client: Mutex::new(client),
            injector,
        });
        let mut devices = Vec::with_capacity(boards.len());
        for ((board, role), (bundle, built, image_crc32)) in
            boards.iter().zip(scenario.roles()).zip(bundles)
        {
            let stand_board = StandBoard {
                equipment: std::rc::Rc::clone(&equipment),
                board: board.mac.clone(),
                owner: owner.clone(),
            };
            stand_board.write(&root, role.image, &bundle)?;
            let link = stand_board.link(true)?;
            devices.push(SessionDevice {
                link: Some(link),
                image_crc32,
                built,
                board: stand_board,
                board_id: board.id.clone(),
            });
        }
        Ok(Self {
            devices,
            equipment,
            root,
        })
    }

    /// The checkout the scenario runs in.
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// The devices under test, in the scenario's order of roles.
    pub(crate) fn devices(&mut self) -> &mut [SessionDevice] {
        &mut self.devices
    }

    /// How each device under test on its link ended the cases its ending
    /// describes; the links stay with the session until its lease ends. The
    /// first device left unclean is the session's cleanup.
    pub(crate) fn finish(&mut self, links: Vec<Option<Link>>, endings: &[Ending]) -> Cleanup {
        let mut cleanup = Cleanup::Clean;
        for ((device, link), ending) in self.devices.iter_mut().zip(links).zip(endings) {
            let ended = finish::finish(link.as_ref(), device.image_crc32, *ending, WAITS);
            device.link = link;
            if cleanup == Cleanup::Clean {
                cleanup = ended;
            }
        }
        cleanup
    }

    /// Close the links and the injector's line, and finish the lease: the
    /// LAB service stops its operations and confirms the boards' end state,
    /// or says why not.
    pub(crate) fn release(&mut self) -> std::result::Result<(), String> {
        for device in &mut self.devices {
            drop(device.link.take());
        }
        if let Some(injector) = &self.equipment.injector {
            let _ = injector.cancel();
        }
        self.equipment
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .release()
    }
}

/// A leased board, reached through the service.
pub(crate) struct StandBoard {
    equipment: std::rc::Rc<Equipment>,
    /// The board's MAC.
    board: String,
    owner: String,
}

impl StandBoard {
    fn client(&self) -> std::sync::MutexGuard<'_, Client> {
        self.equipment
            .client
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `operation` to its result, its output discarded.
    fn run(&self, operation: Operation) -> std::result::Result<serde_json::Value, String> {
        self.client().run(operation, &mut std::io::sink(), None)
    }

    /// Write `bundle` unless the board runs it; the service journals it.
    fn write(&self, root: &Path, image: &str, bundle: &ImageBundle) -> Result<()> {
        eprintln!("hil: writing {image} unless the board runs it");
        let git = |arguments: &[&str]| oer_tools_os_process::git::text(root, arguments).ok();
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

impl finish::Board for StandBoard {
    fn reset(&self) -> std::result::Result<(), String> {
        self.run(Operation::Reset {
            board: self.board.clone(),
            path: None,
        })
        .map(drop)
    }

    fn connect(&self) -> std::result::Result<Link, String> {
        self.link(false)
    }

    fn router(
        &self,
        request: oer_tools_lab_client::protocol::RouterOperation,
    ) -> std::result::Result<serde_json::Value, String> {
        self.run(Operation::Router { request })
    }

    fn injector(
        &self,
        command: &str,
        timeout: Duration,
    ) -> std::result::Result<oer_tools_lab_client::peer::Answer, String> {
        let Some(injector) = &self.equipment.injector else {
            return Err("the scenario's lease holds no injector".to_owned());
        };
        injector.send(command, timeout, &mut |line| eprintln!("injector: {line}"))
    }
}

/// What a run of `scenario` on the boards with `macs` leases: the boards,
/// the injector's board when its cases command it, the router exclusively
/// when its cases use it (they change its network), and the air as the
/// scenario uses it, at least shared with the router.
fn claims(macs: &[String], injector: Option<&str>, scenario: &Scenario) -> Vec<Claim> {
    let mut claims = macs.iter().map(|mac| Claim::board(mac)).collect::<Vec<_>>();
    if let Some(mac) = injector {
        claims.push(Claim::board(mac));
    }
    if scenario.router {
        claims.push(Claim::exclusive(ROUTER));
    }
    match scenario.air {
        Some(mode) => claims.push(Claim {
            resource: AIR.to_owned(),
            mode,
        }),
        None if scenario.router => claims.push(Claim::shared(AIR)),
        None => {}
    }
    claims
}

/// Build `image` through the image pipeline into its bundle.
fn build(root: &Path, image: &str, choice: &Choice) -> Result<ImageBundle> {
    eprintln!("hil: building {image}");
    let mut spec = images::spec(root, image, oer_tools_image_checks::Enforced::Reported)?;
    spec.application
        .features
        .extend(choice.features.iter().cloned());
    spec.application.default_features = choice.default_features;
    let bundle = oer_tools_image_build::build(&spec)?;
    for warning in &bundle.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(bundle)
}

/// Each role's board on `stand` as `selection` names it (the one device
/// under test of the image's chip for a scenario of one, named or not), the
/// injector's board when the scenario commands one, and the owner `owner`
/// resolves, in that order: everything a run needs from the stand before it
/// builds anything.
type Resolved = (
    Vec<oer_tools_lab_model::Board>,
    Option<oer_tools_lab_model::Board>,
    String,
);

fn resolve(
    stand: &Stand,
    scenario: &Scenario,
    selection: &Selection,
    owner: impl FnOnce() -> Result<String>,
) -> Result<Resolved> {
    let roles = scenario.roles();
    let named = |selector: &str| {
        stand
            .board(selector)
            .cloned()
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                format!("the stand file has no board `{selector}`").into()
            })
    };
    let boards = match (roles, selection.boards.as_slice()) {
        ([_], [selector]) if !selector.contains('=') => vec![named(selector)?],
        ([_], []) => vec![device_under_test(stand)?.clone()],
        _ => {
            let mut boards = Vec::with_capacity(roles.len());
            for role in roles {
                let prefix = format!("{}=", role.name);
                let Some(selector) = selection
                    .boards
                    .iter()
                    .find_map(|selector| selector.strip_prefix(prefix.as_str()))
                else {
                    return Err(format!(
                        "name the board of the role `{}` with --board {}=<board>",
                        role.name, role.name
                    )
                    .into());
                };
                boards.push(named(selector)?);
            }
            if let Some(unknown) = selection.boards.iter().find(|selector| {
                !roles
                    .iter()
                    .any(|role| selector.starts_with(&format!("{}=", role.name)))
            }) {
                return Err(format!("the scenario has no role for --board {unknown}").into());
            }
            boards
        }
    };
    let injector = match (scenario.injector, &selection.injector) {
        (true, Some(selector)) => Some(named(selector)?),
        (true, None) => {
            return Err(
                "the scenario commands the injector: name its board with --injector".into(),
            );
        }
        (false, Some(selector)) => {
            return Err(
                format!("the scenario commands no injector, not --injector {selector}").into(),
            );
        }
        (false, None) => None,
    };
    Ok((boards, injector, owner()?))
}

/// The one board of the image's chip that may be a device under test.
fn device_under_test(stand: &Stand) -> Result<&oer_tools_lab_model::Board> {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand with `duts` boards of the image's chip that may be the
    /// device under test.
    fn stand_with(duts: usize) -> Stand {
        let mut text = String::from(
            "schema = 2\n[stand]\nid = \"t\"\nair = \"exclusive\"\n\
             [[hub]]\nid = \"h\"\nusb2 = \"1-1\"\n",
        );
        for index in 0..duts {
            text.push_str(&format!(
                "[[board]]\nid = \"dut-{index}\"\nusb-serial = \"02:00:00:00:25:6{index}\"\n\
                 chip = \"{chip}\"\nradios = [\"wifi-2g4\"]\nroles = [\"dut\", \"peer\"]\n\
                 port = {{ hub = \"h\", port = {port} }}\nreset = [\"jtag\"]\n",
                chip = images::CHIP,
                port = index + 1,
            ));
        }
        Stand::parse(&text).unwrap()
    }

    const ONE: Scenario = Scenario::new("aes", "hil-aes", &[]);
    const PAIR_ROLES: &[crate::Role] = &[
        crate::Role::new("receiver", "esp32s31-rx-ring", &[]),
        crate::Role::new("sender", "esp32s31-case-c-sender", &[]),
    ];
    const PAIR: Scenario = Scenario::with_roles("pair", PAIR_ROLES).using_injector();

    fn selecting(boards: &[&str], injector: Option<&str>) -> Selection {
        Selection {
            boards: boards.iter().map(|board| (*board).to_owned()).collect(),
            injector: injector.map(str::to_owned),
        }
    }

    fn ids(boards: &[oer_tools_lab_model::Board]) -> Vec<&str> {
        boards.iter().map(|board| board.id.as_str()).collect()
    }

    #[test]
    fn an_ambiguous_board_selection_fails_before_the_owner_or_the_build() {
        let stand = stand_with(2);
        let mut asked = false;
        let error = resolve(&stand, &ONE, &selecting(&[], None), || {
            asked = true;
            Ok("owner".to_owned())
        })
        .unwrap_err();
        assert!(
            error.to_string().contains("name one with --board"),
            "{error}"
        );
        assert!(!asked, "the owner is asked only for a resolved board");
        let error = resolve(&stand, &ONE, &selecting(&["dut-9"], None), || {
            Ok("owner".to_owned())
        })
        .unwrap_err();
        assert!(error.to_string().contains("no board `dut-9`"), "{error}");
    }

    #[test]
    fn a_named_or_single_board_resolves_with_its_owner() {
        let stand = stand_with(2);
        let (boards, injector, owner) = resolve(&stand, &ONE, &selecting(&["dut-1"], None), || {
            Ok("owner".to_owned())
        })
        .unwrap();
        assert_eq!((ids(&boards), owner.as_str()), (vec!["dut-1"], "owner"));
        assert!(injector.is_none());
        let single = stand_with(1);
        let (boards, ..) = resolve(&single, &ONE, &selecting(&[], None), || {
            Ok("owner".to_owned())
        })
        .unwrap();
        assert_eq!(ids(&boards), ["dut-0"]);
        let error = resolve(&single, &ONE, &selecting(&[], None), || {
            Err("no stand owner is registered".into())
        })
        .unwrap_err();
        assert!(error.to_string().contains("no stand owner"), "{error}");
    }

    #[test]
    fn each_role_names_its_board_and_an_injector_is_named_only_when_commanded() {
        let stand = stand_with(3);
        let owner = || Ok("owner".to_owned());
        let (boards, injector, _) = resolve(
            &stand,
            &PAIR,
            &selecting(&["sender=dut-1", "receiver=dut-0"], Some("dut-2")),
            owner,
        )
        .unwrap();
        assert_eq!(ids(&boards), ["dut-0", "dut-1"], "the scenario's order");
        assert_eq!(injector.map(|board| board.id), Some("dut-2".to_owned()));
        for (selection, expected) in [
            (
                selecting(&["receiver=dut-0"], Some("dut-2")),
                "--board sender=<board>",
            ),
            (
                selecting(
                    &["receiver=dut-0", "sender=dut-1", "other=dut-2"],
                    Some("dut-2"),
                ),
                "no role for --board other=dut-2",
            ),
            (
                selecting(&["receiver=dut-0", "sender=dut-1"], None),
                "name its board with --injector",
            ),
        ] {
            let error = resolve(&stand, &PAIR, &selection, owner).unwrap_err();
            assert!(error.to_string().contains(expected), "{error}");
        }
        let error =
            resolve(&stand, &ONE, &selecting(&["dut-0"], Some("dut-2")), owner).unwrap_err();
        assert!(
            error.to_string().contains("commands no injector"),
            "{error}"
        );
    }

    #[test]
    fn a_run_leases_the_air_only_for_a_scenario_that_turns_a_radio_on() {
        let mac = "02:00:00:00:00:01";
        let macs = [mac.to_owned()];
        let quiet = Scenario::new("aes", "hil-aes", &[]);
        assert_eq!(claims(&macs, None, &quiet), [Claim::board(mac)]);
        assert_eq!(
            claims(&macs, None, &quiet.using_air()),
            [Claim::board(mac), Claim::shared(AIR)]
        );
        assert_eq!(
            claims(&macs, None, &quiet.using_router()),
            [
                Claim::board(mac),
                Claim::exclusive(ROUTER),
                Claim::shared(AIR)
            ]
        );
        assert_eq!(
            claims(&macs, None, &quiet.using_router().using_air_exclusively()),
            [
                Claim::board(mac),
                Claim::exclusive(ROUTER),
                Claim::exclusive(AIR)
            ]
        );
    }

    #[test]
    fn a_pair_leases_both_boards_the_injector_and_the_air() {
        let macs = [
            "02:00:00:00:00:01".to_owned(),
            "02:00:00:00:00:02".to_owned(),
        ];
        let pair = PAIR.using_air_exclusively();
        assert_eq!(
            claims(&macs, Some("02:00:00:00:00:03"), &pair),
            [
                Claim::board("02:00:00:00:00:01"),
                Claim::board("02:00:00:00:00:02"),
                Claim::board("02:00:00:00:00:03"),
                Claim::exclusive(AIR)
            ]
        );
    }

    #[test]
    fn two_router_runs_on_different_boards_conflict_on_the_router() {
        use oer_tools_lab_arbiter::claims::{conflict, normalize};
        let station = Scenario::new("wifi-station", "image", &[]).using_router();
        let one = normalize(&claims(&["02:00:00:00:00:01".to_owned()], None, &station));
        let other = normalize(&claims(&["02:00:00:00:00:02".to_owned()], None, &station));
        assert!(conflict(&one, &other), "both change the router's network");
        let quiet = Scenario::new("aes", "hil-aes", &[]);
        assert!(!conflict(
            &one,
            &normalize(&claims(&["02:00:00:00:00:02".to_owned()], None, &quiet))
        ));
    }
}
