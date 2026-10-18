use super::*;

#[test]
fn evidence_keeps_hardware_protocol_and_unavailable_provenance_distinct() {
    let smpdu = RxSmpduCounters::new();
    smpdu.observe_hardware(true);
    smpdu.observe_unavailable();
    assert_eq!(smpdu.snapshot().s_mpdu_frames, 1);
    assert_eq!(smpdu.snapshot().unavailable_frames, 1);

    let ampdu = RxAmpduCounters::new();
    ampdu.observe_hardware(true);
    ampdu.observe_protocol(true);
    ampdu.observe_unavailable();
    let snapshot = ampdu.snapshot();
    assert_eq!(snapshot.ampdu_frames, 2);
    assert_eq!(snapshot.hardware_ampdu_frames, 1);
    assert_eq!(snapshot.protocol_ampdu_frames, 1);
    assert_eq!(snapshot.unavailable_frames, 1);
}
