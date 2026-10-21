//! Nominal, irreversible static placement of port backing.

use oer_radio_port::{SlotClaim, StaticId};

#[doc(hidden)]
pub use oer_radio_port as contract;
#[doc(hidden)]
pub use static_cell::StaticCell;

/// Construct the static declaration's affine placement authority.
///
/// # Safety
/// The caller must be the sole issuer for `Declaration`. It must permanently
/// claim the declaration's one static allocation before this call and never
/// issue another claim with that type, including after quarantine or reset of
/// a software model. The allocation outlives every registration and endpoint.
/// This binds the nominal queue/owner domain to one backing allocation.
#[doc(hidden)]
#[allow(
    unsafe_code,
    reason = "the declaration establishes unique static placement"
)]
pub unsafe fn static_claim<Declaration>() -> SlotClaim<StaticId<Declaration>> {
    // SAFETY: SlotClaim is repr(C) with only invariant PhantomData, hence its
    // representation is zero sized and contains no invalid bit patterns. The
    // caller's contract establishes the unique construction authority that
    // its private fields otherwise protect; no reference or owner is cast.
    unsafe { core::mem::zeroed() }
}

/// Declare one nominal slot with an irreversible `take_static()` operation.
///
/// The state type must be a fully qualified path and implement the port's
/// `SlotOwner` for the declaration's `Id`. Placement does not run or register
/// that state. The first take returns stable backing; every later take returns
/// `None`, regardless of whether an installation has joined or quarantined.
#[macro_export]
macro_rules! declare_port_slot {
    ($visibility:vis $name:ident: $state:ty) => {
        $visibility mod $name {
            /// The declaration's nominal identity tag.
            pub enum Declaration {}
            /// Identity of this single static backing allocation.
            pub type Id = $crate::port_slot::contract::StaticId<Declaration>;

            static STORAGE: $crate::port_slot::StaticCell<
                $crate::port_slot::contract::VacantPortSlot<Id, $state>,
            > = $crate::port_slot::StaticCell::new();

            /// Take this declaration's backing once, with no reset operation.
            #[inline(never)]
            pub fn take_static() -> Option<
                &'static mut $crate::port_slot::contract::PortSlot<Id, $state>,
            > {
                let backing = STORAGE.try_init_with(|| const {
                    $crate::port_slot::contract::VacantPortSlot::<Id, $state>::vacant()
                })?;
                // SAFETY: StaticCell claimed this declaration's private
                // allocation exactly once. Its initialized vacant backing is
                // static and has not published any endpoints or capacity.
                let claim = unsafe { $crate::port_slot::static_claim::<Declaration>() };
                Some(claim.place_in(backing))
            }
        }
    };
}

#[cfg(test)]
mod tests;
