//! The A-MPDU limits and program of the lower-MAC core.
//!
//! The core's data arenas ([`super::arena`]) publish each A-MPDU through a
//! [`RetainedDmaAmpduTx`] chain whose descriptor words were bound at intake:
//! [`AmpduBudget`] holds the aggregate to its rate's length, the core derives
//! its program ([`AmpduSettings`]) and [`submit_planned`] starts it.
//!
//! # Limits
//!
//! HT and HE SU, at most `SLOTS` subframes and, at each rate, the octets
//! [`esp32s31_ampdu_length`] gives ([`ESP32S31_AMPDU_LENGTHS`] is the same
//! limit as the port's table). The HE path carries the recipient's density
//! and the access category's TXOP limit in its checked plan. Trigger-based
//! aggregates stay outside the declared formats; they require a separate
//! Trigger reservation.

use crate::mac::tx::{
    HeAmpduTxConfig, HtAmpduDensity, HtChannelWidth, HtGuardInterval, HtMcs, HtProtectionSpacing,
    HtRate, LegacyTxQueue, TxControlFrame, TxCookie,
    ampdu::{HeAmpduPolicy, HtAmpduHardware, HtAmpduTxError, RetainedDmaAmpduTx},
};
use oer_chips_esp32s31_hal::system::memory::dma_static::PlacedBacking;
use oer_protocols_ieee80211_contracts_port::{AmpduCapabilities, AmpduLengthTable, PhyFormatSet};

use crate::ampdu_tx::{AmpduTxRoleAdapter, HtAmpduPublicationInputs, ht_ampdu_publication_config};

/// The longest aggregate the core publishes at any rate: the 16 bits of the
/// HT-SIG HT Length field (IEEE Std 802.11-2020, 19.3.9.4.3). Every rate's
/// length ([`esp32s31_ampdu_length`]) lies within it, and the aggregate
/// owner is configured to it; [`AmpduBudget`] holds each aggregate to its
/// rate's length.
pub const ESP32S31_AMPDU_MAX_LENGTH: u16 = u16::MAX;

/// The longest aggregate the core publishes at `rate`: at an HT rate the
/// vendor's per-rate ceiling (`rx11NRate2AMPDULimit`, see
/// [`HtRate::vendor_ampdu_byte_limit`]), or [`ESP32S31_AMPDU_MAX_LENGTH`]
/// where the vendor sets none; at an HE rate the ROM's APEP of the rate
/// and guard interval at the default TXOP
/// ([`HeRate::checked_maximum_apep_bytes`](crate::mac::tx::HeRate::checked_maximum_apep_bytes)),
/// within [`ESP32S31_AMPDU_MAX_LENGTH`]: what [`AmpduBudget`] takes when
/// the access category has no TXOP limit, while a TXOP limit in force
/// bounds it further; `None` at a legacy rate.
///
/// SOURCE(esp32s31): `libpp[trc.o]::rcSetTxAmpduLimit` stores the per-rate
/// ceiling (`rcGetAmpduSched+0x90`, `rx11NRate2AMPDULimit`) beside the
/// peer's length at the controller's `+0x80` and `+0x7e`, which only the HT
/// check `libpp[pp.o]::ppCheckTxAMPDUlength` (`+0x38`, `+0x4e`) reads. The HE
/// check `libpp[pp.o]::ppCheckTxHEAMPDUlength` bounds an aggregate by the
/// BlockAck window (`+0x6c`) and by `libpp[trc.o]::rx11AXRate2AMPDULimit`
/// (`+0x154`), the APEP of the rate within its access category's TXOP
/// (`g_he_max_apep_length_tab`, filled by
/// `rx11AXRate2AMPDULimit_update` from `he_max_apep_length` or the TXOP).
pub const fn esp32s31_ampdu_length(rate: crate::mac::tx::TxPhyRate) -> Option<u16> {
    use crate::mac::tx::TxPhyRate;
    match rate {
        TxPhyRate::Ht(rate) => Some(match rate.vendor_ampdu_byte_limit() {
            Some(limit) => limit,
            None => ESP32S31_AMPDU_MAX_LENGTH,
        }),
        TxPhyRate::He(rate) => {
            match rate.checked_maximum_apep_bytes(crate::mac::tx::HeEdcaTxopLimit::DEFAULT) {
                Some(apep) if apep < ESP32S31_AMPDU_MAX_LENGTH as u32 => Some(apep as u16),
                Some(_) => Some(ESP32S31_AMPDU_MAX_LENGTH),
                None => None,
            }
        }
        TxPhyRate::Legacy(_) => None,
    }
}

