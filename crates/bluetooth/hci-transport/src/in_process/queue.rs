//! Async packet FIFO and cancellation-safe readiness registration.

use super::*;

pub(super) struct AsyncPacketQueue<M, const DEPTH: usize, const PACKET_CAPACITY: usize>
where
    M: RawMutex,
{
    state: Mutex<M, RefCell<AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>>>,
}

impl<M, const DEPTH: usize, const PACKET_CAPACITY: usize>
    AsyncPacketQueue<M, DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    pub(super) const fn new() -> Self {
        Self {
            state: Mutex::new(RefCell::new(AsyncPacketQueueState::new())),
        }
    }

    pub(super) fn epoch(&self) -> PacketQueueEpoch<'_, M, DEPTH, PACKET_CAPACITY> {
        PacketQueueEpoch {
            queue: self,
            generation: self.state.lock(|state| state.borrow().generation),
        }
    }

    pub(super) fn is_pristine(&self) -> bool {
        self.epoch().is_pristine()
    }

    #[cfg(test)]
    pub(super) fn vacant_storage_is_zeroed(&self) -> bool {
        self.epoch().vacant_storage_is_zeroed()
    }
}

/// A queue reference bound to one non-wrapping transport generation.
/// Every check and mutation shares the queue lock, including waker registration.
pub(super) struct PacketQueueEpoch<'a, M: RawMutex, const DEPTH: usize, const PC: usize> {
    queue: &'a AsyncPacketQueue<M, DEPTH, PC>,
    generation: u64,
}
impl<M: RawMutex, const DEPTH: usize, const PC: usize> Copy for PacketQueueEpoch<'_, M, DEPTH, PC> {}
impl<M: RawMutex, const DEPTH: usize, const PC: usize> Clone
    for PacketQueueEpoch<'_, M, DEPTH, PC>
{
    fn clone(&self) -> Self {
        *self
    }
}
impl<'a, M: RawMutex, const DEPTH: usize, const PACKET_CAPACITY: usize>
    PacketQueueEpoch<'a, M, DEPTH, PACKET_CAPACITY>
{
    pub(super) const fn generation(self) -> u64 {
        self.generation
    }

    fn admit_restart<const C2H: usize>(
        self,
        outgoing: PacketQueueEpoch<'a, M, C2H, PACKET_CAPACITY>,
        incoming: &AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>,
        out: &AsyncPacketQueueState<C2H, PACKET_CAPACITY>,
    ) -> Result<u64, super::HciRestartError> {
        use super::HciRestartError as Error;
        if incoming.generation != self.generation
            || out.generation != outgoing.generation
            || self.generation != outgoing.generation
        {
            return Err(Error::EpochMismatch);
        }
        if !incoming.closed || !out.closed || incoming.length != 0 || out.length != 0 {
            return Err(Error::NotRetired);
        }
        self.generation
            .checked_add(1)
            .ok_or(Error::GenerationExhausted)
    }

    pub(super) fn restart_with<const C2H: usize>(
        self,
        outgoing: PacketQueueEpoch<'a, M, C2H, PACKET_CAPACITY>,
    ) -> Result<(Self, PacketQueueEpoch<'a, M, C2H, PACKET_CAPACITY>), super::HciRestartError> {
        self.queue.state.lock(|incoming| {
            outgoing.queue.state.lock(|outgoing_state| {
                let mut incoming = incoming.borrow_mut();
                let mut out = outgoing_state.borrow_mut();
                let generation = self.admit_restart(outgoing, &incoming, &out)?;
                incoming.restart(generation);
                out.restart(generation);
                Ok((
                    Self { generation, ..self },
                    PacketQueueEpoch {
                        generation,
                        ..outgoing
                    },
                ))
            })
        })
    }

    pub(super) async fn send(
        &self,
        packet: PacketSlot<PACKET_CAPACITY>,
    ) -> Result<(), HciChannelError> {
        poll_fn(|context| {
            self.queue.state.lock(|state| {
                let mut state = state.borrow_mut();
                match state.try_send_for(self.generation, packet) {
                    Err(HciChannelError::Full) => {
                        state.sender_waker.register(context.waker());
                        Poll::Pending
                    }
                    result => Poll::Ready(result),
                }
            })
        })
        .await
    }

    pub(super) async fn wait_send_ready(&self) {
        poll_fn(|context| {
            self.queue.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.generation != self.generation || state.closed || state.length < DEPTH {
                    Poll::Ready(())
                } else {
                    state.sender_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) fn try_send(
        &self,
        packet: PacketSlot<PACKET_CAPACITY>,
    ) -> Result<(), HciChannelError> {
        self.queue
            .state
            .lock(|state| state.borrow_mut().try_send_for(self.generation, packet))
    }

    /// Observe a drained pair without consuming packets or closing admission.
    /// Dedicated waiters do not replace ordinary packet/capacity registrations.
    pub(super) async fn wait_drained_with<const C2H: usize>(
        self,
        outgoing: PacketQueueEpoch<'_, M, C2H, PACKET_CAPACITY>,
    ) -> Result<(), super::HciRetirementError> {
        poll_fn(|context| {
            self.queue.state.lock(|incoming| {
                outgoing.queue.state.lock(|outgoing| {
                    let mut incoming = incoming.borrow_mut();
                    let mut outgoing = outgoing.borrow_mut();
                    if incoming.generation != self.generation
                        || outgoing.generation != self.generation
                        || incoming.closed
                        || outgoing.closed
                    {
                        return Poll::Ready(Err(super::HciRetirementError::Closed));
                    }
                    if incoming.length == 0 && outgoing.length == 0 {
                        return Poll::Ready(Ok(()));
                    }
                    incoming.drain_waker.register(context.waker());
                    outgoing.drain_waker.register(context.waker());
                    Poll::Pending
                })
            })
        })
        .await
    }

    /// Always lock Host-to-Controller before Controller-to-Host, as in the drain
    /// observer. The two queues are distinct fields of the same channel.
    pub(super) fn try_retire_with<const C2H: usize>(
        self,
        outgoing: PacketQueueEpoch<'_, M, C2H, PACKET_CAPACITY>,
    ) -> Result<(), super::HciRetirementError> {
        use super::HciRetirementError as Error;
        self.queue.state.lock(|incoming| {
            outgoing.queue.state.lock(|outgoing| {
                let mut incoming = incoming.borrow_mut();
                let mut outgoing = outgoing.borrow_mut();
                if incoming.generation != self.generation
                    || outgoing.generation != self.generation
                    || incoming.closed
                    || outgoing.closed
                {
                    return Err(Error::Closed);
                }
                if incoming.length != 0 {
                    return Err(Error::HostPacketsPending);
                }
                if outgoing.length != 0 {
                    return Err(Error::ControllerPacketsPending);
                }
                incoming.close();
                outgoing.close();
                Ok(())
            })
        })
    }

    /// Seal admission under the same lock as publication, preserving the FIFO.
    pub(super) fn close(&self) {
        self.queue.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.generation == self.generation {
                state.close();
            }
        });
    }

    pub(super) async fn receive(&self) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
        poll_fn(|context| {
            self.queue.state.lock(|state| {
                let mut state = state.borrow_mut();
                match state.try_receive_for(self.generation) {
                    Err(HciChannelError::Empty) => {
                        state.receiver_waker.register(context.waker());
                        Poll::Pending
                    }
                    result => Poll::Ready(result),
                }
            })
        })
        .await
    }

    pub(super) async fn wait_receive_ready(&self) {
        poll_fn(|context| {
            self.queue.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.generation != self.generation || state.closed || state.length > 0 {
                    Poll::Ready(())
                } else {
                    state.receiver_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) fn try_receive(&self) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
        self.queue
            .state
            .lock(|state| state.borrow_mut().try_receive_for(self.generation))
    }

    /// Active ACL backpressure retains data while commands keep progressing.
    pub(super) fn try_receive_admitted(
        &self,
        admit_acl: impl Fn(ConnHandle) -> bool,
    ) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
        self.queue.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.generation != self.generation {
                return Err(HciChannelError::Closed);
            }
            state.try_receive_admitted(&admit_acl)
        })
    }

    pub(super) async fn wait_receive_admitted(&self, admit_acl: impl Fn(ConnHandle) -> bool) {
        poll_fn(|context| {
            self.queue.state.lock(|state| {
                let mut state = state.borrow_mut();
                if state.generation != self.generation
                    || state.closed
                    || state.admitted_offset(&admit_acl).is_some()
                {
                    Poll::Ready(())
                } else {
                    state.receiver_waker.register(context.waker());
                    Poll::Pending
                }
            })
        })
        .await
    }

    pub(super) fn is_pristine(&self) -> bool {
        self.queue.state.lock(|state| {
            let state = state.borrow();
            !state.closed
                && state.generation == self.generation
                && state.length == 0
                && !state.has_published_packet
        })
    }

    #[cfg(test)]
    pub(super) fn vacant_storage_is_zeroed(&self) -> bool {
        self.queue.state.lock(|state| {
            let state = state.borrow();
            state
                .slots
                .iter()
                .all(|slot| slot.length != 0 || slot.bytes.iter().all(|byte| *byte == 0))
        })
    }
}

