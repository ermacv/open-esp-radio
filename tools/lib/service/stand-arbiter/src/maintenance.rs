//! Boards taken out of service.
//!
//! A board under maintenance serves only the owner who took it out: every
//! other request that claims it, or the whole stand, is refused with the
//! reason instead of queueing. A quarantined board serves nobody: the runner
//! quarantines a board that no script can bring back to a flashable state,
//! and only a person who reset or power-cycled it returns it, after it
//! answers again. Maintenance never stops a lease already held;
//! it keeps later ones away. The state is `maintenance.json` beside the
//! queue, so every checkout of the host user sees it.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::Arbiter;
use oer_stand_claims::Claim;
use oer_stand_journal::Confirmation;

const SCHEMA: u32 = 1;

/// The owner of every quarantine: no request is ever made by it.
pub const QUARANTINE_OWNER: &str = "quarantine";

/// The `mac` of a maintenance entry that takes the whole stand out of
/// service.
pub const STAND_SERVICE: &str = "stand";

/// How often a request or `cargo stand wait` reads whether what it
/// waits for is back in service.
pub const SERVICE_POLL: std::time::Duration = std::time::Duration::from_secs(5);

/// Why a board is out of service.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ServiceKind {
    /// Taken out by an owner, who keeps using it.
    #[default]
    Maintenance,
    /// Taken out by the stand until a person resets or power-cycles it.
    Quarantine,
}

/// What made the stand quarantine a board.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum QuarantineTrigger {
    /// It stayed unreachable after every automatic recovery step.
    Unreachable,
    /// An older runner's quarantine of a board it recovered often; no build
    /// sets it now, since a board the stand recovers needs no person.
    Flaky,
    /// Its ROM answers, but its bootloader resets in a loop that no reset
    /// the stand has (RTS, JTAG, EN, power) cleared.
    BootLoop,
}

/// One board out of service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Maintenance {
    /// The board's MAC.
    pub mac: String,
    /// The only owner whose requests may claim the board.
    pub owner: String,
    pub reason: String,
    pub since_unix: u64,
    #[serde(default)]
    pub kind: ServiceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trigger: Option<QuarantineTrigger>,
    /// Where the stand kept what it saw before and during recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub unknown: crate::Unknown,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct Boards {
    schema: u32,
    boards: Vec<Maintenance>,
    #[serde(flatten)]
    unknown: crate::Unknown,
}

impl Arbiter {
    fn maintenance_path(&self) -> std::path::PathBuf {
        self.directory().join("maintenance.json")
    }

    /// The boards out of service.
    pub fn maintenance(&self) -> crate::Result<Vec<Maintenance>> {
        let path = self.maintenance_path();
        self.locked(|| read(&path).map(|boards| boards.boards))
    }

    /// Take a board out of service for `entry.owner`, replacing an earlier
    /// entry of the same board.
    pub fn set_maintenance(&self, entry: Maintenance) -> crate::Result<()> {
        let path = self.maintenance_path();
        self.locked(|| {
            let mut boards = read(&path)?;
            boards.boards.retain(|known| known.mac != entry.mac);
            boards.boards.push(entry);
            write(&path, &boards)
        })
    }

    /// Quarantine the board with `mac`: no request is granted it until
    /// [`Self::release_quarantine`]. The user is notified.
    pub fn quarantine(
        &self,
        mac: &str,
        trigger: QuarantineTrigger,
        reason: String,
        evidence: Option<String>,
    ) -> crate::Result<()> {
        crate::notify::send(
            "HIL board quarantined",
            &format!(
                "{mac}: {reason}; press RST or power-cycle it, then `cargo stand devices release {mac} --confirm reset|power-cycle`"
            ),
        );
        eprintln!(
            "hil-arbiter: board {mac} quarantined ({trigger:?}): {reason}{}",
            evidence
                .as_deref()
                .map(|path| format!("; evidence in {path}"))
                .unwrap_or_default()
        );
        self.set_maintenance(Maintenance {
            mac: mac.to_owned(),
            owner: QUARANTINE_OWNER.to_owned(),
            reason,
            since_unix: oer_durable::unix_seconds(),
            kind: ServiceKind::Quarantine,
            trigger: Some(trigger),
            evidence,
            unknown: crate::Unknown::default(),
        })
    }

