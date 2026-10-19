//! Configuration values, commands and identities of the IEEE 802.15.4 MAC.

/// One of the four source-confirmed MAC PAN contexts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanIndex(u8);

impl Ieee802154MultipanIndex {
    pub const COUNT: u8 = 4;
    pub const CONTEXT0: Self = Self(0);
    pub const CONTEXT1: Self = Self(1);
    pub const CONTEXT2: Self = Self(2);
    pub const CONTEXT3: Self = Self(3);

    pub const fn new(value: u8) -> Option<Self> {
        if value < Self::COUNT {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    #[doc(hidden)]
    pub const fn as_usize(self) -> usize {
        self.0 as usize
    }
}

/// Semantic enable state for the four source-confirmed Multi-PAN contexts.
///
/// Hardware bit positions are owned by generated PAC field accessors. This
/// type stores one boolean per context and cannot represent a register image.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanEnableState([bool; 4]);

impl Ieee802154MultipanEnableState {
    pub const NONE: Self = Self([false; 4]);
    pub const ALL: Self = Self([true; 4]);

    /// Construct an explicit semantic state without exposing register bits.
    pub const fn new(context0: bool, context1: bool, context2: bool, context3: bool) -> Self {
        Self([context0, context1, context2, context3])
    }

    /// Construct the state from one boolean per context.
    #[doc(hidden)]
    pub const fn from_enabled(enabled: [bool; 4]) -> Self {
        Self(enabled)
    }

    #[doc(hidden)]
    pub const fn enabled(self) -> [bool; 4] {
        self.0
    }

    pub const fn contains(self, index: Ieee802154MultipanIndex) -> bool {
        self.0[index.as_usize()]
    }

    pub const fn with(self, index: Ieee802154MultipanIndex) -> Self {
        let mut enabled = self.0;
        enabled[index.as_usize()] = true;
        Self(enabled)
    }

    pub const fn without(self, index: Ieee802154MultipanIndex) -> Self {
        let mut enabled = self.0;
        enabled[index.as_usize()] = false;
        Self(enabled)
    }
}

/// Source-confirmed energy-detection sampling rate.
///
/// The discriminants are the two-bit PAC field values.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154EdSampleRate {
    OnePerMicrosecond = 0,
    TwoPerMicrosecond = 1,
    FourPerMicrosecond = 2,
    EightPerMicrosecond = 3,
}

impl Ieee802154EdSampleRate {
    pub const fn field_value(self) -> u8 {
        self as u8
    }

    #[doc(hidden)]
    pub const fn from_field(value: u8) -> Self {
        match value {
            0 => Self::OnePerMicrosecond,
            1 => Self::TwoPerMicrosecond,
            2 => Self::FourPerMicrosecond,
            3 => Self::EightPerMicrosecond,
            _ => unreachable!(),
        }
    }
}

/// Seven-bit transmit-security payload offset.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154SecurityPayloadOffset(u8);

impl Ieee802154SecurityPayloadOffset {
    pub const MAX: u8 = 0x7f;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    pub const fn value(self) -> u8 {
        self.0
    }

    /// The observed seven-bit field value.
    #[doc(hidden)]
    pub const fn from_field(value: u8) -> Self {
        Self(value)
    }
}

/// Readable transmit-security control state without write-only key material.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154TransmitSecurityControl {
    enabled: bool,
    payload_offset: Ieee802154SecurityPayloadOffset,
}

impl Ieee802154TransmitSecurityControl {
    #[doc(hidden)]
    pub const fn new(enabled: bool, payload_offset: Ieee802154SecurityPayloadOffset) -> Self {
        Self {
            enabled,
            payload_offset,
        }
    }

    pub const fn enabled(self) -> bool {
        self.enabled
    }

    pub const fn payload_offset(self) -> Ieee802154SecurityPayloadOffset {
        self.payload_offset
    }
}

/// One source-confirmed clear-channel-assessment mode.
///
/// The discriminants are field values, not shifted register images.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum Ieee802154CcaMode {
    Carrier = 0,
    EnergyDetection = 1,
    CarrierOrEnergyDetection = 2,
    CarrierAndEnergyDetection = 3,
}

impl Ieee802154CcaMode {
    pub const fn field_value(self) -> u8 {
        self as u8
    }

    #[doc(hidden)]
    pub const fn from_field(value: u8) -> Self {
        match value {
            0 => Self::Carrier,
            1 => Self::EnergyDetection,
            2 => Self::CarrierOrEnergyDetection,
            3 => Self::CarrierAndEnergyDetection,
            _ => unreachable!(),
        }
    }
}