/// [`esp32s31_ampdu_length`] at every portable rate the transmit formatter
/// sends (HT MCS 0 to 7 at 20 and 40 MHz with either guard interval, HE20
/// SU MCS 0 to 9 with every guard interval); zero at every other rate. An
/// HE entry holds the rate without DCM, which halves the APEP: the rates
/// the port is given carry none, and a DCM aggregate the plan sized above
/// its APEP is refused with its owners returned.
pub static ESP32S31_AMPDU_LENGTHS: AmpduLengthTable = esp32s31_ampdu_lengths();

const fn esp32s31_ampdu_lengths() -> AmpduLengthTable {
    use crate::mac::tx::TxPhyRate;
    // The vendor's selector takes the rate code alone: both widths share
    // one length.
    const fn ht(mcs: u8, guard: HtGuardInterval) -> u32 {
        let Some(mcs) = HtMcs::from_index(mcs) else {
            panic!("HT MCS 0 to 7");
        };
        match esp32s31_ampdu_length(TxPhyRate::Ht(HtRate::new(
            mcs,
            guard,
            HtChannelWidth::Mhz20,
        ))) {
            Some(length) => length as u32,
            None => 0,
        }
    }
    let mut table = AmpduLengthTable::uniform(0);
    let mut mcs = 0;
    while mcs <= HtMcs::Mcs7.index() {
        let long = ht(mcs, HtGuardInterval::Long800Ns);
        let short = ht(mcs, HtGuardInterval::Short400Ns);
        let index = mcs as usize;
        table.ht[0][0][index] = long;
        table.ht[1][0][index] = long;
        table.ht[0][1][index] = short;
        table.ht[1][1][index] = short;
        mcs += 1;
    }
    // The table's guard intervals: 0.8 µs (with either HE-LTF size, whose
    // APEP is one), 1.6 µs and 3.2 µs.
    const fn he(mcs: u8, guard: crate::mac::rx::HeGuardIntervalAndLtf) -> u32 {
        let Some(mcs) = crate::mac::tx::HeMcs::from_index(mcs) else {
            panic!("HE MCS 0 to 9");
        };
        match esp32s31_ampdu_length(TxPhyRate::He(crate::mac::tx::HeRate::new(mcs, guard))) {
            Some(length) => length as u32,
            None => 0,
        }
    }
    use crate::mac::rx::HeGuardIntervalAndLtf as Gi;
    let mut mcs = 0;
    while mcs < 10 {
        let index = mcs as usize;
        let one = he(mcs, Gi::OneLtf800Ns);
        let two = he(mcs, Gi::TwoLtf800Ns);
        table.he[0][0][index] = if one < two { one } else { two };
        table.he[0][1][index] = he(mcs, Gi::TwoLtf1600Ns);
        table.he[0][2][index] = he(mcs, Gi::FourLtf3200Ns);
        mcs += 1;
    }
    table
}

/// The limits of the core's aggregates of up to `slots` subframes.
pub const fn esp32s31_ampdu_capabilities(slots: usize) -> AmpduCapabilities {
    AmpduCapabilities {
        max_subframes: if slots > u16::MAX as usize {
            u16::MAX
        } else {
            slots as u16
        },
        formats: PhyFormatSet::HT.union(PhyFormatSet::HE),
        max_length: &ESP32S31_AMPDU_LENGTHS,
    }
}

