//! The IEEE 802.15.4 module: the radio's probes, air check, peer session and
//! Thread client, with the properties IEEE 802.15.4 images advertise.

#[cfg(feature = "i")]
use postcard_schema::Schema;
#[cfg(feature = "i")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "i")]
mod payload;
#[cfg(feature = "i")]
pub use payload::*;

crate::messages! {
    /// The bounded `EVENT_STATUS` observation probe.
    property EventStatusProbe = "ieee802154/event-status-probe";
    /// The bounded ED-DONE/TIMER0 selective-write discriminator.
    property EdEventProbe = "ieee802154/ed-event-probe";
    /// The single-device on-air check.
    property AirCheck = "ieee802154/air-check";
    /// A peer session.
    property Session = "ieee802154/session";
    /// OpenThread over the composed client.
    property Thread = "ieee802154/thread";
    /// Same-bit arrival and level retrigger of the source-132 route.
    property RouteProbe = "ieee802154/route-probe";
    /// The MAC trace: a failure's post-mortem holds the last MAC events.
    property MacTrace = "ieee802154/mac-trace";
    #[cfg(feature = "i")]
    endpoint ProbeEventStatus = "ieee802154/event-status/probe" => crate::ieee802154::EventStatusProbed;
    #[cfg(feature = "i")]
    topic EventStatusProbed = "ieee802154/event-status/probed";
    #[cfg(feature = "i")]
    endpoint ProbeRoute = "ieee802154/route/probe" => crate::ieee802154::RouteProbed;
    #[cfg(feature = "i")]
    topic RouteProbed = "ieee802154/route/probed";
    #[cfg(feature = "i")]
    endpoint ProbeEdEvent = "ieee802154/ed-event/probe" => crate::ieee802154::EdEventProbed;
    #[cfg(feature = "i")]
    topic EdEventProbed = "ieee802154/ed-event/probed";
    #[cfg(feature = "i")]
    endpoint RunAirCheck = "ieee802154/air-check/run" => crate::ieee802154::AirCheckCompleted;
    #[cfg(feature = "i")]
    topic AirCheckCompleted = "ieee802154/air-check/completed";
    #[cfg(feature = "i")]
    endpoint StartSession = "ieee802154/session/start" => crate::ieee802154::SessionStarted;
    #[cfg(feature = "i")]
    topic SessionStarted = "ieee802154/session/started";
    #[cfg(feature = "i")]
    endpoint TransmitSession = "ieee802154/session/transmit" => crate::ieee802154::SessionTransmitted;
    #[cfg(feature = "i")]
    topic SessionTransmitted = "ieee802154/session/transmitted";
    #[cfg(feature = "i")]
    endpoint ReceiveSession = "ieee802154/session/receive" => crate::base::Accepted;
    #[cfg(feature = "i")]
    endpoint CollectSession = "ieee802154/session/collect" => crate::ieee802154::SessionReceived;
    #[cfg(feature = "i")]
    topic SessionReceived = "ieee802154/session/received";
    #[cfg(feature = "i")]
    endpoint SetSessionPending = "ieee802154/session/pending/set" => crate::base::Accepted;
    #[cfg(feature = "i")]
    endpoint StopSession = "ieee802154/session/stop" => crate::ieee802154::SessionStopped;
    #[cfg(feature = "i")]
    topic SessionStopped = "ieee802154/session/stopped";
    #[cfg(feature = "i")]
    endpoint MaintainSessionPhy = "ieee802154/session/phy/maintain" => crate::ieee802154::SessionPhyMaintained;
    #[cfg(feature = "i")]
    topic SessionPhyMaintained = "ieee802154/session/phy/maintained";
    #[cfg(feature = "i")]
    endpoint AssessSessionChannel = "ieee802154/session/channel/assess" => crate::ieee802154::SessionAssessed;
    #[cfg(feature = "i")]
    topic SessionAssessed = "ieee802154/session/channel/assessed";
    #[cfg(feature = "i")]
    endpoint RestartSessionRadio = "ieee802154/session/radio/restart" => crate::ieee802154::SessionRadioRestarted;
    #[cfg(feature = "i")]
    topic SessionRadioRestarted = "ieee802154/session/radio/restarted";
    #[cfg(feature = "i")]
    endpoint ReadSessionRecentRssi = "ieee802154/session/recent-rssi/read" => crate::ieee802154::SessionRecentRssi;
    #[cfg(feature = "i")]
    topic SessionRecentRssi = "ieee802154/session/recent-rssi";
    #[cfg(feature = "i")]
    endpoint StartThread = "ieee802154/thread/start" => crate::ieee802154::ThreadStarted;
    #[cfg(feature = "i")]
    topic ThreadStarted = "ieee802154/thread/started";
    #[cfg(feature = "i")]
    endpoint GetThread = "ieee802154/thread/get" => crate::ieee802154::ThreadState;
    #[cfg(feature = "i")]
    topic ThreadState = "ieee802154/thread/state";
    #[cfg(feature = "i")]
    endpoint SendThread = "ieee802154/thread/send" => crate::ieee802154::ThreadSent;
    #[cfg(feature = "i")]
    topic ThreadSent = "ieee802154/thread/sent";
    #[cfg(feature = "i")]
    endpoint CollectThread = "ieee802154/thread/collect" => crate::ieee802154::ThreadReceived;
    #[cfg(feature = "i")]
    topic ThreadReceived = "ieee802154/thread/received";
    #[cfg(feature = "i")]
    endpoint StopThread = "ieee802154/thread/stop" => crate::ieee802154::ThreadStopped;
    #[cfg(feature = "i")]
    topic ThreadStopped = "ieee802154/thread/stopped";
}

