//! The HIL observer's build identity, as data a run records.
//!
//! The runner is the observer of every HIL run. Its identity is computed
//! from the parts defined here: the projection of the runner's resolved
//! dependency graph onto the inputs each workload uses ([`inputs`],
//! [`cargo_inputs`]) and the content store a run's observer record refers to
//! ([`store`]). Building the runner and resolving its graph is
//! `oer-hil-observer`'s.

/// Version of an observer build (`build.schema`).
///
/// Schema 3 records the runner's resolved Cargo graph as Cargo's graph:
/// `resolved.packages` holds each of Cargo's nodes (a package with its
/// features) once, the runner first, with the indices of the nodes it
/// depends on in `edges`. A package that is both a normal and a build
/// dependency with different features is two nodes; a reader merges a
/// package's nodes ([`inputs::projection`]). Schema 2 recorded the
/// graph as a tree with a node per path to a package, hundreds of megabytes;
/// no reader for it remains, so an observation recorded with it is not this
/// observer's.
pub const BUILD_SCHEMA: u64 = 3;

pub mod cargo_inputs;
pub mod inputs;
pub mod store;