    /// Whether the board with `mac` is quarantined.
    pub fn is_quarantined(&self, mac: &str) -> crate::Result<bool> {
        Ok(self
            .maintenance()?
            .iter()
            .any(|entry| entry.mac == mac && entry.kind == ServiceKind::Quarantine))
    }

    /// Return a quarantined board to service after a person's `confirmation`,
    /// recorded in the board journal; `healthy` is the caller's check that
    /// it answers again, and the board stays quarantined when it fails.
    pub fn release_quarantine(
        &self,
        mac: &str,
        owner: &str,
        confirmation: Confirmation,
        healthy: impl FnOnce() -> crate::Result<String>,
    ) -> crate::Result<String> {
        if !self.is_quarantined(mac)? {
            return Err(format!("board {mac} is not quarantined").into());
        }
        let answer = healthy().map_err(|error| {
            format!("board {mac} still does not answer; it stays quarantined: {error}")
        })?;
        self.journal().record_by(
            owner.to_owned(),
            Some(mac.to_owned()),
            oer_stand_journal::BoardEventKind::QuarantineReleased {
                confirmation,
                check: answer.clone(),
            },
        )?;
        self.clear_maintenance(mac)?;
        Ok(answer)
    }

    /// What keeps the boards with `macs` (every board when empty) out of
    /// service: their own entries and a whole-stand one.
    pub fn out_of_service(&self, macs: &[String]) -> crate::Result<Vec<Maintenance>> {
        Ok(affecting(self.maintenance()?, macs))
    }

    /// Block until nothing in [`Self::out_of_service`] remains for `macs`,
    /// calling `waiting` with what is out of service each time it changes.
    pub fn wait_for_service(
        &self,
        macs: &[String],
        mut waiting: impl FnMut(&[Maintenance]),
    ) -> crate::Result<()> {
        let mut shown: Option<Vec<Maintenance>> = None;
        loop {
            let out = self.out_of_service(macs)?;
            if out.is_empty() {
                return Ok(());
            }
            if shown.as_ref() != Some(&out) {
                waiting(&out);
                shown = Some(out);
            }
            oer_process::check_cancelled()?;
            std::thread::sleep(SERVICE_POLL);
        }
    }

    /// Return the board with `mac` to service; whether it was out of service.
    pub fn clear_maintenance(&self, mac: &str) -> crate::Result<bool> {
        let path = self.maintenance_path();
        self.locked(|| {
            let mut boards = read(&path)?;
            let before = boards.boards.len();
            boards.boards.retain(|known| known.mac != mac);
            let cleared = boards.boards.len() != before;
            write(&path, &boards)?;
            Ok(cleared)
        })
    }
}

/// Why `owner` may not claim `claims`: a board, or the whole stand, holding
/// a board under another owner's maintenance.
pub(crate) fn refusal(boards: &[Maintenance], owner: &str, claims: &[Claim]) -> Option<String> {
    let whole = claims
        .iter()
        .any(|claim| claim.resource == oer_stand_claims::STAND);
    boards
        .iter()
        .filter(|board| board.kind == ServiceKind::Quarantine || board.owner != owner)
        .find(|board| {
            whole
                || board.mac == STAND_SERVICE
                || claims
                    .iter()
                    .any(|claim| claim.resource == Claim::board(&board.mac).resource)
        })
        .map(|board| match board.kind {
            _ if board.mac == STAND_SERVICE => format!(
                "the stand is under maintenance by {}: {}; it serves only its maintainer until \
                 `cargo stand devices release --stand`; `cargo stand wait` blocks until then",
                board.owner, board.reason
            ),
            ServiceKind::Quarantine => format!(
                "board {} is quarantined: {}; a person must reset or power-cycle it and run \
                 `cargo stand devices release {} --confirm reset|power-cycle`",
                board.mac, board.reason, board.mac
            ),
            ServiceKind::Maintenance => format!(
                "board {} is under maintenance by {}: {}; it serves only its maintainer until \
                 `cargo stand devices release`",
                board.mac, board.owner, board.reason
            ),
        })
}

