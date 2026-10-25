//! PHY register sets assembled from the PAC's unique ownership parts.
//! No register logic or additional claim lives in these compositions.
#![forbid(unsafe_code)]
use oer_esp32s31_pac::{fe, instances, wifi_bb, wifi_bb_brx, wifi_mac};
use oer_mmio::{Fixed, Placement};

/// Placements of the physical blocks a PHY operation spans.
pub trait PhyPlacement {
    type Fe: Placement;
    type WifiBb: Placement;
    type WifiBbBrx: Placement;
    type WifiMac: Placement;
}
/// The chip's generated placements.
pub enum ChipPlacement {}
impl PhyPlacement for ChipPlacement {
    type Fe = Fixed<{ instances::fe::BASE }>;
    type WifiBb = Fixed<{ instances::wifi_bb::BASE }>;
    type WifiBbBrx = Fixed<{ instances::wifi_bb_brx::BASE }>;
    type WifiMac = Fixed<{ instances::wifi_mac::BASE }>;
}
#[cfg(test)]
pub struct HostPlacement<'a>(core::marker::PhantomData<&'a mut [u32]>);
#[cfg(test)]
impl<'a> PhyPlacement for HostPlacement<'a> {
    type Fe = oer_mmio::Host<'a>;
    type WifiBb = oer_mmio::Host<'a>;
    type WifiBbBrx = oer_mmio::Host<'a>;
    type WifiMac = oer_mmio::Host<'a>;
}

/// Exclusive parts used by the phy_agc_oracle operations.
pub struct AgcRegisters<P: PhyPlacement = ChipPlacement> {
    pub(crate) fe: fe::PhyAgcOracleFe<P::Fe>,
    pub(crate) wifi_bb: wifi_bb::PhyAgcOracleWifiBb<P::WifiBb>,
    pub(crate) wifi_bb_brx: wifi_bb_brx::PhyAgcOracleWifiBbBrx<P::WifiBbBrx>,
}

#[cfg(test)]
impl<'a> AgcRegisters<HostPlacement<'a>> {
    pub(crate) fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        const REQUIRED: usize = 8223;
        if words.len() < REQUIRED {
            return Err(oer_mmio::HostTooSmall {
                required: REQUIRED * 4,
                available: words.len() * 4,
            });
        }
        let rest = words;
        let (_, rest) = rest.split_at_mut(0);
        let (fe, rest) = rest.split_at_mut(560);
        let (_, rest) = rest.split_at_mut(6608);
        let (wifi_bb, rest) = rest.split_at_mut(852);
        let (_, rest) = rest.split_at_mut(172);
        let (wifi_bb_brx, rest) = rest.split_at_mut(31);
        let _ = rest;
        Ok(Self {
            fe: fe::PhyAgcOracleFe::host(fe)?,
            wifi_bb: wifi_bb::PhyAgcOracleWifiBb::host(wifi_bb)?,
            wifi_bb_brx: wifi_bb_brx::PhyAgcOracleWifiBbBrx::host(wifi_bb_brx)?,
        })
    }
}

/// Exclusive parts used by the phy_baseband_config_oracle operations.
pub struct BasebandRegisters<P: PhyPlacement = ChipPlacement> {
    pub(crate) fe: fe::PhyBasebandConfigOracleFe<P::Fe>,
    pub(crate) wifi_bb: wifi_bb::PhyBasebandConfigOracleWifiBb<P::WifiBb>,
    pub(crate) wifi_mac: wifi_mac::PhyBasebandConfigOracleWifiMac<P::WifiMac>,
}

