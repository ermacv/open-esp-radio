//! Family-agnostic HIL scenario envelope, catalog and campaign plan, with
//! the laboratory requirements and target settings a scenario declares.
//!
//! A scenario document is a common header plus exactly one radio-family
//! table. The family owns its typed workload, its validation and the
//! [`Plan`] projection every family-independent consumer reads: the firmware
//! image, laboratory requirements, target initialization and named checks.

use std::{
    fmt::Debug,
    path::{Path, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

use oer_tool_data_hil_scenario_catalog::link::WifiLabUse;
use oer_tool_data_hil_scenario_catalog::{
    HEADER_FIELDS, Header, ProfileHarts, ProfileRequest, Role, requirements::Requirements,
};
use oer_tool_data_hil_schema::image::ImageClass;

pub mod campaign;
pub mod catalog;
pub mod identity;

pub use catalog::Catalog;

/// The protocol's arming command of a scenario's profile request.
pub fn profile_control(
    request: ProfileRequest,
) -> oer_tool_data_hil_protocol::telemetry::ProfileControl {
    oer_tool_data_hil_protocol::telemetry::ProfileControl::Arm {
        harts: match request.harts {
            ProfileHarts::Both => oer_tool_data_hil_protocol::telemetry::ProfileHarts::Both,
            ProfileHarts::Core0 => oer_tool_data_hil_protocol::telemetry::ProfileHarts::Core0,
            ProfileHarts::Core1 => oer_tool_data_hil_protocol::telemetry::ProfileHarts::Core1,
        },
        period_us: request.period_us,
    }
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// What family-independent execution, planning and evidence need to know.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub image: ImageClass,
    pub requirements: Requirements,
    /// Named observations the workload publishes. Their acceptance limits
    /// remain in the family's criteria; listing a check is not a verdict.
    pub checks: Vec<&'static str>,
    pub wifi: WifiLabUse,
}

impl Plan {
    /// A plan for a scenario that uses no laboratory service beyond the
    /// target's serial port and firmware.
    pub fn target_only(image: ImageClass) -> Self {
        Self {
            image,
            requirements: Requirements::default(),
            checks: Vec::new(),
            wifi: WifiLabUse::default(),
        }
    }
}

/// A catalog image the reference peer board must carry for a scenario.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PeerImage {
    /// The board-journal and catalog image name.
    pub name: &'static str,
    /// How to restore it when another consumer replaced it.
    pub reflash: &'static str,
}

/// A frequency range a scenario's radio work occupies, as the family states
/// it; the runner turns it into the claims of its stand lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AirUse {
    /// The 2.4 GHz band: links that hop across it (Bluetooth LE) and radio
    /// work without one fixed channel.
    Band2G4,
    /// One IEEE 802.15.4 channel.
    Ieee802154Channel(u8),
    /// The channel of the scenario's Wi-Fi link, as the laboratory sets it
    /// up for the scenario's [`WifiLabUse`].
    WifiLink,
}

/// The typed family content of a scenario document: what every
/// family-independent consumer (planning, selection, the stand lease, the
/// runner core and evidence) reads.
///
/// [`Scenario`] deserializes it from the document without its header: the
/// single family key and its table. A radio family implements it for its
/// table's type; the runner's family registry (`oer-tool-service-hil-workload`) wraps the
/// families it composes into one document-level type, so the runner core
/// never names a family.
pub trait ScenarioFamily: Clone + Debug + Eq + Serialize + DeserializeOwned {
    /// Validate value ranges and relations inside the family table.
    fn validate(&self) -> Result<()>;

    fn plan(&self) -> Plan;

    /// The laboratory services the scenario needs.
    fn requirements(&self) -> Requirements {
        self.plan().requirements
    }

    /// Whether an image of the planned class that reports `capabilities` can
    /// run this scenario. The class alone suffices unless the family names
    /// a role the class's image may omit.
    fn served_by(&self, _capabilities: &oer_tool_data_hil_protocol::DeviceImageKeys) -> bool {
        true
    }

