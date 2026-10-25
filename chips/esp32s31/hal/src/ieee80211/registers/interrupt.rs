//! The MAC and WDEVPWR interrupt banks, which the radio interrupt handler
//! holds for the image's life.
//!
//! The event-status words are acknowledged only through the generated
//! sample and acknowledgement, and through the generated clear-every-event
//! images that mask their bank first (`mac_interrupt_clear`,
//! `clear_all_mac_power_interrupts`). Every enable word is written by the
//! bank's owner alone: no task-side owner of either bank exists.

use oer_esp32s31_pac::{
    modem_pwr, modem_pwr::WifiMacPowerInterrupt, wifi_mac, wifi_mac::WifiMacInterrupt,
};
use oer_mmio::{Bits, Placement};

use crate::ieee80211::registers::{Chip, MacInterruptMask, MacPlacement, WifiRadioRegisters};

/// Semantic MAC work causes recovered from reviewed vendor transactions.
///
/// This type is deliberately not a register image. The generated PAC owns
/// STATUS field geometry; higher layers can only combine named causes.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptEvents {
    pub(crate) tx_complete: bool,
    pub(crate) bss_color_collision: bool,
    pub(crate) rx_success: bool,
    pub(crate) tx_timeout: bool,
}

impl MacInterruptEvents {
    pub const TX_COMPLETE: Self = Self::from_causes(true, false, false, false);
    pub const BSS_COLOR_COLLISION: Self = Self::from_causes(false, true, false, false);
    pub const RX_SUCCESS: Self = Self::from_causes(false, false, true, false);
    pub const TX_TIMEOUT: Self = Self::from_causes(false, false, false, true);

    pub const fn empty() -> Self {
        Self::from_causes(false, false, false, false)
    }

    pub const fn from_causes(
        tx_complete: bool,
        bss_color_collision: bool,
        rx_success: bool,
        tx_timeout: bool,
    ) -> Self {
        Self {
            tx_complete,
            bss_color_collision,
            rx_success,
            tx_timeout,
        }
    }

    pub const fn union(self, other: Self) -> Self {
        Self::from_causes(
            self.tx_complete || other.tx_complete,
            self.bss_color_collision || other.bss_color_collision,
            self.rx_success || other.rx_success,
            self.tx_timeout || other.tx_timeout,
        )
    }

    pub const fn contains(self, other: Self) -> bool {
        (!other.tx_complete || self.tx_complete)
            && (!other.bss_color_collision || self.bss_color_collision)
            && (!other.rx_success || self.rx_success)
            && (!other.tx_timeout || self.tx_timeout)
    }

    pub const fn is_empty(self) -> bool {
        !self.tx_complete && !self.bss_color_collision && !self.rx_success && !self.tx_timeout
    }

    pub const fn tx_complete(self) -> bool {
        self.tx_complete
    }

    pub const fn bss_color_collision(self) -> bool {
        self.bss_color_collision
    }

    pub const fn rx_success(self) -> bool {
        self.rx_success
    }

    pub const fn tx_timeout(self) -> bool {
        self.tx_timeout
    }
}

impl core::ops::BitOr for MacInterruptEvents {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// Semantic partition of one sampled MAC interrupt image.
///
/// Higher layers route qualified work, acknowledge-only auxiliary events and
/// opaque evidence without applying masks to the sampled register.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacInterruptObservation {
    pub(crate) work_events: MacInterruptEvents,
    pub(crate) auxiliary_event: bool,
    pub(crate) unhandled_event: bool,
}

impl MacInterruptObservation {
    /// Construct semantic evidence for a host-side interrupt implementation.
    ///
    /// No register image enters this API: test doubles name the causes they
    /// simulate in the same vocabulary consumed by the MAC driver.
    pub const fn from_semantic_events(
        work_events: MacInterruptEvents,
        auxiliary_event: bool,
        unhandled_event: bool,
    ) -> Self {
        Self {
            work_events,
            auxiliary_event,
            unhandled_event,
        }
    }

    pub const fn is_empty(self) -> bool {
        self.work_events.is_empty() && !self.auxiliary_event && !self.unhandled_event
    }

    pub const fn work_events(self) -> MacInterruptEvents {
        self.work_events
    }

    pub const fn has_auxiliary_event(self) -> bool {
        self.auxiliary_event
    }

