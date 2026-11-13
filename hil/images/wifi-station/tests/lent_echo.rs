//! Cancellation of Echo after retaining an owner from an RX prefix.
//! A separate process gives this image its own one-shot network Brand.
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
fn cancelling_echo_after_retaining_a_prefix_frame_returns_its_owner() {
    let port = Port::default();
    port.0.borrow_mut().lends = true;
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
        let mut retained = None;
        {
            let mut prefix = core::pin::pin!(receiving.prefix(|cell| {
                retained = cell.take();
                false
            }));
            assert!(matches!(
                prefix
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop())),
                core::task::Poll::Ready(None)
            ));
        }
        assert_eq!(port.0.borrow().back.get(), 0);
        {
            let packet = receiving.retained(retained.take().unwrap());
            let mut echo = Box::pin(packet.echo(header::Header {
                run: 1,
                sequence: 2,
                sent_us: 3,
                echo_request: false,
                echo_reply: true,
            }));
            assert!(
                echo.as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            assert_eq!(
                port.0.borrow().back.get(),
                0,
                "Echo keeps the owner while reserving its block"
            );
            // Cancel the separate asynchronous Echo, not a return batch.
        }
        assert_eq!(port.0.borrow().back.get(), 1);
    }
    drop(run);
    let mut ended = service.retire();
    drop(net);
    ended.collect();
    assert!(ended.retired.pool.loans().released());
    assert_eq!(ended.retired.pool.outstanding(), 0);
}
