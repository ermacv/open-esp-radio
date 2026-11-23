//! Composition of the shared PHY and coexistence parts from the reset root.
#![forbid(unsafe_code)]
use super::values::{CoexTimerClientValue, CoexTimerPtiValue};
use super::{CoexTimerBankRegisters, CoexTimerRegister};
use crate::radio::phy::RadioPhyRegisters;
use oer_chips_esp32s31_pac::Bits;
use oer_chips_esp32s31_pac::ownership::{CoexistencePeripherals, SharedRadioPeripherals};

/// The fields of the three words inside the `WIFI_MAC` block that are not
/// the Wi-Fi MAC's (the PHY's channel offset word, the PHY's I2C TX rate
/// word and the shared radio init word), as one observation reads them.
///
/// For a probe's observation of the block's reset: read without effect,
/// decided on by nothing. `minimum_power` and `channel_bit_17` are the
/// Wi-Fi cold preparation's in the first word, `init_low` its low twelve
/// bits of the last; `init_bit_31` is the Bluetooth baseband's.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ForeignWifiMacWords {
    /// `CHANNEL_TX_OFFSET_CONTROL.PRIMARY_20_CHANNEL_OFFSET` [3:0], the PHY's.
    pub primary_20_channel_offset: Bits<4>,
    /// `MINIMUM_POWER_LOW` [7:4].
    pub minimum_power_low: Bits<4>,
    /// `MINIMUM_POWER_HIGH` [9:8].
    pub minimum_power_high: Bits<2>,
    /// `BITS_10_16_OPAQUE` [16:10].
    pub channel_bits_10_16: Bits<7>,
    /// `BIT_17_OPAQUE` [17].
    pub channel_bit_17: bool,
    /// `BITS_18_31_OPAQUE` [31:18].
    pub channel_bits_18_31: Bits<14>,
    /// `I2C_TX_RATE_CONTROL.BITS_0_1_OPAQUE` [1:0], the PHY's.
    pub i2c_bits_0_1: Bits<2>,
    /// `I2C_TX_RATE_CONTROL.BITS_18_25_OPAQUE` [25:18], the PHY's.
    pub i2c_bits_18_25: u8,
    /// `RADIO_INIT_CONTROL.BITS_0_11_OPAQUE` [11:0].
    pub init_low: Bits<12>,
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
    /// Assemble the unique shared parts without accessing registers. The
    /// Bluetooth and IEEE 802.15.4 baseband words have no owner in the
    /// build: they are dropped here, as the root drops those radios' own
    /// blocks.
    pub(crate) fn new(
        radio_phy: RadioPhyRegisters,
        coexistence: CoexistencePeripherals,
        shared_radio: SharedRadioPeripherals,
    ) -> Self {
        let SharedRadioPeripherals {
            bt_v32_baseband: _,
            bt_v32_baseband_rx_info: _,
            shared_radio_init_control,
            shared_baseband_reset_idle_status: _,
            shared_baseband_tx_timing: _,
            zbbb_radio_control: _,
        } = shared_radio;
        Self {
            radio_phy,
            coexistence,
            shared_radio: SharedBasebandParts {
                shared_radio_init_control,
            },
        }
    }

    pub const fn radio_phy(&self) -> &RadioPhyRegisters {
        &self.radio_phy
    }

    /// Write back, after an `RST_WIFIMAC` pulse, what the block's other
    /// owners keep in it, and Wi-Fi's constants of those same words, so that
    /// each word has one writing context: the radio system's.
    ///
    /// - The PHY's channel nibble of `CHANNEL_TX_OFFSET_CONTROL`, by the
    ///   first write of the channel change's own last writer of it,
    ///   `phy_bb_cbw_chan_cfg`
    ///   ([`crate::radio::phy::frequency::ops::write_channel_tx_offset`],
    ///   which [`crate::radio::phy::frequency::ops::configure_channel_cbw`]
    ///   calls), with the tuned channel's `cbw`. The channel's three
    ///   baseband fields are outside the block and are not written.
    /// - Wi-Fi's minimum transmit power (`hal_set_tx_min_pwr(-11)`) and bit
    ///   17 of the same word, which `hal_he_init` writes.
    /// - The low twelve bits of `RADIO_INIT_CONTROL`, `0x0f0`, which
    ///   `mac_txrx_init` writes; the Bluetooth baseband's bit 31 is cleared
    ///   by the pulse already and is not written.
    /// - `I2C_TX_RATE_CONTROL` is not written: the vendor's station runs
    ///   with it cleared.
    /// - The read of the PHY's AGC word that `hal_he_init`'s prefix makes
    ///   and discards.
    ///
    /// SOURCE: HW_WIFIMAC_RESET_FOREIGN_WORDS_20261118 (what the pulse
    /// clears); complete pinned `libpp.a[hal_mac_ctl.o]::hal_he_init`,
    /// `libpp.a[hal_mac_tx.o]::hal_set_tx_min_pwr` and
    /// `libpp.a[hal_mac.o]::mac_txrx_init` (the constants).
    pub(crate) fn write_back_wifi_mac_foreign_words(&mut self, cbw: u8) {
        let parts = &mut self.radio_phy.peripherals;
        write_back_wifi_mac_foreign_words(
            &mut parts.phy_frequency_channel_oracle.wifi_mac,
            &mut self.shared_radio.shared_radio_init_control,
            &parts.phy_agc_oracle.wifi_bb,
            cbw,
        );
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
        let init = self
            .shared_radio
            .shared_radio_init_control
            .read_radio_init_control();
        ForeignWifiMacWords {
            primary_20_channel_offset: channel.primary_20_channel_offset(),
            minimum_power_low: channel.minimum_power_low(),
            minimum_power_high: channel.minimum_power_high(),
            channel_bits_10_16: channel.bits_10_16_opaque(),
            channel_bit_17: channel.bit_17_opaque(),
            channel_bits_18_31: channel.bits_18_31_opaque(),
            i2c_bits_0_1: i2c.bits_0_1_opaque(),
            i2c_bits_18_25: i2c.bits_18_25_opaque(),
            init_low: init.bits_0_11_opaque(),
            init_bit_31: init.bit_31_opaque(),
        }
    }
    pub fn radio_phy_mut(&mut self) -> &mut RadioPhyRegisters {
        &mut self.radio_phy
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

/// The shared baseband word Wi-Fi writes: `RADIO_INIT_CONTROL`.
pub struct SharedBasebandParts {
    pub(crate) shared_radio_init_control: oer_chips_esp32s31_pac::wifi_mac::SharedRadioInitControl,
}

/// [`SharedRadioRegisters::write_back_wifi_mac_foreign_words`] over its
/// parts, at any placement.
pub(crate) fn write_back_wifi_mac_foreign_words<
    M: oer_portable_mmio::Placement,
    B: oer_portable_mmio::Placement,
>(
    channel: &mut oer_chips_esp32s31_pac::wifi_mac::PhyFrequencyChannelOracleWifiMac<M>,
    init: &mut oer_chips_esp32s31_pac::wifi_mac::SharedRadioInitControl<M>,
    agc: &oer_chips_esp32s31_pac::wifi_bb::PhyAgcOracleWifiBb<B>,
    cbw: u8,
) {
    use crate::radio::ieee80211::registers::he::init_suffix::{
        MINIMUM_POWER_HIGH, MINIMUM_POWER_LOW,
    };
    crate::radio::phy::frequency::ops::write_channel_tx_offset(channel, u32::from(cbw));
    channel.modify_channel_tx_offset_control(|w| {
        w.set_minimum_power_low(Bits::constant::<MINIMUM_POWER_LOW>())
            .set_minimum_power_high(Bits::constant::<MINIMUM_POWER_HIGH>());
    });
    channel.modify_channel_tx_offset_control(|w| {
        w.set_bit_17_opaque(true);
    });
    init.modify_radio_init_control(|w| {
        w.set_bits_0_11_opaque(Bits::constant::<0x0f0>());
    });
    // The complete `hal_he_init` prefix reads this word and discards it; the
    // read is kept because an MMIO read may synchronize hardware state.
    let _ = agc.read_agc_init_high_control().bits_24_31_opaque();
}

#[cfg(test)]
mod foreign_word_tests {
    use super::write_back_wifi_mac_foreign_words;
    use crate::radio::phy::registers::FrequencyRegisters;
    use oer_chips_esp32s31_pac::{
        instances, wifi_bb::PhyAgcOracleWifiBb, wifi_mac::SharedRadioInitControl,
    };

    #[test]
    fn the_write_back_sets_the_channels_nibble_wifis_constants_and_nothing_of_bluetooth() {
        for fill in [0, u32::MAX] {
            let mut frequency_words = std::vec![fill; 0x10000];
            let mut init_words = std::vec![fill; instances::wifi_mac::SIZE.div_ceil(4)];
            let mut agc_words = std::vec![fill; instances::wifi_bb::SIZE.div_ceil(4)];
            let agc_before = agc_words.clone();
            {
                let mut frequency =
                    FrequencyRegisters::host(&mut frequency_words).expect("host block");
                let mut init =
                    SharedRadioInitControl::host(&mut init_words).expect("the part's host words");
                let agc = PhyAgcOracleWifiBb::host(&mut agc_words).expect("the part's host words");
                let baseband = (
                    frequency
                        .wifi_bb
                        .read_channel_cbw_control_0()
                        .bits_0_1_opaque(),
                    frequency
                        .wifi_bb
                        .read_channel_cbw_control_1()
                        .bits_2_4_opaque(),
                    frequency
                        .wifi_bb
                        .read_channel_cbw_control_1()
                        .bits_0_1_opaque(),
                );
                // HT40 with the secondary below, as the radio system lowers it.
                write_back_wifi_mac_foreign_words(&mut frequency.wifi_mac, &mut init, &agc, 3);
                // The baseband's channel fields are outside the MAC's block:
                // the write-back leaves what the channel's selection wrote.
                assert_eq!(
                    (
                        frequency
                            .wifi_bb
                            .read_channel_cbw_control_0()
                            .bits_0_1_opaque(),
                        frequency
                            .wifi_bb
                            .read_channel_cbw_control_1()
                            .bits_2_4_opaque(),
                        frequency
                            .wifi_bb
                            .read_channel_cbw_control_1()
                            .bits_0_1_opaque(),
                    ),
                    baseband
                );
                let offsets = frequency.wifi_mac.read_channel_tx_offset_control();
                assert_eq!(offsets.primary_20_channel_offset().get(), 1);
                assert_eq!(offsets.minimum_power_low().get(), 5);
                assert_eq!(offsets.minimum_power_high().get(), 3);
                assert!(offsets.bit_17_opaque());
                let control = init.read_radio_init_control();
                assert_eq!(control.bits_0_11_opaque().get(), 0x0f0);
                // The Bluetooth baseband's bit keeps what it held.
                assert_eq!(control.bit_31_opaque(), fill == u32::MAX);
            }
            assert_eq!(agc_words, agc_before, "the AGC word is only read");
        }
    }
}
