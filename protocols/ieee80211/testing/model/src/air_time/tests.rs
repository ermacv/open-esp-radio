//! The air rules, each against a number worked by hand from the standard or
//! against the production code that states the same rule.

use super::*;
use alloc::rc::Rc;
use alloc::vec;
use core::cell::{Cell, RefCell};
use oer_protocols_ieee80211_contracts_port::{
    AmpduCapabilities, AmpduLengthTable, Channel, ChannelWidth, PhyFormatSet,
};
use oer_protocols_ieee80211_core_upper_mac::aggregate::{AmpduLimits, subframe_bound};
use oer_protocols_ieee80211_wire::extensions::wmm::WmmParameterSet;
use oer_protocols_ieee80211_wire::phy::{HtMcs, HtRate, PpduBandwidth};

const STAND_MPDU: u32 = 1554;

fn stand_rate() -> PhyRate {
    PhyRate::Ht(HtRate::new(HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, true).unwrap())
}

fn best_effort() -> WmmAcParameters {
    WmmParameterSet::STANDARD_DEFAULT.access_category(WmmAccessCategory::BestEffort)
}

fn tid() -> Tid {
    Tid::new(0).unwrap()
}

fn meta() -> ModelRxMeta {
    ModelRxMeta::new(
        Channel::ghz2_4(9, ChannelWidth::Mhz40Below).unwrap(),
        -40,
        -95,
    )
}

/// An air the medium runs on: `ring` slots a test frees by hand.
#[derive(Default)]
struct World {
    agreed: Cell<Option<u16>>,
    ring: Cell<usize>,
    landed: RefCell<Vec<Vec<u8>>>,
    /// Sequence Numbers whose next landing finds no slot.
    refused: RefCell<Vec<u16>>,
}

impl AirWorld for World {
    fn agreed_window(&self, _: Tid) -> Option<u16> {
        self.agreed.get()
    }

    fn land(&self, frame: Vec<u8>, _: ModelRxMeta) -> bool {
        let mut refused = self.refused.borrow_mut();
        if let Some(at) = refused
            .iter()
            .position(|refused| *refused == sequence_of(&frame))
        {
            refused.remove(at);
            return false;
        }
        if self.ring.get() == 0 {
            return false;
        }
        self.ring.set(self.ring.get() - 1);
        self.landed.borrow_mut().push(frame);
        true
    }
}

/// A world whose ring has room for everything.
fn roomy() -> World {
    World {
        ring: Cell::new(usize::MAX),
        ..World::default()
    }
}

/// A frame whose image starts with its Sequence Number.
fn frames(octets: usize) -> Box<dyn FnMut(SequenceNumber) -> Vec<u8>> {
    Box::new(move |sequence| {
        let mut frame = vec![0_u8; octets];
        frame[..2].copy_from_slice(&sequence.get().to_le_bytes());
        frame
    })
}

fn sequence_of(frame: &[u8]) -> u16 {
    u16::from_le_bytes([frame[0], frame[1]])
}

fn peer(window: PeerWindow, offered: Offered) -> AirPeer {
    AirPeer::best_effort(
        frames(STAND_MPDU as usize - 4),
        meta(),
        stand_rate(),
        offered,
        window,
        tid(),
    )
}

fn medium(seed: u64, peer: Option<AirPeer>) -> Medium {
    Medium::new(0, seed, AirTiming::HT_2_4_GHZ, None, peer)
}

/// Ask for one 1000-octet exchange of the station at `at`.
fn request(medium: &mut Medium, at: u64) -> AirTicket {
    medium
        .request(
            at,
            stand_rate(),
            1000,
            Protection::None,
            AirResponse::BlockAck,
            best_effort(),
            1,
        )
        .unwrap()
}

/// Keep the station saturated until `end`; its tickets.
fn saturate(medium: &mut Medium, world: &World, end: u64) -> u64 {
    let mut count = 0;
    let mut ticket = request(medium, 0);
    let mut last = None;
    while let Some(at) = medium.next_event() {
        if at > end {
            break;
        }
        assert_ne!(Some(at), last.replace(at), "the channel stands still");
        medium.advance(at, world).unwrap();
        if medium.progress(ticket) != AirProgress::Pending {
            medium.forget(ticket);
            count += 1;
            ticket = request(medium, at);
        }
    }
    count
}

