//! What the station core decided without delivering it: lost tokens, lost
//! data, dropped RSN input and scan table outcomes; and for a sink that
//! measures, each step of a connect and each frame its scan heard.
//!
//! The station reports each as a [`StaEvent`] to the sink its caller lends a
//! call ([`oer_portable_events`]). It keeps no count of them.

/// One decision of the station core.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaEvent {
    /// A token came back to a full pool and was dropped: it was not one of
    /// the owner's, whose number the pool's creation checks.
    TokenLost,
    /// The data machine lost one MSDU or MPDU.
    DataLost(DataLoss),
    /// The connected station dropped one RSN input and kept its link.
    RsnInputDropped(RsnDrop),
    /// The station heard one well-formed beacon or probe response while
    /// scanning.
    ScanBssObserved(ScanOutcome),
    /// A returned buffer's memory was not the station's: its renewal or
    /// encoding was refused, and the token stays in its pool.
    BufferMemoryUnavailable,
    /// A data aggregate's terminal, or its refusal before the port, settled
    /// its members.
    AggregateSettled(AggregateSettled),
    /// The station sent a BlockAckReq to move a recipient's window past a
    /// dropped MPDU.
    BlockAckRequestSent,
    /// The station's step of a connect at `at`, reported at every entry
    /// whether it changed or not: a sink keeps the changes.
    ConnectStep {
        at: oer_portable_time::Instant,
        step: ConnectStep,
        /// The channel a scan visits or a join uses.
        channel: Option<u8>,
    },
    /// A beacon or probe response reached the scan at `at`.
    ScanHeard {
        at: oer_portable_time::Instant,
        heard: ScanHeard,
    },
}

/// Where a connect is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectStep {
    /// Not connecting: idle, returning owners, leaving or reporting why not.
    Rest,
    /// A scan visit asks for its channel.
    ScanAsk,
    /// The visit's lease is granted; the port moves at its start.
    ScanTune,
    /// The port is on the channel; the receive rules change.
    ScanConfigure,
    /// The visit's probe is owed.
    ScanProbe,
    /// The visit listens until its lease ends.
    ScanListen,
    /// The join asks for its channel.
    JoinAsk,
    /// The join's lease is granted; the port moves at its start.
    JoinTune,
    /// The port is on the channel; the receive rules change.
    JoinConfigure,
    /// Authenticating.
    Authenticate,
    /// Associating.
    Associate,
    /// The association's parameters go to the port and the link starts.
    StartLink,
    /// The WPA2 four-way handshake.
    Handshake,
    /// The link is up.
    Connected,
}

impl ConnectStep {
    /// Every step, each at its discriminant.
    pub const ALL: [Self; 14] = [
        Self::Rest,
        Self::ScanAsk,
        Self::ScanTune,
        Self::ScanConfigure,
        Self::ScanProbe,
        Self::ScanListen,
        Self::JoinAsk,
        Self::JoinTune,
        Self::JoinConfigure,
        Self::Authenticate,
        Self::Associate,
        Self::StartLink,
        Self::Handshake,
        Self::Connected,
    ];
}

/// One beacon or probe response a scan heard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanHeard {
    /// It names the connect's network.
    pub own: bool,
    /// A probe response, not a beacon.
    pub probe_response: bool,
    /// Its received level in dBm, when the receiver reported one.
    pub rssi: Option<i8>,
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
    pub holes: Option<u32>,
    /// The rate its last transmission went at; `None` when none reached
    /// the air.
    pub rate: Option<oer_protocols_ieee80211_contracts_port::PhyRate>,
}

/// The returned buffer, renewed for a new MSDU. A buffer whose memory is
/// not the station's still returns to its pool, reported.
pub(crate) fn fresh<B: oer_protocols_ieee80211_contracts_port::TxBufferMemory, S>(
    returned: oer_protocols_ieee80211_contracts_port::ReturnedMpdu<B, S>,
    renew: impl FnOnce(S) -> B,
    events: &mut impl oer_portable_events::EventSink<StaEvent>,
) -> B {
    let (buffer, renewed) = returned.into_fresh(renew);
    if renewed.is_err() {
        oer_portable_events::report(events, || StaEvent::BufferMemoryUnavailable);
    }
    buffer
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

/// Which ordinary RSN input of the connected link the station dropped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RsnDrop {
    /// An EAPOL-Key frame the supplicant refused: an invalid MIC, a stale
    /// replay counter or another non-fatal refusal.
    EapolRefused,
    /// The group key data did not unwrap.
    KeyUnwrapFailed,
    /// The group key data exceeded the station's bound.
    KeyDataTooLong,
    /// An unwrapped group key did not install.
    GroupInstallFailed,
}

/// What the scan table did with one beacon or probe response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanOutcome {
    /// A new BSS took a free record.
    Inserted,
    /// A known BSS's record changed.
    Updated,
    /// A known BSS's record stayed as it was.
    Duplicate,
    /// A new BSS found the table full: the scan result is incomplete.
    Dropped,
}
