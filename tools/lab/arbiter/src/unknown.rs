//! Fields of a stored record that this build does not know.
//!
//! Every checkout of the host user shares the arbiter's files, and a checkout
//! built before a field existed still rewrites records that carry it. Each
//! stored record flattens an [`Unknown`] so such a rewrite keeps the newer
//! fields instead of dropping them.

use serde::{Deserialize, Serialize};

/// The fields of a record this build does not know, written back unchanged.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(transparent)]
pub struct Unknown(serde_json::Map<String, serde_json::Value>);
