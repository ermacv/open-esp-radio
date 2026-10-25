//! The MAC interrupt banks: the task-side setup of one interrupt epoch and
//! the hard interrupt handlers' disjoint capabilities.
//!
//! The MAC event-status register is written only by acknowledging a sample
//! of it, but for the clear-every-latched-event images the vendor stores
//! while a bank is masked (`mac_interrupt_clear`,
//! `clear_all_mac_power_interrupts`).

use oer_esp32s31_pac::{MacInterruptMask, wifi_mac_interrupt, wifi_mac_power_interrupt};
use oer_mmio::Bits;

use super::{Chip, MacInterruptSetup, MacPlacement, WifiRadioRegisters};

/// Proof that the connected-STA hardware policy was applied before IRQ activation.
///
/// Construction is private to the exact two-register transaction below. The
/// connected runtime consumes this value when it activates its interrupt
/// epoch, so removing the transaction from that lifecycle becomes a compile
/// error rather than an intermittent runtime regression.
#[must_use = "connected STA interrupt activation requires this preparation proof"]
pub struct ConnectedStaWithoutPowerSavePrepared {
    _private: (),
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

/// Write the whole MAC interrupt enable image of `event_mask` in one
/// read-modify-write that sets every field.
///
/// SOURCE: `BLOB_LIBPP_HAL_ENABLE_MAC` (`hal_enable_mac` publishes its
/// argument) and `BLOB_LIBPP_HAL_DISABLE_MAC`: the reviewed images are zero
/// and the station image `0x19a8_78e0`, the cold receive image with the HE
/// BSS-color event disabled. `MacInterruptMask` holds only those two.
pub(crate) fn publish_mac_interrupt_mask<P: oer_mmio::Placement>(
    interrupt: &mut wifi_mac_interrupt::WifiMacInterrupt<P>,
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
/// SOURCE: `BLOB_LIBPP_HAL_PWR_INTERRUPT`.
fn mask_mac_power_interrupts<P: oer_mmio::Placement>(
    power: &mut wifi_mac_power_interrupt::WifiMacPowerInterrupt<P>,
) {
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

pub(crate) fn observe_mac_interrupt_enable<P: oer_mmio::Placement>(
    interrupt: &wifi_mac_interrupt::WifiMacInterrupt<P>,
) -> MacInterruptEnableState {
    let enable = interrupt.read_enable();
    let disabled = enable.bits_0_4_opaque().get() == 0
        && !enable.rx_associated_auxiliary_5()
        && !enable.bit_6_opaque()
        && !enable.tx_complete()
        && !enable.bss_color_collision()
        && enable.bits_9_10_opaque().get() == 0
        && !enable.watchdog()
        && !enable.bit_12_opaque()
        && !enable.bit_13_opaque()
        && !enable.rx_success()
        && !enable.sta_beacon_filter()
        && enable.bits_16_18_opaque().get() == 0
        && !enable.tx_timeout()
        && !enable.bit_20_opaque()
        && !enable.bit_21_opaque()
        && !enable.bit_22_opaque()
        && !enable.bit_23_opaque()
        && !enable.rx_associated_auxiliary_24()
        && enable.bits_25_26_opaque().get() == 0
        && !enable.bit_27_opaque()
        && !enable.bit_28_opaque()
        && enable.bits_29_31_opaque().get() == 0;
    if disabled {
        return MacInterruptEnableState::Disabled;
    }

    let station = enable.bits_0_4_opaque().get() == 0
        && enable.rx_associated_auxiliary_5()
        && enable.bit_6_opaque()
        && enable.tx_complete()
        && !enable.bss_color_collision()
        && enable.bits_9_10_opaque().get() == 0
        && enable.watchdog()
        && enable.bit_12_opaque()
        && enable.bit_13_opaque()
        && enable.rx_success()
        && !enable.sta_beacon_filter()
        && enable.bits_16_18_opaque().get() == 0
        && enable.tx_timeout()
        && !enable.bit_20_opaque()
        && enable.bit_21_opaque()
        && !enable.bit_22_opaque()
        && enable.bit_23_opaque()
        && enable.rx_associated_auxiliary_24()
        && enable.bits_25_26_opaque().get() == 0
        && enable.bit_27_opaque()
        && enable.bit_28_opaque()
        && enable.bits_29_31_opaque().get() == 0;
    if station {
        MacInterruptEnableState::Station
    } else {
        MacInterruptEnableState::Unknown
    }
}

trait MacInterruptActivationBackend {
    fn mask_mac_events(&mut self);
    fn mask_power_events(&mut self);
    fn clear_mac_events(&mut self);
    fn clear_power_events(&mut self);
    fn publish_mac_events(&mut self, event_mask: MacInterruptMask);
    fn publish_power_events(&mut self);
    fn fence(&mut self);
}

fn activate_mac_interrupt_epoch(
    backend: &mut impl MacInterruptActivationBackend,
    event_mask: MacInterruptMask,
) {
    // A previous role may leave a level event latched after its CPU route has
    // been detached. Keep both banks masked until every stale event is
    // acknowledged. Publishing the runtime mask before CLEAR creates a real
    // preemption window in which the regular level route can enter before
    // setup returns.
    backend.mask_mac_events();
    backend.mask_power_events();
    backend.clear_mac_events();
    backend.clear_power_events();
    backend.fence();
    backend.publish_mac_events(event_mask);
    backend.publish_power_events();
    backend.fence();
}

/// One live interrupt bank whose complete enable image can be masked.
trait InterruptBankMaskBackend {
    fn mask_events(&mut self);
    fn fence(&mut self);
}

fn mask_interrupt_bank(backend: &mut impl InterruptBankMaskBackend) {
    // Masking is not an acknowledgement: a latched event stays in STATUS and
    // becomes visible again when its source is unmasked.
    backend.mask_events();
    backend.fence();
}

impl<M: MacPlacement> MacInterruptActivationBackend for MacInterruptSetup<M> {
    fn mask_mac_events(&mut self) {
        publish_mac_interrupt_mask(&mut self.mac, MacInterruptMask::NONE);
    }

    fn mask_power_events(&mut self) {
        mask_mac_power_interrupts(&mut self.power);
    }

    fn clear_mac_events(&mut self) {
        self.mac.mac_interrupt_clear();
    }

    fn clear_power_events(&mut self) {
        self.power.clear_all_mac_power_interrupts();
    }

    fn publish_mac_events(&mut self, event_mask: MacInterruptMask) {
        publish_mac_interrupt_mask(&mut self.mac, event_mask);
    }

    /// Enable the station TBTT event for the whole route epoch.
    ///
    /// SOURCE: complete pinned `libpp[hal_tsf.o]::hal_enable_sta_tbtt`
    /// enables the station TBTT event at bit 3 of `0x2010_d8b4`. One route
    /// serves every Wi-Fi role in turn, so the mask stays enabled and the
    /// station TBTT enable, which only a connected station sets, gates it.
    fn publish_power_events(&mut self) {
        self.power.modify_enable(|w| {
            w.set_tbtt_0(true);
        });
    }

    fn fence(&mut self) {
        oer_mmio::fence();
    }
}

impl<M: MacPlacement> MacInterruptSetup<M> {
    /// Read the currently published MAC interrupt mask.
    pub fn mac_interrupt_enable(&self) -> MacInterruptEnableState {
        observe_mac_interrupt_enable(&self.mac)
    }

    /// Mask every MAC interrupt source.
    pub fn mask_all_mac_interrupts(&mut self) {
        publish_mac_interrupt_mask(&mut self.mac, MacInterruptMask::NONE);
    }

    /// Acknowledge every pending MAC interrupt source, masked or not.
    pub fn clear_all_mac_interrupts(&mut self) {
        self.mac.mac_interrupt_clear();
    }

    /// Mask every MAC event and acknowledge every stale event.
    pub fn mask_and_clear_all_mac_interrupts(&mut self) {
        publish_mac_interrupt_mask(&mut self.mac, MacInterruptMask::NONE);
        self.mac.mac_interrupt_clear();
        oer_mmio::fence();
    }

    /// Disable vendor hardware beacon filtering before a connected STA epoch.
    ///
    /// The operation requires both still-disjoint task owners and therefore
    /// cannot race an active ISR. It is the complete vendor disable leaf
    /// (`libpp[hal_mac.o]::hal_disable_sta_beacon_filter`): CONTROL bits 2:0
    /// are cleared before interrupt-enable bit 15, each in its own fresh-read
    /// RMW, in that order.
    pub fn prepare_connected_sta_without_power_save(
        &mut self,
        registers: &mut WifiRadioRegisters<M>,
    ) -> ConnectedStaWithoutPowerSavePrepared {
        registers
            .mac
            .wifi_mac_sta_beacon_filter
            .modify_control(|w| {
                w.set_bits_0_2_opaque(Bits::constant::<0>());
            });
        self.mac.modify_enable(|w| {
            w.set_sta_beacon_filter(false);
        });
        ConnectedStaWithoutPowerSavePrepared { _private: () }
    }

    /// Publish the runtime event mask and create the finite ISR capability.
    ///
    /// The CPU interrupt route must still be unbound while this transaction
    /// executes. The returned value should be installed in its final static
    /// storage before the platform route is enabled.
    pub fn activate(
        mut self,
        event_mask: MacInterruptMask,
    ) -> (MacInterruptRegisters<M>, MacPowerInterruptRegisters<M>) {
        // Clear stale level events while masked, then publish the complete MAC
        // mask as the last interrupt-producing edge before the caller exposes
        // either ISR capability.
        activate_mac_interrupt_epoch(&mut self, event_mask);
        (
            MacInterruptRegisters { mac: self.mac },
            MacPowerInterruptRegisters { power: self.power },
        )
    }
}

/// Disjoint generated register capability intended for the hard power ISR.
///
/// This bank is split from the cold owner together with
/// [`MacInterruptRegisters`]. Ordinary [`WifiRadioRegisters`] therefore
/// cannot race its STATUS/CLEAR transaction from task context.
pub struct MacPowerInterruptRegisters<M: MacPlacement = Chip> {
    power: wifi_mac_power_interrupt::WifiMacPowerInterrupt<M::PowerInterrupt>,
}

impl<M: MacPlacement> InterruptBankMaskBackend for MacPowerInterruptRegisters<M> {
    fn mask_events(&mut self) {
        mask_mac_power_interrupts(&mut self.power);
    }

    fn fence(&mut self) {
        oer_mmio::fence();
    }
}

impl<M: MacPlacement> MacPowerInterruptRegisters<M> {
    /// Mask every WDEVPWR cause of the live epoch without acknowledging one.
    ///
    /// Publishes the all-zero ENABLE image, then orders the write. A latched
    /// cause stays in STATUS; the CPU line deasserts once the masked image
    /// reaches the bank.
    pub fn mask_all_power_interrupts(&mut self) {
        mask_interrupt_bank(self);
    }

    /// Sample the complete masked WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp[hal_tsf.o]::hal_pwr_interrupt_get_event` reads `0x2010_d8bc`.
    pub fn power_interrupt_status(&self) -> MacPowerInterruptSnapshot {
        MacPowerInterruptSnapshot(self.power.sample_mac_power_interrupt())
    }

    /// Acknowledge the complete sampled WDEVPWR event image.
    ///
    /// SOURCE: complete `libpp[hal_tsf.o]::hal_pwr_interrupt_clr_event` stores its argument to `0x2010_d8c0`.
    pub fn acknowledge_power_interrupts(&mut self, snapshot: MacPowerInterruptSnapshot) {
        self.power.acknowledge_mac_power_interrupt(snapshot.0);
        oer_mmio::fence();
    }
}

/// Disjoint generated register capability intended for the hard MAC ISR.
///
/// It is issued by [`MacInterruptSetup::activate`]; construction is
/// crate-private so application code cannot manufacture another ISR owner or
/// retain task-side interrupt enable/clear access during an active epoch.
pub struct MacInterruptRegisters<M: MacPlacement = Chip> {
    pub(crate) mac: wifi_mac_interrupt::WifiMacInterrupt<M::Interrupt>,
}

impl<M: MacPlacement> InterruptBankMaskBackend for MacInterruptRegisters<M> {
    fn mask_events(&mut self) {
        publish_mac_interrupt_mask(&mut self.mac, MacInterruptMask::NONE);
    }

    fn fence(&mut self) {
        oer_mmio::fence();
    }
}

impl<M: MacPlacement> MacInterruptRegisters<M> {
    /// Mask every MAC interrupt source of the live epoch without
    /// acknowledging one.
    ///
    /// Publishes the all-zero ENABLE image, then orders the write. A latched
    /// event stays in STATUS. [`Self::unmask_rx_delivery_interrupts`]
    /// afterwards restores only the RX delivery group.
    pub fn mask_all_mac_interrupts(&mut self) {
        mask_interrupt_bank(self);
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
        oer_mmio::fence();
    }

    /// Temporarily suppress the observed RX delivery group without changing
    /// any other member of the reviewed runtime mask.
    ///
    /// This is a source-moderation operation, not an acknowledgement. The
    /// hard ISR uses it before acknowledging the RX image. The group contains
    /// RX_SUCCESS and the two auxiliary sources observed on every saturated
    /// RX edge; TX and unrelated sources remain independently live.
    pub fn mask_rx_delivery_interrupts(&mut self) {
        self.mac.modify_enable(|w| {
            w.set_rx_associated_auxiliary_5(false)
                .set_rx_success(false)
                .set_rx_associated_auxiliary_24(false);
        });
        oer_mmio::fence();
    }

    /// Restore the RX delivery group while preserving the current mask.
    ///
    /// A latched RX status which arrived while masked becomes visible through
    /// the level CPU route after this ordered write, so the task does not need
    /// to fabricate a hardware completion edge.
    pub fn unmask_rx_delivery_interrupts(&mut self) {
        self.mac.modify_enable(|w| {
            w.set_rx_associated_auxiliary_5(true)
                .set_rx_success(true)
                .set_rx_associated_auxiliary_24(true);
        });
        oer_mmio::fence();
    }

    /// Mask and acknowledge both interrupt banks, returning task-side setup.
    ///
    /// The caller must first disable both CPU interrupt routes and prove that
    /// neither hard handler retains a reference to `self` or `power`. Owning
    /// both values then closes the finite ISR epoch and makes a later
    /// [`MacInterruptSetup::activate`] transaction possible without stealing
    /// either PAC block a second time.
    pub fn deactivate(mut self, mut power: MacPowerInterruptRegisters<M>) -> MacInterruptSetup<M> {
        publish_mac_interrupt_mask(&mut self.mac, MacInterruptMask::NONE);
        mask_mac_power_interrupts(&mut power.power);
        self.mac.mac_interrupt_clear();
        power.power.clear_all_mac_power_interrupts();
        oer_mmio::fence();
        MacInterruptSetup {
            mac: self.mac,
            power: power.power,
        }
    }
}

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

/// PAC-owned semantic partition of one sampled MAC interrupt image.
///
/// The raw W1C geometry stays private to this crate. Higher layers can route
/// qualified work, acknowledge-only auxiliary events, and opaque evidence
/// without applying masks to the sampled register themselves.
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

/// One sampled MAC interrupt image which can be acknowledged exactly once.
pub struct MacInterruptSnapshot(pub(crate) wifi_mac_interrupt::MacInterruptSnapshot);

impl MacInterruptSnapshot {
    pub fn observation(&self) -> MacInterruptObservation {
        let work_events = MacInterruptEvents::from_causes(
            self.0.tx_complete(),
            self.0.bss_color_collision(),
            self.0.rx_success(),
            self.0.tx_timeout(),
        );
        let auxiliary_event =
            self.0.rx_associated_auxiliary_5() || self.0.rx_associated_auxiliary_24();
        let unhandled_event = self.0.bits_0_4_opaque().get() != 0
            || self.0.bit_6_opaque()
            || self.0.bits_9_10_opaque().get() != 0
            || self.0.watchdog()
            || self.0.bit_12_opaque()
            || self.0.bit_13_opaque()
            || self.0.sta_beacon_filter()
            || self.0.bits_16_18_opaque().get() != 0
            || self.0.bit_20_opaque()
            || self.0.bit_21_opaque()
            || self.0.bit_22_opaque()
            || self.0.bit_23_opaque()
            || self.0.bits_25_26_opaque().get() != 0
            || self.0.bit_27_opaque()
            || self.0.bit_28_opaque()
            || self.0.bits_29_31_opaque().get() != 0;
        MacInterruptObservation::from_semantic_events(work_events, auxiliary_event, unhandled_event)
    }

    /// The sampled image, for a probe image that reports it to a vendor
    /// comparison.
    #[cfg(oer_probe)]
    #[doc(hidden)]
    pub const fn probe_bits(&self) -> u32 {
        let sample = &self.0;
        sample.bits_0_4_opaque().get()
            | (sample.rx_associated_auxiliary_5() as u32) << 5
            | (sample.bit_6_opaque() as u32) << 6
            | (sample.tx_complete() as u32) << 7
            | (sample.bss_color_collision() as u32) << 8
            | sample.bits_9_10_opaque().get() << 9
            | (sample.watchdog() as u32) << 11
            | (sample.bit_12_opaque() as u32) << 12
            | (sample.bit_13_opaque() as u32) << 13
            | (sample.rx_success() as u32) << 14
            | (sample.sta_beacon_filter() as u32) << 15
            | sample.bits_16_18_opaque().get() << 16
            | (sample.tx_timeout() as u32) << 19
            | (sample.bit_20_opaque() as u32) << 20
            | (sample.bit_21_opaque() as u32) << 21
            | (sample.bit_22_opaque() as u32) << 22
            | (sample.bit_23_opaque() as u32) << 23
            | (sample.rx_associated_auxiliary_24() as u32) << 24
            | sample.bits_25_26_opaque().get() << 25
            | (sample.bit_27_opaque() as u32) << 27
            | (sample.bit_28_opaque() as u32) << 28
            | sample.bits_29_31_opaque().get() << 29
    }
}

/// One sampled power-interrupt image with intentionally opaque bit semantics.
pub struct MacPowerInterruptSnapshot(pub(crate) wifi_mac_power_interrupt::MacPowerInterruptSnapshot);

impl MacPowerInterruptSnapshot {
    pub fn observation(&self) -> MacPowerInterruptObservation {
        MacPowerInterruptObservation::from_semantic_events(
            self.0.tsf_timer_0(),
            self.0.tsf_timer_1(),
            self.0.tsf_timer_2(),
            self.0.tsf_timer_3(),
            self.0.tbtt_0(),
            // Only the station TSF's TBTT is ever enabled.
            self.0.tbtt_1()
                || self.0.tbtt_2()
                || self.0.tbtt_3()
                || self.0.bits_8_31_opaque().get() != 0,
        )
    }

    /// The sampled image, for a probe image that reports it to a vendor
    /// comparison.
    #[cfg(oer_probe)]
    #[doc(hidden)]
    pub const fn probe_bits(&self) -> u32 {
        let sample = &self.0;
        sample.tbtt_3() as u32
            | (sample.tbtt_2() as u32) << 1
            | (sample.tbtt_1() as u32) << 2
            | (sample.tbtt_0() as u32) << 3
            | (sample.tsf_timer_3() as u32) << 4
            | (sample.tsf_timer_2() as u32) << 5
            | (sample.tsf_timer_1() as u32) << 6
            | (sample.tsf_timer_0() as u32) << 7
            | sample.bits_8_31_opaque().get() << 8
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

#[cfg(test)]
mod tests;