/// The entries of `boards` that keep `macs` (every board when empty) out of
/// service.
fn affecting(boards: Vec<Maintenance>, macs: &[String]) -> Vec<Maintenance> {
    boards
        .into_iter()
        .filter(|board| macs.is_empty() || board.mac == STAND_SERVICE || macs.contains(&board.mac))
        .collect()
}

fn read(path: &std::path::Path) -> crate::Result<Boards> {
    match fs::read(path) {
        Ok(bytes) => {
            let boards: Boards = serde_json::from_slice(&bytes)?;
            if boards.schema != SCHEMA {
                return Err(format!(
                    "{} has schema {}; update this checkout",
                    path.display(),
                    boards.schema
                )
                .into());
            }
            Ok(boards)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Boards {
            schema: SCHEMA,
            boards: Vec::new(),
            unknown: Default::default(),
        }),
        Err(error) => Err(error.into()),
    }
}

fn write(path: &std::path::Path, boards: &Boards) -> crate::Result<()> {
    oer_durable::atomic_write(path, &serde_json::to_vec_pretty(boards)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board(mac: &str, owner: &str) -> Maintenance {
        Maintenance {
            mac: mac.into(),
            owner: owner.into(),
            reason: "USB hangs after a reset".into(),
            since_unix: 1,
            kind: ServiceKind::Maintenance,
            trigger: None,
            evidence: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn a_whole_stand_maintenance_refuses_every_other_owner() {
        let boards = [board(STAND_SERVICE, "stand")];
        let refused = refusal(&boards, "802154", &[Claim::board("AA")]).unwrap();
        assert!(
            refused.contains("the stand is under maintenance"),
            "{refused}"
        );
        assert_eq!(refusal(&boards, "stand", &[Claim::board("AA")]), None);
    }

    #[test]
    fn out_of_service_names_the_boards_asked_for_and_the_stand() {
        let boards = vec![board("AA", "x"), board("BB", "y")];
        let macs = |out: Vec<Maintenance>| out.into_iter().map(|b| b.mac).collect::<Vec<_>>();
        assert_eq!(macs(affecting(boards.clone(), &[])), ["AA", "BB"]);
        assert_eq!(macs(affecting(boards.clone(), &["BB".into()])), ["BB"]);
        assert!(affecting(boards.clone(), &["CC".into()]).is_empty());
        let mut stand = boards;
        stand.push(board(STAND_SERVICE, "z"));
        assert_eq!(macs(affecting(stand, &["CC".into()])), [STAND_SERVICE]);
    }

    #[test]
    fn waiting_for_service_returns_once_the_board_is_released() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        arbiter.set_maintenance(board("AA", "x")).unwrap();
        let mut seen = Vec::new();
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(std::time::Duration::from_millis(200));
                arbiter.clear_maintenance("AA").unwrap();
            });
            arbiter
                .wait_for_service(&["AA".into()], |out| seen.push(out.len()))
                .unwrap();
        });
        assert_eq!(seen, [1]);
        // Nothing out of service: no wait at all.
        arbiter
            .wait_for_service(&[], |_| panic!("nothing to wait for"))
            .unwrap();
    }

    #[test]
    fn a_quarantined_board_serves_nobody_until_a_checked_release() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        arbiter
            .quarantine(
                "AA",
                QuarantineTrigger::Unreachable,
                "no answer after an EN reset".into(),
                Some("/runs/x/post-mortem".into()),
            )
            .unwrap();
        let boards = arbiter.maintenance().unwrap();
        let claims = [Claim::board("AA")];
        for owner in ["802154", QUARANTINE_OWNER, "stand"] {
            assert!(
                refusal(&boards, owner, &claims)
                    .unwrap()
                    .contains("quarantined"),
                "{owner}"
            );
        }
        let still = arbiter.release_quarantine("AA", "user", Confirmation::Reset, || {
            Err("no ROM line".into())
        });
        assert!(still.is_err());
        assert!(arbiter.is_quarantined("AA").unwrap());
        let answer = arbiter
            .release_quarantine("AA", "user", Confirmation::PowerCycle, || {
                Ok(String::from("rst:0x1 (POWERON)"))
            })
            .unwrap();
        assert_eq!(answer, "rst:0x1 (POWERON)");
        assert!(!arbiter.is_quarantined("AA").unwrap());
        assert!(
            arbiter
                .journal()
                .events()
                .unwrap()
                .iter()
                .any(|event| matches!(
                    event.kind,
                    oer_stand_journal::BoardEventKind::QuarantineReleased {
                        confirmation: Confirmation::PowerCycle,
                        ..
                    }
                ))
        );
    }

    #[test]
    fn a_board_whose_rom_answers_returns_without_a_person_only_after_it_answers() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        arbiter
            .quarantine(
                "AA",
                QuarantineTrigger::Unreachable,
                "old runner".into(),
                None,
            )
            .unwrap();
        assert!(
            arbiter
                .release_quarantine("AA", "stand", Confirmation::RomAnswers, || {
                    Err("no ROM line".into())
                })
                .is_err()
        );
        assert!(arbiter.is_quarantined("AA").unwrap());
        arbiter
            .release_quarantine("AA", "stand", Confirmation::RomAnswers, || {
                Ok(String::from("rst:0x17 (CHIP_USB_UART_RESET),boot:0x58"))
            })
            .unwrap();
        assert!(!arbiter.is_quarantined("AA").unwrap());
        assert_eq!(
            serde_json::to_value(Confirmation::RomAnswers).unwrap(),
            "rom-answers"
        );
    }

    #[test]
    fn a_board_under_maintenance_serves_only_its_maintainer() {
        let boards = [board("AA", "stand")];
        let on = |mac: &str| vec![Claim::board(mac), Claim::shared(oer_stand_claims::AIR)];
        assert!(
            refusal(&boards, "802154", &on("AA"))
                .unwrap()
                .contains("maintenance by stand")
        );
        assert_eq!(refusal(&boards, "stand", &on("AA")), None);
        assert_eq!(refusal(&boards, "802154", &on("BB")), None, "another board");
        assert!(
            refusal(&boards, "802154", &[Claim::stand()]).is_some(),
            "the whole stand"
        );
        assert_eq!(refusal(&[], "802154", &on("AA")), None);
    }

    #[test]
    fn maintenance_is_set_replaced_and_cleared() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        assert!(arbiter.maintenance().unwrap().is_empty());
        arbiter.set_maintenance(board("AA", "stand")).unwrap();
        arbiter.set_maintenance(board("AA", "other")).unwrap();
        assert_eq!(arbiter.maintenance().unwrap(), [board("AA", "other")]);
        assert!(arbiter.clear_maintenance("AA").unwrap());
        assert!(!arbiter.clear_maintenance("AA").unwrap());
    }

    #[test]
    fn fields_a_newer_build_wrote_survive_a_rewrite() {
        let directory = tempfile::tempdir().unwrap();
        let arbiter = Arbiter::at(directory.path()).unwrap();
        fs::write(
            directory.path().join("maintenance.json"),
            serde_json::to_vec(&serde_json::json!({
                "schema": SCHEMA, "future": 1,
                "boards": [{"mac": "AA", "owner": "stand", "reason": "r", "since_unix": 1,
                    "until_unix": 2}],
            }))
            .unwrap(),
        )
        .unwrap();
        arbiter.set_maintenance(board("BB", "stand")).unwrap();
        let stored: serde_json::Value =
            serde_json::from_slice(&fs::read(directory.path().join("maintenance.json")).unwrap())
                .unwrap();
        assert_eq!(stored["future"], 1);
        assert_eq!(stored["boards"][0]["until_unix"], 2);
    }
}