struct AsyncPacketQueueState<const DEPTH: usize, const PACKET_CAPACITY: usize> {
    slots: [PacketSlot<PACKET_CAPACITY>; DEPTH],
    head: usize,
    length: usize,
    has_published_packet: bool,
    closed: bool,
    generation: u64,
    receiver_waker: WakerRegistration,
    sender_waker: WakerRegistration,
    drain_waker: WakerRegistration,
}

impl<const DEPTH: usize, const PACKET_CAPACITY: usize>
    AsyncPacketQueueState<DEPTH, PACKET_CAPACITY>
{
    const fn new() -> Self {
        Self {
            slots: [PacketSlot::EMPTY; DEPTH],
            head: 0,
            length: 0,
            has_published_packet: false,
            closed: false,
            generation: 0,
            receiver_waker: WakerRegistration::new(),
            sender_waker: WakerRegistration::new(),
            drain_waker: WakerRegistration::new(),
        }
    }

    fn restart(&mut self, generation: u64) {
        self.sender_waker.wake();
        self.receiver_waker.wake();
        self.drain_waker.wake();
        self.generation = generation;
        self.closed = false;
        self.has_published_packet = false;
        self.head = 0;
    }

    fn try_send_for(
        &mut self,
        generation: u64,
        packet: PacketSlot<PACKET_CAPACITY>,
    ) -> Result<(), HciChannelError> {
        if generation != self.generation {
            return Err(HciChannelError::Closed);
        }
        self.try_send(packet)
    }
    fn try_receive_for(
        &mut self,
        generation: u64,
    ) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
        if generation != self.generation {
            return Err(HciChannelError::Closed);
        }
        self.try_receive()
    }

    fn close(&mut self) {
        self.closed = true;
        self.sender_waker.wake();
        self.receiver_waker.wake();
        self.drain_waker.wake();
    }

    fn try_send(&mut self, packet: PacketSlot<PACKET_CAPACITY>) -> Result<(), HciChannelError> {
        if self.closed {
            return Err(HciChannelError::Closed);
        }
        if self.length == DEPTH {
            return Err(HciChannelError::Full);
        }
        let tail = (self.head + self.length) % DEPTH;
        self.slots[tail] = packet;
        self.length += 1;
        self.has_published_packet = true;
        self.receiver_waker.wake();
        Ok(())
    }

    fn try_receive(&mut self) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
        self.try_receive_admitted(&|_| true)
    }

    /// The oldest packet the receiver takes now: a command, or ACL data
    /// whose connection handle `admit_acl` admits. A handle refused once
    /// refuses its later packets too, so each handle keeps its order.
    fn admitted_offset(&self, admit_acl: &impl Fn(ConnHandle) -> bool) -> Option<usize> {
        (0..self.length).find(|offset| {
            let slot = &self.slots[(self.head + offset) % DEPTH];
            slot.kind != PacketKind::AclData
                || admit_acl(ConnHandle::new(
                    u16::from_le_bytes([slot.bytes[0], slot.bytes[1]]) & 0x0fff,
                ))
        })
    }

    fn try_receive_admitted(
        &mut self,
        admit_acl: &impl Fn(ConnHandle) -> bool,
    ) -> Result<PacketSlot<PACKET_CAPACITY>, HciChannelError> {
        let Some(offset) = self.admitted_offset(admit_acl) else {
            return Err(if self.closed {
                HciChannelError::Closed
            } else {
                HciChannelError::Empty
            });
        };
        let packet = self.slots[(self.head + offset) % DEPTH];
        // Compact only the prefix before the selected command. ACL and command
        // order each remain FIFO; no second packet store or queue credit exists.
        for index in (1..=offset).rev() {
            self.slots[(self.head + index) % DEPTH] = self.slots[(self.head + index - 1) % DEPTH];
        }
        self.slots[self.head].bytes.fill(0);
        self.slots[self.head].length = 0;
        self.head = (self.head + 1) % DEPTH;
        self.length -= 1;
        self.sender_waker.wake();
        if self.length == 0 {
            self.drain_waker.wake();
        }
        Ok(packet)
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    use core::{
        future::Future,
        pin::pin,
        task::{Context, Waker},
    };
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;

    #[test]
    fn old_pending_waiters_cannot_read_write_or_close_the_replacement_epoch() {
        let incoming = AsyncPacketQueue::<NoopRawMutex, 1, 8>::new();
        let outgoing = AsyncPacketQueue::<NoopRawMutex, 1, 8>::new();
        let a = incoming.epoch();
        let b = outgoing.epoch();
        let packet = PacketSlot::EMPTY;
        a.try_send(packet).unwrap();
        let mut blocked_write = pin!(a.send(packet));
        let mut blocked_read = pin!(b.receive());
        let mut context = Context::from_waker(Waker::noop());
        assert!(blocked_write.as_mut().poll(&mut context).is_pending());
        assert!(blocked_read.as_mut().poll(&mut context).is_pending());
        a.try_receive().unwrap();
        a.try_retire_with(b).unwrap();
        let (fresh_a, fresh_b) = a.restart_with(b).unwrap();
        fresh_b.try_send(packet).unwrap();
        assert!(matches!(
            blocked_write.as_mut().poll(&mut context),
            Poll::Ready(Err(HciChannelError::Closed))
        ));
        assert!(matches!(
            blocked_read.as_mut().poll(&mut context),
            Poll::Ready(Err(HciChannelError::Closed))
        ));
        a.close();
        b.close();
        assert!(
            fresh_b.try_receive().is_ok(),
            "old receiver must not steal new data"
        );
        assert!(
            fresh_a.try_send(packet).is_ok(),
            "old writer/close must not enter the new queue"
        );
        assert!(fresh_a.try_receive().is_ok());
    }

    #[test]
    fn restart_is_atomic_on_live_queues_stale_generation_and_exhaustion() {
        let incoming = AsyncPacketQueue::<NoopRawMutex, 1, 8>::new();
        let outgoing = AsyncPacketQueue::<NoopRawMutex, 1, 8>::new();
        let a = incoming.epoch();
        let b = outgoing.epoch();
        assert!(matches!(
            a.restart_with(b),
            Err(super::HciRestartError::NotRetired)
        ));
        a.try_send(PacketSlot::EMPTY).unwrap();
        a.close();
        b.close();
        assert!(matches!(
            a.restart_with(b),
            Err(super::HciRestartError::NotRetired)
        ));
        a.try_receive().unwrap();
        let (fresh_a, fresh_b) = a.restart_with(b).unwrap();
        assert!(matches!(
            a.restart_with(b),
            Err(super::HciRestartError::EpochMismatch)
        ));
        fresh_a.try_retire_with(fresh_b).unwrap();
        incoming
            .state
            .lock(|state| state.borrow_mut().generation = u64::MAX);
        outgoing
            .state
            .lock(|state| state.borrow_mut().generation = u64::MAX);
        let a = incoming.epoch();
        let b = outgoing.epoch();
        assert!(matches!(
            a.restart_with(b),
            Err(super::HciRestartError::GenerationExhausted)
        ));
        assert!(matches!(
            a.try_send(PacketSlot::EMPTY),
            Err(HciChannelError::Closed)
        ));
        assert!(matches!(b.try_receive(), Err(HciChannelError::Closed)));
    }
}

