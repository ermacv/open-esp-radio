//! Statically bounded channel resources of one Controller epoch.

use embassy_sync::blocking_mutex::raw::RawMutex;

use oer_bluetooth_hci::{
    BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY, LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY,
    LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY, LeControllerBootstrapConfig,
};

use crate::{
    InProcessHciControllerTransport, InProcessHciHostTransport, in_process::InProcessHciChannel,
};

const HCI_ACL_HEADER_BYTES: usize = 4;

/// Why an HCI profile cannot represent its advertised LE resources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeControllerHciResourcesError {
    /// A complete command, event or advertised LE ACL packet does not fit one
    /// transport slot.
    PacketCapacityTooSmall {
        /// Minimum packet-body storage required by the profile.
        required: usize,
        /// Compile-time storage selected by the caller.
        available: usize,
    },
    /// The Controller advertised more simultaneous Host ACL credits than its
    /// inbound queue can retain.
    AclCreditsExceedHostQueue {
        /// Credits reported by LE Read Buffer Size.
        credits: usize,
        /// Complete Host packet slots owned by this epoch.
        slots: usize,
    },
}

/// Both endpoints borrowed from one HCI resource epoch.
#[must_use = "all HCI endpoints belong to one resource epoch"]
// CAPABILITY: hci-controller-endpoints
pub struct LeControllerHciEndpoints<
    'resources,
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    /// Host-facing typed HCI transport.
    pub host: InProcessHciHostTransport<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
    /// Controller-facing raw transport.
    pub controller: InProcessHciControllerTransport<
        'resources,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    >,
}

/// Allocation-free packet storage for one HCI epoch.
///
/// Construction checks that every packet of the bootstrap profile and every
/// advertised Host ACL credit fits the storage. The aggregate is neither
/// `Copy` nor `Clone`; splitting requires a unique borrow.
#[must_use = "HCI runtime resources must remain owned by their Controller epoch"]
// CAPABILITY: hci-handoff-storage
pub struct LeControllerHciResources<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> where
    M: RawMutex,
{
    channel:
        InProcessHciChannel<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>,
    config: LeControllerBootstrapConfig,
}

impl<
    M,
    const HOST_TO_CONTROLLER_DEPTH: usize,
    const CONTROLLER_TO_HOST_DEPTH: usize,
    const PACKET_CAPACITY: usize,
> LeControllerHciResources<M, HOST_TO_CONTROLLER_DEPTH, CONTROLLER_TO_HOST_DEPTH, PACKET_CAPACITY>
where
    M: RawMutex,
{
    /// Construct an empty HCI epoch whose storage covers every advertised
    /// initial LE packet and credit.
    pub fn new(config: LeControllerBootstrapConfig) -> Result<Self, LeControllerHciResourcesError> {
        let acl_packet_capacity =
            usize::from(config.le_acl_data_packet_length()).saturating_add(HCI_ACL_HEADER_BYTES);
        let required = acl_packet_capacity
            .max(BOOTSTRAP_COMMAND_COMPLETE_EVENT_CAPACITY)
            .max(LE_DTM_COMMAND_COMPLETE_EVENT_CAPACITY)
            .max(LE_LEGACY_ADVERTISING_REPORT_EVENT_CAPACITY);
        if PACKET_CAPACITY < required {
            return Err(LeControllerHciResourcesError::PacketCapacityTooSmall {
                required,
                available: PACKET_CAPACITY,
            });
        }

        let credits = usize::from(config.total_num_le_acl_data_packets());
        if credits > HOST_TO_CONTROLLER_DEPTH {
            return Err(LeControllerHciResourcesError::AclCreditsExceedHostQueue {
                credits,
                slots: HOST_TO_CONTROLLER_DEPTH,
            });
        }

        Ok(Self {
            channel: InProcessHciChannel::new(),
            config,
        })
    }

    /// Bootstrap profile this storage was checked against.
    pub const fn config(&self) -> LeControllerBootstrapConfig {
        self.config
    }

    /// Whether the transport is open and no packet has entered either direction.
    pub fn is_pristine(&self) -> bool {
        self.channel.is_pristine()
    }

    /// Borrow the only Host and Controller endpoints.
    pub fn split(
        &mut self,
    ) -> LeControllerHciEndpoints<
        '_,
        M,
        HOST_TO_CONTROLLER_DEPTH,
        CONTROLLER_TO_HOST_DEPTH,
        PACKET_CAPACITY,
    > {
        let (host, controller) = self.channel.split();
        LeControllerHciEndpoints { host, controller }
    }
}

#[cfg(test)]
mod tests;
