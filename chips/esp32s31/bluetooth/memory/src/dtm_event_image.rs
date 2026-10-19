//! CPU-owned positional words covered by the reviewed DTM event transforms.
//!
//! These value types describe offsets inside the bound link-state and
//! scheduler-item allocations. Raw words remain private to this memory crate;
//! upper layers can apply only the semantic transforms published below. The
//! memory-graph transition validates allocation-binding anchors, and none of
//! these values grants publication or controller ownership.

#![forbid(unsafe_code)]

use crate::{
    ControllerSramLinkAddress,
    le_phy_packet::{LeAccessAddress, LeCrcInit},
    le_tx_power::LeTxPower,
    link_state_event::{LinkStateEventWord, item_with_power},
};

const LOW_TWENTY_MASK: u32 = 0x000f_ffff;
const LINK_STATE_CONFIG_MASK: u32 = 0x3f00_0000;
const SCHEDULER_FREQUENCY_MASK: u32 = 0x0000_7f00;
const SCHEDULER_RATE_LANES_MASK: u32 = 0xf000_0000;
const INITIAL_RECEIVER_CONFIGURATION_IMAGE: u32 = 0x000f_0001;
const RECURRING_RECEIVER_CONFIGURATION_IMAGE: u32 = 0x0000_0001;

/// Opaque controller projection of this graph's current TX-header head.
///
/// The memory graph creates this value only while producing a positional
/// event seed. It carries neither a general SRAM address nor publication
/// authority, so it cannot be substituted for the RX-header tail projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmTxHeaderHeadProjection(u32);

impl DtmTxHeaderHeadProjection {
    pub(super) const fn from_bound(address: ControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn from_link_state_word(word: u32) -> Self {
        Self(word & LOW_TWENTY_MASK)
    }

    const fn apply_to_link_state_word(self, word: u32) -> u32 {
        (word & !LOW_TWENTY_MASK) | self.0
    }
}

/// Opaque controller projection of this graph's current RX-header tail.
///
/// The memory graph creates this value only while producing a positional
/// event seed. It carries neither a general SRAM address nor publication
/// authority, so it cannot be substituted for the TX-header head projection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmRxHeaderTailProjection(u32);

impl DtmRxHeaderTailProjection {
    pub(super) const fn from_bound(address: ControllerSramLinkAddress) -> Self {
        Self(address.compressed_image())
    }

    const fn from_link_state_word(word: u32) -> Self {
        Self(word & LOW_TWENTY_MASK)
    }

    const fn apply_to_link_state_word(self, word: u32) -> u32 {
        (word & !LOW_TWENTY_MASK) | self.0
    }
}

/// DTM role shared by the CPU-owned link-state and scheduler-item formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmRole {
    /// Repeated transmitter-test events use role image one.
    Transmitter,
    /// Repeated receiver-test events use role image two.
    Receiver,
}

/// Phase selecting the role-private receiver configuration for one DTM event.
///
/// The initial event establishes the complete receiver configuration. After
/// one completion has returned the private graph, the recurring vendor path
/// deliberately replaces it with the narrower re-use configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmReceiverEventPhase {
    /// First receiver event in one active test session.
    Initial,
    /// Receiver event prepared from a recycled active-session graph.
    Recurring,
}

/// PHY choice admitted by a transmitter DTM scheduler item.
///
/// These variants retain the role-specific meaning established by the DTM
/// command validator. They deliberately do not expose the controller field
/// image used by the private SRAM codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerTransmitterPhy {
    /// LE 1M transmitter.
    Le1M,
    /// LE 2M transmitter.
    Le2M,
    /// LE Coded transmitter using S=8 coding.
    LeCodedS8,
    /// LE Coded transmitter using S=2 coding.
    LeCodedS2,
}

