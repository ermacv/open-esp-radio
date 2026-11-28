//! Narrow register capability for one PHY channel transaction.

use crate::{radio::phy::restore::PhyRouteState, system::owner::SharedPhyAccess};

use crate::{radio::ieee80211::registers::WifiRadioRegisters, radio::phy::RadioPhyRegisters};

/// Temporary channel-programming borrow: the Wi-Fi client's MAC with the
/// shared PHY and route restore state borrowed from the radio domain.
///
/// Dropping this value ends every borrow; no PAC owner or generic register
/// accessor is exposed.
#[cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        dead_code,
        reason = "the channel operations are built for the chip only; the host reads none of this"
    )
)]
struct ChannelRegisters<'radio> {
    mac: &'radio mut WifiRadioRegisters,
    phy: &'radio mut RadioPhyRegisters,
    restore: &'radio mut PhyRouteState,
}

#[cfg_attr(
    not(target_arch = "riscv32"),
    expect(
        dead_code,
        reason = "the channel operations are built for the chip only; the host reads none of this"
    )
)]
impl ChannelRegisters<'_> {
    fn mac_mut(&mut self) -> &mut WifiRadioRegisters {
        &mut self.mac
    }

    fn radio_phy(&self) -> &RadioPhyRegisters {
        self.phy
    }

    fn route_state(&self) -> &PhyRouteState {
        self.restore
    }

    fn phy_parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        (self.phy, self.restore)
    }
}

/// One channel operation's registers: the Wi-Fi client's MAC with the
/// shared PHY and its route state borrowed from the radio domain, and the
/// platform. Letting it go ends every borrow.
pub struct RadioChannelHal<'radio, P> {
    #[cfg_attr(
        not(target_arch = "riscv32"),
        expect(
            dead_code,
            reason = "the channel operations are built for the chip only; the host reads none of this"
        )
    )]
    pub(crate) platform: &'radio mut P,
    registers: ChannelRegisters<'radio>,
}

impl<'radio, P> RadioChannelHal<'radio, P> {
    pub(crate) fn from_leased(
        platform: &'radio mut P,
        mac: &'radio mut WifiRadioRegisters,
        phy: &'radio mut RadioPhyRegisters,
        restore: &'radio mut PhyRouteState,
    ) -> Self {
        Self {
            platform,
            registers: ChannelRegisters { mac, phy, restore },
        }
    }
}

impl<P> crate::sealed::SharedPhyAccess for RadioChannelHal<'_, P> {
    fn pac(&self) -> &RadioPhyRegisters {
        self.registers.radio_phy()
    }

    fn route_state(&self) -> &PhyRouteState {
        self.registers.route_state()
    }

    fn parts_mut(&mut self) -> (&mut RadioPhyRegisters, &mut PhyRouteState) {
        self.registers.phy_parts_mut()
    }
}

impl<P> SharedPhyAccess for RadioChannelHal<'_, P> {}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    /// Borrow the platform through the channel transaction.
    ///
    /// Callers remain constrained by their generic platform traits; this does
    /// not expose the radio PAC owner.
    #[doc(hidden)]
    pub fn platform_mut(&mut self) -> &mut P {
        self.platform
    }

    pub fn enable_agc(&mut self) {
        crate::radio::phy::agc::enable(self);
    }

    pub fn disable_agc(&mut self) {
        crate::radio::phy::agc::disable(self);
    }

    pub fn start_frequency_switch(&mut self, frequency_index: u8) {
        crate::radio::phy::frequency::start_channel_switch(self, frequency_index);
    }

    pub fn clear_frequency_switch(&mut self) {
        crate::radio::phy::frequency::clear_channel_switch(self);
    }

    pub fn frequency_ready(&mut self) -> bool {
        crate::radio::phy::frequency::sample_frequency_ready(self)
    }

    pub fn configure_nrx(&mut self, frequency_mhz: u16) {
        crate::radio::phy::frequency::configure_nrx_frequency(self, u32::from(frequency_mhz));
    }

    pub fn configure_rx_compensation(&mut self) {
        crate::radio::phy::agc::configure_rx_compensation(self);
    }

    pub fn publish_tx_cap(&mut self, value: u8) {
        crate::radio::phy::frequency::publish_tx_cap(self, value);
    }

    pub fn configure_channel_cbw(&mut self, cbw: u8) {
        crate::radio::phy::frequency::configure_channel_cbw(self, u32::from(cbw));
    }

    pub fn clear_dc_memory(&mut self) {
        crate::radio::phy::agc::clear_dc_memory(self);
    }

    pub fn table_memory_base_index(&self) -> u8 {
        crate::radio::phy::memory::read_table_memory_base_index(self)
    }

    pub fn program_gain_memory_entry(&mut self, entry: crate::types::PhyGainMemoryEntry) {
        crate::radio::phy::memory::program_gain_memory_entry(self, entry);
    }

    pub fn request_mac_stop(&mut self) {
        crate::radio::ieee80211::mac::WifiMacHal::from_mac(self.registers.mac_mut())
            .request_channel_stop();
    }

    pub fn mac_active_state(&mut self) -> u8 {
        crate::radio::ieee80211::mac::WifiMacHal::from_mac(self.registers.mac_mut())
            .channel_active_state()
    }

    pub fn restart_mac(&mut self) -> u8 {
        crate::radio::ieee80211::mac::WifiMacHal::from_mac(self.registers.mac_mut())
            .restart_after_channel_switch()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    pub fn set_bbpll_calibration(&mut self, mode: crate::types::PhyBbpllCalibration) {
        crate::radio::phy::i2c::configure_bbpll_calibration(self, mode);
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    pub fn configure_bss_cbw(&mut self, cbw: u8) {
        crate::radio::phy::frequency::configure_bss_cbw(self, cbw);
    }
}