#[test]
fn an_aggregate_of_the_stand_mode_takes_what_the_standard_works_out() {
    // 31 subframes of delimiter, MPDU and padding to four, then the last
    // without padding: 31 * 1560 + 1558 octets.
    let psdu = ampdu_psdu_octets([STAND_MPDU; 32], stand_rate(), 0).unwrap();
    assert_eq!(psdu, 49_918);
    // HT40 MCS 7: 540 data bits per symbol. 16 + 399344 + 6 bits take 740
    // symbols of 3.6 us, 2664 us, on a 4-us grid; 36 us of preamble and 6 us
    // of signal extension.
    let exchange = AirExchange::new(
        AirTiming::HT_2_4_GHZ,
        stand_rate(),
        psdu,
        Protection::None,
        AirResponse::BlockAck,
    )
    .unwrap();
    assert_eq!(exchange.ppdu_micros, 2706);
    // SIFS, then 32 octets at 24 Mb/s: 96 bits per symbol, 3 symbols, 20 us
    // of preamble and 6 us of signal extension.
    assert_eq!(exchange.response_micros, 10 + 38);
    assert_eq!(exchange.occupancy_micros(), Ok(2754));
}

#[test]
fn a_response_and_a_protection_exchange_take_their_frames_and_sifs() {
    let timing = AirTiming::HT_2_4_GHZ;
    let of = |protection, response| {
        AirExchange::new(timing, stand_rate(), 1000, protection, response).unwrap()
    };
    let ppdu = of(Protection::None, AirResponse::None).ppdu_micros;
    // 14 octets at 24 Mb/s: 2 symbols.
    assert_eq!(
        of(Protection::None, AirResponse::Ack).response_micros,
        10 + 34
    );
    assert_eq!(of(Protection::None, AirResponse::None).response_micros, 0);
    // RTS, 20 octets, 2 symbols; CTS, 14 octets, 2 symbols, at 24 Mb/s.
    let rate = LegacyRate::Ofdm24M;
    let rts = of(Protection::RtsCts { rate }, AirResponse::BlockAck);
    assert_eq!(rts.protection_micros, 34 + 10 + 34 + 10);
    assert_eq!(rts.occupancy_micros(), Ok(88 + ppdu + 48));
    // At 6 Mb/s: 24 bits per symbol, 8 and 6 symbols.
    let slow = of(
        Protection::RtsCts {
            rate: LegacyRate::Ofdm6M,
        },
        AirResponse::None,
    );
    assert_eq!(slow.protection_micros, 58 + 10 + 50 + 10);
    let cts = of(Protection::CtsToSelf { rate }, AirResponse::None);
    assert_eq!(cts.protection_micros, 34 + 10);
}

#[test]
fn a_collision_costs_an_rts_exchange_its_rts_and_any_other_frame_its_ppdu() {
    let timing = AirTiming::HT_2_4_GHZ;
    let rate = LegacyRate::Ofdm24M;
    let of = |protection| {
        AirExchange::new(timing, stand_rate(), 1000, protection, AirResponse::Ack).unwrap()
    };
    assert_eq!(of(Protection::RtsCts { rate }).lost_micros, 34 + 10 + 34);
    let plain = of(Protection::None);
    assert_eq!(plain.lost_micros, plain.ppdu_micros);
    let cts = of(Protection::CtsToSelf { rate });
    assert_eq!(cts.lost_micros, 44 + plain.ppdu_micros);
}

#[test]
fn start_spacing_pads_between_subframes_not_after_the_last() {
    // 16 us at 150 Mb/s is 300 octets between starts: 104-octet subframes
    // take 49 empty delimiters each, the last none.
    let psdu = ampdu_psdu_octets([100, 100], stand_rate(), 7).unwrap();
    let spaced = 4 + 100 + 49 * 4 + 4 + 100;
    assert_eq!(psdu, spaced);
    assert_eq!(ampdu_psdu_octets([100, 100], stand_rate(), 0).unwrap(), 208);
    assert_eq!(
        ampdu_psdu_octets(core::iter::empty(), stand_rate(), 0),
        Err(AirTimeError::EmptyFrame)
    );
    assert_eq!(
        ampdu_psdu_octets([0], stand_rate(), 0),
        Err(AirTimeError::EmptyFrame)
    );
    assert_eq!(
        ampdu_psdu_octets([100], PhyRate::Legacy(LegacyRate::Ofdm54M), 0),
        Err(AirTimeError::Unrepresentable)
    );
}

#[test]
fn access_follows_each_categorys_parameters() {
    let timing = AirTiming::HT_2_4_GHZ;
    let of = |category| {
        AirAccess::of(
            timing,
            WmmParameterSet::STANDARD_DEFAULT.access_category(category),
        )
    };
    let best = of(WmmAccessCategory::BestEffort);
    assert_eq!(
        (best.aifs_micros, best.cw_min, best.cw_max),
        (10 + 3 * 9, 15, 1023)
    );
    let voice = of(WmmAccessCategory::Voice);
    assert_eq!(
        (voice.aifs_micros, voice.cw_min, voice.cw_max),
        (10 + 2 * 9, 3, 7)
    );
}

