//! The run documents: the manifest, plan, suite, scenario and repetition
//! results with their measurements, cleanup and USB records, the integrity
//! seal and scenario seals, the typed fixture records ([`fixtures`]), and
//! the structural validation every writer and
//! reader applies.

mod attempt;
pub mod fixtures;
pub mod integrity;
mod model;
mod records;
pub mod validation;

pub use attempt::{ATTEMPT_SEAL_SCHEMA, ATTEMPTS, Attempt, AttemptSeal, completed, material_files};
pub use integrity::{
    INTEGRITY, collect_attachments, collect_integrity_files, write_integrity_index,
};
pub use model::*;
pub use records::*;
