//! The hand-over of one request from the console to the image's worker.

use oer_hil_protocol::RequestIdentity;
use oer_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};

type Entry<R> = (RequestIdentity, R);

/// Room for one request of the image's service set `R` between the console
/// and the worker. The image keeps it in a static and splits it once.
pub struct Mailbox<R>(WakingSpscQueue<Entry<R>, 1>);

impl<R> Mailbox<R> {
    pub const fn new() -> Self {
        Self(WakingSpscQueue::new())
    }

    /// The console's end and the worker's end.
    pub fn split(&mut self) -> (Outbox<'_, R>, Inbox<'_, R>) {
        let (sender, receiver) = self.0.split();
        (Outbox(sender), Inbox(receiver))
    }
}

impl<R> Default for Mailbox<R> {
    fn default() -> Self {
        Self::new()
    }
}

/// A request arrived while the worker had not yet taken the previous one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Busy;

/// The console's end: it hands each request over without waiting.
pub struct Outbox<'a, R>(WakingSpscSender<'a, Entry<R>, 1>);

impl<R> Outbox<'_, R> {
    /// Hand `request` to the worker, or refuse it while another waits.
    ///
    /// # Errors
    /// [`Busy`] while the worker has not taken the previous request.
    pub fn hand(&self, identity: RequestIdentity, request: R) -> Result<(), Busy> {
        self.0.try_send((identity, request)).map_err(|_| Busy)
    }
}

/// The worker's end: the requests in the order the console took them.
pub struct Inbox<'a, R>(WakingSpscReceiver<'a, Entry<R>, 1>);

impl<R> Inbox<'_, R> {
    /// The next request, once the console hands one over.
    pub async fn next(&mut self) -> (RequestIdentity, R) {
        loop {
            self.0.ready().await;
            if let Some(entry) = self.0.try_receive() {
                return entry;
            }
        }
    }
}

#[cfg(test)]
mod tests;
