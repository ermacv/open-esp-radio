use oer_ieee80211_mac::sequence::SequenceNumber;
use std::vec::Vec;

use oer_esp32s31_ieee80211_dma::descriptor::{BIT_30, BIT_31, LENGTH_SHIFT};

use super::*;

const STATION: [u8; 6] = [0x10, 0x11, 0x12, 0x13, 0x14, 0x15];
const BSSID: [u8; 6] = [0x20, 0x21, 0x22, 0x23, 0x24, 0x25];
const SOURCE: [u8; 6] = [0x30, 0x31, 0x32, 0x33, 0x34, 0x35];
const TAIL_OFFSET: usize = 0x38;
const FRAME_OFFSET: usize = 0x40;

/// Receive times of a frame handed off at `handoff_micros`, without a
/// receive timestamp.
fn times(handoff_micros: u64) -> Option<oer_esp32s31_ieee80211_mac::rx::pool::RxTimes> {
    Some(oer_esp32s31_ieee80211_mac::rx::pool::RxTimes {
        handoff: oer_time::Instant::from_micros(handoff_micros),
        stamp: None,
    })
}

fn replay_resource() -> (StaCcmpRxReplayRxEndpoint, StaCcmpRxReplayControlEndpoint) {
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap()
}

fn ccmp_header(packet_number: u64, key_id: u8) -> CcmpHeader {
    CcmpHeader::new(
        oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(packet_number).unwrap(),
        CcmpKeyId::new(key_id).unwrap(),
    )
}

#[test]
fn shared_group_rotation_changes_key_id_and_rsc_without_resetting_pairwise() {
    let (mut rx, mut control) = replay_resource();
    drop(
        rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(9, 0))
            .unwrap(),
    );
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(7, 1))
            .unwrap(),
    );

    let prepared = control
        .prepare_group_rotation(2, [20, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(prepared).unwrap();
    assert_eq!(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(21, 2),)
            .err(),
        Some(StaCcmpRxReplayError::GroupRotationInProgress)
    );
    // The group gate never resets or suspends PTK replay ownership.
    drop(
        rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(10, 0))
            .unwrap(),
    );
    control.commit_group_rotation(installing).unwrap();

    assert!(matches!(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(22, 1),),
        Err(StaCcmpRxReplayError::UnexpectedKeyId { .. })
    ));
    assert!(matches!(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(20, 2),),
        Err(StaCcmpRxReplayError::Replay(
            CcmpReplayError::Replayed { .. }
        ))
    ));
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(3), ccmp_header(21, 2))
            .unwrap(),
    );
    assert!(matches!(
        rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(10, 0),),
        Err(StaCcmpRxReplayError::Replay(
            CcmpReplayError::Replayed { .. }
        ))
    ));
    drop(
        rx.prepare_publication(ConnectedRxProtection::Pairwise, Some(3), ccmp_header(11, 0))
            .unwrap(),
    );
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn shared_group_rotation_applies_same_key_id_rsc_and_rejects_stale_candidate() {
    let (mut rx, mut control) = replay_resource();
    let stale = control
        .prepare_group_rotation(1, [4, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let current = control
        .prepare_group_rotation(1, [8, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(current).unwrap();
    control.commit_group_rotation(installing).unwrap();
    assert_eq!(
        control.begin_group_rotation(stale).err(),
        Some(StaCcmpRxReplayError::StaleGroupRotation)
    );
    assert!(matches!(
        rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(8, 1),),
        Err(StaCcmpRxReplayError::Replay(
            CcmpReplayError::Replayed { .. }
        ))
    ));
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(9, 1))
            .unwrap(),
    );
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn same_key_id_rotation_merges_each_lane_monotonically_but_new_key_id_resets() {
    let (mut rx, mut control) = replay_resource();
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(0), ccmp_header(10, 1))
            .unwrap(),
    );
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(7), ccmp_header(20, 1))
            .unwrap(),
    );
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(5, 1))
            .unwrap(),
    );

    let lower = control
        .prepare_group_rotation(1, [8, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(lower).unwrap();
    control.commit_group_rotation(installing).unwrap();
    for (tid, highest) in [(Some(0), 10), (Some(7), 20), (None, 8)] {
        assert!(matches!(
            rx.prepare_publication(ConnectedRxProtection::Group, tid, ccmp_header(highest, 1),),
            Err(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed { .. }
            ))
        ));
    }

    let higher = control
        .prepare_group_rotation(1, [25, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(higher).unwrap();
    control.commit_group_rotation(installing).unwrap();
    let equal = control
        .prepare_group_rotation(1, [25, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(equal).unwrap();
    control.commit_group_rotation(installing).unwrap();
    for tid in [None, Some(0), Some(7), Some(15)] {
        assert!(matches!(
            rx.prepare_publication(ConnectedRxProtection::Group, tid, ccmp_header(25, 1),),
            Err(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed { .. }
            ))
        ));
    }

    let new_key = control
        .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(new_key).unwrap();
    control.commit_group_rotation(installing).unwrap();
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, Some(7), ccmp_header(4, 2))
            .unwrap(),
    );
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn same_key_id_begin_refreshes_all_lanes_advanced_after_prepare() {
    let (mut rx, mut control) = replay_resource();
    let prepared = control
        .prepare_group_rotation(1, [7, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();

    for tid in core::iter::once(None).chain((0_u8..16).map(Some)) {
        let packet_number = 40 + u64::from(tid.unwrap_or(16));
        drop(
            rx.prepare_publication(
                ConnectedRxProtection::Group,
                tid,
                ccmp_header(packet_number, 1),
            )
            .unwrap(),
        );
    }

    let installing = control.begin_group_rotation(prepared).unwrap();
    control.commit_group_rotation(installing).unwrap();
    for tid in core::iter::once(None).chain((0_u8..16).map(Some)) {
        let packet_number = 40 + u64::from(tid.unwrap_or(16));
        assert!(matches!(
            rx.prepare_publication(
                ConnectedRxProtection::Group,
                tid,
                ccmp_header(packet_number, 1),
            ),
            Err(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed { .. }
            ))
        ));
    }
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn group_publication_and_stop_races_fail_without_mixing_epochs() {
    let (mut rx, mut control) = replay_resource();
    let permit = rx
        .prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 1))
        .unwrap();
    let prepared = control
        .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    assert_eq!(
        control.begin_group_rotation(prepared).err(),
        Some(StaCcmpRxReplayError::PublicationInFlight)
    );
    drop(permit);

    let prepared = control
        .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(prepared).unwrap();
    control.abort_group_rotation(installing).unwrap();
    rx.stop().unwrap();
    control.stop().unwrap();

    let (mut rx, mut control) = replay_resource();
    let prepared = control
        .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = control.begin_group_rotation(prepared).unwrap();
    rx.stop().unwrap();
    assert_eq!(
        control.commit_group_rotation(installing),
        Err(StaCcmpRxReplayError::RxStopped)
    );
    control.stop().unwrap();
}

#[test]
fn endpoint_drop_defers_epoch_release_until_group_publication_returns() {
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (mut rx, control) = resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap();
    let permit = rx
        .prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 1))
        .unwrap();
    drop(rx);
    drop(control);
    let busy = match resource.start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap()) {
        Ok(_) => panic!("a live publication must retain the old replay epoch"),
        Err(failure) => failure,
    };
    let (error, recovered) = busy.into_parts();
    assert_eq!(error, StaCcmpRxReplayStartError::Busy);

    drop(permit);
    let (mut rx, mut control) = resource.start(recovered).unwrap();
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn stale_generation_commit_cannot_quarantine_new_epoch() {
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (mut old_rx, mut old_control) = resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap();
    let prepared = old_control
        .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = old_control.begin_group_rotation(prepared).unwrap();
    old_rx.stop().unwrap();
    old_control.stop().unwrap();

    let (mut rx, mut control) = resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 2, [0; 8]).unwrap())
        .unwrap();
    assert_eq!(
        old_control.commit_group_rotation(installing),
        Err(StaCcmpRxReplayError::StaleGroupRotation)
    );
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 2))
            .unwrap(),
    );
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn stale_generation_abort_cannot_quarantine_new_epoch() {
    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (mut old_rx, mut old_control) = resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap();
    let prepared = old_control
        .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let installing = old_control.begin_group_rotation(prepared).unwrap();
    old_rx.stop().unwrap();
    old_control.stop().unwrap();

    let (mut rx, mut control) = resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 2, [0; 8]).unwrap())
        .unwrap();
    assert_eq!(
        old_control.abort_group_rotation(installing),
        Err(StaCcmpRxReplayError::StaleGroupRotation)
    );
    drop(
        rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 2))
            .unwrap(),
    );
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[test]
fn generation_and_rotation_ticket_exhaustion_fail_closed_without_wrap() {
    let exhausted_generation =
        std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    critical_section::with(|cs| {
        exhausted_generation
            .state
            .borrow(cs)
            .borrow_mut()
            .generation = u32::MAX;
    });
    let exhausted =
        match exhausted_generation.start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap()) {
            Ok(_) => panic!("an exhausted generation must not wrap"),
            Err(failure) => failure,
        };
    let (error, recovered) = exhausted.into_parts();
    assert_eq!(error, StaCcmpRxReplayStartError::GenerationExhausted);
    let recovery_resource =
        std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (mut recovered_rx, mut recovered_control) = recovery_resource.start(recovered).unwrap();
    recovered_rx.stop().unwrap();
    recovered_control.stop().unwrap();

    let resource = std::boxed::Box::leak(std::boxed::Box::new(StaCcmpRxReplayResource::new()));
    let (mut rx, mut control) = resource
        .start(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap())
        .unwrap();
    critical_section::with(|cs| {
        resource.state.borrow(cs).borrow_mut().next_rotation_ticket = u32::MAX;
    });
    assert_eq!(
        control
            .prepare_group_rotation(2, [3, 0, 0, 0, 0, 0, 0, 0])
            .err(),
        Some(StaCcmpRxReplayError::GroupRotationTicketExhausted)
    );
    assert_eq!(
        rx.prepare_publication(ConnectedRxProtection::Group, None, ccmp_header(1, 1))
            .err(),
        Some(StaCcmpRxReplayError::GroupRotationInProgress)
    );
    drop(
        rx.prepare_publication(ConnectedRxProtection::Pairwise, None, ccmp_header(1, 0))
            .unwrap(),
    );
    rx.stop().unwrap();
    control.stop().unwrap();
}

