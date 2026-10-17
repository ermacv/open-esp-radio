use core::{
    ptr::NonNull,
    sync::atomic::{AtomicU8, Ordering},
};

use super::*;

struct ReleaseProbe {
    slot: *const ExternalRxHandoffSlot,
    calls: AtomicU8,
}

#[allow(
    unsafe_code,
    reason = "test callback reconstructs the exact owner installed in ExternalRxBuffer"
)]
unsafe fn observe_release_state(owner: NonNull<()>, _owner_index: usize) {
    // SAFETY: the test installs a stable pointer to `ReleaseProbe` as the
    // callback owner and keeps both the probe and pool alive throughout
    // the complete affine lease transition.
    let probe = unsafe { owner.cast::<ReleaseProbe>().as_ref() };
    // SAFETY: the same lifetime proof keeps the referenced pool slot live.
    let state = unsafe { (*probe.slot).state.load(Ordering::Acquire) };
    assert_eq!(
        state, SLOT_RELEASING,
        "handoff credit became free before its DMA buffer was released"
    );
    probe.calls.fetch_add(1, Ordering::Relaxed);
}

#[test]
fn dma_buffer_releases_before_handoff_credit_becomes_free() {
    let pool = ExternalRxHandoffPool::<64, 1>::new();
    let probe = ReleaseProbe {
        slot: core::ptr::addr_of!(pool.slots[0]),
        calls: AtomicU8::new(0),
    };
    let mut bytes = [0_u8; 64];
    let pointer = NonNull::new(bytes.as_mut_ptr()).unwrap();
    let owner = NonNull::from(&probe).cast::<()>();
    #[allow(
        unsafe_code,
        reason = "test owns stable bytes and the exact callback context"
    )]
    // SAFETY: the stack allocation remains stable and exclusively owned
    // until the network lease invokes the bound callback exactly once.
    let buffer = unsafe {
        ExternalRxBuffer::new(
            pointer,
            bytes.len(),
            bytes.len(),
            owner,
            0,
            observe_release_state,
        )
    };

    let radio = match pool.try_claim_radio(buffer, 0) {
        Ok(radio) => radio,
        Err(_) => panic!("fresh handoff slot must accept one buffer"),
    };
    let index = radio.republish(0, bytes.len());
    let network = pool.claim_network(index);
    assert_eq!(network.release(), 0);

    assert_eq!(probe.calls.load(Ordering::Relaxed), 1);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn an_adopted_network_slot_exposes_its_view_and_returns_on_release() {
    let pool = ExternalRxHandoffPool::<64, 1>::new();
    let probe = ReleaseProbe {
        slot: core::ptr::addr_of!(pool.slots[0]),
        calls: AtomicU8::new(0),
    };
    let mut bytes = [0_u8; 64];
    let pointer = NonNull::new(bytes.as_mut_ptr()).unwrap();
    let owner = NonNull::from(&probe).cast::<()>();
    #[allow(
        unsafe_code,
        reason = "test owns stable bytes and the exact callback context"
    )]
    // SAFETY: the stack allocation remains stable and exclusively owned
    // until the adopted slot is released exactly once.
    let buffer =
        unsafe { ExternalRxBuffer::new(pointer, 48, bytes.len(), owner, 0, observe_release_state) };
    let Ok(radio) = pool.try_claim_radio(buffer, 0) else {
        panic!("fresh handoff slot must accept one buffer");
    };
    let index = radio.republish(10, 30);
    let adoption = pool.claim_network(index).into_adoption();
    #[allow(unsafe_code, reason = "test compares the adopted address")]
    // SAFETY: offset 10 lies inside the 64-byte test allocation.
    let expected = unsafe { pointer.add(10) };
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
    assert_eq!(probe.calls.load(Ordering::Relaxed), 0);
    #[allow(unsafe_code, reason = "test returns its one adoption")]
    // SAFETY: the adoption above is released exactly once and not accessed after.
    unsafe {
        pool.release_adopted(adoption.index)
    };
    assert_eq!(probe.calls.load(Ordering::Relaxed), 1);
    assert_eq!(pool.claimed_slots(), 0);
}

#[allow(unsafe_code, reason = "test callback reconstructs its counter")]
unsafe fn count_release(owner: NonNull<()>, _owner_index: usize) {
    // SAFETY: the test installs a pointer to a live `AtomicU8` as the owner.
    unsafe { owner.cast::<AtomicU8>().as_ref() }.fetch_add(1, Ordering::Relaxed);
}

fn counted_buffer(bytes: &mut [u8; 64], released: &AtomicU8) -> ExternalRxBuffer {
    let pointer = NonNull::new(bytes.as_mut_ptr()).unwrap();
    #[allow(unsafe_code, reason = "test owns stable bytes and the counter")]
    // SAFETY: the bytes and the counter outlive the buffer in every test.
    unsafe {
        ExternalRxBuffer::new(
            pointer,
            bytes.len(),
            bytes.len(),
            NonNull::from(released).cast(),
            0,
            count_release,
        )
    }
}

#[test]
fn a_zeroed_pool_is_free_and_binds_without_releasing_a_previous_buffer() {
    let pool = crate::zeroed::zeroed::<ExternalRxHandoffPool<64, 2>>();
    assert_eq!(pool.claimed_slots(), 0);
    let released = AtomicU8::new(0);
    let mut bytes = [0_u8; 64];
    let radio = match pool.try_claim_radio(counted_buffer(&mut bytes, &released), 0) {
        Ok(radio) => radio,
        Err(_) => panic!("a zeroed slot must accept one buffer"),
    };
    // A free slot holds no buffer, so binding one releases nothing.
    assert_eq!(released.load(Ordering::Relaxed), 0);
    let index = radio.republish(0, 64);
    assert_eq!(pool.claim_network(index).release(), 0);
    assert_eq!(released.load(Ordering::Relaxed), 1);
}

#[test]
fn dropping_the_pool_releases_a_published_buffer_once() {
    let released = AtomicU8::new(0);
    let mut bytes = [0_u8; 64];
    {
        let pool = ExternalRxHandoffPool::<64, 1>::new();
        let radio = match pool.try_claim_radio(counted_buffer(&mut bytes, &released), 0) {
            Ok(radio) => radio,
            Err(_) => panic!("a fresh slot must accept one buffer"),
        };
        let _ready = radio.republish(0, 64);
        assert_eq!(released.load(Ordering::Relaxed), 0);
    }
    assert_eq!(released.load(Ordering::Relaxed), 1);
}
