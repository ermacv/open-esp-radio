//! Scannable non-connectable advertising observed by an active scanner.
//!
//! The ESP Controller advertises `ADV_SCAN_IND` with a name carried only in
//! its scan response. The Linux adapter discovers LE devices by active
//! scanning, so BlueZ learns that name only if the Controller answered the
//! adapter's `SCAN_REQ`. The name is fresh for every run, so a cached device
//! cannot satisfy the check.
use super::hci;
use crate::{
    Result,
    fixture::bluetooth::{att, discovery::Discovery, model::PeerAddress},
};
use oer_hil_link::SerialCapture;
use oer_hil_workload::context::Context;
use std::{
    path::Path,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

/// 100 ms advertising interval in 0.625 ms units.
const INTERVAL: u16 = 160;
const ADV_SCAN_IND: u8 = 0x02;
const ADV_IND: u8 = 0x00;
const DISCOVERY_DEADLINE: Duration = Duration::from_secs(15);

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("missing Bluetooth adapter")?;
    let mut owner = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let mut report = serde_json::Map::new();
        let probe = exercise(capture, adapter, &mut report);
        let cleanup = oer_process::cleanup(|| hci::command(capture, hci::RESET, &[]).map(|_| ()));
        report.insert("schema".into(), 2.into());
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
            .observe("scannable-advertising", &serde_json::Value::Object(report));
        probe.and(cleanup)
    });
    let restore = oer_process::cleanup(|| owner.restore());
    result.and(restore)
}

fn fresh_name() -> Result<String> {
    Ok(format!(
        "oer-scan-{:08x}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.subsec_nanos()
    ))
}

fn exercise(
    capture: &SerialCapture,
    adapter: crate::fixture::bluetooth::model::Adapter,
    report: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    hci::require(capture)?;
    hci::command(capture, hci::RESET, &[])?;
    let address: [u8; 6] = hci::command(capture, hci::READ_BD_ADDR, &[])?
        .try_into()
        .map_err(|_| "Read BD_ADDR returned no address")?;
    let address = PeerAddress(address);
    report.insert("advertiser".into(), address.to_string().into());
    let scannable = attempt(capture, adapter, address, ADV_SCAN_IND)?;
    report.insert("scannable".into(), scannable.json());
    if scannable.answered() {
        return Ok(());
    }
    // A connectable set answers scans through the same response graph; its
    // result separates the scannable PDU from the Linux scanner.
    let connectable = attempt(capture, adapter, address, ADV_IND)?;
    report.insert("connectable_control".into(), connectable.json());
    Err(
        format!("no scan response to ADV_SCAN_IND from {address} within {DISCOVERY_DEADLINE:?}")
            .into(),
    )
}

struct Attempt {
    name: String,
    seen: bool,
    discovered: Option<String>,
}

impl Attempt {
    fn answered(&self) -> bool {
        self.discovered.as_deref() == Some(self.name.as_str())
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "scan_response_name": self.name,
            "seen": self.seen,
            "discovered_name": self.discovered,
        })
    }
}

fn attempt(
    capture: &SerialCapture,
    adapter: crate::fixture::bluetooth::model::Adapter,
    address: PeerAddress,
    kind: u8,
) -> Result<Attempt> {
    let name = fresh_name()?;
    let mut parameters = [0; 15];
    parameters[0..2].copy_from_slice(&INTERVAL.to_le_bytes());
    parameters[2..4].copy_from_slice(&INTERVAL.to_le_bytes());
    parameters[4] = kind;
    parameters[13] = 0x07;
    hci::command(capture, hci::LE_SET_ADVERTISING_PARAMETERS, &parameters)?;
    // Flags: LE General Discoverable, BR/EDR not supported.
    hci::command(
        capture,
        hci::LE_SET_ADVERTISING_DATA,
        &hci::data(&[2, 0x01, 0x06])?,
    )?;
    let mut response = vec![name.len() as u8 + 1, 0x09];
    response.extend_from_slice(name.as_bytes());
    hci::command(
        capture,
        hci::LE_SET_SCAN_RESPONSE_DATA,
        &hci::data(&response)?,
    )?;

    let mut discovery = Discovery::start(adapter)?;
    discovery.forget(address)?;
    hci::command(capture, hci::LE_SET_ADVERTISING_ENABLE, &[1])?;
    let deadline = Instant::now() + DISCOVERY_DEADLINE;
    let mut result = Attempt {
        name,
        seen: false,
        discovered: None,
    };
    while Instant::now() < deadline {
        result.seen |= discovery.seen(address);
        result.discovered = discovery.name(address)?;
        if result.answered() {
            break;
        }
        oer_process::sleep(Duration::from_millis(250))?;
    }
    let disable = hci::command(capture, hci::LE_SET_ADVERTISING_ENABLE, &[0]).map(|_| ());
    let stop = discovery.stop();
    let forget = discovery.forget(address);
    disable.and(stop).and(forget)?;
    Ok(result)
}
