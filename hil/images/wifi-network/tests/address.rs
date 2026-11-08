//! The image's UDP facade before and after the network's lease: a run starts
//! once the network holds an address, on the port bound at the group's start.
//! `oer` and the load service stand in for their chip-side names.
extern crate self as oer;
extern crate self as oer_hil_services_load;

#[path = "support/load.rs"]
mod names;
pub use names::{LoadError, Start};

#[path = "support/wifi.rs"]
pub mod wifi;

#[path = "../../../services/load/src/header.rs"]
pub mod header;

#[path = "../src/udp.rs"]
mod udp;

#[path = "support/bench.rs"]
mod bench;

use bench::*;

#[test]
fn a_run_waits_for_the_network_s_lease_and_names_its_bound_port() {
    let port = Port::default();
    let wifi::Built { net, address, .. } = wifi::network(port, LOCAL, OURS, 5001, 5002);
    address.set(None);
    assert!(matches!(
        udp::open(&net, &start()),
        Err(LoadError::NoAddress)
    ));
    // The service published, but holds no address: before the lease.
    address.set(Some(oer_protocols_network_core::Configuration {
        ipv4: None,
        version: 0,
        lease: None,
        dhcp: Some(oer_protocols_network_core::DhcpState::Selecting),
    }));
    assert!(matches!(
        udp::open(&net, &start()),
        Err(LoadError::NoAddress)
    ));
    address.set(wifi::holding(OURS));
    assert!(matches!(
        udp::open(&net, &Start { port: 5003 }),
        Err(LoadError::InvalidParameters)
    ));
    assert!(udp::open(&net, &start()).is_ok());
    assert_eq!(udp::address(&net).unwrap().unwrap().address(), OURS);
}
