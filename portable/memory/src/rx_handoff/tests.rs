use super::RxHandoffPool;

#[test]
fn one_address_crosses_radio_and_network_ownership() {
    let pool = RxHandoffPool::<32, 1>::new();
    let radio = pool.claim_radio(0).unwrap();
    let (index, radio_address) = radio.publish(crate::FrameLen::constant::<4>(), |frame| {
        frame.copy_from_slice(&[1, 2, 3, 4]);
        frame.as_ptr() as usize
    });

    let mut network = pool.claim_network(index).unwrap();
    let network_address = network.with_frame(|frame| {
        assert_eq!(frame, &[1, 2, 3, 4]);
        frame.as_ptr() as usize
    });
    assert_eq!(radio_address, network_address);
    assert_eq!(network.release(), 0);
}

#[test]
fn dropped_leases_restore_their_slot_state() {
    let pool = RxHandoffPool::<16, 1>::new();
    drop(pool.claim_radio(0).unwrap());

    let radio = pool.claim_radio(0).unwrap();
    let (index, ()) = radio.publish(crate::FrameLen::constant::<1>(), |frame| frame[0] = 7);
    drop(pool.claim_network(index).unwrap());

    let radio = pool.claim_radio(0).unwrap();
    let (index, ()) = radio.publish(crate::FrameLen::constant::<1>(), |frame| frame[0] = 9);
    assert_eq!(pool.claim_network(index).unwrap().release(), 0);
}

#[test]
fn failed_any_slot_claim_never_releases_the_current_owner() {
    let pool = RxHandoffPool::<16, 1>::new();
    let owner = pool.try_claim_radio().unwrap();
    assert!(pool.try_claim_radio().is_none());
    assert_eq!(pool.claimed_slots(), 1);
    drop(owner);
    assert_eq!(pool.claimed_slots(), 0);
    assert!(pool.try_claim_radio().is_some());
}

#[test]
fn hinted_claim_wraps_and_keeps_the_atomic_slot_proof() {
    let pool = RxHandoffPool::<16, 4>::new();
    let third = pool.try_claim_radio_from(2).unwrap();
    assert_eq!(third.index(), 2);
    let fourth = pool.try_claim_radio_from(2).unwrap();
    assert_eq!(fourth.index(), 3);
    let first = pool.try_claim_radio_from(2).unwrap();
    assert_eq!(first.index(), 0);
    drop(third);
    drop(fourth);
    drop(first);
}

#[test]
fn protocol_owner_can_republish_an_initialized_subrange() {
    let pool = RxHandoffPool::<32, 1>::new();
    let (index, ()) =
        pool.claim_radio(0)
            .unwrap()
            .publish(crate::FrameLen::constant::<20>(), |frame| {
                for (index, byte) in frame.iter_mut().enumerate() {
                    *byte = index as u8;
                }
            });
    let protocol = pool.claim_network(index).unwrap();
    let index = protocol
        .republish(6, crate::FrameLen::constant::<8>())
        .ok()
        .expect("valid test range");

    let network = pool.claim_network(index).unwrap();
    assert_eq!(network.frame(), &[6, 7, 8, 9, 10, 11, 12, 13]);
    drop(network);
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn invalid_claims_and_ranges_preserve_the_original_owner() {
    let pool = RxHandoffPool::<16, 1>::new();
    assert!(matches!(
        pool.claim_network(0),
        Err(crate::PoolError::State {
            observed: super::SLOT_FREE,
            ..
        })
    ));
    assert!(matches!(
        pool.claim_radio(1),
        Err(crate::PoolError::Index { .. })
    ));
    let radio = pool.claim_radio(0).unwrap();
    assert!(pool.claim_radio(0).is_err());
    let (index, ()) = radio.publish(crate::FrameLen::constant::<8>(), |bytes| bytes.fill(7));
    let network = pool.claim_network(index).unwrap();
    let (network, error) = network
        .republish(usize::MAX, crate::FrameLen::constant::<1>())
        .err()
        .expect("range rejected");
    assert_eq!(error.offset, usize::MAX);
    assert_eq!(network.frame(), &[7; 8]);
    assert_eq!(pool.claimed_slots(), 1);
    let index = network
        .republish(4, crate::FrameLen::constant::<4>())
        .ok()
        .unwrap();
    drop(pool.claim_network(index).unwrap());
    assert_eq!(pool.claimed_slots(), 0);
}

#[test]
fn an_unbounded_round_robin_hint_does_not_overflow() {
    let pool = RxHandoffPool::<16, 4>::new();
    assert_eq!(pool.try_claim_radio_from(usize::MAX).unwrap().index(), 3);
}

#[test]
fn direct_network_handoff_never_lends_a_ready_index_to_another_consumer() {
    extern crate std;
    use core::sync::atomic::{AtomicBool, Ordering};
    use std::{sync::Barrier, thread};
    let pool = RxHandoffPool::<16, 1>::new();
    let running = AtomicBool::new(true);
    let stolen = AtomicBool::new(false);
    let start = Barrier::new(2);
    let mut refused_owner = false;
    thread::scope(|scope| {
        let contender = scope.spawn(|| {
            start.wait();
            while running.load(Ordering::Acquire) {
                if let Ok(lease) = pool.claim_network(0) {
                    stolen.store(true, Ordering::Release);
                    core::mem::forget(lease);
                }
            }
        });
        start.wait();
        for _ in 0..20_000 {
            let radio = match pool.claim_radio(0) {
                Ok(radio) => radio,
                Err(_) => {
                    refused_owner = true;
                    break;
                }
            };
            let network = radio.into_network(crate::FrameLen::constant::<0>());
            drop(network);
        }
        running.store(false, Ordering::Release);
        contender.join().unwrap();
    });
    assert!(!refused_owner);
    assert!(!stolen.load(Ordering::Acquire));
    assert_eq!(pool.claimed_slots(), 0);
}
