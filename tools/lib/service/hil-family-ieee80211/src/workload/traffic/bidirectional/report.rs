//! The typed results of a bidirectional session.

use super::*;

/// The result of a performance image's session: transport and host
/// samples only, since the image does not publish driver observation.
#[derive(Serialize)]
pub(super) struct BidirectionalPerformanceObservation<'a> {
    pub(super) config: &'a Config,
    pub(super) host_offer: HostTransmission,
    pub(super) host_sink: Burst,
    pub(super) session: SessionEvidence,
    pub(super) rx_kbps: u64,
    pub(super) tx_kbps: u64,
    pub(super) host_receive_buffer_bytes: usize,
}

/// The result of a qualified session: every host, target, fixture and air
/// sample the acceptance used, and the acceptance failure.
#[derive(Serialize)]
pub(super) struct BidirectionalObservation<'a> {
    pub(super) config: &'a Config,
    pub(super) delivery: &'static str,
    pub(super) evidence: &'a BidirectionalEvidence,
    pub(super) rx_delivery: Option<evidence::rx_delivery::RxDeliveryAssessment>,
    /// Cross-device correlation of the AP TX monitor and the independent air
    /// observer, when both ran.
    pub(super) air_correlation: Option<AirCorrelation>,
    pub(super) failure: Option<&'a str>,
}

/// The AP TX monitor's logical MPDUs matched against an independent air
/// observer. Matched MPDUs prove transmission on air; unmatched ones stay
/// ambiguous because a passive observer may miss frames. ath10k's AP monitor
/// tap omits the QoS TID, so matching projects it away.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub(super) struct AirCorrelation {
    pub(super) ap_with_metadata: u32,
    pub(super) observer_with_metadata: u32,
    pub(super) ap_known_tid: u32,
    pub(super) observer_known_tid: u32,
    /// Matched by `(sequence, fragment)`.
    pub(super) matched: u32,
    pub(super) ap_not_observed: u32,
    pub(super) observer_extra: u32,
}

impl AirCorrelation {
    pub(super) fn of(ap: &OpenWrtTxMonitorEvidence, observer: &LocalAirMonitorEvidence) -> Self {
        let with_metadata = |units: &BTreeMap<MacFrameKey, u32>| units.values().sum::<u32>();
        let known_tid = |units: &BTreeMap<MacFrameKey, u32>| {
            units
                .iter()
                .filter(|(key, _)| key.tid != u8::MAX)
                .map(|(_, count)| count)
                .sum::<u32>()
        };
        let ap_projected = project_mac_units(&ap.mac_units);
        let observer_projected = project_mac_units(&observer.mac_units);
        let matched = ap_projected
            .iter()
            .map(|(key, count)| *count.min(observer_projected.get(key).unwrap_or(&0)))
            .sum::<u32>();
        let ap_with_metadata = with_metadata(&ap.mac_units);
        let observer_with_metadata = with_metadata(&observer.mac_units);
        Self {
            ap_with_metadata,
            observer_with_metadata,
            ap_known_tid: known_tid(&ap.mac_units),
            observer_known_tid: known_tid(&observer.mac_units),
            matched,
            ap_not_observed: ap_with_metadata.saturating_sub(matched),
            observer_extra: observer_with_metadata.saturating_sub(matched),
        }
    }
}

/// Project captures onto the fields which both monitor drivers expose
/// consistently. ath10k's AP monitor tap omits QoS TID for these frames, while
/// the independent Intel monitor reports it. Sequence and fragment therefore
/// form the strongest honest cross-device correlation key available here.
pub(super) fn project_mac_units(units: &BTreeMap<MacFrameKey, u32>) -> BTreeMap<(u16, u8), u32> {
    let mut projected = BTreeMap::new();
    for (key, count) in units {
        let total = projected
            .entry((key.sequence, key.fragment))
            .or_insert(0_u32);
        *total = total.saturating_add(*count);
    }
    projected
}
