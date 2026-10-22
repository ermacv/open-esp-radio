use super::*;

#[test]
fn a_foreign_pool_of_the_same_kind_returns_the_unchanged_lease() {
    let mut owner = pool();
    let mut foreign = pool();
    let lease = owner.acquire().expect("owner has capacity");
    assert_eq!(
        foreign.submit(&lease, 0),
        Err(SchedulerPoolError::ForeignPool)
    );
    let rejected = foreign
        .release(lease)
        .expect_err("same kind does not prove identity");
    assert_eq!(rejected.error, SchedulerPoolError::ForeignPool);
    owner
        .release(rejected.instance)
        .expect("rejection preserves the rightful lease");
    assert!(foreign.acquire().is_some());
}

#[test]
fn a_reacquired_slot_does_not_accept_the_previous_item_identity() {
    let mut owner = pool();
    let old = owner.acquire().expect("slot");
    prepare(&mut owner, &old);
    let old_id = owner.submit(&old, 0).expect("prepared");
    owner.retire(old_id).expect("model completion");
    owner.release(old).expect("returned");
    let new = owner.acquire().expect("same slot");
    prepare(&mut owner, &new);
    let new_id = owner.submit(&new, 0).expect("prepared");
    assert_ne!(old_id, new_id);
    assert_eq!(owner.retire(old_id), Err(SchedulerPoolError::StaleInstance));
    assert!(owner.is_listed(new_id));
    owner.retire(new_id).expect("new identity remains listed");
    owner.release(new).expect("new lease remains usable");
}
