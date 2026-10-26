// Finite production prefix through FE/TSENS, before the explicitly omitted
// RC/filter/RFPLL search suffix. See README's preparation-difference ledger.
impl Sar {
    fn check(&self, stage: u32, observed: u32, expected: u32) -> bool {
        emit(0x1408, stage, observed, expected);
        if observed != expected {
            emit(0x14ee, 9, stage, observed);
            return false;
        }
        true
    }
    fn power_readback(&self) -> bool {
        // SOURCE: hal/src/power.rs::execute_owned readback checkpoints.
        let p = &self.radio;
        let hp = p.hp_sys_clkrst_radio.read_modem_conf();
        let clocks = p.modem_syscon_radio.read_clk_conf1();
        let maps = p.modem_syscon_radio.read_clk_conf_power_st();
        let shared = p.modem_lpcon_shared_clock.read_clk_conf_power_st();
        let has = |bits: u32, expected: u32| bits & expected == expected;
        let observations = [
            !p.modem_syscon_radio.read_modem_rst_conf().rst_wifibb()
                && !p.modem_syscon_radio.read_modem_rst_conf().rst_wifimac(),
            p.pmu_radio
                .read_hp_active_icg_modem()
                .hp_active_dig_icg_modem_code_variant()
                == Some(pac::pmu::hp_active_icg_modem::HpActiveDigIcgModemCode::Active),
            p.hp_sys_clkrst_radio.read_modem_ctrl0().reg_modem_clk_en(),
            has(maps.clk_zb_st_map().get(), 4)
                && has(maps.clk_fe_st_map().get(), 6)
                && has(maps.clk_bt_st_map().get(), 4)
                && has(maps.clk_wifi_st_map().get(), 6)
                && has(maps.clk_modem_peri_st_map().get(), 4)
                && has(maps.clk_modem_apb_st_map().get(), 6),
            has(shared.clk_wifipwr_st_map().get(), 6)
                && has(shared.clk_coex_st_map().get(), 6)
                && has(shared.clk_i2c_mst_st_map().get(), 6)
                && has(shared.clk_lp_apb_st_map().get(), 6),
            hp.modem_apb_clk_en()
                && !hp.modem_rst_en()
                && hp.modem_clk_en()
                && hp.modem_clk_source_sel()
                && hp.modem_pll_clk_en()
                && !hp.modem_xtal_clk_en(),
            clocks.clk_wifibb_22m_en()
                && clocks.clk_wifibb_40m_en()
                && clocks.clk_wifibb_44m_en()
                && clocks.clk_wifibb_80m_en()
                && clocks.clk_wifibb_40x_en()
                && clocks.clk_wifibb_80x_en()
                && clocks.clk_wifibb_40x1_en()
                && clocks.clk_wifibb_80x1_en()
                && clocks.clk_wifibb_160x1_en()
                && clocks.clk_wifi_apb_en()
                && clocks.clk_fe_80m_en()
                && clocks.clk_fe_160m_en()
                && clocks.clk_fe_apb_en()
                && clocks.clk_bt_apb_en()
                && clocks.clk_btbb_en()
                && clocks.clk_fe_pwdet_adc_en()
                && clocks.clk_fe_adc_en()
                && clocks.clk_fe_dac_en(),
            p.modem_syscon_radio.read_clk_conf().clk_i2c_mst_sel_160m(),
        ];
        observations
            .into_iter()
            .enumerate()
            .all(|(stage, v)| self.check(stage as u32 + 1, v.into(), 1))
    }
    fn analog_modify(&mut self, field: pac::analog_i2c::Field, value: u8) -> bool {
        let Some(old) = self.analog_byte(field.address(), None) else {
            emit(
                0x14ee,
                2,
                field.address().block().into(),
                field.address().index().into(),
            );
            return false;
        };
        let Some(new) = field.insert(old, value) else {
            emit(0x14ee, 3, old.into(), value.into());
            return false;
        };
        self.analog_byte(field.address(), Some(new)).is_some()
    }
    fn cold_prefix(&mut self) -> bool {
        // SOURCE: phy register calibration clock and PhyRfInitPrefixTransition.
        self.radio.phy_clock_oracle_fe.modify_word_0800_opaque(|w| {
            w.set_bit_2_opaque(true);
        });
        let p = &mut self.radio;
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

        // SOURCE: production i2c/master.rs::set_bbpll_calibration(true).
        self.analog.i2c_ana_mst.modify_ana_conf0(|w| {
            w.set_bbpll_cal_mode(Bits::truncate(
                pac::i2c_ana_mst::ana_conf0::BbpllCalMode::Enabled.bits(),
            ));
        });
        // SOURCE: production i2c/commands.rs::BiasRegisters (two writes).
        if self
            .analog_byte(
                pac::analog_i2c::bias::i2c_bias_dreg_1p6::ADDRESS,
                Some(0xaf),
            )
            .is_none()
            || self
                .analog_byte(
                    pac::analog_i2c::bias::i2c_bias_dreg_1p1_pvt::ADDRESS,
                    Some(0x7f),
                )
                .is_none()
        {
            emit(0x14ee, 4, 1, 0);
            return false;
        }
        // SOURCE: analog_i2c.rs::prepare_open_i2c_pre_delay. Releasing the
        // write-trigger power tie writes nothing in production as well.
        self.radio.pmu_radio.modify_rf_pwc(|w| {
            w.set_xpd_rf_circuit(pac::pmu::rf_pwc::XpdRfCircuit::PoweredOff);
        });
        self.until(oer_riscv::cycle32(), 100);
        let p = &mut self.radio.pmu_radio;
        p.modify_rf_pwc(|w| {
            w.set_xpd_rf_circuit(pac::pmu::rf_pwc::XpdRfCircuit::PoweredOn);
        });
        p.trigger_imm_hp_ck_power_0_tie_high_xpd_bb_i2c();
        if !p.read_ana_peri_pwr_ctrl().xpd_perif_i2c() {
            p.modify_ana_peri_pwr_ctrl(|w| {
                w.set_xpd_perif_i2c(true);
            });
            p.modify_ana_peri_pwr_ctrl(|w| {
                w.set_rstb_perif_i2c(false);
            });
            p.modify_ana_peri_pwr_ctrl(|w| {
                w.set_rstb_perif_i2c(true);
            });
        }
        if !p.read_ana_peri_pwr_ctrl().rstb_perif_i2c() {
            p.modify_ana_peri_pwr_ctrl(|w| {
                w.set_rstb_perif_i2c(true);
            });
        }
        // SOURCE: OpenI2cXpdTransition accepts whole SDM byte 0x5b. This
        // short probe uses a 100 us mcycle limit rather than 9999 MAC ticks.
        let sdm_start = oer_riscv::cycle32();
        loop {
            let value = self.analog_byte(
                pac::analog_i2c::sdm::update_enable::UPDATE_ENABLE.address(),
                None,
            );
            if value == Some(0x5b) {
                emit(
                    0x1409,
                    1,
                    0x5b,
                    oer_riscv::cycle32().wrapping_sub(sdm_start),
                );
                break;
            }
            if value.is_none() || oer_riscv::cycle32().wrapping_sub(sdm_start) >= self.cycles(100) {
                emit(
                    0x14ee,
                    10,
                    value.map_or(u32::MAX, u32::from),
                    oer_riscv::cycle32().wrapping_sub(sdm_start),
                );
                return false;
            }
        }
        self.until(oer_riscv::cycle32(), 10);
        if !self.pbus_clear() {
            return false;
        }
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

        // SOURCE: EnableBbpllCalibration: modify the field, then a second read.
        let adc = pac::analog_i2c::bbpll::rstb_div_adc::ADC_RATE_CONFIGURATION;
        if !self.analog_modify(adc, 0) || self.analog_byte(adc.address(), None).is_none() {
            emit(0x14ee, 4, 2, 0);
            return false;
        }
        if !self.analog_modify(
            adc,
            pac::analog_i2c::bbpll::rstb_div_adc::ADC_RATE_CONFIGURATION_HIGH,
        ) {
            emit(0x14ee, 4, 3, 0);
            return false;
        }
        let bb = &mut self.radio.phy_baseband_config_oracle_fe;
        bb.modify_adc_rate_and_front_end_control(|w| {
            w.set_bit_1_opaque(fe::adc_rate_and_front_end_control::Bit1Opaque::High);
        });
        bb.modify_adc_rate_and_front_end_control(|w| {
            w.set_bit_0_opaque(fe::adc_rate_and_front_end_control::Bit0Opaque::High);
        });
        let master = &mut self.analog.i2c_ana_mst;

        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_ana_conf0(|w| {
            w.set_phy_register_mode(Bits::constant::<2>());
        });
        // SOURCE: ROM_REV0_PHY_I2C, S31_ESP_PACS_SVD.
        master.modify_ana_conf0(|w| {
            w.set_phy_register_enable(true);
        });

        emit(0x1409, 2, 1, 0);
        true
    }
    fn pbus_clear(&mut self) -> bool {
        // SOURCE: phy/src/analog/pbus.rs::PHY_PBUS_CLEAR_TRANSACTIONS.
        self.radio.phy_pbus.modify_mode(|w| {
            w.set_work_mode_enable(fe::mode::WorkModeEnable::Disabled);
        });
        self.radio.phy_pbus.modify_pbus_command(|w| {
            w.set_debug_mode_enable(fe::pbus_command::DebugModeEnable::Enabled);
        });
        for (selector, path, value) in [
            (4, 1, 0),
            (4, 2, 0),
            (5, 1, 0),
            (5, 2, 0),
            (0, 1, 0),
            (0, 2, 0),
            (1, 1, 0),
            (1, 2, 0),
            (2, 1, 0x100),
            (3, 1, 0x100),
            (2, 2, 0x100),
            (3, 2, 0x100),
        ] {
            let bound = self.cycles(20);
            let pbus = &mut self.radio.phy_pbus;
            if pbus.read_status_clock_force().busy() {
                emit(0x14ee, 11, selector, path);
                return false;
            }
            pbus.modify_pbus_command(|w| {
                w.set_transaction_start(true)
                    .set_selector(Bits::truncate(selector))
                    .set_test_value(Bits::truncate(value))
                    .set_path(Bits::truncate(path));
            });
            let start = oer_riscv::cycle32();
            while pbus.read_status_clock_force().busy() {
                if oer_riscv::cycle32().wrapping_sub(start) >= bound {
                    emit(0x14ee, 11, selector, path);
                    return false;
                }
            }
            pbus.modify_pbus_command(|w| {
                w.set_transaction_start(false);
            });
        }
        self.radio.phy_pbus.modify_pbus_command(|w| {
            w.set_debug_mode_enable(fe::pbus_command::DebugModeEnable::Disabled);
        });
        self.radio.phy_pbus.modify_mode(|w| {
            w.set_work_mode_enable(fe::mode::WorkModeEnable::Enabled);
        });
        if self
            .radio
            .modem_syscon_radio
            .read_wifi_bb_cfg()
            .wifi_enable()
        {
            self.until(oer_riscv::cycle32(), 1);
            let agc = &mut self.radio.phy_agc_oracle_wifi_bb;
            agc.modify_agc_shared_control(|w| {
                w.set_forced_gain_index(0x32);
            });
            agc.modify_agc_shared_control(|w| {
                w.set_forced_gain_enable(wifi_bb::agc_shared_control::ForcedGainEnable::Enabled);
            });
            self.until(oer_riscv::cycle32(), 2);
            self.radio
                .phy_agc_oracle_wifi_bb
                .modify_agc_shared_control(|w| {
                    w.set_forced_gain_enable(
                        wifi_bb::agc_shared_control::ForcedGainEnable::Disabled,
                    );
                });
        }
        true
    }
    fn front_end(&mut self) {
        // SOURCE: phy/sequence.rs::initialize_front_end, exact 17 writes.
        let p = &mut self.radio;
        p.phy_pbus.modify_read_result_0(|w| {
            w.set_bit_20_opaque(true);
        });
        // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_word_0c08_opaque(|w| {
                w.set_bit_25_opaque(true);
            });
        // SOURCE: BLOB_LIBPHY_PHY_FE_REG_UPDATE, ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_word_0c08_opaque(|w| {
                w.set_bit_26_opaque(true);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_front_end_clear_control(|w| {
                w.set_bit_8_opaque(false);
            });
        p.phy_clock_oracle_fe.modify_table_memory_index_source(|w| {
            w.set_base_index(0xa0);
        });

        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_front_end_and_tone_stop_control(|w| {
                w.set_bit_2_opaque(true);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_iq_correction_control(|w| {
                w.set_rx_iq_correction_mode_low(true)
                    .set_rx_iq_correction_mode_high(true);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, ROM_REV0_PHY_REGISTER_INITIALIZATION, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_iq_correction_aux(|w| {
                w.set_tx_iq_correction_mode_low(true)
                    .set_tx_iq_correction_mode_high(true);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_front_end_clear_control(|w| {
                w.set_bit_11_opaque(false);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_adc_rate_and_front_end_control(|w| {
                w.set_bit_1_opaque(fe::adc_rate_and_front_end_control::Bit1Opaque::High);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_adc_rate_and_front_end_control(|w| {
                w.set_bit_0_opaque(fe::adc_rate_and_front_end_control::Bit0Opaque::High);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe.modify_tx_pa_control_0(|w| {
            w.set_bits_0_7_opaque(0x4);
        });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_adc_rate_and_front_end_control(|w| {
                w.set_bit_0_opaque(fe::adc_rate_and_front_end_control::Bit0Opaque::High);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_adc_rate_and_front_end_control(|w| {
                w.set_bit_1_opaque(fe::adc_rate_and_front_end_control::Bit1Opaque::High);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_iq_correction_control(|w| {
                w.set_bit_31_opaque(true);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_iq_correction_aux(|w| {
                w.set_bit_15_opaque(true);
            });
        // SOURCE: ROM_REV0_PHY_FE_REG_INIT, S31_ESP_PACS_SVD.
        p.phy_baseband_config_oracle_fe
            .modify_word_0c20_opaque(|w| {
                w.set_bits_0_7_opaque(0x57);
            });
    }
    fn temperature(&mut self) {
        // SOURCE: temperature.rs::Restore::acquire_with(cold=true), first owner.
        let p = &mut self.radio;
        p.lp_tsens.modify_clk_conf(|w| {
            w.set_clk_en(true);
        });
        p.lp_periclkrst.modify_tsens_ctrl(|w| {
            w.set_lp_tsens_clk_en(pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Enabled);
        });
        p.lp_tsens.modify_clk_conf(|w| {
            w.set_bit_23_opaque(true);
        });
        p.lp_tsens.modify_clk_conf(|w| {
            w.set_bit_9_opaque(true);
        });
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(false);
        });
        self.until(oer_riscv::cycle32(), 1);
        let p = &mut self.radio;
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_xpd_perif_i2c(true);
        });
        p.pmu_radio.modify_ana_peri_pwr_ctrl(|w| {
            w.set_rstb_perif_i2c(true);
        });
        p.lp_periclkrst.modify_tsens_ctrl(|w| {
            w.set_lp_tsens_clk_en(pac::lp_peri_clkrst::tsens_ctrl::LpTsensClkEn::Enabled);
        });
        p.lp_periclkrst.modify_tsens_ctrl(|w| {
            w.set_lp_tsens_rst_en(true);
        });
        p.lp_periclkrst.modify_tsens_ctrl(|w| {
            w.set_lp_tsens_rst_en(false);
        });
        p.lp_tsens.modify_ctrl(|w| {
            w.set_power_up(pac::tsens::ctrl::PowerUp::Enabled);
        });
        self.temperature_acquired = true;
    }
    fn enable_pwdet(&mut self) {
        let p = &mut self.radio;
        let fe = &mut p.phy_baseband_config_oracle_fe;
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

        // Production enables background control here. The manual experiment
        // deliberately leaves it disabled before entering reference-tone mode.
    }
}
