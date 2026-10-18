use super::{charge_pair_tx_frames, select_pair_tx_slot};

#[test]
fn unequal_aggregate_sizes_receive_equal_frame_service() {
    let mut served = [0_u64; 2];
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
    charge_pair_tx_frames(&mut served, 0, 32, true);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
    charge_pair_tx_frames(&mut served, 1, 16, true);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
    charge_pair_tx_frames(&mut served, 1, 16, true);
    assert_eq!(served, [0, 0]);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
}

#[test]
fn frames_served_while_the_peer_is_idle_build_no_lead() {
    let mut served = [0_u64; 2];
    // One VIF saturates while the other has nothing queued.
    for _ in 0..1_000 {
        charge_pair_tx_frames(&mut served, 0, 32, false);
    }
    assert_eq!(served, [0, 0]);
    // The returning peer gets one turn, then the saturating VIF again.
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
    charge_pair_tx_frames(&mut served, 0, 32, true);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(1));
    charge_pair_tx_frames(&mut served, 1, 32, true);
    assert_eq!(select_pair_tx_slot([true; 2], served), Some(0));
}
