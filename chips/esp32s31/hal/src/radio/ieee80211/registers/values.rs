//! The value domains of the Wi-Fi MAC register operations: bounded
//! indices, identifiers and reviewed images their callers pass in.
//!
//! Each type admits only the values its operation's fields take; the
//! operations turn them into the generated accessors' field values.

/// Reviewed Wi-Fi MAC interface selector shared by address, BSSID, crypto, TX and ordinary receive-BlockAck transactions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacInterface {
    /// Infrastructure-station hardware context; traced from STA callers into interface and key-control fields.
    Station,
    /// SoftAP hardware context; traced from AP callers into interface and key-control fields.
    AccessPoint,
    /// Reviewed third hardware selector accepted by the complete EDCA and RX BlockAck leaves; its protocol-role semantics are not assigned.
    Context2,
    /// Reviewed fourth hardware selector accepted by the complete EDCA and RX BlockAck leaves; its protocol-role semantics are not assigned.
    Context3,
}

impl MacInterface {
    /// The interface's element of the four-entry interface register arrays.
    pub const fn index(
        self,
    ) -> oer_portable_mmio::Index<{ oer_chips_esp32s31_pac::wifi_mac::config::COUNT }> {
        match self {
            Self::Station => oer_portable_mmio::Index::constant::<0>(),
            Self::AccessPoint => oer_portable_mmio::Index::constant::<1>(),
            Self::Context2 => oer_portable_mmio::Index::constant::<2>(),
            Self::Context3 => oer_portable_mmio::Index::constant::<3>(),
        }
    }

    /// The interface's `CONFIG%s.INTERFACE` value.
    pub const fn config(self) -> oer_chips_esp32s31_pac::wifi_mac::config::Interface {
        use oer_chips_esp32s31_pac::wifi_mac::config::Interface;
        match self {
            Self::Station => Interface::Station,
            Self::AccessPoint => Interface::AccessPoint,
            Self::Context2 => Interface::Context2,
            Self::Context3 => Interface::Context3,
        }
    }

    /// The two-bit field value of the interface.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::wifi_mac::config::INTERFACE_WIDTH }>
    {
        match self {
            Self::Station => oer_portable_mmio::Bits::constant::<0>(),
            Self::AccessPoint => oer_portable_mmio::Bits::constant::<1>(),
            Self::Context2 => oer_portable_mmio::Bits::constant::<2>(),
            Self::Context3 => oer_portable_mmio::Bits::constant::<3>(),
        }
    }
}

/// The interface each of the MAC's interface fields reads as, and the
/// field value of each interface: every value of the two bits is one.
macro_rules! interface_of_field {
    ($($register:ident),+) => {$(
        impl From<oer_chips_esp32s31_pac::wifi_mac::$register::Interface> for MacInterface {
            fn from(value: oer_chips_esp32s31_pac::wifi_mac::$register::Interface) -> Self {
                use oer_chips_esp32s31_pac::wifi_mac::$register::Interface;
                match value {
                    Interface::Station => Self::Station,
                    Interface::AccessPoint => Self::AccessPoint,
                    Interface::Context2 => Self::Context2,
                    Interface::Context3 => Self::Context3,
                }
            }
        }

        impl From<MacInterface> for oer_chips_esp32s31_pac::wifi_mac::$register::Interface {
            fn from(value: MacInterface) -> Self {
                match value {
                    MacInterface::Station => Self::Station,
                    MacInterface::AccessPoint => Self::AccessPoint,
                    MacInterface::Context2 => Self::Context2,
                    MacInterface::Context3 => Self::Context3,
                }
            }
        }
    )+};
}

interface_of_field!(
    config,
    rx_block_ack_entry_peer_tail_and_policy,
    extra_softap_rx_block_ack_peer_tail_and_policy,
    entry_peer_control
);

/// Reviewed writable images for the MAC interrupt-enable register.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptMask(u32);

impl MacInterruptMask {
    /// Keep every MAC interrupt source disabled.
    pub const NONE: Self = Self(0x00000000);
    /// Station primary mask with TX queue collision disabled; INT1 is separate.
    pub const STATION: Self = Self(0x19a878e0);

