//! Coalesced P1 settings, read only by a bounded P2 poll.

use super::*;
use atomic_words::AtomicWords;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicUsize, Ordering};
use oer_ieee80211_wire::{
    channel::ChannelWidth,
    extensions::wmm::{WmmAcParameters, WmmParameterSet},
    qos::WmmAccessCategory,
    sequence::SequenceNumber,
};

pub(super) struct Flag(AtomicBool);
impl Flag {
    pub(super) const fn new(value: bool) -> Self {
        Self(AtomicBool::new(value))
    }
    pub(super) fn get(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub(super) fn set(&self, value: bool) {
        self.0.store(value, Ordering::SeqCst);
    }
    pub(super) fn replace(&self, value: bool) -> bool {
        self.0.swap(value, Ordering::SeqCst)
    }
}

/// The received units P2 queued for P1 that P1 has not taken, and whether
/// P2 waits for P1 to take one. P2 asks before it reads the count again,
/// and P1 takes before it reads the ask, so a take after P2 saw the queue
/// full always finds the ask, on one hart or across two.
pub(super) struct ReceivedInFlight {
    queued: AtomicUsize,
    wanted: AtomicBool,
}
impl ReceivedInFlight {
    pub(super) const fn new() -> Self {
        Self {
            queued: AtomicUsize::new(0),
            wanted: AtomicBool::new(false),
        }
    }
    /// P2 queued one unit.
    pub(super) fn queue(&self) {
        self.queued.fetch_add(1, Ordering::SeqCst);
    }
    /// The units P2 may still queue under `bound`.
    pub(super) fn room(&self, bound: usize) -> usize {
        bound.saturating_sub(self.queued.load(Ordering::SeqCst))
    }
    /// P2 waits for P1 to take a unit: P1's next take wakes it.
    pub(super) fn want(&self) {
        self.wanted.store(true, Ordering::SeqCst);
    }
    /// P1 took one unit: whether P2 waits for it. A take of a unit never
    /// queued is refused and leaves the count as it was.
    pub(super) fn take(&self) -> Result<bool, SettingFault> {
        if self.queued.fetch_sub(1, Ordering::SeqCst) == 0 {
            // The count wrapped below zero for one step; P2 read no room.
            self.queued.fetch_add(1, Ordering::SeqCst);
            return Err(SettingFault::ReceivedUnqueued);
        }
        Ok(self.wanted.swap(false, Ordering::SeqCst))
    }
    /// A new installation queued nothing yet.
    pub(super) fn reset(&self) {
        self.queued.store(0, Ordering::SeqCst);
        self.wanted.store(false, Ordering::SeqCst);
    }
}

pub(super) struct Byte(AtomicU8);
impl Byte {
    pub(super) const fn new() -> Self {
        Self(AtomicU8::new(0))
    }
    pub(super) fn get(&self) -> u8 {
        self.0.load(Ordering::SeqCst)
    }
    pub(super) fn set(&self, value: u8) {
        self.0.store(value, Ordering::SeqCst);
    }
}

/// Why a published setting cannot be stored or used.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingFault {
    /// The channel is not a 2.4 GHz channel, the only band S31 tunes.
    UnsupportedChannel,
    /// No channel was published yet, or its byte names none.
    ChannelUnset,
    /// An MSDU lifetime exceeds the microseconds of its word.
    EdcaLifetime,
    /// A published Block Ack record holds no sequence number.
    BlockAckSequence,
    /// P1 took a received unit P2 never queued.
    ReceivedUnqueued,
}

/// One 2.4 GHz channel, 20 or 40 MHz wide: its number and its width in
/// one word, so P2 never reads one without the other; zero until the first
/// publication.
pub(super) struct ChannelSetting(AtomicU16);
impl ChannelSetting {
    const fn new() -> Self {
        Self(AtomicU16::new(0))
    }
    /// Store a 2.4 GHz channel; any other is refused unchanged.
    pub(super) fn set(&self, channel: Channel) -> Result<(), SettingFault> {
        if channel.band() != Band::Ghz2_4 {
            return Err(SettingFault::UnsupportedChannel);
        }
        let width = match channel.width() {
            ChannelWidth::Mhz20 => 0,
            ChannelWidth::Mhz40Above => 1,
            ChannelWidth::Mhz40Below => 2,
        };
        self.0.store(
            u16::from_le_bytes([channel.number(), width]),
            Ordering::SeqCst,
        );
        Ok(())
    }
    /// The last stored channel.
    pub(super) fn get(&self) -> Result<Channel, SettingFault> {
        let [number, width] = self.0.load(Ordering::SeqCst).to_le_bytes();
        let width = match width {
            0 => ChannelWidth::Mhz20,
            1 => ChannelWidth::Mhz40Above,
            2 => ChannelWidth::Mhz40Below,
            _ => return Err(SettingFault::ChannelUnset),
        };
        match number {
            0 => Err(SettingFault::ChannelUnset),
            number => Channel::ghz2_4(number, width).map_err(|_| SettingFault::ChannelUnset),
        }
    }
}