    pub const fn has_unhandled_event(self) -> bool {
        self.unhandled_event
    }
}

/// One sampled MAC interrupt image which can be acknowledged exactly once:
/// the generated sample of `STATUS`.
#[must_use = "an interrupt snapshot must be inspected and acknowledged"]
pub struct MacInterruptSnapshot(wifi_mac::MacInterruptSnapshot);

impl MacInterruptSnapshot {
    pub fn observation(&self) -> MacInterruptObservation {
        let sample = &self.0;
        let work_events = MacInterruptEvents::from_causes(
            sample.tx_complete(),
            sample.bss_color_collision(),
            sample.rx_success(),
            sample.tx_timeout(),
        );
        let auxiliary_event =
            sample.rx_associated_auxiliary_5() || sample.rx_associated_auxiliary_24();
        let unhandled_event = sample.bits_0_4_opaque().get() != 0
            || sample.bit_6_opaque()
            || sample.bits_9_10_opaque().get() != 0
            || sample.watchdog()
            || sample.bit_12_opaque()
            || sample.bit_13_opaque()
            || sample.sta_beacon_filter()
            || sample.bits_16_18_opaque().get() != 0
            || sample.bit_20_opaque()
            || sample.bit_21_opaque()
            || sample.bit_22_opaque()
            || sample.bit_23_opaque()
            || sample.bits_25_26_opaque().get() != 0
            || sample.bit_27_opaque()
            || sample.bit_28_opaque()
            || sample.bits_29_31_opaque().get() != 0;
        MacInterruptObservation::from_semantic_events(work_events, auxiliary_event, unhandled_event)
    }
}

impl core::fmt::Debug for MacInterruptSnapshot {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("MacInterruptSnapshot")
            .field(&self.observation())
            .finish()
    }
}

/// One sampled power-interrupt image with intentionally opaque bit semantics:
/// the generated sample of `POWER_INTERRUPT_STATUS`.
#[must_use = "an interrupt snapshot must be inspected and acknowledged"]
pub struct MacPowerInterruptSnapshot(modem_pwr::MacPowerInterruptSnapshot);

impl MacPowerInterruptSnapshot {
    pub fn observation(&self) -> MacPowerInterruptObservation {
        let sample = &self.0;
        MacPowerInterruptObservation::from_semantic_events(
            sample.tsf_timer_0(),
            sample.tsf_timer_1(),
            sample.tsf_timer_2(),
            sample.tsf_timer_3(),
            sample.tbtt_0(),
            // Only the station TSF's TBTT is ever enabled.
            sample.tbtt_1()
                || sample.tbtt_2()
                || sample.tbtt_3()
                || sample.bits_8_31_opaque().get() != 0,
        )
    }
}

impl core::fmt::Debug for MacPowerInterruptSnapshot {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("MacPowerInterruptSnapshot")
            .field(&self.observation())
            .finish()
    }
}

/// Semantic WDEVPWR causes sampled through generated PAC field accessors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacPowerInterruptObservation(pub(crate) u8);

impl MacPowerInterruptObservation {
    // Driver-local semantic flags. Their positions intentionally do not match
    // WDEVPWR register geometry, which remains behind generated accessors.
    pub(crate) const TSF_TIMER_0: u8 = 0x01;
    pub(crate) const TSF_TIMER_1: u8 = 0x02;
    pub(crate) const TSF_TIMER_2: u8 = 0x04;
    pub(crate) const TSF_TIMER_3: u8 = 0x08;
    pub(crate) const UNHANDLED_EVENT: u8 = 0x10;
    pub(crate) const STA_TBTT: u8 = 0x20;

    pub const fn from_semantic_events(
        tsf_timer_0: bool,
        tsf_timer_1: bool,
        tsf_timer_2: bool,
        tsf_timer_3: bool,
        sta_tbtt: bool,
        unhandled_event: bool,
    ) -> Self {
        Self(
            (if tsf_timer_0 { Self::TSF_TIMER_0 } else { 0 })
                | (if tsf_timer_1 { Self::TSF_TIMER_1 } else { 0 })
                | (if tsf_timer_2 { Self::TSF_TIMER_2 } else { 0 })
                | (if tsf_timer_3 { Self::TSF_TIMER_3 } else { 0 })
                | (if sta_tbtt { Self::STA_TBTT } else { 0 })
                | (if unhandled_event {
                    Self::UNHANDLED_EVENT
                } else {
                    0
                }),
        )
    }

    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn tsf_timer_0(self) -> bool {
        self.0 & Self::TSF_TIMER_0 != 0
    }

