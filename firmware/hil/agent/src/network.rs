//! HIL network composition pieces shared by the runtime's stack variants.
pub mod progress;

#[cfg(feature = "owned-network")]
pub mod embassy_ipv4;
#[cfg(feature = "owned-network")]
pub mod sockets;
