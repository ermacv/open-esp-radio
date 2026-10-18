//! A board's health at a glance, from the stand's own records.
//!
//! Health combines what the stand knows without touching the board: whether
//! it is attached, out of service or quarantined, and how often the stand had
//! to recover it within [`RECOVERY_WINDOW`].

use serde::Serialize;

use oer_stand_journal::{BoardEvent, BoardEventKind};

use crate::{Maintenance, maintenance::ServiceKind};

/// The window over which a board's recoveries count towards its health.
pub const RECOVERY_WINDOW: std::time::Duration = std::time::Duration::from_secs(3600);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HealthState {
    /// Attached, in service, and not recovered recently.
    Ok,
    /// In service, but the stand recovered it within the window.
    RecoveredRecently,
    /// Reserved for one owner.
    Maintenance,
    /// Out of service until a person resets or power-cycles it.
    Quarantined,
    /// Registered or journaled, but not attached now.
    NotAttached,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
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
    let service = maintenance.iter().find(|entry| entry.mac == mac);
    let state = match service.map(|entry| entry.kind) {
        Some(ServiceKind::Quarantine) => HealthState::Quarantined,
        Some(_) => HealthState::Maintenance,
        None if !attached => HealthState::NotAttached,
        None if !recent.is_empty() => HealthState::RecoveredRecently,
        None => HealthState::Ok,
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
    use crate::QuarantineTrigger;
    use oer_devices::reset::RecoveryStep;

    const MAC: &str = "xx:xx:xx:xx:F6:D0";

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

    fn service(kind: ServiceKind) -> Maintenance {
        Maintenance {
            mac: MAC.into(),
            owner: String::from("stand"),
            reason: String::from("why"),
            since_unix: 0,
            kind,
            trigger: Some(QuarantineTrigger::Unreachable),
            evidence: None,
            unknown: Default::default(),
        }
    }

    #[test]
    fn health_names_the_worst_known_state_and_recent_recoveries() {
        let now = 10_000;
        assert_eq!(of(MAC, true, &[], &[], now).state, HealthState::Ok);
        assert_eq!(
            of(MAC, false, &[], &[], now).state,
            HealthState::NotAttached
        );
        // A recovery older than the window is history, not health.
        let old = [recovered(now - 7200, true)];
        let health = of(MAC, true, &[], &old, now);
        assert_eq!(
            (health.state, health.recoveries, health.last_recovery_unix),
            (HealthState::Ok, 0, Some(now - 7200))
        );
        let recent = [recovered(now - 60, true), recovered(now - 30, false)];
        let health = of(MAC, true, &[], &recent, now);
        assert_eq!(health.state, HealthState::RecoveredRecently);
        assert_eq!((health.recoveries, health.hardware_recoveries), (2, 1));
        assert_eq!(
            health.to_string(),
            "recovered recently (2 recoveries, 1 hardware, in the last hour)"
        );
        let quarantined = of(MAC, true, &[service(ServiceKind::Quarantine)], &recent, now);
        assert_eq!(quarantined.state, HealthState::Quarantined);
        assert_eq!(
            of(MAC, false, &[service(ServiceKind::default())], &[], now).state,
            HealthState::Maintenance
        );
    }
}
