//! Reviewed decisions, reviewed through git: production lines no compared
//! observation depends on, and vendor state no relation compares. Vendor
//! locations the comparisons do not reach are reviewed in
//! `verification/esp32c5/decisions/coverage.toml`, outside this crate, so a
//! shard depends only on the decisions that apply to it. Each engine module
//! owns its decision type and fails a run on a stale decision.
pub mod observation;
pub mod state;
