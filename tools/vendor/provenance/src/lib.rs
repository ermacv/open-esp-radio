//! Vendor function provenance. A recovered fact cites the vendor function
//! it came from in a `SOURCE` block (`crate::markers::source`, the one recogniser
//! of that grammar, which tidy uses too), and the citation is registered with
//! that function's [`fingerprint`] in the chip's provenance registry
//! ([`registry`]): its check fails when a pinned artifact's function no
//! longer matches its registered fingerprint. [`diff`] compares two
//! revisions of an archive function by function, and `oer-vendor-lineage`
//! pairs functions across revisions by the same fingerprints.

#![forbid(unsafe_code)]

pub mod markers;

pub mod diff;
mod files;
pub mod fingerprint;
pub mod registry;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