/// PHY choice admitted by a receiver DTM scheduler item.
///
/// The receiver command has no S=2 selector. Encoding that restriction in the
/// type prevents a transmitter-only choice from reaching the SRAM codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerReceiverPhy {
    /// LE 1M receiver.
    Le1M,
    /// LE 2M receiver.
    Le2M,
    /// Generic LE Coded receiver.
    LeCoded,
}

/// Private encoding applied to both replicated scheduler PHY lanes.
///
/// Current vendor bodies prove that both lanes receive the same two-bit code,
/// but do not establish independent names for the lanes. Keeping the code in
/// this private value avoids inventing that missing hardware semantic.
#[derive(Clone, Copy)]
struct DtmSchedulerPhyEncoding(u32);

impl DtmSchedulerPhyEncoding {
    const fn for_transmitter(phy: DtmSchedulerTransmitterPhy) -> Self {
        Self(match phy {
            DtmSchedulerTransmitterPhy::Le1M => 0,
            DtmSchedulerTransmitterPhy::Le2M => 1,
            DtmSchedulerTransmitterPhy::LeCodedS8 => 2,
            DtmSchedulerTransmitterPhy::LeCodedS2 => 3,
        })
    }

    const fn for_receiver(phy: DtmSchedulerReceiverPhy) -> Self {
        Self(match phy {
            DtmSchedulerReceiverPhy::Le1M => 0,
            DtmSchedulerReceiverPhy::Le2M => 1,
            DtmSchedulerReceiverPhy::LeCoded => 3,
        })
    }
}

/// Private codec for the link-state word changed by the DTM reset profile.
///
/// Both reviewed implementations select the same two-bit image in this word,
/// and the complete descriptor is subsequently reachable by the controller.
/// No field-specific hardware reader or independent bit names have been
/// recovered, so only the observed aggregate DTM selection is exposed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct DtmLinkStateProfileWord(u32);

impl DtmLinkStateProfileWord {
    const DIRECT_TEST_MODE_MASK: u32 = 0x0c00_0000;

    pub(super) const fn from_storage(word: u32) -> Self {
        Self(word)
    }

    const fn select_direct_test_mode(self) -> Self {
        Self((self.0 & !Self::DIRECT_TEST_MODE_MASK) | Self::DIRECT_TEST_MODE_MASK)
    }

    pub(super) const fn into_storage(self) -> u32 {
        self.0
    }

    #[cfg(test)]
    pub(super) const fn direct_test_mode_is_selected(self) -> bool {
        self.0 & Self::DIRECT_TEST_MODE_MASK == Self::DIRECT_TEST_MODE_MASK
    }
}

/// Role and phase selecting one reviewed scheduler-item event transform.
///
/// Transmitter events have no phase-dependent scheduler-item configuration.
/// Nesting the receiver phase in the receiver variant prevents a caller from
/// attaching receiver-only phase semantics to a transmitter descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmSchedulerItemEventType {
    /// One transmitter-test scheduler item.
    Transmitter(DtmSchedulerTransmitterPhy),
    /// One receiver-test scheduler item in its explicit session phase.
    Receiver {
        /// Initial or recurring receiver configuration.
        phase: DtmReceiverEventPhase,
        /// Receiver-valid PHY retained without exposing its field image.
        phy: DtmSchedulerReceiverPhy,
    },
}

impl DtmSchedulerItemEventType {
    /// Return the DTM role selected by this event type.
    pub const fn role(self) -> DtmRole {
        match self {
            Self::Transmitter(_) => DtmRole::Transmitter,
            Self::Receiver { .. } => DtmRole::Receiver,
        }
    }

    /// Return the receiver phase, or `None` for a transmitter event.
    pub const fn receiver_phase(self) -> Option<DtmReceiverEventPhase> {
        match self {
            Self::Transmitter(_) => None,
            Self::Receiver { phase, .. } => Some(phase),
        }
    }

    const fn phy_encoding(self) -> DtmSchedulerPhyEncoding {
        match self {
            Self::Transmitter(phy) => DtmSchedulerPhyEncoding::for_transmitter(phy),
            Self::Receiver { phy, .. } => DtmSchedulerPhyEncoding::for_receiver(phy),
        }
    }
}

