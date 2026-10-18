//! The Bluetooth LE operations the workloads ask of the board under test,
//! over the link's generic exchange: Direct Test Mode, raw HCI, the GATT
//! applications' observations and controls, and the IRQ stack policy of a
//! Bluetooth image.

use std::time::Duration;

use oer_hil_link::SerialCapture;

use crate::Result;

pub fn dtm(
    capture: &SerialCapture,
    operation: oer_hil_protocol::bluetooth::BluetoothDtmOperation,
) -> Result<oer_hil_protocol::bluetooth::BluetoothDtmEvidence> {
    match capture.call(
        0,
        oer_hil_protocol::bluetooth::RunDtm(operation),
        Duration::from_secs(5),
    )? {
        Ok(oer_hil_protocol::bluetooth::DtmResult(evidence)) if evidence.completed(operation) => {
            Ok(evidence)
        }
        response => Err(format!("Bluetooth {operation:?} failed: {response:?}").into()),
    }
}

/// One raw HCI exchange with the Controller of a `bluetooth_hci` image.
pub fn hci(
    capture: &SerialCapture,
    request: oer_hil_protocol::bluetooth::BluetoothHciRequest,
) -> Result<oer_hil_protocol::bluetooth::BluetoothHciResponse> {
    let wait = match &request {
        oer_hil_protocol::bluetooth::BluetoothHciRequest::NextEvent { wait_ms } => {
            Duration::from_millis(u64::from(*wait_ms))
        }
        oer_hil_protocol::bluetooth::BluetoothHciRequest::Command { .. } => Duration::ZERO,
    };
    match capture.call(
        0,
        oer_hil_protocol::bluetooth::ExchangeHci(request),
        Duration::from_secs(5) + wait,
    )? {
        Ok(oer_hil_protocol::bluetooth::HciResponse(response)) => Ok(response),
        response => Err(format!("Bluetooth HCI exchange rejected: {response:?}").into()),
    }
}

/// The standalone GATT image's observation, accepted only with its
/// single-core Bluetooth IRQ stack evidence.
pub fn gatt(capture: &SerialCapture) -> Result<oer_hil_protocol::bluetooth::BluetoothGattEvidence> {
    let evidence = gatt_observation(capture)?;
    require_irq_stack(capture)?;
    Ok(evidence)
}

/// The GATT application's observation without an image-specific stack
/// policy; the joint Wi-Fi/Bluetooth image reports its stacks through the
/// Wi-Fi evidence instead.
pub fn gatt_observation(
    capture: &SerialCapture,
) -> Result<oer_hil_protocol::bluetooth::BluetoothGattEvidence> {
    match capture.call(
        0,
        oer_hil_protocol::bluetooth::GetGatt,
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::GattState(evidence)) => Ok(evidence),
        response => Err(format!("invalid GATT observation: {response:?}").into()),
    }
}

pub fn secure_gatt(
    capture: &SerialCapture,
) -> Result<oer_hil_protocol::bluetooth::BluetoothSecureGattEvidence> {
    let evidence = secure_gatt_snapshot(capture)?;
    require_irq_stack(capture)?;
    Ok(evidence)
}

/// Protocol observation only. IRQ watermark scanning masks interrupts;
/// callers selecting this path must explicitly own their sampling policy.
pub fn secure_gatt_snapshot(
    capture: &SerialCapture,
) -> Result<oer_hil_protocol::bluetooth::BluetoothSecureGattEvidence> {
    match capture.call(
        0,
        oer_hil_protocol::bluetooth::GetSecureGatt,
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::SecureGattState(evidence)) => Ok(evidence),
        response => Err(format!("invalid secure GATT observation: {response:?}").into()),
    }
}

pub fn restart_gatt(capture: &SerialCapture, boot: u64, epoch: u32) -> Result<()> {
    if boot == 0 {
        return Err("secure GATT restart requires the observed boot identity".into());
    }
    match capture.call_boot(
        boot,
        0,
        oer_hil_protocol::bluetooth::RestartGatt { epoch },
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::SecureGattState(e)) if e.epoch == epoch && e.restarting => {
            Ok(())
        }
        response => Err(format!("secure GATT restart rejected: {response:?}").into()),
    }
}

