//! ATT echo load for the joint Wi-Fi/Bluetooth coexistence image.
//!
//! One Linux ATT connection to the image's GATT application writes a value
//! and reads it back for a fixed interval while the caller runs Wi-Fi traffic
//! on the same radio. [`Echo::connect`] establishes the connection before the
//! measured interval, [`Echo::run`] is the load and [`Echo::finish`] closes the
//! connection gracefully and restores the adapter. The load fails on any
//! disconnect, missing or mismatched response, or disagreement between the
//! peer's count and the application's own observations.
use super::gatt::discover;
use crate::{
    Result,
    fixture::bluetooth::{
        att,
        model::{Adapter, PeerAddress},
    },
};
use oer_hil_link::SerialCapture;
use oer_hil_protocol::bluetooth::BluetoothGattEvidence as Evidence;
use std::{
    path::Path,
    time::{Duration, Instant},
};

/// One live echo connection over the joint image's GATT application.
pub struct Echo<'a> {
    capture: &'a SerialCapture,
    results: oer_hil_workload::results::Results,
    owner: att::Owner,
    peer: Option<att::Att>,
    handle: u16,
    address: [u8; 6],
}

/// What one [`Echo::run`] interval proved.
#[derive(Clone, Copy, Debug, serde::Serialize)]
pub struct EchoReport {
    /// Write/read pairs whose read returned the written value.
    pub echoes: u32,
    /// The measured interval.
    pub elapsed: Duration,
    /// The slowest write/read pair.
    pub max_round_trip: Duration,
    /// The application's observation after the interval.
    pub application: Evidence,
}

impl<'a> Echo<'a> {
    /// Power the adapter, connect to the advertising application of a fresh
    /// epoch and discover its value handle.
    pub fn connect(
        capture: &'a SerialCapture,
        adapter: Adapter,
        output: &Path,
        results: &oer_hil_workload::results::Results,
    ) -> Result<Self> {
        if !capture
            .request_image_keys(Duration::from_secs(10))?
            .has::<oer_hil_protocol::bluetooth::Gatt>()
        {
            return Err("joint image without the GATT application".into());
        }
        let initial = wait(capture, |e| e.address.is_some() && e.advertising)?;
        if initial.connections != 0 || initial.writes != 0 || initial.reads != 0 {
            return Err(format!("echo requires a fresh application epoch: {initial:?}").into());
        }
        let address = initial.address.ok_or("missing Controller address")?;
        let owner = att::Owner::acquire(adapter, output)?;
        let peer = owner.connect(PeerAddress(address))?;
        wait(capture, |e| e.connected && e.connections == 1)?;
        let mut exchanges = Vec::new();
        let handle = discover(&peer, 1, &mut exchanges)?;
        results.observe(
            "ble-echo-discovery",
            &serde_json::json!({"handle": handle, "exchanges": exchanges}),
        );
        Ok(Self {
            capture,
            results: results.clone(),
            owner,
            peer: Some(peer),
            handle,
            address,
        })
    }

    /// Echo values for `duration`, then check the application agrees.
    pub fn run(&mut self, duration: Duration) -> Result<EchoReport> {
        let peer = self.peer.as_ref().ok_or("echo connection closed")?;
        let [low, high] = self.handle.to_le_bytes();
        let started = Instant::now();
        let mut echoes = 0u32;
        let mut max_round_trip = Duration::ZERO;
        while started.elapsed() < duration {
            let value = echo_value(echoes);
            let sent = Instant::now();
            let written = request(peer, &[0x12, low, high, value])?;
            if written != [0x13] {
                return Err(format!("echo {echoes}: write not acknowledged: {written:?}").into());
            }
            let read = request(peer, &[0x0a, low, high])?;
            if read != [0x0b, value] {
                return Err(format!("echo {echoes}: read {read:?}, wrote {value:#04x}").into());
            }
            max_round_trip = max_round_trip.max(sent.elapsed());
            echoes += 1;
        }
        let elapsed = started.elapsed();
        let application = crate::link::gatt_observation(self.capture)?;
        validate(&application, echoes, self.address)?;
        let report = EchoReport {
            echoes,
            elapsed,
            max_round_trip,
            application,
        };
        self.results.observe("ble-echo", &report);
        Ok(report)
    }

    /// Disconnect from the host side, require the application to see a
    /// graceful remote termination and restore the adapter.
    pub fn finish(mut self) -> Result<Evidence> {
        drop(self.peer.take());
        let closed = wait(self.capture, |e| !e.connected);
        let restored = self.owner.restore();
        let closed = closed?;
        restored?;
        if closed.disconnections != 1 || closed.last_disconnect_reason != Some(0x13) {
            return Err(format!("expected one graceful disconnect: {closed:?}").into());
        }
        Ok(closed)
    }
}

/// A nonzero value that differs from the previous echo's.
fn echo_value(index: u32) -> u8 {
    (index % 255) as u8 + 1
}

fn request(peer: &att::Att, bytes: &[u8]) -> Result<Vec<u8>> {
    peer.send(bytes)?;
    peer.receive()
}

fn wait(capture: &SerialCapture, predicate: impl Fn(&Evidence) -> bool) -> Result<Evidence> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let evidence = crate::link::gatt_observation(capture)?;
        if predicate(&evidence) {
            return Ok(evidence);
        }
        if Instant::now() >= deadline {
            return Err(format!("GATT transition deadline: {evidence:?}").into());
        }
        oer_process::sleep(Duration::from_millis(50))?;
    }
}

fn validate(e: &Evidence, echoes: u32, address: [u8; 6]) -> Result<()> {
    let last = echoes.checked_sub(1).map(echo_value);
    if !e.connected
        || e.advertising
        || e.connections != 1
        || e.disconnections != 0
        || e.writes != echoes
        || e.reads != echoes
        || last.is_some_and(|value| e.value != value)
        || e.address != Some(address)
    {
        return Err(format!("{echoes} echoes disagree with the application: {e:?}").into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn consecutive_echo_values_differ_and_are_never_zero() {
        for index in 0..1024 {
            assert_ne!(echo_value(index), 0);
            assert_ne!(echo_value(index), echo_value(index + 1));
        }
    }

    #[test]
    fn peer_count_must_match_the_application() {
        let good = Evidence {
            address: Some([1; 6]),
            connected: true,
            connections: 1,
            reads: 3,
            writes: 3,
            value: echo_value(2),
            ..Evidence::default()
        };
        assert!(validate(&good, 3, [1; 6]).is_ok());
        for bad in [
            Evidence { writes: 2, ..good },
            Evidence { reads: 4, ..good },
            Evidence { value: 0, ..good },
            Evidence {
                connections: 2,
                disconnections: 1,
                ..good
            },
            Evidence {
                connected: false,
                ..good
            },
            Evidence {
                address: Some([2; 6]),
                ..good
            },
        ] {
            assert!(validate(&bad, 3, [1; 6]).is_err());
        }
    }

    #[test]
    fn a_live_echo_can_run_beside_wifi_traffic() {
        fn send<T: Send>() {}
        send::<Echo<'static>>();
    }
}