#[test]
fn backoff_slots_run_out_only_while_the_channel_is_idle() {
    let access = AirAccess::of(AirTiming::HT_2_4_GHZ, best_effort());
    let mut side = Contender::new(access);
    side.slots = 10;
    side.resume(100);
    assert_eq!(side.from(), 137);
    assert_eq!(side.expiry(9), 137 + 90);
    // Three whole slots passed: 28 us after AIFS.
    side.freeze(137 + 28, 9);
    assert_eq!(side.slots, 7);
    // A transmission during AIFS takes no slot.
    side.resume(1000);
    side.freeze(1020, 9);
    assert_eq!(side.slots, 7);
    // Nor do more slots pass than remain.
    side.freeze(u64::MAX, 9);
    assert_eq!(side.slots, 0);
}

#[test]
fn the_window_doubles_on_a_collision_up_to_its_maximum_and_resets_on_success() {
    let access = AirAccess {
        aifs_micros: 37,
        cw_min: 15,
        cw_max: 63,
    };
    let mut rng = Seeded(1);
    let mut side = Contender::new(access);
    let mut windows = vec![];
    for _ in 0..4 {
        side.finished(0, true, &mut rng);
        windows.push(side.cw);
    }
    assert_eq!(windows, [31, 63, 63, 63]);
    side.finished(0, false, &mut rng);
    assert_eq!(side.cw, 15);
}

#[test]
fn a_frame_on_an_idle_channel_goes_at_once_and_one_on_a_busy_channel_backs_off() {
    let mut rng = Seeded(1);
    let access = AirAccess {
        aifs_micros: 37,
        cw_min: 1023,
        cw_max: 1023,
    };
    let mut idle = Contender::new(access);
    idle.resume(0);
    idle.arrived(1000, false, &mut rng);
    assert_eq!(idle.slots, 0, "an idle AIFS needs no backoff");
    let mut busy = Contender::new(access);
    busy.arrived(1000, true, &mut rng);
    assert_ne!(busy.slots, 0);
    let mut settling = Contender::new(access);
    settling.resume(1000);
    settling.arrived(1000, false, &mut rng);
    assert_ne!(settling.slots, 0, "AIFS has not passed yet");
    // A backoff drawn after its own transmission stands, even at zero.
    let mut own = Contender::new(access);
    own.finished(1000, false, &mut rng);
    own.slots = 0;
    own.arrived(1000, false, &mut rng);
    assert_eq!(own.slots, 0);
}

#[test]
fn a_saturated_side_waits_aifs_and_its_backoff_between_exchanges() {
    let world = roomy();
    let mut medium = medium(7, None);
    assert!(saturate(&mut medium, &world, 10_000_000) > 1000);
    let spans = medium.spans();
    let mut slots = 0_u64;
    let gaps = spans.len() as u64 - 1;
    for pair in spans.windows(2) {
        let gap = pair[1].start_micros - pair[0].end_micros;
        assert!(gap >= 37, "{gap}");
        assert_eq!((gap - 37) % 9, 0, "{gap}");
        let drawn = (gap - 37) / 9;
        assert!(drawn <= 15, "{drawn}");
        slots += drawn;
    }
    let mean = slots as f64 / gaps as f64;
    assert!((7.0..=8.0).contains(&mean), "mean backoff {mean}");
}

#[test]
fn the_first_request_on_a_quiet_channel_starts_at_its_instant_and_ends_with_its_exchange() {
    let world = roomy();
    let mut medium = medium(1, None);
    let ticket = request(&mut medium, 1000);
    assert_eq!(medium.next_event(), Some(1000));
    medium.advance(1000, &world).unwrap();
    assert_eq!(medium.progress(ticket), AirProgress::Pending);
    let exchange = AirExchange::new(
        AirTiming::HT_2_4_GHZ,
        stand_rate(),
        1000,
        Protection::None,
        AirResponse::BlockAck,
    )
    .unwrap();
    let end = 1000 + u64::from(exchange.occupancy_micros().unwrap());
    assert_eq!(medium.next_event(), Some(end));
    medium.advance(end - 1, &world).unwrap();
    assert_eq!(medium.progress(ticket), AirProgress::Pending);
    medium.advance(end, &world).unwrap();
    assert_eq!(medium.progress(ticket), AirProgress::Answered);
    let [span] = medium.spans() else {
        panic!("one transmission");
    };
    assert_eq!(
        (span.holder, span.start_micros, span.end_micros),
        (AirHolder::Station, 1000, end)
    );
}