#[derive(Default)]
struct RecordingSink {
    beacons: Vec<StaBeaconObservation>,
    beacon_metadata: Vec<MacRxMetadata<RxPhyInfo>>,
    probe_responses: u32,
    ethernet: Vec<Vec<u8>>,
    ethernet_metadata: Vec<MacRxMetadata<RxPhyInfo>>,
    block_ack: Vec<BlockAckAction>,
    peer_disconnects: Vec<StaDisconnect>,
    unprotected_disconnects: Vec<StaDisconnect>,
    protected_management: Vec<(Vec<u8>, bool)>,
    power_save_deliveries: Vec<PowerSaveData>,
    unprotected_eapol: Vec<Vec<u8>>,
}

impl ConnectedRxSink for RecordingSink {
    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        match event {
            ConnectedRxEvent::Beacon {
                observation,
                metadata,
                ..
            } => {
                self.beacons.push(observation);
                self.beacon_metadata.push(metadata);
            }
            ConnectedRxEvent::ProbeResponse => {
                self.probe_responses = self.probe_responses.saturating_add(1);
            }
            ConnectedRxEvent::Ethernet {
                frame, metadata, ..
            } => {
                let mut bytes = std::vec![0; frame.length()];
                frame.copy_to(&mut bytes).unwrap();
                self.ethernet.push(bytes);
                self.ethernet_metadata.push(metadata);
            }
            ConnectedRxEvent::BlockAck { action, .. } => self.block_ack.push(action),
            ConnectedRxEvent::PeerDisconnect(disconnect) => {
                self.peer_disconnects.push(disconnect);
            }
            ConnectedRxEvent::UnprotectedDisconnect(disconnect) => {
                self.unprotected_disconnects.push(disconnect);
            }
            ConnectedRxEvent::ProtectedManagement { frame, group } => {
                self.protected_management.push((frame.to_vec(), group));
            }
            ConnectedRxEvent::PowerSaveData(delivery) => {
                self.power_save_deliveries.push(delivery);
            }
            ConnectedRxEvent::UnprotectedEapol { payload, .. } => {
                self.unprotected_eapol.push(payload.to_vec());
            }
            ConnectedRxEvent::Trigger { .. }
            | ConnectedRxEvent::Ndpa { .. }
            | ConnectedRxEvent::IndividualTwt { .. }
            | ConnectedRxEvent::EspNow { .. } => {}
        }
    }
}

