use super::*;
use crate::interface::{VifId, VirtualInterface};
use oer_ieee80211_port::VifRole;
use oer_ieee80211_wire::channel::ChannelWidth;

fn channel() -> WifiChannel {
    WifiChannel::new_2_4_ghz(1, ChannelWidth::Mhz20).unwrap()
}

fn peer(last: u8, key: u8) -> EspNowEncryptedPeerConfig {
    EspNowEncryptedPeerConfig::new(
        EspNowUnicastAddress::new([2, 0, 0, 0, 0, last]).unwrap(),
        channel(),
        EspNowLmk::new([key; 16]),
    )
}

fn pn(value: u64) -> EspNowCcmpPacketNumber {
    EspNowCcmpPacketNumber::new(value).unwrap()
}

#[test]
fn pmk_replace_returns_old_key_and_retains_proposal_on_each_refusal() {
    let mut owner = EspNowPmkOwner::from_key(EspNowPmk::new([1; 16]));
    let first = owner.current_id().unwrap();
    let (current, old) = owner.replace(first, EspNowPmk::new([2; 16])).unwrap();
    assert_eq!(old.expose_secret(), &[1; 16]);
    assert_eq!(owner.key(current).unwrap().expose_secret(), &[2; 16]);
    let (error, proposed) = owner
        .replace(first, EspNowPmk::new([3; 16]))
        .unwrap_err()
        .into_parts();
    assert_eq!(error, EspNowPmkError::Stale(first));
    assert_eq!(proposed.expose_secret(), &[3; 16]);
    owner.remove(current).unwrap();
    let (error, proposed) = owner
        .replace(current, EspNowPmk::new([4; 16]))
        .unwrap_err()
        .into_parts();
    assert_eq!(error, EspNowPmkError::Stale(current));
    assert_eq!(proposed.expose_secret(), &[4; 16]);
    owner.generation = u32::MAX;
    owner.key = Some(EspNowPmk::new([5; 16]));
    let last = owner.current_id().unwrap();
    let (error, proposed) = owner
        .replace(last, EspNowPmk::new([6; 16]))
        .unwrap_err()
        .into_parts();
    assert_eq!(error, EspNowPmkError::GenerationExhausted);
    assert_eq!(proposed.expose_secret(), &[6; 16]);
    assert_eq!(owner.key(last).unwrap().expose_secret(), &[5; 16]);
    owner.remove(last).unwrap();
    assert_eq!(
        owner
            .replace(last, EspNowPmk::new([7; 16]))
            .unwrap_err()
            .error(),
        EspNowPmkError::Stale(last)
    );
}

#[test]
fn invalid_counts_refuse_encrypted_mutations_without_losing_keys() {
    let mut table = EspNowEncryptedPeerTable::<2>::new(channel());
    table.length = usize::MAX;
    let (error, proposed) = table.install(peer(2, 1)).unwrap_err().into_parts();
    assert_eq!(
        error,
        EspNowEncryptedPeerError::InvalidCount {
            count: usize::MAX,
            capacity: 2
        }
    );
    assert_eq!(proposed.lmk().expose_secret(), &[1; 16]);
    assert!(
        table
            .slots
            .iter()
            .all(|slot| slot.state.is_none() && slot.generation == 0)
    );
    table.length = 2;
    let (error, proposed) = table.install(peer(2, 1)).unwrap_err().into_parts();
    assert_eq!(
        error,
        EspNowEncryptedPeerError::InvalidCount {
            count: 2,
            capacity: 2
        }
    );
    assert_eq!(proposed.lmk().expose_secret(), &[1; 16]);
    table.length = 0;
    let first = table.install(peer(2, 1)).unwrap();
    assert_eq!(table.len(), 1);
    let second = table.install(peer(3, 2)).unwrap();
    table.length = 0;
    assert_eq!(
        table.remove(first).unwrap_err(),
        EspNowEncryptedPeerError::InvalidCount {
            count: 0,
            capacity: 2
        }
    );
    assert_eq!(table.lmk(first).unwrap().expose_secret(), &[1; 16]);
    table.length = 3;
    assert_eq!(
        table.remove(first).unwrap_err(),
        EspNowEncryptedPeerError::InvalidCount {
            count: 3,
            capacity: 2
        }
    );
    table.length = 2;
    let removed = table.remove(first).unwrap();
    assert_eq!(table.len(), 1);
    table.length = usize::MAX;
    let (error, removed) = table.restore_removed(removed).unwrap_err().into_parts();
    assert_eq!(
        error,
        EspNowEncryptedPeerError::InvalidCount {
            count: usize::MAX,
            capacity: 2
        }
    );
    assert_eq!(removed.config().lmk().expose_secret(), &[1; 16]);
    assert!(table.slots[0].state.is_none());
    table.length = 2;
    let (error, removed) = table.restore_removed(removed).unwrap_err().into_parts();
    assert_eq!(
        error,
        EspNowEncryptedPeerError::InvalidCount {
            count: 2,
            capacity: 2
        }
    );
    assert_eq!(removed.config().lmk().expose_secret(), &[1; 16]);
    table.length = 1;
    let restored = table.restore_removed(removed).unwrap();
    assert_eq!(table.len(), 2);
    assert_eq!(table.lmk(restored).unwrap().expose_secret(), &[1; 16]);
    assert_eq!(table.lmk(second).unwrap().expose_secret(), &[2; 16]);
    assert_ne!(restored, first);
    assert_eq!(table.clear_on_shutdown(), 2);
    assert_eq!(table.clear_on_shutdown(), 0);
    assert_eq!(table.diagnostics().cleared_on_shutdown, 2);
    assert_eq!(table.len(), 0);
}

