//! Recover vendor function names across obfuscated archive revisions.
//!
//! An early vendor archive can carry source function names that later
//! revisions replace with generated tokens. [`lineage::trace`] pairs the
//! functions of every adjacent revision pair ([`correspond`]) and carries each
//! source name forward to the final revision, retaining the evidence of every
//! step. A function without a complete chain keeps no recovered name.

#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

pub mod archive;
pub mod correspond;
pub mod lineage;
