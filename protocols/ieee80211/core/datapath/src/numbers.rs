//! The link's transmit numbers: its sequence spaces. The data plane draws a
//! data MPDU's from them, and the link's holder a station frame's. The
//! pairwise key whose packet numbers a protected MPDU draws ([`TxKey`]) is
//! held beside them, by whoever holds the port's key.

use oer_protocols_ieee80211_core_upper_mac::ccmp::{CcmpTxPacketNumber, InstalledCcmpTxKey};
use oer_protocols_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_protocols_ieee80211_wire::qos::WmmUserPriority;
use oer_protocols_ieee80211_wire::sequence::SequenceNumber;

/// The space a station frame's Sequence Number comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkSpace {
    /// The station's own space: its management frames, its non-QoS data
    /// and EAPOL, and every data MPDU of a non-QoS BSS.
    Station,
    /// A TID's space of a QoS BSS, which the data of that TID shares.
    Tid(WmmUserPriority),
}

/// A pairwise key whose transmit packet numbers the link draws from.
pub trait TxKey {
    /// The key's transmit packet numbers.
    fn packet_numbers(&self) -> &CcmpTxPacketNumber;
}

impl<K> TxKey for InstalledCcmpTxKey<K> {
    fn packet_numbers(&self) -> &CcmpTxPacketNumber {
        InstalledCcmpTxKey::packet_numbers(self)
    }
}

/// The link's transmit numbers: the station's own sequence space and each
/// TID's space on a QoS BSS. The pairwise key whose packet numbers a
/// protected MPDU draws is not here: it lives with the port's key it
/// belongs to.
pub struct TxNumbers {
    station: SequenceCounter,
    tids: Option<[SequenceCounter; 8]>,
}

impl TxNumbers {
    pub const fn new(station: SequenceCounter, tids: Option<[SequenceCounter; 8]>) -> Self {
        Self { station, tids }
    }

    /// `tid`'s counter goes back to `next`: the numbers after it named no
    /// frame that was transmitted.
    pub(crate) fn rewind(&mut self, tid: WmmUserPriority, next: SequenceNumber) {
        *self.counter(tid) = SequenceCounter::new(next);
    }

    /// Whether the link is a QoS BSS's: a sequence space per TID.
    pub const fn qos(&self) -> bool {
        self.tids.is_some()
    }

    /// The next Sequence Number of data of `tid`: its TID's on a QoS BSS,
    /// the station's own otherwise.
    pub fn next(&self, tid: WmmUserPriority) -> SequenceNumber {
        match &self.tids {
            Some(tids) => tid.select(tids).peek(),
            None => self.station.peek(),
        }
    }

    /// The counter data of `tid` takes its numbers from.
    pub(crate) fn counter(&mut self, tid: WmmUserPriority) -> &mut SequenceCounter {
        match &mut self.tids {
            Some(tids) => tid.select_mut(tids),
            None => &mut self.station,
        }
    }

    /// Name the next Sequence Number of `space` for a controller's frame:
    /// the station's own space's is taken, as the frame carries it; a TID's
    /// is only read, and the TID's first data MPDU after carries it (an
    /// ADDBA Request's Starting Sequence Number, IEEE Std 802.11-2020,
    /// 9.4.1.13). `None` for a TID of a non-QoS BSS.
    pub fn name(&mut self, space: LinkSpace) -> Option<SequenceNumber> {
        match (space, &self.tids) {
            (LinkSpace::Station, _) => Some(self.station.take()),
            (LinkSpace::Tid(tid), Some(tids)) => Some(tid.select(tids).peek()),
            (LinkSpace::Tid(_), None) => None,
        }
    }

    /// Take the next Sequence Number of `space`; `None` for a TID of a
    /// non-QoS BSS.
    pub fn take(&mut self, space: LinkSpace) -> Option<SequenceNumber> {
        match (space, &mut self.tids) {
            (LinkSpace::Station, _) => Some(self.station.take()),
            (LinkSpace::Tid(tid), Some(tids)) => Some(tid.select_mut(tids).take()),
            (LinkSpace::Tid(_), None) => None,
        }
    }

    /// The numbers' state, for the engine's search: each space's next.
    #[cfg(test)]
    pub(crate) fn state_key(&self) -> [u16; 9] {
        let mut next = [self.station.peek().get(); 9];
        if let Some(tids) = &self.tids {
            for (slot, tid) in next.iter_mut().skip(1).zip(tids) {
                *slot = tid.peek().get();
            }
        }
        next
    }

    /// The link's sequence spaces, taken apart.
    pub fn into_parts(self) -> (SequenceCounter, Option<[SequenceCounter; 8]>) {
        (self.station, self.tids)
    }
}
