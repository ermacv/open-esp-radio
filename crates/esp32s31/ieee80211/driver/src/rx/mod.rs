//! Role-neutral physical RX frontier and storage contracts.
//!
//! This owner composes the typed DMA ring without an executor or an upper
//! queue. Embassy timer implementations and staged protocol publication stay
//! in the adapter; their lifetimes never manufacture another ring owner.

pub mod arena;
pub mod control;
pub mod counters;
pub mod frontier;
pub mod receive;
pub mod returns;
pub mod sink;
pub mod storage;

pub mod transaction;
