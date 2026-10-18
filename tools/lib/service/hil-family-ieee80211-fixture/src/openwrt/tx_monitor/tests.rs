use super::*;

#[test]
fn extracts_sequence_from_truncated_plaintext_ap_tx() {
    let mut packet = vec![0, 0, 0x08, 0x00];
    packet.extend_from_slice(&[
        0x45, 0, 0x05, 0xdc, 0, 0, 0x40, 0, 64, 17, 0, 0, 192, 168, 1, 129, 192, 168, 1, 182, 0x83,
        0xcb, 0x10, 0xe3, 0x05, 0xc8, 0, 0, 0, 0, 0, 42,
    ]);
    assert_eq!(
        udp_sequence(&packet, Ipv4Addr::new(192, 168, 1, 182), 4323),
        Some(42)
    );
}

#[test]
fn sequence_tracker_separates_late_recovery_from_final_loss() {
    let mut tracker = SequenceTracker::default();
    for sequence in [0, 2, 1, 4, -1] {
        tracker.observe(sequence, None, 0, 5);
    }
    let evidence = tracker.finish(5);
    assert_eq!(evidence.forward_missing, 2);
    assert_eq!(evidence.late_recovered, 1);
    assert_eq!(evidence.unrecovered, 1);
    assert_eq!(evidence.terminal_markers, 1);
}