#[test]
fn routes_associated_beacon_and_local_tim_as_owned_control_state() {
    const FIXED: usize = 36;
    const TIM: [u8; 6] = [5, 4, 0, 3, 1, 0x80];
    const MPDU: usize = FIXED + TIM.len();
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, SIGNAL);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
    frame[..2].copy_from_slice(&BEACON_FRAME_CONTROL.to_le_bytes());
    frame[4..10].fill(0xff);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..32].copy_from_slice(&123_u64.to_le_bytes());
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());
    frame[FIXED..].copy_from_slice(&TIM);

    let mut dispatcher = ConnectedRxDispatcher::new(config());
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    assert!(!dispatcher.may_publish_amsdu(segment(&storage, SIGNAL)));
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Beacon
    );
    assert_eq!(sink.beacons.len(), 1);
    assert_eq!(sink.beacons[0].timestamp_tsf.as_micros(), 123);
    assert_eq!(sink.beacons[0].interval_tu, 100);
    assert!(sink.beacons[0].tim.unwrap().unicast_buffered);
    assert!(sink.beacons[0].tim.unwrap().group_buffered);
    assert_eq!(
        sink.beacon_metadata[0].s_mpdu,
        MacRxEvidence::HardwareObserved(true)
    );
    assert_eq!(
        sink.beacon_metadata[0].ampdu,
        MacRxEvidence::ProtocolValidated(false)
    );
}

#[test]
fn drops_whole_a_beacon_whose_tsf_a_32_bit_elapsed_time_carries_past_its_range() {
    const MPDU: usize = 36;
    const SIGNAL: usize = MPDU + 4;
    let last_followable = u64::MAX - u64::from(u32::MAX);
    for (timestamp, published) in [(last_followable, true), (last_followable + 1, false)] {
        let mut storage = [0_u8; 192];
        set_tail(&mut storage, SIGNAL);
        let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
        frame[..2].copy_from_slice(&BEACON_FRAME_CONTROL.to_le_bytes());
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&BSSID);
        frame[16..22].copy_from_slice(&BSSID);
        frame[24..32].copy_from_slice(&timestamp.to_le_bytes());
        frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
        frame[34..36].copy_from_slice(&0x0431_u16.to_le_bytes());

        let mut dispatcher = ConnectedRxDispatcher::new(config());
        let mut sink = RecordingSink::default();
        let mut mpdu = [0_u8; 128];
        let mut ethernet = [0_u8; 128];
        let dispatch = dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        );
        if published {
            assert_eq!(dispatch, ConnectedRxDispatch::Beacon);
            assert_eq!(sink.beacons.len(), 1);
        } else {
            assert!(matches!(
                dispatch,
                ConnectedRxDispatch::Rejected {
                    error: ConnectedRxError::UnfollowableBeaconTsf,
                    ..
                }
            ));
            assert!(sink.beacons.is_empty());
        }
    }
}

#[test]
fn routes_only_probe_responses_from_the_associated_bssid() {
    const MPDU: usize = 36;
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, SIGNAL);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
    frame[..2].copy_from_slice(&PROBE_RESPONSE_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);

    let mut dispatcher = ConnectedRxDispatcher::new(config());
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::ProbeResponse
    );
    assert_eq!(sink.probe_responses, 1);

    storage[FRAME_OFFSET + 10..FRAME_OFFSET + 16].copy_from_slice(&SOURCE);
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Ignored
    );
    assert_eq!(sink.probe_responses, 1);
}

fn config() -> ConnectedRxConfig {
    ConnectedRxConfig {
        station_address: STATION,
        bssid: BSSID,
        association_id: 7,
        ingress: RxIngressConfig {
            ring_entry_limit: 1,
            csi_config: 0,
            flags: 0,
        },
        security: LinkProtection::Ccmp,
        peer_qos: true,
        management_protection: false,
    }
}

fn dispatcher() -> ConnectedRxDispatcher {
    let mut dispatcher = ConnectedRxDispatcher::new(config());
    dispatcher.install_ccmp_rx_replay(StaCcmpRxReplayEpoch::new([0; 8], 1, [0; 8]).unwrap());
    dispatcher
}

fn segment(storage: &[u8; 192], signal_length: usize) -> RxSegment<'_> {
    let received = FRAME_OFFSET + signal_length;
    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 192 | ((received as u32) << LENGTH_SHIFT) | BIT_30 | BIT_31,
        buffer: storage,
        next_descriptor_address: 0,
    }
}

fn large_segment(storage: &[u8; 256], signal_length: usize) -> RxSegment<'_> {
    RxSegment {
        descriptor_address: 0x2f00_4000,
        descriptor_word0: 256
            | (((FRAME_OFFSET + signal_length) as u32) << LENGTH_SHIFT)
            | BIT_30
            | BIT_31,
        buffer: storage,
        next_descriptor_address: 0,
    }
}

fn set_tail(storage: &mut [u8; 192], signal_length: usize) {
    // Synthetic connected frames are standalone MPDUs unless a test
    // explicitly overrides the hardware `cur_single_mpdu` bit.
    storage[0x1f] = 1;
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
    );
}

fn open_fragment(
    storage: &mut [u8; 192],
    sequence: u16,
    fragment: u8,
    more_fragments: bool,
    retry: bool,
    source: [u8; 6],
    payload: &[u8],
) -> usize {
    let mpdu_length = 24 + payload.len();
    let signal_length = mpdu_length + 4;
    storage.fill(0);
    set_tail(storage, signal_length);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu_length];
    let mut frame_control = 0x0208_u16;
    if more_fragments {
        frame_control |= MORE_FRAGMENTS;
    }
    if retry {
        frame_control |= 0x0800;
    }
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
    frame[24..].copy_from_slice(payload);
    signal_length
}

#[expect(
    clippy::too_many_arguments,
    reason = "the test vector keeps every independently mutated 802.11/CCMP field explicit"
)]
fn protected_fragment(
    storage: &mut [u8; 192],
    sequence: u16,
    fragment: u8,
    more_fragments: bool,
    retry: bool,
    packet_number: u64,
    source: [u8; 6],
    payload: &[u8],
) -> usize {
    let mpdu_length = 24 + 8 + payload.len() + 8;
    let signal_length = mpdu_length + 4;
    storage.fill(0);
    set_tail(storage, signal_length);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu_length];
    let mut frame_control = 0x4208_u16;
    if more_fragments {
        frame_control |= MORE_FRAGMENTS;
    }
    if retry {
        frame_control |= 0x0800;
    }
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&source);
    frame[22..24].copy_from_slice(&((sequence << 4) | u16::from(fragment)).to_le_bytes());
    frame[24..32].copy_from_slice(
        &CcmpHeader::new(
            oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(packet_number).unwrap(),
            CcmpKeyId::PAIRWISE,
        )
        .encode(),
    );
    frame[32..32 + payload.len()].copy_from_slice(payload);
    signal_length
}

