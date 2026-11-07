//! Role-neutral physical RX frontier and storage contracts.
//!
//! This owner composes the typed DMA ring without an executor. Staged
//! leases reach the application through the queue of [`upper`]; Embassy
//! timer implementations stay in the adapter; their lifetimes never
//! manufacture another ring owner.

pub mod arena;
pub mod control;
pub mod counters;
pub mod frontier;
pub mod receive;
pub mod returns;
pub mod sink;
pub mod storage;
pub mod upper;

#[cfg(test)]
pub(crate) mod test_ring;

pub mod transaction;

/// A receive epoch that returns stopped register/IRQ ownership for retune.
pub mod session;
