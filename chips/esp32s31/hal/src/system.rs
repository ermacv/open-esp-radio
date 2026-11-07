//! The chip below every peripheral: the reset root and its owners, the
//! harts, the platform port, the DMA memory core and the boot parts.

pub mod boot;
pub mod cores;
pub mod memory;
pub mod owner;
pub mod platform;
pub mod root;
