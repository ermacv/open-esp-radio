//! Controller-global SRAM workspace retained by ordinary BLE link states.
//!
//! The open driver reserves one `0x20`-byte direction-finding environment,
//! retaining padding after the pinned initializer's `0x1c`-byte allocation. The
//! open driver owns that allocation directly: it initializes only the two
//! reviewed baseline members, validates the complete physical extent and
//! exposes opaque typed links rather than a vendor allocator or raw words.

#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

use core::{marker::PhantomPinned, pin::Pin};

use crate::bluetooth::engine::{ControllerDmaSpace, ControllerSramLink, Placement};
use crate::dma_static::Placed;

use crate::bluetooth::BluetoothControllerReset;
use crate::layouts::*;

/// Bytes retained by the recovered controller-global direction-finding environment.
pub const BLUETOOTH_DIRECTION_FINDING_WORKSPACE_BYTES: usize = BleDirectionFindingWorkspace::SIZE;

/// Static storage for the controller-global disabled-CTE baseline.
///
/// Volatile cells reflect that the Controller may observe and later mutate the
/// published descriptor. No public API grants CPU access after publication.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct DirectionFindingWorkspaceStorage {
    layout: BleDirectionFindingWorkspace,
    _pin: PhantomPinned,
}

const _: () = {
    assert!(size_of::<DirectionFindingWorkspaceStorage>() == 0x20);
    assert!(align_of::<DirectionFindingWorkspaceStorage>() == 4);
};

/// Opaque link from an ordinary BLE link state to the global DF environment.
///
/// This value carries reviewed address geometry only. It grants neither CPU
/// access to the allocation nor CTE-register publication authority.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct DirectionFindingWorkspaceLink {
    link_state_configuration: ControllerSramLink<BleDirectionFindingConfiguration>,
}

impl DirectionFindingWorkspaceLink {
    /// The configuration a link state's direction-finding word points at,
    /// consumed only by sibling SRAM codecs.
    pub(crate) const fn link_state_configuration(self) -> DmaRef<BleDirectionFindingConfiguration> {
        self.link_state_configuration.reference()
    }
}

/// Immutable geometry retained with one workspace allocation.
pub(crate) struct DirectionFindingWorkspaceBinding {
    link_state_configuration: ControllerSramLink<BleDirectionFindingConfiguration>,
    disabled_cte_descriptor: DmaRef<BleDirectionFindingDescriptor>,
}

impl DirectionFindingWorkspaceBinding {
    pub(super) fn new(
        storage: &DirectionFindingWorkspaceStorage,
        placement: &crate::dma_address::PlacementProjection,
    ) -> Result<Self, crate::dma_address::ProjectionError> {
        let disabled_cte_descriptor = placement
            .reference_at::<BleDirectionFindingDescriptor, _, _>(
                &storage.layout,
                storage.layout.word_000c_opaque(),
            )?;
        let link_state_configuration = placement
            .reference_at::<BleDirectionFindingConfiguration, _, _>(
                &storage.layout,
                storage.layout.word_0008_opaque(),
            )?;
        Ok(Self {
            link_state_configuration: ControllerSramLink::new(link_state_configuration),
            disabled_cte_descriptor,
        })
    }

    /// Descriptor published into CTE hardware buffer zero.
    pub const fn disabled_cte_descriptor_address(&self) -> DmaRef<BleDirectionFindingDescriptor> {
        self.disabled_cte_descriptor
    }

    /// Opaque ordinary-role link into the controller-global environment.
    pub const fn link(&self) -> DirectionFindingWorkspaceLink {
        DirectionFindingWorkspaceLink {
            link_state_configuration: self.link_state_configuration,
        }
    }
}

/// Unique CPU owner of one initialized, address-bound DF workspace.
#[must_use = "the direction-finding workspace must be retained or published"]
pub(crate) struct DirectionFindingWorkspaceCpuOwned {
    storage: Pin<&'static mut DirectionFindingWorkspaceStorage>,
    binding: DirectionFindingWorkspaceBinding,
}

impl DirectionFindingWorkspaceCpuOwned {
    /// Publish the disabled-CTE descriptor retained by this owner.
    pub fn publish_disabled_baseline(
        &self,
        controller: &mut crate::bluetooth::ControllerHal<'_>,
    ) -> crate::bluetooth::DirectionFindingDisabledBaselineOwner {
        controller.prepare_direction_finding_disabled_baseline(&self.disabled_cte_descriptor())
    }

