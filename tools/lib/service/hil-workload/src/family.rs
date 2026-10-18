//! The radio families a runner composes and the registry it dispatches
//! through.
//!
//! A family implements [`Workload`] for its scenario table and names it with
//! a [`Kind`]; the composing binary lists its kinds and fixture providers in
//! a [`Registry`]. The runner core holds every scenario's family as an
//! [`AnyFamily`] of that registry and calls the family through it: it never
//! matches on, or names, a family.

use std::{fmt, marker::PhantomData, path::Path, sync::Arc};

use oer_hil_lab::config::LabConfig;
use oer_hil_protocol::DeviceImageKeys;
use oer_hil_scenario::{AirUse, PeerImage, Plan, ScenarioFamily};
use oer_hil_scenario_catalog::requirements::Requirements;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::{Result, context::Context, fixture::Fixtures};

/// A radio family's scenario table, runnable.
pub trait Workload: ScenarioFamily + Send + Sync + 'static {
    /// What the laboratory must offer before the scenario may run, beyond
    /// its requirements: a failure blocks the scenario as a precondition.
    fn precondition(&self, _lab: &LabConfig) -> Result<()> {
        Ok(())
    }

    /// Run one repetition into `output`, with the fixtures the registry's
    /// providers prepared for it.
    fn run(&self, output: &Path, context: &Context<'_>, fixtures: &Fixtures) -> Result<()>;
}

/// A family's key in a scenario document and how to read its table.
pub struct Kind {
    pub key: &'static str,
    parse: fn(serde_json::Value) -> Result<Arc<dyn Erased>>,
}

impl Kind {
    /// The family of tables `T`, under `key`.
    pub const fn of<T: Workload>(key: &'static str) -> Self {
        Self {
            key,
            parse: parse_as::<T>,
        }
    }
}

fn parse_as<T: Workload>(value: serde_json::Value) -> Result<Arc<dyn Erased>> {
    Ok(Arc::new(serde_json::from_value::<T>(value)?))
}

/// The laboratory fixtures a requirement calls for, prepared for every
/// scenario whose plan requires them, whatever its family.
pub trait FixtureProvider: Sync {
    /// Before anything of a run is built: whether the host has the fixture
    /// software the `required` services of a whole selection need.
    fn admit(&self, _lab: &LabConfig, _required: Requirements) -> Result<()> {
        Ok(())
    }

    /// Whether the laboratory satisfies `plan`'s requirements; a failure
    /// blocks the scenario as a precondition.
    fn precondition(&self, _lab: &LabConfig, _plan: &Plan) -> Result<()> {
        Ok(())
    }

    /// Check, without preparing them, that the fixtures `plan` requires
    /// are reachable and fit.
    fn check(&self, lab: &LabConfig, plan: &Plan) -> Result<()>;

    /// Prepare the fixtures `plan` requires for one repetition, insert them
    /// into `fixtures` and record their evidence in `output`.
    fn prepare(
        &self,
        lab: &LabConfig,
        plan: &Plan,
        output: &Path,
        fixtures: &mut Fixtures,
    ) -> Result<()>;

    /// Prepare and restore the fixtures `plan` requires without the board
    /// under test, recording into `output`: `cargo hil fixture check`.
    fn exercise(&self, _lab: &LabConfig, _plan: &Plan, _output: &Path) -> Result<()> {
        Ok(())
    }
}

/// The families and fixture providers a runner composes.
pub trait Registry: 'static {
    const FAMILIES: &'static [Kind];
    const FIXTURES: &'static [&'static dyn FixtureProvider];
}

