//! Complete ESP32-S31 station scan composition.
//!
//! Initial and reconnect scans use different RX owners, but their PHY,
//! active-probe, storage and finite service composition is identical.  This
//! module is the single ownership transaction for that common work: callers
//! provide one coherent resource set and receive every owner back together
//! with a value-only decision.

use crate::roles::scan::{
    port::{
        ScanPhyPort, ScanPort, ScanPortError, ScanRadio, ScanReceivePort, ScanStation, ScanStorage,
        ScanTelemetry,
    },
    rx::ScanFrameObserver,
};

use oer_esp32s31_hal::shared_radio::PlatformClockProvider;
use oer_esp32s31_phy::{ConcurrentWifiChannelError, PhyTargetObserver};
use oer_esp32s31_radio_runtime::RadioSystem;

use crate::roles::radio_channel::RadioChannel;

use oer_esp32s31_ieee80211::coex::WifiCoexActivity;

use oer_esp32s31_ieee80211::ordinary_tx::{WifiTxEntropy, WifiTxPowerProfile};

use oer_esp32s31_ieee80211_mac::{
    init::{MacRuntimeStopHardware, MacSnifferHardware},
    tx::TxHardware,
};

use oer_esp32s31_ieee80211_sta::{
    attempt::{StaAttemptSecurity, StaIdentity},
    control_tx::{ControlTransmitter, ControlTxError},
    scan::{StaScanConfig, StaScanError},
    scan_tx::{RunningScanTx, ScanTxSummary},
};

use oer_ieee80211_sta_service::scan::StaScanBackend;

use oer_ieee80211_mac::{
    scan::{ScanRecord, ScanTable},
    security::StaSecurityPolicy,
    station::StaSequenceCounter,
};

use oer_ieee80211_sta::{
    request::{StationDiscovery, WifiSsid},
    scan::{StaCandidateScanExit, StaScanPlanError, StaScanProgress},
    station::{StaAttemptFailure, StaAttemptOutcome, StaFailureDisposition, StaLifecycleStage},
};
use oer_ieee80211_sta_service::scan::StaCandidateScanService;

use super::composer::StationInitialScanExit;

/// Qualified active-probe rate set used by the normal ESP32-S31 station
/// service. It belongs to the chip/runtime composition, not to board or HIL
/// policy.
pub const ESP32S31_STATION_PROBE_RATES: [u8; 12] = [
    0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24, 0x30, 0x48, 0x60, 0x6c,
];

/// Descriptor admission bound for one default active probe publication.
pub const ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY: u32 = 88;

/// Owned, finite S31 scan plan derived from one portable discovery request.
///
/// Board firmware and HIL may supply a preferred first channel, but neither
/// can replace the selected set, dwell or SSID after this value is built.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationScanPlan {
    config: StaScanConfig,
    channels: [u8; 14],
    channel_count: u8,
    ssid: WifiSsid,
    security: StaSecurityPolicy,
}

impl StationScanPlan {
    pub fn new(
        discovery: StationDiscovery,
        preferred_channel: Option<u8>,
        security: StaSecurityPolicy,
    ) -> Self {
        let mut channels = [0; 14];
        let mut channel_count = 0_usize;
        for channel in discovery
            .scan()
            .channels()
            .primary_channels_preferred(preferred_channel)
        {
            channels[channel_count] = channel;
            channel_count += 1;
        }
        Self {
            config: StaScanConfig::new(discovery.scan().dwell_millis())
                .expect("StationScanPolicy guarantees a nonzero dwell"),
            channels,
            channel_count: channel_count as u8,
            ssid: discovery.ssid(),
            security,
        }
    }

    pub const fn channel_count(&self) -> u8 {
        self.channel_count
    }

    pub fn channels(&self) -> &[u8] {
        &self.channels[..usize::from(self.channel_count)]
    }

    pub fn target_ssid(&self) -> &[u8] {
        self.ssid.as_bytes()
    }

    pub fn request(&self, station_address: [u8; 6]) -> StationScanRequest<'_, '_, '_> {
        StationScanRequest::new(
            self.config,
            self.channels(),
            station_address,
            self.target_ssid(),
            &ESP32S31_STATION_PROBE_RATES,
            self.security,
        )
        .with_descriptor_capacity(ESP32S31_STATION_PROBE_DESCRIPTOR_CAPACITY)
    }
}

