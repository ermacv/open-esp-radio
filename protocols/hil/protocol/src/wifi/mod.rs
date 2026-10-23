//! Station admission and link state, separate from network transport sessions.

#[cfg(feature = "wifi")]
mod credentials;
#[cfg(feature = "wifi")]
mod link;
#[cfg(feature = "wifi")]
pub use credentials::*;
#[cfg(feature = "wifi")]
pub use link::*;

crate::messages! {
    property Station = "wifi/station";
    /// Accept a station configuration; authorization is a later link event.
    #[cfg(feature = "wifi")]
    endpoint StartStation = "wifi/station/start" => crate::base::Accepted;
    #[cfg(feature = "wifi")]
    topic StationLink = "wifi/station/link";
}

#[cfg(feature = "wifi")]
#[derive(Debug, serde::Serialize, serde::Deserialize, postcard_schema::Schema)]
pub struct StartStation(pub StationStart);

#[cfg(all(test, feature = "wifi"))]
mod tests;
