//! The run documents: the manifest, plan, suite, scenario and repetition
//! results with their measurements, cleanup and USB records, the typed
//! fixture records ([`fixtures`]), file listings ([`files`]), and
//! the structural validation every writer and
//! reader applies.

pub mod files;
pub mod fixtures;
mod model;
mod records;
pub mod validation;

pub use files::{collect_attachments, inventory};
pub use model::*;
pub use records::*;
