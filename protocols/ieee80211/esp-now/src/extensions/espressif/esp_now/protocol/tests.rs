use super::*;
use oer_ieee80211_arbiter::interface::{VifId, VirtualInterface};
use oer_ieee80211_wire::channel::ChannelWidth;

fn channel(number: u8) -> WifiChannel {
    WifiChannel::new_2_4_ghz(number, ChannelWidth::Mhz20).unwrap()
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

#[test]
fn invalid_counts_refuse_mutations_without_losing_peers() {
    let mut table = EspNowPeerTable::<2>::new(channel(1));
    // Fault injection: the former increment and decrement panicked here.
    table.length = usize::MAX;
    assert_eq!(
        table.add(peer(2)),
        Err(EspNowPeerTableError::InvalidCount {
            count: usize::MAX,
            capacity: 2
        })
    );
    assert!(
        table
            .slots
            .iter()
            .all(|slot| slot.config.is_none() && slot.generation == 0)
    );
    table.length = 2;
    assert_eq!(
        table.add(peer(2)),
        Err(EspNowPeerTableError::InvalidCount {
            count: 2,
            capacity: 2
        })
    );
    table.length = 0;
    let first = table.add(peer(2)).unwrap();
    assert_eq!(table.len(), 1);
    let second = table.add(peer(3)).unwrap();
    table.length = 0;
    assert_eq!(
        table.remove(first),
        Err(EspNowPeerTableError::InvalidCount {
            count: 0,
            capacity: 2
        })
    );
    assert_eq!(table.get(first), Ok(peer(2)));
    table.length = 3;
    assert_eq!(
        table.remove(first),
        Err(EspNowPeerTableError::InvalidCount {
            count: 3,
            capacity: 2
        })
    );
    table.length = 2;
    assert_eq!(table.remove(first), Ok(peer(2)));
    assert_eq!(table.len(), 1);
    assert_eq!(table.get(second), Ok(peer(3)));
    assert_eq!(table.remove(first), Err(EspNowPeerTableError::Stale(first)));
    assert_eq!(table.len(), 1);
}

#[test]
fn zero_capacity_exhaustion_and_sparse_iteration_preserve_slot_identity() {
    assert_eq!(
        EspNowPeerTable::<0>::new(channel(1)).add(peer(2)),
        Err(EspNowPeerTableError::Full)
    );
    let mut table = EspNowPeerTable::<3>::new(channel(1));
    table.slots[0].generation = u32::MAX;
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
    assert_eq!(epoch.peer_count(), 1);
    assert!(protocol.owns_rx_epoch(&epoch));
    assert_eq!(epoch.peer_phy_mode(last), Some(EspNowPhyMode::LegacyDsss1M));
    assert_eq!(epoch.peer_phy_mode(first), None);
    protocol.remove_peer(last).unwrap();
    assert!(!protocol.owns_rx_epoch(&epoch));
    let empty = EspNowProtocol::<0>::new(config);
    assert_eq!(
        empty
            .begin_rx_epoch(config.station(), config.home_channel())
            .unwrap()
            .peer_count(),
        0
    );
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
            SequenceNumber::from_low_bits(u16::from(number)),
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
