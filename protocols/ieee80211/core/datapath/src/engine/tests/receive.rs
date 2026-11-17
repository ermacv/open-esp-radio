//! The receive half's cases (C1): a round a section over the model port.
//! The deleted actor's receive tests (`b88a3d2cc`, the reading's section 5,
//! rows 10, 13, 14, 17, 18 and 29) as the engine's; 15 and 16 are the
//! reorder's (C2).

use super::*;
use oer_protocols_ieee80211_contracts_port::{RxCryptoStatus, RxEvidence, RxMeta};
use oer_protocols_ieee80211_wire::channel::{Channel, ChannelWidth};

/// A source behind the access point.
const SOURCE: [u8; 6] = [2, 0, 0, 0, 0, 0x20];
pub(super) const IPV4: u16 = 0x0800;

pub(super) fn meta() -> RxMeta {
    RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap())
}

fn verified() -> RxMeta {
    RxMeta {
        crypto: RxEvidence::HardwareObserved(RxCryptoStatus::DecryptedAndIntegrityVerified),
        ..meta()
    }
}

/// A data MPDU from the access point to the station: `ether_type` and a
/// payload of `tag`.
pub(super) fn data(sequence: u16, ether_type: u16, tag: u8) -> Vec<u8> {
    let mut frame = std::vec![0x08, 0x02, 0, 0];
    frame.extend_from_slice(&STATION);
    frame.extend_from_slice(&ACCESS_POINT_ADDRESS);
    frame.extend_from_slice(&SOURCE);
    frame.extend_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(&[tag; 8]);
    frame
}

/// The Ethernet frame `data(_, ether_type, tag)` carries.
fn ethernet(ether_type: u16, tag: u8) -> Vec<u8> {
    let mut frame = STATION.to_vec();
    frame.extend_from_slice(&SOURCE);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(&[tag; 8]);
    frame
}

/// The same data MPDU, protected: a CCMP header the port keeps before the
/// body.
fn protected(sequence: u16, tag: u8) -> Vec<u8> {
    let plain = data(sequence, IPV4, tag);
    let mut frame = plain[..24].to_vec();
    frame[1] |= 0x40;
    frame.extend_from_slice(&[1, 0, 0, 0x20, 0, 0, 0, 0]);
    frame.extend_from_slice(&plain[24..]);
    frame
}

/// A QoS data MPDU carrying an A-MSDU of one subframe a tag.
fn amsdu(tags: &[u8]) -> Vec<u8> {
    let mut frame = std::vec![0x88, 0x02, 0, 0];
    frame.extend_from_slice(&STATION);
    frame.extend_from_slice(&ACCESS_POINT_ADDRESS);
    frame.extend_from_slice(&SOURCE);
    frame.extend_from_slice(&[0, 0]);
    // QoS Control: TID 0, A-MSDU Present.
    frame.extend_from_slice(&[0x80, 0]);
    for (index, tag) in tags.iter().enumerate() {
        let mut msdu = std::vec![0xaa, 0xaa, 0x03, 0, 0, 0];
        msdu.extend_from_slice(&IPV4.to_be_bytes());
        msdu.extend_from_slice(&[*tag; 8]);
        frame.extend_from_slice(&STATION);
        frame.extend_from_slice(&SOURCE);
        frame.extend_from_slice(&(msdu.len() as u16).to_be_bytes());
        frame.extend_from_slice(&msdu);
        if index + 1 < tags.len() {
            while frame.len() % 4 != 26 % 4 {
                frame.push(0);
            }
        }
    }
    frame
}

/// A beacon of the access point.
pub(super) fn beacon(sequence: u16) -> Vec<u8> {
    let mut frame = std::vec![0x80, 0x00, 0, 0];
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&ACCESS_POINT_ADDRESS);
    frame.extend_from_slice(&ACCESS_POINT_ADDRESS);
    frame.extend_from_slice(&(sequence << 4).to_le_bytes());
    frame.extend_from_slice(&[0; 12]);
    frame
}

/// The Ethernet frame a lent unit at `place` holds.
fn lent_frame(around: &Around<'_, STATION_SINGLES, 0>, index: usize) -> Vec<u8> {
    let (unit, span) = &around.net.lent[index];
    let bytes = around.port.place_bytes(unit.place()).unwrap();
    let start = usize::from(span.offset);
    bytes[start..start + usize::from(span.length)].to_vec()
}