#[test]
fn open_station_reassembles_only_the_exact_fragment_identity() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
    let final_payload = [3, 4, 5];
    let mut first_storage = [0_u8; 192];
    let first_signal = open_fragment(
        &mut first_storage,
        0x123,
        0,
        true,
        false,
        SOURCE,
        &first_payload,
    );
    let mut final_storage = [0_u8; 192];
    let final_signal = open_fragment(
        &mut final_storage,
        0x123,
        1,
        false,
        false,
        SOURCE,
        &final_payload,
    );
    let mut open = config();
    open.security = LinkProtection::Open;
    open.peer_qos = false;
    let mut dispatcher = ConnectedRxDispatcher::new(open);
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];

    assert!(!dispatcher.may_publish_ethernet(segment(&first_storage, first_signal)));
    assert!(!dispatcher.may_complete_open_fragment(segment(&first_storage, first_signal)));
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(10),
            &mut sink,
        ),
        ConnectedRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    assert!(sink.power_save_deliveries.is_empty());

    // A retried first fragment is a defragmenter duplicate and cannot
    // complete the one-shot PS-Poll delivery lane.
    first_storage[FRAME_OFFSET + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(11),
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );
    assert!(sink.power_save_deliveries.is_empty());

    // A retry cannot clear More Fragments and route its partial first
    // body through ordinary decapsulation while the exact sequence is
    // still retained.
    first_storage[FRAME_OFFSET + 1] &= !0x04;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(11),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::MoreFragmentsMismatch),
        }
    );
    first_storage[FRAME_OFFSET + 1] &= !0x08;
    first_storage[FRAME_OFFSET + 1] |= 0x04;

    final_storage[FRAME_OFFSET + 16] ^= 1;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&final_storage, final_signal),
            &mut mpdu,
            &mut ethernet,
            times(11),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::IdentityMismatch),
        }
    );
    assert!(sink.ethernet.is_empty());

    final_storage[FRAME_OFFSET + 16] ^= 1;
    assert!(!dispatcher.may_publish_ethernet(segment(&final_storage, final_signal)));
    assert!(dispatcher.may_complete_open_fragment(segment(&final_storage, final_signal)));
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&final_storage, final_signal),
            &mut mpdu,
            &mut ethernet,
            times(12),
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);
    assert_eq!(&sink.ethernet[0][..6], &STATION);
    assert_eq!(&sink.ethernet[0][6..12], &SOURCE);
    assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
    assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);
    assert_eq!(
        sink.power_save_deliveries,
        [PowerSaveData {
            group: false,
            more_data: false
        }]
    );

    let _ = dispatcher.dispatch_received(
        segment(&first_storage, first_signal),
        &mut mpdu,
        &mut ethernet,
        times(20),
        &mut sink,
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 1);
}

#[test]
fn station_ccmp_fragments_commit_each_pn_before_one_final_publication() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1, 2];
    let final_payload = [3, 4, 5];
    let mut first_storage = [0_u8; 192];
    let first_signal = protected_fragment(
        &mut first_storage,
        7,
        0,
        true,
        false,
        3,
        SOURCE,
        &first_payload,
    );
    let mut final_storage = [0_u8; 192];
    let final_signal = protected_fragment(
        &mut final_storage,
        7,
        1,
        false,
        false,
        4,
        SOURCE,
        &final_payload,
    );
    let mut dispatcher = dispatcher();
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    assert!(!dispatcher.may_publish_ethernet(segment(&first_storage, first_signal)));
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(1),
            &mut sink,
        ),
        ConnectedRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    assert!(sink.ethernet.is_empty());

    first_storage[FRAME_OFFSET + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(2),
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );

    first_storage[FRAME_OFFSET + 24..FRAME_OFFSET + 32].copy_from_slice(
        &CcmpHeader::new(
            oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(4).unwrap(),
            CcmpKeyId::PAIRWISE,
        )
        .encode(),
    );
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(3),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::RetryPacketNumberMismatch {
                fragment_number: 0,
                expected: oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(3).unwrap(),
                observed: oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(4).unwrap(),
            }),
        }
    );
    first_storage[FRAME_OFFSET + 24..FRAME_OFFSET + 32].copy_from_slice(
        &CcmpHeader::new(
            oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(3).unwrap(),
            CcmpKeyId::PAIRWISE,
        )
        .encode(),
    );
    first_storage[FRAME_OFFSET + 32] ^= 1;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&first_storage, first_signal),
            &mut mpdu,
            &mut ethernet,
            times(4),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::RetryPayloadMismatch {
                fragment_number: 0
            }),
        }
    );
    assert!(dispatcher.may_complete_fragment(segment(&final_storage, final_signal)));
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&final_storage, final_signal),
            &mut mpdu,
            &mut ethernet,
            times(5),
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);
    assert_eq!(&sink.ethernet[0][14..], &[1, 2, 3, 4, 5]);
    assert_eq!(
        sink.power_save_deliveries,
        [PowerSaveData {
            group: false,
            more_data: false
        }]
    );

    final_storage[FRAME_OFFSET + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&final_storage, final_signal),
            &mut mpdu,
            &mut ethernet,
            times(6),
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );
    assert_eq!(sink.ethernet.len(), 1);
    let replay = dispatcher
        .owned_ccmp_replay
        .as_ref()
        .expect("test dispatcher owns one replay epoch");
    assert_eq!(
        replay.pairwise.highest(CcmpReplayLane::NonQos),
        oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(4)
    );
}

