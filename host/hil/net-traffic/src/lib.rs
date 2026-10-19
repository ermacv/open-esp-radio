//! Network traffic of HIL workloads: the target's traffic sessions over the
//! link ([`NetworkSession`]: configure, arm, start, typed evidence, replay
//! and acknowledgement), the readiness of its network services, and the
//! host side of the traffic: paced UDP and TCP generation and reception
//! ([`paced_udp`], [`paced_tcp`]) and the offered load of several flows
//! ([`offered_load`]), UDP sockets sized for qualification ([`udp`]) and ICMP
//! echo through datagram sockets ([`icmp`]), with the one nearest-rank
//! percentile ([`icmp::nearest_rank`]).
//!
//! Every station, access-point and coexistence workload measures its
//! traffic through this crate; it depends on the link and the protocol, never
//! on a radio family.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

pub mod icmp;
pub mod offered_load;
pub mod paced_tcp;
pub mod paced_udp;
mod readiness;
mod session;
pub mod udp;
mod validation;

pub use oer_hil_link::Result;
pub use readiness::{
    UdpRxReady, prepare_udp_reverse_flow, probe_udp_rx_ready, probe_udp_rx_ready_via,
};
pub use session::{NetworkSession, SESSION_START_TIMEOUT, SessionEvidence, SessionHandle};
pub use validation::validate_stack_usage;

#[cfg(test)]
mod tests;