    /// Combine reviewed writable values.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Test whether all bits in `other` are present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Read-only numeric image for diagnostics.
    /// There is intentionally no inverse integer constructor.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl core::ops::BitOr for MacInterruptMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// Reviewed CSI append selector in a captured RX/CSI control image.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WifiRxCsiControlMask(u32);

impl WifiRxCsiControlMask {
    /// CSI appending enabled by hal_mac_set_csi.
    pub const APPEND: Self = Self(
        oer_chips_esp32s31_pac::wifi_mac::rx_csi_control::CsiAppendEnable::Enabled.register_bits(),
    );

    /// Combine reviewed writable values.
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Test whether all bits in `other` are present.
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Read-only numeric image for diagnostics.
    /// There is intentionally no inverse integer constructor.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

impl core::ops::BitOr for WifiRxCsiControlMask {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

/// Reviewed receive-beacon clear requests. The type cannot select an unknown request bit.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacRxBeaconClearRequest {
    /// The bit-zero receive-beacon clear request traced from hal_clear_rx_beacon_pti.
    Beacon,
}

impl MacRxBeaconClearRequest {
    /// Numeric image for diagnostics.
    pub const fn bits(self) -> u32 {
        oer_chips_esp32s31_pac::modem_pwr::clear_request::Bits0_31Opaque::RxBeacon.bits()
    }
}

/// The eighteen-bit slow-clock calibration hal_timer_update_by_rtc publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacSlowClockCalibrationValue(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::modem_pwr::slow_clock_calibration::VALUE_WIDTH },
    >,
);

impl MacSlowClockCalibrationValue {
    pub const MIN: u32 = 0x00000000;

    /// Zero, the calibration the HAL publishes before a measurement.
    pub const ZERO: Self = Self(oer_portable_mmio::Bits::constant::<0>());

    /// The low eighteen bits of `value`: the vendor's own truncation of the
    /// OS interface's calibration (`hal_timer_update_by_rtc`).
    pub const fn from_low_bits(value: u32) -> Self {
        Self(oer_portable_mmio::Bits::truncate(value))
    }
    pub const MAX: u32 = oer_chips_esp32s31_pac::modem_pwr::slow_clock_calibration::VALUE_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 18-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::modem_pwr::slow_clock_calibration::VALUE_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::modem_pwr::slow_clock_calibration::VALUE_WIDTH },
    > {
        self.0
    }
}

/// The low twenty address bits of a Wi-Fi DMA descriptor that the RX walker cursors and the TX queue head hold.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WifiDescriptorAddressLow(u32);

impl WifiDescriptorAddressLow {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::control_n::DESCRIPTOR_ADDRESS_LOW_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 20-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::control_n::DESCRIPTOR_ADDRESS_LOW_WIDTH },
        >,
    ) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One reviewed four-bit Wi-Fi packet-traffic-information value. Its scheduling policy remains outside the PAC.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacPti(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::default_control::WIFI_DEFAULT_PTI_WIDTH },
    >,
);

impl MacPti {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::default_control::WIFI_DEFAULT_PTI_MAX;

    /// Preserve a shared coexistence priority in the MAC field. The entire
    /// source domain must fit the generated field geometry at compile time.
    pub const fn from_coex(pti: oer_chips_family_radio_coex::CoexPti) -> Self {
        const {
            if oer_chips_family_radio_coex::CoexPti::MAX as u32 > Self::MAX {
                panic!("the coexistence priority domain must fit the MAC PTI field");
            }
        };
        // The domain fits the field (checked above, at build): nothing is cut.
        Self(oer_portable_mmio::Bits::truncate(pti.value() as u32))
    }

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(bits) => Some(Self(bits)),
            Err(_) => None,
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::default_control::WIFI_DEFAULT_PTI_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The priority as the four-bit PTI fields hold it.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::default_control::WIFI_DEFAULT_PTI_WIDTH },
    > {
        self.0
    }
}

/// Non-zero eleven-bit association identifier accepted by the reviewed HE association transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacAssociationId(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::wifi_mac::bssid_high::ASSOCIATION_ID_WIDTH }>,
);

impl MacAssociationId {
    pub const MIN: u32 = 0x00000001;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::bssid_high::ASSOCIATION_ID_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value >= Self::MIN && value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::bssid_high::ASSOCIATION_ID_WIDTH },
    > {
        self.0
    }
}

/// Three-bit minimum MPDU start-spacing value accepted by the reviewed HE association transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacMinimumMpduStartSpacing(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::bssid_high::MINIMUM_MPDU_START_SPACING_WIDTH },
    >,
);

