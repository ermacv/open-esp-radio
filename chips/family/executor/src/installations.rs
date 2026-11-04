//! Nominal, irreversible static identity of a port's successive
//! installations.

use oer_protocols_radio_port::{SlotClaim, StaticId};

#[doc(hidden)]
pub use oer_protocols_radio_port as contract;
#[doc(hidden)]
pub use static_cell::StaticCell;

/// Construct the static declaration's affine identity claim, which
/// [`declare_installations!`] turns at once into the identity's
/// [`contract::Installations`].
///
/// # Safety
/// The caller must be the sole issuer for `Declaration`. It must permanently
/// claim the declaration's one static take before this call and never issue
/// another claim with that type, including after quarantine or reset of a
/// software model: owned queue permissions and installation tokens rely on
/// that identity being unique.
#[doc(hidden)]
#[allow(
    unsafe_code,
    reason = "the declaration establishes a unique static identity"
)]
#[expect(
    clippy::disallowed_methods,
    reason = "the unique static claim constructs only invariant PhantomData in a zero-sized SlotClaim"
)]
pub unsafe fn static_claim<Declaration>() -> SlotClaim<StaticId<Declaration>> {
    // SAFETY: SlotClaim is repr(C) with only invariant PhantomData, hence its
    // representation is zero sized and contains no invalid bit patterns. The
    // caller's contract establishes the unique construction authority that
    // its private fields otherwise protect; no reference or owner is cast.
    unsafe { core::mem::zeroed() }
}

/// Declare one nominal static slot identity with an irreversible
/// `take_static()` that claims the minting authority of its successive
/// installations ([`contract::Installations`]). The first take returns the
/// authority; every later take returns `None`.
#[macro_export]
macro_rules! declare_installations {
    ($visibility:vis $name:ident) => {
        $visibility mod $name {
            /// The declaration's nominal identity tag.
            pub enum Declaration {}
            /// The identity every installation of this declaration shares.
            pub type Id = $crate::installations::contract::StaticId<Declaration>;

            static TAKEN: $crate::installations::StaticCell<()> = $crate::installations::StaticCell::new();

            /// Take this declaration's minting authority once, with no
            /// reset operation.
            #[inline(never)]
            pub fn take_static<P: $crate::installations::contract::PoolSet>(
            ) -> Option<$crate::installations::contract::Installations<Id, P>> {
                TAKEN.try_init(())?;
                // SAFETY: StaticCell claimed this declaration's one take
                // exactly once; no other claim with its type is issued.
                let claim = unsafe { $crate::installations::static_claim::<Declaration>() };
                Some($crate::installations::contract::Installations::claim(claim))
            }
        }
    };
}

#[cfg(test)]
mod tests;
