//! Independent Linux SMP/GATT peer with automated, boot-bound HIL comparison.
mod comparison;
mod read_failure;
mod reset_gate;
mod shutdown;
use super::establish::{Attempt, CONFIRMATION, establish};
use crate::scenario::SecureGattShutdown;
use crate::{
    Result, fixture::bluetooth::att, fixture::bluetooth::model::PeerAddress,
    fixture::bluetooth::secure_gatt::Owner,
};
use oer_hil_link::SerialCapture;
use oer_hil_protocol::bluetooth::BluetoothSecureGattEvidence as Evidence;
use oer_hil_workload::context::Context;
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum IrqSampling {
    #[default]
    EverySnapshot,
    BoundaryOnly,
}

/// Keep the observer policy with the capture across pairing and retirement;
/// no helper may accidentally restore intrusive sampling in the timing case.
struct Observation<'a> {
    capture: &'a SerialCapture,
    irq: IrqSampling,
}

impl std::ops::Deref for Observation<'_> {
    type Target = SerialCapture;
    fn deref(&self) -> &Self::Target {
        self.capture
    }
}

impl Observation<'_> {
    /// The secure GATT observation under this observation's IRQ sampling
    /// policy.
    fn secure_gatt(&self) -> Result<Evidence> {
        match self.irq {
            IrqSampling::EverySnapshot => crate::link::secure_gatt(self.capture),
            IrqSampling::BoundaryOnly => crate::link::secure_gatt_snapshot(self.capture),
        }
    }
}

