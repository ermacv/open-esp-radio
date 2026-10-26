extern crate std;

use self::std::{boxed::Box, cell::RefCell, rc::Rc};
use core::cell::Cell;

use super::*;

std::thread_local! {
    static RETURN_OBSERVER: RefCell<Option<Box<dyn Fn()>>> = const { RefCell::new(None) };
}

pub(super) fn observe_return() {
    RETURN_OBSERVER.with(|observer| {
        if let Some(observer) = observer.borrow().as_ref() {
            observer();
        }
    });
}

/// Clear the thread-local observer even when an assertion unwinds.
struct ReturnObserver(Rc<Cell<usize>>);

impl ReturnObserver {
    fn new(observe: impl Fn() + 'static) -> Self {
        let calls = Rc::new(Cell::new(0));
        let observed_calls = Rc::clone(&calls);
        RETURN_OBSERVER.with(|observer| {
            assert!(observer.borrow().is_none());
            *observer.borrow_mut() = Some(Box::new(move || {
                observed_calls.set(observed_calls.get() + 1);
                observe();
            }));
        });
        Self(calls)
    }

    fn calls(&self) -> usize {
        self.0.get()
    }
}

impl Drop for ReturnObserver {
    fn drop(&mut self) {
        RETURN_OBSERVER.with(|observer| *observer.borrow_mut() = None);
    }
}

struct Allocation {
    ret: &'static ExternalRxReturn,
    pointer: NonNull<u8>,
}

impl Allocation {
    fn new() -> Self {
        Self {
            ret: Box::leak(Box::new(ExternalRxReturn::new())),
            pointer: NonNull::from(Box::leak(Box::new([0_u8; 64]))).cast(),
        }
    }

    fn detach(&self, length: usize) -> Result<ExternalRxBuffer, ExternalRxReturnState> {
        #[allow(
            unsafe_code,
            reason = "test owns one leaked allocation and its return state"
        )]
        // SAFETY: each allocation has its own leaked state and initialized
        // bytes; only a successfully detached buffer accesses those bytes.
        unsafe {
            self.ret.detach(self.pointer, length, 64)
        }
    }
}

fn observe_release_state(
    pool: &'static ExternalRxHandoffPool<64, 1>,
    ret: &'static ExternalRxReturn,
) -> ReturnObserver {
    ReturnObserver::new(move || {
        assert_eq!(ret.state(), ExternalRxReturnState::Returned);
        assert_eq!(
            pool.slots[0].state.load(Ordering::Acquire),
            SLOT_RELEASING,
            "handoff credit became free before its DMA buffer was released"
        );
    })
}

#[test]
fn dma_buffer_releases_before_handoff_credit_becomes_free() {
    let pool = &*Box::leak(Box::new(ExternalRxHandoffPool::<64, 1>::new()));
    let allocation = Allocation::new();
    let observer = observe_release_state(pool, allocation.ret);
    let Ok(radio) = pool.try_claim_radio(allocation.detach(64).unwrap(), 0) else {
        panic!("fresh handoff slot must accept one buffer");
    };
    let index = radio.republish(0, 64);
    let network = pool.claim_network(index);
    assert_eq!(network.release(), 0);

    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 1);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn an_adopted_network_slot_exposes_its_view_and_returns_on_release() {
    let pool = &*Box::leak(Box::new(ExternalRxHandoffPool::<64, 1>::new()));
    let allocation = Allocation::new();
    let observer = observe_release_state(pool, allocation.ret);
    let Ok(radio) = pool.try_claim_radio(allocation.detach(48).unwrap(), 0) else {
        panic!("fresh handoff slot must accept one buffer");
    };
    let index = radio.republish(10, 30);
    let network = pool.claim_network(index);
    let view = network.adoption_view();
    assert_eq!(pool.network_slots(), 1);
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Detached);
    assert_eq!(observer.calls(), 0);
    assert_eq!(network.frame().len(), view.length);
    let adoption = network.into_adoption();
    assert_eq!(adoption, view);
    #[allow(unsafe_code, reason = "test compares the adopted address")]
    // SAFETY: offset 10 lies inside the 64-byte test allocation.
    let expected = unsafe { allocation.pointer.add(10) };
    assert_eq!(
        adoption,
        ExternalRxAdoption {
            index,
            data: expected,
            length: 30,
            available: 54,
        }
    );
    assert_eq!(pool.network_slots(), 1);
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Detached);
    assert_eq!(observer.calls(), 0);
    #[allow(unsafe_code, reason = "test returns its one adoption")]
    // SAFETY: the adoption above is released exactly once and not accessed after.
    unsafe {
        pool.release_adopted(adoption.index)
    };
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 1);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn a_zeroed_pool_is_free_and_binds_without_releasing_a_previous_buffer() {
    let pool = crate::zeroed::zeroed::<ExternalRxHandoffPool<64, 2>>();
    assert_eq!(pool.claimed_slots(), 0);
    let allocation = Allocation::new();
    let observer = ReturnObserver::new(|| {});
    let Ok(radio) = pool.try_claim_radio(allocation.detach(64).unwrap(), 0) else {
        panic!("a zeroed slot must accept one buffer");
    };
    // A free slot holds no buffer, so binding one releases nothing.
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Detached);
    assert_eq!(observer.calls(), 0);
    let index = radio.republish(0, 64);
    assert_eq!(pool.claim_network(index).release(), 0);
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 1);
}