/// The nine link-state words whose reset behavior is complete.
///
/// Names are byte offsets, not semantic descriptor fields. The omitted bytes
/// and the hardware consumer remain unresolved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmLinkStateReviewedWords {
    /// Complete word at byte offset `+0x00`; low 20 bits carry the TX head.
    pub(crate) word_00: u32,
    /// Complete word at byte offset `+0x08`; low 20 bits carry the RX tail.
    pub(crate) word_08: u32,
    /// Private word at byte offset `+0x14`; its field-specific hardware
    /// meaning remains unresolved.
    pub(super) profile_word_14: DtmLinkStateProfileWord,
    /// Protocol-level CRC initialization value encoded in the low 24 bits by
    /// the private SRAM codec. The high byte remains opaque and preserved.
    pub(super) crc_init: LeCrcInit,
    /// Complete word at byte offset `+0x30`; the reset rewrites its low
    /// halfword.
    pub(crate) word_30: u32,
    /// Complete word at byte offset `+0x34`.
    pub(crate) word_34: u32,
    /// Protocol-level synchronization value encoded by the private SRAM
    /// codec. This does not grant hardware-consumer authority.
    pub(super) access_address: LeAccessAddress,
    /// Complete word at byte offset `+0x50`.
    pub(crate) word_50: u32,
    /// Complete word at byte offset `+0x60`; byte `+0x61` carries the
    /// transmit-power index.
    pub(crate) word_60: u32,
}

impl DtmLinkStateReviewedWords {
    /// Apply the complete reviewed DTM reset to this CPU-owned word subset.
    ///
    /// SOURCE: pinned `libble_app.a[dtm_4.o]::sym_dtm_7gOGApwmDG1TxZOfQhrh`
    /// (`r_ble_lll_dtm_reset_link_state`). It stores the power index in byte
    /// `+0x61`, rewrites the low halfword of `+0x30` and, for a receiver,
    /// stores the zero tick difference in `+0x34`; unlike the 7f20740 body it
    /// leaves `+0x04` alone.
    ///
    /// All positional encoding stays inside the controller-memory codec. The
    /// caller supplies only validated semantic values and bound graph links.
    pub const fn apply_reset(
        mut self,
        tx_head: Option<DtmTxHeaderHeadProjection>,
        rx_tail: Option<DtmRxHeaderTailProjection>,
        default_tx_power: LeTxPower,
        config: u8,
        role: DtmRole,
    ) -> Self {
        let word_00_with_link = match tx_head {
            Some(tx_head) => tx_head.apply_to_link_state_word(self.word_00),
            None => self.word_00 & !LOW_TWENTY_MASK,
        };
        let transformed_high_half = (((word_00_with_link >> 16) as u16 & 0x600f) | 0x8ff0) as u32;

        self.word_00 = (word_00_with_link & 0x0000_ffff) | (transformed_high_half << 16);
        self.word_08 = match rx_tail {
            Some(rx_tail) => rx_tail.apply_to_link_state_word(self.word_08),
            None => self.word_08 & !LOW_TWENTY_MASK,
        };
        self.word_08 = (self.word_08 & 0xf00f_ffff) | 0x0ff0_0000;
        self.profile_word_14 = self.profile_word_14.select_direct_test_mode();
        let halfword_30 = ((self.word_30 as u16) & 0xc100) | 0x1e00;
        self.word_30 = (self.word_30 & 0xffff_0000) | halfword_30 as u32;
        self.word_60 = LinkStateEventWord::from_word(self.word_60)
            .with_power(default_tx_power)
            .word();
        if matches!(role, DtmRole::Receiver) {
            self.word_34 = 0;
        }
        self.word_50 = (self.word_50 & !LINK_STATE_CONFIG_MASK) | ((config as u32) << 24);
        self.with_crc_init(LeCrcInit::LE_PRESET)
            .with_access_address(LeAccessAddress::DIRECT_TEST_MODE)
    }

