//! Nominal, irreversible static placement of port backing.

use core::{marker::PhantomData, ptr::NonNull};
use oer_radio_port::{
    Joined, PortSlot, SlotFrontend, SlotIdentity, SlotOwner, SlotRegisterError, SlotRegistration,
    SlotRetireRefused,
};
use oer_radio_port::{SlotClaim, StaticId};

#[doc(hidden)]
pub use oer_radio_port as contract;
#[doc(hidden)]
pub use static_cell::StaticCell;

/// Permanent slot retained by its one driver task across joined installations.
///
/// The task registers and destroys the P2 guard in its own context, and sends
/// only `SlotFrontend` to P1. Endpoints can outlive any frontend future; losing
/// join authority permanently prevents another borrow of the backing.
pub struct StaticPortSlot<Id: SlotIdentity + 'static, S: SlotOwner<Id> + 'static> {
    slot: NonNull<PortSlot<Id, S>>,
    installed: bool,
    local: PhantomData<*mut PortSlot<Id, S>>,
}

impl<Id: SlotIdentity + 'static, S: SlotOwner<Id> + 'static> StaticPortSlot<Id, S> {
    /// Transfer the permanently located allocation to its driver task.
    pub fn new(slot: &'static mut PortSlot<Id, S>) -> Self {
        Self {
            slot: NonNull::from(slot),
            installed: false,
            local: PhantomData,
        }
    }

    /// Register once, retaining the guard on P2 before publishing its frontend.
    pub fn register(
        &mut self,
        owner: S,
    ) -> Result<
        (
            SlotFrontend<'static, Id, S>,
            SlotRegistration<'static, Id, S>,
        ),
        SlotRegisterError<'static, Id, S>,
    > {
        if self.installed {
            return Err(SlotRegisterError::Occupied(owner));
        }
        self.installed = true;
        #[allow(
            unsafe_code,
            reason = "the first registration exclusively borrows the retained static allocation"
        )]
        // SAFETY: new consumed the only static mutable reference. No endpoint
        // has been issued, and installed prohibits any repeated initial borrow.
        let slot = unsafe { &mut *self.slot.as_ptr() };
        slot.register(owner)
    }

    /// Borrow the same backing again only after its ordinary join proof.
    pub fn reinstall<Resume: FnOnce(S::Vacant) -> S>(
        &mut self,
        joined: Joined<Id>,
        resume: Resume,
    ) -> Result<
        (
            SlotFrontend<'static, Id, S>,
            SlotRegistration<'static, Id, S>,
        ),
        SlotRegisterError<'static, Id, S, Resume>,
    > {
        let (slot, joined) = self.joined(joined);
        slot.reinstall(joined, resume)
    }

    /// Return the vacant owner after its ordinary join proof, ending the
    /// backing's registrations.
    pub fn retire(&mut self, joined: Joined<Id>) -> Result<S::Vacant, SlotRetireRefused<Id>> {
        let (slot, joined) = self.joined(joined);
        slot.retire(joined)
    }

    /// The backing, borrowed again under the join proof that the caller must
    /// pass on to the slot operation consuming it.
    fn joined(&mut self, joined: Joined<Id>) -> (&'static mut PortSlot<Id, S>, Joined<Id>) {
        #[allow(
            unsafe_code,
            reason = "the consumed Joined proof ends the previous endpoint epoch"
        )]
        // SAFETY: this nominal Id has one allocation. Joined is issued only
        // after the frontend ended its endpoint loans and the P2 registration
        // finished, destroyed its owner and invocation, and unregistered.
        // Quarantine issues no Joined, so it cannot permit this reborrow. Both
        // callers hand the returned Joined to the slot operation consuming it.
        let slot = unsafe { &mut *self.slot.as_ptr() };
        (slot, joined)
    }
}

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
