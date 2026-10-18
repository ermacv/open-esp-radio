//! Fault-close evidence is separate from normal reconnect and from RF loss.
use super::{Duration, Evidence, Instant, Observation, Owner, Result};
use oer_hil_protocol::{
    bluetooth::BluetoothGattResetOutcome as Reset, bluetooth::BluetoothGattShutdown,
    bluetooth::BluetoothGattStopCause as Cause,
};

fn require_closed(before: Evidence, after: Evidence) -> Result<()> {
    if after.shutdown
        != Some(BluetoothGattShutdown {
            cause: Cause::InjectedBondLoadFailure,
            reset: Reset::Completed,
        })
        || !after.application_stopped
        || after.restarting
        || after.epoch != before.epoch
        || after.cold_releases != before.cold_releases + 1
        || !after.old_hci_closed
        || after.bond_load_fault_armed
        || after.bond_load_failures != 1
        || after.traffic.connected
        || after.traffic.advertising
        || after.traffic.disconnections != before.traffic.disconnections + 1
        || after.traffic.connections != before.traffic.connections
        || after.traffic.advertising_starts != before.traffic.advertising_starts
        || after.comparisons != before.comparisons
        || after.bonds_stored != before.bonds_stored
        || after.bonds_resumed != before.bonds_resumed
        || after.traffic.reads != before.traffic.reads
        || after.traffic.writes != before.traffic.writes
        || after.notifications_queued != before.notifications_queued
    {
        return Err(format!(
            "bond-load failure did not reach checked terminal cold ownership: {after:?}"
        )
        .into());
    }
    Ok(())
}

pub(super) fn bond_load_failure(
    capture: &Observation<'_>,
    peer: &Owner,
    samples: &mut Vec<Evidence>,
    boot: u64,
    before: Evidence,
) -> Result<()> {
    crate::link::fail_next_gatt_bond_load(capture, boot, before.epoch)?;
    // The real disconnect causes the unchanged application to reload its store.
    peer.disconnect()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let e = capture.secure_gatt()?;
        samples.push(e);
        if e.cold_releases > before.cold_releases {
            require_closed(before, e)?;
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("fault-close deadline: {e:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
    crate::link::require_gatt_restart_rejected(capture, boot, before.epoch)?;
    // Observation window only: never a production shutdown timeout or RF bound.
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        let e = capture.secure_gatt()?;
        samples.push(e);
        require_closed(before, e)?;
        if capture.latest_boot_id() != Some(boot)
            || !e
                .traffic
                .cpu0_stack
                .is_some_and(|s| s.has_required_headroom())
        {
            return Err("fault shutdown rebooted or lost stack headroom evidence".into());
        }
        if Instant::now() >= until {
            break;
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn failure_or_reset_alone_is_not_cold_shutdown_evidence() {
        let before = Evidence {
            epoch: 2,
            cold_releases: 1,
            ..Default::default()
        };
        let after = Evidence {
            shutdown: Some(BluetoothGattShutdown {
                cause: Cause::InjectedBondLoadFailure,
                reset: Reset::Completed,
            }),
            application_stopped: true,
            epoch: 2,
            cold_releases: 2,
            old_hci_closed: true,
            bond_load_failures: 1,
            traffic: oer_hil_protocol::bluetooth::BluetoothGattEvidence {
                disconnections: 1,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(require_closed(before, after).is_ok());
        for invalid in [
            Evidence {
                cold_releases: 1,
                ..after
            },
            Evidence { epoch: 3, ..after },
            Evidence {
                old_hci_closed: false,
                ..after
            },
            Evidence {
                bond_load_failures: 0,
                ..after
            },
            Evidence {
                shutdown: Some(BluetoothGattShutdown {
                    cause: Cause::Requested,
                    reset: Reset::Completed,
                }),
                ..after
            },
            Evidence {
                shutdown: Some(BluetoothGattShutdown {
                    cause: Cause::InjectedBondLoadFailure,
                    reset: Reset::CommandFailed,
                }),
                ..after
            },
        ] {
            assert!(require_closed(before, invalid).is_err());
        }
    }
}