    /// Return the transmit-power index a recurring event copies.
    const fn power_index(self) -> u8 {
        LinkStateEventWord::from_word(self.word_60).power_index()
    }

    pub(super) const fn access_address(self) -> LeAccessAddress {
        self.access_address
    }

    pub(super) const fn crc_init(self) -> LeCrcInit {
        self.crc_init
    }

    const fn with_crc_init(mut self, crc_init: LeCrcInit) -> Self {
        self.crc_init = crc_init;
        self
    }

    const fn with_access_address(mut self, access_address: LeAccessAddress) -> Self {
        self.access_address = access_address;
        self
    }
}

/// The ten scheduler-item words whose DTM event transform is complete.
///
/// Names are byte offsets. This is not the complete scheduler object and has
/// no list-linkage or hardware-ownership authority.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmSchedulerItemReviewedWords {
    /// Complete word at byte offset `+0x00`; only byte `+0x02` is transformed.
    pub(crate) word_00: u32,
    /// Complete word at byte offset `+0x04`.
    pub(crate) word_04: u32,
    /// Complete word at byte offset `+0x08`; low 20 bits retain link-state.
    pub(crate) word_08: u32,
    /// Complete sequence-start word at byte offset `+0x0c`.
    pub(crate) word_0c: u32,
    /// Complete word at byte offset `+0x14`.
    pub(crate) word_14: u32,
    /// Complete word at byte offset `+0x18`.
    pub(crate) word_18: u32,
    /// Complete word at byte offset `+0x2c`.
    pub(crate) word_2c: u32,
    /// Complete raw-tick word at byte offset `+0x44`.
    pub(crate) word_44: u32,
    /// Complete raw-tick word at byte offset `+0x48`.
    pub(crate) word_48: u32,
    /// Complete word at byte offset `+0x4c`; only its low byte is cleared.
    pub(crate) word_4c: u32,
}

impl DtmSchedulerItemReviewedWords {
    /// The power index the item carries.
    #[cfg(test)]
    const fn power_index(self) -> u8 {
        crate::link_state_event::item_power_index(self.word_14)
    }

    /// Apply every complete reviewed DTM event transform before insertion.
    ///
    /// `frequency` and `rate` are already validated controller images;
    /// `event_type` carries the role and the receiver's initial/recurring
    /// configuration choice; scheduler times are raw values derived from the
    /// owned epoch.
    pub const fn apply_event(
        self,
        frequency: u8,
        event_type: DtmSchedulerItemEventType,
        scheduler_start: u32,
        scheduler_end: u32,
    ) -> Self {
        let role = event_type.role();
        let current_role_byte = ((self.word_00 >> 16) & 0xff) as u8;
        let role_byte = (current_role_byte & 0xaf)
            | match role {
                DtmRole::Transmitter => 0x10,
                DtmRole::Receiver => 0x40,
            };
        let rate = event_type.phy_encoding().0;

        Self {
            word_00: (self.word_00 & 0xff00_ffff) | ((role_byte as u32) << 16),
            word_04: self.word_04 | 0x8000_0000,
            word_08: self.word_08 & 0xff0f_ffff,
            word_0c: self.word_0c,
            word_14: (self.word_14 & !SCHEDULER_RATE_LANES_MASK) | (rate << 28) | (rate << 30),
            word_18: (self.word_18 & !(SCHEDULER_FREQUENCY_MASK | 0x0f))
                | ((frequency as u32) << 8)
                | 0x03,
            word_2c: match event_type {
                DtmSchedulerItemEventType::Transmitter(_) => self.word_2c,
                DtmSchedulerItemEventType::Receiver {
                    phase: DtmReceiverEventPhase::Initial,
                    ..
                } => INITIAL_RECEIVER_CONFIGURATION_IMAGE,
                DtmSchedulerItemEventType::Receiver {
                    phase: DtmReceiverEventPhase::Recurring,
                    ..
                } => RECURRING_RECEIVER_CONFIGURATION_IMAGE,
            },
            word_44: scheduler_start,
            word_48: scheduler_end,
            word_4c: self.word_4c & 0xffff_ff00,
        }
    }