// SupportedEdca bounds every lifetime by 500 TU, so its microseconds fit
// one atomic word. Each VIF publishes only its own table.
pub(super) struct EdcaSetting([AtomicWords<9>; 2]);
impl EdcaSetting {
    const fn new() -> Self {
        Self([const { AtomicWords::new() }; 2])
    }
    /// Publish one VIF's table; a lifetime beyond its word is refused and
    /// the published table stays unchanged.
    pub(super) fn set(&self, vif: usize, table: EdcaParameterSet) -> Result<(), SettingFault> {
        let wmm = table.wmm();
        let mut words = [0; 9];
        words[0] = u32::from(wmm.parameter_set_count) | (u32::from(wmm.uapsd) << 8);
        for (index, category) in WmmAccessCategory::ALL.into_iter().enumerate() {
            let ac = table.access_category(category);
            let parameters = ac.wmm;
            let at = 1 + index * 2;
            words[at] = u32::from(parameters.aifsn.get())
                | (u32::from(parameters.ecw_min.get()) << 4)
                | (u32::from(parameters.ecw_max.get()) << 8)
                | (u32::from(parameters.admission_control_mandatory) << 12)
                | (u32::from(parameters.txop_limit_units_32_us) << 16);
            words[at + 1] = u32::try_from(ac.msdu_lifetime.as_micros())
                .map_err(|_| SettingFault::EdcaLifetime)?;
        }
        self.0[vif].set(words);
        Ok(())
    }
    /// One VIF's last complete publication; `None` while P1 is publishing
    /// it, which P2 retries after P1's commit wake.
    pub(super) fn get(&self, vif: usize) -> Option<EdcaParameterSet> {
        let words = self.0[vif].get()?;
        let acs = core::array::from_fn(|index| {
            let bits = words[1 + index * 2];
            WmmAcParameters {
                aifsn: oer_wire::U8::from_low_bits((bits & 15) as u8),
                ecw_min: oer_wire::U8::from_low_bits(((bits >> 4) & 15) as u8),
                ecw_max: oer_wire::U8::from_low_bits(((bits >> 8) & 15) as u8),
                admission_control_mandatory: bits & (1 << 12) != 0,
                txop_limit_units_32_us: (bits >> 16) as u16,
            }
        });
        let lifetimes =
            core::array::from_fn(|index| Duration::from_micros(u64::from(words[2 + index * 2])));
        Some(EdcaParameterSet::new(
            WmmParameterSet::new(words[0] as u8, words[0] & (1 << 8) != 0, acs),
            lifetimes,
        ))
    }
    pub(super) fn lifetime(&self, vif: usize, category: WmmAccessCategory) -> Duration {
        Duration::from_micros(u64::from(self.0[vif].word(2 + category as usize * 2)))
    }
}

pub(super) struct BlockAckSetting(AtomicWords<24>);
impl BlockAckSetting {
    const fn new() -> Self {
        Self(AtomicWords::new())
    }
    pub(super) fn set(&self, index: usize, value: Option<core_mac::RxBlockAckValues>) {
        let words = match value {
            None => [0; 3],
            Some(value) => {
                let [a, b, c, d, e, f] = value.peer;
                [
                    u32::from_le_bytes([a, b, c, d]),
                    u32::from(u16::from_le_bytes([e, f]))
                        | (u32::from(value.tid) << 16)
                        | (u32::from(value.role == VifRole::AccessPoint) << 20)
                        | (1 << 21)
                        | (u32::from(value.window) << 24),
                    u32::from(value.start_sequence.get()),
                ]
            }
        };
        self.0.set_at(index * 3, words);
    }
    pub(super) fn clear(&self) {
        self.0.set([0; 24]);
    }
    /// One agreement's last complete publication: `Ok(None)` without an
    /// agreement, `Err(None)` while P1 is publishing it, which P2 retries
    /// after P1's commit wake, and `Err(Some(fault))` for a record no
    /// publication writes.
    pub(super) fn get(
        &self,
        index: usize,
    ) -> Result<Option<core_mac::RxBlockAckValues>, Option<SettingFault>> {
        decode_block_ack(self.0.get_at(index * 3).ok_or(None)?).map_err(Some)
    }
}

