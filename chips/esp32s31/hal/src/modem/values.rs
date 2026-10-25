//! Value domains of the recovered modem and PHY operations.
#![forbid(unsafe_code)]

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Boolean state accepted by reviewed MODEM_SYSCON clock-gate transactions.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModemSysconClockGateState {
    /// The selected MODEM_SYSCON clock gate is disabled.
    Disabled,
    /// The selected MODEM_SYSCON clock gate is enabled.
    Enabled,
}
impl ModemSysconClockGateState {
    /// Numeric image for diagnostics and HAL diagnostics.
    pub const fn bits(self) -> u32 {
        match self {
            Self::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Disabled.bits()
            }
            Self::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled.bits()
            }
        }
    }
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Boolean state accepted by reviewed MODEM_SYSCON functional enable fields.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModemSysconEnableState {
    /// The selected MODEM_SYSCON function is disabled.
    Disabled,
    /// The selected MODEM_SYSCON function is enabled.
    Enabled,
}
impl ModemSysconEnableState {
    /// Numeric image for diagnostics and HAL diagnostics.
    pub const fn bits(self) -> u32 {
        match self {
            Self::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Disabled.bits()
            }
            Self::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled.bits()
            }
        }
    }
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Boolean state accepted by reviewed MODEM_SYSCON reset transactions.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ModemSysconResetState {
    /// The selected MODEM_SYSCON reset is released.
    Released,
    /// The selected MODEM_SYSCON reset is asserted.
    Asserted,
}
impl ModemSysconResetState {
    /// Numeric image for diagnostics and HAL diagnostics.
    pub const fn bits(self) -> u32 {
        match self {
            Self::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifibb::Released.bits()
            }
            Self::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted.bits()
            }
        }
    }
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Reviewed MODEM_SYSCON Wi-Fi digital baseband 40 MHz selection.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WifiBaseband40MhzState {
    /// Select the reviewed non-40-MHz digital baseband state.
    Disabled,
    /// Select the reviewed 40 MHz digital baseband state.
    Enabled,
}
impl WifiBaseband40MhzState {
    /// Numeric image for diagnostics and HAL diagnostics.
    pub const fn bits(self) -> u32 {
        match self {
            Self::Disabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital::Disabled.bits()
            }
            Self::Enabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital::Enabled.bits()
            }
        }
    }
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Reviewed MODEM_SYSCON Wi-Fi baseband AGC update modes.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum WifiBasebandAgcUpdateMode {
    /// Select the initialization update mode.
    Initialization,
    /// Enable subsequent baseband register updates.
    RegisterUpdatesEnabled,
}
impl WifiBasebandAgcUpdateMode {
    /// Numeric image for diagnostics and HAL diagnostics.
    pub const fn bits(self) -> u32 {
        match self {
            Self::Initialization => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque::Initialization.bits()
            }
            Self::RegisterUpdatesEnabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque::RegisterUpdatesEnabled
                    .bits()
            }
        }
    }
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Twelve-bit divider accepted by the canonical MODEM_LPCON low-power timer selector.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModemLowPowerClockDivider(u32);
impl ModemLowPowerClockDivider {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<
            { oer_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_WIDTH },
        >,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// Twelve-bit coexistence timer clock divider image, the divisor minus one, that complete coex_hw_timer_freq_set publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerClockDividerMinusOne(u32);
impl CoexTimerClockDividerMinusOne {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::modem_lpcon::coex_lp_clk_conf::CLK_COEX_LP_DIV_NUM_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<
            { oer_esp32s31_pac::modem_lpcon::coex_lp_clk_conf::CLK_COEX_LP_DIV_NUM_WIDTH },
        >,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// One-hot coexistence timer clock selector that complete coex_hw_timer_freq_set publishes in the low nibble of COEX_LP_CLK_CONF.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CoexTimerClockSource {
    /// Selector 1, accepted with a divisor of at least 3.
    Selector1 = 0x00000001,
    /// Selector 2, accepted with a divisor of at least 40.
    Selector2 = 0x00000002,
    /// Selector 4, the crystal clock coex_core_pre_init selects on a real chip with divisor 50.
    Selector4 = 0x00000004,
    /// Selector 8, accepted only with divisor 1; coex_core_pre_init selects it off silicon.
    Selector8 = 0x00000008,
}
impl CoexTimerClockSource {
    /// Numeric image for diagnostics and HAL diagnostics.
    pub const fn bits(self) -> u32 {
        self as u32
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// Two-bit coexistence client selector published by complete coex_hw_timer_set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerClientValue(u32);
impl CoexTimerClientValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::configuration::PARAMETER_1_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 2-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::configuration::PARAMETER_1_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// Four-bit coexistence priority value published by complete coex_hw_timer_set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerPtiValue(u32);
impl CoexTimerPtiValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::configuration::PARAMETER_2_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::configuration::PARAMETER_2_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// One external coexistence control bit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexFlag(u32);
impl ExternalCoexFlag {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::control::VALIDATE_HIGH_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 1-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::control::VALIDATE_HIGH_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// The grant delay in microseconds: the upper nibble of the byte hal_set_extern_coex_delay publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexGrantDelay(u32);
impl ExternalCoexGrantDelay {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::control::GRANT_DELAY_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::control::GRANT_DELAY_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// One four-bit external coexistence priority or grant field image.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexNibble(u32);
impl ExternalCoexNibble {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::pti::PRIORITY_0_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::pti::PRIORITY_0_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// The ten-bit synchronization image hal_set_extern_coex_sync publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexSync(u32);
impl ExternalCoexSync {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::control::SYNC_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 10-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::control::SYNC_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// The two-bit external coexistence work mode hal_set_extern_coex_mode publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexWorkMode(u32);
impl ExternalCoexWorkMode {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::coex::pti::WORK_MODE_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 2-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::coex::pti::WORK_MODE_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: complete rev0 ROM phy_force_pwr_index.
/// Six-bit transmit-power table index accepted by complete rev0 ROM phy_force_pwr_index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhyForcedPowerIndex(u32);
impl PhyForcedPowerIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 6-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<
            { oer_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_WIDTH },
        >,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: complete libphy phy_config_hccfr, phy_iccfr_en and phy_force_iccfr.
/// Complete twelve-bit value accepted by reviewed HCCFR and forced-ICCFR transactions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CfrValue(u32);
impl CfrValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::wifi_bb::hccfr_value::VALUE_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<{ oer_esp32s31_pac::wifi_bb::hccfr_value::VALUE_WIDTH }>,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// SOURCE: complete rev0 ROM phy_force_rx_gain (ROM_REV0_PHY_AGC).
/// Complete byte accepted by the recovered forced-RX-gain transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ForcedRxGain(u32);
impl ForcedRxGain {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_esp32s31_pac::wifi_bb::agc_shared_control::FORCED_GAIN_INDEX_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 8-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_mmio::Bits<
            { oer_esp32s31_pac::wifi_bb::agc_shared_control::FORCED_GAIN_INDEX_WIDTH },
        >,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

// The HAL domains fit the generated accessor argument widths without narrowing.
const _: () = {
    assert!(ModemLowPowerClockDivider::MAX == oer_mmio::Bits::<12>::MAX);
    assert!(CoexTimerClockDividerMinusOne::MAX == oer_mmio::Bits::<12>::MAX);
    assert!(CoexTimerClientValue::MAX == oer_mmio::Bits::<2>::MAX);
    assert!(CoexTimerPtiValue::MAX == oer_mmio::Bits::<4>::MAX);
    assert!(ExternalCoexGrantDelay::MAX == oer_mmio::Bits::<4>::MAX);
    assert!(ExternalCoexNibble::MAX == oer_mmio::Bits::<4>::MAX);
    assert!(ExternalCoexSync::MAX == oer_mmio::Bits::<10>::MAX);
    assert!(ExternalCoexWorkMode::MAX == oer_mmio::Bits::<2>::MAX);
    assert!(PhyForcedPowerIndex::MAX == oer_mmio::Bits::<6>::MAX);
    assert!(CfrValue::MAX == oer_mmio::Bits::<12>::MAX);
    assert!(ForcedRxGain::MAX == oer_mmio::Bits::<8>::MAX);
};