    /// Store the sequence start the DTM event bodies derive from the item's
    /// own start.
    ///
    /// SOURCE: pinned `libble_app.a[dtm_4.o]` initial event
    /// `sym_dtm_2zeOUjc7g55zkDNQuZkg` and recurring event
    /// `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` store
    /// `r_sched_timer_convertTimeToTicks(r_sched_timer_convertTimeToUs(+0x44))`
    /// in `+0x0c`. They publish the item directly as the list head and leave
    /// the window length at `+0x10` to the allocation, so this codec does not
    /// write it.
    pub const fn apply_sequence_start(mut self, raw_sequence_start: u32) -> Self {
        self.word_0c = raw_sequence_start;
        self
    }

    /// Copy the link state's power index into the item, as only the
    /// recurring event body does.
    ///
    /// SOURCE: pinned `sym_dtm_C15YAGhOCEEdWMnhjaY3.part.1` stores link-state
    /// byte `+0x61` in bits 27:20 of `+0x14`; the initial event body
    /// `sym_dtm_2zeOUjc7g55zkDNQuZkg` writes no power into the item.
    pub const fn apply_recurring_power(mut self, link_state: DtmLinkStateReviewedWords) -> Self {
        self.word_14 = item_with_power(self.word_14, link_state.power_index());
        self
    }
}

/// Complete CPU-side DTM event word subset accepted by the memory graph.
///
/// This aggregate is positional, not proof that the upper DTM transforms were
/// used. The consuming graph transition separately validates its three bound
/// links before writing any word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmPositionalEventWords {
    link_state: DtmLinkStateReviewedWords,
    scheduler_item: DtmSchedulerItemReviewedWords,
}

impl DtmPositionalEventWords {
    /// Pair the two positional word subsets without publishing either object.
    pub const fn new(
        link_state: DtmLinkStateReviewedWords,
        scheduler_item: DtmSchedulerItemReviewedWords,
    ) -> Self {
        Self {
            link_state,
            scheduler_item,
        }
    }

    /// Return the complete reviewed link-state subset.
    pub const fn link_state(self) -> DtmLinkStateReviewedWords {
        self.link_state
    }

    /// Return the complete reviewed scheduler-item subset.
    pub const fn scheduler_item(self) -> DtmSchedulerItemReviewedWords {
        self.scheduler_item
    }

    pub(super) const fn tx_header_head_projection(self) -> DtmTxHeaderHeadProjection {
        DtmTxHeaderHeadProjection::from_link_state_word(self.link_state.word_00)
    }

    pub(super) const fn rx_header_tail_projection(self) -> DtmRxHeaderTailProjection {
        DtmRxHeaderTailProjection::from_link_state_word(self.link_state.word_08)
    }

    pub(super) const fn scheduler_item_retains_link_state(
        self,
        link_state: ControllerSramLinkAddress,
    ) -> bool {
        self.scheduler_item.word_08 & LOW_TWENTY_MASK == link_state.compressed_image()
    }
}

/// Private codec for the complete scheduler-item word containing the
/// hardware-consumed successor link.
///
/// The common scheduler treats the low twenty bits as a compressed successor
/// and preserves the independent prefix. Keeping the complete word inside
/// this type lets the memory owner terminate or roll back the chain without
/// exposing the field image to its typestate transitions.
#[derive(Clone, Copy)]
pub(super) struct DtmSchedulerHardwareChainWord(u32);

impl DtmSchedulerHardwareChainWord {
    pub(super) const fn from_storage(word: u32) -> Self {
        Self(word)
    }

    pub(super) const fn terminate(self) -> Self {
        Self(self.0 & !LOW_TWENTY_MASK)
    }

    pub(super) const fn into_storage(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests;
