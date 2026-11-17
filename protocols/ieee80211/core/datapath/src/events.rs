//! What the data plane decided without delivering it: lost data and how
//! each aggregate settled. The plane reports each as a [`DataEvent`] to the
//! sink its caller lends a call ([`oer_portable_events`]), and keeps no
//! count of them.

/// One decision of the data plane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataEvent {
    /// The data machine lost one MSDU or MPDU.
    DataLost(DataLoss),
    /// A data aggregate's terminal, or its refusal before the port, settled
    /// its members.
    AggregateSettled(AggregateSettled),
}

/// How one data aggregate's members settled.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AggregateSettled {
    /// Its members: the subframes it went with.
    pub members: u8,
    /// Members the BlockAck acknowledged.
    pub acknowledged: u8,
    /// Members back in their queue for a later aggregate.
    pub retried: u8,
    /// Members dropped: their lifetime or the policy's attempts ran out, a
    /// refusal or an outcome ended them, or their memory was lost.
    pub dropped: u8,
    /// Whether the aggregate was refused before the port.
    pub refused: bool,
    /// The positions its BlockAck left unacknowledged, bit 0 the first
    /// subframe on air: of a first transmission that got an answer.
    pub holes: Option<u64>,
    /// The rate its last transmission went at; `None` when none reached
    /// the air.
    pub rate: Option<oer_protocols_ieee80211_contracts_port::PhyRate>,
}

/// Why the data machine retired an MSDU of the current link, or dropped an
/// MPDU, without its delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataLoss {
    /// The hardware outcome of its transmission is unknown.
    Indeterminate,
    /// Its lifetime expired.
    Expired,
    /// Its aggregates on the air missed it as often as the policy allows.
    RetryLimit,
    /// Its contention budget ran out.
    ContentionLimited,
    /// A full queue could not hold it; its tokens came back.
    Overflow,
    /// Its link ended before it reached the air.
    LeftBehind,
}