#[test]
fn the_head_request_contends_by_its_own_access_category() {
    let voice = WmmParameterSet::STANDARD_DEFAULT.access_category(WmmAccessCategory::Voice);
    let gaps: Vec<u64> = (0..64)
        .map(|seed| {
            let world = roomy();
            let mut medium = medium(seed, None);
            let first = request(&mut medium, 0);
            medium.advance(10_000, &world).unwrap();
            medium.forget(first);
            let end = medium.spans()[0].end_micros;
            medium
                .request(
                    end,
                    stand_rate(),
                    1000,
                    Protection::None,
                    AirResponse::BlockAck,
                    voice,
                    1,
                )
                .unwrap();
            medium.advance(20_000, &world).unwrap();
            medium.spans()[1].start_micros - end
        })
        .collect();
    // AIFSN 2 after SIFS, and the backoff the first exchange left.
    assert!(gaps.contains(&28), "{gaps:?}");
    assert!(gaps.iter().all(|gap| (*gap - 28) % 9 == 0), "{gaps:?}");
}

#[test]
fn a_later_request_waits_for_the_one_on_the_channel() {
    let world = roomy();
    let mut medium = medium(1, None);
    let first = request(&mut medium, 0);
    let second = request(&mut medium, 0);
    medium.advance(10_000, &world).unwrap();
    assert_eq!(medium.progress(first), AirProgress::Answered);
    assert_eq!(medium.progress(second), AirProgress::Answered);
    let [one, two] = medium.spans() else {
        panic!("two transmissions");
    };
    assert!(two.start_micros >= one.end_micros + 37);
}

#[test]
fn a_forgotten_request_leaves_the_channel_and_one_on_it_keeps_its_time() {
    let world = roomy();
    let mut medium = medium(1, None);
    let on_air = request(&mut medium, 0);
    let waiting = request(&mut medium, 0);
    medium.advance(0, &world).unwrap();
    medium.forget(waiting);
    medium.forget(on_air);
    medium.advance(10_000, &world).unwrap();
    assert_eq!(medium.spans().len(), 1);
}

#[test]
fn two_sides_on_one_instant_collide_nobody_hears_them_and_both_double_their_windows() {
    let world = roomy();
    world.agreed.set(Some(16));
    let mut medium = medium(1, Some(peer(PeerWindow::Agreed, Offered::Saturated)));
    let ticket = request(&mut medium, 0);
    assert_eq!(medium.next_event(), Some(0));
    medium.advance(0, &world).unwrap();
    let end = medium.next_event().unwrap();
    medium.advance(end, &world).unwrap();
    assert_eq!(medium.progress(ticket), AirProgress::Collided);
    assert!(world.landed.borrow().is_empty(), "nothing landed");
    assert_eq!(medium.station.cw, 31);
    assert_eq!(medium.peer.as_ref().unwrap().contender.cw, 31);
    assert_eq!(
        medium.peer.as_ref().unwrap().unacked.len(),
        16,
        "every frame goes again"
    );
    let report = medium.report();
    assert_eq!((report.station.collisions, report.peer.collisions), (1, 1));
    let [span] = medium.spans() else {
        panic!("one span for both");
    };
    assert_eq!(span.holder, AirHolder::Collision);
    let station = AirExchange::new(
        AirTiming::HT_2_4_GHZ,
        stand_rate(),
        1000,
        Protection::None,
        AirResponse::BlockAck,
    )
    .unwrap();
    assert!(span.end_micros - span.start_micros > u64::from(station.ppdu_micros));
}

#[test]
fn one_transmission_at_a_time_whoever_contends() {
    let world = roomy();
    world.agreed.set(Some(16));
    let mut medium = Medium::new(
        0,
        3,
        AirTiming::HT_2_4_GHZ,
        Some(BusyChannel {
            period_micros: NonZeroU32::new(5000).unwrap(),
            busy_micros: 400,
        }),
        Some(peer(PeerWindow::Agreed, Offered::Saturated)),
    );
    saturate(&mut medium, &world, 500_000);
    let spans = medium.spans();
    assert!(spans.len() > 100);
    assert!(spans.iter().any(|span| span.holder == AirHolder::Station));
    assert!(
        spans
            .iter()
            .any(|span| span.holder == AirHolder::AccessPoint)
    );
    assert!(spans.iter().any(|span| span.holder == AirHolder::Busy));
    for pair in spans.windows(2) {
        assert!(pair[0].end_micros <= pair[1].start_micros, "{pair:?}");
    }
    // The other side's transmission freezes the waiting one: neither gets
    // the channel twice as often as the other.
    let share = |holder| spans.iter().filter(|span| span.holder == holder).count() as f64;
    let ratio = share(AirHolder::Station) / share(AirHolder::AccessPoint);
    assert!((0.7..=1.4).contains(&ratio), "{ratio}");
}

