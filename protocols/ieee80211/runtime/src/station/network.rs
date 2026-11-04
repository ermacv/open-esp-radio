//! The MSDUs the actor hands the network stack: lent in place or copied.
//!
//! An MPDU that delivered exactly one MSDU is lent: the port moves its
//! receive buffer to the network ([`DataPort::lend`]) and the frame names
//! its payload in that buffer. The actor copies an MSDU instead when its
//! MPDU delivered several (an A-MSDU) or the port kept the buffer;
//! [`RxMsdusCopied`] counts both.
//!
//! [`DataPort::lend`]: oer_protocols_ieee80211_contracts_port::DataPort::lend

use oer_protocols_ieee80211_contracts_port::LentRxBuffer;
use oer_protocols_network_port::{FrameStorage, LentFrame, ReceivedFrame};

/// A lent receive buffer as network storage: the MPDU, then the rest of its
/// allocation.
pub struct NetworkRx<L>(pub(crate) L);

impl<L: LentRxBuffer> FrameStorage for NetworkRx<L> {
    fn bytes(&self) -> &[u8] {
        self.0.storage()
    }
    fn bytes_mut(&mut self) -> &mut [u8] {
        self.0.storage_mut()
    }
    fn advance(
        &mut self,
        by: usize,
    ) -> Result<(), oer_protocols_network_port::StorageAdvanceRefused> {
        self.0.advance(by).map_err(
            |refused| oer_protocols_network_port::StorageAdvanceRefused {
                by: refused.by,
                length: refused.mpdu_len,
            },
        )
    }
}

/// The actor's copies the network queue's [`Received::Copied`] marks stand
/// for, in their order.
pub const RX_COPIED_DEPTH: usize = 4;

/// One received MSDU for the network stack: small, since every queue and
/// bridge hop moves it by value.
pub enum Received<L> {
    /// In the backend's receive buffer, lent to the network.
    Lent(LentFrame<NetworkRx<L>>),
    /// The actor's own copy, the oldest one in the copies' queue.
    Copied(CopiedMark),
}

/// What stands for one of the actor's copies in the network queue; the
/// device reads the copy in place ([`NetDevice::read_copied`]).
///
/// [`NetDevice::read_copied`]: oer_protocols_network_port::NetDevice::read_copied
#[derive(Debug)]
pub struct CopiedMark(());

impl CopiedMark {
    pub(crate) const fn new() -> Self {
        Self(())
    }
}

impl<L: LentRxBuffer> ReceivedFrame for Received<L> {
    type Storage = NetworkRx<L>;
    type Copied = CopiedMark;
    fn into_lent(self) -> Result<LentFrame<NetworkRx<L>>, CopiedMark> {
        match self {
            Self::Lent(frame) => Ok(frame),
            Self::Copied(mark) => Err(mark),
        }
    }
}

/// The MSDUs of one received frame the actor copied instead of lending,
/// by reason. Each count saturates at `u16::MAX`; at least one is nonzero.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxMsdusCopied {
    /// MSDUs of a frame that delivered more than one.
    pub aggregated: u16,
    /// MSDUs whose buffer the backend kept
    /// ([`LendRefusal`](oer_protocols_ieee80211_contracts_port::LendRefusal)).
    pub kept: u16,
}

/// The network side that holds the station's lent receive buffers: the
/// application's sockets and the IP service. A stop asks it to stop before
/// the port retires, and the port retires only once the network returned
/// every buffer.
pub trait NetworkStop {
    /// Whether a network takes the station's frames at all. A composition
    /// without one makes the actor refuse every MSDU at once, returning
    /// its buffer, so nothing waits for a consumer that does not exist.
    fn takes_frames(&self) -> bool;
    /// Begin stopping: take no new frame and drop every frame it holds.
    fn request_stop(&self);
    /// Ready once stopped, or once the participant was lost before it
    /// reported: a lost participant proves no stop.
    fn poll_stopped(
        &self,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Result<(), NetworkStopLost>>;
}

/// The participant was lost before it reported its stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkStopLost;

/// A composition with no network side: nothing holds lent buffers.
pub struct NoNetwork;

impl NetworkStop for NoNetwork {
    fn takes_frames(&self) -> bool {
        false
    }
    fn request_stop(&self) {}
    fn poll_stopped(
        &self,
        _: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Result<(), NetworkStopLost>> {
        core::task::Poll::Ready(Ok(()))
    }
}

/// The deadline of a stop no request timed: a stop after a failure, or a
/// request that named none. The composition chooses it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopPolicy {
    pub drain: oer_portable_time::Duration,
}
