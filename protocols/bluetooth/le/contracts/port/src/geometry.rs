//! The installation's geometry, checked at build
//! (`design/ble-port-contract-2026-11-19.md`, section 2).

use core::marker::PhantomData;

use crate::{LeCapabilities, LeRadioCapabilities, rights::MAX_INDEX};

/// The counts of an installation's geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Counts {
    pub(crate) advertising: usize,
    pub(crate) scanners: usize,
    pub(crate) connections: usize,
    pub(crate) events: usize,
    pub(crate) transmit: usize,
    pub(crate) receive: usize,
    pub(crate) tests: usize,
}

/// Which relation of the geometry does not hold: the build's failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Misfit {
    EventsBelowTwo,
    IndexBeyondAnOctet,
    NodeBeyondTwoOctets,
    Scanners,
    Tests,
    AdvertisingSets,
    Connections,
    ConnectionEvents,
    TransmitPlaces,
    ReceiveNodes,
    NoAdvertising,
    NoScanning,
    NoConnections,
    NoTest,
}

/// The first relation of `counts` that `caps` does not hold, in this
/// order.
pub(crate) const fn misfit(caps: &LeRadioCapabilities, counts: Counts) -> Option<Misfit> {
    let limits = caps.limits;
    let Counts {
        advertising,
        scanners,
        connections,
        events,
        transmit,
        receive,
        tests,
    } = counts;
    if events < 2 {
        return Some(Misfit::EventsBelowTwo);
    }
    if advertising > MAX_INDEX
        || connections > MAX_INDEX
        || events > MAX_INDEX
        || transmit > MAX_INDEX
    {
        return Some(Misfit::IndexBeyondAnOctet);
    }
    if receive > u16::MAX as usize {
        return Some(Misfit::NodeBeyondTwoOctets);
    }
    if scanners > 1 {
        return Some(Misfit::Scanners);
    }
    if tests > 1 {
        return Some(Misfit::Tests);
    }
    if advertising > limits.advertising_sets {
        return Some(Misfit::AdvertisingSets);
    }
    if connections > limits.connections {
        return Some(Misfit::Connections);
    }
    if connections > 0 && events > limits.connection_events {
        return Some(Misfit::ConnectionEvents);
    }
    if connections > 0 && transmit > limits.transmit_places {
        return Some(Misfit::TransmitPlaces);
    }
    if receive > limits.receive_nodes {
        return Some(Misfit::ReceiveNodes);
    }
    if advertising > 0 && !caps.legacy_advertising.any() {
        return Some(Misfit::NoAdvertising);
    }
    if scanners > 0 && !caps.scanning() {
        return Some(Misfit::NoScanning);
    }
    if connections > 0 && caps.peripheral_connection.is_none() {
        return Some(Misfit::NoConnections);
    }
    if tests > 0 && !caps.direct_test_mode {
        return Some(Misfit::NoTest);
    }
    None
}

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
    /// inline constants and constant items: the mint's and a round's.
    pub(crate) const CHECKED: () = match misfit(
        &K::CAPS,
        Counts {
            advertising: ADV,
            scanners: SCAN,
            connections: CONN,
            events: EVENTS,
            transmit: TX,
            receive: RX,
            tests: TEST,
        },
    ) {
        None => (),
        Some(Misfit::EventsBelowTwo) => {
            panic!("a connection has at least two event places: the event out and the next")
        }
        Some(Misfit::IndexBeyondAnOctet) => panic!("an instance or a slot is named by one octet"),
        Some(Misfit::NodeBeyondTwoOctets) => panic!("a receive node is named by two octets"),
        Some(Misfit::Scanners) => panic!("an installation has one scanner at most"),
        Some(Misfit::Tests) => panic!("an installation has one test session at most"),
        Some(Misfit::AdvertisingSets) => panic!("more advertising sets than the capabilities hold"),
        Some(Misfit::Connections) => panic!("more connections than the capabilities hold"),
        Some(Misfit::ConnectionEvents) => {
            panic!("more event places a connection than the capabilities hold")
        }
        Some(Misfit::TransmitPlaces) => {
            panic!("more transmit places a connection than the capabilities hold")
        }
        Some(Misfit::ReceiveNodes) => panic!("more receive nodes than the capabilities hold"),
        Some(Misfit::NoAdvertising) => {
            panic!("advertising sets with capabilities that serve no advertising")
        }
        Some(Misfit::NoScanning) => panic!("a scanner with capabilities that serve no scanning"),
        Some(Misfit::NoConnections) => panic!("connections with capabilities that serve none"),
        Some(Misfit::NoTest) => {
            panic!("a test session with capabilities that serve no Direct Test Mode")
        }
    };
}

#[cfg(test)]
mod tests;