/// The length rule of one aggregate: what [`super::LowerMacCore::plan_ampdu`]
/// checks, from the rate, the access category's advertised TXOP limit an HE
/// aggregate carries and the recipient's density, and what a port checks
/// subframe by subframe before it accepts one into an aggregate.
///
/// The empty delimiters an HE subframe needs grow with the density, so a
/// budget at the widest density (16 µs) admits nothing the recipient's own
/// density refuses.
pub struct AmpduBudget {
    budget: crate::mac::tx::ampdu::HtAmpduLengthAccumulator,
    he: Option<HeAmpduPolicy>,
}

impl AmpduBudget {
    /// The widest MPDU start spacing (16 µs, IEEE encoding 7).
    pub const WIDEST_DENSITY: HtAmpduDensity = HtAmpduDensity::from_spacing_code(7);

    /// An empty budget of up to `slots` subframes at `rate`.
    ///
    /// # Errors
    ///
    /// A non-HT rate, or an HE TXOP limit with no room for any subframe.
    pub fn new(
        rate: crate::mac::tx::TxPhyRate,
        txop_limit_units_32_us: u16,
        density: HtAmpduDensity,
        slots: usize,
    ) -> Result<Self, super::AmpduRefusal> {
        use crate::mac::tx::{HeEdcaTxopLimit, TxPhyRate, ampdu::HtAmpduLengthAccumulator};
        let max_subframes = u8::try_from(slots.min(32)).unwrap_or(32);
        let (max_bytes, he) = match rate {
            TxPhyRate::Ht(_) => (
                esp32s31_ampdu_length(rate).ok_or(super::AmpduRefusal::LegacyRate)?,
                None,
            ),
            TxPhyRate::He(rate) => {
                let txop = crate::mac::tx::runtime::he_txop_limit(
                    txop_limit_units_32_us,
                    HeEdcaTxopLimit::DEFAULT,
                )
                .map_err(|_| super::AmpduRefusal::Length)?;
                let policy = HeAmpduPolicy::new(rate, density, txop);
                let apep = rate
                    .checked_maximum_apep_bytes(txop)
                    .ok_or(super::AmpduRefusal::Length)?;
                let max_bytes = u16::try_from(apep).unwrap_or(ESP32S31_AMPDU_MAX_LENGTH);
                (max_bytes, Some(policy))
            }
            TxPhyRate::Legacy(_) => return Err(super::AmpduRefusal::LegacyRate),
        };
        let budget = HtAmpduLengthAccumulator::new(max_subframes, max_bytes)
            .map_err(|_| super::AmpduRefusal::Length)?;
        Ok(Self { budget, he })
    }

    /// The HE policy the budget follows; `None` for HT.
    pub const fn he_policy(&self) -> Option<HeAmpduPolicy> {
        self.he
    }

    /// The longest aggregate the budget admits.
    pub const fn max_bytes(&self) -> u16 {
        self.budget.max_bytes()
    }

    /// Count one more subframe of `psdu` octets: its MPDU with the hardware
    /// MIC and FCS.
    ///
    /// # Errors
    ///
    /// The aggregate would grow past the format's length or subframe limit;
    /// the budget is unchanged.
    pub fn push(&mut self, psdu: usize) -> Result<(), super::AmpduRefusal> {
        let delimiters = match self.he {
            None => 0,
            Some(policy) => {
                let psdu = u16::try_from(psdu).map_err(|_| super::AmpduRefusal::Length)?;
                policy
                    .rate()
                    .ampdu_empty_delimiters(psdu, policy.density())
                    .ok_or(super::AmpduRefusal::Length)?
            }
        };
        let psdu = u32::try_from(psdu).map_err(|_| super::AmpduRefusal::Length)?;
        self.budget
            .push(psdu, delimiters)
            .map_err(|_| super::AmpduRefusal::Length)
    }
}

