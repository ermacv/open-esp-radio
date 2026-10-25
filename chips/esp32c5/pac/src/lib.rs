//! Closed typed ESP32-C5 radio peripheral access crate.
//!
//! The generated capability catalog comes from the reviewed
//! [register publication](../../registers/README.md). Owners
//! that expose its transactions are added per domain.

#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod test_memory;

#[cfg(test)]
mod typed_register_tests;

#[cfg(test)]
mod soc_tests;

// The generated capability catalog is intentionally broader than this crate's
// restricted ownership facade. Reviewed leaves stay unreachable until an owner
// transition exposes them.
#[allow(
    dead_code,
    reason = "generated capability catalog is wider than the restricted ownership facade"
)]
mod generated;

/// Every peripheral of the vendor SVD the curated model does not describe,
/// as typed register blocks with the chip's interrupt sources
/// (`soc::Interrupt`); the PAC root claims them at once.
pub use generated::soc;

/// What the pinned ESP-IDF headers take away from the vendor SVD's API.
///
/// SOC_ETM's event status is write-one-to-clear (`soc_etm_reg.h:2297`): it is
/// sampled and acknowledged,
///
/// ```
/// use oer_esp32c5_pac::soc::soc_etm::SocEtm;
/// fn acknowledge(etm: &mut SocEtm<oer_mmio::Host<'_>>) {
///     let sample = etm.sample_evt_st0();
///     etm.acknowledge_evt_st0(sample);
/// }
/// ```
///
/// never modified:
///
/// ```compile_fail
/// use oer_esp32c5_pac::soc::soc_etm::SocEtm;
/// fn modify(etm: &mut SocEtm<oer_mmio::Host<'_>>) {
///     etm.modify_evt_st0(|_| {});
/// }
/// ```
///
/// PCR.CTRL_CLK_OUT_EN bit 6 is the 480 MHz output (`pcr_reg.h:2163`),
///
/// ```
/// use oer_esp32c5_pac::soc::pcr::Pcr;
/// fn enable(pcr: &mut Pcr<oer_mmio::Host<'_>>) {
///     pcr.modify_ctrl_clk_out_en(|clocks| {
///         clocks.set_clk_480m_oen(true);
///     });
/// }
/// ```
///
/// not the SVD's 320 MHz one,
///
/// ```compile_fail
/// use oer_esp32c5_pac::soc::pcr::Pcr;
/// fn enable(pcr: &mut Pcr<oer_mmio::Host<'_>>) {
///     pcr.modify_ctrl_clk_out_en(|clocks| {
///         clocks.set_clk_320m_oen(true);
///     });
/// }
/// ```
///
/// and its bits 7..9 are reserved (`pcr_reg.h:2170`, `:2177`, `:2184`):
///
/// ```compile_fail
/// use oer_esp32c5_pac::soc::pcr::Pcr;
/// fn enable(pcr: &mut Pcr<oer_mmio::Host<'_>>) {
///     pcr.modify_ctrl_clk_out_en(|clocks| {
///         clocks.set_clk_adc_inf_oen(true);
///     });
/// }
/// ```
///
/// TIMG has one hardware timer (`timer_group_struct.h:607-608`): its first
/// is read,
///
/// ```
/// use oer_esp32c5_pac::soc::timg0::Timg0;
/// fn config(timg: &Timg0<oer_mmio::Host<'_>>) -> bool {
///     timg.read_t0config().t0_en()
/// }
/// ```
///
/// the vendor SVD's second has no API:
///
/// ```compile_fail
/// use oer_esp32c5_pac::soc::timg0::Timg0;
/// fn alarm(timg: &Timg0<oer_mmio::Host<'_>>) {
///     let _ = timg.read_t1alarmlo();
/// }
/// ```
#[cfg(doctest)]
pub struct SocHeaderSemantics;

pub(crate) mod ieee802154;
pub(crate) mod modem;
pub(crate) mod ownership;
pub(crate) mod phy;
pub mod platform;
pub mod watchdog;

pub(crate) use ownership::device_fence;

/// A field of at most eight bits read back, as the byte the shared
/// IEEE 802.15.4 value types take.
pub(crate) const fn field_byte<const WIDTH: u32>(bits: oer_mmio::Bits<WIDTH>) -> u8 {
    const { assert!(WIDTH <= 8, "the field fits a byte") };
    bits.get().to_le_bytes()[0]
}

#[doc(hidden)]
pub use ieee802154::mac::{Ieee802154PolledRegisterLease, Ieee802154RegisterLease};

pub use ieee802154::mac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154DebugCounter, Ieee802154EdCcaSnapshot,
    Ieee802154EdCommand, Ieee802154EdDurationUnits, Ieee802154EdSampleMode, Ieee802154EdSampleRate,
    Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventEnableState,
    Ieee802154EventMask, Ieee802154EventObservation, Ieee802154EventObservationError,
    Ieee802154FoundationSnapshot, Ieee802154FrequencyCode, Ieee802154MacCommand,
    Ieee802154MacConfigurationReadback, Ieee802154MacControl, Ieee802154MacPolicySnapshot,
    Ieee802154MultipanEnableState, Ieee802154MultipanIndex, Ieee802154ObservedEventState,
    Ieee802154OperationEventEnableObservation, Ieee802154OperationRxAbortEnableObservation,
    Ieee802154PanIdentity, Ieee802154Pti, Ieee802154RouteState, Ieee802154RxAbortEnableSet,
    Ieee802154RxAbortEnableState, Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation,
    Ieee802154RxStateCode, Ieee802154RxStatus, Ieee802154SecurityPayloadOffset,
    Ieee802154StateSnapshot, Ieee802154Timer0ThresholdWord, Ieee802154Timer0ValueWord,
    Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord, Ieee802154TimerLease,
    Ieee802154TransmitSecurityControl, Ieee802154TxAbortEnableSet, Ieee802154TxAbortReason,
    Ieee802154TxAbortReasonObservation, Ieee802154TxPowerCode, Ieee802154TxSecurityError,
    Ieee802154TxSecurityErrorObservation, Ieee802154TxStateCode, Ieee802154TxStatus,
    Ieee802154ValidationEdDurationState, Ieee802154ValidationEventEnableState,
};

/// The typed IEEE 802.15.4 MAC block, for the register probe's raw writes.
#[cfg(oer_probe)]
#[doc(hidden)]
pub use generated::ieee802154_mac;
pub use generated::{PhyRxEvmParameter, layouts};
pub use modem::clock::{Ieee802154ResetObservation, ModemClockDevice, ModemClockRegisters};
pub use modem::etm::Ieee802154EtmChannels;
pub use ownership::{
    Ieee802154InterruptRegisters, Ieee802154InterruptSetup, Ieee802154Partition,
    Ieee802154TaskRegisters, RadioHandoff, RadioPartitions, WifiInterruptPartition,
    WifiMacPartition,
};
pub use phy::i2c::PhyI2cRegisters;
#[cfg(any(test, feature = "test-support", oer_probe))]
pub use phy::i2c::{
    PHY_I2C_INITIALIZATION_PAIR_COUNT, PhyI2cAccessError, PhyI2cClockSelection,
    PhyI2cConfiguration, PhyI2cConfigurationCommand, PhyI2cInitializationInputs,
    PhyI2cParallelWrite, PhyI2cRcCalibration, PhyI2cSar2Code,
};
pub use phy::radio::PhyRadioRegisters;

// Generated analog-I2C identities, fields and transport routes.
include!("analog_generated.rs");
