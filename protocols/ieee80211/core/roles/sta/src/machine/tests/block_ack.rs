use super::*;
use oer_portable_clock::Clock;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_wire::block_ack::frame::{
    BlockAckAction, encode_addba_request, parse_block_ack_action,
};
use oer_protocols_ieee80211_wire::management::DialogToken;
use oer_protocols_ieee80211_wire::qos::Tid;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

#[test]
fn unsupported_addba_receives_an_explicit_decline() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world.frames.clear();
        let clock = VirtualClock::<8>::starting_at(at(40));
        // The backend accepts TIDs 0..=7. A parsed request for TID 15 still
        // needs a response, without allocating a receive agreement.
        let body = encode_addba_request(
            DialogToken::new(17),
            Tid::constant::<15>(),
            16,
            0,
            oer_protocols_ieee80211_wire::block_ack::frame::AmsduSupport::Supported,
            SequenceNumber::ZERO,
        )
        .unwrap();
        let mut request = std::vec![0; 24];
        request[0] = 0xd0;
        request[4..10].copy_from_slice(&LOCAL);
        request[10..16].copy_from_slice(&BSSID);
        request[16..22].copy_from_slice(&BSSID);
        request.extend_from_slice(&body);
        world.receive(clock.now(), &request);

        let responses: Vec<_> = world
            .frames
            .iter()
            .filter_map(|frame| oer_protocols_ieee80211_wire::frame::DecodedHeader::decode(frame))
            .filter(|header| header.control().subtype() == 13)
            .filter_map(|header| parse_block_ack_action(header.body()))
            .collect();
        assert!(matches!(
            responses.as_slice(),
            [BlockAckAction::AddbaResponse {
                dialog_token,
                status: oer_protocols_ieee80211_wire::status::StatusCode::REQUEST_DECLINED,
                parameters,
                ..
            }] if *dialog_token == DialogToken::new(17)
                && parameters.tid() == Tid::constant::<15>()
        ));
        assert!(world.station.is_connected());
    });
}

#[test]
fn an_encoder_refusal_returns_the_reply_without_marking_tx_busy() {
    installed(|mint| {
        let mut world = connected_world(mint);
        // The reply does not fit the station's buffer now.
        world.station.frame.limit = 24;
        let reply = RxBlockAckReply::for_association(
            world.station.epoch.unwrap(),
            Bssid::new(IndividualAddress::new(Eui48::new(BSSID)).unwrap()).unwrap(),
            oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaResponse {
                dialog_token: DialogToken::new(17),
                status:
                    oer_protocols_ieee80211_core_upper_mac::block_ack::AddbaStatus::RequestDeclined,
                tid: Tid::constant::<0>(),
                window: 8,
                timeout_tu: 0,
                amsdu: false,
            },
        )
        .unwrap();
        let (reply, reason) = world.station.try_block_ack_reply(reply).err().unwrap();
        assert_eq!(reason, BlockAckReplyRefusal::Encode);
        assert!(world.station.is_connected());
        assert!(
            matches!(&world.station.state, State::Connected(connected) if connected.transmission == crate::machine::Transmission::Free)
        );
        world.station.frame.limit = STA_FRAME_CAPACITY;
        let Ok(StaAction::Transmit(transmit)) = world.station.try_block_ack_reply(reply) else {
            panic!("the same reply fits the restored buffer");
        };
        transmitted(record!(world), transmit, Sealing::Open);
        let clock = VirtualClock::<8>::starting_at(at(40));
        world.run(clock.now());
    });
}

/// The DELBA the station sends to end its own transmit agreement names it
/// the originator, with the TID and the timeout's reason.
#[test]
fn the_station_s_own_delba_names_it_the_agreement_s_originator() {
    for tid in WmmUserPriority::ALL {
        let body = super::super::own_delba(tid);
        assert_eq!(
            parse_block_ack_action(&body),
            Some(BlockAckAction::Delba {
                tid: tid.tid(),
                initiator:
                    oer_protocols_ieee80211_wire::block_ack::frame::DelbaInitiator::Originator,
                reason: oer_protocols_ieee80211_wire::status::ReasonCode::TIMEOUT,
            })
        );
    }
}

/// What the transmit agreements owe after a dropped frame goes out as a
/// BlockAckReq at the plane's head.
#[test]
fn an_owed_block_ack_request_is_transmitted() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let tx = crate::machine::tx_block_ack::tests::operational();
        world.station.config.tx_block_ack = Some(tx);
        let State::Connected(connected) = &mut world.station.state else {
            panic!("the world is connected");
        };
        connected.tx_block_ack = crate::machine::TxAgreements::Started;
        world
            .station
            .tx_block_ack_request(WmmUserPriority::UP0, SequenceNumber::new(9).unwrap());
        world.take();
        world.run(at(40));
        assert!(world.seen.iter().any(|seen| matches!(
            seen,
            Seen::Transmit {
                subtype: 0x0084,
                ..
            }
        )));
    });
}

/// The station's agreement as the link's holder hears it: negotiated at the
/// number the holder told, and its window once it is operational.
#[test]
fn the_links_holder_numbers_a_negotiation_and_hears_its_agreement() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let started = |world: &mut World<'_>, tx| {
            world.station.config.tx_block_ack = Some(tx);
            let State::Connected(connected) = &mut world.station.state else {
                panic!("the world is connected");
            };
            connected.tx_block_ack = crate::machine::TxAgreements::Started;
            world.link_actions.clear();
            world.frames.clear();
            world.run(at(40));
        };
        world.switches.on(Switch::HoldTransmissions);
        started(
            &mut world,
            crate::machine::tx_block_ack::tests::negotiating(WmmUserPriority::UP5),
        );
        assert!(world.link_actions.contains(&LinkSeen::Sequence(5)));
        let requests: Vec<_> = world
            .frames
            .iter()
            .filter_map(|frame| oer_protocols_ieee80211_wire::frame::DecodedHeader::decode(frame))
            .filter_map(|header| parse_block_ack_action(header.body()))
            .collect();
        assert!(matches!(
            requests.as_slice(),
            [BlockAckAction::AddbaRequest { parameters, .. }]
                if parameters.tid() == WmmUserPriority::UP5.tid()
        ));
        assert_eq!(
            world.submitted.last().unwrap().names_sequence(),
            Some(WmmUserPriority::UP5)
        );
        started(
            &mut world,
            crate::machine::tx_block_ack::tests::operational(),
        );
        assert!(
            world.link_actions.iter().any(
                |seen| matches!(seen, LinkSeen::Agreement(0, Some(window)) if window.size == 32)
            )
        );
    });
}
