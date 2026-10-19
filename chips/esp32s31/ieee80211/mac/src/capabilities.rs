//! Exact portable MAC-service profile exposed by the ESP32-S31 driver.
//!
//! This is deliberately not a datasheet feature list.  Every field describes
//! the complete source-owned service that exists today, including its bounded
//! Rust storage.  In particular, aggregate and retry operations are split at
//! their real hardware/software boundary instead of being advertised by one
//! ambiguous offload boolean.

use oer_ieee80211_softmac::{
    MacInterfaceCapabilities, MacOperationOwner, MacOperationOwnership, MacResourceLimits,
    MacServiceCapabilities,
};

use crate::{
    rx::ampdu::{RX_BLOCK_ACK_BANK_COUNT, RX_BLOCK_ACK_MAX_WINDOW},
    rx::hardware::S31_RX_BLOCK_ACK_MAX_TID,
    tx::ampdu::{TX_AMPDU_SLOT_CAPACITY, TX_BLOCK_ACK_MAX_WINDOW},
};

/// MAC-service capabilities implemented by the current ESP32-S31 driver.
///
/// Evidence for the split ownership is kept beside the implementation:
///
/// - TX encoders exclude the hardware FCS; normal RX policy enables auto-ACK;
/// - `tx_runtime` owns retry limits, rate ladders and contention updates;
/// - portable station state owns sequence spaces, while the typed CCMP slot
///   owns PN publication and selects a hardware key;
/// - hardware BA tables/completion registers match sequence spaces and capture
///   bitmaps, while `rx_ampdu` and `tx_runtime` own reorder/retention policy.
///
/// The station exposes one group-key slot until additional receive-side key
/// lookup is established. One slot cannot receive both old and new GTK IDs
/// after replacement. The hardware helper
/// [`crate::crypto::replace_sta_group_ccmp_with_rollback`] restores the old
/// entry if a replacement write fails; it does not own replay state. The
/// portable phase's replacement terminal defines that state transition.
/// These capabilities describe hardware capacity, not an S31 backend of
/// that phase contract, which does not exist yet.
pub const ESP32S31_MAC_SERVICE_CAPABILITIES: MacServiceCapabilities = MacServiceCapabilities {
    interfaces: MacInterfaceCapabilities {
        // These are implemented owner graphs, not merely address-match slots
        // visible in hardware. The concurrent graph retains one physical RX
        // producer, one physical TX owner, and one IRQ epoch.
        station_interfaces: 1,
        access_point_interfaces: 1,
        simultaneous_station_access_point: true,
        standalone_monitor: true,
        monitor_with_interfaces: false,
        raw_monitor_tap: false,
        normalized_monitor_tap: true,
        protocol_validated_monitor_tap: false,
    },
    operations: MacOperationOwnership {
        tx_fcs_generation: MacOperationOwner::Hardware,
        immediate_ack_response: MacOperationOwner::Hardware,
        csma_ca_backoff_countdown: MacOperationOwner::Hardware,
        unicast_retry_policy: MacOperationOwner::Software,
        tx_sequence_assignment: MacOperationOwner::Software,
        ccmp_key_selection: MacOperationOwner::Software,
        ccmp_packet_number: MacOperationOwner::Software,
        ccmp_transform: MacOperationOwner::Hardware,
        rx_block_ack_matching: MacOperationOwner::Hardware,
        rx_reorder: MacOperationOwner::Software,
        tx_block_ack_capture: MacOperationOwner::Hardware,
        tx_ampdu_retry_selection: MacOperationOwner::Software,
    },
    resources: MacResourceLimits {
        // The current radio/channel owner has one home/current selector and no
        // simultaneous multi-channel scheduler.
        channel_contexts: 1,
        ordinary_tx_queues: 4,
        rx_block_ack_entries: RX_BLOCK_ACK_BANK_COUNT as u8,
        rx_block_ack_max_tid: S31_RX_BLOCK_ACK_MAX_TID,
        rx_block_ack_max_window: RX_BLOCK_ACK_MAX_WINDOW,
        tx_block_ack_max_window: TX_BLOCK_ACK_MAX_WINDOW,
        tx_ampdu_max_subframes: TX_AMPDU_SLOT_CAPACITY as u16,
        // These are the slots intentionally exposed by the open STA driver,
        // not the size of the underlying hardware key table.
        station_pairwise_ccmp_slots: 1,
        // One station group slot until receive-side lookup for another entry
        // is proven. This count alone makes no rekey-ordering guarantee.
        station_group_ccmp_slots: 1,
        access_point_pairwise_ccmp_slots: 15,
        access_point_group_ccmp_slots: 1,
        access_point_association_entries: 15,
        access_point_encrypted_clients: 15,
    },
};

#[cfg(test)]
mod tests;