#[test]
fn dropping_the_pool_releases_a_published_buffer_once() {
    let allocation = Allocation::new();
    let observer = ReturnObserver::new(|| {});
    {
        let pool = ExternalRxHandoffPool::<64, 1>::new();
        let Ok(radio) = pool.try_claim_radio(allocation.detach(64).unwrap(), 0) else {
            panic!("a fresh slot must accept one buffer");
        };
        let _ready = radio.republish(0, 64);
        assert_eq!(allocation.ret.state(), ExternalRxReturnState::Detached);
        assert_eq!(observer.calls(), 0);
    }
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 1);
}

#[test]
fn buffer_drop_and_explicit_release_each_return_exactly_once() {
    let allocation = Allocation::new();
    let observer = ReturnObserver::new(|| {});
    drop(allocation.detach(64).unwrap());
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 1);

    let allocation = Allocation::new();
    let mut buffer = allocation.detach(64).unwrap();
    buffer.release_once();
    buffer.release_once();
    drop(buffer);
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 2);
}

#[test]
fn network_drop_returns_once_before_freeing_the_slot() {
    let pool = &*Box::leak(Box::new(ExternalRxHandoffPool::<64, 1>::new()));
    let allocation = Allocation::new();
    let observer = observe_release_state(pool, allocation.ret);
    let Ok(radio) = pool.try_claim_radio(allocation.detach(64).unwrap(), 0) else {
        panic!("fresh handoff slot must accept one buffer");
    };
    let index = radio.republish(0, 64);
    let network = pool.claim_network(index);
    let view = network.adoption_view();
    assert_eq!(view.length, 64);
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Detached);
    assert_eq!(observer.calls(), 0);
    drop(network);
    assert_eq!(allocation.ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(observer.calls(), 1);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
#[should_panic(expected = "detached RX DMA buffer must be released exactly once")]
fn returning_the_same_state_twice_panics() {
    let allocation = Allocation::new();
    drop(allocation.detach(64).unwrap());
    allocation.ret.return_once();
}

#[test]
fn return_state_preserves_the_complete_cycle_and_refuses_live_owners() {
    let allocation = Allocation::new();
    let ret = allocation.ret;
    assert_eq!(ret.state(), ExternalRxReturnState::Attached);
    assert_eq!(ret.reattach(), Ok(()));
    let buffer = allocation.detach(64).unwrap();
    assert_eq!(ret.state(), ExternalRxReturnState::Detached);
    assert_eq!(
        allocation.detach(64).err(),
        Some(ExternalRxReturnState::Detached)
    );
    assert_eq!(ret.reattach(), Err(ExternalRxReturnState::Detached));
    drop(buffer);
    assert_eq!(ret.state(), ExternalRxReturnState::Returned);
    assert_eq!(
        allocation.detach(64).err(),
        Some(ExternalRxReturnState::Returned)
    );
    assert_eq!(ret.reattach(), Ok(()));
    assert_eq!(ret.state(), ExternalRxReturnState::Attached);
    drop(allocation.detach(64).unwrap());
    assert_eq!(ret.state(), ExternalRxReturnState::Returned);
}

#[test]
fn return_state_has_the_atomic_layout_and_zero_is_attached() {
    use core::mem::{align_of, size_of};
    assert_eq!(size_of::<ExternalRxReturn>(), size_of::<AtomicU8>());
    assert_eq!(align_of::<ExternalRxReturn>(), align_of::<AtomicU8>());
    let ret = Box::leak(Box::new(crate::zeroed::zeroed::<ExternalRxReturn>()));
    assert_eq!(ret.state(), ExternalRxReturnState::Attached);
    assert_eq!(ret.0.load(Ordering::Acquire), 0);
    let pointer = NonNull::from(Box::leak(Box::new([0_u8; 64]))).cast();
    #[allow(
        unsafe_code,
        reason = "test owns leaked initialized storage and its state"
    )]
    // SAFETY: the leaked storage has no DMA or other CPU owner.
    let buffer = unsafe { ret.detach(pointer, 64, 64) }.unwrap();
    assert_eq!(ret.0.load(Ordering::Acquire), 1);
    drop(buffer);
    assert_eq!(ret.0.load(Ordering::Acquire), 2);
}