/// The aggregate owner behind one buffer. The port publishes only
/// referenced subframes, so its formatter workspace is empty.
pub type Esp32s31AmpduOwner<'slot, B, const SLOTS: usize> = RetainedDmaAmpduTx<'slot, B, SLOTS>;

/// Register values already derived from the owned admission.
#[derive(Clone, Copy)]
pub(super) struct AmpduSettings {
    pub queue: LegacyTxQueue,
    pub format: AmpduFormat,
    pub role: AmpduTxRoleAdapter,
    /// Everything but the aggregate's length and subframe count, which the
    /// owner computes as the subframes are committed.
    pub publication: AmpduPublication,
}

/// The PPDU format of an aggregate and what it alone needs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AmpduFormat {
    Ht {
        rate: HtRate,
        protection_spacing: HtProtectionSpacing,
    },
    /// An HE SU aggregate: its rate, recipient density and TXOP limit, and
    /// the BSS color every HE PPDU of the station carries.
    He {
        policy: HeAmpduPolicy,
        bss_color: u8,
    },
}

/// What an aggregate's publication shares in both formats.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AmpduPublication {
    pub data_power_primary: u8,
    pub data_power_alternate: u8,
    pub control: TxControlFrame,
    pub aifsn: u8,
    pub contention_window: u16,
    pub scheduler_priority: u8,
    pub packet_priority: u8,
}

/// The owner's protection spacing for a recipient's Minimum MPDU Start
/// Spacing encoding (IEEE 0-7, bits 4:2 of its A-MPDU Parameters).
pub const fn protection_spacing(min_mpdu_start_spacing: u8) -> HtProtectionSpacing {
    HtProtectionSpacing::from_density(HtAmpduDensity::from_spacing_code(min_mpdu_start_spacing))
}

/// Program and start the reserved batch `cookie` of `owner`, `aggregate`
/// long, as the aggregate `plan` describes.
pub(super) fn submit_planned<L, H, const SLOTS: usize>(
    hardware: &mut H,
    owner: &mut Esp32s31AmpduOwner<'_, L, SLOTS>,
    cookie: TxCookie,
    plan: &AmpduSettings,
    aggregate: crate::mac::tx::ampdu::HtAmpduLength,
) -> Result<(), HtAmpduTxError>
where
    L: PlacedBacking,
    H: HtAmpduHardware,
{
    let publication = plan.publication;
    match plan.format {
        AmpduFormat::Ht {
            rate,
            protection_spacing,
        } => {
            let config = ht_ampdu_publication_config(
                plan.role,
                HtAmpduPublicationInputs {
                    rate,
                    aggregate_length: aggregate.bytes,
                    subframes: aggregate.subframes,
                    protection_spacing,
                    data_power_primary: publication.data_power_primary,
                    data_power_alternate: publication.data_power_alternate,
                    control: publication.control,
                    aifsn: publication.aifsn,
                    contention_window: publication.contention_window,
                    scheduler_priority: publication.scheduler_priority,
                    packet_priority: publication.packet_priority,
                },
            )
            .ok_or(HtAmpduTxError::AggregateConfigurationMismatch)?;
            owner.submit(hardware, cookie, plan.queue, config)
        }
        AmpduFormat::He { policy, bss_color } => {
            let mut config = HeAmpduTxConfig::new_with_txop(
                policy.rate(),
                bss_color,
                aggregate.bytes,
                aggregate.subframes,
                policy.density(),
                policy.txop_limit(),
            )
            .ok_or(HtAmpduTxError::AggregateConfigurationMismatch)?;
            config.data_power_primary = publication.data_power_primary;
            config.data_power_alternate = publication.data_power_alternate;
            config.control = publication.control;
            config.aifsn = publication.aifsn;
            config.contention_window = publication.contention_window;
            config.interface = plan.role.interface;
            config.scheduler_priority = publication.scheduler_priority;
            config.pti = publication.packet_priority;
            config.pti_count = 1;
            config.hardware_key_selector = plan.role.hardware_key_selector;
            owner.submit_he(hardware, cookie, plan.queue, config)
        }
    }
}