#[test]
fn encrypted_capacity_and_generation_refusals_retain_proposed_key() {
    let (error, proposed) = EspNowEncryptedPeerTable::<0>::new(channel())
        .install(peer(2, 7))
        .unwrap_err()
        .into_parts();
    assert_eq!(error, EspNowEncryptedPeerError::Full);
    assert_eq!(proposed.lmk().expose_secret(), &[7; 16]);
    let mut table = EspNowEncryptedPeerTable::<1>::new(channel());
    table.slots[0].generation = u32::MAX;
    let (error, proposed) = table.install(peer(2, 8)).unwrap_err().into_parts();
    assert_eq!(error, EspNowEncryptedPeerError::GenerationExhausted);
    assert_eq!(proposed.lmk().expose_secret(), &[8; 16]);
    assert!(table.is_empty());
}

#[test]
fn encrypted_replacement_preserves_packet_number_spaces_and_old_key() {
    let mut table = EspNowEncryptedPeerTable::<1>::new(channel());
    let first = table.install(peer(2, 1)).unwrap();
    assert_eq!(table.allocate_tx_packet_number(first), Ok(pn(1)));
    let replay = table.prepare_rx_replay(first, pn(50)).unwrap();
    table.commit_authenticated_rx(replay).unwrap();
    let replacement = table.replace(first, peer(2, 2)).unwrap();
    assert_eq!(replacement.replaced.lmk().expose_secret(), &[1; 16]);
    assert_eq!(
        table.lmk(replacement.peer).unwrap().expose_secret(),
        &[2; 16]
    );
    assert_eq!(table.allocate_tx_packet_number(replacement.peer), Ok(pn(2)));
    assert!(matches!(
        table.prepare_rx_replay(replacement.peer, pn(50)),
        Err(EspNowEncryptedPeerError::ReplayDuplicate { .. })
    ));
    let (error, proposed) = table.replace(first, peer(2, 3)).unwrap_err().into_parts();
    assert_eq!(error, EspNowEncryptedPeerError::Stale(first));
    assert_eq!(proposed.lmk().expose_secret(), &[3; 16]);
    let current = replacement.peer;
    let view = table.get(current).unwrap();
    let (error, proposed) = table.replace(current, peer(3, 4)).unwrap_err().into_parts();
    assert!(matches!(
        error,
        EspNowEncryptedPeerError::ReplacementDestinationMismatch { .. }
    ));
    assert_eq!(proposed.lmk().expose_secret(), &[4; 16]);
    assert_eq!(table.get(current), Ok(view));
    table.slots[0].generation = u32::MAX;
    let last = EspNowEncryptedPeerId {
        index: 0,
        generation: u32::MAX,
    };
    let (error, proposed) = table.replace(last, peer(2, 5)).unwrap_err().into_parts();
    assert_eq!(error, EspNowEncryptedPeerError::GenerationExhausted);
    assert_eq!(proposed.lmk().expose_secret(), &[5; 16]);
    assert_eq!(table.lmk(last).unwrap().expose_secret(), &[2; 16]);
}

#[test]
fn invalid_or_vacant_encrypted_slots_refuse_without_consuming_secrets() {
    let mut table = EspNowEncryptedPeerTable::<1>::new(channel());
    for index in [0, usize::MAX] {
        let stale = EspNowEncryptedPeerId {
            index,
            generation: 0,
        };
        let (error, proposed) = table.replace(stale, peer(2, 9)).unwrap_err().into_parts();
        assert_eq!(error, EspNowEncryptedPeerError::Stale(stale));
        assert_eq!(proposed.lmk().expose_secret(), &[9; 16]);
    }
    let installed = table.install(peer(2, 1)).unwrap();
    let mut removed = table.remove(installed).unwrap();
    removed.index = usize::MAX;
    let previous = removed.previous_id();
    let (error, removed) = table.restore_removed(removed).unwrap_err().into_parts();
    assert_eq!(error, EspNowEncryptedPeerError::Stale(previous));
    assert_eq!(removed.config().lmk().expose_secret(), &[1; 16]);
}

