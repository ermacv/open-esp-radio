use oer_ieee80211_mac::beacon::{TimAssociationId, TimVirtualBitmap};

use oer_ieee80211_mac::{
    ap::profile::{LegacyRates, WmmParameters},
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
};

use super::*;

/// An advertisement of one-stream HT with the ordinary legacy rates.
const ADVERTISEMENT: Advertisement = match Advertisement::new(
    LegacyRates::new(
        [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
        [0x6c, 0x12, 0x24, 0x48],
    ),
    HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
    match WmmParameters::new(
        4,
        false,
        [WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: 3,
            ecw_min: 4,
            ecw_max: 10,
            txop_limit_units_32_us: 0,
        }; 4],
    ) {
        Some(value) => value,
        None => panic!("the WMM parameters fit their fields"),
    },
    0x0421,
) {
    Some(value) => value,
    None => panic!("the advertisement leaves Privacy to the security mode"),
};

const fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

fn publication_beacon(storage: &mut [u8; AP_BEACON_CAPACITY]) -> ApBeacon<'_> {
    ApBeacon::new(
        storage,
        &ADVERTISEMENT,
        [2; 6],
        &WifiSsid::new(b"ap").unwrap(),
        Channel::ghz2_4(6, oer_ieee80211_mac::channel::ChannelWidth::Mhz20).unwrap(),
        100,
        2,
        SequenceNumber::ZERO,
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap()
}

#[test]
fn only_a_delivered_dtim_beacon_releases_its_group_prefix() {
    for outcome in [
        ApBeaconTxOutcome::Delivered,
        ApBeaconTxOutcome::NotDelivered,
        ApBeaconTxOutcome::Indeterminate,
        ApBeaconTxOutcome::Withdrawn,
        ApBeaconTxOutcome::Unsent,
    ] {
        let mut storage = [0; AP_BEACON_CAPACITY];
        let mut beacon = publication_beacon(&mut storage);
        let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
        let publication = beacon
            .prepare(at(102_400), SequenceNumber::ZERO, 3, bitmap.partial())
            .unwrap();
        let (frame, context) = publication.into_parts();
        let (offset, count, _) = dtim(frame).unwrap();
        assert_eq!(count, 0);
        assert_eq!(frame[offset + 4] & 1, 1);
        let release = context
            .complete(outcome)
            .map(|grant| grant.into_frame_count().get());
        assert_eq!(
            release,
            (outcome == ApBeaconTxOutcome::Delivered).then_some(3)
        );
    }
}

#[test]
fn later_buffered_frames_and_tim_phases_do_not_change_a_published_prefix() {
    let mut storage = [0; AP_BEACON_CAPACITY];
    let mut beacon = publication_beacon(&mut storage);
    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    let first = beacon
        .prepare(at(102_400), SequenceNumber::ZERO, 2, bitmap.partial())
        .unwrap();
    let (first_frame, first_context) = first.into_parts();
    let (offset, count, _) = dtim(first_frame).unwrap();
    assert_eq!(count, 0);
    assert_eq!(first_frame[offset + 4] & 1, 1);

    // Three new group frames arrive after the copied MPDU entered TX. The
    // template now describes a non-DTIM beacon while its old context lives.
    let later = beacon
        .prepare(
            at(204_800),
            SequenceNumber::new(1).unwrap(),
            5,
            bitmap.partial(),
        )
        .unwrap();
    let (later_frame, later_context) = later.into_parts();
    let (offset, count, _) = dtim(later_frame).unwrap();
    assert_eq!(count, 1);
    assert_eq!(later_frame[offset + 4] & 1, 0);
    assert!(
        later_context
            .complete(ApBeaconTxOutcome::Delivered)
            .is_none()
    );
    let release = first_context
        .complete(ApBeaconTxOutcome::Delivered)
        .unwrap();
    assert_eq!(release.into_frame_count().get(), 2);
}

