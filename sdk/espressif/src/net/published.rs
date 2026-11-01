//! The IP service's status as its one writer publishes it for one reader in
//! another task: whole, under a sequence number that is odd while a write
//! runs, in atomic words only. A read that sees the number change was
//! concurrent with a write and is refused; the writer tells the reader's
//! notify after each publication, so a reader that registers first and
//! then reads never misses one.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use oer_embassy_net::ip::{Ipv4Configuration, Ipv4Status, Lifecycle, Status};
use oer_network_port::{LinkSnapshot, LinkState};

use super::ServiceError;

/// A read ran while the writer wrote: its words are no publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Concurrent;

/// One published [`Status`].
pub struct Published {
    sequence: AtomicU32,
    words: [AtomicU32; WORDS],
}

const WORDS: usize = 6;

// Word 3, the shape: bit 0 the link is up; bits 1 and 2 the IPv4 status
// (0 unconfigured, 1 lease, 2 static); bit 3 its gateway; bits 8 to 15 its
// prefix; bits 16 to 23 the lifecycle (0 running, 1 stopped, 2 failed);
// bits 24 to 31 the failure's code. Word 4 the address, word 5 the gateway;
// words 0 and 1 the link's epoch, low first; word 2 the failure's detail.
const UP: u32 = 1;
const LEASE: u32 = 1 << 1;
const STATIC: u32 = 2 << 1;
const KIND: u32 = 3 << 1;
const GATEWAY: u32 = 1 << 3;
const PREFIX_SHIFT: u32 = 8;
const LIFECYCLE_SHIFT: u32 = 16;
const STOPPED: u32 = 1;
const FAILED: u32 = 2;
const ERROR_SHIFT: u32 = 24;

impl Published {
    /// Nothing published yet: the link down, no address, the service
    /// running.
    pub const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            words: [const { AtomicU32::new(0) }; WORDS],
        }
    }

    /// Publish `status`; only the one writer calls it.
    pub fn publish(&self, status: Status) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        for (word, value) in self.words.iter().zip(encode(status)) {
            word.store(value, Ordering::Relaxed);
        }
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The published status.
    ///
    /// # Errors
    /// [`Concurrent`] while the writer writes, or when the words read are
    /// none a publication writes.
    pub fn read(&self) -> Result<Status, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let mut words = [0; WORDS];
        for (value, word) in words.iter_mut().zip(&self.words) {
            *value = word.load(Ordering::Relaxed);
        }
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        decode(words).ok_or(Concurrent)
    }
}

impl Default for Published {
    fn default() -> Self {
        Self::new()
    }
}

fn encode(status: Status) -> [u32; WORDS] {
    let epoch = status.link.epoch.to_le_bytes();
    let [e0, e1, e2, e3, e4, e5, e6, e7] = epoch;
    let mut shape = if status.link.state == LinkState::Up {
        UP
    } else {
        0
    };
    let (address, gateway) = match status.ipv4 {
        Ipv4Status::Unconfigured => (0, 0),
        Ipv4Status::Lease(configuration) | Ipv4Status::Static(configuration) => {
            shape |= if matches!(status.ipv4, Ipv4Status::Lease(_)) {
                LEASE
            } else {
                STATIC
            };
            shape |= u32::from(configuration.prefix_length) << PREFIX_SHIFT;
            if configuration.gateway.is_some() {
                shape |= GATEWAY;
            }
            (
                u32::from_le_bytes(configuration.address),
                configuration.gateway.map_or(0, u32::from_le_bytes),
            )
        }
    };
    let detail = match status.lifecycle {
        Lifecycle::Running => 0,
        Lifecycle::Stopped => {
            shape |= STOPPED << LIFECYCLE_SHIFT;
            0
        }
        Lifecycle::Failed(error) => {
            let (code, detail) = error_code(error);
            shape |= FAILED << LIFECYCLE_SHIFT | u32::from(code) << ERROR_SHIFT;
            detail
        }
    };
    [
        u32::from_le_bytes([e0, e1, e2, e3]),
        u32::from_le_bytes([e4, e5, e6, e7]),
        detail,
        shape,
        address,
        gateway,
    ]
}

/// The status `words` hold, or none when no publication wrote them.
fn decode(words: [u32; WORDS]) -> Option<Status> {
    let [epoch_low, epoch_high, detail, shape, address, gateway] = words;
    let link = LinkSnapshot {
        state: if shape & UP != 0 {
            LinkState::Up
        } else {
            LinkState::Down
        },
        epoch: u64::from(epoch_low) | u64::from(epoch_high) << 32,
    };
    let configuration = Ipv4Configuration {
        address: address.to_le_bytes(),
        prefix_length: (shape >> PREFIX_SHIFT).to_le_bytes()[0],
        gateway: (shape & GATEWAY != 0).then(|| gateway.to_le_bytes()),
    };
    let ipv4 = match shape & KIND {
        0 => Ipv4Status::Unconfigured,
        LEASE => Ipv4Status::Lease(configuration),
        STATIC => Ipv4Status::Static(configuration),
        _ => return None,
    };
    let lifecycle = match (shape >> LIFECYCLE_SHIFT) & 0xff {
        0 => Lifecycle::Running,
        STOPPED => Lifecycle::Stopped,
        FAILED => Lifecycle::Failed(error_of((shape >> ERROR_SHIFT).to_le_bytes()[0], detail)?),
        _ => return None,
    };
    Some(Status {
        link,
        ipv4,
        lifecycle,
    })
}

/// A failure's code and detail word.
fn error_code(error: ServiceError) -> (u8, u32) {
    match error {
        ServiceError::InvalidPrefixLength(prefix) => (1, u32::from(prefix)),
        ServiceError::EndpointAlreadySplit => (2, 0),
        ServiceError::RxWaiterUnavailable => (3, 0),
        ServiceError::InterfaceTableFull => (4, 0),
        ServiceError::UnsupportedMedium => (5, 0),
        ServiceError::HardwareAddressMismatch => (6, 0),
        ServiceError::AddressNotUnicast => (7, 0),
        ServiceError::AddressTableFull => (8, 0),
        ServiceError::GatewayNotUnicast => (9, 0),
        ServiceError::RouteTableFull => (10, 0),
        ServiceError::DhcpMediumMismatch => (11, 0),
        ServiceError::ContextAccessRefused => (12, 0),
    }
}

/// The failure of `code` and `detail`, or none for a code [`error_code`]
/// never writes.
fn error_of(code: u8, detail: u32) -> Option<ServiceError> {
    Some(match code {
        1 => ServiceError::InvalidPrefixLength(detail.to_le_bytes()[0]),
        2 => ServiceError::EndpointAlreadySplit,
        3 => ServiceError::RxWaiterUnavailable,
        4 => ServiceError::InterfaceTableFull,
        5 => ServiceError::UnsupportedMedium,
        6 => ServiceError::HardwareAddressMismatch,
        7 => ServiceError::AddressNotUnicast,
        8 => ServiceError::AddressTableFull,
        9 => ServiceError::GatewayNotUnicast,
        10 => ServiceError::RouteTableFull,
        11 => ServiceError::DhcpMediumMismatch,
        12 => ServiceError::ContextAccessRefused,
        _ => return None,
    })
}

#[cfg(test)]
mod tests;