fn refusals<const POOL: usize>(engine: &mut StationEngine<'_, POOL>) -> RxRefusals {
    station_flowing(engine).refusals()
}

/// The deleted actor's `received_data_reaches_the_network_without_the_lifecycle`:
/// a data frame of the access point is lent to the network as an Ethernet
/// frame, written in its place, and the controller is told nothing.
#[test]
fn received_data_is_lent_to_the_network_as_an_ethernet_frame_and_the_controller_is_told_nothing() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.port.land(&data(1, IPV4, 9), meta());
        assert_eq!(engine.received(&mut around.turn()), Rest::Wake(None));
        assert_eq!(around.net.lent.len(), 1);
        assert_eq!(lent_frame(&around, 0), ethernet(IPV4, 9));
        assert!(around.told.frames.is_empty());
        assert_eq!(
            around.port.places_held(),
            1,
            "the lent unit keeps its place"
        );
        around.net.lent.clear();
        let _ = engine.returned(&mut around.turn());
        assert_eq!(around.port.places_held(), 0, "it came home");
    });
}

/// The deleted actor's `a_frame_whose_header_does_not_decode_goes_to_the_station_whole`,
/// as the contract has it: the port does not stage a frame whose header
/// does not decode, so it reaches neither the network nor the controller.
#[test]
fn a_frame_whose_header_does_not_decode_reaches_no_one() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.port.land(&data(1, IPV4, 9)[..20], meta());
        let _ = engine.received(&mut around.turn());
        assert!(around.net.received.is_empty());
        assert!(around.told.frames.is_empty());
        assert_eq!(around.port.counts().dropped, 1, "counted by the port");
        assert_eq!(around.port.ring_len(), 0);
    });
}

/// A management frame and EAPOL go to the controller, read in place inside
/// the round, and back to the ring at its close.
#[test]
fn management_and_eapol_are_read_by_the_controller_inside_the_round() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.port.land(&beacon(1), meta());
        around.port.land(
            &data(
                2,
                oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL,
                7,
            ),
            meta(),
        );
        let _ = engine.received(&mut around.turn());
        assert_eq!(around.told.frames.len(), 2);
        assert_eq!(around.told.frames[0], (Some(0), beacon(1)));
        assert_eq!(
            around.told.frames[1].1,
            data(
                2,
                oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL,
                7
            )
        );
        assert!(around.net.received.is_empty());
        assert_eq!(around.port.places_held(), 0, "both went back");
    });
}

/// The deleted actor's `frames_for_the_station_wait_in_the_ring_while_the_lifecycle_holds_its_lane`,
/// as D4 has it: there is no lane, so six management frames are six reads,
/// and none waits in the ring.
#[test]
fn every_management_frame_is_read_in_its_round_and_none_waits() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        for sequence in 0..6 {
            around.port.land(&beacon(sequence), meta());
        }
        let _ = engine.received(&mut around.turn());
        assert_eq!(around.told.frames.len(), 6);
        assert_eq!(around.port.ring_len(), 0);
        assert_eq!(around.port.places_held(), 0);
    });
}

/// The deleted actor's `every_unit_the_port_gives_is_observed_once_before_the_reordering`:
/// each staged unit is routed once: the access point's data to the network,
/// another BSS's data back, a management frame to the controller.
#[test]
fn every_staged_unit_is_routed_once() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        let mut foreign = data(2, IPV4, 2);
        foreign[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 0x30]);
        around.port.land(&data(1, IPV4, 1), meta());
        around.port.land(&foreign, meta());
        around.port.land(&beacon(3), meta());
        let _ = engine.received(&mut around.turn());
        assert_eq!(around.net.received, [Got::Lent(0)]);
        assert_eq!(around.told.frames, [(Some(0), beacon(3))]);
        assert_eq!(around.port.places_held(), 1, "the lent one");
        assert_eq!(around.port.counts().services, 1, "the ring serviced once");
        assert_eq!(refusals(&mut engine), RxRefusals::default());
    });
}