    pub const fn tsf_timer_1(self) -> bool {
        self.0 & Self::TSF_TIMER_1 != 0
    }

    pub const fn tsf_timer_2(self) -> bool {
        self.0 & Self::TSF_TIMER_2 != 0
    }

    pub const fn tsf_timer_3(self) -> bool {
        self.0 & Self::TSF_TIMER_3 != 0
    }

    /// The station TSF reached its target beacon transmission time.
    pub const fn sta_tbtt(self) -> bool {
        self.0 & Self::STA_TBTT != 0
    }

    pub const fn has_unhandled_event(self) -> bool {
        self.0 & Self::UNHANDLED_EVENT != 0
    }
}

/// Proof that the connected-STA hardware policy was applied.
///
/// Construction is private to the CONTROL transaction below.
#[must_use = "connected STA interrupt activation requires this preparation proof"]
pub struct ConnectedStaWithoutPowerSavePrepared {
    _private: (),
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Disable vendor hardware beacon filtering before a connected STA epoch.
    ///
    /// SOURCE: complete `libpp[hal_mac.o]::hal_disable_sta_beacon_filter`
    /// clears CONTROL bits 2:0, then the matching interrupt-enable bit 15.
    /// The enable word belongs to the interrupt handler: every image it
    /// publishes holds bit 15 clear ([`publish_mac_interrupt_mask`]), and it
    /// stays masked between epochs, so the source is masked before and after
    /// this edge.
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
    ) -> ConnectedStaWithoutPowerSavePrepared {
        self.mac
            .wifi_mac_sta_beacon_filter
            .modify_sta_beacon_filter_control(|w| {
                w.set_bits_0_2_opaque(Bits::constant::<0>());
            });
        ConnectedStaWithoutPowerSavePrepared { _private: () }
    }
}

/// Classified field-level readback of the MAC interrupt enable register.
///
/// Unknown images are retained as an explicit state instead of reconstructing
/// a handwritten integer register image from the generated field accessors.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MacInterruptEnableState {
    /// Every interrupt-enable field is clear.
    Disabled,
    /// Station mask with the independent HE BSS-color event disabled.
    Station,
    /// A field combination outside the reviewed writable images.
    Unknown,
}

/// Publish the whole MAC interrupt enable image of `event_mask` in one
/// read-modify-write that sets every field: the vendor's whole-word store.
///
/// SOURCE: `BLOB_LIBPP_HAL_ENABLE_MAC` (`mask_mac_interrupts`, the zero
/// image) and `BLOB_LIBPP_HAL_INIT_TAIL`, `POLICY_STATION_BSS_COLOR_MASK`
/// (`enable_station_mac_interrupts`, the station image `0x19a8_78e0`: the
/// cold receive image with the HE BSS-color event disabled).
/// [`MacInterruptMask`] holds only those two images.
fn publish_mac_interrupt_mask<P: Placement>(
    interrupt: &mut WifiMacInterrupt<P>,
    event_mask: MacInterruptMask,
) {
    let station = event_mask != MacInterruptMask::NONE;
    interrupt.modify_enable(|w| {
        w.set_bits_0_4_opaque(Bits::constant::<0>())
            .set_rx_associated_auxiliary_5(station)
            .set_bit_6_opaque(station)
            .set_tx_complete(station)
            .set_bss_color_collision(false)
            .set_bits_9_10_opaque(Bits::constant::<0>())
            .set_watchdog(station)
            .set_bit_12_opaque(station)
            .set_bit_13_opaque(station)
            .set_rx_success(station)
            .set_sta_beacon_filter(false)
            .set_bits_16_18_opaque(Bits::constant::<0>())
            .set_tx_timeout(station)
            .set_bit_20_opaque(false)
            .set_bit_21_opaque(station)
            .set_bit_22_opaque(false)
            .set_bit_23_opaque(station)
            .set_rx_associated_auxiliary_24(station)
            .set_bits_25_26_opaque(Bits::constant::<0>())
            .set_bit_27_opaque(station)
            .set_bit_28_opaque(station)
            .set_bits_29_31_opaque(Bits::constant::<0>());
    });
}

