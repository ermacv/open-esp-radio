//! Value domains of the recovered modem and PHY operations.
#![forbid(unsafe_code)]

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Twelve-bit divider accepted by the canonical MODEM_LPCON low-power timer selector.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModemLowPowerClockDivider(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_WIDTH },
    >,
);
impl ModemLowPowerClockDivider {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_WIDTH },
    > {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// The coexistence timer clock's divisor minus one, whole, that complete
/// coex_hw_timer_freq_set publishes: the twelve-bit field's vendor cut
/// keeps its low bits.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerClockDividerMinusOne(u32);
impl CoexTimerClockDividerMinusOne {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::modem_lpcon::coex_lp_clk_conf::CLK_COEX_LP_DIV_NUM_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// `divisor` minus one, wrapping, as the vendor computes it.
    pub const fn from_divisor(divisor: u32) -> Self {
        Self(divisor.wrapping_sub(1))
    }

    /// The vendor's number.
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
pub struct CoexTimerClientValue(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::configuration::PARAMETER_1_WIDTH }>,
);
impl From<oer_chips_esp32s31_pac::coex::configuration::Parameter1> for CoexTimerClientValue {
    /// A named client of the timer bank's configuration.
    fn from(client: oer_chips_esp32s31_pac::coex::configuration::Parameter1) -> Self {
        Self(client.into())
    }
}
impl CoexTimerClientValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::coex::configuration::PARAMETER_1_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::configuration::PARAMETER_1_WIDTH }>
    {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// Four-bit coexistence priority value published by complete coex_hw_timer_set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerPtiValue(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::configuration::PARAMETER_2_WIDTH }>,
);
impl CoexTimerPtiValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::coex::configuration::PARAMETER_2_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::configuration::PARAMETER_2_WIDTH }>
    {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// The grant delay in microseconds: the upper nibble of the byte hal_set_extern_coex_delay publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexGrantDelay(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::control::GRANT_DELAY_WIDTH }>,
);
impl ExternalCoexGrantDelay {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::coex::control::GRANT_DELAY_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::control::GRANT_DELAY_WIDTH }> {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// One four-bit external coexistence priority or grant field image.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexNibble(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::pti::PRIORITY_0_WIDTH }>,
);
impl ExternalCoexNibble {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::coex::pti::PRIORITY_0_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::pti::PRIORITY_0_WIDTH }> {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_EXTERNAL.
/// The two-bit external coexistence work mode hal_set_extern_coex_mode publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExternalCoexWorkMode(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::pti::WORK_MODE_WIDTH }>,
);
impl ExternalCoexWorkMode {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::coex::pti::WORK_MODE_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::coex::pti::WORK_MODE_WIDTH }> {
        self.0
    }
}

// SOURCE: complete rev0 ROM phy_force_pwr_index.
/// Six-bit transmit-power table index accepted by complete rev0 ROM phy_force_pwr_index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhyForcedPowerIndex(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_WIDTH },
    >,
);
impl PhyForcedPowerIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(field) => Some(Self(field)),
            Err(_) => None,
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value's field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_WIDTH },
    > {
        self.0
    }
}

// The HAL domains fit the generated accessor argument widths without narrowing.
const _: () = {
    if !(ModemLowPowerClockDivider::MAX == oer_portable_mmio::Bits::<12>::MAX) {
        panic!(
            "build-time check: ModemLowPowerClockDivider::MAX == oer_portable_mmio::Bits::<12>::MAX"
        )
    };
    if !(CoexTimerClockDividerMinusOne::MAX == oer_portable_mmio::Bits::<12>::MAX) {
        panic!(
            "build-time check: CoexTimerClockDividerMinusOne::MAX == oer_portable_mmio::Bits::<12>::MAX"
        )
    };
    if !(CoexTimerClientValue::MAX == oer_portable_mmio::Bits::<2>::MAX) {
        panic!("build-time check: CoexTimerClientValue::MAX == oer_portable_mmio::Bits::<2>::MAX")
    };
    if !(CoexTimerPtiValue::MAX == oer_portable_mmio::Bits::<4>::MAX) {
        panic!("build-time check: CoexTimerPtiValue::MAX == oer_portable_mmio::Bits::<4>::MAX")
    };
    if !(ExternalCoexGrantDelay::MAX == oer_portable_mmio::Bits::<4>::MAX) {
        panic!("build-time check: ExternalCoexGrantDelay::MAX == oer_portable_mmio::Bits::<4>::MAX")
    };
    if !(ExternalCoexNibble::MAX == oer_portable_mmio::Bits::<4>::MAX) {
        panic!("build-time check: ExternalCoexNibble::MAX == oer_portable_mmio::Bits::<4>::MAX")
    };
    if !(ExternalCoexWorkMode::MAX == oer_portable_mmio::Bits::<2>::MAX) {
        panic!("build-time check: ExternalCoexWorkMode::MAX == oer_portable_mmio::Bits::<2>::MAX")
    };
    if !(PhyForcedPowerIndex::MAX == oer_portable_mmio::Bits::<6>::MAX) {
        panic!("build-time check: PhyForcedPowerIndex::MAX == oer_portable_mmio::Bits::<6>::MAX")
    };
};