/// A family table behind the registry: a [`Workload`] without its type.
pub trait Erased: fmt::Debug + Send + Sync {
    fn validate(&self) -> Result<()>;
    fn plan(&self) -> Plan;
    fn served_by(&self, capabilities: &DeviceImageKeys) -> bool;
    fn peer_image(&self) -> Option<PeerImage>;
    fn air_use(&self) -> Vec<AirUse>;
    fn precondition(&self, lab: &LabConfig) -> Result<()>;
    fn run(&self, output: &Path, context: &Context<'_>, fixtures: &Fixtures) -> Result<()>;
    fn value(&self) -> serde_json::Value;
}

impl<T: Workload> Erased for T {
    fn validate(&self) -> Result<()> {
        ScenarioFamily::validate(self)
    }
    fn plan(&self) -> Plan {
        ScenarioFamily::plan(self)
    }
    fn served_by(&self, capabilities: &DeviceImageKeys) -> bool {
        ScenarioFamily::served_by(self, capabilities)
    }
    fn peer_image(&self) -> Option<PeerImage> {
        ScenarioFamily::peer_image(self)
    }
    fn air_use(&self) -> Vec<AirUse> {
        ScenarioFamily::air_use(self)
    }
    fn precondition(&self, lab: &LabConfig) -> Result<()> {
        Workload::precondition(self, lab)
    }
    fn run(&self, output: &Path, context: &Context<'_>, fixtures: &Fixtures) -> Result<()> {
        Workload::run(self, output, context, fixtures)
    }
    fn value(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }
}

/// The family table of a scenario document, of any family registry `R`
/// composes: the document's one family key and its typed table.
pub struct AnyFamily<R> {
    key: &'static str,
    table: Arc<dyn Erased>,
    registry: PhantomData<fn() -> R>,
}

impl<R: Registry> AnyFamily<R> {
    /// The family's key in the document.
    pub fn key(&self) -> &'static str {
        self.key
    }

    /// See [`Workload::precondition`].
    pub fn precondition(&self, lab: &LabConfig) -> Result<()> {
        self.table.precondition(lab)
    }

    /// See [`Workload::run`].
    pub fn run(&self, output: &Path, context: &Context<'_>, fixtures: &Fixtures) -> Result<()> {
        self.table.run(output, context, fixtures)
    }

    /// The family's table as its document value.
    pub fn table(&self) -> serde_json::Value {
        self.table.value()
    }
}

impl<R> Clone for AnyFamily<R> {
    fn clone(&self) -> Self {
        Self {
            key: self.key,
            table: Arc::clone(&self.table),
            registry: PhantomData,
        }
    }
}

impl<R> fmt::Debug for AnyFamily<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AnyFamily")
            .field("key", &self.key)
            .field("table", &self.table)
            .finish()
    }
}

impl<R> PartialEq for AnyFamily<R> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key && self.table.value() == other.table.value()
    }
}

impl<R> Eq for AnyFamily<R> {}

impl<R> Serialize for AnyFamily<R> {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(Some(1))?;
        map.serialize_entry(self.key, &self.table.value())?;
        map.end()
    }
}

impl<'de, R: Registry> Deserialize<'de> for AnyFamily<R> {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::Error;
        let document = serde_json::Map::<String, serde_json::Value>::deserialize(deserializer)?;
        let mut entries = document.into_iter();
        let (Some((key, table)), None) = (entries.next(), entries.next()) else {
            return Err(D::Error::custom(format!(
                "a scenario names exactly one family table: {}",
                R::FAMILIES
                    .iter()
                    .map(|kind| kind.key)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        };
        let kind = R::FAMILIES
            .iter()
            .find(|kind| kind.key == key)
            .ok_or_else(|| D::Error::unknown_variant(&key, &[]))?;
        let table = (kind.parse)(table).map_err(D::Error::custom)?;
        Ok(Self {
            key: kind.key,
            table,
            registry: PhantomData,
        })
    }
}

impl<R: Registry> ScenarioFamily for AnyFamily<R> {
    fn validate(&self) -> Result<()> {
        self.table.validate()
    }
    fn plan(&self) -> Plan {
        self.table.plan()
    }
    fn served_by(&self, capabilities: &DeviceImageKeys) -> bool {
        self.table.served_by(capabilities)
    }
    fn peer_image(&self) -> Option<PeerImage> {
        self.table.peer_image()
    }
    fn air_use(&self) -> Vec<AirUse> {
        self.table.air_use()
    }
}

/// Read a family table of type `T` from its document value; for tests of a
/// family and of its registry.
pub fn table_of<T: DeserializeOwned>(family: &impl Serialize) -> Result<T> {
    let serde_json::Value::Object(document) = serde_json::to_value(family)? else {
        return Err("a family serializes as a table".into());
    };
    let (_, table) = document
        .into_iter()
        .next()
        .ok_or("a family has one table")?;
    Ok(serde_json::from_value(table)?)
}

#[cfg(test)]
mod tests;
