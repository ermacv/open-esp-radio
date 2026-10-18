//! The HIL observer's build identity, as data a run records.
//!
//! The runner is the observer of every HIL run. Its identity is computed
//! from the parts defined here: the projection of the runner's resolved
//! dependency graph onto the inputs each workload uses ([`inputs`],
//! [`cargo_inputs`]), Cargo's actual compilation units bound to the executable ([`artifacts`]), the content
//! store a run's observer record refers to ([`store`]) and the receipt an
//! invocation of the runner carries ([`receipt`]). Building the runner,
//! resolving its graph and preparing its receipt is `oer-tool-service-hil-observer`'s;
//! evaluators read the prepared descriptors here.

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

pub mod artifacts;
pub mod cargo_inputs;
pub mod inputs;
pub mod receipt;
pub mod store;
