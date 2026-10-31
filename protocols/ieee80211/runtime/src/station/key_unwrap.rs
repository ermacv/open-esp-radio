//! The service's one key-data unwrap slot.

use core::{
    future::Future,
    pin::Pin,
    task::{Context, Poll},
};
use oer_crypto::KeyUnwrap;
use oer_ieee80211_core_roles_sta::machine::{
    KEY_DATA_CAPACITY, KeyUnwrapId, StaKeyUnwrapRequest, StaKeyUnwrapResponse,
};

/// The composition's key-unwrap engine and the station's one operation in
/// flight. The image chooses the engine as a static type; the service runs
/// it beside the port's events, commands and deadlines.
///
/// The caller owns the slot, so an operation outlives a dropped service run:
/// its buffer stays with the engine's operation until the engine finishes,
/// and the next run drains it before serving. A service that ends normally
/// waits for its operation and returns the station's ticket first.
pub struct KeyUnwrapSlot<E: KeyUnwrap<KEY_DATA_CAPACITY>> {
    state: SlotState<E>,
}

enum SlotState<E: KeyUnwrap<KEY_DATA_CAPACITY>> {
    Idle(E),
    Running {
        id: KeyUnwrapId,
        operation: E::Operation,
    },
    /// Only within one call, while the engine or its operation moves.
    Moving,
}

impl<E: KeyUnwrap<KEY_DATA_CAPACITY, Operation: Unpin>> KeyUnwrapSlot<E> {
    pub const fn new(engine: E) -> Self {
        Self {
            state: SlotState::Idle(engine),
        }
    }

    /// An operation is in flight.
    pub fn is_running(&self) -> bool {
        matches!(self.state, SlotState::Running { .. })
    }

    /// The engine, when no operation is in flight; otherwise the slot.
    pub fn into_engine(self) -> Result<E, Self> {
        match self.state {
            SlotState::Idle(engine) => Ok(engine),
            state => Err(Self { state }),
        }
    }

    /// Move `request` into the engine; a slot already running returns it.
    #[expect(
        clippy::result_large_err,
        reason = "a refused request returns by value; the service has no allocator"
    )]
    pub(super) fn start(
        &mut self,
        id: KeyUnwrapId,
        request: StaKeyUnwrapRequest,
    ) -> Result<(), (KeyUnwrapId, StaKeyUnwrapRequest)> {
        match core::mem::replace(&mut self.state, SlotState::Moving) {
            SlotState::Idle(engine) => {
                self.state = SlotState::Running {
                    id,
                    operation: engine.unwrap_key(request),
                };
                Ok(())
            }
            state => {
                self.state = state;
                Err((id, request))
            }
        }
    }

    /// The finished operation's ticket and response; pending while it runs
    /// and while the slot is idle.
    pub(super) fn poll_done(
        &mut self,
        context: &mut Context<'_>,
    ) -> Poll<(KeyUnwrapId, StaKeyUnwrapResponse)> {
        // An idle slot stays in place: moving the engine out and back on
        // every poll copies it twice.
        if !self.is_running() {
            return Poll::Pending;
        }
        match core::mem::replace(&mut self.state, SlotState::Moving) {
            SlotState::Running { id, mut operation } => {
                match Pin::new(&mut operation).poll(context) {
                    Poll::Ready(done) => {
                        self.state = SlotState::Idle(done.engine);
                        Poll::Ready((id, done.response))
                    }
                    Poll::Pending => {
                        self.state = SlotState::Running { id, operation };
                        Poll::Pending
                    }
                }
            }
            state => {
                self.state = state;
                Poll::Pending
            }
        }
    }

    /// Wait for the operation in flight, if any.
    pub(super) async fn finish(&mut self) -> Option<(KeyUnwrapId, StaKeyUnwrapResponse)> {
        if !self.is_running() {
            return None;
        }
        Some(core::future::poll_fn(|context| self.poll_done(context)).await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::cell::Cell;
    use core::pin::pin;
    use core::task::Waker;
    use oer_crypto::{Aes128Key, KeyData, KeyUnwrapDone};
    use oer_ieee80211_core_security_cipher::aes::{SoftwareAes128, SoftwareKeyUnwrap};

    /// An engine whose operation finishes once `release` is set, recording
    /// whether it was dropped while it still held the request.
    struct Gated<'a> {
        release: &'a Cell<bool>,
        dropped_running: &'a Cell<bool>,
    }

    struct GatedOperation<'a> {
        engine: Option<Gated<'a>>,
        request: Option<StaKeyUnwrapRequest>,
    }

    impl Drop for GatedOperation<'_> {
        fn drop(&mut self) {
            if let Some(engine) = &self.engine
                && self.request.is_some()
            {
                engine.dropped_running.set(true);
            }
        }
    }

    impl<'a> Future for GatedOperation<'a> {
        type Output = KeyUnwrapDone<Gated<'a>, KEY_DATA_CAPACITY>;
        fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<Self::Output> {
            if !self.engine.as_ref().unwrap().release.get() {
                return Poll::Pending;
            }
            let request = self.request.take().unwrap();
            Poll::Ready(KeyUnwrapDone {
                engine: self.engine.take().unwrap(),
                response: SoftwareKeyUnwrap::new(SoftwareAes128).unwrap_now(request),
            })
        }
    }

    impl<'a> KeyUnwrap<KEY_DATA_CAPACITY> for Gated<'a> {
        type Operation = GatedOperation<'a>;
        fn unwrap_key(self, request: StaKeyUnwrapRequest) -> GatedOperation<'a> {
            GatedOperation {
                engine: Some(self),
                request: Some(request),
            }
        }
    }

    fn request() -> StaKeyUnwrapRequest {
        StaKeyUnwrapRequest::new(
            Aes128Key::copy_from(&oer_crypto::conformance::RFC3394_4_1_KEK),
            KeyData::try_copy(&oer_crypto::conformance::RFC3394_4_1_CIPHERTEXT).unwrap(),
        )
    }

    #[test]
    fn a_dropped_wait_keeps_the_operation_and_its_buffer_until_the_engine_finishes() {
        let release = Cell::new(false);
        let dropped_running = Cell::new(false);
        let mut slot = KeyUnwrapSlot::new(Gated {
            release: &release,
            dropped_running: &dropped_running,
        });
        let mut context = Context::from_waker(Waker::noop());
        let id = KeyUnwrapId::for_test();
        assert!(slot.start(id, request()).is_ok());
        {
            let mut wait = pin!(slot.finish());
            assert!(wait.as_mut().poll(&mut context).is_pending());
        }
        assert!(slot.is_running());
        assert!(!dropped_running.get());

        // A second request returns at once: the slot holds one operation.
        let (refused, _) = slot
            .start(KeyUnwrapId::for_test(), request())
            .err()
            .unwrap();
        drop(refused);
        let slot = slot.into_engine().err().unwrap();

        release.set(true);
        let mut slot = slot;
        let mut wait = pin!(slot.finish());
        let Poll::Ready(Some((_, response))) = wait.as_mut().poll(&mut context) else {
            panic!("the released operation finishes")
        };
        assert_eq!(
            response.plaintext(),
            Ok(&oer_crypto::conformance::RFC3394_4_1_KEY_DATA[..])
        );
        assert!(!dropped_running.get());
    }
}
