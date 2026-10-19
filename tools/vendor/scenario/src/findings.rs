//! What a scenario run leaves for a reviewer beside its verdicts.
//!
//! The engine hands compared pairs with no claim to the binary's report.
use crate::harness::Result;
use std::collections::BTreeSet;

/// One claimed suite's findings, handed to the report at the end of
/// [`crate::session::Session::claims`].
pub struct Findings<'a> {
    pub suite: &'a str,
    /// Compared (vendor, production) pairs no claim names.
    pub unclaimed: &'a BTreeSet<(String, String)>,
}

/// Renders a suite's findings for a reviewer; suites of one run report
/// from their own threads.
pub trait RunReport: Sync {
    fn report(&self, findings: &Findings<'_>) -> Result<()>;
}
