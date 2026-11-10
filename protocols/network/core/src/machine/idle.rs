//! A request owed in the neighbor table keeps the core from idle on its own.
//! Through the core's calls a request is owed only beside the work that
//! needs it, so the table is set here directly.

use oer_portable_time::Instant;

use super::Core;
use crate::{Interface, claim};

#[derive(Debug)]
struct Net;

#[test]
fn an_owed_neighbor_request_alone_keeps_the_core_from_idle() {
    let mut storage = [[0_u8; 64]; 2];
    let (_pool, brand) = claim::<Net, 64, 2>(&mut storage).unwrap();
    let interface = Interface::new([2, 0, 0, 0, 0, 1], [192, 0, 2, 1], 24, None, 1500).unwrap();
    let mut core = Core::<'_, Net, 64, 1, 1, 1, 2>::new(brand, interface);
    let now = Instant::from_micros(0);
    assert!(core.idle());
    let _asking = core.neighbors.resolve(now, [192, 0, 2, 2], 1, |_| true);
    assert_eq!(core.neighbors.owed_requests().count(), 1);
    assert!(!core.idle(), "a request is owed");
    core.neighbors.request_accepted(0, now);
    assert!(core.idle(), "the request went: none is owed");
}
