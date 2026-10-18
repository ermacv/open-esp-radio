use super::{Device, Event};
use core::task::Waker;
use embassy_net::driver::{self, Driver, PacketBuf};
impl<D: Driver> Driver for Device<D> {
    fn capabilities(&self) -> driver::Capabilities {
        self.inner.capabilities()
    }
    fn hardware_address(&self) -> driver::HardwareAddress {
        self.inner.hardware_address()
    }
    fn link_state(&mut self) -> driver::LinkState {
        self.inner.link_state()
    }
    fn register_waker(&mut self, waker: &Waker) -> Result<(), driver::NotSupported> {
        self.inner.register_waker(waker)
    }
    fn receive(&mut self) -> Option<PacketBuf> {
        let packet = self.inner.receive();
        self.counters.record(if packet.is_some() {
            Event::RxDelivered
        } else {
            Event::RxEmpty
        });
        packet
    }
    fn can_transmit(&mut self) -> bool {
        let ready = self.inner.can_transmit();
        self.counters.record(if ready {
            Event::TxReady
        } else {
            Event::TxUnavailable
        });
        ready
    }
    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        let result = self.inner.transmit(packet);
        self.counters.record(if result.is_ok() {
            Event::TxAccepted
        } else {
            Event::TxRejected
        });
        result
    }
}