/// Sixteen-bit ACK-timeout field value.
///
/// The PAC deliberately does not assign physical units. The HAL owns the
/// source-confirmed conversion between microseconds and this field.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154AckTimeoutUnits(u16);

impl Ieee802154AckTimeoutUnits {
    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u16 {
        self.0
    }
}

/// One finite energy-detection command accepted by the narrow PAC lease.
///
/// `Stop` maps to the source-confirmed common MAC `STOP` opcode, but this type
/// grants no generic STOP operation to callers outside the ED transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154EdCommand {
    /// Start one configured energy-detection/CCA sampling transaction.
    Start,
    /// Stop the active energy-detection transaction.
    Stop,
}

/// Source-confirmed command images accepted by the IEEE 802.15.4 MAC.
///
/// Each variant maps to one complete generated `COMMAND` image. There is no
/// integer constructor, so callers cannot publish test-only or unknown
/// opcodes through the production task capability.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154MacCommand {
    /// Start transmission from the published TX DMA address.
    Transmit,
    /// Start reception into the published RX DMA address.
    Receive,
    /// Perform CCA and transmit when the channel is clear.
    ClearChannelThenTransmit,
    /// Start one configured energy-detection transaction.
    EnergyDetection,
    /// Stop the current state-specific MAC operation.
    Stop,
}

/// Semantic state of the chip's IEEE 802.15.4 CPU interrupt route.
///
/// Register words and field geometry remain private to this PAC domain. Every
/// non-reset state is distinct from [`ResetDetached`](Self::ResetDetached), so
/// callers can fail closed without receiving register images.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154RouteState {
    /// Every CPU route retains the complete reset-detached state.
    ResetDetached,
    /// A route has a CPU-interrupt destination assigned.
    DestinationAssigned,
    /// No destination is assigned, but a pass-through or pass-level field is
    /// configured.
    PassLevelConfigured,
    /// A non-reset bit outside the reviewed destination and pass fields
    /// was observed.
    UnclassifiedNonReset,
}

impl Ieee802154RouteState {
    /// Classify one route observation of the chip.
    #[doc(hidden)]
    pub const fn from_observation(
        both_reset: bool,
        destination_assigned: bool,
        pass_level_configured: bool,
    ) -> Self {
        if both_reset {
            Self::ResetDetached
        } else if destination_assigned {
            Self::DestinationAssigned
        } else if pass_level_configured {
            Self::PassLevelConfigured
        } else {
            Self::UnclassifiedNonReset
        }
    }

    /// Return whether every CPU route retains the complete reset-detached state.
    pub const fn is_reset_detached(self) -> bool {
        matches!(self, Self::ResetDetached)
    }
}

/// Source-confirmed MAC control fields programmed as one semantic policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacControl {
    tx_auto_ack: bool,
    rx_auto_ack: bool,
    enhanced_ack_tx: bool,
    coordinator: bool,
    promiscuous: bool,
    enhanced_pending: bool,
}

impl Ieee802154MacControl {
    pub const fn new(
        tx_auto_ack: bool,
        rx_auto_ack: bool,
        enhanced_ack_tx: bool,
        coordinator: bool,
        promiscuous: bool,
        enhanced_pending: bool,
    ) -> Self {
        Self {
            tx_auto_ack,
            rx_auto_ack,
            enhanced_ack_tx,
            coordinator,
            promiscuous,
            enhanced_pending,
        }
    }

    pub const fn tx_auto_ack(self) -> bool {
        self.tx_auto_ack
    }

    pub const fn rx_auto_ack(self) -> bool {
        self.rx_auto_ack
    }

    pub const fn enhanced_ack_tx(self) -> bool {
        self.enhanced_ack_tx
    }

    pub const fn coordinator(self) -> bool {
        self.coordinator
    }

    pub const fn promiscuous(self) -> bool {
        self.promiscuous
    }

    pub const fn enhanced_pending(self) -> bool {
        self.enhanced_pending
    }
}

/// Address-filter identity for the public API's primary PAN context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PanIdentity {
    pan_id: u16,
    short_address: u16,
    extended_address: [u8; 8],
}

impl Ieee802154PanIdentity {
    pub const fn new(pan_id: u16, short_address: u16, extended_address: [u8; 8]) -> Self {
        Self {
            pan_id,
            short_address,
            extended_address,
        }
    }

    pub const fn pan_id(self) -> u16 {
        self.pan_id
    }

    pub const fn short_address(self) -> u16 {
        self.short_address
    }

    pub const fn extended_address(self) -> [u8; 8] {
        self.extended_address
    }
}
