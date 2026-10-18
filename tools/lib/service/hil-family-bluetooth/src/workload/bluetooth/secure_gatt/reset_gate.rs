//! Observe a pending software Reset without treating it as an RF-close proof.
use super::{Duration, Evidence, Instant, Observation, Result, sample, wait};
use oer_hil_protocol::bluetooth::BluetoothGattResetReadGate as Phase;

fn require_held(before: Evidence, held: Evidence) -> Result<()> {
    if held.reset_read_gate != Phase::ReaderHeld
        || !held.restarting
        || held.application_stopped
        || held.shutdown.is_some()
        || held.epoch != before.epoch
        || held.cold_releases != before.cold_releases
        || held.old_hci_closed != before.old_hci_closed
        || held.traffic.connections != before.traffic.connections
        || held.traffic.disconnections != before.traffic.disconnections
        || held.traffic.advertising_starts != before.traffic.advertising_starts
        || held.bonds_stored != before.bonds_stored
        || held.comparisons != before.comparisons
    {
        return Err(
            format!("Reset reader suspension did not retain the old epoch: {held:?}").into(),
        );
    }
    Ok(())
}

pub(super) fn restart(
    capture: &Observation<'_>,
    samples: &mut Vec<Evidence>,
    boot: u64,
    before: Evidence,
) -> Result<()> {
    crate::link::gatt_reset_read_gate(capture, boot, before.epoch, false)?;
    crate::link::restart_gatt(capture, boot, before.epoch)?;
    let held = wait(capture, samples, |e| e.reset_read_gate == Phase::ReaderHeld)?;
    require_held(before, held)?;
    crate::link::require_gatt_restart_rejected(capture, boot, before.epoch)?;
    // Only an observation interval, never a production timeout or PHY bound.
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        let e = sample(capture, samples)?;
        require_held(before, e)?;
        if capture.latest_boot_id() != Some(boot) {
            return Err("Reset wait rebooted the SoC".into());
        }
        if Instant::now() >= until {
            break;
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
    crate::link::gatt_reset_read_gate(capture, boot, before.epoch, true)
    // The caller still requires real cold restart and independent encrypted traffic.
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn held_checkpoint_is_not_just_an_arm_acknowledgement() {
        let before = Evidence {
            epoch: 1,
            ..Default::default()
        };
        let held = Evidence {
            restarting: true,
            reset_read_gate: Phase::ReaderHeld,
            ..before
        };
        assert!(require_held(before, held).is_ok());
        for invalid in [
            Evidence {
                reset_read_gate: Phase::Armed,
                ..held
            },
            Evidence {
                reset_read_gate: Phase::ResetEntered,
                ..held
            },
            Evidence { epoch: 2, ..held },
            Evidence {
                cold_releases: 1,
                ..held
            },
            Evidence {
                old_hci_closed: true,
                ..held
            },
            Evidence {
                application_stopped: true,
                ..held
            },
        ] {
            assert!(require_held(before, invalid).is_err());
        }
    }
}
