//! Controller receive memory-list pointer publication.

#![deny(unsafe_code)]

use crate::bluetooth::registers::{BluetoothPlacements, BluetoothTaskRegisters, device_fence};
use crate::layouts::{BleBufferHeader, DmaRef};

/// One of the three positional hardware list selectors proven by the
/// selector leaves in the ESP32-S31 controller archive.
///
/// These names stay positional. The controller-memory layer owns the
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

/// Exact low-twenty-bit image accepted by the recovered selector leaves.
///
/// `Zero` is intentionally not named empty, null, or disabled: the reviewed
/// RX-link reset supplies literal zero for `NextRx`, but the hardware's
/// rotation and empty-chain semantics are not yet established.
///
/// A scheduler item cannot substitute for a receive buffer header: the
/// address is a `DmaRef<BleBufferHeader>`, and the generated pointer image of
/// the list words takes only that reference.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothMemoryListPointerImage {
    /// Publish the finite low-twenty-bit zero image.
    Zero,
    /// Publish the typed reference to one placed buffer header.
    Address(DmaRef<BleBufferHeader>),
}

impl<P: BluetoothPlacements> BluetoothTaskRegisters<P> {
    /// Finish the reviewed initial RX-list reset after current/next publication.
    ///
    /// Pinned `memMgmt_4.o:r_sym_memMgmt_zJpaiu5wcM3sURVaC4Za`
    /// (`r_ble_lll_mmgmt_reset_rxlink`) clears RX_VALID,
    /// then freshly reads and writes back the whole current-pointer register.
    /// The second operation preserves any intervening hardware update. Its
    /// broader control/rotation meaning remains unassigned. Selector three
    /// has no corresponding operation in that reset body.
    ///
    /// The engine's initial-list transaction performs this after publishing
    /// current and next pointers through the same exclusive register loan.
    #[doc(hidden)]
    pub fn reset_memory_list_initial_control(&mut self, selector: BluetoothMemoryListSelector) {
        let controller = &mut self.bluetooth.bluetooth_controller_core;
        match selector {
            BluetoothMemoryListSelector::One => {
                controller.modify_global_rxlink_1_current(|w| {
                    w.set_rx_valid(false);
                });
                controller.modify_global_rxlink_1_current(|_| {});
            }
            BluetoothMemoryListSelector::Two => {
                controller.modify_global_rxlink_2_current(|w| {
                    w.set_rx_valid(false);
                });
                controller.modify_global_rxlink_2_current(|_| {});
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
    /// evidence for a private path; element layout and lifecycle are not register
    /// policy.
    ///
    /// An address image carries a typed DMA reference to retained volatile
    /// header storage. The engine selects the list from its `RxChainHead` proof
    /// and orders the initial current/next/reset sequence through one exclusive
    /// register loan.
    #[doc(hidden)]
    pub fn program_memory_list_pointer(
        &mut self,
        selector: BluetoothMemoryListSelector,
        slot: BluetoothMemoryListSlot,
        image: BluetoothMemoryListPointerImage,
    ) {
        macro_rules! program {
            ($clear:ident, $publish:ident) => {{
                let controller = &mut self.bluetooth.bluetooth_controller_core;
                controller.$clear(|w| {
                    w.clear_compressed_sram_pointer();
                });
                // OR the image into a fresh read, keeping what the hardware
                // published since the clear: the field accessor replaces the
                // field, so the OR is the policy's named transaction. ORing
                // the zero image writes the fresh read back unchanged.
                match image {
                    BluetoothMemoryListPointerImage::Zero => controller.$clear(|_| {}),
                    BluetoothMemoryListPointerImage::Address(reference) => {
                        controller.$publish(reference)
                    }
                }
            }};
        }

        match (selector, slot) {
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::CurrentRx) => program!(
                modify_global_rxlink_1_current,
                or_bluetooth_memory_list_1_pointer_a
            ),
            (BluetoothMemoryListSelector::One, BluetoothMemoryListSlot::NextRx) => program!(
                modify_global_rxlink_1_next,
                or_bluetooth_memory_list_1_pointer_b
            ),
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::CurrentRx) => program!(
                modify_global_rxlink_2_current,
                or_bluetooth_memory_list_2_pointer_a
            ),
            (BluetoothMemoryListSelector::Two, BluetoothMemoryListSlot::NextRx) => program!(
                modify_global_rxlink_2_next,
                or_bluetooth_memory_list_2_pointer_b
            ),
            (BluetoothMemoryListSelector::Three, BluetoothMemoryListSlot::CurrentRx) => program!(
                modify_global_rxlink_3_current,
                or_bluetooth_memory_list_3_pointer_a
            ),
            (BluetoothMemoryListSelector::Three, BluetoothMemoryListSlot::NextRx) => program!(
                modify_global_rxlink_3_next,
                or_bluetooth_memory_list_3_pointer_b
            ),
        }
        device_fence();
    }
}

#[cfg(test)]
mod tests;