#[test]
fn a_burst_defers_to_a_transmission_and_stops_the_backoff_through_it() {
    let world = roomy();
    let mut medium = Medium::new(
        0,
        5,
        AirTiming::HT_2_4_GHZ,
        Some(BusyChannel {
            period_micros: NonZeroU32::new(1000).unwrap(),
            busy_micros: 300,
        }),
        None,
    );
    // A long exchange starts just after the first burst.
    medium.advance(0, &world).unwrap();
    medium.advance(300, &world).unwrap();
    let ticket = medium
        .request(
            301,
            PhyRate::Legacy(LegacyRate::Ofdm6M),
            1000,
            Protection::None,
            AirResponse::None,
            best_effort(),
            1,
        )
        .unwrap();
    assert_ne!(medium.progress(ticket), AirProgress::Answered);
    medium.advance(10_000, &world).unwrap();
    let spans = medium.spans();
    for pair in spans.windows(2) {
        assert!(pair[0].end_micros <= pair[1].start_micros, "{pair:?}");
    }
    let station = spans
        .iter()
        .position(|span| span.holder == AirHolder::Station)
        .unwrap();
    // The burst due during the exchange starts as soon as it ends.
    assert_eq!(spans[station + 1].holder, AirHolder::Busy);
    assert_eq!(spans[station + 1].start_micros, spans[station].end_micros);
    let busy = medium.report().busy_micros;
    assert_eq!(busy, 300 * 11, "every period's burst came, the first at 0");
}

#[test]
fn a_saturated_channel_gives_way_to_bursts_in_proportion() {
    let quiet = {
        let world = roomy();
        let mut medium = medium(9, None);
        saturate(&mut medium, &world, 1_000_000)
    };
    let world = roomy();
    let mut busy = Medium::new(
        0,
        9,
        AirTiming::HT_2_4_GHZ,
        Some(BusyChannel {
            period_micros: NonZeroU32::new(1000).unwrap(),
            busy_micros: 250,
        }),
        None,
    );
    let served = saturate(&mut busy, &world, 1_000_000);
    let share = served as f64 / quiet as f64;
    assert!((0.6..=0.8).contains(&share), "{share}");
    let report = busy.report().busy_micros;
    assert!(report >= 240_000, "{report}");
}