#[test]
fn empty_dtim_never_grants_a_group_release() {
    let mut storage = [0; AP_BEACON_CAPACITY];
    let mut beacon = publication_beacon(&mut storage);
    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    let (frame, context) = beacon
        .prepare(at(102_400), SequenceNumber::ZERO, 0, bitmap.partial())
        .unwrap()
        .into_parts();
    let (offset, count, _) = dtim(frame).unwrap();
    assert_eq!(count, 0);
    assert_eq!(frame[offset + 4] & 1, 0);
    assert!(context.complete(ApBeaconTxOutcome::Delivered).is_none());
}

#[test]
fn static_storage_owns_beacon_dtim_and_next_deadline() {
    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        &ADVERTISEMENT,
        [2; 6],
        &ssid,
        Channel::ghz2_4(6, oer_ieee80211_mac::channel::ChannelWidth::Mhz20).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    assert!(beacon.publication_due(at(102_400)));
    assert_eq!(beacon.next_publication(), None);
    let mut bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    bitmap.set(TimAssociationId::new(1).unwrap(), true).unwrap();
    let frame = beacon
        .prepare(
            at(102_400),
            SequenceNumber::new(4).unwrap(),
            2,
            bitmap.partial(),
        )
        .unwrap();
    let frame = frame.frame();
    let (offset, count, period) = dtim(frame).unwrap();
    assert_eq!((count, period), (0, 2));
    assert_eq!(frame[offset + 4] & 1, 1);
    assert_eq!(frame[offset + 5], 0x02);
    assert_eq!(&frame[22..24], &0x0040_u16.to_le_bytes());
    assert_eq!(beacon.next_publication(), Some(at(204_800)));
    assert!(!beacon.publication_due(at(204_799)));
    assert!(beacon.publication_due(at(204_800)));
    assert!(beacon.publication_due(at(204_801)));
    assert_eq!(
        beacon.publication_lateness(at(204_800)),
        (0, Duration::from_micros(0))
    );
    assert_eq!(
        beacon.publication_lateness(at(204_801)),
        (0, Duration::from_micros(1))
    );
    assert_eq!(
        beacon.publication_lateness(at(307_200)),
        (1, Duration::from_micros(102_400))
    );
    assert_eq!(
        beacon.publication_lateness(at(307_201)),
        (1, Duration::from_micros(102_401))
    );
}

#[test]
fn late_publication_does_not_move_the_absolute_tbtt_schedule() {
    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        &ADVERTISEMENT,
        [2; 6],
        &ssid,
        Channel::ghz2_4(6, oer_ieee80211_mac::channel::ChannelWidth::Mhz20).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();

    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    assert!(
        beacon
            .prepare(
                at(102_400),
                SequenceNumber::new(4).unwrap(),
                0,
                bitmap.partial(),
            )
            .unwrap()
            .into_parts()
            .1
            .complete(ApBeaconTxOutcome::Unsent)
            .is_none(),
    );
    assert_eq!(beacon.next_publication(), Some(at(204_800)));
    assert_eq!(
        beacon.publication_lateness(at(204_900)),
        (0, Duration::from_micros(100))
    );

    assert!(
        beacon
            .prepare(
                at(204_900),
                SequenceNumber::new(5).unwrap(),
                0,
                bitmap.partial(),
            )
            .unwrap()
            .into_parts()
            .1
            .complete(ApBeaconTxOutcome::Unsent)
            .is_none(),
    );
    assert_eq!(beacon.next_publication(), Some(at(307_200)));
}

#[test]
fn the_tbtt_schedule_survives_the_u32_microsecond_boundary() {
    // About 71.6 minutes after boot the executor clock passes 2^32 µs; the
    // schedule keeps counting on the u64 axis instead of a wrapping tick.
    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        &ADVERTISEMENT,
        [2; 6],
        &ssid,
        Channel::ghz2_4(6, oer_ieee80211_mac::channel::ChannelWidth::Mhz20).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    let first = (1_u64 << 32) - 50_000;
    assert!(
        beacon
            .prepare(
                at(first),
                SequenceNumber::new(4).unwrap(),
                0,
                bitmap.partial(),
            )
            .unwrap()
            .into_parts()
            .1
            .complete(ApBeaconTxOutcome::Unsent)
            .is_none(),
    );
    let next = first + 102_400;
    assert!(next > 1 << 32);
    assert_eq!(beacon.next_publication(), Some(at(next)));
    assert!(!beacon.publication_due(at(1 << 32)));
    assert!(beacon.publication_due(at(next)));
    assert_eq!(
        beacon.publication_lateness(at(next + 5)),
        (0, Duration::from_micros(5))
    );
}

