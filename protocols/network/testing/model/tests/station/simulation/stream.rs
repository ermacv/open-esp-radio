//! A local producer that sends a bounded stream of datagrams to one peer,
//! as an image's load source does: the service declares its offers and fills
//! them in the port's slot in its packet turns.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;
use std::task::{Context, Waker};

use oer_portable_time::Instant;
use oer_protocols_network_core::{Declined, Outcome, Payload, Remote, SendRefusal};
use oer_protocols_network_runtime::{Interest, Next, Offer, Produce, Work};

/// What the stream sends and what became of it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamCounts {
    /// Datagrams not yet offered.
    pub left: usize,
    /// Offers declared or refused, each once.
    pub offered: usize,
    pub filled: usize,
    pub accepted: usize,
    /// Works that ended otherwise.
    pub other: usize,
    /// Temporary refusals, each offered again.
    pub held: usize,
}

struct Source {
    to: Remote,
    length: usize,
    counts: StreamCounts,
    next: u32,
    /// A refused tag, offered again first.
    again: Option<u32>,
    open: BTreeSet<u32>,
    waker: Option<Waker>,
}

/// The stream, shared between the service's producer and the case.
#[derive(Clone)]
pub struct Stream(Rc<RefCell<Source>>);

impl Stream {
    /// `count` datagrams of `length` octets to `to`.
    pub fn new(to: Remote, length: usize, count: usize) -> Self {
        Self(Rc::new(RefCell::new(Source {
            to,
            length,
            counts: StreamCounts {
                left: count,
                ..StreamCounts::default()
            },
            next: 0,
            again: None,
            open: BTreeSet::new(),
            waker: None,
        })))
    }

    pub fn counts(&self) -> StreamCounts {
        self.0.borrow().counts
    }
}

impl Produce for Stream {
    type Tag = u32;

    fn generation(&self) -> u32 {
        0
    }

    fn current(&self, tag: u32) -> bool {
        let source = self.0.borrow();
        source.open.contains(&tag) || source.again == Some(tag)
    }

    fn offer(&mut self, _now: Instant) -> Result<Offer<u32>, Next> {
        let mut source = self.0.borrow_mut();
        let tag = match source.again.take() {
            Some(tag) => tag,
            None if source.counts.left > 0 => {
                source.counts.left -= 1;
                source.counts.offered += 1;
                let tag = source.next;
                source.next += 1;
                tag
            }
            None => return Err(Next::Idle),
        };
        source.open.insert(tag);
        Ok(Offer {
            remote: source.to,
            length: source.length,
            tag,
        })
    }

    fn fill(
        &mut self,
        _now: Instant,
        work: Work<u32>,
        data: &mut Payload<'_>,
    ) -> Result<(), Declined> {
        let mut source = self.0.borrow_mut();
        if !source.open.contains(&work.tag) {
            return Err(Declined::Stale);
        }
        source.counts.filled += 1;
        let mut octets = vec![(work.tag % 251) as u8; source.length];
        octets[..4].copy_from_slice(&work.tag.to_be_bytes());
        data.put(&octets).map_err(|_| Declined::Unwritable)
    }

    fn refused(&mut self, tag: u32, reason: SendRefusal) {
        let mut source = self.0.borrow_mut();
        source.open.remove(&tag);
        if matches!(
            reason,
            SendRefusal::NoPlace | SendRefusal::NeighborsFull | SendRefusal::NoAddress
        ) {
            source.counts.held += 1;
            source.again = Some(tag);
        } else {
            source.counts.other += 1;
        }
    }

    fn ended(&mut self, work: Work<u32>, outcome: Outcome) {
        let mut source = self.0.borrow_mut();
        source.open.remove(&work.tag);
        if outcome == Outcome::Accepted {
            source.counts.accepted += 1;
        } else {
            source.counts.other += 1;
        }
    }

    fn register(&mut self, cx: &mut Context<'_>, _interest: Interest) -> bool {
        let mut source = self.0.borrow_mut();
        source.waker = Some(cx.waker().clone());
        source.again.is_some() || source.counts.left > 0
    }
}