impl MacMinimumMpduStartSpacing {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::wifi_mac::bssid_high::MINIMUM_MPDU_START_SPACING_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::bssid_high::MINIMUM_MPDU_START_SPACING_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::bssid_high::MINIMUM_MPDU_START_SPACING_WIDTH },
    > {
        self.0
    }
}

/// Six-bit BSS color accepted by the reviewed HE BSSID lifecycle transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacHeBssColor(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::multi_bssid_control::BSS_COLOR_WIDTH },
    >,
);

impl MacHeBssColor {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::multi_bssid_control::BSS_COLOR_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 6-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::multi_bssid_control::BSS_COLOR_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::multi_bssid_control::BSS_COLOR_WIDTH },
    > {
        self.0
    }
}

/// Three-bit Default PE Duration accepted by complete hal_he_set_default_pe.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacHeDefaultPacketExtensionDuration(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::he_default_control::DEFAULT_PE_DURATION_WIDTH },
    >,
);

impl MacHeDefaultPacketExtensionDuration {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::wifi_mac::he_default_control::DEFAULT_PE_DURATION_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::he_default_control::DEFAULT_PE_DURATION_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::he_default_control::DEFAULT_PE_DURATION_WIDTH },
    > {
        self.0
    }
}

/// Five-bit nominal packet-padding duration accepted by the reviewed HE peer transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacHePacketPaddingDuration(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::he_packet_padding::BPSK_DURATION_WIDTH },
    >,
);

impl MacHePacketPaddingDuration {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::he_packet_padding::BPSK_DURATION_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 5-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::he_packet_padding::BPSK_DURATION_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::he_packet_padding::BPSK_DURATION_WIDTH },
    > {
        self.0
    }
}

/// Machine-defined five-bit shift index accepted by complete hal_clr_itwt_pti. The protocol meaning of individual request bits remains unknown.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacItwtClearIndex(u32);

impl MacItwtClearIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::modem_pwr::clear_request::BITS_0_31_OPAQUE_WIDTH - 1;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 5-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_portable_mmio::Bits<5>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One of the four ordinary Wi-Fi transmit queues accepted by complete hal_set_tx_pti.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum MacTxQueueIndex {
    Zero,
    One,
    Two,
    Three,
}

impl MacTxQueueIndex {
    pub const MIN: u32 = 0;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::control_n::COUNT as u32 - 1;
    pub const ALL: [Self; 4] = [Self::Zero, Self::One, Self::Two, Self::Three];

    pub const fn new(value: u32) -> Option<Self> {
        match value {
            0 => Some(Self::Zero),
            1 => Some(Self::One),
            2 => Some(Self::Two),
            3 => Some(Self::Three),
            _ => None,
        }
    }

    /// Fixed queue identities are checked during monomorphization.
    ///
    /// ```compile_fail
    /// use oer_chips_esp32s31_hal::types::MacTxQueueIndex;
    /// const INVALID: MacTxQueueIndex = MacTxQueueIndex::constant::<4>();
    /// ```
    pub const fn constant<const QUEUE: u8>() -> Self {
        const {
            match Self::new(QUEUE as u32) {
                Some(queue) => queue,
                None => panic!("ordinary TX queue is out of range"),
            }
        }
    }

    pub const fn from_bits(bits: oer_portable_mmio::Bits<2>) -> Self {
        match bits.get() {
            0 => Self::Zero,
            1 => Self::One,
            2 => Self::Two,
            _ => Self::Three,
        }
    }

    pub const fn get(self) -> u32 {
        self as u32
    }
}
impl From<MacTxQueueIndex> for usize {
    fn from(queue: MacTxQueueIndex) -> Self {
        queue as usize
    }
}
const _: () = {
    if oer_chips_esp32s31_pac::wifi_mac::control_n::COUNT != MacTxQueueIndex::ALL.len() {
        panic!("ordinary queue identities must cover the generated geometry");
    }
};

/// Twelve-bit descriptor metadata count published by complete hal_set_tx_pti.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacTxPtiCount(
    oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::wifi_mac::pti::COUNT_WIDTH }>,
);

impl MacTxPtiCount {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::pti::COUNT_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        match oer_portable_mmio::Bits::new(value) {
            Ok(bits) => Some(Self(bits)),
            Err(_) => None,
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::wifi_mac::pti::COUNT_WIDTH }>,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The count as the PTI word's `COUNT` field holds it.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::wifi_mac::pti::COUNT_WIDTH }> {
        self.0
    }
}

