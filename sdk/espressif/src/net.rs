//! Waiting on the IP service's status: an IPv4 configuration, or the loss of
//! the link it came over.
//!
//! The service (`oer_embassy_net::ip`) publishes each status change in its
//! own task; the library's task wakes every registered waiter
//! ([`Watchers::wake_all`]). A wait reads the status afresh on each poll, so
//! a change between polls is never lost, and ends:
//!
//! - [`wait_ipv4`] with the configuration once the link is up and holds a
//!   lease or a static address; across a link loss it keeps waiting for the
//!   next lease;
//! - [`wait_link_down`] once the link is down or came up again in another
//!   epoch than the one it started in;
//! - both with [`NetError::Stopped`] or [`NetError::Failed`] once the
//!   service ended.
//!
//! A bound on a wait is the caller's (`embassy_time::with_timeout`);
//! dropping a wait leaves only its registered waker, which a later change
//! wakes harmlessly.

use core::task::{Context, Poll, Waker};

use oer_embassy_net::ip::{Ipv4Configuration, Ipv4Status, Lifecycle, Status};
use oer_memory::{ContextCell, Entered, ExecutionContext};
use oer_network_port::LinkState;

pub use oer_embassy_net::ip::Error as ServiceError;

/// Why a wait ended without its condition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetError {
    /// The service was stopped.
    Stopped,
    /// The service failed with this error.
    Failed(ServiceError),
    /// Every waiter slot is taken.
    Waiters,
    /// The wait ran outside the service's execution context.
    Foreign,
}

/// The registered waiters of one service, `K` of them, in context `X`.
pub struct Watchers<X: ExecutionContext, const K: usize> {
    wakers: ContextCell<[Option<Waker>; K], Entered<X>>,
}

impl<X: ExecutionContext, const K: usize> Watchers<X, K> {
    pub const fn new() -> Self {
        const {
            if K == 0 {
                panic!("a service has at least one waiter slot");
            }
        };
        Self {
            wakers: ContextCell::new([const { None }; K]),
        }
    }

    /// Register `waker` for the next change: replace its own or an empty
    /// slot.
    fn register(&self, waker: &Waker) -> Result<(), NetError> {
        let entered = Entered::<X>::enter().ok_or(NetError::Foreign)?;
        self.wakers
            .with(&entered, |wakers| {
                if wakers
                    .iter()
                    .flatten()
                    .any(|stored| stored.will_wake(waker))
                {
                    return Ok(());
                }
                let slot = wakers
                    .iter_mut()
                    .find(|slot| slot.is_none())
                    .ok_or(NetError::Waiters)?;
                *slot = Some(waker.clone());
                Ok(())
            })
            .ok_or(NetError::Foreign)?
    }

    /// Wake every registered waiter, after the borrow ends.
    pub fn wake_all(&self) {
        let Some(entered) = Entered::<X>::enter() else {
            return;
        };
        let Some(taken) = self.wakers.with(&entered, |wakers| {
            core::mem::replace(wakers, [const { None }; K])
        }) else {
            return;
        };
        for waker in taken.into_iter().flatten() {
            waker.wake();
        }
    }
}

impl<X: ExecutionContext, const K: usize> Default for Watchers<X, K> {
    fn default() -> Self {
        Self::new()
    }
}

/// The service's end, if it ended.
fn ended(status: &Status) -> Option<NetError> {
    match status.lifecycle {
        Lifecycle::Running => None,
        Lifecycle::Stopped => Some(NetError::Stopped),
        Lifecycle::Failed(error) => Some(NetError::Failed(error)),
    }
}

/// The IPv4 configuration `status` holds over an up link.
fn configured(status: &Status) -> Option<Ipv4Configuration> {
    if status.link.state != LinkState::Up {
        return None;
    }
    match status.ipv4 {
        Ipv4Status::Lease(configuration) | Ipv4Status::Static(configuration) => Some(configuration),
        Ipv4Status::Unconfigured => None,
    }
}

fn poll_until<X: ExecutionContext, const K: usize, T>(
    watchers: &Watchers<X, K>,
    status: &impl Fn() -> Status,
    cx: &Context<'_>,
    done: impl Fn(&Status) -> Option<T>,
) -> Poll<Result<T, NetError>> {
    let current = status();
    if let Some(value) = done(&current) {
        return Poll::Ready(Ok(value));
    }
    if let Some(error) = ended(&current) {
        return Poll::Ready(Err(error));
    }
    if let Err(error) = watchers.register(cx.waker()) {
        return Poll::Ready(Err(error));
    }
    // A change between the read and the registration is seen now.
    let current = status();
    if let Some(value) = done(&current) {
        return Poll::Ready(Ok(value));
    }
    match ended(&current) {
        Some(error) => Poll::Ready(Err(error)),
        None => Poll::Pending,
    }
}

/// Wait for an IPv4 configuration over an up link.
///
/// # Errors
/// The service stopped or failed, every waiter slot is taken, or the wait
/// runs outside the service's context.
pub async fn wait_ipv4<X: ExecutionContext, const K: usize>(
    watchers: &Watchers<X, K>,
    status: impl Fn() -> Status,
) -> Result<Ipv4Configuration, NetError> {
    core::future::poll_fn(|cx| poll_until(watchers, &status, cx, configured)).await
}

/// Wait until the link that is up now goes down, or comes up again in a
/// later epoch; a link down at the start ends the wait at once.
///
/// # Errors
/// As [`wait_ipv4`].
pub async fn wait_link_down<X: ExecutionContext, const K: usize>(
    watchers: &Watchers<X, K>,
    status: impl Fn() -> Status,
) -> Result<(), NetError> {
    let start = status().link;
    core::future::poll_fn(|cx| {
        poll_until(watchers, &status, cx, |current| {
            let link = current.link;
            (link.state == LinkState::Down || link.epoch != start.epoch).then_some(())
        })
    })
    .await
}

#[cfg(test)]
mod tests;