#[test]
fn protected_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut ordinary_storage = [0_u8; 192];
    let ordinary_signal = protected_fragment(
        &mut ordinary_storage,
        7,
        0,
        false,
        false,
        3,
        SOURCE,
        &payload,
    );
    let mut retry_first_storage = [0_u8; 192];
    let retry_first_signal = protected_fragment(
        &mut retry_first_storage,
        7,
        0,
        true,
        true,
        4,
        SOURCE,
        &payload,
    );
    let mut colliding_final_storage = [0_u8; 192];
    let colliding_final_signal = protected_fragment(
        &mut colliding_final_storage,
        7,
        1,
        false,
        false,
        5,
        SOURCE,
        &[2],
    );
    let mut new_first_storage = [0_u8; 192];
    let new_first_signal = protected_fragment(
        &mut new_first_storage,
        8,
        0,
        true,
        true,
        4,
        SOURCE,
        &payload,
    );
    let mut new_final_storage = [0_u8; 192];
    let new_final_signal =
        protected_fragment(&mut new_final_storage, 8, 1, false, false, 5, SOURCE, &[2]);
    let mut dispatcher = dispatcher();
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];

    assert_eq!(
        dispatcher.dispatch_received(
            segment(&ordinary_storage, ordinary_signal),
            &mut mpdu,
            &mut ethernet,
            times(1),
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);

    assert_eq!(
        dispatcher.dispatch_received(
            segment(&retry_first_storage, retry_first_signal),
            &mut mpdu,
            &mut ethernet,
            times(2),
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );
    assert_eq!(dispatcher.fragments.active_contexts(), 0);
    let replay = dispatcher
        .owned_ccmp_replay
        .as_ref()
        .expect("test dispatcher owns one replay epoch");
    assert_eq!(
        replay.pairwise.highest(CcmpReplayLane::NonQos),
        oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(3),
        "duplicate admission must precede replay commit"
    );

    assert_eq!(
        dispatcher.dispatch_received(
            segment(&colliding_final_storage, colliding_final_signal),
            &mut mpdu,
            &mut ethernet,
            times(3),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::Orphan { fragment_number: 1 }),
        }
    );
    assert_eq!(dispatcher.fragments.active_contexts(), 0);
    assert_eq!(sink.ethernet.len(), 1);

    // Retry is not itself a rejection: a fragment-zero sequence absent
    // from ordinary history still starts and completes a normal train.
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&new_first_storage, new_first_signal),
            &mut mpdu,
            &mut ethernet,
            times(4),
            &mut sink,
        ),
        ConnectedRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&new_final_storage, new_final_signal),
            &mut mpdu,
            &mut ethernet,
            times(5),
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 2);
    let replay = dispatcher
        .owned_ccmp_replay
        .as_ref()
        .expect("test dispatcher owns one replay epoch");
    assert_eq!(
        replay.pairwise.highest(CcmpReplayLane::NonQos),
        oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(5)
    );
}

#[test]
fn replay_rejection_cannot_evict_two_durable_ccmp_fragment_trains() {
    let first_payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut first_a = [0_u8; 192];
    let signal_a = protected_fragment(&mut first_a, 10, 0, true, false, 3, SOURCE, &first_payload);
    let mut first_b = [0_u8; 192];
    let signal_b = protected_fragment(&mut first_b, 11, 0, true, false, 4, SOURCE, &first_payload);
    let mut replayed = [0_u8; 192];
    let replayed_signal =
        protected_fragment(&mut replayed, 12, 0, true, false, 4, SOURCE, &first_payload);
    let mut final_a = [0_u8; 192];
    let final_signal = protected_fragment(&mut final_a, 10, 1, false, false, 5, SOURCE, &[2]);
    let mut dispatcher = dispatcher();
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];

    for (storage, signal, now) in [(&first_a, signal_a, 1), (&first_b, signal_b, 2)] {
        assert!(matches!(
            dispatcher.dispatch_received(
                segment(storage, signal),
                &mut mpdu,
                &mut ethernet,
                times(now),
                &mut sink,
            ),
            ConnectedRxDispatch::FragmentBuffered { .. }
        ));
    }
    assert_eq!(dispatcher.fragments.active_contexts(), 2);
    let pn4 = oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(4).unwrap();
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&replayed, replayed_signal),
            &mut mpdu,
            &mut ethernet,
            times(3),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::CcmpReplay(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed {
                    packet_number: pn4,
                    highest: pn4,
                }
            )),
        }
    );
    assert_eq!(dispatcher.fragments.active_contexts(), 2);
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&final_a, final_signal),
            &mut mpdu,
            &mut ethernet,
            times(4),
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(dispatcher.fragments.active_contexts(), 1);
    assert_eq!(sink.ethernet.len(), 1);
}

#[test]
fn open_retry_cannot_turn_an_ordinary_mpdu_into_a_fragment_train() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut ordinary_storage = [0_u8; 192];
    let ordinary_signal =
        open_fragment(&mut ordinary_storage, 7, 0, false, false, SOURCE, &payload);
    let mut open = config();
    open.security = LinkProtection::Open;
    open.peer_qos = false;
    let mut dispatcher = ConnectedRxDispatcher::new(open);
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];

    assert_eq!(
        dispatcher.dispatch_received(
            segment(&ordinary_storage, ordinary_signal),
            &mut mpdu,
            &mut ethernet,
            times(1),
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );

    ordinary_storage[FRAME_OFFSET + 1] |= 0x04 | 0x08;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&ordinary_storage, ordinary_signal),
            &mut mpdu,
            &mut ethernet,
            times(2),
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 0);

    let mut final_storage = [0_u8; 192];
    let final_signal = open_fragment(&mut final_storage, 7, 1, false, false, SOURCE, &[2]);
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&final_storage, final_signal),
            &mut mpdu,
            &mut ethernet,
            times(3),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::Orphan { fragment_number: 1 }),
        }
    );

    let mut invalid_first_storage = [0_u8; 192];
    let invalid_first_signal = open_fragment(
        &mut invalid_first_storage,
        8,
        0,
        true,
        false,
        SOURCE,
        &[0; 9],
    );
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&invalid_first_storage, invalid_first_signal),
            &mut mpdu,
            &mut ethernet,
            times(4),
            &mut sink,
        ),
        ConnectedRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        }
    );
    let mut invalid_final_storage = [0_u8; 192];
    let invalid_final_signal =
        open_fragment(&mut invalid_final_storage, 8, 1, false, false, SOURCE, &[2]);
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&invalid_final_storage, invalid_final_signal),
            &mut mpdu,
            &mut ethernet,
            times(5),
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::Fragment(OpenDataFragmentError::InvalidLlcSnap),
        }
    );

    invalid_first_storage[FRAME_OFFSET + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch_received(
            segment(&invalid_first_storage, invalid_first_signal),
            &mut mpdu,
            &mut ethernet,
            times(6),
            &mut sink,
        ),
        ConnectedRxDispatch::FragmentBuffered {
            expired: 0,
            evicted: false,
        },
        "failed fragment trains do not poison ordinary duplicate history"
    );
    assert_eq!(dispatcher.clear_open_fragmentation(), 1);
    assert_eq!(sink.ethernet.len(), 1);
}

