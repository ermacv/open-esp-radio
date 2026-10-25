//! Composition of the shared PHY and coexistence parts from the reset root.
#![forbid(unsafe_code)]
use super::values::{CoexTimerClientValue, CoexTimerPtiValue};
use super::{CoexTimerBankRegisters, CoexTimerRegister};
use crate::phy::registers::RadioPhyRegisters;
use oer_esp32s31_pac::ownership::{CoexistencePeripherals, SharedRadioPeripherals};

/// Shared register ownership retained by the radio-system actor.
pub struct SharedRadioRegisters {
    pub(crate) radio_phy: RadioPhyRegisters,
    pub(crate) coexistence: CoexistencePeripherals,
    pub(crate) shared_radio: SharedBasebandParts,
}
impl SharedRadioRegisters {
    /// Assemble the unique shared parts without accessing registers.
    pub fn new(
        radio_phy: RadioPhyRegisters,
        coexistence: CoexistencePeripherals,
        shared_radio: SharedRadioPeripherals,
    ) -> (Self, oer_esp32s31_pac::bt_bb::BtV32BasebandRxInfo) {
        let SharedRadioPeripherals {
            bt_v32_baseband,
            bt_v32_baseband_rx_info,
            shared_radio_init_control,
            shared_baseband_reset_idle_status,
            shared_baseband_tx_timing,
            zbbb_radio_control,
        } = shared_radio;
        (
            Self {
                radio_phy,
                coexistence,
                shared_radio: SharedBasebandParts {
                    bt_v32_baseband,
                    shared_radio_init_control,
                    shared_baseband_reset_idle_status,
                    shared_baseband_tx_timing,
                    zbbb_radio_control,
                },
            },
            bt_v32_baseband_rx_info,
        )
    }

    pub const fn radio_phy(&self) -> &RadioPhyRegisters {
        &self.radio_phy
    }
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.radio_phy
    }
    pub fn coex_timers_mut(&mut self) -> CoexTimerBankRegisters<'_> {
        CoexTimerBankRegisters::new(&mut self.coexistence.coex_hw_timer)
    }
    pub fn radio_phy_and_coex_timers_mut(
        &mut self,
    ) -> (&mut RadioPhyRegisters, CoexTimerBankRegisters<'_>) {
        (
            &mut self.radio_phy,
            CoexTimerBankRegisters::new(&mut self.coexistence.coex_hw_timer),
        )
    }
    pub fn configure_coex_timer(
        &mut self,
        timer: CoexTimerRegister,
        client: CoexTimerClientValue,
        pti: CoexTimerPtiValue,
    ) {
        self.coex_timers_mut().configure(timer, client, pti);
    }
    pub fn set_coex_timer_primary_target(&mut self, timer: CoexTimerRegister, value: u32) {
        self.coex_timers_mut().set_primary_target(timer, value);
    }
    pub fn set_coex_timer_secondary_target(&mut self, timer: CoexTimerRegister, value: u32) {
        self.coex_timers_mut().set_secondary_target(timer, value);
    }
    pub fn enable_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().enable(timer);
    }
    pub fn disable_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().disable(timer);
    }
    pub fn force_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().force(timer);
    }
    pub fn unforce_coex_timer(&mut self, timer: CoexTimerRegister) {
        self.coex_timers_mut().unforce(timer);
    }
    pub fn configure_coexistence_timer_clock(
        &mut self,
        source: super::values::CoexTimerClockSource,
        divider: super::values::CoexTimerClockDividerMinusOne,
    ) {
        self.radio_phy
            .configure_coexistence_timer_clock(source, divider);
    }
    pub fn sample_coexistence_low_power_clock(
        &self,
    ) -> Option<super::CoexistenceLowPowerClockObservation> {
        self.radio_phy.sample_coexistence_low_power_clock()
    }
}

/// Shared baseband ownership after the root transfers RSSI to IEEE 802.15.4.
pub struct SharedBasebandParts {
    pub(crate) bt_v32_baseband: oer_esp32s31_pac::bt_bb::BtV32Baseband,
    pub(crate) shared_radio_init_control: oer_esp32s31_pac::wifi_mac::SharedRadioInitControl,
    pub(crate) shared_baseband_reset_idle_status:
        oer_esp32s31_pac::ieee802154::SharedBasebandResetIdleStatus,
    pub(crate) shared_baseband_tx_timing: oer_esp32s31_pac::bt_bb::SharedBasebandTxTiming,
    pub(crate) zbbb_radio_control: oer_esp32s31_pac::bt_bb::ZbbbRadioControl,
}
