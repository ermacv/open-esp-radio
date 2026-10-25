//! Restricted CTE-disabled direction-finding hardware baseline.

#![deny(unsafe_code)]

use crate::layouts::{BleDirectionFindingDescriptor, DmaRef};

use crate::{BluetoothTaskRegisters, device_fence};

/// Hardware ownership of the controller-global direction-finding descriptor.
///
/// Ordinary BLE roles retain this baseline even when no IQ sampling procedure
/// is enabled. The token carries no register image and grants no SRAM access;
/// the controller lifecycle must retain the matching pinned descriptor until
/// hardware ownership is explicitly recovered by a future teardown path.
#[must_use = "the CTE-disabled hardware baseline owns its pinned descriptor"]
pub struct BluetoothDirectionFindingDisabledBaselinePrepared {
    descriptor: DmaRef<BleDirectionFindingDescriptor>,
}

impl BluetoothDirectionFindingDisabledBaselinePrepared {
    /// Return the exact pinned descriptor claimed by buffer slot zero.
    pub const fn descriptor(&self) -> DmaRef<BleDirectionFindingDescriptor> {
        self.descriptor
    }
}

impl BluetoothTaskRegisters {
    /// Publish the controller-global CTE-disabled baseline used by ordinary BLE roles.
    ///
    /// The reviewed transaction selects the six-entry hardware ring, publishes
    /// the initialized descriptor through entry zero and transfers that entry
    /// from software to hardware ownership. Direction-finding feature policy,
    /// IQ sample allocation and HCI procedures are deliberately absent.
    ///
    /// # Safety
    ///
    /// The caller must retain a powered, exclusively serialized Controller
    /// epoch and the initialized pinned descriptor for the lifetime of the
    /// returned token. No CPU access to hardware-owned descriptor state is
    /// permitted until a separately reviewed retirement transaction succeeds.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the caller retains powered-controller and descriptor-lifetime prerequisites"
    )]
    pub unsafe fn prepare_direction_finding_disabled_baseline(
        &mut self,
        descriptor: DmaRef<BleDirectionFindingDescriptor>,
    ) -> BluetoothDirectionFindingDisabledBaselinePrepared {
        let image =
            crate::generated::ble_hw_cte_ring_control::buffer_pointer::CompressedDescriptorPointerImage::new(
                descriptor,
            )
            .expect("the placed layout has a nonzero BLE encoding");
        let slot = oer_mmio::Index::constant::<0>();
        let ring = &mut self.bluetooth.ble_hw_cte_ring_control;

        device_fence();
        ring.configure_bluetooth_cte_six_buffer_limit();
        ring.publish_bluetooth_cte_sample_descriptor_pointer(slot, image);
        ring.clear_bluetooth_cte_buffer_software_ownership(slot);
        device_fence();

        BluetoothDirectionFindingDisabledBaselinePrepared { descriptor }
    }
}
