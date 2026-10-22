use super::*;
use oer_ieee80211_wire::block_ack::frame::{
    ADDBA_STATUS_REQUEST_DECLINED, BlockAckAction, encode_addba_request, parse_block_ack_action,
};
use oer_ieee80211_wire::sequence::SequenceNumber;
use oer_time::Clock;
use oer_time_virtual::VirtualClock;

#[test]
fn unsupported_addba_receives_an_explicit_decline() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world.frames.clear();
        let clock = VirtualClock::<8>::starting_at(at(40));
        // The backend accepts TIDs 0..=7. A parsed request for TID 15 still
        // needs a response, without allocating a receive agreement.
        let body = encode_addba_request(17, 15, 16, 0, true, SequenceNumber::ZERO).unwrap();
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
            .filter_map(|frame| oer_ieee80211_wire::frame::DecodedHeader::decode(frame))
            .filter(|header| header.control().subtype() == 13)
            .filter_map(|header| parse_block_ack_action(header.body()))
            .collect();
        assert!(matches!(
            responses.as_slice(),
            [BlockAckAction::AddbaResponse {
                dialog_token: 17,
                status: ADDBA_STATUS_REQUEST_DECLINED,
                tid: 15,
                ..
            }]
        ));
        assert!(world.station.is_connected());
    });
}

#[test]
fn an_encoder_refusal_returns_the_reply_and_every_credit_without_marking_tx_busy() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let mut buffers = world.station.buffers.take_all();
        for buffer in &mut buffers {
            buffer.1 = 24;
        }
        world.station.buffers = crate::pool::Pool::from_collection(buffers);
        let reply = RxBlockAckReply::for_association(
            world.station.epoch.unwrap(),
            Bssid::new(IndividualAddress::new(BSSID).unwrap()).unwrap(),
            crate::block_ack::AddbaResponse {
                dialog_token: 17,
                status: crate::block_ack::AddbaStatus::RequestDeclined,
                tid: 0,
                window: 8,
                timeout_tu: 0,
                amsdu: false,
            },
        )
        .unwrap();
        let (reply, reason) = world.station.try_block_ack_reply(reply).err().unwrap();
        assert_eq!(reason, BlockAckReplyRefusal::Encode);
        assert!(world.station.is_connected());
        assert!(matches!(&world.station.state, State::Connected(connected) if !connected.sending));
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        let mut buffers = world.station.buffers.take_all();
        for buffer in &mut buffers {
            buffer.1 = 512;
        }
        world.station.buffers = crate::pool::Pool::from_collection(buffers);
        let Ok(StaAction::Transmit(transmit)) = world.station.try_block_ack_reply(reply) else {
            panic!("the same reply fits the restored control buffer");
        };
        world.transmitted(transmit, false);
        let clock = VirtualClock::<8>::starting_at(at(40));
        world.run(clock.now());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}
