//! Station admission and link state, separate from network transport sessions.

#[cfg(feature = "wifi")]
mod credentials;
#[cfg(feature = "wifi")]
mod link;
#[cfg(feature = "wifi")]
mod observation;
#[cfg(feature = "wifi")]
pub use credentials::*;
#[cfg(feature = "wifi")]
pub use link::*;
#[cfg(feature = "wifi")]
pub use observation::*;

crate::messages! {
    property Station = "wifi/station";
    /// Accept a station configuration; authorization is a later link event.
    #[cfg(feature = "wifi")]
    endpoint StartStation = "wifi/station/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationLink = "wifi/station/link";
    #[cfg(feature = "wifi")]
    endpoint ProbeStation = "wifi/station/probe" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationProbeFinished = "wifi/station/probe/finished";
    #[cfg(feature = "wifi")]
    topic StationProbeReady = "wifi/station/probe/ready";
    #[cfg(feature = "wifi")]
    endpoint GetStationCounters = "wifi/station/counters/get" => StationCounters;
    #[cfg(feature = "wifi")]
    topic StationCounters = "wifi/station/counters";
    #[cfg(feature = "wifi")]
    topic StationGroupKey = "wifi/station/group-key";
    #[cfg(feature = "wifi")]
    topic StationArpFrame = "wifi/station/arp-frame";
    #[cfg(feature = "wifi")]
    endpoint StopStation = "wifi/station/stop" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationStopped = "wifi/station/stopped";
}

#[cfg(feature = "wifi")]
#[derive(Debug, serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
pub struct StartStation(pub StationStart);

#[cfg(all(test, feature = "wifi"))]
mod tests;