/// The deleted actor's `the_networks_lend_limit_holds_frames_until_its_bound_then_the_ring_is_walked`,
/// with the copy: above the lend limit a unit is copied while the network
/// has room to copy; with room for neither it waits, the ring with it, and
/// the episode runs from the first wait: a return does not end or extend
/// it, and at its bound what has no room is refused and the ring walked.
#[test]
fn above_the_lend_limit_a_unit_is_copied_then_waits_until_the_bound() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.net.lend_limit = 1;
        around.net.copy_room = 1;
        for tag in 1..=3 {
            around.port.land(&data(u16::from(tag), IPV4, tag), meta());
        }
        around.port.land(&beacon(9), meta());
        let start = around.micros + 1;
        let rest = engine.received(&mut around.turn());
        // 1 lent, 2 copied, 3 waits; the beacon, staged with them, is read.
        assert_eq!(
            around.net.received,
            [Got::Lent(0), Got::Copied(ethernet(IPV4, 2))]
        );
        assert_eq!(around.told.frames, [(Some(0), beacon(9))]);
        let bound = |start: u64| Instant::from_micros(start).checked_add(LATE).unwrap();
        assert_eq!(rest, Rest::Wake(Some(bound(start))));
        // The network consumed the copy: its room for one is back, 3 goes,
        // and the episode ends.
        around.net.copy_room = 1;
        assert_eq!(engine.returned(&mut around.turn()), Rest::Wake(None));
        assert_eq!(around.net.received[2], Got::Copied(ethernet(IPV4, 3)));
        // No room to copy now: 4 and 5 wait.
        around.port.land(&data(4, IPV4, 4), meta());
        around.port.land(&data(5, IPV4, 5), meta());
        let start = around.micros + 1;
        assert_eq!(
            engine.received(&mut around.turn()),
            Rest::Wake(Some(bound(start)))
        );
        // The network gives 1 back: 4 is lent, 5 still waits, and the
        // bound stays where the first wait put it.
        around.net.lent.clear();
        assert_eq!(
            engine.returned(&mut around.turn()),
            Rest::Wake(Some(bound(start)))
        );
        assert_eq!(around.net.received.len(), 4);
        // At the bound: 5 has no room and is refused; the ring is walked.
        around.pass(LATE.as_micros());
        assert_eq!(engine.due(&mut around.turn()), Rest::Wake(None));
        assert_eq!(refusals(&mut engine).pressure, 1);
        assert_eq!(around.port.places_held(), 1, "4, lent");
        assert_eq!(around.port.ring_len(), 0);
    });
}

/// The deleted actor's `msdu_refusals_of_one_frame_aggregate_by_reason_whatever_their_order`:
/// the refusals are counted by reason, each saturating.
#[test]
fn refusals_are_counted_by_reason() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        // Protected without the port's evidence.
        around.port.land(&protected(1, 1), meta());
        // No RFC 1042 header.
        let mut bare = data(2, IPV4, 2);
        bare[24] = 0;
        around.port.land(&bare, meta());
        let _ = engine.received(&mut around.turn());
        linked(&mut engine, |peer| {
            let _ = peer.hold(&mut around.turn());
        });
        around.port.land(&data(3, IPV4, 3), meta());
        around.port.land(&data(4, IPV4, 4), meta());
        let _ = engine.received(&mut around.turn());
        assert_eq!(
            refusals(&mut engine),
            RxRefusals {
                unverified: 1,
                undecapsulable: 1,
                link_held: 2,
                ..RxRefusals::default()
            }
        );
        assert!(around.net.received.is_empty());
        let mut saturated = RxRefusals {
            pressure: u16::MAX,
            ..RxRefusals::default()
        };
        saturated.count(RxRefusal::Pressure);
        assert_eq!(saturated.pressure, u16::MAX);
    });
}

/// A protected frame with the port's evidence goes to the network, its
/// CCMP header left before the Ethernet frame.
#[test]
fn verified_protected_data_is_delivered_after_its_ccmp_header() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.port.land(&protected(1, 5), verified());
        let _ = engine.received(&mut around.turn());
        assert_eq!(lent_frame(&around, 0), ethernet(IPV4, 5));
    });
}