#[test]
fn reconfigure_revokes_duplicate_and_fragment_history() {
    let payload = [0xaa, 0xaa, 3, 0, 0, 0, 0x08, 0x00, 1];
    let mut ordinary = [0_u8; 192];
    let ordinary_signal = open_fragment(&mut ordinary, 7, 0, false, false, SOURCE, &payload);
    let mut fragment = [0_u8; 192];
    let fragment_signal = open_fragment(&mut fragment, 8, 0, true, false, SOURCE, &payload);
    let mut open = config();
    open.security = LinkProtection::Open;
    open.peer_qos = false;
    let mut dispatcher = ConnectedRxDispatcher::new(open);
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];

    assert!(matches!(
        dispatcher.dispatch(
            segment(&ordinary, ordinary_signal),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Data { .. }
    ));
    ordinary[FRAME_OFFSET + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch(
            segment(&ordinary, ordinary_signal),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );
    assert!(matches!(
        dispatcher.dispatch_received(
            segment(&fragment, fragment_signal),
            &mut mpdu,
            &mut ethernet,
            times(1),
            &mut sink,
        ),
        ConnectedRxDispatch::FragmentBuffered { .. }
    ));
    assert_eq!(dispatcher.fragments.active_contexts(), 1);

    dispatcher.try_reconfigure(open).unwrap();
    assert_eq!(dispatcher.fragments.active_contexts(), 0);
    assert!(matches!(
        dispatcher.dispatch(
            segment(&ordinary, ordinary_signal),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Data { .. }
    ));
}

#[test]
fn reconfigure_refuses_an_in_flight_shared_replay_publication() {
    let (rx, mut control) = replay_resource();
    let original = config();
    let mut replacement = original;
    replacement.bssid = SOURCE;
    let mut dispatcher = ConnectedRxDispatcher::new(original);
    dispatcher.install_shared_ccmp_rx_replay(rx);
    let prepared = prepare_ccmp_replay(
        &mut dispatcher.shared_ccmp_replay,
        &mut dispatcher.owned_ccmp_replay,
        ConnectedRxProtection::Group,
        None,
        ccmp_header(1, 1),
    )
    .unwrap();
    let publication = commit_ccmp_replay(
        &mut dispatcher.shared_ccmp_replay,
        &mut dispatcher.owned_ccmp_replay,
        prepared,
    )
    .unwrap()
    .expect("shared replay prepares one publication permit");

    assert_eq!(
        dispatcher.try_reconfigure(replacement),
        Err(StaCcmpRxReplayError::PublicationInFlight)
    );
    assert_eq!(dispatcher.config(), original);
    assert!(dispatcher.ccmp_rx_replay_enabled());

    drop(publication);
    dispatcher.try_reconfigure(replacement).unwrap();
    assert_eq!(dispatcher.config(), replacement);
    assert!(!dispatcher.ccmp_rx_replay_enabled());
    control.stop().unwrap();
}

#[test]
fn dispatches_protected_ethernet_and_owns_duplicate_history() {
    const HEADER: usize = 24;
    const PAYLOAD: [u8; 4] = [1, 2, 3, 4];
    const MPDU: usize = HEADER + 8 + 8 + PAYLOAD.len() + 8;
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, SIGNAL);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
    frame[0] = 0x08;
    frame[1] = 0x42;
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&SOURCE);
    frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
    frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    frame[HEADER + 8..HEADER + 16].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame[HEADER + 16..HEADER + 20].copy_from_slice(&PAYLOAD);

    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    let mut missing_replay = ConnectedRxDispatcher::new(config());
    assert_eq!(
        missing_replay.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::CcmpReplay(StaCcmpRxReplayError::OwnerUnavailable),
        }
    );
    assert!(sink.ethernet.is_empty());

    let (replay_rx, _replay_control) = replay_resource();
    let mut dispatcher = ConnectedRxDispatcher::new(config());
    dispatcher.install_shared_ccmp_rx_replay(replay_rx);
    assert!(!dispatcher.may_publish_amsdu(segment(&storage, SIGNAL)));
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(sink.ethernet.len(), 1);
    assert_eq!(&sink.ethernet[0][..6], &STATION);
    assert_eq!(&sink.ethernet[0][6..12], &SOURCE);
    assert_eq!(&sink.ethernet[0][12..14], &0x0800_u16.to_be_bytes());
    assert_eq!(&sink.ethernet[0][14..], &PAYLOAD);
    assert_eq!(
        sink.ethernet_metadata[0].crypto,
        MacRxEvidence::HardwareObserved(MacRxCryptoStatus::DecryptedAndIntegrityVerified)
    );
    assert_eq!(
        sink.ethernet_metadata[0].amsdu,
        MacRxEvidence::ProtocolValidated(false)
    );
    assert_eq!(
        sink.ethernet_metadata[0].s_mpdu,
        MacRxEvidence::HardwareObserved(true)
    );
    assert_eq!(
        sink.ethernet_metadata[0].ampdu,
        MacRxEvidence::ProtocolValidated(false)
    );

    // A retransmission reuses the accepted PN. Duplicate removal precedes
    // replay detection, so it is a duplicate rather than a replay.
    storage[FRAME_OFFSET + 1] |= 0x08;
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Duplicate
    );
    assert_eq!(sink.ethernet.len(), 1);

    // Without Retry the reused PN is a replay.
    storage[FRAME_OFFSET + 1] &= !0x08;
    let replay = dispatcher.dispatch(
        segment(&storage, SIGNAL),
        &mut mpdu,
        &mut ethernet,
        &mut sink,
    );
    assert!(matches!(
        replay,
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::CcmpReplay(StaCcmpRxReplayError::Replay(
                CcmpReplayError::Replayed {
                    packet_number,
                    highest,
                }
            )),
        } if packet_number.value() == 3 && highest.value() == 3
    ));
}

