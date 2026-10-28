//! Events of the station core: its token pools, its data machine, its RSN
//! input and its scan table.

use crate::STATION;
use crate::event::{coded, coded_event, unit_event};

unit_event!(
    /// A token came back to a full pool and was dropped: it was not one of
    /// the owner's, whose number the pool's creation checks.
    TokenLost = 1 on STATION,
    "station.token-lost"
);

coded!(
    /// Why the data machine retired an MSDU of the current link, or dropped
    /// an MPDU, without its delivery.
    pub enum DataLoss {
        /// A preparation refusal ended it before admission.
        PreparationRefused = 0,
        /// The hardware outcome of its transmission is unknown.
        Indeterminate = 1,
        /// Its lifetime expired.
        Expired = 2,
        /// Its contention budget ran out.
        ContentionLimited = 3,
        /// A full queue could not hold it; its tokens came back.
        Overflow = 4,
        /// Its link ended before it reached the air.
        LeftBehind = 5,
    }
);

coded_event!(
    /// The data machine lost one MSDU or MPDU.
    DataMsduLost { reason: DataLoss } = 2 on STATION,
    "station.data-lost"
);

coded!(
    /// Which ordinary RSN input of the connected link the station dropped.
    pub enum RsnDrop {
        /// An EAPOL-Key frame the supplicant refused: an invalid MIC, a
        /// stale replay counter or another non-fatal refusal.
        EapolRefused = 0,
        /// The group key data did not unwrap.
        KeyUnwrapFailed = 1,
        /// The group key data exceeded the station's bound.
        KeyDataTooLong = 2,
        /// An unwrapped group key did not install.
        GroupInstallFailed = 3,
    }
);

coded_event!(
    /// The connected station dropped one RSN input and kept its link.
    RsnInputDropped { cause: RsnDrop } = 3 on STATION,
    "station.rsn-dropped"
);

coded!(
    /// What the scan table did with one beacon or probe response.
    pub enum ScanOutcome {
        /// A new BSS took a free record.
        Inserted = 0,
        /// A known BSS's record changed.
        Updated = 1,
        /// A known BSS's record stayed as it was.
        Duplicate = 2,
        /// A new BSS found the table full: the scan result is incomplete.
        Dropped = 3,
    }
);

coded_event!(
    /// The station heard one well-formed beacon or probe response while
    /// scanning.
    ScanBssObserved { outcome: ScanOutcome } = 4 on STATION,
    "station.scan-bss"
);