#[inline(never)]
fn decode_block_ack(words: [u32; 3]) -> Result<Option<core_mac::RxBlockAckValues>, SettingFault> {
    let flags = words[1];
    if flags & (1 << 21) == 0 {
        return Ok(None);
    }
    let [a, b, c, d] = words[0].to_le_bytes();
    let [e, f] = (flags as u16).to_le_bytes();
    let start_sequence =
        SequenceNumber::new(words[2] as u16).ok_or(SettingFault::BlockAckSequence)?;
    Ok(Some(core_mac::RxBlockAckValues {
        role: if flags & (1 << 20) != 0 {
            VifRole::AccessPoint
        } else {
            VifRole::Station
        },
        peer: [a, b, c, d, e, f],
        tid: ((flags >> 16) & 15) as u8,
        window: (flags >> 24) as u8,
        start_sequence,
    }))
}

pub(super) struct ColorSetting(AtomicU8);
impl ColorSetting {
    const fn new() -> Self {
        Self(AtomicU8::new(0))
    }
    pub(super) fn set(&self, color: Option<BssColor>) {
        self.0
            .store(color.map_or(0, |color| color.get() + 1), Ordering::SeqCst);
    }
    pub(super) fn take(&self) -> Option<BssColor> {
        self.0
            .swap(0, Ordering::SeqCst)
            .checked_sub(1)
            .and_then(BssColor::new)
    }
}

