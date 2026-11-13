//! The image's UDP facade over a port that lends its buffers: a lent buffer
//! that no longer holds what the port showed is a typed error, and still
//! goes back to the port. `oer` and the load service stand in for their
//! chip-side names.
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

use core::future::Future;
use core::task::{Context, Waker};

use bench::*;
use oer_portable_time_virtual::VirtualClock;

#[test]
fn an_unreadable_lent_buffer_is_a_typed_error_and_goes_back_to_the_port() {
    let port = Port::default();
    {
        let mut wire = port.0.borrow_mut();
        wire.lends = true;
        wire.breaks = true;
    }
    let wifi::Built {
        mut service, net, ..
    } = wifi::network(port.clone(), LOCAL, OURS, 5001, 5002);
    let start = start();
    let mut run = udp::open(&net, &start).unwrap();
    let clock = VirtualClock::new();
    port.0.borrow_mut().incoming.push_back(datagram());
    turn(&mut service, &clock);
    {
        let (mut receiving, _sending) = run.split();
        let mut error = None;
        {
            let mut prefix = core::pin::pin!(receiving.prefix(|cell| {
                error = cell.as_mut().unwrap().data().err();
                true
            }));
            assert!(matches!(
                prefix
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                core::task::Poll::Ready(None)
            ));
        }
        assert_eq!(
            error,
            Some(wifi::FrameLengthError::OutsideStorage),
            "the broken buffer is an error, not an empty payload"
        );
        assert_eq!(
            port.0.borrow().back.get(),
            1,
            "the prefix returns its unreadable owner"
        );
    }
    drop(run);
    turn(&mut service, &clock);
    let mut ended = service.retire();
    drop(net);
    ended.collect();
    assert!(ended.retired.pool.loans().released(), "its loan came back");
}
