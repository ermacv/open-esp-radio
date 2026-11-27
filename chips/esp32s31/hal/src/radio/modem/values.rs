//! Value domains of the recovered modem and PHY operations.
#![forbid(unsafe_code)]

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT, S31_MODEM_LPCON_STRUCT.
/// Twelve-bit divider accepted by the canonical MODEM_LPCON low-power timer selector.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ModemLowPowerClockDivider(
    oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_WIDTH },
    >,
);
impl ModemLowPowerClockDivider {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_chips_esp32s31_pac::Bits::new(value) {
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
    ) -> oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::modem_lpcon::lp_timer_conf::CLK_LP_TIMER_DIV_NUM_WIDTH },
    > {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// Two-bit coexistence client selector published by complete coex_hw_timer_set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerClientValue(
    oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::coex::configuration::PARAMETER_1_WIDTH },
    >,
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
        match oer_chips_esp32s31_pac::Bits::new(value) {
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
    ) -> oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::coex::configuration::PARAMETER_1_WIDTH },
    > {
        self.0
    }
}

// SOURCE: BLOB_LIBCOEX_HW_TIMER.
/// Four-bit coexistence priority value published by complete coex_hw_timer_set.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CoexTimerPtiValue(
    oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::coex::configuration::PARAMETER_2_WIDTH },
    >,
);
impl CoexTimerPtiValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::coex::configuration::PARAMETER_2_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_chips_esp32s31_pac::Bits::new(value) {
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
    ) -> oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::coex::configuration::PARAMETER_2_WIDTH },
    > {
        self.0
    }
}

// SOURCE: complete rev0 ROM phy_force_pwr_index.
/// Six-bit transmit-power table index accepted by complete rev0 ROM phy_force_pwr_index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PhyForcedPowerIndex(
    oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_WIDTH },
    >,
);
impl PhyForcedPowerIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_chips_esp32s31_pac::Bits::new(value) {
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
    ) -> oer_chips_esp32s31_pac::Bits<
        { oer_chips_esp32s31_pac::fe::table_memory_index_source::FORCED_POWER_INDEX_WIDTH },
    > {
        self.0
    }
}

// The HAL domains fit the generated accessor argument widths without narrowing.
const _: () = {
    if !(ModemLowPowerClockDivider::MAX == oer_chips_esp32s31_pac::Bits::<12>::MAX) {
        panic!(
            "build-time check: ModemLowPowerClockDivider::MAX == oer_chips_esp32s31_pac::Bits::<12>::MAX"
        )
    };
    if !(CoexTimerClientValue::MAX == oer_chips_esp32s31_pac::Bits::<2>::MAX) {
        panic!(
            "build-time check: CoexTimerClientValue::MAX == oer_chips_esp32s31_pac::Bits::<2>::MAX"
        )
    };
    if !(CoexTimerPtiValue::MAX == oer_chips_esp32s31_pac::Bits::<4>::MAX) {
        panic!("build-time check: CoexTimerPtiValue::MAX == oer_chips_esp32s31_pac::Bits::<4>::MAX")
    };
    if !(PhyForcedPowerIndex::MAX == oer_chips_esp32s31_pac::Bits::<6>::MAX) {
        panic!(
            "build-time check: PhyForcedPowerIndex::MAX == oer_chips_esp32s31_pac::Bits::<6>::MAX"
        )
    };
};