pub fn fail_next_gatt_bond_load(capture: &SerialCapture, boot: u64, epoch: u32) -> Result<()> {
    if boot == 0 {
        return Err("bond load fault requires observed boot identity".into());
    }
    match capture.call_boot(
        boot,
        0,
        oer_hil_protocol::bluetooth::FailNextGattBondLoad { epoch },
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
            if e.epoch == epoch && e.bond_load_fault_armed && e.bond_load_failures == 0 =>
        {
            Ok(())
        }
        response => Err(format!("bond load fault rejected: {response:?}").into()),
    }
}

pub fn gatt_reset_read_gate(
    capture: &SerialCapture,
    boot: u64,
    epoch: u32,
    release: bool,
) -> Result<()> {
    use oer_hil_protocol::bluetooth::BluetoothGattResetReadGate as Phase;
    if boot == 0 {
        return Err("Reset gate requires observed boot identity".into());
    }
    let expected = if release {
        Phase::Released
    } else {
        Phase::Armed
    };
    match capture.call_boot(
        boot,
        0,
        oer_hil_protocol::bluetooth::GattResetReadGate { epoch, release },
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
            if e.epoch == epoch && e.reset_read_gate == expected =>
        {
            Ok(())
        }
        response => Err(format!("Reset reader gate rejected: {response:?}").into()),
    }
}

pub fn require_gatt_restart_rejected(capture: &SerialCapture, boot: u64, epoch: u32) -> Result<()> {
    match capture.call_boot(
        boot,
        0,
        oer_hil_protocol::bluetooth::RestartGatt { epoch },
        Duration::from_secs(2),
    )? {
        Err(oer_hil_protocol::base::RejectReason::InvalidState) => Ok(()),
        response => Err(format!(
            "terminal GATT epoch accepted restart or lost identity: {response:?}"
        )
        .into()),
    }
}

pub fn fail_gatt_reset_read(capture: &SerialCapture, boot: u64, epoch: u32) -> Result<()> {
    if boot == 0 {
        return Err("Reset read fault requires observed boot identity".into());
    }
    match capture.call_boot(
        boot,
        0,
        oer_hil_protocol::bluetooth::FailGattResetRead { epoch },
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::SecureGattState(e))
            if e.epoch == epoch
                && e.reset_read_gate
                    == oer_hil_protocol::bluetooth::BluetoothGattResetReadGate::FailureRequested =>
        {
            Ok(())
        }
        response => Err(format!("Reset read fault rejected: {response:?}").into()),
    }
}

pub fn confirm_gatt(
    capture: &SerialCapture,
    boot: u64,
    decision: oer_hil_protocol::bluetooth::BluetoothNumericDecision,
) -> Result<()> {
    if boot == 0 {
        return Err("Numeric Comparison requires the displayed boot identity".into());
    }
    match capture.call_boot(
        boot,
        0,
        oer_hil_protocol::bluetooth::ConfirmGatt(decision),
        Duration::from_secs(2),
    )? {
        Ok(oer_hil_protocol::bluetooth::GattDecisionRecorded(recorded)) if recorded == decision => {
            Ok(())
        }
        response => Err(format!("Numeric Comparison decision rejected: {response:?}").into()),
    }
}

pub fn require_irq_stack(capture: &SerialCapture) -> Result<()> {
    match capture.call(
        0,
        oer_hil_protocol::system::GetInterruptStacks,
        Duration::from_secs(5),
    )? {
        Ok(oer_hil_protocol::system::InterruptStacks { cpu0, cpu1 }) => {
            validate_irq_stack(cpu0, cpu1)
        }
        response => Err(format!(
            "Bluetooth IRQ stack evidence missing or below policy: {response:?}"
        )
        .into()),
    }
}

fn validate_irq_stack(
    cpu0: Option<oer_hil_protocol::system::StackWatermark>,
    cpu1: Option<oer_hil_protocol::system::StackWatermark>,
) -> Result<()> {
    match (cpu0, cpu1) {
        (Some(irq), None) if irq.has_required_headroom() => Ok(()),
        _ => Err(format!(
            "Bluetooth IRQ stack evidence missing or below policy: cpu0={cpu0:?} cpu1={cpu1:?}"
        )
        .into()),
    }
}

#[cfg(test)]
mod tests;
