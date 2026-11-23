//! The value domains of the Wi-Fi MAC register operations: bounded
//! indices, identifiers and reviewed images their callers pass in.
//!
//! Each type admits only the values its operation's fields take; the
//! operations turn them into the generated accessors' field values.

/// The MAC interface selector: the model's `INTERFACE` dictionary, which
/// every interface field of the MAC takes.
pub use oer_chips_esp32s31_pac::wifi_mac::dictionaries::Interface as MacInterface;

/// The interface's selector in a TX queue's `PLCP1.BSSID_SELECT`, which the
/// model leaves a number: the HAL writes the interface's own value there.
///
/// SOURCE: `libpp[hal_mac_tx.o]::mac_tx_set_plcp1+0x38..0x46` (pinned
/// `esp32-wifi-lib@f856b5db`, `libpp.a` SHA-256 `4b9cd74b…`) stores bits
/// 24:17 from one byte of the frame's control block (`+0x10`): the key
/// entry index in its low six bits, this selector in its top two;
/// `libpp[hal_debug.o]::dbg_read_key_entry` prints bits 24:23 as
/// `bssid_sel`. The vendor's station frames carry zero there
/// (`HIL_VENDOR_HE20_MCS0_DCM_RAW_2026_07_29`, PLCP1 `0x0401a000`). That
/// the selector is the interface's value for every interface is the HAL's
/// reading, not a vendor record: no capture of an access point's frame
/// establishes selector one.
pub(crate) const fn interface_bssid_select(
    interface: MacInterface,
) -> oer_portable_mmio::Bits<{ oer_chips_esp32s31_pac::wifi_mac::plcp1::BSSID_SELECT_WIDTH }> {
    match interface {
        MacInterface::Station => oer_portable_mmio::Bits::constant::<0>(),
        MacInterface::AccessPoint => oer_portable_mmio::Bits::constant::<1>(),
        MacInterface::Context2 => oer_portable_mmio::Bits::constant::<2>(),
        MacInterface::Context3 => oer_portable_mmio::Bits::constant::<3>(),
    }
}

/// The interface's element of the four-entry interface register arrays.
pub const fn interface_index(
    interface: MacInterface,
) -> oer_portable_bounded::Index<{ oer_chips_esp32s31_pac::wifi_mac::config::COUNT }> {
    match interface {
        MacInterface::Station => oer_portable_bounded::Index::constant::<0>(),
        MacInterface::AccessPoint => oer_portable_bounded::Index::constant::<1>(),
        MacInterface::Context2 => oer_portable_bounded::Index::constant::<2>(),
        MacInterface::Context3 => oer_portable_bounded::Index::constant::<3>(),
    }
}

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
    pub fn from_coex(pti: oer_chips_family_radio_coex::CoexPti) -> Self {
        // The field holds every number below sixteen, checked by the `From`
        // where it is monomorphized.
        Self(oer_portable_mmio::Bits::from(pti.index()))
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

/// The first argument of the vendor's `hal_set_itwt_pti`, which writes the
/// low byte of `ITWT_CONTROL` as `seqz(argument)`: one for a zero argument,
/// zero for any other. The byte's meaning is not established; these are the
/// two images the vendor writes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MacItwtArgument {
    Zero,
    Nonzero,
}

impl MacItwtArgument {
    /// The control byte the vendor writes for the argument.
    pub(crate) const fn control_byte(self) -> u8 {
        match self {
            Self::Zero => 1,
            Self::Nonzero => 0,
        }
    }
}
