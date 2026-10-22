/// Native analog-I2C knowledge; these values perform no hardware access.
pub mod analog_i2c {
    pub use oer_espressif_analog::model::{
        BlockCode as Block, ByteAddress as Address, Host, KnownReset, RegisterFacts, Route,
        SampledByte,
    };
    pub type Field = oer_espressif_analog::AnalogField<Address>;
    pub mod blocks {
        use super::Block;
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD: Block = Block::new(0x61);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const SDM: Block = Block::new(0x63);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_64: Block = Block::new(0x64);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const CKGEN6G: Block = Block::new(0x65);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL: Block = Block::new(0x66);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const BBTOP: Block = Block::new(0x67);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const CKGEN: Block = Block::new(0x68);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC: Block = Block::new(0x69);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const BIAS: Block = Block::new(0x6a);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const TXRF: Block = Block::new(0x6b);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_6C: Block = Block::new(0x6c);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG: Block = Block::new(0x6d);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const TXTOP: Block = Block::new(0x6e);
        /// SOURCE: C5_BLOB_LIBPHY_PHY_I2C
        pub const BLOCK_6F: Block = Block::new(0x6f);
    }
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub enum Profile {
        LibphyNormal,
    }
    /// Resolve the original request before any command-code alias.
    pub const fn route(profile: Profile, request_code: u8) -> Option<Route> {
        match (profile, request_code) {
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x61) => Some(Route {
                request_code: 0x61,
                command_code: 0x61,
                host: Host::Host0,
                read_select: 0x100,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x63) => Some(Route {
                request_code: 0x63,
                command_code: 0x63,
                host: Host::Host1,
                read_select: 0x10,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x64) => Some(Route {
                request_code: 0x64,
                command_code: 0x64,
                host: Host::Host0,
                read_select: 0x0,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x62) => Some(Route {
                request_code: 0x62,
                command_code: 0x65,
                host: Host::Host0,
                read_select: 0x0,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x65) => Some(Route {
                request_code: 0x65,
                command_code: 0x65,
                host: Host::Host1,
                read_select: 0x1000,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x66) => Some(Route {
                request_code: 0x66,
                command_code: 0x66,
                host: Host::Host1,
                read_select: 0x80,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x67) => Some(Route {
                request_code: 0x67,
                command_code: 0x67,
                host: Host::Host0,
                read_select: 0x4,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x68) => Some(Route {
                request_code: 0x68,
                command_code: 0x68,
                host: Host::Host1,
                read_select: 0x20,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x69) => Some(Route {
                request_code: 0x69,
                command_code: 0x69,
                host: Host::Host0,
                read_select: 0x200,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6a) => Some(Route {
                request_code: 0x6a,
                command_code: 0x6a,
                host: Host::Host1,
                read_select: 0x40,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6b) => Some(Route {
                request_code: 0x6b,
                command_code: 0x6b,
                host: Host::Host1,
                read_select: 0x8,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6c) => Some(Route {
                request_code: 0x6c,
                command_code: 0x6c,
                host: Host::Host0,
                read_select: 0x2000,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6d) => Some(Route {
                request_code: 0x6d,
                command_code: 0x6d,
                host: Host::Host0,
                read_select: 0x400,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6e) => Some(Route {
                request_code: 0x6e,
                command_code: 0x6e,
                host: Host::Host1,
                read_select: 0x800,
                read_mask_bits: 32,
            }),
            // SOURCE: C5_BLOB_LIBPHY_PHY_I2C
            (Profile::LibphyNormal, 0x6f) => Some(Route {
                request_code: 0x6f,
                command_code: 0x6f,
                host: Host::Host0,
                read_select: 0x4000,
                read_mask_bits: 32,
            }),
            _ => None,
        }
    }
    pub mod registers {
        use super::Address;
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_IR_RESETB: Address = Address::new(0x61, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_O_DONE_FLAG: Address = Address::new(0x61, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_OCODE: Address = Address::new(0x61, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_BOD_THRESHOLD: Address = Address::new(0x61, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_EXT_CODE: Address = Address::new(0x61, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_DELAY: Address = Address::new(0x66, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_EXT_CAP: Address = Address::new(0x66, 0x1);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_REF_DIV: Address = Address::new(0x66, 0x2);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DIV_7_0: Address = Address::new(0x66, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_RSTB_DIV_ADC: Address = Address::new(0x66, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DR1: Address = Address::new(0x66, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DCUR: Address = Address::new(0x66, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_CAP: Address = Address::new(0x66, 0x8);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_VCO_DBIAS: Address = Address::new(0x66, 0x9);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_BBADC_DCUR: Address = Address::new(0x66, 0xa);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_INITIAL_CODE_LOW_ADDR: Address = Address::new(0x69, 0x0);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_INITIAL_CODE_HIGH_ADDR: Address = Address::new(0x69, 0x1);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_SAMPLE_CYCLE_ADDR: Address = Address::new(0x69, 0x2);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_LOW_ADDR: Address = Address::new(0x69, 0x3);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR: Address = Address::new(0x69, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_SAMPLE_CYCLE_ADDR: Address = Address::new(0x69, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_TSENS_DAC: Address = Address::new(0x69, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_DTEST_RTC_ADDR: Address = Address::new(0x69, 0x7);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC1_EN_TOUT_ADDR: Address = Address::new(0x69, 0x8);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PERIF: Address = Address::new(0x69, 0xa);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PERIF: Address = Address::new(0x69, 0xb);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_XTAL: Address = Address::new(0x69, 0xc);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG: Address = Address::new(0x6d, 0x4);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG_SLEEP: Address = Address::new(0x6d, 0x5);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG: Address = Address::new(0x6d, 0x6);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG_SLEEP: Address = Address::new(0x6d, 0x7);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_OR_EN_CONT_CAL: Address = Address::new(0x6d, 0x9);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_XPD_RTC_REG: Address = Address::new(0x6d, 0xd);
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_SCK_DCAP: Address = Address::new(0x6d, 0xe);
    }
    pub mod fields {
        use super::{Address, Field};
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_IR_RESETB: Field = match Field::new(Address::new(0x61, 0x0), 0, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_IR_FORCE_XPD_CK: Field =
            match Field::new(Address::new(0x61, 0x0), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_IR_FORCE_XPD_IPH: Field =
            match Field::new(Address::new(0x61, 0x0), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_IR_DISABLE_WATCHDOG_CK: Field =
            match Field::new(Address::new(0x61, 0x0), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_O_DONE_FLAG: Field =
            match Field::new(Address::new(0x61, 0x3), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_BG_O_DONE_FLAG: Field =
            match Field::new(Address::new(0x61, 0x3), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_OCODE: Field = match Field::new(Address::new(0x61, 0x4), 7, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_BOD_THRESHOLD: Field = match Field::new(Address::new(0x61, 0x5), 2, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_IR_FORCE_CODE: Field =
            match Field::new(Address::new(0x61, 0x5), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BROWNOUT, ESP_IDF_4D59230D_C5_REGI2C_LP_BIAS
        pub const ULP_BOD_I2C_ULP_EXT_CODE: Field = match Field::new(Address::new(0x61, 0x6), 7, 0)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_DELAY: Field =
            match Field::new(Address::new(0x66, 0x0), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_CK_DIV: Field =
            match Field::new(Address::new(0x66, 0x0), 7, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_EXT_CAP: Field =
            match Field::new(Address::new(0x66, 0x1), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_ENX_CAP: Field =
            match Field::new(Address::new(0x66, 0x1), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_RSTB: Field =
            match Field::new(Address::new(0x66, 0x1), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_START: Field =
            match Field::new(Address::new(0x66, 0x1), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_IR_CAL_UNSTOP: Field =
            match Field::new(Address::new(0x66, 0x1), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_REF_DIV: Field =
            match Field::new(Address::new(0x66, 0x2), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DCHGP: Field = match Field::new(Address::new(0x66, 0x2), 6, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_ENB_FCAL: Field =
            match Field::new(Address::new(0x66, 0x2), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DIV_7_0: Field =
            match Field::new(Address::new(0x66, 0x3), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_RSTB_DIV_ADC: Field =
            match Field::new(Address::new(0x66, 0x4), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_MODE_HF: Field = match Field::new(Address::new(0x66, 0x4), 1, 1) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DIV_ADC: Field = match Field::new(Address::new(0x66, 0x4), 3, 2) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DIV_DAC: Field = match Field::new(Address::new(0x66, 0x4), 4, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DIV_CPU: Field = match Field::new(Address::new(0x66, 0x4), 5, 5) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_ENB_VCON: Field =
            match Field::new(Address::new(0x66, 0x4), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_TSCHGP: Field = match Field::new(Address::new(0x66, 0x4), 7, 7)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DR1: Field = match Field::new(Address::new(0x66, 0x5), 2, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DR3: Field = match Field::new(Address::new(0x66, 0x5), 6, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_EN_USB: Field = match Field::new(Address::new(0x66, 0x5), 7, 7) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DCUR: Field = match Field::new(Address::new(0x66, 0x6), 2, 0) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_INC_CUR: Field = match Field::new(Address::new(0x66, 0x6), 3, 3) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DHREF_SEL: Field =
            match Field::new(Address::new(0x66, 0x6), 5, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_DLREF_SEL: Field =
            match Field::new(Address::new(0x66, 0x6), 7, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_CAP: Field =
            match Field::new(Address::new(0x66, 0x8), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_UDF: Field =
            match Field::new(Address::new(0x66, 0x8), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_OVF: Field =
            match Field::new(Address::new(0x66, 0x8), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_CAL_END: Field =
            match Field::new(Address::new(0x66, 0x8), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OR_LOCK: Field = match Field::new(Address::new(0x66, 0x8), 7, 7) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_OC_VCO_DBIAS: Field =
            match Field::new(Address::new(0x66, 0x9), 1, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_BBADC_DELAY2: Field =
            match Field::new(Address::new(0x66, 0x9), 3, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_BBADC_DVDD: Field =
            match Field::new(Address::new(0x66, 0x9), 5, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_BBADC_DREF: Field =
            match Field::new(Address::new(0x66, 0x9), 7, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_BBADC_DCUR: Field =
            match Field::new(Address::new(0x66, 0xa), 1, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_BBADC_INPUT_SHORT: Field =
            match Field::new(Address::new(0x66, 0xa), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_ENT_PLL: Field = match Field::new(Address::new(0x66, 0xa), 3, 3) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_DTEST: Field = match Field::new(Address::new(0x66, 0xa), 5, 4) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_BBPLL
        pub const BBPLL_I2C_BBPLL_ENT_ADC: Field = match Field::new(Address::new(0x66, 0xa), 7, 6) {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_INITIAL_CODE_LOW_ADDR: Field =
            match Field::new(Address::new(0x69, 0x0), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_INITIAL_CODE_HIGH_ADDR: Field =
            match Field::new(Address::new(0x69, 0x1), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_SAMPLE_CYCLE_ADDR: Field =
            match Field::new(Address::new(0x69, 0x2), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_DREF_ADDR: Field = match Field::new(Address::new(0x69, 0x2), 6, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_LOW_ADDR: Field =
            match Field::new(Address::new(0x69, 0x3), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_INITIAL_CODE_HIGH_ADDR: Field =
            match Field::new(Address::new(0x69, 0x4), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_SAMPLE_CYCLE_ADDR: Field =
            match Field::new(Address::new(0x69, 0x5), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_DREF_ADDR: Field = match Field::new(Address::new(0x69, 0x5), 6, 4)
        {
            Some(field) => field,
            None => panic!("validated analog field"),
        };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_I2C_SARADC_TSENS_DAC: Field =
            match Field::new(Address::new(0x69, 0x6), 3, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_DTEST_RTC_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 1, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_ENT_TSENS_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC1_EN_TOUT_SAR1_BUS_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 4, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC2_EN_TOUT_SAR2_BUS_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 5, 5) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC_ENT_PERIF_ADDR: Field =
            match Field::new(Address::new(0x69, 0x7), 6, 6) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC1_EN_TOUT_ADDR: Field =
            match Field::new(Address::new(0x69, 0x8), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SARADC2_EN_TOUT_ADDR: Field =
            match Field::new(Address::new(0x69, 0x8), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR1_ENCAL_GND_ADDR: Field =
            match Field::new(Address::new(0x69, 0x8), 1, 1) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_ADC_SAR2_ENCAL_GND_ADDR: Field =
            match Field::new(Address::new(0x69, 0x8), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PERIF: Field =
            match Field::new(Address::new(0x69, 0xa), 0, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_VDDPST: Field =
            match Field::new(Address::new(0x69, 0xa), 1, 1) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_XTAL: Field =
            match Field::new(Address::new(0x69, 0xa), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_XPD_VDET_PLL: Field =
            match Field::new(Address::new(0x69, 0xa), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PERIF: Field =
            match Field::new(Address::new(0x69, 0xb), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_VDDPST: Field =
            match Field::new(Address::new(0x69, 0xb), 6, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_XTAL: Field =
            match Field::new(Address::new(0x69, 0xc), 2, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_SARADC
        pub const SARADC_POWER_GLITCH_DREF_VDET_PLL: Field =
            match Field::new(Address::new(0x69, 0xc), 6, 4) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG: Field =
            match Field::new(Address::new(0x6d, 0x4), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENX_RTC_DREG: Field =
            match Field::new(Address::new(0x6d, 0x4), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_RTC_DREG_SLEEP: Field =
            match Field::new(Address::new(0x6d, 0x5), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENIF_RTC_DREG: Field =
            match Field::new(Address::new(0x6d, 0x5), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG: Field =
            match Field::new(Address::new(0x6d, 0x6), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENX_DIG_DREG: Field =
            match Field::new(Address::new(0x6d, 0x6), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_EXT_DIG_DREG_SLEEP: Field =
            match Field::new(Address::new(0x6d, 0x7), 4, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_ENIF_DIG_DREG: Field =
            match Field::new(Address::new(0x6d, 0x7), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_OR_EN_CONT_CAL: Field =
            match Field::new(Address::new(0x6d, 0x9), 7, 7) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_XPD_RTC_REG: Field =
            match Field::new(Address::new(0x6d, 0xd), 2, 2) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_XPD_DIG_REG: Field =
            match Field::new(Address::new(0x6d, 0xd), 3, 3) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
        /// SOURCE: ESP_IDF_4D59230D_C5_REGI2C_DIG_REG
        pub const DIG_REG_I2C_DIG_REG_SCK_DCAP: Field =
            match Field::new(Address::new(0x6d, 0xe), 7, 0) {
                Some(field) => field,
                None => panic!("validated analog field"),
            };
    }
    pub const REGISTERS: [RegisterFacts; 34] = [
        RegisterFacts {
            address: Address::new(0x61, 0x0),
            described: 0x55,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x61, 0x3),
            described: 0x9,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x61, 0x4),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x61, 0x5),
            described: 0x47,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x61, 0x6),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
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
            described: 0xff,
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
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
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
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x8),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0x9),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x66, 0xa),
            described: 0xff,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
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
            read_safe_epoch: None,
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
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x7),
            described: 0x77,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0x8),
            described: 0xf,
            opaque: 0x0,
            reserved: 0x0,
            reset: None,
            read_safe_epoch: None,
        },
        RegisterFacts {
            address: Address::new(0x69, 0xa),
            described: 0xf,
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
    pub const READ_SAFE: &[(Address, &str)] = &[];
}