fn join(a: Result<()>, b: Result<()>) -> Result<()> {
    match (a, b) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(e), Ok(())) | (Ok(()), Err(e)) => Err(e),
        (Err(a), Err(b)) => Err(format!("{a}; cleanup: {b}").into()),
    }
}
fn sample(capture: &Observation<'_>, samples: &mut Vec<Evidence>) -> Result<Evidence> {
    let e = capture.secure_gatt()?;
    samples.push(e);
    if e.application_stopped {
        return Err(format!(
            "secure application stopped: shutdown={:?}, application_failure={:?}, advertising_start_rejection={:?}",
            e.shutdown, e.application_failure, e.advertising_start_rejection
        )
        .into());
    }
    if !e
        .traffic
        .cpu0_stack
        .is_some_and(|s| s.has_required_headroom())
    {
        return Err("secure GATT stack headroom missing or insufficient".into());
    }
    Ok(e)
}
fn wait(
    capture: &Observation<'_>,
    samples: &mut Vec<Evidence>,
    predicate: impl Fn(&Evidence) -> bool,
) -> Result<Evidence> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let e = sample(capture, samples)?;
        if predicate(&e) {
            return Ok(e);
        }
        if Instant::now() >= deadline {
            return Err(format!("secure GATT transition deadline: {e:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

pub fn run(
    output: &Path,
    context: &Context<'_>,
    shutdown: SecureGattShutdown,
    irq_sampling: IrqSampling,
) -> Result<()> {
    let adapter = context
        .lab
        .bluetooth_adapter
        .ok_or("Bluetooth adapter required")?;
    att::preflight(adapter)?;
    let mut radio = att::Owner::acquire(adapter, output)?;
    let result = context.with_capture(output, |capture| {
        let observation = Observation { capture, irq: irq_sampling };
        let capture = &observation;
        let mut samples = Vec::new();
        let mut exchanges = Vec::new();
        let mut attempts: Vec<Attempt> = Vec::new();
        let mut peer = None;
        let probe = (|| -> Result<()> {
            let capabilities = capture.request_image_keys(Duration::from_secs(10))?;
            if !capabilities.has::<oer_hil_protocol::bluetooth::SecureGatt>() || capabilities.has::<oer_hil_protocol::bluetooth::Gatt>() { return Err("exclusive secure GATT image required".into()); }
            crate::link::require_irq_stack(capture)?;
            let initial = wait(capture,&mut samples,|e|e.traffic.advertising && e.traffic.address.is_some())?;
            if initial.traffic.connections != 0 || initial.bonds_stored != 0 || initial.comparisons != 0 || initial.traffic.value != 0 { return Err("fresh secure application epoch required".into()); }
            let address = PeerAddress(initial.traffic.address.ok_or("DUT address missing")?);
            // Reserve the previously absent device record before raw ATT or discovery.
            peer = Some(Owner::acquire(adapter,address)?);
            let peer = peer.as_mut().unwrap();
            let plaintext = establish("plaintext-connect", next(&initial), &mut attempts, &mut samples, |_| radio.connect(address), |samples| confirmed(capture, samples, &initial), |_, att| { drop(att); Ok(()) }, |samples| unseen(capture, samples, &initial))?;
            plaintext_denied(plaintext,&mut exchanges)?;
            let idle = wait(capture,&mut samples,|e| !e.traffic.connected && e.traffic.advertising && e.traffic.disconnections == 1)?;
            if idle.traffic.writes != 0 || idle.traffic.value != 0 || idle.bonds_stored != 0 { return Err("plaintext ATT changed protected state".into()); }
            peer.discover()?;
            let before = sample(capture,&mut samples)?;
            establish("declined-pairing", next(&before), &mut attempts, &mut samples, |samples| pairing(capture,peer,samples,false, &mut exchanges), |_| Ok(true), |_, ()| Ok(()), |samples| unseen(capture, samples, &before))?;
            peer.disconnect()?;
            let declined = wait(capture,&mut samples,|e| !e.traffic.connected && e.traffic.advertising && e.declined == 1)?;
            if declined.bonds_stored != 0 || declined.accepted != 0 { return Err("declined pairing established application trust".into()); }
            let before = sample(capture,&mut samples)?;
            establish("accepted-pairing", next(&before), &mut attempts, &mut samples, |samples| pairing(capture,peer,samples,true, &mut exchanges), |_| Ok(true), |_, ()| Ok(()), |samples| unseen(capture, samples, &before))?;
            wait(capture,&mut samples,|e|e.bonds_stored == 1 && e.accepted == 1)?;
            protected_exchange(peer,0,0x41,&mut exchanges)?;
            peer.disconnect()?;
            let before = wait(capture,&mut samples,|e|!e.traffic.connected && e.traffic.advertising)?;
            establish("bonded-reconnect", next(&before), &mut attempts, &mut samples, |_| peer.connect(), |samples| confirmed(capture, samples, &before), |_, ()| peer.disconnect(), |samples| unseen(capture, samples, &before))?;
            protected_exchange(peer,0x41,0x62,&mut exchanges)?;
            let resumed = wait(capture,&mut samples,|e|e.bonds_resumed == 1 && e.notifications_queued == 2)?;
            if resumed.bonds_stored != 1 || resumed.comparisons != 2 || resumed.accepted != 1 || resumed.declined != 1 || resumed.traffic.writes != 2 || resumed.traffic.value != 0x62 { return Err("bonded reconnect silently repaired, downgraded or changed epoch".into()); }
            peer.disconnect()?;
            let idle = wait(capture,&mut samples,|e|!e.traffic.connected && e.traffic.advertising)?;
            let boot = capture.latest_boot_id().ok_or("secure GATT boot missing")?;
            match shutdown {
                SecureGattShutdown::BondLoadFailure => reset_gate::restart(capture, &mut samples, boot, idle)?,
                SecureGattShutdown::HciReadFailure => crate::link::restart_gatt(capture, boot, idle.epoch)?,
            }
            let restarted = wait(capture,&mut samples,|e|e.epoch == resumed.epoch + 1 && !e.restarting && e.traffic.advertising)?;
            if capture.latest_boot_id() != Some(boot) || restarted.cold_releases != 1 || !restarted.old_hci_closed || restarted.bonds_stored != 1 {
                return Err("cold restart did not retain the application bond and retire the old HCI epoch".into());
            }
            establish("cold-reconnect", next(&restarted), &mut attempts, &mut samples, |_| peer.connect(), |samples| confirmed(capture, samples, &restarted), |_, ()| peer.disconnect(), |samples| unseen(capture, samples, &restarted))?;
            protected_exchange(peer,0,0x73,&mut exchanges)?;
            let restored = wait(capture,&mut samples,|e|e.bonds_resumed == 2 && e.notifications_queued == 3)?;
            if restored.comparisons != 2 || restored.accepted != 1 || restored.declined != 1 || restored.bonds_stored != 1 {
                return Err("cold reconnect required re-pairing or changed the retained bond".into());
            }
            match shutdown {
                SecureGattShutdown::BondLoadFailure => shutdown::bond_load_failure(capture, peer, &mut samples, boot, restored),
                SecureGattShutdown::HciReadFailure => {
                    peer.disconnect()?;
                    let idle = wait(capture, &mut samples, |e| !e.traffic.connected && e.traffic.advertising)?;
                    read_failure::run(capture, &mut samples, boot, idle)
                }
            }
        })();
        context.measurements.record(super::establish::measurements(&attempts));
        // Also retain the terminal high-water measurement when the workload
        // fails. A scan failure is never replaced by the traffic outcome.
        let probe = join(probe, crate::link::require_irq_stack(capture));
        let cleanup = oer_process::cleanup(|| {
            let bond = peer.as_mut().map_or(Ok(()), Owner::restore);
            let power = radio.restore();
            join(bond,power)
        });
        context.results.observe("trouble-secure-gatt", &serde_json::json!({
            "schema":7, "pairing":"numeric-comparison-only", "connection_attempts":attempts, "dut_store":"ram", "linux_bond":"temporary", "shutdown":shutdown,
            "irq_sampling":irq_sampling,
            "confirmation":"automated-hil-number-comparison", "human_presence_verified":false,
            "samples":samples,"peer_exchanges":exchanges,"passed":probe.is_ok() && cleanup.is_ok(),
            "error":probe.as_ref().err().map(ToString::to_string), "restored":cleanup.is_ok(),
            "cleanup_error":cleanup.as_ref().err().map(ToString::to_string),
            "coordinated_controller_retirement":probe.is_ok(),
            "bond_load_failure_cold_shutdown":probe.is_ok() && shutdown == SecureGattShutdown::BondLoadFailure,
            "withheld_reset_reader_retention_and_resume":probe.is_ok() && shutdown == SecureGattShutdown::BondLoadFailure,
            "hci_read_failure_retained":probe.is_ok() && shutdown == SecureGattShutdown::HciReadFailure,
        }));
        join(probe,cleanup)
    });
    join(result, oer_process::cleanup(|| radio.restore()))
}

fn pairing(
    capture: &Observation<'_>,
    peer: &Owner,
    samples: &mut Vec<Evidence>,
    accept: bool,
    exchanges: &mut Vec<serde_json::Value>,
) -> Result<()> {
    std::thread::scope(|scope| {
        let pair = scope.spawn(|| peer.pair());
        let decision = (|| -> Result<()> {
            let deadline = Instant::now() + Duration::from_secs(15);
            let mut prompt = None;
            loop {
                if prompt.is_none() {
                    prompt = peer.prompt()?;
                }
                let e = sample(capture, samples)?;
                if let (Some(linux), Some(dut)) = (prompt.as_ref(), e.comparison) {
                    let boot = capture.latest_boot_id().ok_or("boot identity missing")?;
                    exchanges.push(serde_json::json!({"boot":boot,"dut_challenge":dut,"linux_number":linux.number,"requested_accept":accept}));
                    let decision = comparison::decision(boot, dut, linux.number, accept)?;
                    // Only this HIL scenario acts as the test operator. The DUT
                    // still requires an explicit, exact boot/request decision.
                    crate::link::confirm_gatt(capture, boot, decision)?;
                    if accept {
                        prompt.take().unwrap().answer(true)?;
                    } else {
                        // Let the DUT consume its explicit decline first. Its
                        // SMP failure may already cancel the peer's callback;
                        // dropping the prompt rejects any still-pending request.
                        wait(capture, samples, |e| e.declined == 1)?;
                        drop(prompt.take());
                    }
                    return Ok(());
                }
                if pair.is_finished() {
                    return Err("Pair ended without both Numeric Comparison prompts".into());
                }
                if Instant::now() >= deadline {
                    return Err("Numeric Comparison prompts missing".into());
                }
                oer_process::sleep(Duration::from_millis(50))?;
            }
        })();
        let cancel = if decision.is_err() && !pair.is_finished() {
            peer.cancel_pairing()
        } else {
            Ok(())
        };
        let paired = pair.join().map_err(|_| "BlueZ pairing worker panicked")?;
        join(decision, cancel)?;
        match (accept, paired) {
            (true, Ok(())) | (false, Err(_)) => Ok(()),
            (true, Err(e)) => Err(e),
            (false, Ok(())) => Err("rejected pairing unexpectedly succeeded".into()),
        }
    })
}

fn protected_exchange(
    peer: &Owner,
    previous: u8,
    value: u8,
    evidence: &mut Vec<serde_json::Value>,
) -> Result<()> {
    let characteristic = peer.characteristic()?;
    let before = peer.read(&characteristic)?;
    if before != [previous] {
        return Err("protected value did not survive reconnect".into());
    }
    let notify = peer.notify(&characteristic)?;
    peer.write(&characteristic, value)?;
    let notification = notify.receive()?;
    let after = peer.read(&characteristic)?;
    evidence.push(serde_json::json!({"read_before":before,"written":value,"notification":notification,"read_after":after}));
    if notification != [value] || after != [value] {
        return Err("independent protected read/notification mismatch".into());
    }
    Ok(())
}

/// Whether the DUT never saw the connection an attempt opened after
/// `before`: it counts no new connection or comparison, is not connected and
/// advertises.
fn unseen(
    capture: &Observation<'_>,
    samples: &mut Vec<Evidence>,
    before: &Evidence,
) -> Result<bool> {
    let e = sample(capture, samples)?;
    Ok(never_reached(&e, before))
}

/// The DUT connection number an attempt after `before` opens.
fn next(before: &Evidence) -> u32 {
    before.traffic.connections + 1
}

/// Whether the DUT reports, within [`CONFIRMATION`], the connection an
/// attempt after `before` opened. A failed observation is the error, never
/// a `false`.
fn confirmed(
    capture: &Observation<'_>,
    samples: &mut Vec<Evidence>,
    before: &Evidence,
) -> Result<bool> {
    let deadline = Instant::now() + CONFIRMATION;
    loop {
        if confirms(&sample(capture, samples)?, before) {
            return Ok(true);
        }
        if Instant::now() >= deadline {
            return Ok(false);
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

/// Whether the DUT reports open the connection an attempt after `before`
/// opened.
fn confirms(e: &Evidence, before: &Evidence) -> bool {
    e.traffic.connected && e.traffic.connections == next(before)
}

fn never_reached(e: &Evidence, before: &Evidence) -> bool {
    e.traffic.connections == before.traffic.connections
        && e.comparisons == before.comparisons
        && !e.traffic.connected
        && e.traffic.advertising
}

fn plaintext_denied(peer: att::Att, evidence: &mut Vec<serde_json::Value>) -> Result<()> {
    let mut exchange = |request: &[u8]| -> Result<Vec<u8>> {
        peer.send(request)?;
        let response = peer.receive()?;
        evidence.push(serde_json::json!({"plaintext_request":request,"response":response}));
        Ok(response)
    };
    let service = exchange(&[6, 1, 0, 255, 255, 0, 0x28, 0xf0, 0xff])?;
    if service.len() != 5 || service[0] != 7 {
        return Err("secure service discovery failed".into());
    }
    let characteristic = exchange(&[8, service[1], service[2], service[3], service[4], 3, 0x28])?;
    let (value, cccd_start) = discover_value(&characteristic, &service)?;
    let [low, high] = value.to_le_bytes();
    let [dl, dh] = cccd_start.to_le_bytes();
    let descriptors = exchange(&[4, dl, dh, service[3], service[4]])?;
    if descriptors.len() != 6 || descriptors[..2] != [5, 1] || descriptors[4..] != [2, 0x29] {
        return Err("expected one discovered CCCD".into());
    }
    let cccd = u16::from_le_bytes([descriptors[2], descriptors[3]]);
    let end = u16::from_le_bytes([service[3], service[4]]);
    if cccd < cccd_start || cccd > end {
        return Err("CCCD outside service".into());
    }
    for request in [
        vec![0x0a, low, high],
        vec![0x12, low, high, 0x99],
        vec![0x12, descriptors[2], descriptors[3], 1, 0],
    ] {
        require_denied(&request, &exchange(&request)?)?;
    }
    Ok(())
}
fn discover_value(characteristic: &[u8], service: &[u8]) -> Result<(u16, u16)> {
    if characteristic.len() != 9
        || characteristic[..2] != [9, 7]
        || characteristic[4] != 0x1a
        || characteristic[7..] != [0xf1, 0xff]
    {
        return Err("expected read/write/notify value".into());
    }
    let start = u16::from_le_bytes([service[1], service[2]]);
    let end = u16::from_le_bytes([service[3], service[4]]);
    let declaration = u16::from_le_bytes([characteristic[2], characteristic[3]]);
    let value = u16::from_le_bytes([characteristic[5], characteristic[6]]);
    if start == 0 || declaration <= start || value <= declaration || value >= end {
        return Err("invalid secure service handles".into());
    }
    Ok((value, value + 1))
}
fn require_denied(request: &[u8], response: &[u8]) -> Result<()> {
    if response.len() == 5
        && response[0] == 1
        && response[1..4] == request[..3]
        && matches!(response[4], 5 | 8)
    {
        Ok(())
    } else {
        Err(format!("protected plaintext access was not denied: {response:?}").into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn denial_requires_exact_operation_handle_and_security_error() {
        assert!(require_denied(&[10, 17, 0], &[1, 10, 17, 0, 5]).is_ok());
        for response in [
            &[11, 0][..],
            &[1, 10, 18, 0, 5],
            &[1, 10, 17, 0, 1],
            &[1, 18, 17, 0, 5],
        ] {
            assert!(require_denied(&[10, 17, 0], response).is_err());
        }
    }
    #[test]
    fn only_the_attempted_connection_open_on_the_dut_confirms_it() {
        let mut before = Evidence::default();
        before.traffic.connections = 2;
        let mut open = before;
        open.traffic.connections = 3;
        open.traffic.connected = true;
        assert!(confirms(&open, &before));
        let mut earlier = open;
        earlier.traffic.connections = 2;
        let mut closed = open;
        closed.traffic.connected = false;
        assert!(!confirms(&earlier, &before));
        assert!(!confirms(&closed, &before));
    }

    #[test]
    fn an_attempt_is_for_the_dut_connection_after_the_last_counted() {
        let mut before = Evidence::default();
        before.traffic.connections = 2;
        assert_eq!(next(&before), 3);
    }
    #[test]
    fn only_a_connection_the_dut_never_saw_is_unreached() {
        let mut before = Evidence::default();
        before.traffic.connections = 1;
        before.traffic.advertising = true;
        before.comparisons = 1;
        assert!(never_reached(&before, &before));
        let mut counted = before;
        counted.traffic.connections = 2;
        let mut connected = before;
        connected.traffic.connected = true;
        let mut silent = before;
        silent.traffic.advertising = false;
        let mut compared = before;
        compared.comparisons = 2;
        for seen in [counted, connected, silent, compared] {
            assert!(!never_reached(&seen, &before));
        }
    }
    #[test]
    fn secure_discovery_rejects_plaintext_properties_and_foreign_handles() {
        let service = [7, 8, 0, 11, 0];
        let value = [9, 7, 9, 0, 0x1a, 10, 0, 0xf1, 0xff];
        assert_eq!(discover_value(&value, &service).unwrap(), (10, 11));
        let mut wrong = value;
        wrong[4] = 0x0a;
        assert!(discover_value(&wrong, &service).is_err());
        assert!(discover_value(&value, &[7, 10, 0, 11, 0]).is_err());
    }
}
