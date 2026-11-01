//! What the station's hart and the network's hart tell each other besides
//! the station's own channels: the station's exit and the network's end,
//! and the DHCP lease the network holds.
//!
//! No waker crosses harts: each side waits through its task's notify, which
//! the other hart raises after publishing (`oer_memory::QueueNotify`).

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, fence};
use core::task::{Context, Poll};

use oer_memory::QueueNotify;
use oer_services_wifi_station::Ipv4;

/// The station's exit asks the network to stop its IP service; the network
/// reports once the service ended and it withdrew its interface.
pub(crate) struct Exit<'a, N, S> {
    requested: AtomicBool,
    ended: AtomicBool,
    /// The network task's notify.
    network: &'a N,
    /// The station task's notify.
    station: &'a S,
}

impl<'a, N: QueueNotify, S: QueueNotify> Exit<'a, N, S> {
    pub(crate) const fn new(network: &'a N, station: &'a S) -> Self {
        Self {
            requested: AtomicBool::new(false),
            ended: AtomicBool::new(false),
            network,
            station,
        }
    }

    /// The station's end: ask the network to stop.
    pub(crate) fn request(&self) {
        self.requested.store(true, Ordering::Release);
        self.network.notify();
    }

    /// The network's end: ready once the station asked.
    pub(crate) async fn requested(&self) {
        poll_fn(|cx| ready(&self.requested, self.network, cx)).await;
    }

    /// The network's end: its IP service ended.
    pub(crate) fn end(&self) {
        self.ended.store(true, Ordering::Release);
        self.station.notify();
    }

    /// The station's end: ready once the network's IP service ended.
    pub(crate) async fn ended(&self) {
        poll_fn(|cx| ready(&self.ended, self.station, cx)).await;
    }
}

/// Register `notify` first, then read `flag`: a raise after the read wakes
/// this registration.
fn ready(flag: &AtomicBool, notify: &impl QueueNotify, cx: &mut Context<'_>) -> Poll<()> {
    notify.register(cx);
    if flag.load(Ordering::Acquire) {
        Poll::Ready(())
    } else {
        Poll::Pending
    }
}

/// The DHCP lease the network's IP service holds, published whole by its
/// one writer (the network's task) under a sequence number, odd while it
/// writes; a reader that sees the number change was concurrent with a
/// write.
pub(crate) struct Lease<'a, R> {
    sequence: AtomicU32,
    address: AtomicU32,
    gateway: AtomicU32,
    /// Bit 0: a lease; bit 1: its gateway; bits 8 to 15: its prefix.
    shape: AtomicU32,
    /// The reading task's notify, raised after each publication.
    reader: &'a R,
}

/// The lease's writer was writing while it was read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Concurrent;

const LEASED: u32 = 1;
const GATEWAY: u32 = 2;
const PREFIX_SHIFT: u32 = 8;

impl<'a, R: QueueNotify> Lease<'a, R> {
    pub(crate) const fn new(reader: &'a R) -> Self {
        Self {
            sequence: AtomicU32::new(0),
            address: AtomicU32::new(0),
            gateway: AtomicU32::new(0),
            shape: AtomicU32::new(0),
            reader,
        }
    }

    /// Publish `lease` and tell the reader; only the writer calls it.
    pub(crate) fn publish(&self, lease: Option<Ipv4>) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        let (address, gateway, shape) = match lease {
            None => (0, 0, 0),
            Some(Ipv4 {
                address,
                prefix,
                gateway,
            }) => (
                u32::from_le_bytes(address),
                gateway.map_or(0, u32::from_le_bytes),
                LEASED
                    | if gateway.is_some() { GATEWAY } else { 0 }
                    | u32::from(prefix) << PREFIX_SHIFT,
            ),
        };
        self.address.store(address, Ordering::Relaxed);
        self.gateway.store(gateway, Ordering::Relaxed);
        self.shape.store(shape, Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
        self.reader.notify();
    }