/// Value-only policy for one finite station candidate scan.
#[derive(Clone, Copy)]
pub struct StationScanRequest<'ssid, 'rates, 'channels> {
    pub config: StaScanConfig,
    pub channels: &'channels [u8],
    pub station_address: [u8; 6],
    pub target_ssid: &'ssid [u8],
    pub supported_rates: &'rates [u8],
    pub descriptor_capacity: Option<u32>,
    pub select_candidate: bool,
    pub security: StaSecurityPolicy,
}

impl<'ssid, 'rates, 'channels> StationScanRequest<'ssid, 'rates, 'channels> {
    pub const fn new(
        config: StaScanConfig,
        channels: &'channels [u8],
        station_address: [u8; 6],
        target_ssid: &'ssid [u8],
        supported_rates: &'rates [u8],
        security: StaSecurityPolicy,
    ) -> Self {
        Self {
            config,
            channels,
            station_address,
            target_ssid,
            supported_rates,
            descriptor_capacity: None,
            select_candidate: true,
            security,
        }
    }

    pub const fn with_descriptor_capacity(mut self, capacity: u32) -> Self {
        self.descriptor_capacity = Some(capacity);
        self
    }

    /// Collect observations without selecting a peer for association.
    pub const fn without_candidate_selection(mut self) -> Self {
        self.select_candidate = false;
        self
    }
}

/// Exact owner set consumed by one initial or reconnect scan.
#[allow(clippy::type_complexity)]
pub struct StationScanResources<
    'radio,
    'storage,
    'sequence,
    'slot,
    P,
    C,
    Q,
    D,
    H,
    R,
    X,
    E,
    T,
    O,
    W,
    const RECORDS: usize,
    const TX_BUFFER_SIZE: usize,
> {
    /// The shared radio; each channel switch leases it for one transaction.
    pub radio: &'radio RadioSystem<P, C, D>,
    pub phy_observer: Q,
    pub hardware: H,
    pub receive: R,
    pub control: ControlTransmitter<'slot, X, E, T, TX_BUFFER_SIZE>,
    pub table: &'storage mut ScanTable<RECORDS>,
    pub frame: &'storage mut [u8],
    pub scan_observer: O,
    pub sequence: &'sequence mut StaSequenceCounter,
    pub timer: W,
}

/// Owners returned after the scan transaction has stopped RX.
///
/// The role-specific caller decides whether `receive` becomes an initial
/// halted ring or a stopped connected RX epoch.  Keeping that conversion
/// outside this common transaction prevents the two distinct owner frontiers
/// from being conflated.
#[allow(clippy::type_complexity)]
pub struct StationScanReturned<
    'storage,
    'sequence,
    'slot,
    P,
    H,
    R,
    X,
    E,
    T,
    O,
    W,
    const RECORDS: usize,
    const TX_BUFFER_SIZE: usize,
> {
    pub hardware: H,
    pub receive: R,
    pub control: ControlTransmitter<'slot, X, E, T, TX_BUFFER_SIZE>,
    pub phy_observer: P,
    pub scan_observer: O,
    pub timer: W,
    pub table: &'storage mut ScanTable<RECORDS>,
    pub frame: &'storage mut [u8],
    pub sequence: &'sequence mut StaSequenceCounter,
    pub telemetry: ScanTelemetry,
    pub transmit: ScanTxSummary,
}

/// Value-only result of one finite candidate scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[expect(
    clippy::large_enum_variant,
    reason = "the selected scan record is returned by value without heap allocation"
)]
pub enum StationScanDecision<E> {
    Selected {
        candidate: ScanRecord,
        progress: StaScanProgress,
    },
    NoCandidate {
        progress: StaScanProgress,
    },
    Stopped {
        progress: StaScanProgress,
    },
    Failed {
        error: E,
        progress: StaScanProgress,
    },
    InvalidPlan {
        error: StaScanPlanError,
        progress: StaScanProgress,
    },
}

/// Complete finite result: no decision is observable without all owners.
pub struct StationScanOutcome<R, E> {
    pub returned: R,
    pub decision: StationScanDecision<E>,
}

