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
#[allow(
    clippy::large_enum_variant,
    reason = "a queue slot holds a copied MSDU's bytes; a lent one's stay in its buffer"
)]
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

/// The network side that holds the station's lent receive buffers: the
/// application's sockets and the IP service. A stop asks it to stop before
/// the port retires, and the port retires only once the network returned
/// every buffer.
pub trait NetworkStop {
    /// Begin stopping: take no new frame and drop every frame it holds.
    fn request_stop(&self);
    /// Ready once stopped.
    fn poll_stopped(&self, cx: &mut core::task::Context<'_>) -> core::task::Poll<()>;
}

/// No network side: nothing to stop.
impl NetworkStop for () {
    fn request_stop(&self) {}
    fn poll_stopped(&self, _: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        core::task::Poll::Ready(())
    }
}

/// How long a stop waits for the network's buffers when no request named
/// its deadline: a stop after a failure, or a request without a deadline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopPolicy {
    pub drain: oer_time::Duration,
}

impl StopPolicy {
    /// Two seconds: an IP stack drops its frames well within it.
    pub const DEFAULT: Self = Self {
        drain: oer_time::Duration::from_secs(2),
    };
}

/// Stop the network and wait, until `deadline`, for it to stop and return
/// every lent buffer; `Err` holds the buffers it still holds then.
pub(super) async fn drain<'p, 'id: 'p, P: oer_ieee80211_contracts_port::LowerMacPhase<'p, 'id>>(
    phase: &mut P,
    network: &dyn NetworkStop,
    timer: &impl oer_time::Timer,
    deadline: oer_time::Instant,
) -> Result<(), usize> {
    use core::{future::Future, task::Poll};
    network.request_stop();
    let mut alarm = core::pin::pin!(timer.wait_until(deadline));
    core::future::poll_fn(|cx| {
        let stopped = network.poll_stopped(cx).is_ready();
        let returned = phase.poll_lent_returned(cx).is_ready();
        if stopped && returned {
            Poll::Ready(Ok(()))
        } else if alarm.as_mut().poll(cx).is_ready() {
            Poll::Ready(Err(phase.lent()))
        } else {
            Poll::Pending
        }
    })
    .await
}