/// Only P1 changes configuration after split. Its busy flag brackets complete
/// coalesced updates, including the dirty flag. P2 checks it before touching
/// settings; on this core P1 cannot preempt that bounded P2 poll. An interrupted
/// P1 publication is skipped without retry and wakes P2 after committing.
pub(super) struct Settings {
    pub(super) publishing: Flag,
    pub(super) clock_requested: Flag,
    pub(super) enabled: Flag,
    pub(super) want_enabled: Flag,
    pub(super) channel: ChannelSetting,
    pub(super) edca: EdcaSetting,
    pub(super) dirty_edca: Flag,
    pub(super) block_acks: BlockAckSetting,
    pub(super) dirty_ba: Flag,
    pub(super) color: ColorSetting,
    pub(super) loss: Flag,
    #[cfg(oer_debug_control)]
    pub(super) publication_hold: Byte,
    #[cfg(oer_debug_control)]
    pub(super) completion_hold: Flag,
    #[cfg(oer_debug_control)]
    pub(super) reject_group_replacement: Flag,
}
impl Settings {
    pub(super) const fn new() -> Self {
        Self {
            publishing: Flag::new(false),
            clock_requested: Flag::new(false),
            enabled: Flag::new(false),
            want_enabled: Flag::new(false),
            channel: ChannelSetting::new(),
            edca: EdcaSetting::new(),
            dirty_edca: Flag::new(false),
            block_acks: BlockAckSetting::new(),
            dirty_ba: Flag::new(false),
            color: ColorSetting::new(),
            loss: Flag::new(false),
            #[cfg(oer_debug_control)]
            publication_hold: Byte::new(),
            #[cfg(oer_debug_control)]
            completion_hold: Flag::new(false),
            #[cfg(oer_debug_control)]
            reject_group_replacement: Flag::new(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_edca_preserves_both_vifs_and_every_validated_record_field() {
        let caps = Esp32s31Caps::<512>::CAPS;
        let records = core::array::from_fn(|index| WmmAcParameters {
            admission_control_mandatory: index & 1 != 0,
            aifsn: oer_wire::U8::new(caps.edca_max_aifsn.saturating_sub(index as u8)).unwrap(),
            ecw_min: oer_wire::U8::new((index as u8).min(caps.edca_max_cw_exponent)).unwrap(),
            ecw_max: oer_wire::U8::new(caps.edca_max_cw_exponent).unwrap(),
            txop_limit_units_32_us: caps
                .edca_max_txop_limit_units_32_us
                .saturating_sub(index as u16),
        });
        let table = EdcaParameterSet::new(
            WmmParameterSet::new(15, true, records),
            [
                Duration::ZERO,
                Duration::from_micros(1),
                Duration::from_micros(65535),
                EdcaParameterSet::DEFAULT_MSDU_LIFETIME,
            ],
        );
        assert!(SupportedEdca::<Esp32s31Caps<512>>::new(table).is_some());
        let setting = EdcaSetting::new();
        let expected = [EdcaParameterSet::STANDARD_DEFAULT, table];
        assert_eq!(setting.set(0, expected[0]), Ok(()));
        assert_eq!(setting.set(1, expected[1]), Ok(()));
        assert_eq!([setting.get(0), setting.get(1)], expected.map(Some));
        for category in WmmAccessCategory::ALL {
            assert_eq!(
                setting.lifetime(1, category),
                table.access_category(category).msdu_lifetime
            );
        }
    }

    #[test]
    fn updating_one_block_ack_place_preserves_the_other_interface_and_empty_places() {
        let settings = BlockAckSetting::new();
        let station = core_mac::RxBlockAckValues {
            role: VifRole::Station,
            peer: [2, 255, 1, 254, 3, 253],
            tid: 15,
            start_sequence: SequenceNumber::new(4095).expect("sequence"),
            window: 64,
        };
        let access_point = core_mac::RxBlockAckValues {
            role: VifRole::AccessPoint,
            tid: 0,
            ..station
        };
        settings.set(0, Some(station));
        settings.set(7, Some(access_point));
        let actual: [_; 8] = core::array::from_fn(|index| settings.get(index));
        assert_eq!(actual[0], Ok(Some(station)));
        assert_eq!(actual[7], Ok(Some(access_point)));
        assert!(actual[1..7].iter().all(|place| *place == Ok(None)));
        settings.set(0, None);
        let actual: [_; 8] = core::array::from_fn(|index| settings.get(index));
        assert!(actual[..7].iter().all(|place| *place == Ok(None)));
        assert_eq!(actual[7], Ok(Some(access_point)));
    }

    #[test]
    fn the_channel_setting_stores_only_s31_channels_and_reads_none_before_one() {
        let setting = ChannelSetting::new();
        assert_eq!(setting.get(), Err(SettingFault::ChannelUnset));
        let six = Channel::ghz2_4(6, ChannelWidth::Mhz20).expect("channel 6");
        assert_eq!(setting.set(six), Ok(()));
        // A refused channel leaves the stored one unchanged.
        let five_ghz = Channel::ghz5(36, ChannelWidth::Mhz20).expect("channel 36");
        assert_eq!(setting.set(five_ghz), Err(SettingFault::UnsupportedChannel));
        assert_eq!(setting.get(), Ok(six));
    }

    #[test]
    fn a_take_reports_a_waiting_driver_once_and_refuses_a_unit_never_queued() {
        let received = ReceivedInFlight::new();
        assert_eq!(received.take(), Err(SettingFault::ReceivedUnqueued));
        assert_eq!(received.room(8), 8);
        for _ in 0..8 {
            received.queue();
        }
        assert_eq!(received.room(8), 0);
        assert_eq!(received.take(), Ok(false));
        received.want();
        assert_eq!(received.take(), Ok(true));
        assert_eq!(received.take(), Ok(false));
        assert_eq!(received.room(8), 3);
        received.reset();
        assert_eq!(received.room(8), 8);
        assert_eq!(received.take(), Err(SettingFault::ReceivedUnqueued));
        assert_eq!(received.room(8), 8);
    }

    #[test]
    fn the_channel_setting_keeps_a_40_mhz_channel_with_its_secondary() {
        let setting = ChannelSetting::new();
        for (number, width) in [
            (13, ChannelWidth::Mhz40Below),
            (6, ChannelWidth::Mhz40Above),
            (1, ChannelWidth::Mhz20),
        ] {
            let channel = Channel::ghz2_4(number, width).expect("2.4 GHz channel");
            assert_eq!(setting.set(channel), Ok(()));
            assert_eq!(setting.get(), Ok(channel));
        }
    }

    #[test]
    fn an_edca_lifetime_beyond_its_word_is_refused_and_keeps_the_published_table() {
        let setting = EdcaSetting::new();
        assert_eq!(setting.set(0, EdcaParameterSet::STANDARD_DEFAULT), Ok(()));
        let unbounded = EdcaParameterSet::new(
            EdcaParameterSet::STANDARD_DEFAULT.wmm(),
            [Duration::from_micros(u64::from(u32::MAX) + 1); 4],
        );
        assert_eq!(setting.set(0, unbounded), Err(SettingFault::EdcaLifetime));
        assert_eq!(setting.get(0), Some(EdcaParameterSet::STANDARD_DEFAULT));
    }

    #[test]
    fn a_block_ack_record_without_a_sequence_number_is_a_typed_fault() {
        // Present, but its sequence word holds no 12-bit sequence number.
        assert_eq!(
            decode_block_ack([0, 1 << 21, 4096]),
            Err(SettingFault::BlockAckSequence)
        );
        assert_eq!(decode_block_ack([0, 0, 0]), Ok(None));
    }
}