#[test]
fn invalid_and_exhausted_tx_numbers_return_error_without_advancing() {
    let mut table = EspNowEncryptedPeerTable::<1>::new(channel());
    let peer = table.install(peer(2, 1)).unwrap();
    for last in [
        ESP_NOW_PACKET_NUMBER_MAX,
        ESP_NOW_PACKET_NUMBER_MAX + 1,
        u64::MAX,
    ] {
        table.state_mut(peer).unwrap().last_tx_packet_number = last;
        assert_eq!(
            table.allocate_tx_packet_number(peer),
            Err(EspNowEncryptedPeerError::TxPacketNumberExhausted(peer))
        );
        assert_eq!(table.get(peer).unwrap().last_tx_packet_number, last);
    }
    table.state_mut(peer).unwrap().last_tx_packet_number = ESP_NOW_PACKET_NUMBER_MAX - 1;
    assert_eq!(
        table.allocate_tx_packet_number(peer),
        Ok(pn(ESP_NOW_PACKET_NUMBER_MAX))
    );
    assert_eq!(
        table.allocate_tx_packet_number(peer),
        Err(EspNowEncryptedPeerError::TxPacketNumberExhausted(peer))
    );
    assert_eq!(table.diagnostics().tx_packet_numbers_allocated, 1);
    assert_eq!(table.diagnostics().tx_packet_number_exhaustions, 4);
}

#[test]
fn replay_distances_cover_window_edges_and_full_packet_number_range() {
    let mut table = EspNowEncryptedPeerTable::<1>::new(channel());
    let peer = table.install(peer(2, 1)).unwrap();
    let first = table.prepare_rx_replay(peer, pn(1)).unwrap();
    table.commit_authenticated_rx(first).unwrap();
    let edge = table.prepare_rx_replay(peer, pn(64)).unwrap();
    table.commit_authenticated_rx(edge).unwrap();
    assert!(matches!(
        table.prepare_rx_replay(peer, pn(1)),
        Err(EspNowEncryptedPeerError::ReplayDuplicate { .. })
    ));
    let next = table.prepare_rx_replay(peer, pn(65)).unwrap();
    table.commit_authenticated_rx(next).unwrap();
    assert!(matches!(
        table.prepare_rx_replay(peer, pn(1)),
        Err(EspNowEncryptedPeerError::ReplayTooOld { .. })
    ));
    let unseen = table.prepare_rx_replay(peer, pn(2)).unwrap();
    table.commit_authenticated_rx(unseen).unwrap();
    let far = table
        .prepare_rx_replay(peer, pn(ESP_NOW_PACKET_NUMBER_MAX))
        .unwrap();
    table.commit_authenticated_rx(far).unwrap();
    assert!(matches!(
        table.prepare_rx_replay(peer, pn(65)),
        Err(EspNowEncryptedPeerError::ReplayTooOld { .. })
    ));
}

#[test]
fn encrypted_length_validation_precedes_packet_and_sequence_consumption() {
    assert_eq!(
        encrypted_v1_encoded_len(usize::MAX),
        Err(EspNowV1WireError::PayloadTooLong { length: usize::MAX })
    );
    let config = EspNowConfig::new(
        BoundVirtualInterface::new(
            VirtualInterface::new(VifId::PRIMARY, VifRole::Station, [2, 0, 0, 0, 0, 1]),
            ChannelContextId::PRIMARY,
        ),
        channel(),
    )
    .unwrap();
    let mut protocol = EspNowEncryptedProtocol::<1>::new(config, EspNowPmk::new([1; 16]));
    let peer = protocol.add_peer(peer(2, 2)).unwrap();
    let mut sequence = SequenceCounter::new(SequenceNumber::ZERO);
    let random = EspNowRandomValue::new([1; 4]);
    assert!(matches!(
        protocol.prepare_v1_tx(peer, &mut sequence, random, &[0; 251]),
        Err(EspNowEncryptedSendError::Wire(
            EspNowV1WireError::PayloadTooLong { .. }
        ))
    ));
    assert_eq!(sequence.peek(), SequenceNumber::ZERO);
    assert_eq!(protocol.peers().get(peer).unwrap().last_tx_packet_number, 0);
    let empty = protocol
        .prepare_v1_tx(peer, &mut sequence, random, &[])
        .unwrap();
    assert_eq!(empty.encoded_len(), 55);
    let payload = [0; 250];
    let full = protocol
        .prepare_v1_tx(peer, &mut sequence, random, &payload)
        .unwrap();
    assert_eq!(full.encoded_len(), 305);
    assert_eq!(full.packet_number(), pn(2));
}
