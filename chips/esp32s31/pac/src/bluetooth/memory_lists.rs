//! Restricted controller memory-list pointer publication.

#![deny(unsafe_code)]

use crate::{BluetoothTaskRegisters, device_fence};

const CONTROLLER_SRAM_ADDRESS_MASK: u32 = 0xffc0_0003;
const CONTROLLER_SRAM_ADDRESS_PREFIX: u32 = 0x2f00_0000;

/// One of the three positional hardware list selectors proven by the
/// selector leaves in the ESP32-S31 controller archive.
///
/// The PAC keeps these names positional. The controller-memory layer owns the
/// separately proven scan/non-scan global-insertion routing, while selector
/// three and the private DTM publication path remain semantically unassigned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListSelector {
    /// Vendor selector value one.
    One,
    /// Vendor selector value two.
    Two,
    /// Vendor selector value three.
    Three,
}

/// One of the two reviewed receive-pointer words associated with each selector.
///
/// Current setter bodies are instruction-identical to the same-chip named
/// `r_ble_phy_global_curr_rxptr_set` and
/// `r_ble_phy_global_next_rxptr_set` leaves over all three selector branches.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListSlot {
    /// Pointer consumed as the current receive buffer/header chain.
    CurrentRx,
    /// Pointer consumed as the next receive buffer/header chain.
    NextRx,
}

/// Address accepted by the controller's reviewed compressed-pointer format.
///
/// The type proves only the instruction-level encoding contract: four-byte
/// alignment and the fixed `0x2f` SRAM prefix reconstructed by controller
/// software. It does not prove allocation, lifetime, list contents, or which
/// protocol role owns the pointed-to memory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothControllerSramAddress(u32);

/// Why a raw address cannot be represented by the reviewed controller format.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothControllerSramAddressError {
    /// The low two address bits are not zero.
    Unaligned,
    /// The fixed address positions do not match the observed SRAM window.
    OutsideEncodableWindow,
}

impl BluetoothControllerSramAddress {
    /// Validate one raw address against the exact compressed-pointer domain.
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        if address & 0x3 != 0 {
            return Err(BluetoothControllerSramAddressError::Unaligned);
        }
        if address & CONTROLLER_SRAM_ADDRESS_MASK != CONTROLLER_SRAM_ADDRESS_PREFIX {
            return Err(BluetoothControllerSramAddressError::OutsideEncodableWindow);
        }
        Ok(Self(address))
    }

    /// Return the validated CPU address without granting dereference access.
    pub const fn address(self) -> u32 {
        self.0
    }

    /// Reconstruct one address read from a reviewed controller pointer field.
    ///
    /// The field width itself guarantees the encodable window. Callers must
    /// keep any zero/null interpretation in the owning register transaction.
    pub(crate) const fn from_compressed_image(image: u32) -> Self {
        Self(CONTROLLER_SRAM_ADDRESS_PREFIX | (image << 2))
    }

    /// Return the exact low-twenty-bit positional controller image.
    ///
    /// This does not grant dereference or publication authority. It is shared
    /// by reviewed list registers and controller-SRAM descriptor links.
    pub const fn compressed_image(self) -> u32 {
        (self.0 >> 2) & 0x000f_ffff
    }
}

/// Exact low-twenty-bit image accepted by the recovered selector leaves.
///
/// `Zero` is intentionally not named empty, null, or disabled: the reviewed
/// RX-link reset supplies literal zero for `NextRx`, but the hardware's
/// rotation and empty-chain semantics are not yet established.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListPointerImage {
    /// Publish the finite low-twenty-bit zero image.
    Zero,
    /// Publish one validated compressed SRAM address.
    Address(BluetoothControllerSramAddress),
}

impl BluetoothMemoryListPointerImage {
    const fn compressed(self) -> u32 {
        match self {
            Self::Zero => 0,
            Self::Address(address) => address.compressed_image(),
        }
    }
}

