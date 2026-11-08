//! The station services' waits on the network's publications, through the
//! image's own module: an address over an up link while the DHCP client
//! holds it bound, and the loss of the link up at a wait's start. `oer`
//! stands in for its chip-side names.
extern crate self as oer;

pub mod wifi {
    pub use oer_protocols_network_core::{Configuration, DhcpState, Ipv4};
    pub use oer_protocols_network_port::{LinkSnapshot, LinkState};
    pub use oer_protocols_network_runtime::WatchRefused;
}

#[path = "../src/waits.rs"]
mod waits;

use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use wifi::{Configuration, DhcpState, Ipv4, LinkSnapshot, LinkState, WatchRefused};

const OURS: [u8; 4] = [10, 231, 0, 50];

fn up(epoch: u64) -> LinkSnapshot {
    LinkSnapshot {
        state: LinkState::Up,
        epoch,
    }
}

fn ipv4() -> Ipv4 {
    Ipv4::new(OURS, 24, Some([10, 231, 0, 1])).unwrap()
}

fn published(dhcp: Option<DhcpState>, link: LinkSnapshot) -> Configuration {
    Configuration {
        ipv4: Some(ipv4()),
        version: 1,
        lease: None,
        dhcp,
        link,
    }
}

/// The network's publications as a test sets them.
struct Watch {
    value: Cell<Option<Configuration>>,
    changes: Cell<u32>,
    refused: Cell<bool>,
}

impl Watch {
    fn new(value: Option<Configuration>) -> Self {
        Self {
            value: Cell::new(value),
            changes: Cell::new(1),
            refused: Cell::new(false),
        }
    }

    fn publish(&self, value: Option<Configuration>) {
        self.value.set(value);
        self.changes.set(self.changes.get().wrapping_add(1));
    }
}

impl waits::Published for Watch {
    fn configuration(&self) -> Result<Option<Configuration>, WatchRefused> {
        if self.refused.get() {
            Err(WatchRefused::Publishing)
        } else {
            Ok(self.value.get())
        }
    }

    fn changes(&self) -> u32 {
        self.changes.get()
    }

    async fn changed(&self, seen: u32) -> u32 {
        poll_fn(|_| {
            let changes = self.changes.get();
            if changes == seen {
                Poll::Pending
            } else {
                Poll::Ready(changes)
            }
        })
        .await
    }
}

fn poll<F: Future>(future: core::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

#[test]
fn only_a_bound_client_over_an_up_link_holds_the_address_a_wait_ends_with() {
    for bound in [
        None,
        Some(DhcpState::Bound),
        Some(DhcpState::Renewing),
        Some(DhcpState::Rebinding),
    ] {
        assert_eq!(waits::leased(&published(bound, up(1))), Some(ipv4()));
        // The address of a link that went is no address over an up link.
        assert_eq!(waits::leased(&published(bound, LinkSnapshot::DOWN)), None);
    }
    // Still asking: an INIT-REBOOT keeps the address in use, but no server
    // confirmed it on this link yet.
    for asking in [
        DhcpState::Selecting,
        DhcpState::Requesting,
        DhcpState::Rebooting,
    ] {
        assert_eq!(waits::leased(&published(Some(asking), up(2))), None);
    }
    let mut none = published(Some(DhcpState::Bound), up(1));
    none.ipv4 = None;
    assert_eq!(waits::leased(&none), None);
}

#[test]
fn a_link_is_lost_once_it_is_down_or_up_again_in_a_later_epoch() {
    assert!(!waits::lost(up(3), up(3)));
    assert!(waits::lost(up(3), LinkSnapshot::DOWN));
    assert!(waits::lost(up(3), up(4)));
    // Down at the start: lost at once.
    assert!(waits::lost(LinkSnapshot::DOWN, LinkSnapshot::DOWN));
}

#[test]
fn a_wait_for_an_address_goes_on_across_a_link_loss_to_the_next_bound_lease() {
    let watch = Watch::new(Some(published(Some(DhcpState::Bound), LinkSnapshot::DOWN)));
    let mut wait = pin!(waits::until(&watch, waits::leased));
    assert!(poll(wait.as_mut()).is_pending());
    watch.publish(Some(published(Some(DhcpState::Rebooting), up(2))));
    assert!(poll(wait.as_mut()).is_pending());
    watch.publish(Some(published(Some(DhcpState::Bound), up(2))));
    assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(Some(ipv4()))));
}

#[test]
fn a_wait_ends_without_an_answer_once_the_network_ended() {
    let watch = Watch::new(Some(published(Some(DhcpState::Selecting), up(1))));
    let mut wait = pin!(waits::until(&watch, waits::leased));
    assert!(poll(wait.as_mut()).is_pending());
    watch.publish(None);
    assert_eq!(poll(wait.as_mut()), Poll::Ready(Ok(None)));
}

#[test]
fn a_wait_for_the_link_s_loss_ends_at_a_later_epoch_and_a_refused_read_is_its_error() {
    let start = up(1);
    let watch = Watch::new(Some(published(Some(DhcpState::Bound), start)));
    let mut lost = pin!(waits::until(&watch, |configuration| {
        waits::lost(start, configuration.link).then_some(())
    }));
    assert!(poll(lost.as_mut()).is_pending());
    // The same link published again, with a renewal: not lost.
    watch.publish(Some(published(Some(DhcpState::Renewing), start)));
    assert!(poll(lost.as_mut()).is_pending());
    watch.publish(Some(published(Some(DhcpState::Rebooting), up(2))));
    assert_eq!(poll(lost.as_mut()), Poll::Ready(Ok(Some(()))));

    let refused = Watch::new(None);
    refused.refused.set(true);
    let mut wait = pin!(waits::until(&refused, waits::leased));
    assert_eq!(
        poll(wait.as_mut()),
        Poll::Ready(Err(WatchRefused::Publishing))
    );
}
