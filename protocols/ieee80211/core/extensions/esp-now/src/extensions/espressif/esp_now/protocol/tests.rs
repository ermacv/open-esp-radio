extern crate std;

use super::*;
use oer_protocols_ieee80211_core_arbiter::interface::{VifId, VirtualInterface};
use oer_protocols_ieee80211_wire::channel::ChannelWidth;

fn channel(number: u8) -> Ghz2_4Channel {
    Ghz2_4Channel::new(number, ChannelWidth::Mhz20).unwrap()
}

fn config() -> EspNowConfig {
    EspNowConfig::new(
        BoundVirtualInterface::new(
            VirtualInterface::new(VifId::PRIMARY, VifRole::Station, [2, 0, 0, 0, 0, 1]),
            ChannelContextId::PRIMARY,
        ),
        channel(1),
    )
    .unwrap()
}

fn peer(last: u8) -> EspNowPeerConfig {
    EspNowPeerConfig::plaintext(
        EspNowDestination::Unicast(EspNowUnicastAddress::new([2, 0, 0, 0, 0, last]).unwrap()),
        channel(1),
    )
}

/// The count is the occupied slots: it follows each add and remove, and a
/// stale handle changes nothing.
#[test]
fn the_count_follows_the_occupied_slots() {
    let mut table = EspNowPeerTable::<2>::new(channel(1));
    assert!(table.is_empty());
    let first = table.add(peer(2)).unwrap();
    assert_eq!(table.len(), 1);
    let second = table.add(peer(3)).unwrap();
    assert_eq!(table.len(), 2);
    assert!(!table.is_empty());
    assert_eq!(table.add(peer(4)), Err(EspNowPeerTableError::Full));
    assert_eq!(table.remove(first), Ok(peer(2)));
    assert_eq!(table.len(), 1);
    assert_eq!(table.get(second), Ok(peer(3)));
    assert_eq!(table.remove(first), Err(EspNowPeerTableError::Stale(first)));
    assert_eq!(table.get(first), Err(EspNowPeerTableError::Stale(first)));
    assert_eq!(table.len(), 1);
    assert_eq!(table.remove(second), Ok(peer(3)));
    assert!(table.is_empty());
}

#[test]
fn zero_capacity_exhaustion_and_sparse_iteration_preserve_slot_identity() {
    assert_eq!(
        EspNowPeerTable::<0>::new(channel(1)).add(peer(2)),
        Err(EspNowPeerTableError::Full)
    );
    let mut table = EspNowPeerTable::<3>::new(channel(1));
    table.slots[0] = EspNowPeerSlot::Vacant {
        generation: u32::MAX,
    };
    let first = table.add(peer(2)).unwrap();
    let last = table.add(peer(3)).unwrap();
    assert_eq!((first.index(), last.index()), (1, 2));
    assert_eq!(
        table.add(peer(2)),
        Err(EspNowPeerTableError::Duplicate(peer(2).destination()))
    );
    assert_eq!(
        table.add(peer(4)),
        Err(EspNowPeerTableError::GenerationExhausted)
    );
    table.remove(first).unwrap();
    let mut peers = table.iter();
    assert_eq!(peers.size_hint(), (0, Some(3)));
    assert_eq!(peers.next(), Some((last, peer(3))));
    assert_eq!(peers.size_hint(), (0, Some(0)));
    assert_eq!(peers.next(), None);
    assert_eq!(peers.next(), None);
    let reused = table.add(peer(4)).unwrap();
    assert_eq!(reused.index(), first.index());
    assert_ne!(reused.generation(), first.generation());
    let invalid = EspNowPeerId {
        index: usize::MAX,
        generation: 1,
    };
    assert_eq!(
        table.get(invalid),
        Err(EspNowPeerTableError::Stale(invalid))
    );
    assert_eq!(
        table.remove(invalid),
        Err(EspNowPeerTableError::Stale(invalid))
    );
}

