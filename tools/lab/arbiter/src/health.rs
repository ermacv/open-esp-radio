//! A board's health at a glance, from the stand's own records.
//!
//! Health combines what the stand knows without touching the board: whether
//! it is attached, out of service or quarantined, and how often the stand had
//! to recover it within [`RECOVERY_WINDOW`].

use serde::{Deserialize, Serialize};

use crate::journal::{BoardEvent, BoardEventKind};

use crate::{Maintenance, readiness::Readiness};

/// The window over which a board's recoveries count towards its health.
pub const RECOVERY_WINDOW: std::time::Duration = std::time::Duration::from_secs(3600);

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    /// Attached, in service, and not recovered recently.
    Ok,
    /// In service, but the stand recovered it within the window.
    RecoveredRecently,
    /// Reserved for one owner.
    Maintenance,
    /// No driver brought it to an end state: unavailable until a person
    /// returns it.
    Quarantined,
    /// Registered or journaled, but not attached now.
    NotAttached,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Health {
    pub state: HealthState,
    /// Automatic recoveries within the window, and how many of them were
    /// hardware-level.
    pub recoveries: usize,
    pub hardware_recoveries: usize,
    /// When the stand last recovered the board.
    pub last_recovery_unix: Option<u64>,
}

impl std::fmt::Display for Health {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = match self.state {
            HealthState::Ok => "ok",
            HealthState::RecoveredRecently => "recovered recently",
            HealthState::Maintenance => "maintenance",
            HealthState::Quarantined => "QUARANTINED",
            HealthState::NotAttached => "not attached",
        };
        formatter.write_str(state)?;
        if self.recoveries > 0 {
            write!(
                formatter,
                " ({} recoveries, {} hardware, in the last hour)",
                self.recoveries, self.hardware_recoveries
            )?;
        }
        Ok(())
    }
}

/// The health of the board with `mac` at `now_unix`.
pub(crate) fn of(
    mac: &str,
    attached: bool,
    readiness: Option<&Readiness>,
    maintenance: &[Maintenance],
    events: &[BoardEvent],
    now_unix: u64,
) -> Health {
    let since = now_unix.saturating_sub(RECOVERY_WINDOW.as_secs());
    let recoveries = events
        .iter()
        .filter(|event| event.device.as_deref() == Some(mac))
        .filter_map(|event| match event.kind {
            BoardEventKind::Recovered { hardware, .. } => Some((event.unix, hardware)),
            _ => None,
        })
        .collect::<Vec<_>>();
    let recent = recoveries
        .iter()
        .filter(|(unix, _)| *unix >= since)
        .collect::<Vec<_>>();
    let maintained = maintenance.iter().any(|entry| entry.mac == mac);
    let state = match readiness {
        Some(Readiness::Unavailable { .. }) => HealthState::Quarantined,
        _ if maintained => HealthState::Maintenance,
        _ if !attached => HealthState::NotAttached,
        _ if !recent.is_empty() => HealthState::RecoveredRecently,
        _ => HealthState::Ok,
    };
    Health {
        state,
        recoveries: recent.len(),
        hardware_recoveries: recent.iter().filter(|(_, hardware)| *hardware).count(),
        last_recovery_unix: recoveries.iter().map(|(unix, _)| *unix).max(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_lab_model::rom::RecoveryStep;

    const MAC: &str = "02:00:00:00:F6:D0";

    fn recovered(unix: u64, hardware: bool) -> BoardEvent {
        BoardEvent {
            unix,
            owner: String::from("stand"),
            checkout: None,
            device: Some(MAC.into()),
            kind: BoardEventKind::Recovered {
                step: RecoveryStep::RtsReset,
                hardware,
                reset_line: None,
                origin: String::from("run"),
            },
        }
    }

    fn maintained() -> Maintenance {
        Maintenance {
            mac: MAC.into(),
            owner: String::from("ann"),
            reason: String::from("why"),
            since_unix: 0,
            unknown: Default::default(),
        }
    }

    #[test]
    fn health_names_the_worst_known_state_and_recent_recoveries() {
        let now = 10_000;
        assert_eq!(of(MAC, true, None, &[], &[], now).state, HealthState::Ok);
        assert_eq!(
            of(MAC, false, None, &[], &[], now).state,
            HealthState::NotAttached
        );
        // A recovery older than the window is history, not health.
        let old = [recovered(now - 7200, true)];
        let health = of(MAC, true, None, &[], &old, now);
        assert_eq!(
            (health.state, health.recoveries, health.last_recovery_unix),
            (HealthState::Ok, 0, Some(now - 7200))
        );
        let recent = [recovered(now - 60, true), recovered(now - 30, false)];
        let health = of(MAC, true, None, &[], &recent, now);
        assert_eq!(health.state, HealthState::RecoveredRecently);
        assert_eq!((health.recoveries, health.hardware_recoveries), (2, 1));
        assert_eq!(
            health.to_string(),
            "recovered recently (2 recoveries, 1 hardware, in the last hour)"
        );
        let unavailable = Readiness::Unavailable {
            reason: String::from("silent"),
            emission: crate::readiness::Emission::Unknown,
            generation: 1,
        };
        let quarantined = of(MAC, true, Some(&unavailable), &[], &recent, now);
        assert_eq!(quarantined.state, HealthState::Quarantined);
        assert_eq!(
            of(MAC, false, None, &[maintained()], &[], now).state,
            HealthState::Maintenance
        );
    }
}
