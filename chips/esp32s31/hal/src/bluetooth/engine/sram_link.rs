//! Typed links of the reviewed controller-SRAM graph fields.

#![forbid(unsafe_code)]

use crate::layouts::{DmaLayout, DmaRef, ble_word_index};

/// A typed link to one placed `L` of the controller-SRAM graph.
///
/// The reference is a placed layout ([`DmaRef`]) whose BLE word index is
/// nonzero ([`ble_word_index`]), so a bound link never collapses onto the
/// zero image a graph field uses for no link. The generated layouts store
/// it through their typed pointer setters; it is not a backing-storage or
/// publication token.
pub(crate) struct ControllerSramLink<L: DmaLayout> {
    reference: DmaRef<L>,
}

impl<L: DmaLayout> ControllerSramLink<L> {
    /// The link to a placed layout.
    ///
    /// # Panics
    ///
    /// The placed layout lies outside the controller SRAM window the BLE
    /// encoding reaches, or at its base: placement proves neither.
    pub(crate) const fn new(reference: DmaRef<L>) -> Self {
        assert!(
            ble_word_index(reference.bits()).is_some(),
            "a placed controller layout has a nonzero BLE encoding"
        );
        Self { reference }
    }

    /// The typed reference the graph field stores.
    pub(crate) const fn reference(self) -> DmaRef<L> {
        self.reference
    }

    /// The CPU address behind this bound link.
    ///
    /// The value remains address geometry only. It grants neither CPU access
    /// to the backing allocation nor scheduler publication authority.
    pub(crate) const fn controller_address(self) -> u32 {
        self.reference.bits()
    }

    /// Whether `address`, a graph field's decoded address, names this link.
    pub(crate) const fn is(self, address: Option<u32>) -> bool {
        matches!(address, Some(address) if address == self.reference.bits())
    }
}

impl<L: DmaLayout> Clone for ControllerSramLink<L> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<L: DmaLayout> Copy for ControllerSramLink<L> {}

impl<L: DmaLayout> PartialEq for ControllerSramLink<L> {
    fn eq(&self, other: &Self) -> bool {
        self.reference == other.reference
    }
}

impl<L: DmaLayout> Eq for ControllerSramLink<L> {}

impl<L: DmaLayout> core::fmt::Debug for ControllerSramLink<L> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("ControllerSramLink")
            .field(&self.reference.bits())
            .finish()
    }
}

/// A link to one scheduler item, the identity the scheduler lists use.
pub(crate) type SchedulerItemLink = ControllerSramLink<crate::layouts::BleSchedulerItem>;

/// A host model's link to the synthetic address `address`.
#[cfg(test)]
pub(crate) fn model_link<L: DmaLayout>(address: u32) -> ControllerSramLink<L> {
    ControllerSramLink::new(crate::memory::dma_address::model_reference(address))
}
