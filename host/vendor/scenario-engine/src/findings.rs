//! What a scenario run leaves for a reviewer beside its verdicts.
//!
//! The engine decides verdicts and the recorded sets of a shard; reviewer
//! aids, such as the untriaged-location report or the closure gateways, are
//! rendered by a [`RunReport`] the scenario binary passes in. Report code
//! therefore depends on the engine and never the other way, and editing it
//! leaves the evidence shards current.
use crate::coverage::Decision;
use crate::harness::Result;
use oer_vendor_evidence_shard::Location;
use std::collections::BTreeSet;
use std::path::Path;

/// One claimed suite's findings, handed to the report at the end of
/// [`crate::session::Session::claims`].
pub struct Findings<'a> {
    pub suite: &'a str,
    /// The run directory reports are written into.
    pub directory: &'a Path,
    /// Compared (vendor, production) pairs no claim names.
    pub unclaimed: &'a BTreeSet<(String, String)>,
    /// Functions through which a closure reaches code no case executed.
    pub gateways: &'a [String],
    /// Present when the suite left locations no decision triaged.
    pub untriaged: Option<Untriaged<'a>>,
}

/// The locations a suite left untriaged and what a reviewer needs to read
/// them.
pub struct Untriaged<'a> {
    /// The linked production image and the vendor ROM.
    pub images: [&'a [u8]; 2],
    pub decisions: &'a [Decision],
    /// Untriaged locations no closure reached: those the evidence lists.
    pub listed: &'a BTreeSet<Location>,
    pub uncovered: &'a BTreeSet<Location>,
    /// Uncovered locations whose effects reach a compared result.
    pub consequential: &'a BTreeSet<Location>,
}

/// Renders a suite's findings for a reviewer; suites of one run report
/// from their own threads.
pub trait RunReport: Sync {
    fn report(&self, findings: &Findings<'_>) -> Result<()>;
}
