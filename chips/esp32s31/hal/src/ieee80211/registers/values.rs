//! The value domains of the Wi-Fi MAC register operations: bounded
//! indices, identifiers and reviewed images their callers pass in.
//!
//! Each type admits only the values its operation's fields take; the
//! operations turn them into the generated accessors' field values.

/// Reviewed Wi-Fi MAC interface selector shared by address, BSSID, crypto, TX and ordinary receive-BlockAck transactions.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacInterface {
    /// Infrastructure-station hardware context; traced from STA callers into interface and key-control fields.
    Station = 0x00000000,
    /// SoftAP hardware context; traced from AP callers into interface and key-control fields.
    AccessPoint = 0x00000001,
    /// Reviewed third hardware selector accepted by the complete EDCA and RX BlockAck leaves; its protocol-role semantics are not assigned.
    Context2 = 0x00000002,
    /// Reviewed fourth hardware selector accepted by the complete EDCA and RX BlockAck leaves; its protocol-role semantics are not assigned.
    Context3 = 0x00000003,
}

impl MacInterface {
    /// Numeric image for diagnostics.
    pub const fn bits(self) -> u32 {
        self as u32
    }

    /// The interface's element of the four-entry interface register arrays.
    pub const fn index(self) -> oer_mmio::Index<4> {
        match self {
            Self::Station => oer_mmio::Index::constant::<0>(),
            Self::AccessPoint => oer_mmio::Index::constant::<1>(),
            Self::Context2 => oer_mmio::Index::constant::<2>(),
            Self::Context3 => oer_mmio::Index::constant::<3>(),
        }
    }

    /// The interface a two-bit interface field holds: every value is one.
    pub const fn from_field(value: oer_mmio::Bits<2>) -> Self {
        match value.get() {
            0 => Self::Station,
            1 => Self::AccessPoint,
            2 => Self::Context2,
            _ => Self::Context3,
        }
    }

    /// The two-bit field value of the interface.
    pub const fn field(self) -> oer_mmio::Bits<2> {
        oer_mmio::Bits::truncate(self as u32)
    }
}

/// Reviewed writable images for the MAC interrupt-enable register.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacInterruptMask(u32);

impl MacInterruptMask {
    /// Keep every MAC interrupt source disabled.
    pub const NONE: Self = Self(0x00000000);
    /// Station mask: the cold receive image with HE BSS-color collision disabled until its consumer exists.
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
    pub const APPEND: Self = Self(0x00800000);

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
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MacRxBeaconClearRequest {
    /// The bit-zero receive-beacon clear request traced from hal_clear_rx_beacon_pti.
    Beacon = 0x00000000,
}

impl MacRxBeaconClearRequest {
    /// Numeric image for diagnostics.
    pub const fn bits(self) -> u32 {
        self as u32
    }
}

/// The eighteen-bit slow-clock calibration hal_timer_update_by_rtc publishes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacSlowClockCalibrationValue(u32);

impl MacSlowClockCalibrationValue {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x0003ffff;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x0003ffff {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 18-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<18>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// The low twenty address bits of a Wi-Fi DMA descriptor that the RX walker cursors and the TX queue head hold.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct WifiDescriptorAddressLow(u32);

impl WifiDescriptorAddressLow {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x000fffff;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x000fffff {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 20-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<20>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One reviewed four-bit Wi-Fi packet-traffic-information value. Its scheduling policy remains outside the PAC.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacPti(u32);

impl MacPti {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x0000000f;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x0000000f {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<4>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Non-zero eleven-bit association identifier accepted by the reviewed HE association transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacAssociationId(u32);

impl MacAssociationId {
    pub const MIN: u32 = 0x00000001;
    pub const MAX: u32 = 0x000007ff;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value >= 0x00000001 && value <= 0x000007ff {
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

/// Three-bit minimum MPDU start-spacing value accepted by the reviewed HE association transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacMinimumMpduStartSpacing(u32);

impl MacMinimumMpduStartSpacing {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000007;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000007 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<3>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Six-bit BSS color accepted by the reviewed HE BSSID lifecycle transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacHeBssColor(u32);

impl MacHeBssColor {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x0000003f;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x0000003f {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 6-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<6>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Three-bit Default PE Duration accepted by complete hal_he_set_default_pe.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacHeDefaultPacketExtensionDuration(u32);

impl MacHeDefaultPacketExtensionDuration {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000007;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000007 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<3>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Five-bit nominal packet-padding duration accepted by the reviewed HE peer transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacHePacketPaddingDuration(u32);

impl MacHePacketPaddingDuration {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x0000001f;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x0000001f {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 5-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<5>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Machine-defined five-bit shift index accepted by complete hal_clr_itwt_pti. The protocol meaning of individual request bits remains unknown.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacItwtClearIndex(u32);

impl MacItwtClearIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x0000001f;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x0000001f {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 5-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<5>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One of the four ordinary Wi-Fi transmit queues accepted by complete hal_set_tx_pti.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacTxQueueIndex(u32);

impl MacTxQueueIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000003;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000003 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 2-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<2>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Twelve-bit descriptor metadata count published by complete hal_set_tx_pti.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacTxPtiCount(u32);

impl MacTxPtiCount {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000fff;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000fff {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<12>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One of the twenty-five hardware crypto-table entries accepted by the reviewed MAC key transactions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacKeyEntryIndex(u32);

impl MacKeyEntryIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000018;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000018 {
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
    pub const MAX: u32 = 0x00000007;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000007 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<3>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// One of the eight extra-SoftAP receive BlockAck entries selected through the shared staging window.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacExtraSoftApRxBlockAckEntryIndex(u32);

impl MacExtraSoftApRxBlockAckEntryIndex {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000007;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000007 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 3-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<3>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Four-bit traffic identifier encoded by the reviewed receive BlockAck transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckTid(u32);

impl MacRxBlockAckTid {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x0000000f;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x0000000f {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 4-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<4>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Twelve-bit starting sequence encoded by the reviewed receive BlockAck transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckStartingSequence(u32);

impl MacRxBlockAckStartingSequence {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x00000fff;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x00000fff {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 12-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<12>) -> Self {
        Self(bits.get())
    }

    /// Return the checked numeric value.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// Non-zero seven-bit receive BlockAck window accepted by the reviewed transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MacRxBlockAckWindow(u32);

impl MacRxBlockAckWindow {
    pub const MIN: u32 = 0x00000001;
    pub const MAX: u32 = 0x0000007f;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value >= 0x00000001 && value <= 0x0000007f {
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

/// Low 26-bit station-TBTT target image formed from station TSF bits 35:10 by complete hal_set_sta_tbtt.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct StationTbttTargetBits35To10(u32);

impl StationTbttTargetBits35To10 {
    pub const MIN: u32 = 0x00000000;
    pub const MAX: u32 = 0x03ffffff;

    /// Construct a value only when it lies in the reviewed inclusive range.
    pub const fn new(value: u32) -> Option<Self> {
        if value <= 0x03ffffff {
            Some(Self(value))
        } else {
            None
        }
    }

    /// The value of a 26-bit field, which the range holds whole.
    pub const fn from_bits(bits: oer_mmio::Bits<26>) -> Self {
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
#[repr(u32)]
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum PhyLowRateState {
    /// Disable the selected PHY low-rate control edge.
    Disabled = 0x00000000,
    /// Enable the selected PHY low-rate control edge.
    Enabled = 0x00000001,
}

impl PhyLowRateState {
    /// Numeric image for diagnostics.
    pub const fn bits(self) -> u32 {
        self as u32
    }
}