#[cfg(test)]
impl<'a> BasebandRegisters<HostPlacement<'a>> {
    pub(crate) fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        const REQUIRED: usize = 7989;
        if words.len() < REQUIRED {
            return Err(oer_mmio::HostTooSmall {
                required: REQUIRED * 4,
                available: words.len() * 4,
            });
        }
        let rest = words;
        let (_, rest) = rest.split_at_mut(0);
        let (fe, rest) = rest.split_at_mut(777);
        let (_, rest) = rest.split_at_mut(3319);
        let (wifi_mac, rest) = rest.split_at_mut(292);
        let (_, rest) = rest.split_at_mut(2780);
        let (wifi_bb, rest) = rest.split_at_mut(821);
        let _ = rest;
        Ok(Self {
            fe: fe::PhyBasebandConfigOracleFe::host(fe)?,
            wifi_bb: wifi_bb::PhyBasebandConfigOracleWifiBb::host(wifi_bb)?,
            wifi_mac: wifi_mac::PhyBasebandConfigOracleWifiMac::host(wifi_mac)?,
        })
    }
}

/// Exclusive parts used by the phy_clock_oracle operations.
pub struct ClockRegisters<P: PhyPlacement = ChipPlacement> {
    pub(crate) fe: fe::PhyClockOracleFe<P::Fe>,
    pub(crate) wifi_bb: wifi_bb::PhyClockOracleWifiBb<P::WifiBb>,
}

#[cfg(test)]
impl<'a> ClockRegisters<HostPlacement<'a>> {
    pub(crate) fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        const REQUIRED: usize = 7969;
        if words.len() < REQUIRED {
            return Err(oer_mmio::HostTooSmall {
                required: REQUIRED * 4,
                available: words.len() * 4,
            });
        }
        let rest = words;
        let (_, rest) = rest.split_at_mut(0);
        let (fe, rest) = rest.split_at_mut(513);
        let (_, rest) = rest.split_at_mut(6655);
        let (wifi_bb, rest) = rest.split_at_mut(801);
        let _ = rest;
        Ok(Self {
            fe: fe::PhyClockOracleFe::host(fe)?,
            wifi_bb: wifi_bb::PhyClockOracleWifiBb::host(wifi_bb)?,
        })
    }
}

/// Exclusive parts used by the phy_frequency_channel_oracle operations.
pub struct FrequencyRegisters<P: PhyPlacement = ChipPlacement> {
    pub(crate) fe: fe::PhyFrequencyChannelOracleFe<P::Fe>,
    pub(crate) wifi_bb: wifi_bb::PhyFrequencyChannelOracleWifiBb<P::WifiBb>,
    pub(crate) wifi_mac: wifi_mac::PhyFrequencyChannelOracleWifiMac<P::WifiMac>,
}

#[cfg(test)]
impl<'a> FrequencyRegisters<HostPlacement<'a>> {
    pub(crate) fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        const REQUIRED: usize = 7994;
        if words.len() < REQUIRED {
            return Err(oer_mmio::HostTooSmall {
                required: REQUIRED * 4,
                available: words.len() * 4,
            });
        }
        let rest = words;
        let (_, rest) = rest.split_at_mut(0);
        let (fe, rest) = rest.split_at_mut(542);
        let (_, rest) = rest.split_at_mut(3554);
        let (wifi_mac, rest) = rest.split_at_mut(257);
        let (_, rest) = rest.split_at_mut(2815);
        let (wifi_bb, rest) = rest.split_at_mut(826);
        let _ = rest;
        Ok(Self {
            fe: fe::PhyFrequencyChannelOracleFe::host(fe)?,
            wifi_bb: wifi_bb::PhyFrequencyChannelOracleWifiBb::host(wifi_bb)?,
            wifi_mac: wifi_mac::PhyFrequencyChannelOracleWifiMac::host(wifi_mac)?,
        })
    }
}