#[test]
fn wpa2_admits_only_plaintext_eapol_from_the_exact_associated_link() {
    const HEADER: usize = 24;
    let message3 = oer_ieee80211_rsn::frames::RsnTxFrame::<512>::message3(
        oer_ieee80211_mac::security::rsn::Akm::Psk,
        STATION,
        2,
        [4; 32],
        [0; 8],
        &[0x55; 8],
    )
    .unwrap();
    let mpdu_length = HEADER + 8 + message3.as_bytes().len();
    let signal_length = mpdu_length + 4;
    let mut storage = [0_u8; 256];
    storage[0x1f] = 1;
    storage[TAIL_OFFSET..TAIL_OFFSET + 4].copy_from_slice(
        &(((signal_length + 4) as u32) << 16 | signal_length as u32).to_le_bytes(),
    );
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu_length];
    frame[..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[HEADER..HEADER + 8].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    frame[HEADER + 8..].copy_from_slice(message3.as_bytes());
    let mut dispatcher = dispatcher();
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 192];
    let mut ethernet = [0_u8; 192];
    assert!(!dispatcher.may_publish_ethernet(large_segment(&storage, signal_length)));
    assert_eq!(
        dispatcher.dispatch(
            large_segment(&storage, signal_length),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::UnprotectedEapol
    );
    assert_eq!(sink.unprotected_eapol, [message3.as_bytes()]);
    assert!(sink.ethernet.is_empty());

    storage[FRAME_OFFSET + 16] ^= 1;
    assert_eq!(
        dispatcher.dispatch(
            large_segment(&storage, signal_length),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::SecurityModeMismatch,
        }
    );
    assert_eq!(sink.unprotected_eapol.len(), 1);

    storage[FRAME_OFFSET + 16] ^= 1;
    storage[FRAME_OFFSET + HEADER + 6..FRAME_OFFSET + HEADER + 8]
        .copy_from_slice(&0x0800_u16.to_be_bytes());
    assert_eq!(
        dispatcher.dispatch(
            large_segment(&storage, signal_length),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Rejected {
            protection: ConnectedRxProtection::Pairwise,
            error: ConnectedRxError::SecurityModeMismatch,
        }
    );
    assert_eq!(sink.unprotected_eapol.len(), 1);

    // Open associations keep their ordinary plaintext data semantics;
    // the special EAPOL lane exists only for an installed WPA2 epoch.
    storage[FRAME_OFFSET + HEADER + 6..FRAME_OFFSET + HEADER + 8]
        .copy_from_slice(&0x888e_u16.to_be_bytes());
    let mut open = config();
    open.security = LinkProtection::Open;
    open.peer_qos = false;
    let mut open_dispatcher = ConnectedRxDispatcher::new(open);
    let mut open_sink = RecordingSink::default();
    assert_eq!(
        open_dispatcher.dispatch(
            large_segment(&storage, signal_length),
            &mut mpdu,
            &mut ethernet,
            &mut open_sink,
        ),
        ConnectedRxDispatch::Data {
            ethernet_frames: 1,
            amsdu: false,
        }
    );
    assert_eq!(open_sink.ethernet.len(), 1);
    assert!(open_sink.unprotected_eapol.is_empty());
}

#[test]
fn preflight_detects_amsdu_without_mutating_dispatch_state() {
    const HEADER: usize = 26;
    const FIRST_SUBFRAME: usize = 24;
    const SECOND_SUBFRAME: usize = 25;
    const MPDU: usize = HEADER + 8 + FIRST_SUBFRAME + SECOND_SUBFRAME + 8;
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, SIGNAL);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
    frame[0] = 0x88;
    frame[1] = 0x42;
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&SOURCE);
    frame[22..24].copy_from_slice(&0x1230_u16.to_le_bytes());
    frame[24] = 0x80;
    frame[HEADER..HEADER + 8].copy_from_slice(&[3, 0, 0, 0x20, 0, 0, 0, 0]);
    let mut offset = HEADER + 8;
    frame[offset..offset + 6].copy_from_slice(&STATION);
    frame[offset + 6..offset + 12].copy_from_slice(&SOURCE);
    frame[offset + 12..offset + 14].copy_from_slice(&10_u16.to_be_bytes());
    frame[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    frame[offset + 22..offset + 24].copy_from_slice(&[1, 2]);
    offset += FIRST_SUBFRAME;
    frame[offset..offset + 6].copy_from_slice(&[0xff; 6]);
    frame[offset + 6..offset + 12].copy_from_slice(&SOURCE);
    frame[offset + 12..offset + 14].copy_from_slice(&11_u16.to_be_bytes());
    frame[offset + 14..offset + 22].copy_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x06]);
    frame[offset + 22..offset + 25].copy_from_slice(&[3, 4, 5]);

    let mut dispatcher = dispatcher();
    let segment = segment(&storage, SIGNAL);
    assert_eq!(
        dispatcher.reorder_key(segment),
        Some(RxBlockAckMpduKey {
            peer: BSSID,
            tid: 0,
            sequence: SequenceNumber::new(0x123).unwrap(),
            retry: false,
        })
    );
    assert!(dispatcher.may_publish_amsdu(segment));
    // Preflight is repeatable and does not claim duplicate history.
    assert!(dispatcher.may_publish_amsdu(segment));

    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    assert_eq!(
        dispatcher.dispatch(segment, &mut mpdu, &mut ethernet, &mut sink),
        ConnectedRxDispatch::Data {
            ethernet_frames: 2,
            amsdu: true,
        }
    );
    assert_eq!(sink.ethernet.len(), 2);
    assert!(sink.ethernet_metadata.iter().all(|metadata| {
        metadata.amsdu == MacRxEvidence::ProtocolValidated(true)
            && metadata.s_mpdu == MacRxEvidence::HardwareObserved(true)
            && metadata.ampdu == MacRxEvidence::ProtocolValidated(false)
            && metadata.crypto
                == MacRxEvidence::HardwareObserved(MacRxCryptoStatus::DecryptedAndIntegrityVerified)
    }));
}

#[test]
fn routes_an_addressed_block_ack_action_without_platform_effects() {
    const BODY: [u8; 9] = [3, 0, 9, 0x02, 0x08, 0, 0, 0x30, 0x12];
    const MPDU: usize = 24 + BODY.len();
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, SIGNAL);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
    frame[0] = 0xd0;
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..].copy_from_slice(&BODY);

    let mut dispatcher = ConnectedRxDispatcher::new(config());
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::BlockAck
    );
    assert_eq!(sink.block_ack.len(), 1);
    assert!(matches!(
        sink.block_ack[0],
        BlockAckAction::AddbaRequest {
            dialog_token: 9,
            tid: 0,
            immediate: true,
            window: 32,
            starting_sequence,
            ..
        } if starting_sequence == SequenceNumber::new(0x123).unwrap()
    ));
}

