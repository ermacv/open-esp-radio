// SPDX-License-Identifier: Apache-2.0
//! Configuration values, commands and identities of the IEEE 802.15.4 MAC.

/// One of the four source-confirmed MAC PAN contexts.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154MultipanIndex(MultipanContext);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum MultipanContext {
    Context0,
    Context1,
    Context2,
    Context3,
}

impl Ieee802154MultipanIndex {
    pub const COUNT: u8 = 4;
    pub const CONTEXT0: Self = Self(MultipanContext::Context0);
    pub const CONTEXT1: Self = Self(MultipanContext::Context1);
    pub const CONTEXT2: Self = Self(MultipanContext::Context2);
    pub const CONTEXT3: Self = Self(MultipanContext::Context3);

    pub const fn new(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::CONTEXT0),
            1 => Some(Self::CONTEXT1),
            2 => Some(Self::CONTEXT2),
            3 => Some(Self::CONTEXT3),
            _ => None,
        }
    }

    pub const fn value(self) -> u8 {
        self.0 as u8
    }

    #[doc(hidden)]
    pub const fn as_usize(self) -> usize {
        self.value() as usize
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
        let [context0, context1, context2, context3] = self.0;
        match index.0 {
            MultipanContext::Context0 => context0,
            MultipanContext::Context1 => context1,
            MultipanContext::Context2 => context2,
            MultipanContext::Context3 => context3,
        }
    }

    pub const fn with(self, index: Ieee802154MultipanIndex) -> Self {
        self.set(index, true)
    }

    pub const fn without(self, index: Ieee802154MultipanIndex) -> Self {
        self.set(index, false)
    }

    const fn set(self, index: Ieee802154MultipanIndex, enabled: bool) -> Self {
        let [context0, context1, context2, context3] = self.0;
        match index.0 {
            MultipanContext::Context0 => Self([enabled, context1, context2, context3]),
            MultipanContext::Context1 => Self([context0, enabled, context2, context3]),
            MultipanContext::Context2 => Self([context0, context1, enabled, context3]),
            MultipanContext::Context3 => Self([context0, context1, context2, enabled]),
        }
    }
}

/// Source-confirmed energy-detection sampling rate.
///
/// The HAL selects the generated PAC field value for each rate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154EdSampleRate {
    OnePerMicrosecond,
    TwoPerMicrosecond,
    FourPerMicrosecond,
    EightPerMicrosecond,
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
/// The HAL selects the generated PAC field value for each mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Ieee802154CcaMode {
    Carrier,
    EnergyDetection,
    CarrierOrEnergyDetection,
    CarrierAndEnergyDetection,
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

/// PTI value the public common LL writes to both PTI fields when coexistence
/// is disabled (`ieee802154_ll_disable_coex`).
/// SOURCE: esp-idf-7b9cc1ac-ieee802154-common-ll, `ieee802154_ll_disable_coex`.
pub const COEX_DISABLED_PTI: u8 = 3;

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    reason = "tests assert behavior and compare with reference arithmetic"
)]
mod tests;