/// Placements of the shared modem, analog bus and low-power blocks.
pub trait RadioPlacement: PhyPlacement {
    type HpClock: Placement;
    type Pmu: Placement;
    type ModemSyscon: Placement;
    type ModemLpcon: Placement;
    type LpAon: Placement;
    type LpPeri: Placement;
    type Tsens: Placement;
    type I2c: Placement;
    type I2cRam: Placement;
    type ModemPwr: Placement;
    type BtBb: Placement;
    type ModemDatadump: Placement;
}
impl RadioPlacement for ChipPlacement {
    type HpClock = Fixed<{ oer_esp32s31_pac::instances::hp_sys_clkrst::BASE }>;
    type Pmu = Fixed<{ oer_esp32s31_pac::instances::pmu::BASE }>;
    type ModemSyscon = Fixed<{ oer_esp32s31_pac::instances::modem_syscon::BASE }>;
    type ModemLpcon = Fixed<{ oer_esp32s31_pac::instances::modem_lpcon::BASE }>;
    type LpAon = Fixed<{ oer_esp32s31_pac::instances::lp_aon_clkrst::BASE }>;
    type LpPeri = Fixed<{ oer_esp32s31_pac::instances::lp_periclkrst::BASE }>;
    type Tsens = Fixed<{ oer_esp32s31_pac::instances::lp_tsens::BASE }>;
    type I2c = Fixed<{ oer_esp32s31_pac::instances::i2c_ana_mst::BASE }>;
    type I2cRam = Fixed<{ oer_esp32s31_pac::instances::i2c_ana_mst_mem::BASE }>;
    type ModemPwr = Fixed<{ oer_esp32s31_pac::instances::modem_pwr::BASE }>;
    type BtBb = Fixed<{ oer_esp32s31_pac::instances::bt_bb::BASE }>;
    type ModemDatadump = Fixed<{ oer_esp32s31_pac::instances::modem_datadump::BASE }>;
}
#[cfg(test)]
impl<'a> RadioPlacement for HostPlacement<'a> {
    type HpClock = oer_mmio::Host<'a>;
    type Pmu = oer_mmio::Host<'a>;
    type ModemSyscon = oer_mmio::Host<'a>;
    type ModemLpcon = oer_mmio::Host<'a>;
    type LpAon = oer_mmio::Host<'a>;
    type LpPeri = oer_mmio::Host<'a>;
    type Tsens = oer_mmio::Host<'a>;
    type I2c = oer_mmio::Host<'a>;
    type I2cRam = oer_mmio::Host<'a>;
    type ModemPwr = oer_mmio::Host<'a>;
    type BtBb = oer_mmio::Host<'a>;
    type ModemDatadump = oer_mmio::Host<'a>;
}