#[test]
fn rx_snapshot_keeps_sparse_home_unicast_slots_and_validates_ownership() {
    let config = config();
    let mut protocol = EspNowProtocol::<4>::new(config);
    let first = protocol.add_peer(peer(2)).unwrap();
    protocol
        .add_peer(EspNowPeerConfig::plaintext(
            EspNowDestination::Broadcast,
            channel(1),
        ))
        .unwrap();
    let last = protocol.add_peer(peer(3)).unwrap();
    protocol
        .add_peer(EspNowPeerConfig::plaintext_off_channel(
            peer(4).destination(),
            channel(6),
        ))
        .unwrap();
    protocol.remove_peer(first).unwrap();
    let epoch = protocol
        .begin_rx_epoch(config.station(), config.home_channel())
        .unwrap();
    assert_eq!(
        epoch
            .slots
            .iter()
            .filter(|slot| slot.peer.is_some())
            .count(),
        1
    );
    assert!(protocol.owns_rx_epoch(&epoch));
    assert_eq!(epoch.peer_phy_mode(last), Some(EspNowPhyMode::LegacyDsss1M));
    assert_eq!(epoch.peer_phy_mode(first), None);
    protocol.remove_peer(last).unwrap();
    assert!(!protocol.owns_rx_epoch(&epoch));
    let empty = EspNowProtocol::<0>::new(config);
    let epoch = empty
        .begin_rx_epoch(config.station(), config.home_channel())
        .unwrap();
    assert!(empty.owns_rx_epoch(&epoch));
}

#[test]
fn duplicate_history_evicts_only_oldest_and_reset_counts_all_fingerprints() {
    let config = config();
    let mut protocol = EspNowProtocol::<1>::new(config);
    let peer = protocol.add_peer(peer(2)).unwrap();
    let mut epoch = protocol
        .begin_rx_epoch(config.station(), config.home_channel())
        .unwrap();
    let frame = |number: u8, output: &mut [u8]| {
        EspNowV1Frame::new(
            EspNowDestination::Unicast(config.local_address()),
            EspNowUnicastAddress::new([2, 0, 0, 0, 0, 2]).unwrap(),
            SequenceNumber::new(u16::from(number)).unwrap(),
            EspNowRandomValue::new([number; 4]),
            b"body",
        )
        .unwrap()
        .encode(output)
        .unwrap()
    };
    let mut bytes = [0; 300];
    for number in 0..5 {
        let len = frame(number, &mut bytes);
        assert!(matches!(
            epoch.receive_v1(&bytes[..len]),
            Ok(EspNowRxOutcome::Received(_))
        ));
    }
    for number in 1..5 {
        let len = frame(number, &mut bytes);
        assert_eq!(
            epoch.receive_v1(&bytes[..len]),
            Ok(EspNowRxOutcome::Duplicate { peer })
        );
    }
    let len = frame(0, &mut bytes);
    assert!(matches!(
        epoch.receive_v1(&bytes[..len]),
        Ok(EspNowRxOutcome::Received(_))
    ));
    assert_eq!(epoch.reset_duplicate_history(), 4);
    assert_eq!(epoch.reset_duplicate_history(), 0);
    assert!(matches!(
        epoch.receive_v1(&bytes[..len]),
        Ok(EspNowRxOutcome::Received(_))
    ));
}

/// A slot reused by a later peer gives it the next generation; the
/// earlier handle is stale there.
#[test]
fn a_reused_slot_makes_the_earlier_handle_stale() {
    let mut table = EspNowPeerTable::<1>::new(channel(1));
    let first = table.add(peer(2)).unwrap();
    assert_eq!(first.generation(), 1);
    table.remove(first).unwrap();
    let second = table.add(peer(3)).unwrap();
    assert_eq!((second.index(), second.generation()), (first.index(), 2));
    assert_eq!(table.iter().next(), Some((second, peer(3))));
    assert_eq!(table.get(first), Err(EspNowPeerTableError::Stale(first)));
    assert_eq!(table.remove(first), Err(EspNowPeerTableError::Stale(first)));
    assert_eq!(table.get(second), Ok(peer(3)));
}

/// Every table error names itself: its text is never empty.
#[test]
fn a_table_error_s_text_names_it() {
    let errors = [
        EspNowPeerTableError::Full,
        EspNowPeerTableError::Duplicate(EspNowDestination::Broadcast),
        EspNowPeerTableError::ChannelMismatch {
            peer: channel(2),
            home: channel(1),
        },
        EspNowPeerTableError::OffChannelMatchesHome(channel(1)),
        EspNowPeerTableError::GenerationExhausted,
    ];
    for error in errors {
        assert!(!std::format!("{error}").is_empty(), "{error:?}");
    }
}
