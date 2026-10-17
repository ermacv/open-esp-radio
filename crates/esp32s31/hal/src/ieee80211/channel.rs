//! Narrow register capability for one PHY channel transaction.

use core::cell::RefMut;

use crate::{ieee80211::MacBorrow, owner::SharedPhyAccess, phy::restore::PhyRouteState};

use oer_esp32s31_pac::{RadioPhyRegisters, WifiRadioRegisters};

/// Temporary channel-programming borrow: the Wi-Fi client's MAC with the
/// shared PHY and route restore state borrowed from the arbiter lease.
///
/// Dropping this value ends every borrow; no PAC owner or generic register
/// accessor is exposed.
#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
struct ChannelRegisters<'radio> {
    mac: MacBorrow<'radio>,
    phy: &'radio mut RadioPhyRegisters,
    restore: &'radio mut PhyRouteState,
}

#[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
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

pub struct RadioChannelHal<'radio, P> {
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
    pub(crate) platform: &'radio mut P,
    #[cfg_attr(not(target_arch = "riscv32"), allow(dead_code))]
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
            registers: ChannelRegisters {
                mac: MacBorrow::Owned(mac),
                phy,
                restore,
            },
        }
    }

    pub(crate) fn from_published(
        platform: &'radio mut P,
        mac: RefMut<'radio, WifiRadioRegisters>,
        phy: &'radio mut RadioPhyRegisters,
        restore: &'radio mut PhyRouteState,
    ) -> Self {
        Self {
            platform,
            registers: ChannelRegisters {
                mac: MacBorrow::Published(mac),
                phy,
                restore,
            },
        }
    }
}

impl<P> crate::sealed::SharedPhyAccess for RadioChannelHal<'_, P> {
    fn pac(&self) -> &oer_esp32s31_pac::RadioPhyRegisters {
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

    pub fn set_agc_enabled(&mut self, enabled: bool) {
        crate::phy::agc::set_enabled(self, enabled);
    }

    pub fn start_frequency_switch(&mut self, frequency_index: u8) {
        crate::phy::frequency::start_channel_switch(self, frequency_index);
    }

    pub fn clear_frequency_switch(&mut self) {
        crate::phy::frequency::clear_channel_switch(self);
    }

    pub fn frequency_ready(&mut self) -> bool {
        crate::phy::frequency::sample_frequency_ready(self)
    }

    pub fn configure_nrx(&mut self, frequency_mhz: u16) {
        crate::phy::frequency::configure_nrx_frequency(self, u32::from(frequency_mhz));
    }

    pub fn configure_rx_compensation(&mut self) {
        crate::phy::agc::configure_rx_compensation(self);
    }

    pub fn publish_tx_cap(&mut self, value: u8) {
        crate::phy::frequency::publish_tx_cap(self, value);
    }

    pub fn configure_channel_cbw(&mut self, cbw: u8) {
        crate::phy::frequency::configure_channel_cbw(self, u32::from(cbw));
    }

    pub fn clear_dc_memory(&mut self) {
        crate::phy::agc::clear_dc_memory(self);
    }

    pub fn table_memory_base_index(&self) -> u8 {
        crate::phy::memory::read_table_memory_base_index(self)
    }

    pub fn program_gain_memory_entry(&mut self, entry: crate::types::PhyGainMemoryEntry) {
        crate::phy::memory::program_gain_memory_entry(self, entry);
    }

    pub fn request_mac_stop(&mut self) {
        crate::ieee80211::mac::WifiMacHal::from_mac(self.registers.mac_mut())
            .request_channel_stop();
    }

    pub fn mac_active_state(&mut self) -> u8 {
        crate::ieee80211::mac::WifiMacHal::from_mac(self.registers.mac_mut()).channel_active_state()
    }

    pub fn restart_mac(&mut self) -> u8 {
        crate::ieee80211::mac::WifiMacHal::from_mac(self.registers.mac_mut())
            .restart_after_channel_switch()
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    pub fn set_bbpll_calibration_enabled(&mut self, enabled: bool) {
        crate::phy::i2c::configure_bbpll_calibration(self, enabled);
    }
}

#[cfg(target_arch = "riscv32")]
impl<P> RadioChannelHal<'_, P> {
    pub fn configure_bss_cbw(&mut self, cbw: u8) {
        crate::phy::frequency::configure_bss_cbw(self, cbw);
    }
}