/// The physical PHY and modem parts retained by the shared radio owner.
pub(crate) struct RadioPhyParts<P: RadioPlacement = ChipPlacement> {
    pub(crate) phy_agc_oracle: AgcRegisters<P>,
    pub(crate) phy_baseband_config_oracle: BasebandRegisters<P>,
    pub(crate) phy_clock_oracle: ClockRegisters<P>,
    pub(crate) phy_frequency_channel_oracle: FrequencyRegisters<P>,
    pub(crate) hp_sys_clkrst_radio: oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P::HpClock>,
    pub(crate) pmu_radio: oer_esp32s31_pac::pmu::PmuRadio<P::Pmu>,
    pub(crate) modem_syscon_radio: oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P::ModemSyscon>,
    pub(crate) phy_agc_recovered_gaps: oer_esp32s31_pac::wifi_bb::PhyAgcRecoveredGaps<P::WifiBb>,
    pub(crate) phy_bb_recovered_gaps: oer_esp32s31_pac::wifi_bb::PhyBbRecoveredGaps<P::WifiBb>,
    pub(crate) phy_brx_recovered_gaps:
        oer_esp32s31_pac::wifi_bb_brx::PhyBrxRecoveredGaps<P::WifiBbBrx>,
    pub(crate) phy_btagc_recovered: oer_esp32s31_pac::bt_bb::PhyBtagcRecovered<P::BtBb>,
    pub(crate) phy_fectrl_recovered: oer_esp32s31_pac::fe::PhyFectrlRecovered<P::Fe>,
    pub(crate) phy_adc_dump_recovered_fe: oer_esp32s31_pac::fe::PhyAdcDumpRecoveredFe<P::Fe>,
    pub(crate) phy_adc_dump_recovered_bt_bb:
        oer_esp32s31_pac::bt_bb::PhyAdcDumpRecoveredBtBb<P::BtBb>,
    pub(crate) phy_adc_dump_recovered_wifi_bb:
        oer_esp32s31_pac::wifi_bb::PhyAdcDumpRecoveredWifiBb<P::WifiBb>,
    pub(crate) phy_adc_dump_recovered_modem_datadump:
        oer_esp32s31_pac::modem_datadump::PhyAdcDumpRecoveredModemDatadump<P::ModemDatadump>,
    pub(crate) phy_fedata_recovered: oer_esp32s31_pac::fe::PhyFedataRecovered<P::Fe>,
    pub(crate) phy_fedata_wifi_recovered: oer_esp32s31_pac::fe::PhyFedataWifiRecovered<P::Fe>,
    pub(crate) phy_i2c_command_ram: oer_esp32s31_pac::i2c_ana_mst_mem::PhyI2cCommandRam<P::I2cRam>,
    pub(crate) phy_iq_estimator_oracle: oer_esp32s31_pac::fe::PhyIqEstimatorOracle<P::Fe>,
    pub(crate) modem_lpcon_shared_clock:
        oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P::ModemLpcon>,
    pub(crate) modem_lpcon_phy_tick:
        oer_esp32s31_pac::modem_lpcon::ModemLpconPhyTick<P::ModemLpcon>,
    pub(crate) lp_aon_clkrst: oer_esp32s31_pac::lp_aonclkrst::LpAonClkrst<P::LpAon>,
    pub(crate) lp_periclkrst: oer_esp32s31_pac::lp_peri_clkrst::LpPericlkrst<P::LpPeri>,
    pub(crate) lp_tsens: oer_esp32s31_pac::tsens::LpTsens<P::Tsens>,
    pub(crate) i2c_ana_mst: oer_esp32s31_pac::i2c_ana_mst::I2cAnaMst<P::I2c>,
    pub(crate) phy_memory: oer_esp32s31_pac::fe::PhyMemory<P::Fe>,
    pub(crate) phy_nrx_recovered_gaps: oer_esp32s31_pac::wifi_bb::PhyNrxRecoveredGaps<P::WifiBb>,
    pub(crate) phy_pbus: oer_esp32s31_pac::fe::PhyPbus<P::Fe>,
    pub(crate) phy_rx_dco_oracle: oer_esp32s31_pac::fe::PhyRxDcoOracle<P::Fe>,
}

/// Shared PHY/modem ownership, assembled by the reset root.
pub struct RadioPhyRegisters<P: RadioPlacement = ChipPlacement> {
    pub(crate) peripherals: RadioPhyParts<P>,
    mac_local_time: crate::ieee80211::registers::MacLocalTime<P::ModemPwr>,
}

