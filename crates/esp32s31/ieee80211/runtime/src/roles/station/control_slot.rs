//! Static storage for the control of one association at a time.
//!
//! The connected control owner is large (BlockAck sessions, power and
//! link-monitor state). The connected services, their teardown and every
//! failure that keeps them moved it by value, so each byte of control state
//! appeared in a by-value teardown frame once per owner that held it. The
//! slot keeps the control in place for the whole association: the owners
//! hold a [`PlacedControl`], one pointer, and the slot returns empty to its
//! owner when the association's teardown releases it.

use core::{
    future::Future,
    ops::{Deref, DerefMut},
};

use crate::{
    datapath::{DatapathControlContext, DatapathControlProgress, services::DatapathControlService},
    roles::station::teardown::ConnectedStaControlTeardown,
};

/// The slot of the station's connected control.
pub type ConnectedControlStorage<'resources, M, const CAPACITY: usize> =
    ConnectedControlSlot<crate::roles::station::control::ConnectedControl<'resources, M, CAPACITY>>;

/// The station's connected control, placed in its slot.
pub type PlacedConnectedControl<'resources, M, const CAPACITY: usize> = PlacedControl<
    'resources,
    crate::roles::station::control::ConnectedControl<'resources, M, CAPACITY>,
>;

/// The empty slot a released connected control hands back.
pub type ReleasedConnectedControl<'resources, M, const CAPACITY: usize> =
    &'resources mut ConnectedControlStorage<'resources, M, CAPACITY>;

/// Storage for one association's control; empty between associations.
///
/// An empty slot is [`Self::new`], not zeroed memory: its emptiness is a
/// niche value of the control, so a slot in a zero-initialized `.bss`
/// section must be written with [`Self::new`] before its first placement.
pub struct ConnectedControlSlot<C> {
    control: Option<C>,
}

impl<C> Default for ConnectedControlSlot<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C> ConnectedControlSlot<C> {
    pub const fn new() -> Self {
        Self { control: None }
    }

    /// Whether a control is placed.
    pub const fn is_occupied(&self) -> bool {
        self.control.is_some()
    }

    /// Place `control` in the slot, replacing a control a faulted
    /// association left behind. Placement writes into the slot's storage;
    /// configure the control afterwards through the lease, not by value.
    #[inline(always)]
    pub fn place(&mut self, control: C) -> PlacedControl<'_, C> {
        self.control = Some(control);
        PlacedControl { slot: self }
    }
}

/// The control of one association, placed in its slot. It dereferences to
/// the control and, released, empties the slot and hands it back.
pub struct PlacedControl<'slot, C> {
    slot: &'slot mut ConnectedControlSlot<C>,
}

impl<'slot, C> PlacedControl<'slot, C> {
    /// Drop the control in place and return the empty slot.
    pub fn release(self) -> &'slot mut ConnectedControlSlot<C> {
        self.slot.control = None;
        self.slot
    }
}

impl<C> Deref for PlacedControl<'_, C> {
    type Target = C;

    fn deref(&self) -> &C {
        match &self.slot.control {
            Some(control) => control,
            None => unreachable!("a placed control stays in its slot until released"),
        }
    }
}

impl<C> DerefMut for PlacedControl<'_, C> {
    fn deref_mut(&mut self) -> &mut C {
        match &mut self.slot.control {
            Some(control) => control,
            None => unreachable!("a placed control stays in its slot until released"),
        }
    }
}

/// The placed control serves the datapath as the control itself.
impl<C, H, X> DatapathControlService<H, X> for PlacedControl<'_, C>
where
    C: DatapathControlService<H, X>,
{
    type Error = C::Error;
    type Exit = C::Exit;

    fn service<'a>(
        &'a mut self,
        hardware: &'a mut H,
        tx: &'a mut X,
        context: DatapathControlContext,
    ) -> impl Future<Output = Result<DatapathControlProgress<Self::Exit>, Self::Error>> + 'a {
        self.deref_mut().service(hardware, tx, context)
    }

    fn ready(&self, tx: &X, now: oer_time::Instant) -> bool {
        self.deref().ready(tx, now)
    }

    fn required_before_network_tx(&self) -> bool {
        self.deref().required_before_network_tx()
    }

    fn required_before_stop(&self) -> bool {
        self.deref().required_before_stop()
    }

    fn admits_network_tx(&self) -> bool {
        self.deref().admits_network_tx()
    }

    fn wait_ready<'a>(&'a mut self, tx: &'a mut X) -> impl Future<Output = ()> + 'a {
        self.deref_mut().wait_ready(tx)
    }
}

/// The placed control shuts down as the control itself; its release
/// drops the control in place and returns the empty slot.
impl<'slot, C, H, X> ConnectedStaControlTeardown<H, X> for PlacedControl<'slot, C>
where
    C: ConnectedStaControlTeardown<H, X>,
{
    type Report = C::Report;
    type Error = C::Error;
    type Released = &'slot mut ConnectedControlSlot<C>;

    fn shutdown(&mut self, hardware: &mut H, tx: &mut X) -> Result<Self::Report, Self::Error> {
        self.deref_mut().shutdown(hardware, tx)
    }

    fn release(self) -> Self::Released {
        PlacedControl::release(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placed_control_is_reached_through_its_lease_and_released_in_place() {
        let mut slot = ConnectedControlSlot::<[u8; 64]>::new();
        let mut placed = slot.place([1; 64]);
        placed[0] = 7;
        assert_eq!(placed[0], 7);
        let slot = placed.release();
        assert!(!slot.is_occupied());
        // The slot takes the next association's control.
        let placed = slot.place([2; 64]);
        assert_eq!(placed[1], 2);
    }

    #[test]
    fn the_teardown_failure_and_the_services_hold_a_placed_control_by_pointer() {
        type Big = [u8; 4096];
        type Failure<'s> = crate::roles::station::teardown::ConnectedStaTeardownFailure<
            (),
            (),
            (),
            (),
            PlacedControl<'s, Big>,
            (),
            (),
        >;
        type Services<'s> =
            crate::datapath::services::SingleRoleServices<(), (), (), PlacedControl<'s, Big>>;
        assert!(core::mem::size_of::<Failure<'_>>() < core::mem::size_of::<Big>());
        assert!(core::mem::size_of::<Services<'_>>() < core::mem::size_of::<Big>());
    }

    #[test]
    fn a_lease_is_one_pointer_whatever_the_control_size() {
        assert_eq!(
            core::mem::size_of::<PlacedControl<'_, [u8; 4096]>>(),
            core::mem::size_of::<usize>()
        );
    }
}
