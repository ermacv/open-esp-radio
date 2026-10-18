//! A failed Reset must retain the old epoch, not claim RF-close or restart.
use super::{Duration, Evidence, Instant, Observation, Result, wait};
use oer_hil_protocol::{
    bluetooth::BluetoothGattResetOutcome as Reset, bluetooth::BluetoothGattResetReadGate as Gate,
    bluetooth::BluetoothGattShutdown, bluetooth::BluetoothGattStopCause as Cause,
};

fn require_retained(before: Evidence, after: Evidence) -> Result<()> {
    if after.shutdown
        != Some(BluetoothGattShutdown {
            cause: Cause::Requested,
            reset: Reset::InjectedReceiveFailure,
        })
        || after.reset_read_gate != Gate::ReadFailed
        || !after.application_stopped
        || after.restarting
        || after.epoch != before.epoch
        || after.cold_releases != before.cold_releases
        || after.old_hci_closed != before.old_hci_closed
        || after.traffic.connections != before.traffic.connections
        || after.traffic.disconnections != before.traffic.disconnections
        || after.traffic.advertising_starts != before.traffic.advertising_starts
        || after.bonds_stored != before.bonds_stored
        || after.bonds_resumed != before.bonds_resumed
        || after.comparisons != before.comparisons
        || after.bond_load_failures != 0
    {
        return Err(
            format!("HCI read failure did not retain its original epoch: {after:?}").into(),
        );
    }
    Ok(())
}

pub(super) fn run(
    capture: &Observation<'_>,
    samples: &mut Vec<Evidence>,
    boot: u64,
    before: Evidence,
) -> Result<()> {
    crate::link::gatt_reset_read_gate(capture, boot, before.epoch, false)?;
    crate::link::restart_gatt(capture, boot, before.epoch)?;
    wait(capture, samples, |e| e.reset_read_gate == Gate::ReaderHeld)?;
    crate::link::fail_gatt_reset_read(capture, boot, before.epoch)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let e = capture.secure_gatt()?;
        samples.push(e);
        if e.application_stopped {
            require_retained(before, e)?;
            break;
        }
        if Instant::now() >= deadline {
            return Err(format!("HCI read fault not observed: {e:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
    crate::link::require_gatt_restart_rejected(capture, boot, before.epoch)?;
    // Test observation only. No production timeout, automatic reset or recovery.
    let until = Instant::now() + Duration::from_secs(1);
    loop {
        let e = capture.secure_gatt()?;
        samples.push(e);
        require_retained(before, e)?;
        if capture.latest_boot_id() != Some(boot)
            || !e
                .traffic
                .cpu0_stack
                .is_some_and(|s| s.has_required_headroom())
        {
            return Err("retained HCI failure rebooted or lost stack headroom".into());
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
    fn retention_requires_exact_error_and_no_new_release_or_epoch() {
        let before = Evidence {
            epoch: 2,
            cold_releases: 1,
            old_hci_closed: true,
            ..Default::default()
        };
        let after = Evidence {
            application_stopped: true,
            reset_read_gate: Gate::ReadFailed,
            shutdown: Some(BluetoothGattShutdown {
                cause: Cause::Requested,
                reset: Reset::InjectedReceiveFailure,
            }),
            ..before
        };
        assert!(require_retained(before, after).is_ok());
        for invalid in [
            Evidence { epoch: 3, ..after },
            Evidence {
                cold_releases: 2,
                ..after
            },
            Evidence {
                restarting: true,
                ..after
            },
            Evidence {
                reset_read_gate: Gate::FailureRequested,
                ..after
            },
            Evidence {
                shutdown: Some(BluetoothGattShutdown {
                    cause: Cause::Requested,
                    reset: Reset::ReceiveFailed,
                }),
                ..after
            },
        ] {
            assert!(require_retained(before, invalid).is_err());
        }
    }
}