/// Run one bounded, observation-only IEEE 802.15.4 `EVENT_STATUS` probe.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProbeEventStatus(pub crate::ieee802154::Ieee802154EventStatusProbeRequest);

/// Correlated observation from its request.
///
/// This event does not attest to same-bit concurrency, level-triggered
/// retrigger behavior, or production interrupt readiness.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EventStatusProbed(pub crate::ieee802154::Ieee802154EventStatusProbeEvidence);

/// Run the IEEE 802.15.4 same-bit and level-retrigger route probe.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProbeRoute(pub crate::ieee802154::Ieee802154RouteProbeRequest);

/// Correlated observation from its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RouteProbed(pub crate::ieee802154::Ieee802154RouteProbeEvidence);

/// Run the bounded ED-DONE/TIMER0 selective-write discriminator.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ProbeEdEvent(pub crate::ieee802154::Ieee802154EdEventProbeRequest);

/// Correlated observation from its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct EdEventProbed(pub crate::ieee802154::Ieee802154EdEventProbeEvidence);

/// Run the single-device IEEE 802.15.4 on-air check.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunAirCheck(pub crate::ieee802154::Ieee802154AirCheckRequest);

/// Correlated observation from its request.
///
/// It records single-device outcomes only; it does not attest to a peer
/// receiving the transmitted frames or to calibrated output power.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AirCheckCompleted(pub crate::ieee802154::Ieee802154AirCheckEvidence);

/// Start an IEEE 802.15.4 peer session with this identity and filter.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartSession(pub crate::ieee802154::Ieee802154SessionConfig);

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionStarted(pub crate::ieee802154::Ieee802154SessionResult);

/// Transmit one frame in the running session.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct TransmitSession(pub crate::ieee802154::Ieee802154SessionTransmitRequest);

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionTransmitted(pub crate::ieee802154::Ieee802154SessionTransmitEvidence);

/// Enter receive mode; frames accumulate until collected.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReceiveSession;

/// Return and forget the frames received since the last collection.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectSession;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionReceived(pub crate::ieee802154::Ieee802154SessionReceiveEvidence);

/// Change the automatic frame-pending decision.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SetSessionPending(pub crate::ieee802154::Ieee802154SessionPendingRequest);

/// Stop the running session.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopSession;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionStopped(pub crate::ieee802154::Ieee802154SessionStopEvidence);

/// Run shared PHY tracking now if it is due.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MaintainSessionPhy;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionPhyMaintained(pub crate::ieee802154::Ieee802154SessionPhyMaintenance);

/// Measure the energy on one channel and assess it once.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct AssessSessionChannel(pub crate::ieee802154::Ieee802154SessionAssessRequest);

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionAssessed(pub crate::ieee802154::Ieee802154SessionAssessment);

/// Stop the session's client and start it again, as between the air
/// check's cycles, then apply the session configuration and receive.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RestartSessionRadio;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionRadioRestarted(pub crate::ieee802154::Ieee802154SessionRestartEvidence);

/// Read the live RSSI of the most recent baseband reception
/// (`esp_i_get_recent_rssi`).
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ReadSessionRecentRssi;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SessionRecentRssi(pub crate::ieee802154::Ieee802154SessionRecentRssi);

/// Start OpenThread over the composed client and join the dataset's
/// network.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StartThread(pub crate::ieee802154::Ieee802154ThreadStartRequest);

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ThreadStarted(pub crate::ieee802154::Ieee802154SessionResult);

/// Report the device's Thread interface.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetThread;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ThreadState(pub crate::ieee802154::Ieee802154ThreadState);

/// Send one datagram from the device's socket.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SendThread(pub crate::ieee802154::Ieee802154ThreadSendRequest);

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ThreadSent(pub crate::ieee802154::Ieee802154SessionResult);

/// Return and forget the datagrams received since the last collection.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CollectThread;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ThreadReceived(pub crate::ieee802154::Ieee802154ThreadReceiveEvidence);

/// Leave the network and stop the client.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StopThread;

/// Correlated result of its request.
#[cfg(feature = "i")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ThreadStopped(pub crate::ieee802154::Ieee802154SessionResult);