#[test]
fn an_announced_switch_counts_down_in_each_beacon_until_it_is_due() {
    use oer_ieee80211_mac::channel::{Channel, ChannelWidth};
    use oer_ieee80211_mac::channel_switch::{
        ChannelSwitchMode, SecondaryChannelOffset, parse_channel_switch,
    };

    let mut storage = [0; AP_BEACON_CAPACITY];
    let ssid = WifiSsid::new(b"ap").unwrap();
    let mut beacon = ApBeacon::new(
        &mut storage,
        &ADVERTISEMENT,
        [2; 6],
        &ssid,
        Channel::ghz2_4(6, oer_ieee80211_mac::channel::ChannelWidth::Mhz20).unwrap(),
        100,
        2,
        SequenceNumber::new(3).unwrap(),
        ApSecurityPolicy::Wpa2Personal,
    )
    .unwrap();
    let bitmap = TimVirtualBitmap::<2>::try_new().unwrap();
    let announced = |frame: &[u8]| parse_channel_switch(&frame[36..]).unwrap();
    let target = Channel::ghz2_4(1, ChannelWidth::Mhz40Above).unwrap();
    let switch = ChannelSwitch::to(target, ChannelSwitchMode::Continue, 2);
    assert_eq!(
        beacon.announce_channel_switch(ChannelSwitch { count: 0, ..switch }),
        Err(ApChannelSwitchError::ZeroCount)
    );
    beacon.announce_channel_switch(switch).unwrap();
    // A probe response repeats the announcement.
    assert_eq!(announced(beacon.advertisement()), Some(switch));

    let sequence = SequenceNumber::new(4).unwrap();
    let mut counts = [0_u8; 2];
    for tbtt in 1..=2_u64 {
        assert_eq!(beacon.channel_switch_due(), None);
        // A wider TIM moves the announcement with the elements after it.
        let mut wide = TimVirtualBitmap::<2>::try_new().unwrap();
        wide.set(TimAssociationId::new(9).unwrap(), tbtt == 2)
            .unwrap();
        let frame = beacon
            .prepare(at(tbtt * 102_400), sequence, 0, wide.partial())
            .unwrap();
        let frame = frame.frame();
        let carried = announced(frame).unwrap();
        assert_eq!(carried.secondary, SecondaryChannelOffset::Above);
        assert_eq!(
            carried.target(oer_ieee80211_mac::channel::Band::Ghz2_4),
            Ok(target)
        );
        counts[tbtt as usize - 1] = carried.count;
        assert!(dtim(frame).is_some());
    }
    assert_eq!(counts, [2, 1]);
    // No beacon goes out on the old channel once the switch is due.
    assert_eq!(
        beacon.channel_switch_due().map(|due| due.channel_number),
        Some(1)
    );
    assert!(beacon.publication_due(at(3 * 102_400)));
    assert!(
        beacon
            .prepare(at(3 * 102_400), sequence, 0, bitmap.partial())
            .is_none()
    );

    // Written again for the new channel, the template drops the
    // announcement and keeps its schedule.
    beacon
        .rewrite(
            &ADVERTISEMENT,
            [2; 6],
            &ssid,
            Channel::ghz2_4(1, oer_ieee80211_mac::channel::ChannelWidth::Mhz40Above).unwrap(),
            100,
            2,
            sequence,
            ApSecurityPolicy::Wpa2Personal,
        )
        .unwrap();
    assert_eq!(beacon.channel_switch(), None);
    assert_eq!(beacon.next_publication(), Some(at(3 * 102_400)));
    let frame = beacon
        .prepare(at(3 * 102_400), sequence, 0, bitmap.partial())
        .unwrap();
    let frame = frame.frame();
    assert_eq!(announced(frame), None);
    // The DS Parameter Set names the new channel.
    let ds = frame[36..]
        .windows(3)
        .position(|window| window[0] == 3 && window[1] == 1)
        .unwrap();
    assert_eq!(frame[36 + ds + 2], 1);
}