/// Mask every WDEVPWR cause: the whole enable image zero, in one
/// read-modify-write that sets every field.
///
/// SOURCE: `BLOB_LIBPP_HAL_TSF_TIMER` (`mask_mac_power_interrupts`).
fn mask_mac_power_interrupts<P: Placement>(power: &mut WifiMacPowerInterrupt<P>) {
    power.modify_enable(|w| {
        w.set_tbtt_3(false)
            .set_tbtt_2(false)
            .set_tbtt_1(false)
            .set_tbtt_0(false)
            .set_tsf_timer_3(false)
            .set_tsf_timer_2(false)
            .set_tsf_timer_1(false)
            .set_tsf_timer_0(false)
            .set_bits_8_31_opaque(Bits::constant::<0>());
    });
}

fn observe_mac_interrupt_enable<P: Placement>(
    interrupt: &WifiMacInterrupt<P>,
) -> MacInterruptEnableState {
    let enable = interrupt.read_enable();
    let fixed_clear = enable.bits_0_4_opaque().get() == 0
        && !enable.bss_color_collision()
        && enable.bits_9_10_opaque().get() == 0
        && !enable.sta_beacon_filter()
        && enable.bits_16_18_opaque().get() == 0
        && !enable.bit_20_opaque()
        && !enable.bit_22_opaque()
        && enable.bits_25_26_opaque().get() == 0
        && enable.bits_29_31_opaque().get() == 0;
    let station_fields = [
        enable.rx_associated_auxiliary_5(),
        enable.bit_6_opaque(),
        enable.tx_complete(),
        enable.watchdog(),
        enable.bit_12_opaque(),
        enable.bit_13_opaque(),
        enable.rx_success(),
        enable.tx_timeout(),
        enable.bit_21_opaque(),
        enable.bit_23_opaque(),
        enable.rx_associated_auxiliary_24(),
        enable.bit_27_opaque(),
        enable.bit_28_opaque(),
    ];
    if fixed_clear && station_fields.iter().all(|set| !set) {
        MacInterruptEnableState::Disabled
    } else if fixed_clear && station_fields.iter().all(|set| *set) {
        MacInterruptEnableState::Station
    } else {
        MacInterruptEnableState::Unknown
    }
}

/// The WDEVPWR interrupt bank: its enable, status and clear words.
///
/// The radio interrupt handler holds it, beside [`MacInterruptRegisters`],
/// for the image's life; no other owner of these words exists.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct MacPowerInterruptRegisters<M: MacPlacement = Chip> {
    power: WifiMacPowerInterrupt<M::Power>,
}

impl<M: MacPlacement> MacPowerInterruptRegisters<M> {
    /// Take the bank. This performs no MMIO.
    pub(crate) const fn new(power: WifiMacPowerInterrupt<M::Power>) -> Self {
        Self { power }
    }

    /// Mask every WDEVPWR cause without acknowledging one.
    ///
    /// Publishes the all-zero ENABLE image, then orders the write. A
    /// latched cause stays in STATUS; the CPU line deasserts once the masked
    /// image reaches the bank.
    pub fn mask_all_power_interrupts(&mut self) {
        mask_mac_power_interrupts(&mut self.power);
        oer_riscv::fence();
    }

    /// Acknowledge every latched WDEVPWR cause: the generated image masks
    /// the bank and stores the whole clear word, then the write is ordered.
    ///
    /// SOURCE: `BLOB_LIBPP_HAL_PWR_INTERRUPT`.
    pub fn clear_all_power_interrupts(&mut self) {
        self.power.clear_all_mac_power_interrupts();
        oer_riscv::fence();
    }

    /// Enable the station TBTT event, then order the write.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_enable_sta_tbtt`
    /// enables the station TBTT event in the WDEVPWR enable word. One
    /// interrupt epoch serves every Wi-Fi role in turn, so the mask stays
    /// enabled and the station TBTT enable, which only a connected station
    /// sets, gates it.
    pub fn enable_station_tbtt_interrupt(&mut self) {
        self.power.modify_enable(|w| {
            w.set_tbtt_0(true);
        });
        oer_riscv::fence();
    }

