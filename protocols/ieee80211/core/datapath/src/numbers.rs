//! The link's transmit numbers: its sequence spaces and the pairwise key
//! whose packet numbers its protected MPDUs draw. The data plane draws a
//! data MPDU's from them, and the link's holder a station frame's.

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

/// The link's transmit numbers: the station's own sequence space, each
/// TID's space on a QoS BSS and the pairwise key `P` of a protected link,
/// whose packet numbers every protected MPDU of the link draws from.
pub struct TxNumbers<P> {
    station: SequenceCounter,
    tids: Option<[SequenceCounter; 8]>,
    key: Option<P>,
}

impl<P> TxNumbers<P> {
    pub const fn new(
        station: SequenceCounter,
        tids: Option<[SequenceCounter; 8]>,
        key: Option<P>,
    ) -> Self {
        Self { station, tids, key }
    }

    /// Whether the link is a QoS BSS's: a sequence space per TID.
    pub const fn qos(&self) -> bool {
        self.tids.is_some()
    }

    /// The link's pairwise key; `None` on an open link.
    pub const fn key(&self) -> Option<&P> {
        self.key.as_ref()
    }

    /// The next Sequence Number of data of `tid`: its TID's on a QoS BSS,
    /// the station's own otherwise.
    pub fn next(&self, tid: WmmUserPriority) -> SequenceNumber {
        match &self.tids {
            Some(tids) => tid.select(tids).peek(),
            None => self.station.peek(),
        }
    }

    /// The counter data of `tid` takes its numbers from, and the key its
    /// packet numbers come from.
    pub(crate) fn data_and_key(
        &mut self,
        tid: WmmUserPriority,
    ) -> (&mut SequenceCounter, Option<&P>) {
        let counter = match &mut self.tids {
            Some(tids) => tid.select_mut(tids),
            None => &mut self.station,
        };
        (counter, self.key.as_ref())
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

    /// The link's sequence spaces and key, taken apart.
    pub fn into_parts(self) -> (SequenceCounter, Option<[SequenceCounter; 8]>, Option<P>) {
        (self.station, self.tids, self.key)
    }
}