impl BluetoothTaskRegisters {
    /// Finish the reviewed initial RX-list reset after current/next publication.
    ///
    /// Pinned `memMgmt_4.o:r_sym_memMgmt_zJpaiu5wcM3sURVaC4Za`
    /// (`r_ble_lll_mmgmt_reset_rxlink`) clears RX_VALID,
    /// then freshly reads and writes back the whole current-pointer register.
    /// The second operation preserves any intervening hardware update. Its
    /// broader control/rotation meaning remains unassigned. Selector three
    /// has no corresponding operation in that reset body.
    ///
    /// # Safety
    /// The caller must retain the same powered, serialized list lifecycle and
    /// pinned storage required by `program_memory_list_pointer`.
    #[allow(unsafe_code, reason = "the caller owns the serialized RX-list reset")]
    #[doc(hidden)]
    pub unsafe fn reset_memory_list_initial_control(
        &mut self,
        selector: BluetoothMemoryListSelector,
    ) {
        let controller = &self.bluetooth.bluetooth_controller_core;
        match selector {
            BluetoothMemoryListSelector::One => {
                crate::generated::clear_bluetooth_memory_list_1_initial_control(controller);
                crate::generated::preserve_bluetooth_memory_list_1_initial_control(controller);
            }
            BluetoothMemoryListSelector::Two => {
                crate::generated::clear_bluetooth_memory_list_2_initial_control(controller);
                crate::generated::preserve_bluetooth_memory_list_2_initial_control(controller);
            }
            BluetoothMemoryListSelector::Three => {}
        }
        device_fence();
    }

    /// Program one controller receive-list pointer.
    ///
    /// SOURCE: complete pinned ESP32-S31 `libble_app.a` `ble_phy.c` member
    /// `ble_70.o` symbols `r_sym_ble_jhaN9KOsH0u0RgIu2DI9` and
    /// `r_sym_ble_95olV4pLl0ue84SnuzXf`. Their complete bodies are
    /// instruction-identical to named same-chip
    /// `r_ble_phy_global_curr_rxptr_set` and
    /// `r_ble_phy_global_next_rxptr_set`. Selector values one through three
    /// choose the three current/next pairs at `0x20101280..=0x20101294`. The
    /// exact transaction performs a fresh read/write clearing bits 0..19,
    /// then a second fresh read/write OR-ing the compressed image into that
    /// complete second read. The latter detail intentionally preserves any
    /// bits that hardware may publish between the two RMW operations.
    ///
    /// This operation intentionally accepts only positional selectors. The
    /// memory layer must choose an active semantic class or retain explicit
    /// evidence for a private path; element layout and lifecycle are not PAC
    /// policy.
    ///
    /// # Safety
    ///
    /// The caller must own a powered controller lifecycle state in which the
    /// selected hardware list may be changed, and must ensure the pointed-to
    /// storage remains correctly initialized, exclusively serialized, and
    /// alive until a later verified transaction removes it from hardware.
    #[allow(
        unsafe_code,
        reason = "the signature retains controller-lifecycle and pointed-storage prerequisites"
    )]
    #[doc(hidden)]
    pub unsafe fn program_memory_list_pointer(
        &mut self,
        selector: BluetoothMemoryListSelector,
        slot: BluetoothMemoryListSlot,
        image: BluetoothMemoryListPointerImage,
    ) {
        let compressed = crate::generated::BluetoothMemoryListPointerBits::new(image.compressed())
            .expect("BluetoothMemoryListPointerImage is always a low-twenty-bit value");

        macro_rules! program {
            ($controller:expr, $clear:path, $publish:path) => {{
                $clear($controller);
                $publish($controller, compressed);
            }};
        }

        let controller = &self.bluetooth.bluetooth_controller_core;
        match (selector, slot) {
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::CurrentRx) => {
                program!(
                    controller,
                    crate::generated::clear_bluetooth_memory_list_1_pointer_a,
                    crate::generated::or_bluetooth_memory_list_1_pointer_a
                );
            }
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::NextRx) => {
                program!(
                    controller,
                    crate::generated::clear_bluetooth_memory_list_1_pointer_b,
                    crate::generated::or_bluetooth_memory_list_1_pointer_b
                );
            }
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::CurrentRx) => {
                program!(
                    controller,
                    crate::generated::clear_bluetooth_memory_list_2_pointer_a,
                    crate::generated::or_bluetooth_memory_list_2_pointer_a
                );
            }
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::NextRx) => {
                program!(
                    controller,
                    crate::generated::clear_bluetooth_memory_list_2_pointer_b,
                    crate::generated::or_bluetooth_memory_list_2_pointer_b
                );
            }
            (BluetoothMemoryListSelector::Three, BluetoothMemoryListSlot::CurrentRx) => {
                program!(
                    controller,
                    crate::generated::clear_bluetooth_memory_list_3_pointer_a,
                    crate::generated::or_bluetooth_memory_list_3_pointer_a
                );
            }
            (BluetoothMemoryListSelector::Three, BluetoothMemoryListSlot::NextRx) => {
                program!(
                    controller,
                    crate::generated::clear_bluetooth_memory_list_3_pointer_b,
                    crate::generated::or_bluetooth_memory_list_3_pointer_b
                );
            }
        }
        device_fence();
    }
}

#[cfg(test)]
mod tests;
