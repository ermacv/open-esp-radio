//! What a run measures besides its trace. The runner converts these into
//! the run record's format, which adds thresholds and verdicts; the model
//! knows neither.

use serde::{Deserialize, Serialize};

use crate::MeasurementName;

/// A measurement's unit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    Count,
    Bytes,
    BitsPerSecond,
    Microseconds,
    BasisPoints,
}

/// One measured value.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct MeasuredValue {
    pub name: MeasurementName,
    pub value: u64,
    pub unit: Unit,
}
