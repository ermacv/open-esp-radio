//! What the station core decided without delivering it: lost tokens, lost
//! data, dropped RSN input and scan table outcomes.
//!
//! The station reports each as a [`StaEvent`] to the sink its caller lends a
//! call ([`oer_events`]). It keeps no count of them.

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
}

/// Why the data machine retired an MSDU of the current link, or dropped an
/// MPDU, without its delivery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataLoss {
    /// A preparation refusal ended it before admission.
    PreparationRefused,
    /// The hardware outcome of its transmission is unknown.
    Indeterminate,
    /// Its lifetime expired.
    Expired,
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