/// On a protected link, unprotected data is refused but for EAPOL, which
/// the controller reads; protected data with the port's evidence goes to
/// the network.
#[test]
fn a_protected_link_takes_unprotected_eapol_alone() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Linked(peer) = data.peer(ACCESS_POINT) else {
                panic!("the access point is linked");
            };
            let _ = peer.unlink(&mut around.turn());
        }
        let mut link = station_link(2, HOUR);
        link.data.header = StaDataHeaderTemplate::new(
            OwnAddress::new(IndividualAddress::new(STATION).unwrap()),
            Bssid::new(IndividualAddress::new(ACCESS_POINT_ADDRESS).unwrap()).unwrap(),
            PeerQos::Qos,
            LinkProtection::Ccmp,
        )
        .unwrap();
        {
            let mut data = station_flowing(&mut engine);
            let PeerPlace::Free(free) = data.peer(ACCESS_POINT) else {
                panic!("the access point is free");
            };
            let _ = free.link(link, &mut around.turn());
        }
        let eapol = oer_protocols_ieee80211_wire::ethertype::ETHER_TYPE_EAPOL;
        around.port.land(&data(1, IPV4, 1), meta());
        around.port.land(&data(2, eapol, 2), meta());
        around.port.land(&protected(3, 3), verified());
        let _ = engine.received(&mut around.turn());
        assert_eq!(refusals(&mut engine).unverified, 1);
        assert_eq!(around.told.frames, [(Some(0), data(2, eapol, 2))]);
        assert_eq!(around.net.lent.len(), 1);
        assert_eq!(lent_frame(&around, 0), ethernet(IPV4, 3));
    });
}

/// An A-MSDU's MSDUs are each written in place and copied, in order.
#[test]
fn an_a_msdus_msdus_are_copied_each_in_order() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.port.land(&amsdu(&[1, 2, 3]), meta());
        let _ = engine.received(&mut around.turn());
        assert_eq!(
            around.net.received,
            [
                Got::Copied(ethernet(IPV4, 1)),
                Got::Copied(ethernet(IPV4, 2)),
                Got::Copied(ethernet(IPV4, 3)),
            ]
        );
        assert_eq!(around.port.places_held(), 0, "copied: back at once");
    });
}

/// After the data's stop nothing is delivered: what waited for the network
/// and what the ring stages go back unread, and the rights come home once
/// the engine's places are quiet; a lent unit is the machine's to wait for.
#[test]
fn a_stop_delivers_nothing_and_sends_back_what_waited() {
    running::<STATION_SINGLES, 0, _>(|run| {
        let mut engine = StationEngine::<'_, 8>::new(8, LATE);
        let mut around = station(run, &mut engine, 2, HOUR);
        around.net.lend_limit = 1;
        around.net.copy_room = 0;
        around.port.land(&data(1, IPV4, 1), meta());
        around.port.land(&data(2, IPV4, 2), meta());
        let _ = engine.received(&mut around.turn());
        assert_eq!(around.port.places_held(), 2, "1 lent, 2 waits");
        let _ = station_flowing(&mut engine).stop(&mut around.turn());
        assert_eq!(around.port.places_held(), 1, "2 went back");
        around.port.land(&data(3, IPV4, 3), meta());
        let _ = engine.received(&mut around.turn());
        assert_eq!(around.net.received, [Got::Lent(0)]);
        assert_eq!(around.port.ring_len(), 0);
        assert_eq!(around.port.places_held(), 1, "3 went back unread");
    });
}

/// An access point's client's data goes to the network; a management frame
/// of no linked client to the controller, of no peer.
#[test]
fn an_access_points_client_data_goes_to_the_network() {
    running::<AP_SINGLES, DATA_SINGLES, _>(|run| {
        let mut engine = ApEngine::<'_, 2, 8>::new(4, LATE);
        let mut around = access_point(run, &mut engine);
        let mut to_ds = data(1, IPV4, 6);
        to_ds[1] = 0x01;
        to_ds[4..10].copy_from_slice(&ACCESS_POINT_ADDRESS);
        to_ds[10..16].copy_from_slice(&CLIENT_B);
        to_ds[16..22].copy_from_slice(&SOURCE);
        around.port.land(&to_ds, meta());
        let mut request = beacon(2);
        request[0] = 0x00;
        request[4..10].copy_from_slice(&ACCESS_POINT_ADDRESS);
        request[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 0x40]);
        around.port.land(&request, meta());
        let _ = engine.received(&mut around.turn());
        assert_eq!(around.net.received, [Got::Lent(0)]);
        assert_eq!(around.told.frames, [(None, request)]);
    });
}
