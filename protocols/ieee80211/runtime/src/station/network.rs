//! The MSDUs the service hands the network stack: lent in place or copied.
//!
//! An MPDU that delivered exactly one MSDU is lent: the backend moves its
//! receive buffer to the network ([`LowerMacPhase::lend`]) and the frame
//! names its payload in that buffer. The service copies an MSDU instead
//! when its MPDU delivered several (an A-MSDU) or the backend kept the
//! buffer; [`RxMsdusCopied`] counts both.
//!
//! [`LowerMacPhase::lend`]: oer_ieee80211_contracts_port::LowerMacPhase::lend

use oer_ieee80211_contracts_port::LentRxBuffer;
use oer_ieee80211_core_roles_sta::data::EthernetFrame;
use oer_network_port::{
    EthernetFrameView, FrameLengthError, FrameStorage, LentFrame, ReceivedFrame,
};

/// A lent receive buffer as network storage: the MPDU, then the rest of its
/// allocation.
pub struct NetworkRx<L>(pub(super) L);

impl<L: LentRxBuffer> FrameStorage for NetworkRx<L> {
    fn bytes(&self) -> &[u8] {
        self.0.storage()
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        self.0.storage_mut()
    }
}

/// One received MSDU for the network stack.
pub enum Received<L> {
    /// In the backend's receive buffer, lent to the network.
    Lent(LentFrame<NetworkRx<L>>),
    /// The service's own copy.
    Copied(EthernetFrame),
}

impl<L: LentRxBuffer> oer_network_port::EthernetFrame for Received<L> {
    fn view(&self) -> Result<EthernetFrameView<'_>, FrameLengthError> {
        match self {
            Self::Lent(frame) => Ok(frame.view()),
            Self::Copied(frame) => frame.view(),
        }
    }
}

impl<L: LentRxBuffer> ReceivedFrame for Received<L> {
    type Storage = NetworkRx<L>;
    fn into_lent(self) -> Result<LentFrame<NetworkRx<L>>, Self> {
        match self {
            Self::Lent(frame) => Ok(frame),
            copied @ Self::Copied(_) => Err(copied),
        }
    }
}

/// The MSDUs of one received frame the service copied instead of lending,
/// by reason. Each count saturates at `u16::MAX`; at least one is nonzero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxMsdusCopied {
    /// MSDUs of a frame that delivered more than one.
    pub aggregated: u16,
    /// MSDUs whose buffer the backend kept
    /// ([`LendRefusal`](oer_ieee80211_contracts_port::LendRefusal)).
    pub kept: u16,
}
