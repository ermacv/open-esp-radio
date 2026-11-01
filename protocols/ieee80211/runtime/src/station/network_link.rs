//! The link as the network stack sees it: published by the station's
//! control, read by its network device, which may run on another hart.

use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Context;

use oer_memory::QueueNotify;
use oer_network_port::{LinkSnapshot, LinkState};
use oer_sync::PriorityWaker;

/// One word: the link's epoch in bits 1 to 31 and its level in bit 0.
///
/// The network side compares epochs for equality and change only, so the
/// word keeps the epoch's low 31 bits: an epoch repeats after 2^31
/// associations.
pub struct NetworkLink {
    word: AtomicU32,
    local: PriorityWaker,
    /// The network device's executor signal, when it runs on another hart.
    peer: Option<&'static dyn QueueNotify>,
}

impl NetworkLink {
    /// A link whose network device runs on the station's hart.
    pub const fn new() -> Self {
        Self {
            word: AtomicU32::new(0),
            local: PriorityWaker::new(),
            peer: None,
        }
    }

    /// A link whose network device runs on another hart: `notify` raises
    /// that device's executor signal.
    pub const fn across_harts(notify: &'static dyn QueueNotify) -> Self {
        Self {
            word: AtomicU32::new(0),
            local: PriorityWaker::new(),
            peer: Some(notify),
        }
    }

    fn encode(link: LinkSnapshot) -> u32 {
        // The low 31 bits always fit.
        let low = u32::try_from(link.epoch & u64::from(u32::MAX >> 1)).unwrap_or(0);
        (low << 1) | u32::from(link.state == LinkState::Up)
    }

    /// Publish `link` and tell the network device.
    pub(super) fn publish(&self, link: LinkSnapshot) {
        self.word.store(Self::encode(link), Ordering::Release);
        match self.peer {
            Some(peer) => peer.notify(),
            None => self.local.wake(),
        }
    }

    /// The published link, in the network side's epoch domain.
    pub fn snapshot(&self) -> LinkSnapshot {
        let word = self.word.load(Ordering::Acquire);
        LinkSnapshot {
            state: if word & 1 != 0 {
                LinkState::Up
            } else {
                LinkState::Down
            },
            epoch: u64::from(word >> 1),
        }
    }

    /// Make the polling task the one a publication wakes; the caller then
    /// reads the snapshot.
    pub fn register(&self, cx: &mut Context<'_>) {
        match self.peer {
            Some(peer) => peer.register(cx),
            None => self.local.register(cx.waker()),
        }
    }
}

impl Default for NetworkLink {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_published_link_reads_back_with_its_level_and_epoch() {
        let link = NetworkLink::new();
        assert_eq!(link.snapshot(), LinkSnapshot::DOWN);
        let up = LinkSnapshot {
            state: LinkState::Up,
            epoch: 7,
        };
        link.publish(up);
        assert_eq!(link.snapshot(), up);
        link.publish(LinkSnapshot {
            state: LinkState::Down,
            epoch: 7,
        });
        assert_eq!(link.snapshot().state, LinkState::Down);
        assert_eq!(link.snapshot().epoch, 7);
    }

    #[test]
    fn a_link_across_harts_notifies_its_peer() {
        use core::sync::atomic::AtomicUsize;
        struct Count(AtomicUsize);
        impl QueueNotify for Count {
            fn register(&self, _: &mut Context<'_>) {}
            fn notify(&self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        static PEER: Count = Count(AtomicUsize::new(0));
        let link = NetworkLink::across_harts(&PEER);
        link.publish(LinkSnapshot {
            state: LinkState::Up,
            epoch: 1,
        });
        assert_eq!(PEER.0.load(Ordering::Relaxed), 1);
        assert_eq!(link.snapshot().state, LinkState::Up);
    }
}