/// Outer lifecycle owners returned by a completely stopped initial scan.
///
/// Observation-only values stay outside this bundle. These are exactly the
/// movable owners needed either to enter the first join or reconstruct the
/// `InitialScan` station phase after a finite exit.
pub struct StationInitialScanReturned<'security, R, H, S, N> {
    pub runtime: R,
    pub hardware: H,
    pub receive: S,
    pub network: N,
    pub identity: StaIdentity,
    pub security: StaAttemptSecurity<'security>,
}

/// Consumer-specific diagnostic errors for the four initial-scan failures.
///
/// Their lifecycle severity is deliberately absent: the common completion
/// transaction below owns that safety policy. HIL and ordinary firmware may
/// use different error vocabularies without choosing different retry rules.
pub struct StationInitialScanFailures<E> {
    pub no_candidate: E,
    pub receive_handoff: E,
    pub transaction: E,
    pub invalid_plan: E,
}

/// Classify a failed production scan using the shared owner-safety policy.
///
/// A failed probe publication or an RX-stop failure may leave a hardware
/// frontier that cannot safely be retried. Other finite scan failures retain
/// a stopped/recoverable owner and may refresh the candidate.
pub const fn esp32s31_station_scan_failure_disposition<P, R, T>(
    error: &StaScanError<ScanPortError<P, R, T>>,
) -> StaFailureDisposition {
    if matches!(
        error,
        StaScanError::ActiveProbe(ScanPortError::Transmit(_)) | StaScanError::ReceiveStop(_)
    ) {
        StaFailureDisposition::Terminal
    } else {
        StaFailureDisposition::RefreshCandidate
    }
}

/// Complete the actor-owned initial scan and return one exact lifecycle edge.
///
/// `prepare_receive` is the only role-specific conversion: normal cold RX and
/// HIL may wrap the same halted ring differently. `restore_owner` only names
/// the consumer's concrete phase-owner alias; it cannot alter the common
/// disposition or candidate-selection rules.
#[allow(clippy::type_complexity)]
pub fn complete_esp32s31_station_initial_scan<'security, R, H, S, X, N, O, E, F, P, Q, T>(
    returned: StationInitialScanReturned<'security, R, H, S, N>,
    decision: StationScanDecision<StaScanError<ScanPortError<P, Q, T>>>,
    prepare_receive: impl FnOnce(S) -> Result<X, S>,
    restore_owner: impl FnOnce(R, H, S, N, StaIdentity, StaAttemptSecurity) -> O,
    failures: StationInitialScanFailures<E>,
) -> StationInitialScanExit<'security, R, H, X, N, O, E, F> {
    let StationInitialScanReturned {
        runtime,
        hardware,
        receive,
        network,
        identity,
        security,
    } = returned;
    let StationInitialScanFailures {
        no_candidate,
        receive_handoff,
        transaction,
        invalid_plan,
    } = failures;
    match decision {
        StationScanDecision::Selected { candidate, .. } => match prepare_receive(receive) {
            Ok(receive) => StationInitialScanExit::join_ready(
                runtime,
                hardware,
                receive,
                network,
                identity.select(candidate),
                security,
            ),
            Err(receive) => StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::Hardware,
                    StaFailureDisposition::Terminal,
                    receive_handoff,
                ),
            }),
        },
        StationScanDecision::NoCandidate { .. } => {
            StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    StaFailureDisposition::RefreshCandidate,
                    no_candidate,
                ),
            })
        }
        StationScanDecision::Stopped { .. } => {
            StationInitialScanExit::complete(StaAttemptOutcome::Stopped {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
            })
        }
        StationScanDecision::Failed { error, .. } => {
            let disposition = esp32s31_station_scan_failure_disposition(&error);
            StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    disposition,
                    transaction,
                ),
            })
        }
        StationScanDecision::InvalidPlan { .. } => {
            StationInitialScanExit::complete(StaAttemptOutcome::Failed {
                owner: restore_owner(runtime, hardware, receive, network, identity, security),
                failure: StaAttemptFailure::new(
                    StaLifecycleStage::CandidateSelection,
                    StaFailureDisposition::Terminal,
                    invalid_plan,
                ),
            })
        }
    }
}

