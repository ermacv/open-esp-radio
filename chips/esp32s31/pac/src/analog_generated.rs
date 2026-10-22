/// Native analog-I2C knowledge; these values perform no hardware access.
pub mod analog_i2c {
    pub use oer_espressif_analog::model::{
        BlockCode as Block, ByteAddress as Address, CaptureRegister, Host, KnownReset,
        RegisterFacts, Route, SampledByte,
    };
    pub type Field = oer_espressif_analog::AnalogField<Address>;
    pub mod blocks {
        use super::Block;
        /// SOURCE: ESP_IDF_4D59230D_S31_ANALOG_SLAVE_SELECTION
        pub const CPLL: Block = Block::new(0xa);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL: Block = Block::new(0xb);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL: Block = Block::new(0xc);
        /// SOURCE: ESP_IDF_4D59230D_S31_ANALOG_SLAVE_SELECTION
        pub const SAR_MASTER: Block = Block::new(0x10);
        /// SOURCE: ESP_IDF_4D59230D_S31_ANALOG_SLAVE_SELECTION
        pub const SAR_SLAVE: Block = Block::new(0x11);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD: Block = Block::new(0x61);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL: Block = Block::new(0x62);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const SDM: Block = Block::new(0x63);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_64: Block = Block::new(0x64);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_65: Block = Block::new(0x65);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL: Block = Block::new(0x66);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP: Block = Block::new(0x67);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_68: Block = Block::new(0x68);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC: Block = Block::new(0x69);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS: Block = Block::new(0x6a);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF: Block = Block::new(0x6b);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_6C: Block = Block::new(0x6c);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG: Block = Block::new(0x6d);
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Profile {
        LibphyNormal,
    }
    /// Resolve the original request before any command-code alias.
    pub const fn route(profile: Profile, request_code: u8) -> Option<Route> {
        match (profile, request_code) {
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x61) => Some(Route {
                request_code: 0x61,
                command_code: 0x61,
                host: Host::Host1,
                read_select: 0x100,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x62) => Some(Route {
                request_code: 0x62,
                command_code: 0x62,
                host: Host::Host1,
                read_select: 0x20,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x63) => Some(Route {
                request_code: 0x63,
                command_code: 0x63,
                host: Host::Host1,
                read_select: 0x10,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x64) => Some(Route {
                request_code: 0x64,
                command_code: 0x64,
                host: Host::Host0,
                read_select: 0x0,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x65) => Some(Route {
                request_code: 0x65,
                command_code: 0x65,
                host: Host::Host0,
                read_select: 0x0,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x66) => Some(Route {
                request_code: 0x66,
                command_code: 0x66,
                host: Host::Host0,
                read_select: 0x80,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x67) => Some(Route {
                request_code: 0x67,
                command_code: 0x67,
                host: Host::Host1,
                read_select: 0x4,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x68) => Some(Route {
                request_code: 0x68,
                command_code: 0x68,
                host: Host::Host0,
                read_select: 0x0,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x69) => Some(Route {
                request_code: 0x69,
                command_code: 0x69,
                host: Host::Host0,
                read_select: 0x800,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6a) => Some(Route {
                request_code: 0x6a,
                command_code: 0x6a,
                host: Host::Host1,
                read_select: 0x40,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6b) => Some(Route {
                request_code: 0x6b,
                command_code: 0x6b,
                host: Host::Host1,
                read_select: 0x8,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6c) => Some(Route {
                request_code: 0x6c,
                command_code: 0x6c,
                host: Host::Host0,
                read_select: 0x0,
                read_mask_bits: 24,
            }),
            // SOURCE: BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6d) => Some(Route {
                request_code: 0x6d,
                command_code: 0x6d,
                host: Host::Host0,
                read_select: 0x8000,
                read_mask_bits: 24,
            }),
            _ => None,
        }
    }

    /// A validated transport request. Its original code is retained independently
    /// of the physical byte identity, including aliases with another host.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct Request {
        profile: Profile,
        code: u8,
        index: u8,
    }
    impl Request {
        pub const fn new(profile: Profile, code: u8, index: u8) -> Option<Self> {
            match route(profile, code) {
                Some(_) => Some(Self {
                    profile,
                    code,
                    index,
                }),
                None => None,
            }
        }
        pub const fn from_address(profile: Profile, address: Address) -> Option<Self> {
            match route(profile, address.block()) {
                Some(r) if r.command_code == address.block() => {
                    Self::new(profile, address.block(), address.index())
                }
                _ => None,
            }
        }
        pub const fn route(self) -> Route {
            match route(self.profile, self.code) {
                Some(route) => route,
                None => panic!("validated analog request"),
            }
        }
        pub const fn address(self) -> Address {
            self.route().address(self.index)
        }
        pub const fn index(self) -> u8 {
            self.index
        }
    }
    pub mod registers {
        use super::Address;
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL_I2C_MPLL_IR_CAL_EXT_CAP: Address = Address::new(0xb, 0x1);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL_I2C_MPLL_DHREF: Address = Address::new(0xb, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_DELAY: Address = Address::new(0xc, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_EXT_CAP: Address = Address::new(0xc, 0x1);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_CK_DIV: Address = Address::new(0xc, 0x2);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_CAL_CAP: Address = Address::new(0xc, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_OUTPUT_DIV: Address = Address::new(0xc, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_DHREF_SEL: Address = Address::new(0xc, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_DVDD: Address = Address::new(0xc, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_DSDM2: Address = Address::new(0xc, 0x7);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_DSDM1: Address = Address::new(0xc, 0x8);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_DSDM0: Address = Address::new(0xc, 0x9);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD_I2C_BOD_THRESHOLD: Address = Address::new(0x61, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD_XTAL_DUTY_CALIBRATION_PATH_ENABLE: Address = Address::new(0x61, 0x7);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD_RC_CALIBRATION_DOUT_PATH_ENABLE: Address = Address::new(0x61, 0x8);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD_XTAL_DUTY_INITIAL: Address = Address::new(0x61, 0x9);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD_XTAL_DUTY_CANDIDATE: Address = Address::new(0x61, 0xa);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_RESTART_HIGH: Address = Address::new(0x62, 0x0);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CAPACITOR_LOW: Address = Address::new(0x62, 0x1);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CAPACITOR_HIGH: Address = Address::new(0x62, 0x2);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_DCODE_CKGEN_RESET: Address = Address::new(0x62, 0x4);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CALIBRATED_CAPACITOR_LOW: Address = Address::new(0x62, 0x5);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_LOCK_STATUS: Address = Address::new(0x62, 0x7);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CAPACITOR_SEARCH_ENABLE: Address = Address::new(0x62, 0xb);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CAPACITOR_SEARCH_STATUS: Address = Address::new(0x62, 0xc);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CHARGE_PUMP_LOCK_STATUS: Address = Address::new(0x62, 0xe);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE: Address = Address::new(0x62, 0xf);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_INTERNAL_DCODE_0: Address = Address::new(0x62, 0x11);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_INTERNAL_DCODE_1: Address = Address::new(0x62, 0x12);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_DCODE_0_SOURCE_SELECT: Address = Address::new(0x62, 0x13);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_DCODE_1_SOURCE_SELECT: Address = Address::new(0x62, 0x14);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_BYTE_0D_OPAQUE: Address = Address::new(0x62, 0xd);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const RFPLL_BYTE_15_OPAQUE: Address = Address::new(0x62, 0x15);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const SDM_RFPLL_SDM_UPDATE_ENABLE: Address = Address::new(0x63, 0x0);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const SDM_RFPLL_SDM_MOST_SIGNIFICANT_BYTE: Address = Address::new(0x63, 0x3);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const SDM_RFPLL_SDM_UPPER_MIDDLE_BYTE: Address = Address::new(0x63, 0x4);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const SDM_RFPLL_SDM_LOWER_MIDDLE_BYTE: Address = Address::new(0x63, 0x5);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const SDM_RFPLL_SDM_LOW: Address = Address::new(0x63, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_DELAY: Address = Address::new(0x66, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_EXT_CAP: Address = Address::new(0x66, 0x1);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_REF_DIV: Address = Address::new(0x66, 0x2);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DIV_7_0: Address = Address::new(0x66, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_RSTB_DIV_ADC: Address = Address::new(0x66, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DR1: Address = Address::new(0x66, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DHREF_SEL: Address = Address::new(0x66, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_UDF: Address = Address::new(0x66, 0x8);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_VCO_DBIAS: Address = Address::new(0x66, 0x9);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_ENT_PLL: Address = Address::new(0x66, 0xa);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_TX_IQ_LOOPBACK_ENABLE: Address = Address::new(0x67, 0x0);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_RF_CLOSE_CONTROL: Address = Address::new(0x67, 0x2);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_SHARED_RX_GAIN_CALIBRATION_ENABLE: Address = Address::new(0x67, 0x3);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BLUETOOTH_TX_POWER_HIGH_0: Address = Address::new(0x67, 0x1e);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BLUETOOTH_TX_POWER_HIGH_1: Address = Address::new(0x67, 0x1f);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_04_OPAQUE: Address = Address::new(0x67, 0x4);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_05_OPAQUE: Address = Address::new(0x67, 0x5);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_06_OPAQUE: Address = Address::new(0x67, 0x6);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_07_OPAQUE: Address = Address::new(0x67, 0x7);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_0C_OPAQUE: Address = Address::new(0x67, 0xc);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_0D_OPAQUE: Address = Address::new(0x67, 0xd);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_0E_OPAQUE: Address = Address::new(0x67, 0xe);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_0F_OPAQUE: Address = Address::new(0x67, 0xf);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_14_OPAQUE: Address = Address::new(0x67, 0x14);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_15_OPAQUE: Address = Address::new(0x67, 0x15);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_16_OPAQUE: Address = Address::new(0x67, 0x16);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_17_OPAQUE: Address = Address::new(0x67, 0x17);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_18_OPAQUE: Address = Address::new(0x67, 0x18);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_19_OPAQUE: Address = Address::new(0x67, 0x19);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_1C_OPAQUE: Address = Address::new(0x67, 0x1c);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BYTE_1D_OPAQUE: Address = Address::new(0x67, 0x1d);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_SAR1_INIT_CODE_LSB: Address = Address::new(0x69, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_SAR1_INIT_CODE_MSB: Address = Address::new(0x69, 0x1);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_SAMPLE_CYCLE_ADDR: Address = Address::new(0x69, 0x2);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_LOW_ADDR: Address = Address::new(0x69, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR: Address = Address::new(0x69, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_SAMPLE_CYCLE_ADDR: Address = Address::new(0x69, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_TSENS_DAC: Address = Address::new(0x69, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_DTEST_RTC_ADDR: Address = Address::new(0x69, 0x7);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PERIF: Address = Address::new(0x69, 0xa);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PERIF: Address = Address::new(0x69, 0xb);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PLLBB: Address = Address::new(0x69, 0xc);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS_I2C_BIAS_DREG_1P6: Address = Address::new(0x6a, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS_I2C_BIAS_DREG_1P1_PVT: Address = Address::new(0x6a, 0x1);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_TX_CAPACITOR_LOW: Address = Address::new(0x6b, 0x2);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_WIFI_TX_TEMPERATURE_TRACKING_0: Address = Address::new(0x6b, 0x3);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_WIFI_TX_TEMPERATURE_TRACKING_1: Address = Address::new(0x6b, 0x7);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CONFIGURATION_1: Address = Address::new(0x6b, 0xf);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CONFIGURATION_0: Address = Address::new(0x6b, 0x11);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CALIBRATION_ENABLE: Address = Address::new(0x6b, 0x13);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CALIBRATION_RESULT: Address = Address::new(0x6b, 0x14);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_01_OPAQUE: Address = Address::new(0x6b, 0x1);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_04_OPAQUE: Address = Address::new(0x6b, 0x4);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_05_OPAQUE: Address = Address::new(0x6b, 0x5);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_06_OPAQUE: Address = Address::new(0x6b, 0x6);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_08_OPAQUE: Address = Address::new(0x6b, 0x8);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_09_OPAQUE: Address = Address::new(0x6b, 0x9);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_0A_OPAQUE: Address = Address::new(0x6b, 0xa);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_0B_OPAQUE: Address = Address::new(0x6b, 0xb);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_0C_OPAQUE: Address = Address::new(0x6b, 0xc);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_0D_OPAQUE: Address = Address::new(0x6b, 0xd);
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_BYTE_0E_OPAQUE: Address = Address::new(0x6b, 0xe);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG: Address = Address::new(0x6d, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG_SLEEP: Address = Address::new(0x6d, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG: Address = Address::new(0x6d, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG_SLEEP: Address = Address::new(0x6d, 0x7);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_OR_EN_CONT_CAL: Address = Address::new(0x6d, 0x9);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_XPD_RTC_REG: Address = Address::new(0x6d, 0xd);
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_SCK_DCAP: Address = Address::new(0x6d, 0xe);
    }
    pub mod fields {
        use super::{Address, Field};
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL_I2C_MPLL_IR_CAL_EXT_CAP: Field =
            match Field::new(Address::new(0xb, 0x1), 1, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL_I2C_MPLL_IR_CAL_ENX_CAP: Field =
            match Field::new(Address::new(0xb, 0x1), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL_I2C_MPLL_IR_CAL_RSTB: Field = match Field::new(Address::new(0xb, 0x1), 5, 5)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_MPLL
        pub const MPLL_I2C_MPLL_DHREF: Field = match Field::new(Address::new(0xb, 0x3), 5, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_DELAY: Field = match Field::new(Address::new(0xc, 0x0), 3, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_RSTB: Field = match Field::new(Address::new(0xc, 0x0), 4, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_START: Field = match Field::new(Address::new(0xc, 0x0), 5, 5)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_UNSTOP: Field =
            match Field::new(Address::new(0xc, 0x0), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_ENB_FCAL: Field = match Field::new(Address::new(0xc, 0x0), 7, 7)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_EXT_CAP: Field =
            match Field::new(Address::new(0xc, 0x1), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_ENX_CAP: Field =
            match Field::new(Address::new(0xc, 0x1), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_LBW: Field = match Field::new(Address::new(0xc, 0x1), 6, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_IR_CAL_CK_DIV: Field =
            match Field::new(Address::new(0xc, 0x2), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_DCHGP: Field = match Field::new(Address::new(0xc, 0x2), 6, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_ENB_VCON: Field = match Field::new(Address::new(0xc, 0x2), 7, 7)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_CAL_CAP: Field = match Field::new(Address::new(0xc, 0x3), 4, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_CAL_UDF: Field = match Field::new(Address::new(0xc, 0x3), 5, 5) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_CAL_OVF: Field = match Field::new(Address::new(0xc, 0x3), 6, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_CAL_END: Field = match Field::new(Address::new(0xc, 0x3), 7, 7) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OR_OUTPUT_DIV: Field =
            match Field::new(Address::new(0xc, 0x4), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_TSCHGP: Field = match Field::new(Address::new(0xc, 0x4), 6, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_EN_FAST_CAL: Field = match Field::new(Address::new(0xc, 0x4), 7, 7)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_DHREF_SEL: Field = match Field::new(Address::new(0xc, 0x5), 1, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_DLREF_SEL: Field = match Field::new(Address::new(0xc, 0x5), 3, 2)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_SDM_DITHER: Field = match Field::new(Address::new(0xc, 0x5), 4, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_SDM_STOP: Field = match Field::new(Address::new(0xc, 0x5), 5, 5) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_SDM_RSTB: Field = match Field::new(Address::new(0xc, 0x5), 6, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_DVDD: Field = match Field::new(Address::new(0xc, 0x6), 4, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_OC_REF_DIV: Field = match Field::new(Address::new(0xc, 0x6), 7, 5) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_DSDM2: Field = match Field::new(Address::new(0xc, 0x7), 5, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_DSDM1: Field = match Field::new(Address::new(0xc, 0x8), 7, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_APLL
        pub const APLL_I2C_APLL_DSDM0: Field = match Field::new(Address::new(0xc, 0x9), 7, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BROWNOUT
        pub const ULP_BOD_I2C_BOD_THRESHOLD: Field = match Field::new(Address::new(0x61, 0x5), 2, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_XTAL_DUTY_CAL
        pub const ULP_BOD_XTAL_DUTY_CALIBRATION_PATH_ENABLE: Field =
            match Field::new(Address::new(0x61, 0x7), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const ULP_BOD_RC_CALIBRATION_DOUT_PATH_ENABLE: Field =
            match Field::new(Address::new(0x61, 0x8), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_XTAL_DUTY_CAL
        pub const ULP_BOD_XTAL_DUTY_INITIAL: Field = match Field::new(Address::new(0x61, 0x9), 5, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_RESTART_HIGH: Field = match Field::new(Address::new(0x62, 0x0), 6, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_RESTART_LOW: Field = match Field::new(Address::new(0x62, 0x0), 5, 5) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CAPACITOR_HIGH: Field = match Field::new(Address::new(0x62, 0x2), 6, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_INITIAL_CONFIGURATION_HIGH: Field =
            match Field::new(Address::new(0x62, 0x2), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_INITIAL_CONFIGURATION_LOW: Field =
            match Field::new(Address::new(0x62, 0x2), 5, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_DCODE_CKGEN_RESET: Field = match Field::new(Address::new(0x62, 0x4), 7, 7) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_LOCK_STATUS: Field = match Field::new(Address::new(0x62, 0x7), 1, 1) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CALIBRATED_CAPACITOR_HIGH: Field =
            match Field::new(Address::new(0x62, 0x7), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CAPACITOR_SEARCH_ENABLE: Field =
            match Field::new(Address::new(0x62, 0xb), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CAPACITOR_SEARCH_STATUS: Field =
            match Field::new(Address::new(0x62, 0xc), 3, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CHARGE_PUMP_LOCK_STATUS: Field =
            match Field::new(Address::new(0x62, 0xe), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CHARGE_PUMP_RESULT: Field = match Field::new(Address::new(0x62, 0xe), 4, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CHARGE_PUMP_CALIBRATION_ENABLE: Field =
            match Field::new(Address::new(0x62, 0xf), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CHARGE_PUMP_CALIBRATION_PULSE: Field =
            match Field::new(Address::new(0x62, 0xf), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_CHARGE_PUMP_VALUE: Field = match Field::new(Address::new(0x62, 0xf), 4, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_INTERNAL_DCODE_0: Field = match Field::new(Address::new(0x62, 0x11), 5, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_INTERNAL_DCODE_1: Field = match Field::new(Address::new(0x62, 0x12), 5, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_DCODE_0_SOURCE_SELECT: Field =
            match Field::new(Address::new(0x62, 0x13), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_EXTERNAL_DCODE_0: Field = match Field::new(Address::new(0x62, 0x13), 5, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_DCODE_1_SOURCE_SELECT: Field =
            match Field::new(Address::new(0x62, 0x14), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const RFPLL_EXTERNAL_DCODE_1: Field = match Field::new(Address::new(0x62, 0x14), 5, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const SDM_RFPLL_SDM_UPDATE_ENABLE: Field =
            match Field::new(Address::new(0x63, 0x0), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_RFPLL_CHANNEL
        pub const SDM_RFPLL_SDM_LOW: Field = match Field::new(Address::new(0x63, 0x6), 2, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_DELAY: Field =
            match Field::new(Address::new(0x66, 0x0), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_CK_DIV: Field =
            match Field::new(Address::new(0x66, 0x0), 7, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_EXT_CAP: Field =
            match Field::new(Address::new(0x66, 0x1), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_ENX_CAP: Field =
            match Field::new(Address::new(0x66, 0x1), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_RSTB: Field =
            match Field::new(Address::new(0x66, 0x1), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_START: Field =
            match Field::new(Address::new(0x66, 0x1), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_UNSTOP: Field =
            match Field::new(Address::new(0x66, 0x1), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_REF_DIV: Field =
            match Field::new(Address::new(0x66, 0x2), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DCHGP: Field = match Field::new(Address::new(0x66, 0x2), 6, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DIV_7_0: Field =
            match Field::new(Address::new(0x66, 0x3), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_RSTB_DIV_ADC: Field =
            match Field::new(Address::new(0x66, 0x4), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_MODE_HF: Field = match Field::new(Address::new(0x66, 0x4), 1, 1) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DIV_DAC: Field = match Field::new(Address::new(0x66, 0x4), 4, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DIV_CPU: Field = match Field::new(Address::new(0x66, 0x4), 5, 5) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBPLL_ADC_RATE_CONFIGURATION: Field =
            match Field::new(Address::new(0x66, 0x4), 3, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DR1: Field = match Field::new(Address::new(0x66, 0x5), 2, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DR3: Field = match Field::new(Address::new(0x66, 0x5), 6, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_EN_USB: Field = match Field::new(Address::new(0x66, 0x5), 7, 7) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DHREF_SEL: Field =
            match Field::new(Address::new(0x66, 0x6), 5, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DLREF_SEL: Field =
            match Field::new(Address::new(0x66, 0x6), 7, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_UDF: Field =
            match Field::new(Address::new(0x66, 0x8), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_OVF: Field =
            match Field::new(Address::new(0x66, 0x8), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_END: Field =
            match Field::new(Address::new(0x66, 0x8), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_LOCK: Field = match Field::new(Address::new(0x66, 0x8), 7, 7) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_VCO_DBIAS: Field =
            match Field::new(Address::new(0x66, 0x9), 1, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_ENT_PLL: Field = match Field::new(Address::new(0x66, 0xa), 3, 3) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DTEST: Field = match Field::new(Address::new(0x66, 0xa), 5, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_RXIQ_CAL_INIT
        pub const BBTOP_TX_IQ_LOOPBACK_ENABLE: Field =
            match Field::new(Address::new(0x67, 0x0), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_SHARED_RX_GAIN_CALIBRATION_ENABLE: Field =
            match Field::new(Address::new(0x67, 0x3), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS
        pub const BBTOP_BLUETOOTH_TX_POWER_HIGH_0: Field =
            match Field::new(Address::new(0x67, 0x1e), 5, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const BBTOP_BLUETOOTH_TX_POWER_HIGH_0_BIT_6_OPAQUE: Field =
            match Field::new(Address::new(0x67, 0x1e), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBBLE_PHY_INIT_REGISTERS
        pub const BBTOP_BLUETOOTH_TX_POWER_HIGH_1: Field =
            match Field::new(Address::new(0x67, 0x1f), 5, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_SAR1_INIT_CODE_LSB: Field =
            match Field::new(Address::new(0x69, 0x0), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_SAR1_INIT_CODE_MSB: Field =
            match Field::new(Address::new(0x69, 0x1), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_SAMPLE_CYCLE_ADDR: Field =
            match Field::new(Address::new(0x69, 0x2), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_DREF_ADDR: Field = match Field::new(Address::new(0x69, 0x2), 6, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_LOW_ADDR: Field =
            match Field::new(Address::new(0x69, 0x3), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR: Field =
            match Field::new(Address::new(0x69, 0x4), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_SAMPLE_CYCLE_ADDR: Field =
            match Field::new(Address::new(0x69, 0x5), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_DREF_ADDR: Field = match Field::new(Address::new(0x69, 0x5), 6, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_TSENS_DAC: Field =
            match Field::new(Address::new(0x69, 0x6), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_DTEST_RTC_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 1, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_ENT_TSENS_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PERIF: Field =
            match Field::new(Address::new(0x69, 0xa), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_VDDPST: Field =
            match Field::new(Address::new(0x69, 0xa), 1, 1) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PLLBB: Field =
            match Field::new(Address::new(0x69, 0xa), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PLL: Field =
            match Field::new(Address::new(0x69, 0xa), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_MODE_VDET_PERIF: Field =
            match Field::new(Address::new(0x69, 0xa), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_MODE_VDET_VDDPST: Field =
            match Field::new(Address::new(0x69, 0xa), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_MODE_VDET_PLLBB: Field =
            match Field::new(Address::new(0x69, 0xa), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_MODE_VDET_PLL: Field =
            match Field::new(Address::new(0x69, 0xa), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PERIF: Field =
            match Field::new(Address::new(0x69, 0xb), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_VDDPST: Field =
            match Field::new(Address::new(0x69, 0xb), 6, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PLLBB: Field =
            match Field::new(Address::new(0x69, 0xc), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PLL: Field =
            match Field::new(Address::new(0x69, 0xc), 6, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS_I2C_BIAS_DREG_1P6: Field = match Field::new(Address::new(0x6a, 0x0), 3, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS_I2C_BIAS_DREG_1P1: Field = match Field::new(Address::new(0x6a, 0x0), 7, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS_I2C_BIAS_DREG_1P1_PVT: Field =
            match Field::new(Address::new(0x6a, 0x1), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_BIAS
        pub const BIAS_I2C_BIAS_DREG_2P2_PVT: Field =
            match Field::new(Address::new(0x6a, 0x1), 7, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_TX_CAPACITOR_LOW: Field = match Field::new(Address::new(0x6b, 0x2), 3, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_TX_CAPACITOR_HIGH: Field = match Field::new(Address::new(0x6b, 0x2), 7, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_WIFI_TX_TEMPERATURE_TRACKING_0: Field =
            match Field::new(Address::new(0x6b, 0x3), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_WIFI_TX_TEMPERATURE_TRACKING_1: Field =
            match Field::new(Address::new(0x6b, 0x7), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CONFIGURATION_1: Field = match Field::new(Address::new(0x6b, 0xf), 7, 3) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CONFIGURATION_0: Field = match Field::new(Address::new(0x6b, 0x11), 5, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CALIBRATION_ENABLE: Field =
            match Field::new(Address::new(0x6b, 0x13), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CALIBRATION_PULSE: Field =
            match Field::new(Address::new(0x6b, 0x13), 1, 1) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CONFIGURATION_2: Field = match Field::new(Address::new(0x6b, 0x13), 5, 2)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: BLOB_LIBPHY_PHY_I2C
        pub const TXRF_RC_CALIBRATION_RESULT: Field =
            match Field::new(Address::new(0x6b, 0x14), 5, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG: Field =
            match Field::new(Address::new(0x6d, 0x4), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENX_RTC_DREG: Field =
            match Field::new(Address::new(0x6d, 0x4), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG_SLEEP: Field =
            match Field::new(Address::new(0x6d, 0x5), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENIF_RTC_DREG: Field =
            match Field::new(Address::new(0x6d, 0x5), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG: Field =
            match Field::new(Address::new(0x6d, 0x6), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENX_DIG_DREG: Field =
            match Field::new(Address::new(0x6d, 0x6), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG_SLEEP: Field =
            match Field::new(Address::new(0x6d, 0x7), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENIF_DIG_DREG: Field =
            match Field::new(Address::new(0x6d, 0x7), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_OR_EN_CONT_CAL: Field =
            match Field::new(Address::new(0x6d, 0x9), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_XPD_RTC_REG: Field =
            match Field::new(Address::new(0x6d, 0xd), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_XPD_DIG_REG: Field =
            match Field::new(Address::new(0x6d, 0xd), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_S31_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_SCK_DCAP: Field =
            match Field::new(Address::new(0x6d, 0xe), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
    }
    pub const REGISTERS: [RegisterFacts; 107] = [
        RegisterFacts {
            address: Address::new(0xb, 0x1),
            described: 0x27,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xb, 0x3),
            described: 0x30,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x0),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x1),
            described: 0x7f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x2),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x3),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x4),
            described: 0xdf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x5),
            described: 0x7f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x6),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x7),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x8),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0xc, 0x9),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x61, 0x5),
            described: 0x7,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x61, 0x7),
            described: 0x20,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x61, 0x8),
            described: 0x4,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x61, 0x9),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x61, 0xa),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x0),
            described: 0x60,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x1),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x2),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x4),
            described: 0x80,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x5),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x7),
            described: 0x6,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0xb),
            described: 0x40,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0xc),
            described: 0xc,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0xd),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x62, 0xe),
            described: 0x9f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0xf),
            described: 0x7f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x11),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x12),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x13),
            described: 0x7f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x14),
            described: 0x7f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x62, 0x15),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x63, 0x0),
            described: 0x8,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x63, 0x3),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x63, 0x4),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x63, 0x5),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x63, 0x6),
            described: 0x7,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x66, 0x0),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x1),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x2),
            described: 0x7f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x3),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x4),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x66, 0x5),
            described: 0xf7,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x6),
            described: 0xf0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x8),
            described: 0xf0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x9),
            described: 0x3,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0xa),
            described: 0x38,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x0),
            described: 0x40,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x67, 0x2),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x67, 0x3),
            described: 0x4,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x67, 0x4),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x5),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x6),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x7),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0xc),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0xd),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0xe),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0xf),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x14),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x15),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x16),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x17),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x18),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x19),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x1c),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x1d),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x67, 0x1e),
            described: 0x3f,
            opaque: 0x40,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x67, 0x1f),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x69, 0x0),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x1),
            described: 0xf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x2),
            described: 0x77,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x3),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x4),
            described: 0xf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x69, 0x5),
            described: 0x77,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x6),
            described: 0xf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x69, 0x7),
            described: 0x7,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0xa),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0xb),
            described: 0x77,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0xc),
            described: 0x77,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6a, 0x0),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6a, 0x1),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x1),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x2),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x3),
            described: 0xf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x4),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x5),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x6),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x7),
            described: 0xf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x8),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x9),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0xa),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0xb),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0xc),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0xd),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0xe),
            described: 0x0,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6b, 0xf),
            described: 0xf8,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x11),
            described: 0x30,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x13),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6b, 0x14),
            described: 0x3f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: Some("r1-calibrated-radio"),
        },
        RegisterFacts {
            address: Address::new(0x6d, 0x4),
            described: 0x9f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6d, 0x5),
            described: 0x9f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6d, 0x6),
            described: 0x9f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6d, 0x7),
            described: 0x9f,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6d, 0x9),
            described: 0x80,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6d, 0xd),
            described: 0xc,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x6d, 0xe),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
    ];
    /// Registers with an independently reviewed finite read-safe epoch.
    pub const READ_SAFE: &[(Address, &str)] = &[
        (Address::new(0x61, 0x7), "r1-calibrated-radio"),
        (Address::new(0x61, 0x8), "r1-calibrated-radio"),
        (Address::new(0x61, 0x9), "r1-calibrated-radio"),
        (Address::new(0x61, 0xa), "r1-calibrated-radio"),
        (Address::new(0x62, 0x0), "r1-calibrated-radio"),
        (Address::new(0x62, 0x1), "r1-calibrated-radio"),
        (Address::new(0x62, 0x2), "r1-calibrated-radio"),
        (Address::new(0x62, 0x4), "r1-calibrated-radio"),
        (Address::new(0x62, 0x5), "r1-calibrated-radio"),
        (Address::new(0x62, 0x7), "r1-calibrated-radio"),
        (Address::new(0x62, 0xb), "r1-calibrated-radio"),
        (Address::new(0x62, 0xc), "r1-calibrated-radio"),
        (Address::new(0x62, 0xe), "r1-calibrated-radio"),
        (Address::new(0x62, 0xf), "r1-calibrated-radio"),
        (Address::new(0x62, 0x11), "r1-calibrated-radio"),
        (Address::new(0x62, 0x12), "r1-calibrated-radio"),
        (Address::new(0x62, 0x13), "r1-calibrated-radio"),
        (Address::new(0x62, 0x14), "r1-calibrated-radio"),
        (Address::new(0x63, 0x0), "r1-calibrated-radio"),
        (Address::new(0x63, 0x3), "r1-calibrated-radio"),
        (Address::new(0x63, 0x4), "r1-calibrated-radio"),
        (Address::new(0x63, 0x5), "r1-calibrated-radio"),
        (Address::new(0x63, 0x6), "r1-calibrated-radio"),
        (Address::new(0x66, 0x4), "r1-calibrated-radio"),
        (Address::new(0x67, 0x0), "r1-calibrated-radio"),
        (Address::new(0x67, 0x2), "r1-calibrated-radio"),
        (Address::new(0x67, 0x3), "r1-calibrated-radio"),
        (Address::new(0x67, 0x1e), "r1-calibrated-radio"),
        (Address::new(0x67, 0x1f), "r1-calibrated-radio"),
        (Address::new(0x69, 0x4), "r1-calibrated-radio"),
        (Address::new(0x69, 0x6), "r1-calibrated-radio"),
        (Address::new(0x6a, 0x0), "r1-calibrated-radio"),
        (Address::new(0x6a, 0x1), "r1-calibrated-radio"),
        (Address::new(0x6b, 0x2), "r1-calibrated-radio"),
        (Address::new(0x6b, 0x3), "r1-calibrated-radio"),
        (Address::new(0x6b, 0x7), "r1-calibrated-radio"),
        (Address::new(0x6b, 0xf), "r1-calibrated-radio"),
        (Address::new(0x6b, 0x11), "r1-calibrated-radio"),
        (Address::new(0x6b, 0x13), "r1-calibrated-radio"),
        (Address::new(0x6b, 0x14), "r1-calibrated-radio"),
    ];
    /// The qualified comparison image, ordered by physical transport identity.
    pub const CAPTURE: &[CaptureRegister] = &[
        CaptureRegister {
            address: Address::new(0x61, 0x7),
            mask: 0x20,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x61, 0x8),
            mask: 0x4,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x61, 0x9),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x61, 0xa),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x0),
            mask: 0x60,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x1),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x2),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x4),
            mask: 0x80,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x5),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x7),
            mask: 0x6,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0xb),
            mask: 0x40,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0xc),
            mask: 0xc,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0xe),
            mask: 0x9f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0xf),
            mask: 0x7f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x11),
            mask: 0x3f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x12),
            mask: 0x3f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x13),
            mask: 0x7f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x62, 0x14),
            mask: 0x7f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x63, 0x0),
            mask: 0x8,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x63, 0x3),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x63, 0x4),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x63, 0x5),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x63, 0x6),
            mask: 0x7,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x66, 0x4),
            mask: 0xc,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x67, 0x0),
            mask: 0x40,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x67, 0x2),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x67, 0x3),
            mask: 0x4,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x67, 0x1e),
            mask: 0x7f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x67, 0x1f),
            mask: 0x3f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x69, 0x4),
            mask: 0xf,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x69, 0x6),
            mask: 0xf,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6a, 0x0),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6a, 0x1),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0x2),
            mask: 0xff,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0x3),
            mask: 0xf,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0x7),
            mask: 0xf,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0xf),
            mask: 0xf8,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0x11),
            mask: 0x30,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0x13),
            mask: 0x3f,
            epoch: "r1-calibrated-radio",
        },
        CaptureRegister {
            address: Address::new(0x6b, 0x14),
            mask: 0x3f,
            epoch: "r1-calibrated-radio",
        },
    ];
}
