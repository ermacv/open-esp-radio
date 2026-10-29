//! Messages of a test image's own services, as an image's dictionary
//! declares them.

use oer_hil_protocol::{Endpoint, Message, WireKind};
use serde::{Deserialize, Serialize};

/// How a probe request should behave.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
pub enum ProbeMode {
    Complete,
    Stall,
}

/// A test endpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
pub struct Probe(pub ProbeMode);
impl Message for Probe {
    const PATH: &'static str = "test/probe";
    const WIRE_KIND: WireKind = WireKind::Command;
}
impl Endpoint for Probe {
    type Response = ProbeArmed;
}

/// Its response.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
pub struct ProbeArmed(pub ProbeMode);
impl Message for ProbeArmed {
    const PATH: &'static str = "test/probe/armed";
}

/// A second test endpoint, never mounted.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
pub struct Switch;
impl Message for Switch {
    const PATH: &'static str = "test/switch";
    const WIRE_KIND: WireKind = WireKind::Command;
}
impl Endpoint for Switch {
    type Response = Switched;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
pub struct Switched;
impl Message for Switched {
    const PATH: &'static str = "test/switched";
}

/// A test image property.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, postcard_schema::Schema)]
pub struct Marker;
impl Message for Marker {
    const PATH: &'static str = "test/marker";
}