/// Compose and run the common ESP32-S31 station candidate scan.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub async fn run_esp32s31_station_scan<
    'radio,
    'storage,
    'sequence,
    'slot,
    'ssid,
    'rates,
    'channels,
    P,
    C,
    Q,
    D,
    H,
    R,
    X,
    E,
    T,
    O,
    W,
    const RECORDS: usize,
    const TX_BUFFER_SIZE: usize,
>(
    resources: StationScanResources<
        'radio,
        'storage,
        'sequence,
        'slot,
        P,
        C,
        Q,
        D,
        H,
        R,
        X,
        E,
        T,
        O,
        W,
        RECORDS,
        TX_BUFFER_SIZE,
    >,
    request: StationScanRequest<'ssid, 'rates, 'channels>,
) -> StationScanOutcome<
    StationScanReturned<
        'storage,
        'sequence,
        'slot,
        Q,
        H,
        R,
        X,
        E,
        T,
        O,
        W,
        RECORDS,
        TX_BUFFER_SIZE,
    >,
    StaScanError<
        ScanPortError<ConcurrentWifiChannelError, <R as ScanReceivePort<H>>::Error, ControlTxError>,
    >,
>
where
    Q: PhyTargetObserver,
    D: oer_time::Timer,
    H: TxHardware + MacSnifferHardware + MacRuntimeStopHardware,
    R: ScanReceivePort<H>,
    X: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
    O: ScanFrameObserver,
    W: oer_time::Timer + crate::mac_clock::ReceptionClock,
    C: PlatformClockProvider,
    RadioChannel<'radio, P, C, Q, D>: ScanPhyPort<H, Error = ConcurrentWifiChannelError>,
{
    let StationScanResources {
        radio,
        phy_observer,
        hardware,
        receive,
        control,
        table,
        frame,
        scan_observer,
        sequence,
        timer,
    } = resources;
    let station = match request.descriptor_capacity {
        Some(capacity) => ScanStation::new(
            request.station_address,
            request.target_ssid,
            request.supported_rates,
            request.security,
        )
        .with_descriptor_capacity(capacity),
        None => ScanStation::new(
            request.station_address,
            request.target_ssid,
            request.supported_rates,
            request.security,
        ),
    }
    .with_candidate_selection(request.select_candidate);
    let owner = ScanPort::new(
        ScanRadio::new(
            RadioChannel::<_, _, _, D>::new(radio, phy_observer),
            hardware,
            receive,
            RunningScanTx::new(control),
        ),
        ScanStorage::new(table, frame, scan_observer, sequence),
        station,
        timer,
    );
    let mut service = StaCandidateScanService::new(StaScanBackend::new(request.config));
    let (owner, decision) = match service.run(owner, request.channels).await {
        StaCandidateScanExit::Selected {
            owner,
            candidate,
            progress,
        } => (
            owner,
            StationScanDecision::Selected {
                candidate,
                progress,
            },
        ),
        StaCandidateScanExit::NoCandidate { owner, progress } => {
            (owner, StationScanDecision::NoCandidate { progress })
        }
        StaCandidateScanExit::Stopped { owner, progress } => {
            (owner, StationScanDecision::Stopped { progress })
        }
        StaCandidateScanExit::Failed {
            owner,
            error,
            progress,
        } => (owner, StationScanDecision::Failed { error, progress }),
        StaCandidateScanExit::InvalidPlan {
            owner,
            error,
            progress,
        } => (owner, StationScanDecision::InvalidPlan { error, progress }),
    };
    let parts = owner.into_parts();
    // A station scans only while unassociated, so the vendor `scan_op_end`
    // publishes an idle Wi-Fi when the scan ends.
    parts
        .phy
        .publish_coex_activity(WifiCoexActivity::Idle)
        .await;
    let phy_observer = parts.phy.into_observer();
    let (control, transmit) = parts.tx.into_parts();
    StationScanOutcome {
        returned: StationScanReturned {
            hardware: parts.hardware,
            receive: parts.rx,
            control,
            phy_observer,
            scan_observer: parts.observer,
            timer: parts.timer,
            table: parts.table,
            frame: parts.frame,
            sequence: parts.sequence,
            telemetry: parts.telemetry,
            transmit,
        },
        decision,
    }
}

#[cfg(test)]
mod tests;