impl RadioPhyRegisters {
    /// Consume the generated ownership partitions of the PHY and the
    /// analog-I2C bus, with a copy of the MAC local-time read.
    pub fn new(
        parts: oer_esp32s31_pac::ownership::RadioPhyPeripherals,
        analog_bus: oer_esp32s31_pac::ownership::AnalogBusPeripherals,
        mac_local_time: crate::ieee80211::registers::MacLocalTime,
    ) -> Self {
        Self {
            peripherals: RadioPhyParts {
                phy_agc_oracle: AgcRegisters {
                    fe: parts.phy_agc_oracle_fe,
                    wifi_bb: parts.phy_agc_oracle_wifi_bb,
                    wifi_bb_brx: parts.phy_agc_oracle_wifi_bb_brx,
                },
                phy_baseband_config_oracle: BasebandRegisters {
                    fe: parts.phy_baseband_config_oracle_fe,
                    wifi_bb: parts.phy_baseband_config_oracle_wifi_bb,
                    wifi_mac: parts.phy_baseband_config_oracle_wifi_mac,
                },
                phy_clock_oracle: ClockRegisters {
                    fe: parts.phy_clock_oracle_fe,
                    wifi_bb: parts.phy_clock_oracle_wifi_bb,
                },
                phy_frequency_channel_oracle: FrequencyRegisters {
                    fe: parts.phy_frequency_channel_oracle_fe,
                    wifi_bb: parts.phy_frequency_channel_oracle_wifi_bb,
                    wifi_mac: parts.phy_frequency_channel_oracle_wifi_mac,
                },
                hp_sys_clkrst_radio: parts.hp_sys_clkrst_radio,
                pmu_radio: parts.pmu_radio,
                modem_syscon_radio: parts.modem_syscon_radio,
                phy_agc_recovered_gaps: parts.phy_agc_recovered_gaps,
                phy_bb_recovered_gaps: parts.phy_bb_recovered_gaps,
                phy_brx_recovered_gaps: parts.phy_brx_recovered_gaps,
                phy_btagc_recovered: parts.phy_btagc_recovered,
                phy_fectrl_recovered: parts.phy_fectrl_recovered,
                phy_adc_dump_recovered_fe: parts.phy_adc_dump_recovered_fe,
                phy_adc_dump_recovered_bt_bb: parts.phy_adc_dump_recovered_bt_bb,
                phy_adc_dump_recovered_wifi_bb: parts.phy_adc_dump_recovered_wifi_bb,
                phy_adc_dump_recovered_modem_datadump: parts.phy_adc_dump_recovered_modem_datadump,
                phy_fedata_recovered: parts.phy_fedata_recovered,
                phy_fedata_wifi_recovered: parts.phy_fedata_wifi_recovered,
                phy_i2c_command_ram: analog_bus.phy_i2c_command_ram,
                phy_iq_estimator_oracle: parts.phy_iq_estimator_oracle,
                modem_lpcon_shared_clock: parts.modem_lpcon_shared_clock,
                modem_lpcon_phy_tick: parts.modem_lpcon_phy_tick,
                lp_aon_clkrst: parts.lp_aon_clkrst,
                lp_periclkrst: parts.lp_periclkrst,
                lp_tsens: parts.lp_tsens,
                i2c_ana_mst: analog_bus.i2c_ana_mst,
                phy_memory: parts.phy_memory,
                phy_nrx_recovered_gaps: parts.phy_nrx_recovered_gaps,
                phy_pbus: parts.phy_pbus,
                phy_rx_dco_oracle: parts.phy_rx_dco_oracle,
            },
            mac_local_time,
        }
    }
}

impl<P: RadioPlacement> RadioPhyRegisters<P> {
    /// The AGC oracle's `WIFI_BB` words, lent to the Wi-Fi MAC's HE
    /// initialization prefix, which reads them.
    pub(crate) fn phy_agc_oracle_wifi_bb(&self) -> &wifi_bb::PhyAgcOracleWifiBb<P::WifiBb> {
        &self.peripherals.phy_agc_oracle.wifi_bb
    }

    /// The frequency-channel oracle's `WIFI_MAC` words, lent to the Wi-Fi
    /// MAC's HE initialization suffix.
    pub(crate) fn phy_frequency_channel_oracle_wifi_mac_mut(
        &mut self,
    ) -> &mut wifi_mac::PhyFrequencyChannelOracleWifiMac<P::WifiMac> {
        &mut self.peripherals.phy_frequency_channel_oracle.wifi_mac
    }

    /// Sample the microsecond counter used by the SDM stability deadline.
    pub fn sample_sdm_deadline_counter(&self) -> u32 {
        self.mac_local_time.now()
    }
}

