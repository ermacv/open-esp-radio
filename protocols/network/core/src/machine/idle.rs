//! A request owed in the neighbor table keeps the core from idle on its own.
//! Through the core's calls a request is owed only beside the work that
//! needs it, so the table is set here directly.

use oer_portable_time::Instant;
use oer_protocols_ieee802_wire::Eui48;
use oer_protocols_network_wire::address::Ipv4Address;

use super::Core;
use crate::{BlockPlace, Interface, claim, with_local_network};

#[test]
fn an_owed_neighbor_request_alone_keeps_the_core_from_idle() {
    let mut storage = [const { BlockPlace::<64>::new() }; 2];
    with_local_network(|right| {
        let (_pool, brand) = claim(right, &mut storage);
        let interface = Interface::new(
            Eui48::new([2, 0, 0, 0, 0, 1]),
            Ipv4Address::new([192, 0, 2, 1]),
            24,
            None,
            1500,
        )
        .unwrap();
        let mut core = Core::<'_, _, 64, 1, 1, 1, 2>::new(brand, interface);
        let now = Instant::from_micros(0);
        assert!(core.idle());
        let _asking = core.neighbors.resolve(
            now,
            Ipv4Address::new([192, 0, 2, 2]),
            crate::serial::Serials::new().next(),
            |_| true,
        );
        assert_eq!(core.neighbors.owed_requests().count(), 1);
        assert!(!core.idle(), "a request is owed");
        let (owed, _, _) = core.neighbors.owed_requests().next().unwrap();
        core.neighbors.request_accepted(owed, now);
        assert!(core.idle(), "the request went: none is owed");
    });
}
