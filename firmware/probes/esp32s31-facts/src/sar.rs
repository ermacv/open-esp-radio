//! Short SAR experiments; no HAL, RF calibration or background tracking.
//! SOURCE: production hal/src/power.rs::execute_owned, modem/ops.rs,
//! phy/clock/ops.rs, phy/i2c/master.rs, phy/baseband/ops.rs and
//! oer-notes/research/opaque-blobray-phy.md (P-SAR-HOLD / P-SAR-EDGE).
use super::{Bits, emit, pac};
use pac::{fe, wifi_bb};

pub(super) struct Sar {
    pub radio: pac::ownership::RadioPhyPeripherals,
    pub clocks: pac::ownership::ChipClockPeripherals,
    pub boot: pac::ownership::BootPeripherals,
    pub analog: pac::ownership::AnalogBusPeripherals,
    hz: u32,
    ticks_per_us: u32,
    trigger_window: [u32; 2],
    observations: [(u32, u32); 99],
    usable: bool,
    distinct: bool,
}
impl Sar {
    pub fn new(
        radio: pac::ownership::RadioPhyPeripherals,
        clocks: pac::ownership::ChipClockPeripherals,
        boot: pac::ownership::BootPeripherals,
        analog: pac::ownership::AnalogBusPeripherals,
    ) -> Self {
        Self {
            radio,
            clocks,
            boot,
            analog,
            hz: 0,
            ticks_per_us: 0,
            trigger_window: [0; 2],
            observations: [(0, 0); 99],
            usable: false,
            distinct: false,
        }
    }
    fn clock(&mut self) -> bool {
        // SOURCE: production hal/src/boot/clock.rs::CpuFrequency::from_dividers.
        let root = self
            .boot
            .hp_sys_clkrst_boot_clock
            .read_soc_clk_sel()
            .reg_soc_clk_sel();
        let cpu = self.boot.hp_sys_clkrst_boot_clock.read_cpu_freq_ctrl0();
        let pll = self.boot.lp_aon_clkrst_cpll.read_cpll_div();
        emit(
            0x1400,
            root.get(),
            u32::from(cpu.reg_cpu_clk_div_num()),
            pll.cpll_fb_div().into(),
        );
        if cpu.reg_cpu_clk_div_numerator().get() != 0 {
            return false;
        }
        let base = match pac::hp_sys_clkrst::soc_clk_sel::RegSocClkSel::from_field(root.get()) {
            pac::hp_sys_clkrst::soc_clk_sel::RegSocClkSel::Xtal => 40_000_000_u32,
            pac::hp_sys_clkrst::soc_clk_sel::RegSocClkSel::Cpll => {
                let Some(hz) = 40_000_000_u32
                    .checked_mul(pll.cpll_fb_div().into())
                    .and_then(|v| v.checked_div(pll.cpll_ref_div().get()))
                else {
                    return false;
                };
                hz
            }
            _ => return false,
        };
        self.hz = base / (u32::from(cpu.reg_cpu_clk_div_num()) + 1);
        emit(
            0x1401,
            self.hz,
            pll.cpll_ref_div().get(),
            cpu.reg_cpu_clk_div_numerator().get(),
        );
        self.ticks_per_us = self.hz / 1_000_000;
        self.ticks_per_us != 0 && self.hz.is_multiple_of(1_000_000)
    }
    #[inline(always)]
    fn cycles(&self, us: u32) -> u32 {
        self.ticks_per_us.saturating_mul(us)
    }
    #[inline(always)]
    fn until(&self, start: u32, us: u32) {
        let bound = self.cycles(us);
        while oer_riscv::cycle32().wrapping_sub(start) < bound {}
    }
    fn prerequisites(&mut self) {
        let p = &mut self.radio;
        p.pmu_radio.select_hp_active_modem_icg();

        p.pmu_radio.trigger_imm_modem_icg_update_dig_icg_modem_en();

        p.pmu_radio.trigger_imm_sleep_sysclk_update_dig_icg_switch();

        p.hp_sys_clkrst_radio.modify_modem_ctrl0(|w| {
            w.set_reg_modem_clk_en(
                oer_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled,
            );
        });

        p.modem_syscon_radio.modify_clk_conf_power_st(|w| {
            w.set_clk_zb_st_map(Bits::truncate(w.clk_zb_st_map().get() | 4));
            w.set_clk_fe_st_map(Bits::truncate(w.clk_fe_st_map().get() | 6));
            w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
            w.set_clk_wifi_st_map(Bits::truncate(w.clk_wifi_st_map().get() | 6));
            w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
            w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
        });

        p.modem_lpcon_shared_clock.modify_clk_conf_power_st(|w| {
            w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
            w.set_clk_coex_st_map(Bits::truncate(w.clk_coex_st_map().get() | 6));
            w.set_clk_i2c_mst_st_map(Bits::truncate(w.clk_i2c_mst_st_map().get() | 6));
            w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
        });

        self.clocks.hp_sys_clkrst_pll160.modify_ref_160m_ctrl0(|w| {
            w.set_reg_ref_160m_clk_en(true);
        });

        p.hp_sys_clkrst_radio.modify_modem_conf(|w| {
            w.set_modem_apb_clk_en(true);
            w.set_modem_rst_en(false);
            w.set_modem_clk_en(true);
            w.set_modem_clk_source_sel(true);
            w.set_modem_pll_clk_en(true);
            w.set_modem_xtal_clk_en(true);
        });

        p.modem_syscon_radio.modify_modem_rst_conf(|w| {
            w.set_rst_wifibb(pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted);
        });
        p.modem_syscon_radio.modify_modem_rst_conf(|w| {
            w.set_rst_wifibb(pac::modem_syscon::modem_rst_conf::RstWifibb::Released);
        });

        p.modem_syscon_radio.modify_clk_conf1(|w| {
            w.set_clk_wifibb_22m_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb22mEn::Enabled,
            );
            w.set_clk_wifibb_40m_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40mEn::Enabled,
            );
            w.set_clk_wifibb_44m_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb44mEn::Enabled,
            );
            w.set_clk_wifibb_80m_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80mEn::Enabled,
            );
            w.set_clk_wifibb_40x_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40xEn::Enabled,
            );
            w.set_clk_wifibb_80x_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80xEn::Enabled,
            );
            w.set_clk_wifibb_40x1_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40x1En::Enabled,
            );
            w.set_clk_wifibb_80x1_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled,
            );
            w.set_clk_wifibb_160x1_en(
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb160x1En::Enabled,
            );
            w.set_clk_wifi_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifiApbEn::Enabled);
            w.set_clk_fe_80m_en(true);
            w.set_clk_fe_160m_en(true);
            w.set_clk_fe_apb_en(true);
            w.set_clk_bt_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtApbEn::Enabled);
            w.set_clk_btbb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtbbEn::Enabled);
            w.set_clk_fe_pwdet_adc_en(true);
            w.set_clk_fe_adc_en(true);
            w.set_clk_fe_dac_en(true);
        });

        p.modem_syscon_radio.modify_clk_conf(|w| {
            w.set_clk_i2c_mst_sel_160m(
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled,
            );
        });

        self.clocks
            .modem_lpcon_chip_clocks
            .set_modem_pwr_wifi_power_clock(true);
        self.clocks
            .modem_lpcon_chip_clocks
            .set_i2c_ana_mst_master_clock(true);

        // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
        p.phy_clock_oracle_fe.write_word_0400_opaque(|w| {
            w.set_bits_0_2_opaque(Bits::constant::<0x7>())
                .set_bits_5_8_opaque(Bits::constant::<0xf>());
        });
        // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
        p.phy_clock_oracle_fe.modify_word_0800_opaque(|w| {
            w.set_bits_0_1_opaque(fe::word_0800_opaque::Bits0_1Opaque::Open);
        });
        // SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK.
        p.phy_clock_oracle_wifi_bb.write_word_0c80_opaque(|w| {
            w.set_state(wifi_bb::word_0c80_opaque::State::Open);
        });

        p.pmu_radio.modify_hp_active_hp_ck_power(|w| {
            w.set_bits_0_3_opaque(
                oer_esp32s31_pac::pmu::hp_active_hp_ck_power::Bits0_3Opaque::RomRequired,
            );
            w.set_hp_active_xpd_bb_i2c(true);
        });
        p.pmu_radio.trigger_imm_hp_ck_power_0_tie_high_xpd_bb_i2c();

        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_xpd_perif_i2c(true);
        });

        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(true);
        });

        let master = &mut self.analog.i2c_ana_mst;

        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_i2c0_ctrl1(|w| {
            w.set_i2c0_sda_side_guard(Bits::constant::<0x2>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_i2c0_ctrl1(|w| {
            w.set_i2c0_scl_pulse_dur(Bits::constant::<0x4>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_i2c1_ctrl1(|w| {
            w.set_i2c1_sda_side_guard(Bits::constant::<0x2>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_i2c1_ctrl1(|w| {
            w.set_i2c1_scl_pulse_dur(Bits::constant::<0x4>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_hw_i2c_ctrl(|w| {
            w.set_hw_i2c_sda_side_guard(Bits::constant::<0x2>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_hw_i2c_ctrl(|w| {
            w.set_hw_i2c_scl_pulse_dur(Bits::constant::<0x4>());
        });

        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_ana_conf0(|w| {
            w.set_phy_register_mode(Bits::constant::<2>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_ana_conf0(|w| {
            w.set_phy_register_enable(true);
        });
    }
    fn analog_byte(&mut self, address: pac::analog_i2c::Address, write: Option<u8>) -> Option<u8> {
        // SOURCE: production phy/i2c/master.rs; SARADC uses normal route host 0.
        // A 20 µs experimental bound applies to each byte, not a silicon maximum.
        let m = &mut self.analog.i2c_ana_mst;
        if m.read_i2c0_ctrl().i2c0_busy() {
            return None;
        }
        m.modify_ana_conf2(|w| {
            w.set_phy_host_map(Bits::constant::<0x3fa0>());
        });
        if write.is_none() {
            let route =
                pac::analog_i2c::route(pac::analog_i2c::Profile::LibphyNormal, address.block())?;
            m.publish_phy_i2c_read_mask(Bits::truncate(route.complemented_read_select()));
        }
        m.write_i2c0_ctrl(|w| {
            w.set_slave_addr(address.block())
                .set_slave_reg_addr(address.index())
                .set_data(write.unwrap_or(0))
                .set_read_write(write.is_some())
                .set_start_or_reset(true);
        });
        let start = oer_riscv::cycle32();
        let bound = ((u64::from(self.hz) * 20).div_ceil(1_000_000)) as u32;
        loop {
            let value = m.read_i2c0_ctrl();
            if !value.i2c0_busy() {
                return Some(value.data());
            }
            if oer_riscv::cycle32().wrapping_sub(start) >= bound {
                return None;
            }
        }
    }
}

impl Sar {
    #[inline(always)]
    fn ready(&self) -> u32 {
        self.radio
            .phy_baseband_config_oracle_fe
            .read_power_detector_sar_control_status()
            .sar_ready()
            .get()
    }
    #[inline(always)]
    fn sample(&self) -> u32 {
        self.radio
            .phy_baseband_config_oracle_fe
            .read_power_detector_sar_result()
            .sar_sample()
            .get()
    }
    #[inline(always)]
    fn arm(&mut self, armed: bool) {
        self.radio
            .phy_baseband_config_oracle_fe
            .modify_tone_path_0_control(|w| {
                w.set_tone_enable_or_arm(if armed {
                    fe::tone_path_0_control::ToneEnableOrArm::Armed
                } else {
                    fe::tone_path_0_control::ToneEnableOrArm::Disarmed
                });
            });
    }
    #[inline(always)]
    fn reference(&mut self, value: u16) {
        self.radio
            .phy_baseband_config_oracle_fe
            .write_power_detector_reference(|w| {
                w.set_reference_code(value);
            });
    }
    #[inline(always)]
    fn trigger(&mut self) -> u32 {
        self.radio
            .phy_baseband_config_oracle_fe
            .modify_power_detector_control(|w| {
                w.set_sar_trigger(false);
            });
        // The actual 0->1 PAC store lies between these cycle reads.
        let before = oer_riscv::cycle32();
        self.radio
            .phy_baseband_config_oracle_fe
            .modify_power_detector_control(|w| {
                w.set_sar_trigger(true);
            });
        let after = oer_riscv::cycle32();
        // Defer publication until after the timing-critical observations.
        self.trigger_window = [before, after];
        after
    }
    fn begin(&mut self) -> Option<u32> {
        self.observations.fill((0, 0));
        self.arm(true);
        let arm = oer_riscv::cycle32();
        self.until(arm, 1);
        let trigger = self.trigger();
        // One due observation per cycle-counter boundary, including the
        // vendor's >=2 µs first test. No late wake is treated as a sample.
        let mut ready_at = None;
        for (index, us) in (2..=100).enumerate() {
            self.until(trigger, us);
            let before = oer_riscv::cycle32();
            let ready = self.ready();
            let after = oer_riscv::cycle32();
            if let Some(slot) = self.observations.get_mut(index) {
                *slot = (after.wrapping_sub(trigger), ready);
            }
            if ready == 7 {
                ready_at = Some(before);
                break;
            }
            if after.wrapping_sub(trigger) >= self.cycles(100) {
                break;
            }
        }
        ready_at
    }
    fn publish_poll(&self, tag: u32) {
        emit(tag, self.trigger_window[0], self.trigger_window[1], self.hz);
        for &(elapsed, ready) in &self.observations {
            if elapsed == 0 {
                break;
            }
            emit(tag + 1, elapsed, ready, 0);
        }
    }
    fn configure(&mut self) -> bool {
        if !self.clock() {
            emit(0x14ee, 1, 0, 0);
            return false;
        }
        self.prerequisites();
        let control = self
            .radio
            .phy_baseband_config_oracle_fe
            .read_power_detector_control();
        emit(
            0x1406,
            u32::from(control.bit_16_opaque()),
            self.ready(),
            control.bits_20_22_opaque().get(),
        );
        // Isolate the manual trigger epoch explicitly. SOURCE: production
        // baseband/ops.rs::enable_power_detector_background_control names
        // this background enable; this experiment keeps it disabled.
        self.radio
            .phy_baseband_config_oracle_fe
            .modify_power_detector_control(|w| {
                w.set_bit_16_opaque(false);
            });
        // SOURCE: production phy/sequence.rs::initialize_power_detector,
        // baseband/ops.rs::initialize_power_detector_registers / configure_power_detector_enabled.
        let p = &mut self.radio;
        let fe = &mut p.phy_baseband_config_oracle_fe;
        fe.initialize_power_detector_table_0();
        fe.initialize_power_detector_table_1();
        fe.modify_power_detector_control(|w| {
            w.set_bits_4_11_opaque(0x50);
        });
        fe.write_power_detector_reference(|w| {
            w.set_reference_code(0xaaaa);
        });
        fe.modify_power_detector_control(|w| {
            w.set_bits_20_22_opaque(Bits::constant::<2>());
        });
        p.lp_aon_clkrst.modify_rtc_sar2_pwdet_cct(|w| {
            w.set_rtc_sar2_pwdet_cct(Bits::constant::<4>());
        });
        fe.modify_power_detector_control(|w| {
            w.set_bit_2_opaque(false);
        });
        fe.modify_power_detector_control(|w| {
            w.set_bit_1_opaque(false);
        });
        fe.modify_power_detector_control(|w| {
            w.set_bit_3_opaque(false);
        });
        fe.modify_power_detector_sar_control_status(|w| {
            w.set_bits_12_13_opaque(Bits::constant::<3>());
        });
        fe.modify_power_detector_sar_control_status(|w| {
            w.set_bit_9_opaque(false);
        });
        fe.write_power_detector_reference(|w| {
            w.set_reference_code(0x016a);
        });
        p.lp_aon_clkrst.modify_rtc_sar2_pwdet_cct(|w| {
            w.set_rtc_sar2_pwdet_cct(Bits::constant::<4>());
        });
        // SOURCE: production phy/i2c/commands.rs::Sar2Initialization; preserve
        // the high byte's other nibble through the generated analog field.
        let high = pac::analog_i2c::saradc::adc_sar2_initial_code_high_addr::ADC_SAR2_INITIAL_CODE_HIGH_ADDR;
        let Some(old) = self.analog_byte(high.address(), None) else {
            emit(0x14ee, 2, 0, 0);
            return false;
        };
        let Some(value) = high.insert(old, 5) else {
            emit(0x14ee, 3, old.into(), 0);
            return false;
        };
        if self.analog_byte(high.address(), Some(value)).is_none()
            || self
                .analog_byte(
                    pac::analog_i2c::saradc::adc_sar2_initial_code_low_addr::ADDRESS,
                    Some(0x78),
                )
                .is_none()
        {
            emit(0x14ee, 4, 0, 0);
            return false;
        }
        // SOURCE: production power_detector::configure_calibration_mode and
        // baseband::configure_power_control_tone(0x80, 0x50), used by
        // phy_pwdet_ref_code. No PBus DCO/gain calibration is fabricated here.
        let p = &mut self.radio;
        p.lp_aon_clkrst.modify_rtc_sar2_pwdet_cct(|w| {
            w.set_rtc_sar2_pwdet_cct(Bits::constant::<2>());
        });
        let fe = &mut p.phy_baseband_config_oracle_fe;
        fe.modify_front_end_and_tone_stop_control(|w| {
            w.set_bits_0_1_opaque(Bits::constant::<0>());
        });
        fe.modify_dac_scale_control(|w| {
            w.set_bits_16_23_opaque(0);
        });
        fe.modify_dac_scale_control(|w| {
            w.set_bits_8_15_opaque(0);
        });
        fe.modify_tx_gain_compensation(|w| {
            w.set_bits_0_7_opaque(0)
                .set_bits_8_15_opaque(0)
                .set_bits_16_23_opaque(0)
                .set_bits_24_31_opaque(0);
        });
        fe.write_tx_gain_compensation_aux(|w| {
            w.set_value(0);
        });
        fe.modify_tone_selector_control(|w| {
            w.set_path_0_selector_low(Bits::constant::<0>());
        });
        fe.modify_tone_selector_control(|w| {
            w.set_path_1_selector_low(Bits::constant::<0>());
        });
        fe.modify_tone_path_0_control(|w| {
            w.set_selector_high(0x20)
                .set_bits_8_9_opaque(Bits::constant::<0>())
                .set_negated_step_or_attenuation(0_u8.wrapping_sub(0x50))
                .set_tone_enable_or_arm(fe::tone_path_0_control::ToneEnableOrArm::Armed)
                .set_bits_19_21_opaque(Bits::constant::<0>())
                .set_bits_22_23_opaque(Bits::constant::<0>())
                .set_txiq_polarity_image(Bits::constant::<0>());
        });
        fe.modify_tone_path_1_control(|w| {
            w.set_selector_high(fe::tone_path_1_control::SelectorHigh::Cleared)
                .set_bits_8_9_opaque(fe::tone_path_1_control::Bits8_9Opaque::Cleared)
                .set_negated_step_or_attenuation(
                    fe::tone_path_1_control::NegatedStepOrAttenuation::Cleared,
                )
                .set_tone_enable_or_arm(false)
                .set_bits_19_27_opaque(fe::tone_path_1_control::Bits19_27Opaque::Cleared);
        });
        emit(
            0x1407,
            u32::from(fe.read_power_detector_control().bit_16_opaque()),
            fe.read_power_detector_sar_control_status()
                .bits_12_13_opaque()
                .get(),
            p.lp_aon_clkrst
                .read_rtc_sar2_pwdet_cct()
                .rtc_sar2_pwdet_cct()
                .get(),
        );
        true
    }
    pub fn preflight(&mut self) {
        if !self.configure() {
            self.cleanup();
            return;
        }
        let mut samples = [0_u32; 2];
        for (reference, sample) in [0, 0x5555].into_iter().zip(samples.iter_mut()) {
            self.reference(reference);
            let ready = self.begin();
            if ready.is_some() {
                *sample = self.sample();
            }
            self.publish_poll(0x1402);
            emit(
                0x1404,
                reference.into(),
                *sample,
                u32::from(ready.is_some()),
            );
            if ready.is_none() {
                self.cleanup();
                return;
            }
        }
        self.usable = samples.iter().all(|v| *v != 0);
        self.distinct = self.usable && samples[0] != samples[1];
        emit(
            0x1405,
            u32::from(self.usable),
            u32::from(self.distinct),
            self.ready(),
        );
        if !self.usable {
            self.cleanup();
        }
    }
    pub fn hold(&mut self, delayed: bool, clear: bool) {
        if !self.usable {
            emit(0x14ee, 5, 0, 0);
            return;
        }
        self.reference(0x5555);
        let mut captures = [[0_u32; 5]; 4];
        let Some(ready_at) = self.begin() else {
            self.publish_poll(0x1410);
            self.cleanup();
            return;
        };
        if clear {
            self.arm(false);
        }
        for (us, out) in [0, 1, 10, 100].into_iter().zip(captures.iter_mut()) {
            if delayed && us != 100 {
                continue;
            }
            self.until(ready_at, us);
            let before = oer_riscv::cycle32().wrapping_sub(ready_at);
            let r0 = self.ready();
            let sample = self.sample();
            let r1 = self.ready();
            *out = [
                before,
                r0,
                sample,
                r1,
                oer_riscv::cycle32().wrapping_sub(ready_at),
            ];
        }
        self.publish_poll(0x1410);
        emit(0x1412, u32::from(delayed), u32::from(clear), ready_at);
        for (us, out) in [0, 1, 10, 100].into_iter().zip(captures) {
            if delayed && us != 100 {
                continue;
            }
            emit(0x1413, us, out[0], out[4]);
            emit(0x1414, out[1], out[2], out[3]);
        }
    }
    pub fn edge(&mut self, delayed: bool) {
        if !self.distinct {
            emit(0x14ee, 6, 0, 0);
            return;
        }
        for reference in [0_u16, 0x5555] {
            let ready_before_reference = self.ready();
            let before = self.sample();
            self.reference(reference);
            let after = self.sample();
            let ready_after_reference = self.ready();
            let mut observations = [[0_u32; 5]; 8];
            self.arm(true);
            self.until(oer_riscv::cycle32(), 1);
            let ready_after_arm = self.ready();
            let trigger = self.trigger();
            for (us, out) in [0, 1, 2, 3, 5, 10, 20, 100]
                .into_iter()
                .zip(observations.iter_mut())
            {
                self.until(trigger, us);
                let begin = oer_riscv::cycle32().wrapping_sub(trigger);
                let ready = self.ready();
                let sample = if !delayed || us == 100 {
                    self.sample()
                } else {
                    u32::MAX
                };
                *out = [
                    us,
                    begin,
                    ready,
                    sample,
                    oer_riscv::cycle32().wrapping_sub(trigger),
                ];
            }
            emit(0x1450, reference.into(), before, after);
            emit(
                0x1454,
                ready_before_reference,
                ready_after_reference,
                ready_after_arm,
            );
            emit(0x1451, self.trigger_window[0], trigger, u32::from(delayed));
            for out in observations {
                emit(0x1452, out[0], out[1], out[4]);
                emit(0x1453, out[2], out[3], 0);
            }
        }
    }
    pub fn cleanup(&mut self) {
        // SOURCE: production baseband/ops.rs::stop_power_detector_tone,
        // and the ROM stop-tone DAC-scale restores. Clock ownership stays
        // in this image until the ROM-download reset; no background control.
        self.arm(false);
        let fe = &mut self.radio.phy_baseband_config_oracle_fe;
        fe.modify_tone_path_1_control(|w| {
            w.set_tone_enable_or_arm(false);
        });
        fe.modify_front_end_and_tone_stop_control(|w| {
            w.set_bits_0_1_opaque(Bits::constant::<3>());
        });
        fe.modify_dac_scale_control(|w| {
            w.set_bits_16_23_opaque(0xff);
        });
        fe.modify_dac_scale_control(|w| {
            w.set_bits_8_15_opaque(0xff);
        });
        self.usable = false;
        self.distinct = false;
        emit(0x1490, self.ready(), self.sample(), 0);
    }
}

pub(super) fn dispatch(sar: &mut Sar, id: u32) {
    // This is a probe context observation, not a production scheduling API.
    emit(
        0x14f0,
        id,
        oer_riscv::read_mintstatus() as u32,
        oer_riscv::read_mcause() as u32,
    );
    match id {
        140 => sar.preflight(),
        141 => sar.hold(false, false),
        142 => sar.hold(true, false),
        143 => sar.hold(false, true),
        144 => sar.hold(true, true),
        145 => sar.edge(false),
        146 => sar.edge(true),
        149 => sar.cleanup(),
        _ => emit(0x14ee, 7, id, 0),
    }
}