    /// The catalog image the reference peer board must carry, when the
    /// scenario uses the peer.
    fn peer_image(&self) -> Option<PeerImage> {
        None
    }

    /// The frequency ranges the scenario's radio work occupies; none for
    /// work that never enables a radio.
    fn air_use(&self) -> Vec<AirUse>;
}

/// One validated scenario document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scenario<F> {
    pub header: Header,
    pub family: F,
    pub(crate) source: PathBuf,
}

impl<F: ScenarioFamily> Scenario<F> {
    /// Parse and validate one TOML document.
    pub fn from_toml(text: &str, source: &Path) -> Result<Self> {
        let document: toml::Table =
            toml::from_str(text).map_err(|error| format!("{}: {error}", source.display()))?;
        let value = serde_json::to_value(document)?;
        let mut scenario =
            Self::from_value(value).map_err(|error| format!("{}: {error}", source.display()))?;
        scenario.source = source.to_owned();
        scenario
            .validate()
            .map_err(|error| format!("{}: {error}", source.display()))?;
        Ok(scenario)
    }

    /// Read a recorded scenario snapshot.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let scenario = Self::from_value(serde_json::from_slice(bytes)?)?;
        scenario.validate()?;
        Ok(scenario)
    }

    pub(crate) fn from_value(value: serde_json::Value) -> Result<Self> {
        let serde_json::Value::Object(mut document) = value else {
            return Err("scenario document is not a table".into());
        };
        let mut header = serde_json::Map::new();
        for key in HEADER_FIELDS {
            if let Some(value) = document.remove(key) {
                header.insert(key.to_owned(), value);
            }
        }
        Ok(Self {
            header: serde_json::from_value(serde_json::Value::Object(header))?,
            family: serde_json::from_value(serde_json::Value::Object(document))?,
            source: PathBuf::new(),
        })
    }

    pub fn validate(&self) -> Result<()> {
        self.header.validate()?;
        self.family.validate()?;
        // The program-counter sampler was deleted (); no class serves a profile.
        if self.header.profile.is_some() {
            return Err(format!(
                "scenario {} requests a profile, but its image {} does not sample the program \
                 counter",
                self.header.id,
                self.image().id()
            )
            .into());
        }
        // A diagnostic image carries observers and probes that the product
        // does not: its observations never qualify a product, and never
        // shape a gated performance figure.
        if self.image().diagnostic()
            && (self.header.role == Role::Qualification
                || self.header.tags.iter().any(|tag| tag == "performance"))
        {
            return Err(format!(
                "scenario {} runs on the diagnostic image {}; only an investigation scenario \
                 without the performance tag may",
                self.header.id,
                self.image().id()
            )
            .into());
        }
        Ok(())
    }

    pub fn id(&self) -> &str {
        &self.header.id
    }

    pub fn repetitions(&self) -> u8 {
        self.header.repetitions
    }

    pub fn plan(&self) -> Plan {
        self.family.plan()
    }

    pub fn image(&self) -> ImageClass {
        self.family.plan().image
    }

    pub fn requirements(&self) -> Requirements {
        self.family.requirements()
    }

    pub fn source(&self) -> &Path {
        &self.source
    }
}

/// Scenarios serialize as the document they were read from: the header
/// fields beside the single family table.
impl<F: ScenarioFamily> Serialize for Scenario<F> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let (serde_json::Value::Object(mut document), serde_json::Value::Object(family)) = (
            serde_json::to_value(&self.header).map_err(S::Error::custom)?,
            serde_json::to_value(&self.family).map_err(S::Error::custom)?,
        ) else {
            return Err(S::Error::custom("scenario parts must serialize as tables"));
        };
        document.extend(family);
        document.serialize(serializer)
    }
}

// A minimal family for the tests of this crate and of the runner core.
#[cfg(any(test, feature = "test-support"))]
pub mod test_family;
