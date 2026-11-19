//! The installation's geometry, checked at build
//! (`design/ble-port-contract-2026-11-19.md`, section 2).

use core::marker::PhantomData;

use crate::{LeCapabilities, rights::MAX_INDEX};

/// The geometry `ADV`, `SCAN`, `CONN`, `EVENTS`, `TX`, `RX`, `TEST` of an
/// installation of the capabilities `K`.
pub(crate) struct Geometry<
    K,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
>(PhantomData<fn() -> K>);

impl<
    K: LeCapabilities,
    const ADV: usize,
    const SCAN: usize,
    const CONN: usize,
    const EVENTS: usize,
    const TX: usize,
    const RX: usize,
    const TEST: usize,
> Geometry<K, ADV, SCAN, CONN, EVENTS, TX, RX, TEST>
{
    /// Fails the build when the geometry does not fit `K`. Named only in
    /// inline constants: the mint's and a round's.
    pub(crate) const CHECKED: () = {
        let caps = K::CAPS;
        let limits = caps.limits;
        if EVENTS < 2 {
            panic!("a connection has at least two event places: the event out and the next");
        }
        if ADV > MAX_INDEX || CONN > MAX_INDEX || EVENTS > MAX_INDEX || TX > MAX_INDEX {
            panic!("an instance or a slot is named by one octet");
        }
        if RX > u16::MAX as usize {
            panic!("a receive node is named by two octets");
        }
        if SCAN > 1 {
            panic!("an installation has one scanner at most");
        }
        if TEST > 1 {
            panic!("an installation has one test session at most");
        }
        if ADV > limits.advertising_sets {
            panic!("more advertising sets than the capabilities hold");
        }
        if CONN > limits.connections {
            panic!("more connections than the capabilities hold");
        }
        if CONN > 0 && EVENTS > limits.connection_events {
            panic!("more event places a connection than the capabilities hold");
        }
        if CONN > 0 && TX > limits.transmit_places {
            panic!("more transmit places a connection than the capabilities hold");
        }
        if RX > limits.receive_nodes {
            panic!("more receive nodes than the capabilities hold");
        }
        if ADV > 0 && !caps.legacy_advertising.any() {
            panic!("advertising sets with capabilities that serve no advertising");
        }
        if SCAN > 0 && !caps.scanning() {
            panic!("a scanner with capabilities that serve no scanning");
        }
        if CONN > 0 && caps.peripheral_connection.is_none() {
            panic!("connections with capabilities that serve none");
        }
        if TEST > 0 && !caps.direct_test_mode {
            panic!("a test session with capabilities that serve no Direct Test Mode");
        }
    };
}
