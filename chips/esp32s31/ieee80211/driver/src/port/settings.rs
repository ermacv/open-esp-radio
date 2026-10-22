//! Coalesced P1 settings, read only by a bounded P2 poll.

use super::*;
use atomic_words::AtomicWords;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};
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

pub(super) struct ChannelSetting(Byte);
impl ChannelSetting {
    const fn new() -> Self {
        Self(Byte::new())
    }
    pub(super) fn set(&self, channel: Channel) {
        assert!(
            channel.band() == Band::Ghz2_4 && channel.width() == ChannelWidth::Mhz20,
            "S31 accepts only supported 2.4 GHz HT20 channels"
        );
        self.0.set(channel.number());
    }
    pub(super) fn get(&self) -> Channel {
        Channel::ghz2_4(self.0.get(), ChannelWidth::Mhz20)
            .ok()
            .expect("prepared S31 channel")
    }
}

// SupportedEdca bounds every lifetime by 500 TU, so its microseconds fit
// one atomic word. Each VIF publishes only its own table.
pub(super) struct EdcaSetting([AtomicWords<9>; 2]);
impl EdcaSetting {
    const fn new() -> Self {
        Self([const { AtomicWords::new() }; 2])
    }
    pub(super) fn set(&self, vif: usize, table: EdcaParameterSet) {
        let wmm = table.wmm();
        let mut words = [0; 9];
        words[0] = u32::from(wmm.parameter_set_count) | (u32::from(wmm.uapsd) << 8);
        for (index, category) in WmmAccessCategory::ALL.into_iter().enumerate() {
            let ac = table.access_category(category);
            let parameters = ac.wmm;
            let at = 1 + index * 2;
            words[at] = u32::from(parameters.aifsn)
                | (u32::from(parameters.ecw_min) << 4)
                | (u32::from(parameters.ecw_max) << 8)
                | (u32::from(parameters.admission_control_mandatory) << 12)
                | (u32::from(parameters.txop_limit_units_32_us) << 16);
            words[at + 1] = u32::try_from(ac.msdu_lifetime.as_micros())
                .ok()
                .expect("validated MSDU lifetime fits one word");
        }
        self.0[vif].set(words);
    }
    pub(super) fn get(&self, vif: usize) -> EdcaParameterSet {
        let words = self.0[vif].get().expect("completed P1 EDCA publication");
        let acs = core::array::from_fn(|index| {
            let bits = words[1 + index * 2];
            WmmAcParameters {
                aifsn: (bits & 15) as u8,
                ecw_min: ((bits >> 4) & 15) as u8,
                ecw_max: ((bits >> 8) & 15) as u8,
                admission_control_mandatory: bits & (1 << 12) != 0,
                txop_limit_units_32_us: (bits >> 16) as u16,
            }
        });
        let lifetimes =
            core::array::from_fn(|index| Duration::from_micros(u64::from(words[2 + index * 2])));
        EdcaParameterSet::new(
            WmmParameterSet::new(words[0] as u8, words[0] & (1 << 8) != 0, acs),
            lifetimes,
        )
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
    pub(super) fn get(&self, index: usize) -> Option<core_mac::RxBlockAckValues> {
        decode_block_ack(
            self.0
                .get_at(index * 3)
                .expect("completed P1 Block Ack publication"),
        )
    }
}

#[inline(never)]
fn decode_block_ack(words: [u32; 3]) -> Option<core_mac::RxBlockAckValues> {
    let flags = words[1];
    if flags & (1 << 21) == 0 {
        return None;
    }
    let [a, b, c, d] = words[0].to_le_bytes();
    let [e, f] = (flags as u16).to_le_bytes();
    Some(core_mac::RxBlockAckValues {
        role: if flags & (1 << 20) != 0 {
            VifRole::AccessPoint
        } else {
            VifRole::Station
        },
        peer: [a, b, c, d, e, f],
        tid: ((flags >> 16) & 15) as u8,
        window: (flags >> 24) as u8,
        start_sequence: SequenceNumber::new(words[2] as u16).expect("validated sequence"),
    })
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
            aifsn: caps.edca_max_aifsn.saturating_sub(index as u8),
            ecw_min: (index as u8).min(caps.edca_max_cw_exponent),
            ecw_max: caps.edca_max_cw_exponent,
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
        setting.set(0, expected[0]);
        setting.set(1, expected[1]);
        assert_eq!([setting.get(0), setting.get(1)], expected);
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
        assert_eq!(actual[0], Some(station));
        assert_eq!(actual[7], Some(access_point));
        assert!(actual[1..7].iter().all(Option::is_none));
        settings.set(0, None);
        let actual: [_; 8] = core::array::from_fn(|index| settings.get(index));
        assert!(actual[..7].iter().all(Option::is_none));
        assert_eq!(actual[7], Some(access_point));
    }
}