#[test]
fn frames_land_while_the_ppdu_is_on_the_air_and_the_last_at_its_end() {
    let world = roomy();
    world.agreed.set(Some(4));
    let mut medium = medium(1, Some(peer(PeerWindow::Agreed, Offered::Saturated)));
    medium.advance(0, &world).unwrap();
    assert!(world.landed.borrow().is_empty());
    let mut instants = vec![];
    while world.landed.borrow().len() < 4 {
        let at = medium.next_event().unwrap();
        medium.advance(at, &world).unwrap();
        instants.push((at, world.landed.borrow().len()));
    }
    let landed: Vec<usize> = instants.iter().map(|(_, count)| *count).collect();
    assert_eq!(landed, [1, 2, 3, 4], "one at a time");
    assert!(instants.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let psdu = ampdu_psdu_octets([STAND_MPDU; 4], stand_rate(), 0).unwrap();
    let ppdu = stand_rate().max_ppdu_duration_micros(psdu).unwrap();
    assert_eq!(instants[3].0, u64::from(ppdu), "the last at the PPDU's end");
    let sequences: Vec<u16> = world
        .landed
        .borrow()
        .iter()
        .map(|frame| sequence_of(frame))
        .collect();
    assert_eq!(sequences, [0, 1, 2, 3]);
}

#[test]
fn a_frame_the_ring_has_no_slot_for_is_not_acknowledged_and_goes_again_first() {
    let world = World {
        ring: Cell::new(3),
        ..World::default()
    };
    world.agreed.set(Some(8));
    let mut medium = medium(1, Some(peer(PeerWindow::Agreed, Offered::Saturated)));
    medium.advance(0, &world).unwrap();
    while medium.report().ring_drops < 5 {
        let at = medium.next_event().unwrap();
        medium.advance(at, &world).unwrap();
    }
    let end = medium.on_air.as_ref().unwrap().end;
    medium.advance(end, &world).unwrap();
    let report = medium.report();
    assert_eq!((report.landed, report.ring_drops), (3, 5));
    let unacked: Vec<u16> = medium
        .peer
        .as_ref()
        .unwrap()
        .unacked
        .iter()
        .map(|sequence| sequence.get())
        .collect();
    assert_eq!(unacked, [3, 4, 5, 6, 7]);
    // The next aggregate starts with them, and the window has moved three.
    world.ring.set(usize::MAX);
    world.landed.borrow_mut().clear();
    while world.landed.borrow().len() < 8 {
        let at = medium.next_event().unwrap();
        medium.advance(at, &world).unwrap();
    }
    let sequences: Vec<u16> = world
        .landed
        .borrow()
        .iter()
        .map(|frame| sequence_of(frame))
        .collect();
    assert_eq!(sequences, [3, 4, 5, 6, 7, 8, 9, 10]);
}

#[test]
fn a_hole_at_the_head_holds_the_window_however_much_is_offered() {
    let world = World {
        ring: Cell::new(0),
        ..World::default()
    };
    let mut medium = medium(1, Some(peer(PeerWindow::Assumed(4), Offered::Saturated)));
    for _ in 0..5 {
        let before = medium.report().peer.exchanges;
        while medium.report().peer.exchanges == before {
            let at = medium.next_event().unwrap();
            medium.advance(at, &world).unwrap();
        }
        let end = medium.on_air.as_ref().unwrap().end;
        medium.advance(end, &world).unwrap();
    }
    let peer = medium.peer.as_ref().unwrap();
    assert_eq!(peer.unacked.len(), 4);
    assert_eq!(peer.next_sequence.get(), 4, "nothing beyond the window");
}

#[test]
fn a_lost_head_holds_the_window_while_the_frames_after_it_landed() {
    let world = roomy();
    world.refused.borrow_mut().push(0);
    let mut medium = medium(1, Some(peer(PeerWindow::Assumed(8), Offered::Saturated)));
    medium.advance(0, &world).unwrap();
    let end = medium.on_air.as_ref().unwrap().end;
    medium.advance(end, &world).unwrap();
    assert_eq!(world.landed.borrow().len(), 7);
    // The next aggregate is the hole alone: 8 would lie past the window.
    let before = medium.report().peer.subframes;
    while medium.report().peer.exchanges < 2 {
        let at = medium.next_event().unwrap();
        medium.advance(at, &world).unwrap();
    }
    assert_eq!(medium.report().peer.subframes - before, 1);
    assert_eq!(medium.report().bounds.window, 2);
}

#[test]
fn the_peers_frames_carry_their_overhead_on_the_air() {
    let world = roomy();
    let mut air_peer = peer(PeerWindow::Agreed, Offered::Saturated);
    air_peer.frame = frames(100);
    air_peer.air_overhead_octets = 400;
    let mut medium = medium(1, Some(air_peer));
    medium.advance(0, &world).unwrap();
    let ppdu = stand_rate().max_ppdu_duration_micros(500).unwrap();
    assert_eq!(medium.on_air.as_ref().unwrap().end, u64::from(ppdu + 44));
}

#[test]
fn a_burst_takes_the_slots_that_passed_before_it_and_no_more() {
    let world = roomy();
    let mut medium = Medium::new(
        0,
        1,
        AirTiming::HT_2_4_GHZ,
        Some(BusyChannel {
            period_micros: NonZeroU32::new(100_000).unwrap(),
            busy_micros: 300,
        }),
        None,
    );
    // The first burst is due at once; move it into the backoff.
    medium.bursts.as_mut().unwrap().next = 37 + 3 * 9;
    medium.station.slots = 10;
    medium.station.idle = Some(0);
    medium.station.posted = true;
    let ticket = request(&mut medium, 0);
    medium.advance(10_000, &world).unwrap();
    assert_eq!(medium.progress(ticket), AirProgress::Answered);
    let station = medium
        .spans()
        .iter()
        .find(|span| span.holder == AirHolder::Station)
        .unwrap();
    // Seven slots left after the burst's end and AIFS.
    assert_eq!(station.start_micros, 64 + 300 + 37 + 7 * 9);
}

#[test]
fn the_window_wraps_with_the_sequence_space() {
    let world = roomy();
    let mut medium = medium(1, Some(peer(PeerWindow::Assumed(32), Offered::Saturated)));
    if let Some(peer) = medium.peer.as_mut() {
        peer.next_sequence = SequenceNumber::new(4090).unwrap();
    }
    medium.advance(50_000, &world).unwrap();
    let subframes = medium.report().peer.subframes;
    let exchanges = medium.report().peer.exchanges;
    assert_eq!(subframes, 32 * exchanges);
    let peer = medium.peer.as_ref().unwrap();
    assert!(peer.next_sequence.get() < 4090);
}

#[test]
fn the_peers_aggregate_stops_where_production_stops_it() {
    // AmpduLimits is the rule of the station's own aggregator. The response
    // it reserves is the BlockAck at 6 Mb/s.
    static LENGTHS: AmpduLengthTable = AmpduLengthTable::uniform(1_000_000);
    let timing = AirTiming {
        response_rate: LegacyRate::Ofdm6M,
        ..AirTiming::HT_2_4_GHZ
    };
    let ethernet = 1514_usize;
    let mpdu = subframe_bound(ethernet).unwrap() - 4;
    for (window, parameters, txop_units, spacing) in [
        (16, 3_u8, 0_u16, 0_u8),
        (64, 3, 0, 0),
        (64, 3, 47, 0),
        (64, 3, 94, 0),
        (64, 1, 0, 0),
        (64, 0, 0, 0),
        (64, 3, 47, 7),
        (32, 3, 16, 5),
    ] {
        let peer_parameters = parameters | (spacing << 2);
        let limits = AmpduLimits {
            window,
            port: AmpduCapabilities {
                max_subframes: 64,
                formats: PhyFormatSet::HT,
                max_length: &LENGTHS,
            },
            peer_ampdu_parameters: peer_parameters,
            txop_limit_micros: (txop_units != 0).then(|| u32::from(txop_units) * TXOP_UNIT_MICROS),
            rate: stand_rate(),
        };
        let expected = limits.run(core::iter::repeat_n(ethernet, 64));
        let world = roomy();
        let mut edca = best_effort();
        edca.txop_limit_units_32_us = txop_units;
        let mut air_peer = peer(PeerWindow::Assumed(window), Offered::Saturated);
        air_peer.frame = frames(mpdu as usize);
        air_peer.air_overhead_octets = 0;
        air_peer.edca = edca;
        air_peer.spacing = spacing;
        air_peer.max_length = limits.peer_max_length();
        let mut medium = Medium::new(0, 1, timing, None, Some(air_peer));
        medium.advance(0, &world).unwrap();
        assert_eq!(
            medium.report().peer.subframes,
            expected as u64,
            "window {window}, length exponent {parameters}, txop {txop_units}, spacing {spacing}"
        );
    }
}

#[test]
fn each_aggregate_names_the_limit_that_bound_it() {
    let bound = |window, offered, configure: &dyn Fn(&mut AirPeer)| {
        let world = roomy();
        let mut air_peer = peer(window, offered);
        configure(&mut air_peer);
        let mut medium = medium(1, Some(air_peer));
        medium.advance(0, &world).unwrap();
        medium.report().bounds
    };
    let saturated = Offered::Saturated;
    assert_eq!(
        bound(PeerWindow::Assumed(16), saturated, &|_| {}),
        PeerBounds {
            window: 1,
            ..PeerBounds::default()
        }
    );
    assert_eq!(
        bound(PeerWindow::Assumed(64), saturated, &|_| {}),
        PeerBounds {
            length: 1,
            ..PeerBounds::default()
        }
    );
    assert_eq!(
        bound(PeerWindow::Assumed(64), saturated, &|peer| {
            peer.max_subframes = 8;
        }),
        PeerBounds {
            subframes: 1,
            ..PeerBounds::default()
        }
    );
    assert_eq!(
        bound(PeerWindow::Assumed(64), saturated, &|peer| {
            peer.edca.txop_limit_units_32_us = 47;
        }),
        PeerBounds {
            txop: 1,
            ..PeerBounds::default()
        }
    );
    assert_eq!(
        bound(PeerWindow::Assumed(64), saturated, &|peer| {
            peer.max_length = 1 << 20;
            peer.rate = PhyRate::Ht(
                HtRate::new(HtMcs::new(0).unwrap(), PpduBandwidth::Mhz20, false).unwrap(),
            );
        }),
        PeerBounds {
            ppdu_time: 1,
            ..PeerBounds::default()
        }
    );
}

#[test]
fn without_a_window_the_access_point_sends_single_mpdus_acknowledged_one_by_one() {
    let world = roomy();
    let mut medium = medium(1, Some(peer(PeerWindow::Agreed, Offered::Saturated)));
    medium.advance(50_000, &world).unwrap();
    let report = medium.report();
    assert!(report.peer.exchanges > 2);
    assert_eq!(report.peer.exchanges, report.peer.subframes);
    assert_eq!(report.bounds.window, report.peer.exchanges);
    let [first, second, ..] = medium.spans() else {
        panic!("two exchanges");
    };
    let ppdu = stand_rate().max_ppdu_duration_micros(STAND_MPDU).unwrap();
    assert_eq!(
        second.start_micros.min(first.end_micros) - first.start_micros,
        u64::from(ppdu + 44)
    );
}

#[test]
fn a_rate_without_aggregates_sends_single_mpdus_whatever_the_window() {
    let world = roomy();
    let mut air_peer = peer(PeerWindow::Assumed(16), Offered::Saturated);
    air_peer.rate = PhyRate::Legacy(LegacyRate::Ofdm54M);
    let mut medium = medium(1, Some(air_peer));
    medium.advance(50_000, &world).unwrap();
    let report = medium.report();
    assert!(report.peer.exchanges > 2);
    assert_eq!(report.peer.exchanges, report.peer.subframes);
    let [first, ..] = medium.spans() else {
        panic!("an exchange");
    };
    let ppdu = PhyRate::Legacy(LegacyRate::Ofdm54M)
        .max_ppdu_duration_micros(STAND_MPDU)
        .unwrap();
    assert_eq!(first.end_micros - first.start_micros, u64::from(ppdu + 44));
}

#[test]
fn the_applied_agreement_is_what_the_access_point_reads_at_each_aggregate() {
    let world = roomy();
    let mut medium = medium(1, Some(peer(PeerWindow::Agreed, Offered::Saturated)));
    medium.advance(0, &world).unwrap();
    assert_eq!(medium.report().peer.subframes, 1);
    let end = medium.on_air.as_ref().unwrap().end;
    medium.advance(end, &world).unwrap();
    world.agreed.set(Some(10));
    medium.advance(end + 1_000, &world).unwrap();
    assert_eq!(medium.report().peer.subframes, 11);
}

#[test]
fn an_offered_rate_sends_what_arrived_and_a_quiet_access_point_waits_for_the_next_frame() {
    let world = roomy();
    let rate = NonZeroU32::new(2_000).unwrap();
    let mut medium = medium(
        1,
        Some(peer(
            PeerWindow::Assumed(16),
            Offered::FramesPerSecond(rate),
        )),
    );
    assert_eq!(medium.next_event(), Some(500));
    medium.advance(1_000_000, &world).unwrap();
    let report = medium.report();
    assert_eq!(report.landed, 1_999, "the last arrives at the second's end");
    assert_eq!(report.bounds.offered, report.peer.exchanges);
}

#[test]
fn one_seed_replays_a_channel_and_another_draws_other_backoffs() {
    let spans = |seed| {
        let world = roomy();
        world.agreed.set(Some(16));
        let mut medium = medium(seed, Some(peer(PeerWindow::Agreed, Offered::Saturated)));
        saturate(&mut medium, &world, 200_000);
        medium.spans().to_vec()
    };
    assert_eq!(spans(11), spans(11));
    assert_ne!(spans(11), spans(12));
}

#[test]
fn a_frame_outside_the_access_points_own_limits_is_a_named_error() {
    let world = roomy();
    let mut air_peer = peer(PeerWindow::Assumed(4), Offered::Saturated);
    air_peer.max_length = 100;
    let mut medium = medium(1, Some(air_peer));
    assert_eq!(
        medium.advance(0, &world),
        Err(AirTimeError::PeerFrameOutsideLimits)
    );
}

#[test]
fn the_report_counts_from_its_reset() {
    let world = roomy();
    let mut medium = medium(1, None);
    saturate(&mut medium, &world, 50_000);
    assert_ne!(medium.report(), AirReport::default());
    medium.reset_report();
    assert_eq!(medium.report(), AirReport::default());
    assert!(medium.spans().is_empty());
}

#[test]
fn an_air_refuses_inputs_it_cannot_run_on_and_runs_on_one_medium() {
    let clock = Rc::new(oer_portable_time_virtual::VirtualClock::<8>::new());
    let air = crate::ModelAir::new(clock);
    let config = || AirConfig {
        seed: 1,
        timing: AirTiming::HT_2_4_GHZ,
        arena_frames: 32,
        rx_limits: ModelRxLimits {
            ring: 8,
            held: 8,
            lent: 8,
        },
        busy: None,
        peer: None,
    };
    let refused = |config: AirConfig| air.set_air_time(config).unwrap_err();
    assert_eq!(
        refused(AirConfig {
            arena_frames: 0,
            ..config()
        }),
        AirConfigRefused::ArenaFrames
    );
    assert_eq!(
        refused(AirConfig {
            arena_frames: 33,
            ..config()
        }),
        AirConfigRefused::ArenaFrames
    );
    assert_eq!(
        refused(AirConfig {
            busy: Some(BusyChannel {
                period_micros: NonZeroU32::new(100).unwrap(),
                busy_micros: 100,
            }),
            ..config()
        }),
        AirConfigRefused::BusyPeriod
    );
    let mut silent = peer(PeerWindow::Assumed(4), Offered::Saturated);
    silent.max_subframes = 0;
    assert_eq!(
        refused(AirConfig {
            peer: Some(silent),
            ..config()
        }),
        AirConfigRefused::PeerSubframes
    );
    assert_eq!(air.air_report(), None);
    assert_eq!(air.set_air_time(config()), Ok(()));
    assert_eq!(refused(config()), AirConfigRefused::AlreadyTimed);
    assert_eq!(air.air_report(), Some(AirReport::default()));
}