#[test]
fn routes_only_peer_disconnects_addressed_to_this_station() {
    const MPDU: usize = 26;
    const SIGNAL: usize = MPDU + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, SIGNAL);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + MPDU];
    frame[..2].copy_from_slice(&DEAUTHENTICATION_FRAME_CONTROL.to_le_bytes());
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..26].copy_from_slice(&7_u16.to_le_bytes());

    let mut dispatcher = ConnectedRxDispatcher::new(config());
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::PeerDisconnect
    );
    assert_eq!(
        sink.peer_disconnects,
        [StaDisconnect {
            kind: oer_ieee80211_mac::station::StaDisconnectKind::Deauthentication,
            reason_code: 7,
        }]
    );

    storage[FRAME_OFFSET + 4..FRAME_OFFSET + 10].copy_from_slice(&SOURCE);
    assert_eq!(
        dispatcher.dispatch(
            segment(&storage, SIGNAL),
            &mut mpdu,
            &mut ethernet,
            &mut sink,
        ),
        ConnectedRxDispatch::Ignored
    );
    assert_eq!(sink.peer_disconnects.len(), 1);
}

fn protected_management_config() -> ConnectedRxConfig {
    ConnectedRxConfig {
        management_protection: true,
        ..config()
    }
}

/// One management frame from the access point, `body` after the header.
fn management_storage(frame_control: u16, destination: [u8; 6], body: &[u8]) -> ([u8; 192], usize) {
    let mpdu = 24 + body.len();
    let signal = mpdu + 4;
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, signal);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + mpdu];
    frame[..2].copy_from_slice(&frame_control.to_le_bytes());
    frame[4..10].copy_from_slice(&destination);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[24..].copy_from_slice(body);
    (storage, signal)
}

fn dispatch_management(
    config: ConnectedRxConfig,
    frame_control: u16,
    destination: [u8; 6],
    body: &[u8],
) -> (ConnectedRxDispatch, RecordingSink) {
    let (storage, signal) = management_storage(frame_control, destination, body);
    let mut dispatcher = ConnectedRxDispatcher::new(config);
    let mut sink = RecordingSink::default();
    let mut mpdu = [0_u8; 128];
    let mut ethernet = [0_u8; 128];
    let dispatch = dispatcher.dispatch(
        segment(&storage, signal),
        &mut mpdu,
        &mut ethernet,
        &mut sink,
    );
    (dispatch, sink)
}

#[test]
fn an_unprotected_class_2_or_3_disconnect_starts_an_sa_query_under_protection() {
    for reason in [6_u16, 7] {
        let (dispatch, sink) = dispatch_management(
            protected_management_config(),
            DEAUTHENTICATION_FRAME_CONTROL,
            STATION,
            &reason.to_le_bytes(),
        );
        assert_eq!(dispatch, ConnectedRxDispatch::UnprotectedDisconnect);
        assert!(sink.peer_disconnects.is_empty());
        assert_eq!(sink.unprotected_disconnects[0].reason_code, reason);
    }
    // Any other unprotected disconnect is dropped.
    let (dispatch, sink) = dispatch_management(
        protected_management_config(),
        DISASSOCIATION_FRAME_CONTROL,
        STATION,
        &3_u16.to_le_bytes(),
    );
    assert_eq!(dispatch, ConnectedRxDispatch::Ignored);
    assert!(sink.peer_disconnects.is_empty() && sink.unprotected_disconnects.is_empty());
}

#[test]
fn protected_and_group_robust_frames_go_to_control_to_open() {
    const PROTECTED: u16 = 0x4000;
    let body = [0x5a; 18];
    let (dispatch, sink) = dispatch_management(
        protected_management_config(),
        DEAUTHENTICATION_FRAME_CONTROL | PROTECTED,
        STATION,
        &body,
    );
    assert_eq!(dispatch, ConnectedRxDispatch::ProtectedManagement);
    assert_eq!(sink.protected_management.len(), 1);
    assert!(!sink.protected_management[0].1);
    assert_eq!(sink.protected_management[0].0[24..], body);

    let (dispatch, sink) = dispatch_management(
        protected_management_config(),
        DEAUTHENTICATION_FRAME_CONTROL,
        [0xff; 6],
        &body,
    );
    assert_eq!(dispatch, ConnectedRxDispatch::ProtectedManagement);
    assert!(sink.protected_management[0].1);

    let (dispatch, sink) = dispatch_management(
        protected_management_config(),
        ACTION_FRAME_CONTROL | PROTECTED,
        STATION,
        &body,
    );
    assert_eq!(dispatch, ConnectedRxDispatch::ProtectedManagement);
    assert_eq!(sink.protected_management.len(), 1);
}

#[test]
fn an_unprotected_robust_action_is_dropped_under_protection() {
    // An unprotected ADDBA Request: category 3 is robust.
    let addba = [3, 0, 1, 0x02, 0x10, 0, 0, 0x10, 0];
    let (dispatch, sink) = dispatch_management(
        protected_management_config(),
        ACTION_FRAME_CONTROL,
        STATION,
        &addba,
    );
    assert_eq!(dispatch, ConnectedRxDispatch::Ignored);
    assert!(sink.block_ack.is_empty() && sink.protected_management.is_empty());
    let (dispatch, sink) = dispatch_management(config(), ACTION_FRAME_CONTROL, STATION, &addba);
    assert_eq!(dispatch, ConnectedRxDispatch::BlockAck);
    assert_eq!(sink.block_ack.len(), 1);
}

#[test]
fn a_block_ack_request_from_the_associated_ap_names_its_reorder_session() {
    let mut storage = [0_u8; 192];
    set_tail(&mut storage, 24);
    let frame = &mut storage[FRAME_OFFSET..FRAME_OFFSET + 20];
    frame[..2].copy_from_slice(&[0x84, 0x00]);
    frame[4..10].copy_from_slice(&STATION);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..18].copy_from_slice(&(3_u16 << 12 | 0x0004).to_le_bytes());
    frame[18..20].copy_from_slice(&(77_u16 << 4).to_le_bytes());
    let dispatcher = dispatcher();
    assert_eq!(
        dispatcher.block_ack_request_key(segment(&storage, 24)),
        Some(RxBlockAckRequestKey {
            peer: BSSID,
            tid: 3,
            starting_sequence: SequenceNumber::new(77).unwrap(),
        })
    );
    assert_eq!(dispatcher.reorder_key(segment(&storage, 24)), None);

    storage[FRAME_OFFSET + 10] ^= 1;
    assert_eq!(
        dispatcher.block_ack_request_key(segment(&storage, 24)),
        None
    );
}
