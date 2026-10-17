use super::RxHandoffPool;

#[test]
fn one_address_crosses_radio_and_network_ownership() {
    let pool = RxHandoffPool::<32, 1>::new();
    let radio = pool.claim_radio(0);
    let (index, radio_address) = radio.publish(4, |frame| {
        frame.copy_from_slice(&[1, 2, 3, 4]);
        frame.as_ptr() as usize
    });

    let mut network = pool.claim_network(index);
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
    drop(pool.claim_radio(0));

    let radio = pool.claim_radio(0);
    let (index, ()) = radio.publish(1, |frame| frame[0] = 7);
    drop(pool.claim_network(index));

    let radio = pool.claim_radio(0);
    let (index, ()) = radio.publish(1, |frame| frame[0] = 9);
    assert_eq!(pool.claim_network(index).release(), 0);
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
    let (index, ()) = pool.claim_radio(0).publish(20, |frame| {
        for (index, byte) in frame.iter_mut().enumerate() {
            *byte = index as u8;
        }
    });
    let protocol = pool.claim_network(index);
    let index = protocol.republish(6, 8);

    let network = pool.claim_network(index);
    assert_eq!(network.frame(), &[6, 7, 8, 9, 10, 11, 12, 13]);
    drop(network);
    assert_eq!(pool.claimed_slots(), 0);
}
