//! Directed advertising: its timeout and the connection of its target.
//!
//! High duty cycle directed advertising to an absent random target must end
//! after 1.28 s with LE Connection Complete status Advertising Timeout. Low
//! duty cycle directed advertising to the Linux adapter must then let the
//! adapter connect: the kernel initiates to the ESP's public address and
//! accepts only an advertisement addressed to itself.
use super::hci;
use crate::{
    Result,
    fixture::bluetooth::{att, model::PeerAddress},
};
use oer_hil_link::SerialCapture;
use oer_hil_workload::context::Context;
use std::{path::Path, time::Duration};

const LE_CONNECTION_COMPLETE: u8 = 0x01;
const ADVERTISING_TIMEOUT: u8 = 0x3c;
const HIGH_DUTY: u8 = 0x01;
const LOW_DUTY: u8 = 0x04;
/// 100 ms low duty cycle interval in 0.625 ms units.
const INTERVAL: u16 = 160;
/// A static random address no device answers.
const ABSENT_TARGET: [u8; 6] = [0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0xc5];

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut report = serde_json::Map::new();
        let probe = exercise(capture, &owner, &mut report);
        let cleanup = oer_process::cleanup(|| hci::command(capture, hci::RESET, &[]).map(|_| ()));
        report.insert("schema".into(), 1.into());
        report.insert("passed".into(), (probe.is_ok() && cleanup.is_ok()).into());
        report.insert(
            "error".into(),
            probe.as_ref().err().map(ToString::to_string).into(),
        );
        report.insert(
            "cleanup_error".into(),
            cleanup.as_ref().err().map(ToString::to_string).into(),
        );
        context
            .results
            .observe("directed-advertising", &serde_json::Value::Object(report));
        probe.and(cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    result.and(restore)
}

fn parameters(kind: u8, random_target: bool, target: [u8; 6]) -> [u8; 15] {
    let mut parameters = [0; 15];
    parameters[0..2].copy_from_slice(&INTERVAL.to_le_bytes());
    parameters[2..4].copy_from_slice(&INTERVAL.to_le_bytes());
    parameters[4] = kind;
    parameters[6] = u8::from(random_target);
    parameters[7..13].copy_from_slice(&target);
    parameters[13] = 0x07;
    parameters
}

fn exercise(
    capture: &SerialCapture,
    owner: &att::Owner,
    report: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    hci::require(capture)?;
    hci::command(capture, hci::RESET, &[])?;
    hci::command(capture, hci::SET_EVENT_MASK, &hci::EVENT_MASK_WITH_LE_META)?;
    let address: [u8; 6] = hci::command(capture, hci::READ_BD_ADDR, &[])?
        .try_into()
        .map_err(|_| "Read BD_ADDR returned no address")?;
    let esp = PeerAddress(address);
    report.insert("advertiser".into(), esp.to_string().into());

    // High duty cycle to nobody ends in Advertising Timeout.
    hci::command(
        capture,
        hci::LE_SET_ADVERTISING_PARAMETERS,
        &parameters(HIGH_DUTY, true, ABSENT_TARGET),
    )?;
    hci::command(capture, hci::LE_SET_ADVERTISING_ENABLE, &[1])?;
    let started = std::time::Instant::now();
    let timeout = hci::le_meta_event(capture, LE_CONNECTION_COMPLETE, Duration::from_secs(5))?;
    let elapsed = started.elapsed();
    report.insert("high_duty_status".into(), timeout.get(3).copied().into());
    report.insert(
        "high_duty_millis".into(),
        (elapsed.as_millis() as u64).into(),
    );
    if timeout.get(3) != Some(&ADVERTISING_TIMEOUT) {
        return Err(format!("high duty directed advertising ended with {timeout:02x?}").into());
    }
    if elapsed < Duration::from_millis(1_200) {
        return Err(format!("high duty directed advertising ended after {elapsed:?}").into());
    }

    // Low duty cycle to the adapter lets it connect.
    let pc = owner.address();
    report.insert("target".into(), pc.to_string().into());
    hci::command(
        capture,
        hci::LE_SET_ADVERTISING_PARAMETERS,
        &parameters(LOW_DUTY, false, pc.0),
    )?;
    hci::command(capture, hci::LE_SET_ADVERTISING_ENABLE, &[1])?;
    let att = owner.connect(esp)?;
    let complete = hci::le_meta_event(capture, LE_CONNECTION_COMPLETE, Duration::from_secs(5))?;
    // Code, length, subevent, status, handle, role, peer kind, peer.
    report.insert("connection_status".into(), complete.get(3).copied().into());
    if complete.get(3) != Some(&0) || complete.get(8..14) != Some(&pc.0[..]) {
        return Err(format!("the adapter did not connect: {complete:02x?}").into());
    }
    let handle = [complete[4], complete[5]];
    drop(att);
    let status = hci::command_status(capture, hci::DISCONNECT, &[handle[0], handle[1], 0x13])?;
    if status != 0 {
        return Err(format!("Disconnect returned status {status:#04x}").into());
    }
    Ok(())
}