#[cfg(test)]
mod admission_tests {
    use super::*;
    use core::{future::Future, pin::pin, task::Context};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        task::{Wake, Waker},
    };

    fn packet(kind: PacketKind, value: u8) -> PacketSlot<8> {
        PacketSlot {
            kind,
            length: 1,
            bytes: [value; 8],
        }
    }

    #[test]
    fn bypass_preserves_each_fifo_across_ring_wrap_and_zeros_vacated_storage() {
        for rotation in 0..4 {
            let queue = AsyncPacketQueue::<NoopRawMutex, 4, 8>::new();
            let epoch = queue.epoch();
            for _ in 0..rotation {
                epoch.try_send(packet(PacketKind::Cmd, 99)).unwrap();
                epoch.try_receive().unwrap();
            }
            for (kind, value) in [
                (PacketKind::AclData, 1),
                (PacketKind::Cmd, 2),
                (PacketKind::AclData, 3),
                (PacketKind::Cmd, 4),
            ] {
                epoch.try_send(packet(kind, value)).unwrap();
            }
            assert_eq!(epoch.try_receive_admitted(|_| false).unwrap().bytes[0], 2);
            epoch.try_send(packet(PacketKind::AclData, 5)).unwrap();
            assert_eq!(epoch.try_receive_admitted(|_| false).unwrap().bytes[0], 4);
            assert!(matches!(
                epoch.try_receive_admitted(|_| false),
                Err(HciChannelError::Empty)
            ));
            for value in [1, 3, 5] {
                assert_eq!(
                    epoch.try_receive_admitted(|_| true).unwrap().bytes[0],
                    value
                );
            }
            assert!(epoch.vacant_storage_is_zeroed());
        }
    }

    #[test]
    fn a_busy_handle_holds_back_only_its_own_data() {
        let queue = AsyncPacketQueue::<NoopRawMutex, 4, 8>::new();
        let epoch = queue.epoch();
        // ACL data opens with its handle, little-endian.
        for handle in [1, 2, 1] {
            let mut bytes = [0; 8];
            bytes[0] = handle;
            epoch
                .try_send(PacketSlot {
                    kind: PacketKind::AclData,
                    length: 4,
                    bytes,
                })
                .unwrap();
        }
        let only_two = |handle: ConnHandle| handle.into_inner() == 2;
        assert_eq!(epoch.try_receive_admitted(only_two).unwrap().bytes[0], 2);
        assert!(matches!(
            epoch.try_receive_admitted(only_two),
            Err(HciChannelError::Empty)
        ));
        for _ in 0..2 {
            assert_eq!(epoch.try_receive_admitted(|_| true).unwrap().bytes[0], 1);
        }
    }

    struct Wakes(AtomicUsize);
    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn blocked_data_does_not_spin_and_command_publication_wakes_a_cancelled_replacement() {
        let queue = AsyncPacketQueue::<NoopRawMutex, 4, 8>::new();
        let epoch = queue.epoch();
        epoch.try_send(packet(PacketKind::AclData, 1)).unwrap();
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(wakes.clone());
        let mut cx = Context::from_waker(&waker);
        {
            let mut cancelled = pin!(epoch.wait_receive_admitted(|_| false));
            assert!(cancelled.as_mut().poll(&mut cx).is_pending());
        }
        let mut replacement = pin!(epoch.wait_receive_admitted(|_| false));
        assert!(replacement.as_mut().poll(&mut cx).is_pending());
        epoch.try_send(packet(PacketKind::Cmd, 2)).unwrap();
        assert!(wakes.0.load(Ordering::Relaxed) > 0);
        assert!(replacement.as_mut().poll(&mut cx).is_ready());
        assert_eq!(epoch.try_receive_admitted(|_| false).unwrap().bytes[0], 2);
        assert_eq!(epoch.try_receive_admitted(|_| true).unwrap().bytes[0], 1);
        epoch.close();
        assert!(matches!(
            epoch.try_receive_admitted(|_| false),
            Err(HciChannelError::Closed)
        ));
    }
}