    /// Sample the complete masked WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp[hal_tsf.o]::hal_pwr_interrupt_get_event`.
    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        MacPowerInterruptSnapshot(self.power.sample_mac_power_interrupt())
    }

    /// Acknowledge the complete sampled WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp[hal_tsf.o]::hal_pwr_interrupt_clr_event`
    /// stores its argument to the clear word.
    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        self.power.acknowledge_mac_power_interrupt(snapshot.0);
        oer_riscv::fence();
    }
}

/// The MAC interrupt bank: its enable, status and clear words.
///
/// The radio interrupt handler holds it, beside
/// [`MacPowerInterruptRegisters`], for the image's life; no other owner of
/// these words exists.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct MacInterruptRegisters<M: MacPlacement = Chip> {
    pub(crate) mac: WifiMacInterrupt<M::Mac>,
}

impl<M: MacPlacement> MacInterruptRegisters<M> {
    /// Take the bank. This performs no MMIO.
    pub(crate) const fn new(mac: WifiMacInterrupt<M::Mac>) -> Self {
        Self { mac }
    }

    /// Mask every MAC interrupt source without acknowledging one.
    ///
    /// Publishes the all-zero ENABLE image, then orders the write. A
    /// latched event stays in STATUS. [`Self::unmask_rx_delivery_interrupts`]
    /// afterwards restores only the RX delivery group.
    pub fn mask_all_mac_interrupts(&mut self) {
        publish_mac_interrupt_mask(&mut self.mac, MacInterruptMask::NONE);
        oer_riscv::fence();
    }

    /// Acknowledge every latched MAC event: the generated image stores the
    /// zero enable image and then the whole clear word, then the write is
    /// ordered.
    ///
    /// SOURCE: `BLOB_LIBPP_HAL_MAC_INTERRUPT_CLR_EVENT`.
    pub fn clear_all_mac_interrupts(&mut self) {
        self.mac.mac_interrupt_clear();
        oer_riscv::fence();
    }

    /// Publish one reviewed MAC interrupt mask, then order the write.
    pub fn publish_mac_interrupt_mask(&mut self, event_mask: MacInterruptMask) {
        publish_mac_interrupt_mask(&mut self.mac, event_mask);
        oer_riscv::fence();
    }

    /// Read the currently published MAC interrupt mask.
    pub fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        observe_mac_interrupt_enable(&self.mac)
    }

    /// Sample the complete MAC interrupt status image.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_get_event` proves the
    /// status address and complete `libpp[wdev.o]::wDev_ProcessFiq` consumes exactly this
    /// status image. The runtime mask is configured before IRQ activation and
    /// is not sampled by the vendor FIQ transaction.
    pub fn mac_interrupt_status(&self) -> MacInterruptSnapshot {
        MacInterruptSnapshot(self.mac.sample_mac_interrupt())
    }

    /// Acknowledge the complete sampled event image, then order the ISR edge.
    ///
    /// SOURCE: complete `libpp.a::hal_mac_interrupt_clr_event` is one
    /// full-width store to the write-to-clear register.
    pub fn acknowledge_mac_interrupts(&mut self, snapshot: MacInterruptSnapshot) {
        self.mac.acknowledge_mac_interrupt(snapshot.0);
        oer_riscv::fence();
    }

    /// Temporarily suppress the observed RX delivery group without changing
    /// any other member of the reviewed runtime mask.
    ///
    /// This is a source-moderation operation, not an acknowledgement. The
    /// hard ISR uses it before acknowledging the RX image. The group contains
    /// RX_SUCCESS and the two auxiliary sources observed on every saturated
    /// RX edge; TX and unrelated sources remain independently live.
    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.set_rx_delivery_interrupts(false);
    }

    /// Restore the RX delivery group while preserving the current mask.
    ///
    /// A latched RX status which arrived while masked becomes visible through
    /// the level CPU route after this ordered write, so the task does not need
    /// to fabricate a hardware completion edge.
    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.set_rx_delivery_interrupts(true);
    }

    fn set_rx_delivery_interrupts(&mut self, enabled: bool) {
        self.mac.modify_enable(|w| {
            w.set_rx_associated_auxiliary_5(enabled)
                .set_rx_success(enabled)
                .set_rx_associated_auxiliary_24(enabled);
        });
        oer_riscv::fence();
    }
}

#[cfg(test)]
mod tests;
