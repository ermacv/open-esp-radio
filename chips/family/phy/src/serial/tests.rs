use super::*;
use core::cell::{Cell, RefCell};
use oer_espressif_analog::model::Route;
use oer_time::{Clock, Duration, Instant};
use oer_time_virtual::VirtualClock;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Edge {
    Route(ByteAddress),
    Busy(Host),
    Map,
    Mask(Route),
    Command(Host, ByteAddress, u8, bool),
    Data(Host),
}

struct Fake<'a> {
    edges: &'a RefCell<Vec<Edge>>,
    clock: &'a VirtualClock,
    ready: &'a Cell<Option<Instant>>,
    host: Host,
}
impl SamplePort for Fake<'_> {
    type Address = (ByteAddress, Route);
    fn host(address: Self::Address) -> Host {
        address.1.host
    }
    fn route(&self, address: ByteAddress) -> Option<Self::Address> {
        self.edges.borrow_mut().push(Edge::Route(address));
        (address.block() == 7).then_some((
            address,
            Route {
                request_code: 7,
                command_code: 7,
                host: self.host,
                read_select: 4,
                read_mask_bits: 24,
            },
        ))
    }
    fn is_busy(&self, host: Host) -> bool {
        self.edges.borrow_mut().push(Edge::Busy(host));
        self.ready.get().is_some_and(|at| self.clock.now() < at)
    }
    fn data(&self, host: Host) -> u8 {
        self.edges.borrow_mut().push(Edge::Data(host));
        0xa5
    }
}
impl Port for Fake<'_> {
    fn configure_host_map(&mut self) {
        self.edges.borrow_mut().push(Edge::Map);
    }
    fn publish_read_mask(&mut self, address: Self::Address) {
        self.edges.borrow_mut().push(Edge::Mask(address.1));
    }
    fn publish_command(&mut self, address: Self::Address, value: u8, write: bool) {
        self.edges
            .borrow_mut()
            .push(Edge::Command(address.1.host, address.0, value, write));
        self.ready
            .set(self.clock.now().checked_add(Duration::from_micros(3)));
    }
}

#[test]
fn read_and_write_preserve_host_map_mask_and_command_order() {
    for host in [Host::Host0, Host::Host1] {
        for write in [false, true] {
            let edges = RefCell::new(Vec::new());
            let clock = VirtualClock::new();
            let ready = Cell::new(None);
            let mut bus = AnalogBus::new(Fake {
                edges: &edges,
                clock: &clock,
                ready: &ready,
                host,
            });
            let address = ByteAddress::new(7, 11);
            let expected_route = bus.port.route(address).unwrap().1;
            edges.borrow_mut().clear();
            if write {
                bus.try_start_write(address, 0x96).unwrap();
                assert_eq!(
                    *edges.borrow(),
                    [
                        Edge::Route(address),
                        Edge::Busy(host),
                        Edge::Map,
                        Edge::Command(host, address, 0x96, true),
                    ]
                );
            } else {
                bus.try_start_read(address).unwrap();
                assert_eq!(
                    *edges.borrow(),
                    [
                        Edge::Route(address),
                        Edge::Busy(host),
                        Edge::Map,
                        Edge::Mask(expected_route),
                        Edge::Command(host, address, 0, false),
                    ]
                );
            }
        }
    }
}

#[test]
fn unrouted_commands_and_completions_touch_no_master_atom() {
    let edges = RefCell::new(Vec::new());
    let clock = VirtualClock::new();
    let ready = Cell::new(None);
    let mut bus = AnalogBus::new(Fake {
        edges: &edges,
        clock: &clock,
        ready: &ready,
        host: Host::Host1,
    });
    let address = ByteAddress::new(8, 11);
    assert_eq!(
        bus.try_start_read(address),
        Err(PhyI2cAccessError::UnroutedAddress)
    );
    assert_eq!(
        bus.try_start_write(address, 0xff),
        Err(PhyI2cAccessError::UnroutedAddress)
    );
    assert_eq!(
        bus.try_finish_read(address),
        Err(PhyI2cAccessError::UnroutedAddress)
    );
    assert_eq!(
        bus.try_finish_write(address),
        Err(PhyI2cAccessError::UnroutedAddress)
    );
    assert_eq!(*edges.borrow(), [Edge::Route(address); 4]);
}

#[test]
fn busy_commands_publish_nothing_and_completion_samples_only_once_per_edge() {
    for host in [Host::Host0, Host::Host1] {
        let edges = RefCell::new(Vec::new());
        let clock = VirtualClock::new();
        let ready = Cell::new(None);
        let mut bus = AnalogBus::new(Fake {
            edges: &edges,
            clock: &clock,
            ready: &ready,
            host,
        });
        let address = ByteAddress::new(7, 11);
        bus.try_start_read(address).unwrap();
        let started_at = clock.now();
        edges.borrow_mut().clear();
        assert_eq!(bus.try_start_read(address), Err(PhyI2cAccessError::Busy));
        assert_eq!(
            bus.try_start_write(address, 9),
            Err(PhyI2cAccessError::Busy)
        );
        assert_eq!(bus.try_finish_read(address), Err(PhyI2cAccessError::Busy));
        assert_eq!(bus.try_finish_write(address), Err(PhyI2cAccessError::Busy));
        assert_eq!(
            *edges.borrow(),
            [Edge::Route(address), Edge::Busy(host)].repeat(4)
        );
        assert_eq!(clock.now(), started_at);
        clock.advance(Duration::from_micros(3)).unwrap();
        edges.borrow_mut().clear();
        assert_eq!(bus.try_finish_read(address), Ok(0xa5));
        assert_eq!(bus.try_finish_write(address), Ok(()));
        assert_eq!(
            *edges.borrow(),
            [
                Edge::Route(address),
                Edge::Busy(host),
                Edge::Data(host),
                Edge::Route(address),
                Edge::Busy(host),
            ]
        );
    }
}