/// One of the twenty-five hardware crypto-table entries accepted by the reviewed MAC key transactions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacKeyEntryIndex(u32);

impl MacKeyEntryIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::entry_peer_control::COUNT as u32 - 1;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One of the eight reverse-addressed ordinary receive BlockAck hardware banks.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckEntryIndex(u32);

impl MacRxBlockAckEntryIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_control::COUNT as u32 - 1;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_portable_mmio::Bits<3>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One of the eight extra-SoftAP receive BlockAck entries selected through the shared staging window.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacExtraSoftApRxBlockAckEntryIndex(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::extra_softap_rx_block_ack_control::INDEX_WIDTH },
    >,
);

impl MacExtraSoftApRxBlockAckEntryIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_control::COUNT as u32 - 1;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::extra_softap_rx_block_ack_control::INDEX_WIDTH },
    > {
        self.0
    }
}

/// Four-bit traffic identifier encoded by the reviewed receive BlockAck transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckTid(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_control::TID_WIDTH },
    >,
);

impl MacRxBlockAckTid {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_control::TID_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_control::TID_WIDTH },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_control::TID_WIDTH },
    > {
        self.0
    }
}

/// Twelve-bit starting sequence encoded by the reviewed receive BlockAck transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckStartingSequence(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_current_sequence::SEQUENCE_WIDTH },
    >,
);

impl MacRxBlockAckStartingSequence {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_current_sequence::SEQUENCE_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(
        bits: oer_portable_mmio::Bits<
            {
                oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_current_sequence::SEQUENCE_WIDTH
            },
        >,
    ) -> Self {
        Self(bits)
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_current_sequence::SEQUENCE_WIDTH },
    > {
        self.0
    }
}

/// Non-zero seven-bit receive BlockAck window accepted by the reviewed transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckWindow(
    oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::WINDOW_WIDTH },
    >,
);

impl MacRxBlockAckWindow {
    pub const MIN: u32 = 0x00000001;
    pub const MAX: u32 =
        oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::WINDOW_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value >= Self::MIN && value <= Self::MAX {
            match oer_portable_mmio::Bits::new(value) {
                Ok(field) => Some(Self(field)),
                Err(_) => None,
            }
        } else {
            None
        }
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0.get()
    }

    /// The value as its register field.
    pub const fn field(
        self,
    ) -> oer_portable_mmio::Bits<
        { oer_chips_esp32s31_pac::wifi_mac::rx_block_ack_entry_peer_tail_and_policy::WINDOW_WIDTH },
    > {
        self.0
    }
}

/// Low 26-bit station-TBTT target image formed from station TSF bits 35:10 by complete hal_set_sta_tbtt.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StationTbttTargetBits35To10(u32);

impl StationTbttTargetBits35To10 {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = oer_chips_esp32s31_pac::modem_pwr::target::TSF_BITS_35_10_MAX;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 26-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_portable_mmio::Bits<26>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Register-specific event image written to the MAC interrupt clear register.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptClearImage(u32);

impl MacInterruptClearImage {
    /// Wrap one register-specific opaque value.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the opaque numeric image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Low word of one station TSF value; sequencing remains owned by the station-TSF capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StationTsfLowWord(u32);

impl StationTsfLowWord {
    /// Wrap one register-specific opaque value.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the opaque numeric image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// High word of one station TSF value; sequencing remains owned by the station-TSF capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StationTsfHighWord(u32);

impl StationTsfHighWord {
    /// Wrap one register-specific opaque value.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the opaque numeric image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Zero-or-one control image accepted only by the reviewed hal_set_itwt_pti low-byte replacement helper.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacItwtControlMaskedInput(u32);

impl MacItwtControlMaskedInput {
    /// Wrap one register-specific opaque value.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the opaque numeric image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Boolean state shared by the three separately ordered low-rate control edges.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PhyLowRateState {
    /// Disable the selected PHY low-rate control edge.
    Disabled,
    /// Enable the selected PHY low-rate control edge.
    Enabled,
}

impl PhyLowRateState {
    /// Numeric image for diagnostics.
    pub const fn bits(self) -> u32 {
        match self { Self::Disabled => oer_chips_esp32s31_pac::wifi_bb_brx::low_rate_primary_control::LowRateEnableFirst::Disabled.bits(), Self::Enabled => oer_chips_esp32s31_pac::wifi_bb_brx::low_rate_primary_control::LowRateEnableFirst::Enabled.bits() }
    }
}
