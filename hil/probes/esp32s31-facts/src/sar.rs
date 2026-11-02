//! Short SAR experiments; no HAL, RF calibration or background tracking.
//! SOURCE: production hal/src/power.rs::execute_owned, modem/ops.rs,
//! phy/clock/ops.rs, phy/i2c/master.rs, phy/baseband/ops.rs and
//! oer-notes/research/opaque-blobray-phy.md (P-SAR-HOLD / P-SAR-EDGE).
use super::{Bits, emit, pac};
use pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode;
use pac::{fe, wifi_bb};

include!("sar_prepare.rs");

pub(super) struct Sar {
    pub radio: pac::ownership::RadioPhyPeripherals,
    pub clocks: pac::ownership::ChipClockPeripherals,
    pub boot: pac::ownership::BootPeripherals,
    pub analog: pac::ownership::AnalogBusPeripherals,
    hz: u32,
    ticks_per_us: u32,
    trigger_window: [u32; 2],
    observations: [(u32, u32); 99],
    powered: bool,
    temperature_acquired: bool,
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
            powered: false,
            temperature_acquired: false,
            usable: false,
            distinct: false,
        }
    }
    pub(super) fn clock(&mut self) -> bool {
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
    pub(super) fn until(&self, start: u32, us: u32) {
        let bound = self.cycles(us);
        while oer_riscv::cycle32().wrapping_sub(start) < bound {}
    }
    fn prerequisites(&mut self) -> bool {
        let p = &mut self.radio;
        p.pmu_radio.modify_hp_active_icg_modem(|w| {
            w.set_hp_active_dig_icg_modem_code(HpActiveDigIcgModemCode::Active);
        });

        p.pmu_radio.trigger_imm_modem_icg_update_dig_icg_modem_en();

        p.pmu_radio.trigger_imm_sleep_sysclk_update_dig_icg_switch();

        p.hp_sys_clkrst_radio.modify_modem_ctrl0(|w| {
            w.set_reg_modem_clk_en(
                oer_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled,
            );
        });

        p.modem_syscon_clock_reset.modify_clk_conf_power_st(|w| {
            w.set_clk_zb_st_map(Bits::truncate(w.clk_zb_st_map().get() | 4));
            w.set_clk_fe_st_map(Bits::truncate(w.clk_fe_st_map().get() | 6));
            w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
            w.set_clk_wifi_st_map(Bits::truncate(w.clk_wifi_st_map().get() | 6));
            w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
            w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
        });

        p.modem_lpcon_power.modify_clk_conf_power_st(|w| {
            w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
            w.set_clk_coex_st_map(Bits::truncate(w.clk_coex_st_map().get() | 6));
            w.set_clk_i2c_mst_st_map(Bits::truncate(w.clk_i2c_mst_st_map().get() | 6));
            w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
        });

        self.clocks.hp_sys_clkrst_pll160.modify_ref_160m_ctrl0(|w| {
            w.set_reg_ref_160m_clk_en(true);
        });

        // SOURCE: production modem/clock_device.rs::configure_pll_source_gate.
        // PAC policy whole-word images 0x15 then 0x1d; XTAL is disabled.
        p.hp_sys_clkrst_radio.prepare_open_modem_pll_source_gate();
        p.hp_sys_clkrst_radio.open_modem_pll_source_gate();

        p.modem_syscon_clock_reset.modify_modem_rst_conf(|w| {
            w.set_rst_wifibb(pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted);
        });
        p.modem_syscon_clock_reset.modify_modem_rst_conf(|w| {
            w.set_rst_wifibb(pac::modem_syscon::modem_rst_conf::RstWifibb::Released);
        });

        p.modem_syscon_clock_reset.modify_clk_conf1(|w| {
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

        p.modem_syscon_clock_reset.modify_clk_conf(|w| {
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

        self.powered = true;
        self.power_readback()
    }
    pub(super) fn analog_byte(
        &mut self,
        address: pac::analog_i2c::Address,
        write: Option<u8>,
    ) -> Option<u8> {
        // SOURCE: production phy/i2c/master.rs. Resolve command alias and host
        // for every operation, including writes, through the generated route.
        let route =
            pac::analog_i2c::route(pac::analog_i2c::Profile::LibphyNormal, address.block())?;
        let bound = self.cycles(20);
        let m = &mut self.analog.i2c_ana_mst;
        let busy = match route.host {
            pac::analog_i2c::Host::Host0 => m.read_i2c0_ctrl().i2c0_busy(),
            pac::analog_i2c::Host::Host1 => m.read_i2c1_ctrl().i2c1_busy(),
        };
        if busy {
            return None;
        }
        // SOURCE: production master.rs::configure_host_map / ROM_REV0_PHY_I2C_HOST_LANES.
        // Named lanes preserve the former 0x3fa0 field image and untouched bits.
        m.modify_ana_conf2(|w| {
            w.set_bbtop_mst0_select(false)
                .set_txrf_mst0_select(false)
                .set_sdm_mst0_select(false)
                .set_rfpll_mst0_select(false)
                .set_bias_mst0_select(false)
                .set_bbpll_mst0_select(true)
                .set_ulp_bod_mst0_select(false)
                .set_bits_11_12_opaque(Bits::constant::<3>())
                .set_saradc_mst0_select(true)
                .set_bits_14_16_opaque(Bits::constant::<7>())
                .set_dig_reg_mst0_select(true);
        });
        if write.is_none() {
            m.publish_phy_i2c_read_mask(Bits::truncate(route.complemented_read_select()));
        }
        match route.host {
            pac::analog_i2c::Host::Host0 => m.write_i2c0_ctrl(|w| {
                w.set_slave_addr(route.command_code)
                    .set_slave_reg_addr(address.index())
                    .set_data(write.unwrap_or(0))
                    .set_read_write(write.is_some())
                    .set_start_or_reset(true);
            }),
            pac::analog_i2c::Host::Host1 => m.write_i2c1_ctrl(|w| {
                w.set_slave_addr(route.command_code)
                    .set_slave_reg_addr(address.index())
                    .set_data(write.unwrap_or(0))
                    .set_read_write(write.is_some())
                    .set_start_or_reset(true);
            }),
        }
        let start = oer_riscv::cycle32();
        loop {
            let (busy, data) = match route.host {
                pac::analog_i2c::Host::Host0 => {
                    let v = m.read_i2c0_ctrl();
                    (v.i2c0_busy(), v.data())
                }
                pac::analog_i2c::Host::Host1 => {
                    let v = m.read_i2c1_ctrl();
                    (v.i2c1_busy(), v.data())
                }
            };
            if !busy {
                return Some(data);
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
        if !self.prerequisites() || !self.cold_prefix() {
            return false;
        }
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
        // SOURCE: production RF prefix initializes FE and TSENS after PWDET
        // tables. The later conditional SAR2 init is deliberately omitted:
        // reaching it requires RC/filter/RFPLL initialization outside this probe.
        self.front_end();
        self.temperature();
        self.enable_pwdet();
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
        if !self.powered {
            emit(0x1490, u32::MAX, u32::MAX, 0);
            return;
        }
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
        // SOURCE: temperature.rs::Restore::release_with, sole owner.
        if self.temperature_acquired {
            self.radio.lp_tsens.modify_ctrl(|w| {
                w.set_power_up(pac::tsens::ctrl::PowerUp::Disabled);
            });
            self.radio.lp_periclkrst.modify_tsens_ctrl(|w| {
                w.set_lp_tsens_clk_en(pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Disabled);
            });
            self.radio.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
                w.set_xpd_perif_i2c(false);
            });
            self.temperature_acquired = false;
        }
        self.radio.phy_clock_oracle_fe.modify_word_0800_opaque(|w| {
            w.set_bit_2_opaque(false);
        });
    }
}
