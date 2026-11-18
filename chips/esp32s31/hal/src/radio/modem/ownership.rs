//! Composition of the shared PHY and coexistence parts from the reset root.
#![forbid(unsafe_code)]
use super::values::{CoexTimerClientValue, CoexTimerPtiValue};
use super::{CoexTimerBankRegisters, CoexTimerRegister};
use crate::radio::phy::RadioPhyRegisters;
use oer_chips_esp32s31_pac::ownership::{CoexistencePeripherals, SharedRadioPeripherals};

/// The fields of the three words inside the `WIFI_MAC` block that are not
/// the Wi-Fi MAC's (the PHY's channel offset word, the PHY's I2C TX rate
/// word and the shared radio init word), as one observation reads them.
///
/// For a probe's observation of the block's reset: read without effect,
/// decided on by nothing. `minimum_power` and `channel_bit_17` are the
/// Wi-Fi cold preparation's in the first word, `init_low` its low twelve
/// bits of the last; `init_bit_31` is the Bluetooth baseband's.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ForeignWifiMacWords {
    /// `CHANNEL_TX_OFFSET_CONTROL.PRIMARY_20_CHANNEL_OFFSET` [3:0], the PHY's.
    pub primary_20_channel_offset: u8,
    /// `MINIMUM_POWER_HIGH`:`MINIMUM_POWER_LOW` [9:4].
    pub minimum_power: u8,
    /// `BITS_10_16_OPAQUE` [16:10].
    pub channel_bits_10_16: u8,
    /// `BIT_17_OPAQUE` [17].
    pub channel_bit_17: bool,
    /// `BITS_18_31_OPAQUE` [31:18].
    pub channel_bits_18_31: u16,
    /// `I2C_TX_RATE_CONTROL.BITS_0_1_OPAQUE` [1:0], the PHY's.
    pub i2c_bits_0_1: u8,
    /// `I2C_TX_RATE_CONTROL.BITS_18_25_OPAQUE` [25:18], the PHY's.
    pub i2c_bits_18_25: u8,
    /// `RADIO_INIT_CONTROL.BITS_0_11_OPAQUE` [11:0].
    pub init_low: u16,
    /// `RADIO_INIT_CONTROL.BIT_31_OPAQUE` [31].
    pub init_bit_31: bool,
}

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
    ) -> (Self, oer_chips_esp32s31_pac::bt_bb::BtV32BasebandRxInfo) {
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

    /// Read the three foreign words of the `WIFI_MAC` block
    /// ([`ForeignWifiMacWords`]), without effect.
    pub fn foreign_wifi_mac_words(&self) -> ForeignWifiMacWords {
        let parts = &self.radio_phy.peripherals;
        let channel = parts
            .phy_frequency_channel_oracle
            .wifi_mac
            .read_channel_tx_offset_control();
        let i2c = parts
            .phy_baseband_config_oracle
            .wifi_mac
            .read_i2c_tx_rate_control();
        let init = self.shared_radio.shared_radio_init_control.read_radio_init_control();
        let narrow = |value: u32| u8::try_from(value).unwrap_or(u8::MAX);
        ForeignWifiMacWords {
            primary_20_channel_offset: narrow(u32::from(channel.primary_20_channel_offset().get())),
            minimum_power: narrow(
                (u32::from(channel.minimum_power_high().get()) << 4)
                    | u32::from(channel.minimum_power_low().get()),
            ),
            channel_bits_10_16: narrow(u32::from(channel.bits_10_16_opaque().get())),
            channel_bit_17: channel.bit_17_opaque(),
            channel_bits_18_31: u16::try_from(u32::from(channel.bits_18_31_opaque().get()))
                .unwrap_or(u16::MAX),
            i2c_bits_0_1: narrow(u32::from(i2c.bits_0_1_opaque().get())),
            i2c_bits_18_25: i2c.bits_18_25_opaque(),
            init_low: u16::try_from(u32::from(init.bits_0_11_opaque().get())).unwrap_or(u16::MAX),
            init_bit_31: init.bit_31_opaque(),
        }
    }
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.radio_phy
    }
    /// The shared radio init control, lent to the Wi-Fi MAC's TX/RX
    /// initialization suffix.
    pub(crate) fn shared_radio_init_control_mut(
        &mut self,
    ) -> &mut oer_chips_esp32s31_pac::wifi_mac::SharedRadioInitControl {
        &mut self.shared_radio.shared_radio_init_control
    }

    /// Execute the MMIO path of vendor `bt_bb_v2_init_cmplx(1)` over the
    /// shared baseband parts and the radio PHY's Bluetooth AGC and
    /// baseband-configuration words.
    ///
    /// SOURCE: BLOB_LIBBTBB_BT_BB_V2_INIT.
    pub(crate) fn initialize_btbb_v2_arg_one(&mut self, gain_parameter: u8) {
        use crate::radio::bluetooth::registers::baseband::{
            BtbbV2Registers, ChipBtbb, initialize_btbb_v2_arg_one,
        };
        let phy = &mut self.radio_phy.peripherals;
        let shared = &mut self.shared_radio;
        initialize_btbb_v2_arg_one::<ChipBtbb>(
            BtbbV2Registers {
                btagc: &mut phy.phy_btagc_recovered,
                oracle_fe: &mut phy.phy_baseband_config_oracle.fe,
                oracle_wifi_bb: &mut phy.phy_baseband_config_oracle.wifi_bb,
                baseband: &mut shared.bt_v32_baseband,
                init_control: &mut shared.shared_radio_init_control,
                tx_timing: &mut shared.shared_baseband_tx_timing,
                zbbb: &mut shared.zbbb_radio_control,
            },
            gain_parameter,
        );
    }

    pub fn coex_timers_mut(&mut self) -> CoexTimerBankRegisters<'_> {
        CoexTimerBankRegisters::new(
            &mut self.coexistence.coex_policy_timers,
            &mut self.coexistence.phy_grant_protect_timer,
        )
    }
    pub fn radio_phy_and_coex_timers_mut(
        &mut self,
    ) -> (&mut RadioPhyRegisters, CoexTimerBankRegisters<'_>) {
        (
            &mut self.radio_phy,
            CoexTimerBankRegisters::new(
                &mut self.coexistence.coex_policy_timers,
                &mut self.coexistence.phy_grant_protect_timer,
            ),
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
    pub(crate) bt_v32_baseband: oer_chips_esp32s31_pac::bt_bb::BtV32Baseband,
    pub(crate) shared_radio_init_control: oer_chips_esp32s31_pac::wifi_mac::SharedRadioInitControl,
    pub(crate) shared_baseband_reset_idle_status:
        oer_chips_esp32s31_pac::ieee802154::SharedBasebandResetIdleStatus,
    pub(crate) shared_baseband_tx_timing: oer_chips_esp32s31_pac::bt_bb::SharedBasebandTxTiming,
    pub(crate) zbbb_radio_control: oer_chips_esp32s31_pac::bt_bb::ZbbbRadioControl,
}
