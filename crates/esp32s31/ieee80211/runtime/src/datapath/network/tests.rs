use core::cell::RefCell;
use std::vec::Vec;

use super::{DatapathNetworkRx, EthernetFrameParts};
use oer_network_interface::RxEnqueueError;

#[derive(Debug, Eq, PartialEq)]
enum Event {
    Observed,
    Sent,
    PartsSent,
}

/// A publisher that implements only the required methods.
struct Required<'a>(&'a RefCell<Vec<Event>>);

impl DatapathNetworkRx for Required<'_> {
    fn queue_len(&self) -> usize {
        0
    }

    fn try_send(&mut self, _frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.0.borrow_mut().push(Event::Sent);
        Ok(())
    }

    fn try_send_parts(&mut self, _frame: EthernetFrameParts<'_>) -> Result<(), RxEnqueueError> {
        self.0.borrow_mut().push(Event::PartsSent);
        Ok(())
    }

    fn poll_ready(&mut self, _context: &mut core::task::Context<'_>) -> core::task::Poll<()> {
        core::task::Poll::Ready(())
    }
}

#[test]
fn default_observed_sends_observe_once_before_publication() {
    let events = RefCell::new(Vec::new());
    let mut publisher = Required(&events);
    let mut observe = || events.borrow_mut().push(Event::Observed);
    assert_eq!(publisher.try_send_observed(&[0; 14], &mut observe), Ok(()));
    let frame = EthernetFrameParts {
        destination: [0; 6],
        source: [0; 6],
        ether_type: 0x0800,
        payload: &[],
    };
    assert_eq!(
        publisher.try_send_parts_observed(frame, &mut observe),
        Ok(())
    );
    assert_eq!(
        *events.borrow(),
        [
            Event::Observed,
            Event::Sent,
            Event::Observed,
            Event::PartsSent
        ]
    );
}