    /// Make the reading task the one a publication wakes; it reads next.
    pub(crate) fn register(&self, cx: &mut Context<'_>) {
        self.reader.register(cx);
    }

    /// The published lease.
    ///
    /// # Errors
    /// [`Concurrent`] while the writer writes.
    pub(crate) fn read(&self) -> Result<Option<Ipv4>, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let address = self.address.load(Ordering::Relaxed);
        let gateway = self.gateway.load(Ordering::Relaxed);
        let shape = self.shape.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        Ok((shape & LEASED != 0).then(|| Ipv4 {
            address: address.to_le_bytes(),
            prefix: (shape >> PREFIX_SHIFT).to_le_bytes()[0],
            gateway: (shape & GATEWAY != 0).then(|| gateway.to_le_bytes()),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::pin::pin;
    use core::sync::atomic::AtomicUsize;
    use core::task::Waker;
    use std::future::Future;

    /// A notify that counts its registrations and notifications.
    #[derive(Default)]
    struct Counted {
        registered: AtomicUsize,
        notified: AtomicUsize,
    }

    impl Counted {
        fn counts(&self) -> (usize, usize) {
            (
                self.registered.load(Ordering::Relaxed),
                self.notified.load(Ordering::Relaxed),
            )
        }
    }

    impl QueueNotify for Counted {
        fn register(&self, _: &mut Context<'_>) {
            self.registered.fetch_add(1, Ordering::Relaxed);
        }
        fn notify(&self) {
            self.notified.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn the_exit_wakes_the_network_and_its_end_wakes_the_station() {
        let (network, station) = (Counted::default(), Counted::default());
        let exit = Exit::new(&network, &station);
        let mut cx = Context::from_waker(Waker::noop());

        let mut requested = pin!(exit.requested());
        assert!(requested.as_mut().poll(&mut cx).is_pending());
        assert_eq!(network.counts(), (1, 0), "registered before it read");
        let mut ended = pin!(exit.ended());
        assert!(ended.as_mut().poll(&mut cx).is_pending());

        exit.request();
        assert_eq!(network.counts(), (1, 1), "the request raises the network");
        assert!(requested.as_mut().poll(&mut cx).is_ready());
        assert!(ended.as_mut().poll(&mut cx).is_pending());

        exit.end();
        assert_eq!(station.counts(), (2, 1), "the end raises the station");
        assert!(ended.as_mut().poll(&mut cx).is_ready());
    }

    #[test]
    fn a_lease_reads_back_whole_and_its_withdrawal_reads_as_none() {
        let reader = Counted::default();
        let lease = Lease::new(&reader);
        assert_eq!(lease.read(), Ok(None), "nothing leased yet");
        let leased = Ipv4 {
            address: [192, 168, 4, 23],
            prefix: 24,
            gateway: Some([192, 168, 4, 1]),
        };
        lease.publish(Some(leased));
        assert_eq!(lease.read(), Ok(Some(leased)));
        assert_eq!(
            reader.counts(),
            (0, 1),
            "each publication raises the reader"
        );
        let without_gateway = Ipv4 {
            address: [10, 0, 0, 2],
            prefix: 32,
            gateway: None,
        };
        lease.publish(Some(without_gateway));
        assert_eq!(lease.read(), Ok(Some(without_gateway)));
        lease.publish(None);
        assert_eq!(lease.read(), Ok(None));
        assert_eq!(reader.counts(), (0, 3));
        // The reader waits on its own notify, which the next publication
        // raises.
        lease.register(&mut Context::from_waker(Waker::noop()));
        lease.publish(Some(leased));
        assert_eq!(reader.counts(), (1, 4));
    }

    #[test]
    fn a_read_during_a_write_is_refused() {
        let reader = Counted::default();
        let lease = Lease::new(&reader);
        lease.sequence.store(1, Ordering::Relaxed);
        assert_eq!(lease.read(), Err(Concurrent));
    }
}