impl RadioPhyRegisters {
    /// Words of the radio-PHY diagnostic image [`Self::register_image`]
    /// reads.
    pub const REGISTER_IMAGE_LEN: usize = oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio::<<ChipPlacement as RadioPlacement>::HpClock>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::pmu::PmuRadio::<<ChipPlacement as RadioPlacement>::Pmu>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::modem_syscon::ModemSysconRadio::<<ChipPlacement as RadioPlacement>::ModemSyscon>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + fe::PhyAgcOracleFe::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_bb::PhyAgcOracleWifiBb::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_bb_brx::PhyAgcOracleWifiBbBrx::<<ChipPlacement as PhyPlacement>::WifiBbBrx>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::wifi_bb::PhyAgcRecoveredGaps::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + fe::PhyBasebandConfigOracleFe::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_mac::PhyBasebandConfigOracleWifiMac::<<ChipPlacement as PhyPlacement>::WifiMac>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_bb::PhyBasebandConfigOracleWifiBb::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::wifi_bb::PhyBbRecoveredGaps::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::wifi_bb_brx::PhyBrxRecoveredGaps::<<ChipPlacement as PhyPlacement>::WifiBbBrx>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::bt_bb::PhyBtagcRecovered::<<ChipPlacement as RadioPlacement>::BtBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + fe::PhyClockOracleFe::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_bb::PhyClockOracleWifiBb::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyFectrlRecovered::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyAdcDumpRecoveredFe::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::bt_bb::PhyAdcDumpRecoveredBtBb::<<ChipPlacement as RadioPlacement>::BtBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::wifi_bb::PhyAdcDumpRecoveredWifiBb::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::modem_datadump::PhyAdcDumpRecoveredModemDatadump::<<ChipPlacement as RadioPlacement>::ModemDatadump>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyFedataRecovered::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyFedataWifiRecovered::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + fe::PhyFrequencyChannelOracleFe::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_mac::PhyFrequencyChannelOracleWifiMac::<<ChipPlacement as PhyPlacement>::WifiMac>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + wifi_bb::PhyFrequencyChannelOracleWifiBb::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyIqEstimatorOracle::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock::<<ChipPlacement as RadioPlacement>::ModemLpcon>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::modem_lpcon::ModemLpconPhyTick::<<ChipPlacement as RadioPlacement>::ModemLpcon>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::lp_aonclkrst::LpAonClkrst::<<ChipPlacement as RadioPlacement>::LpAon>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::lp_peri_clkrst::LpPericlkrst::<<ChipPlacement as RadioPlacement>::LpPeri>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::tsens::LpTsens::<<ChipPlacement as RadioPlacement>::Tsens>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyMemory::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::wifi_bb::PhyNrxRecoveredGaps::<<ChipPlacement as PhyPlacement>::WifiBb>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyPbus::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len()
            + oer_esp32s31_pac::fe::PhyRxDcoOracle::<<ChipPlacement as PhyPlacement>::Fe>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len();
}

