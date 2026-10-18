use super::*;

#[test]
fn aggregate_role_handle_remains_a_small_borrowed_owner() {
    // Each arena's retry state carries its aggregate's MSDU lifetime deadline
    // and the Block Ack agreement the aggregate was built under; a published
    // aggregate and one waiting for its BlockAckReq's answer both keep it.
    // The portable AmpduRetryState tracks a 64-entry Block Ack window, so its
    // two original-subframe masks are u64.
    assert!(core::mem::size_of::<AccessPointAmpdu<'static, (), 32, 0>>() <= 328);
}