    /// Borrow the validated address geometry without changing ownership.
    pub(crate) const fn binding(&self) -> &DirectionFindingWorkspaceBinding {
        &self.binding
    }

    /// Opaque link configuration, without exposing descriptor geometry.
    pub const fn link(&self) -> DirectionFindingWorkspaceLink {
        self.binding.link()
    }

    /// The disabled-CTE descriptor of this workspace, as the
    /// direction-finding baseline points the hardware at it.
    pub(crate) fn disabled_cte_descriptor(&self) -> crate::bluetooth::DirectionFindingDescriptor {
        #[allow(unsafe_code, reason = "the owner of the workspace vouches for it")]
        // SAFETY: this owner holds the workspace `Pin<&'static mut>` for as
        // long as it lives, its binding names that workspace's disabled-CTE
        // descriptor, and the workspace words are `VolatileCell`s the CPU
        // reaches only through volatile accesses.
        unsafe {
            crate::bluetooth::DirectionFindingDescriptor::bind(
                self.binding.disabled_cte_descriptor_address(),
            )
        }
    }

    /// Return the workspace to its allocation-time image after the Controller
    /// that used it was reset.
    pub fn reset_after_controller_reset(&mut self, _reset: &BluetoothControllerReset) {
        self.storage
            .as_ref()
            .get_ref()
            .initialize_disabled_baseline();
    }

    /// Model hardware overwriting the workspace during an epoch.
    #[cfg(test)]
    fn emulate_hardware_clear(&self) {
        crate::bluetooth::engine::shared_layout::clear_direction_finding_workspace(
            &self.storage.as_ref().get_ref().layout,
        );
    }

    /// Confirm the source-owned disabled-CTE initialization is still present.
    pub fn is_disabled_baseline_initialized(&self) -> bool {
        let storage = self.storage.as_ref().get_ref();
        storage.layout.word_0008_opaque().read() == DF_CONFIGURATION
            && storage.layout.word_000c_opaque().read() == DF_DESCRIPTOR
    }
}

impl DirectionFindingWorkspaceStorage {
    /// Reserve zeroed controller-global storage before address binding.
    pub const fn new() -> Self {
        Self {
            layout: BleDirectionFindingWorkspace::zeroed(),
            _pin: PhantomPinned,
        }
    }

    /// Claim the linker-proven workspace and install its disabled-CTE baseline.
    pub(crate) fn place<S: ControllerDmaSpace>(
        placed: Placed<Self>,
    ) -> Result<DirectionFindingWorkspaceCpuOwned, crate::dma_address::ProjectionError> {
        let (storage, placement) = Placement::<S>::take(placed);
        let binding = DirectionFindingWorkspaceBinding::new(storage, placement.projection())?;
        Ok(Self::from_parts(storage, binding))
    }

    pub(super) fn from_parts(
        storage: &'static mut Self,
        binding: DirectionFindingWorkspaceBinding,
    ) -> DirectionFindingWorkspaceCpuOwned {
        storage.initialize_disabled_baseline();
        DirectionFindingWorkspaceCpuOwned {
            storage: Pin::static_mut(storage),
            binding,
        }
    }
}

impl DirectionFindingWorkspaceStorage {
    /// Write the zeroed workspace with the source-owned disabled-CTE words.
    fn initialize_disabled_baseline(&self) {
        crate::bluetooth::engine::shared_layout::clear_direction_finding_workspace(&self.layout);
        self.layout.word_0008_opaque().write(DF_CONFIGURATION);
        self.layout.word_000c_opaque().write(DF_DESCRIPTOR);
    }
}

impl Default for DirectionFindingWorkspaceStorage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;

/// SOURCE: pinned `ble_4.o::r_sym_ble_mIskG2GvbtwJXrhsY5EZ`, configuration byte initialized to one.
const DF_CONFIGURATION: BleDirectionFindingWorkspaceWord0008Opaque =
    BleDirectionFindingWorkspaceWord0008Opaque::ZERO.with_bits_0_7_opaque(0x1);
/// SOURCE: the same complete initializer stores word one in the disabled descriptor.
const DF_DESCRIPTOR: BleDirectionFindingWorkspaceWord000cOpaque =
    BleDirectionFindingWorkspaceWord000cOpaque::ZERO.with_bits_0_31_opaque(0x1);

impl core::fmt::Debug for DirectionFindingWorkspaceLink {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("DirectionFindingWorkspaceLink")
    }
}