impl<P: RadioPlacement> RadioPhyRegisters<P> {
    /// Read word `index` of the radio-PHY diagnostic image: each part of
    /// the partition in its policy order, each part's generated image of
    /// its readable, side-effect-free words, whole and undecoded; `None`
    /// past its end.
    ///
    /// SOURCE: HIL_VENDOR_CALIBRATION_REGISTERS_2026_09_27.
    pub fn register_image(&self, index: usize) -> Option<u32> {
        let mut index = index;
        macro_rules! image_word {
            ($part:expr, $type:ty) => {
                let len = <$type>::RADIO_PHY_REGISTER_IMAGE_OFFSETS.len();
                if index < len {
                    return $part.radio_phy_register_image(index);
                }
                index -= len;
            };
        }
        image_word!(
            self.peripherals.hp_sys_clkrst_radio,
            oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio::<P::HpClock>
        );
        image_word!(
            self.peripherals.pmu_radio,
            oer_esp32s31_pac::pmu::PmuRadio::<P::Pmu>
        );
        image_word!(
            self.peripherals.modem_syscon_radio,
            oer_esp32s31_pac::modem_syscon::ModemSysconRadio::<P::ModemSyscon>
        );
        image_word!(
            self.peripherals.phy_agc_oracle.fe,
            fe::PhyAgcOracleFe::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_agc_oracle.wifi_bb,
            wifi_bb::PhyAgcOracleWifiBb::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_agc_oracle.wifi_bb_brx,
            wifi_bb_brx::PhyAgcOracleWifiBbBrx::<P::WifiBbBrx>
        );
        image_word!(
            self.peripherals.phy_agc_recovered_gaps,
            oer_esp32s31_pac::wifi_bb::PhyAgcRecoveredGaps::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_baseband_config_oracle.fe,
            fe::PhyBasebandConfigOracleFe::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_baseband_config_oracle.wifi_mac,
            wifi_mac::PhyBasebandConfigOracleWifiMac::<P::WifiMac>
        );
        image_word!(
            self.peripherals.phy_baseband_config_oracle.wifi_bb,
            wifi_bb::PhyBasebandConfigOracleWifiBb::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_bb_recovered_gaps,
            oer_esp32s31_pac::wifi_bb::PhyBbRecoveredGaps::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_brx_recovered_gaps,
            oer_esp32s31_pac::wifi_bb_brx::PhyBrxRecoveredGaps::<P::WifiBbBrx>
        );
        image_word!(
            self.peripherals.phy_btagc_recovered,
            oer_esp32s31_pac::bt_bb::PhyBtagcRecovered::<P::BtBb>
        );
        image_word!(
            self.peripherals.phy_clock_oracle.fe,
            fe::PhyClockOracleFe::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_clock_oracle.wifi_bb,
            wifi_bb::PhyClockOracleWifiBb::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_fectrl_recovered,
            oer_esp32s31_pac::fe::PhyFectrlRecovered::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_adc_dump_recovered_fe,
            oer_esp32s31_pac::fe::PhyAdcDumpRecoveredFe::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_adc_dump_recovered_bt_bb,
            oer_esp32s31_pac::bt_bb::PhyAdcDumpRecoveredBtBb::<P::BtBb>
        );
        image_word!(
            self.peripherals.phy_adc_dump_recovered_wifi_bb,
            oer_esp32s31_pac::wifi_bb::PhyAdcDumpRecoveredWifiBb::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_adc_dump_recovered_modem_datadump,
            oer_esp32s31_pac::modem_datadump::PhyAdcDumpRecoveredModemDatadump::<P::ModemDatadump>
        );
        image_word!(
            self.peripherals.phy_fedata_recovered,
            oer_esp32s31_pac::fe::PhyFedataRecovered::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_fedata_wifi_recovered,
            oer_esp32s31_pac::fe::PhyFedataWifiRecovered::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_frequency_channel_oracle.fe,
            fe::PhyFrequencyChannelOracleFe::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_frequency_channel_oracle.wifi_mac,
            wifi_mac::PhyFrequencyChannelOracleWifiMac::<P::WifiMac>
        );
        image_word!(
            self.peripherals.phy_frequency_channel_oracle.wifi_bb,
            wifi_bb::PhyFrequencyChannelOracleWifiBb::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_iq_estimator_oracle,
            oer_esp32s31_pac::fe::PhyIqEstimatorOracle::<P::Fe>
        );
        image_word!(
            self.peripherals.modem_lpcon_shared_clock,
            oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock::<P::ModemLpcon>
        );
        image_word!(
            self.peripherals.modem_lpcon_phy_tick,
            oer_esp32s31_pac::modem_lpcon::ModemLpconPhyTick::<P::ModemLpcon>
        );
        image_word!(
            self.peripherals.lp_aon_clkrst,
            oer_esp32s31_pac::lp_aonclkrst::LpAonClkrst::<P::LpAon>
        );
        image_word!(
            self.peripherals.lp_periclkrst,
            oer_esp32s31_pac::lp_peri_clkrst::LpPericlkrst::<P::LpPeri>
        );
        image_word!(
            self.peripherals.lp_tsens,
            oer_esp32s31_pac::tsens::LpTsens::<P::Tsens>
        );
        image_word!(
            self.peripherals.phy_memory,
            oer_esp32s31_pac::fe::PhyMemory::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_nrx_recovered_gaps,
            oer_esp32s31_pac::wifi_bb::PhyNrxRecoveredGaps::<P::WifiBb>
        );
        image_word!(
            self.peripherals.phy_pbus,
            oer_esp32s31_pac::fe::PhyPbus::<P::Fe>
        );
        image_word!(
            self.peripherals.phy_rx_dco_oracle,
            oer_esp32s31_pac::fe::PhyRxDcoOracle::<P::Fe>
        );
        let _ = index;
        None
    }
}
