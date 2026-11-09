//! The image's UDP facade on real sockets: `oer` and the load service stand in
//! for their chip-side names, so only start and the port are substituted.
extern crate self as oer;
extern crate self as oer_hil_services_load;

use core::future::Future;
use core::task::{Context, Waker};
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_network_port::{EthernetFrameView, FramePort, FrameSource, Unpublished};
use oer_protocols_network_wire::checksum::{Checksum, ipv4_pseudo_header, udp_field};
use oer_protocols_network_wire::ethernet::{EtherType, EthernetHeader};
use oer_protocols_network_wire::ip::{DsField, Ipv4Fields};
use oer_protocols_network_wire::transport::{PROTOCOL_UDP, Ports, UdpHeader};
use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

pub struct Start {
    pub port: u16,
}
#[derive(Debug)]
pub enum LoadError {
    NoAddress,
    Busy,
    InvalidParameters,
    Bind,
}

#[path = "support/wifi.rs"]
pub mod wifi;

#[path = "../../../services/load/src/header.rs"]
pub mod header;

#[path = "../src/udp.rs"]
mod udp;

/// Frames waiting to be read; it publishes nothing.
#[derive(Clone, Default)]
struct Port(Rc<RefCell<VecDeque<Vec<u8>>>>);
impl FramePort for Port {
    type Refusal = ();
    type Ticket = ();
    fn publish<E>(
        &mut self,
        _: usize,
        _: impl FnOnce(&mut [u8]) -> Result<(), E>,
    ) -> Result<(), Unpublished<(), E>> {
        Err(Unpublished::Port(()))
    }
}
impl FrameSource for Port {
    type Notice = ();
    type Held = wifi::HostHeld;
    fn take_frame(
        &mut self,
        read: impl FnOnce(
            EthernetFrameView<'_>,
            oer_protocols_network_port::Holding,
        ) -> oer_protocols_network_port::Keep,
    ) -> oer_protocols_network_port::Taken<Self::Held> {
        let Some(frame) = self.0.borrow_mut().pop_front() else {
            return oer_protocols_network_port::Taken::Empty;
        };
        let (header, payload) = EthernetHeader::parse(&frame).unwrap();
        let _copied = read(
            EthernetFrameView {
                destination: header.destination,
                source: header.source,
                ether_type: header.ether_type.value(),
                payload,
            },
            oer_protocols_network_port::Holding::Copied(
                oer_protocols_network_port::CopyReason::Format,
            ),
        );
        oer_protocols_network_port::Taken::Read
    }
    fn take_notice(&mut self) -> Option<()> {
        None
    }
    fn link(&self) -> oer_protocols_network_port::LinkSnapshot {
        oer_protocols_network_port::LinkSnapshot {
            state: oer_protocols_network_port::LinkState::Up,
            epoch: 1,
        }
    }
    fn register(&mut self, _: &mut Context<'_>) {}
}
type TestService = wifi::Served<Port>;
fn turn(service: &mut TestService, clock: &VirtualClock) {
    let mut events = ();
    let mut run = core::pin::pin!(service.run(clock, &mut events));
    assert!(
        run.as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
            .is_pending()
    );
}

/// A datagram for this network's first port.
fn datagram() -> Vec<u8> {
    let (ours, theirs) = ([192, 0, 2, 1], [192, 0, 2, 2]);
    // Room for an echo's header.
    let data = [7; header::LENGTH];
    let length = 8 + data.len() as u16;
    let unfilled = UdpHeader {
        ports: Ports {
            source: 5001,
            destination: 5001,
        },
        length,
        checksum: 0,
    };
    let checksum = udp_field(
        Checksum::new()
            .with(&ipv4_pseudo_header(theirs, ours, PROTOCOL_UDP, length))
            .with(&unfilled.to_bytes())
            .with(&data)
            .finish(),
    );
    let mut frame = EthernetHeader {
        destination: [2; 6],
        source: [4; 6],
        ether_type: EtherType::IPV4,
    }
    .to_bytes()
    .to_vec();
    frame.extend_from_slice(
        &Ipv4Fields {
            services: DsField::from_bytes([0]),
            total_length: length + 20,
            identification: 1,
            time_to_live: 64,
            protocol: PROTOCOL_UDP,
            source: theirs,
            destination: ours,
        }
        .to_bytes(),
    );
    frame.extend_from_slice(
        &UdpHeader {
            checksum,
            ..unfilled
        }
        .to_bytes(),
    );
    frame.extend_from_slice(&data);
    frame
}

#[test]
fn repeated_cancelled_readers_keep_all_unreturned_blocks() {
    let port = Port::default();
    let wifi::Built {
        mut service, net, ..
    } = wifi::network(port.clone(), [2; 6], [192, 0, 2, 1], 5001, 5002);
    let start = Start { port: 5001 };
    let mut run = udp::open(&net, &start).unwrap();
    let clock = VirtualClock::new();
    // Fill the incoming lane and the core's receive queue independently
    // of the client's progress.
    for _ in 0..40 {
        port.0.borrow_mut().push_back(datagram());
    }
    for _ in 0..5 {
        turn(&mut service, &clock);
    }
    {
        let (mut receiving, _sending) = run.split();
        {
            let mut ready = core::pin::pin!(receiving.ready());
            assert!(
                ready
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_ready()
            );
        }
        // Restore the lane's last place while Receiving keeps next.
        turn(&mut service, &clock);
        let reply = header::Header {
            run: 0,
            sequence: 0,
            sent_us: 0,
            echo_request: false,
            echo_reply: true,
        };
        for _ in 0..32 {
            let packet = receiving.take().expect("32 received datagrams");
            // A copied datagram's block carries its echo at once.
            let mut echo = core::pin::pin!(packet.echo(reply));
            assert!(matches!(
                echo.as_mut().poll(&mut Context::from_waker(Waker::noop())),
                core::task::Poll::Ready(true)
            ));
        }
        // The request lane has 32 sends; the 33rd block cannot be returned.
        drop(receiving.take().expect("one ahead of the 32-place lane"));
    }
    drop(run);
    // One service turn opens four request places and delivers seven more
    // queued receives. Split first returns the previously parked block;
    // three further releases fill those four places.
    turn(&mut service, &clock);
    let mut run = udp::open(&net, &start).unwrap();
    {
        let (mut receiving, _sending) = run.split();
        for _ in 0..4 {
            drop(receiving.take().expect("queued receive"));
        }
    }
    drop(run);
    // Service is temporarily not polled, which its cancellation contract
    // permits. Existing receive data remains readable. A cancelled reader
    // must retain every new block even if earlier returns are still parked.
    for _ in 0..3 {
        let mut run = udp::open(&net, &start).unwrap();
        {
            let (mut receiving, _sending) = run.split();
            let mut ready = core::pin::pin!(receiving.ready());
            assert!(
                ready
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_ready()
            );
        }
    }
    // Resume: split returns what is parked into the now-drained request
    // lane. A final service turn processes those returns.
    turn(&mut service, &clock);
    {
        let mut run = udp::open(&net, &start).unwrap();
        let _ends = run.split();
    }
    turn(&mut service, &clock);
    let mut ended = service.retire();
    drop(net);
    ended.collect();
    assert_eq!(
        ended.retired.pool.outstanding(),
        0,
        "a return was dropped after parked filled"
    );
}
