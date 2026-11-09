//! The link as the network stack sees it: published by the station's
//! control, read by its network device, which may run on another hart.

use core::sync::atomic::{AtomicU32, Ordering};
use core::task::Context;

use oer_portable_memory::QueueNotify;
use oer_portable_sync::PriorityWaker;
use oer_protocols_ieee80211_contracts_port::PhyRate;
use oer_protocols_ieee80211_core_upper_mac::rate_control::LegacyRateCode;
use oer_protocols_network_port::{LinkSnapshot, LinkState};

/// Where the word keeps the association's slowest rate, and its epoch.
const FLOOR_SHIFT: u32 = 1;
const FLOOR_MASK: u32 = 0xf;
const EPOCH_SHIFT: u32 = 5;
const EPOCH_MASK: u32 = u32::MAX >> EPOCH_SHIFT;

/// One word: the link's level in bit 0, the slowest data rate of the
/// association it runs, as a [`LegacyRateCode`] in bits 1 to 4 (zero for
/// none), and its epoch in bits 5 to 31; and `N`, how a publication tells
/// the network device: a local waker, or the device's executor signal when
/// it runs on another hart. One word, so a reader never sees one
/// association's epoch with another's rate.
///
/// The network side compares epochs for equality and change only, so the
/// word keeps the epoch's low 27 bits: an epoch repeats after 2^27
/// associations.
pub struct NetworkLink<N = PriorityWaker> {
    word: AtomicU32,
    notify: N,
}

impl NetworkLink {
    /// A link whose network device runs on the station's hart.
    pub const fn new() -> Self {
        Self::across_harts(PriorityWaker::new())
    }
}

impl<N: QueueNotify> NetworkLink<N> {
    /// A link whose publications tell the network device through `notify`:
    /// for a device on another hart, its executor signal.
    pub const fn across_harts(notify: N) -> Self {
        Self {
            word: AtomicU32::new(0),
            notify,
        }
    }

    fn encode(link: LinkSnapshot, floor: Option<PhyRate>) -> u32 {
        // The low 27 bits always fit.
        let low = u32::try_from(link.epoch & u64::from(EPOCH_MASK)).unwrap_or(0);
        let floor = floor
            .and_then(LegacyRateCode::of)
            .map_or(0, |code| u32::from(code.bits()) & FLOOR_MASK);
        (low << EPOCH_SHIFT) | (floor << FLOOR_SHIFT) | u32::from(link.state == LinkState::Up)
    }

    /// Publish `link`, of an association whose slowest data rate is `floor`,
    /// and tell the network device.
    pub(super) fn publish(&self, link: LinkSnapshot, floor: Option<PhyRate>) {
        self.word
            .store(Self::encode(link, floor), Ordering::Release);
        self.notify.notify();
    }

    /// The published link, in the network side's epoch domain.
    pub fn snapshot(&self) -> LinkSnapshot {
        self.read().0
    }

    /// The published link and its association's slowest data rate, from
    /// one publication.
    pub fn read(&self) -> (LinkSnapshot, Option<PhyRate>) {
        let word = self.word.load(Ordering::Acquire);
        let link = LinkSnapshot {
            state: if word & 1 != 0 {
                LinkState::Up
            } else {
                LinkState::Down
            },
            epoch: u64::from(word >> EPOCH_SHIFT),
        };
        let floor = u8::try_from((word >> FLOOR_SHIFT) & FLOOR_MASK)
            .ok()
            .and_then(LegacyRateCode::from_bits)
            .and_then(LegacyRateCode::rate);
        (link, floor)
    }

    /// Make the polling task the one a publication wakes; the caller then
    /// reads the snapshot.
    pub fn register(&self, cx: &mut Context<'_>) {
        self.notify.register(cx);
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
        link.publish(up, None);
        assert_eq!(link.snapshot(), up);
        link.publish(
            LinkSnapshot {
                state: LinkState::Down,
                epoch: 7,
            },
            None,
        );
        assert_eq!(link.snapshot().state, LinkState::Down);
        assert_eq!(link.snapshot().epoch, 7);
    }

    /// The association's slowest rate goes in the same word as its epoch,
    /// and every epoch the word keeps reads back whole.
    #[test]
    fn a_link_carries_its_association_s_slowest_rate_with_its_epoch() {
        use oer_protocols_ieee80211_wire::phy::LegacyRate;
        let link = NetworkLink::new();
        let up = |epoch| LinkSnapshot {
            state: LinkState::Up,
            epoch,
        };
        let six = PhyRate::Legacy(LegacyRate::Ofdm6M);
        link.publish(up(3), Some(six));
        assert_eq!(link.read(), (up(3), Some(six)));
        let one = PhyRate::Legacy(LegacyRate::Dsss1M);
        let last = u64::from(EPOCH_MASK);
        link.publish(up(last), Some(one));
        assert_eq!(link.read(), (up(last), Some(one)));
        link.publish(up(4), None);
        assert_eq!(link.read(), (up(4), None));
        // A rate no legacy entry names is no floor.
        let ht = PhyRate::Ht(
            oer_protocols_ieee80211_wire::phy::HtRate::new(
                oer_protocols_ieee80211_wire::phy::HtMcs::new(0).unwrap(),
                oer_protocols_ieee80211_wire::phy::PpduBandwidth::Mhz20,
                false,
            )
            .unwrap(),
        );
        link.publish(up(5), Some(ht));
        assert_eq!(link.read(), (up(5), None));
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
        link.publish(
            LinkSnapshot {
                state: LinkState::Up,
                epoch: 1,
            },
            None,
        );
        assert_eq!(PEER.0.load(Ordering::Relaxed), 1);
        assert_eq!(link.snapshot().state, LinkState::Up);
    }
}
