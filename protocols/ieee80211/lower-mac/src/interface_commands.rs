//! Affine interface commands: one configuration and one TSF sample per interface.
//!
//! The install's `Mint` creates each permission once. An operation moves its
//! permission into the FIFO and its terminal returns the same owner. Neither
//! configure nor sample replaces a previous command. The TSF permission belongs
//! to the interface because `BeaconTimingPhase` requests that interface's TSF;
//! two interface clocks can have independent requests.
//!
//! Configuration also carries the composition's opaque context. The backend
//! returns it unchanged with the permission: a station's connection epoch does
//! not wait in a second table indexed by a runtime role. Unconsumed context
//! cannot be overwritten; consuming `into_parts` recovers both values.
use crate::{AccessPoint, InterfaceRole, PhaseTypes, Station, VifRole};
use core::marker::PhantomData;
use oer_radio_port::{Child, PoolPath, PoolSet, Root, Slot, Slots};

const CONFIGURE_PER_INTERFACE: usize = 1;
const TSF_PER_INTERFACE: usize = 1;

struct ConfigureKind<R>(PhantomData<R>);
struct TsfKind<R>(PhantomData<R>);

/// One interface's installation component; never construct it outside `Mint`.
pub struct InterfaceCommandPool<R>(PhantomData<R>);
impl<R: InterfaceRole> InterfaceCommandPool<R> {
    /// Unique FIFO permissions minted by this component.
    pub const COMMAND_CAPACITY: usize = CONFIGURE_PER_INTERFACE + TSF_PER_INTERFACE;
    /// Each permission can owe exactly one unconsumed terminal.
    pub const TERMINAL_CAPACITY: usize = Self::COMMAND_CAPACITY;
}
impl<R: InterfaceRole> PoolSet for InterfaceCommandPool<R> {
    type Pools<'id, Path: PoolPath> = InterfacePermissions<'id, R, Path>;
    fn build<'id, Path: PoolPath>(
        builder: oer_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let ([configure], [tsf]) = builder.into_pools::<(
            Slots<ConfigureKind<R>, CONFIGURE_PER_INTERFACE>,
            Slots<TsfKind<R>, TSF_PER_INTERFACE>,
        )>();
        InterfacePermissions {
            configure: ConfigurePermission {
                slot: configure,
                context: (),
            },
            tsf: TsfPermission { slot: tsf },
        }
    }
}
/// The installation gives these two independent command owners to the interface.
#[must_use]
pub struct InterfacePermissions<'id, R: InterfaceRole, Path: PoolPath = Root> {
    /// One receive-configuration FIFO credit.
    pub configure: ConfigurePermission<'id, R, Path>,
    /// One sampling FIFO credit of this interface clock.
    pub tsf: TsfPermission<'id, R, Path>,
}
/// The sole configuration command credit of an interface and installation.
/// Context travels inside the command owner and returns at `Configured`.
#[must_use = "configuration admission returns only at its terminal"]
pub struct ConfigurePermission<'id, R: InterfaceRole, Path: PoolPath = Root, C = ()> {
    slot: Slot<'id, ConfigureKind<R>, Child<Path, 0>>,
    context: C,
}
impl<'id, R: InterfaceRole, Path: PoolPath> ConfigurePermission<'id, R, Path> {
    /// Bind an operation's context without creating or cloning a permission.
    pub fn with_context<C>(self, context: C) -> ConfigurePermission<'id, R, Path, C> {
        ConfigurePermission {
            slot: self.slot,
            context,
        }
    }
}
impl<'id, R: InterfaceRole, Path: PoolPath, C> ConfigurePermission<'id, R, Path, C> {
    /// Consume a terminal's owner to recover its admission and opaque context.
    pub fn into_parts(self) -> (ConfigurePermission<'id, R, Path>, C) {
        (
            ConfigurePermission {
                slot: self.slot,
                context: (),
            },
            self.context,
        )
    }
}
impl<R: InterfaceRole, Path: PoolPath, C: core::fmt::Debug> core::fmt::Debug
    for ConfigurePermission<'_, R, Path, C>
{
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ConfigurePermission")
            .field("context", &self.context)
            .finish_non_exhaustive()
    }
}
/// One outstanding TSF request on this installation's interface.
#[must_use = "TSF sampling admission returns only at its terminal"]
pub struct TsfPermission<'id, R: InterfaceRole, Path: PoolPath = Root> {
    slot: Slot<'id, TsfKind<R>, Child<Path, 1>>,
}
impl<R: InterfaceRole, Path: PoolPath> core::fmt::Debug for TsfPermission<'_, R, Path> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("TsfPermission")
            .field("slot", &self.slot.index())
            .finish_non_exhaustive()
    }
}
/// Configuration terminals preserve the statically selected interface.
#[must_use]
pub enum Configured<'id, P: PhaseTypes<'id>> {
    /// Station interface.
    Station(ConfigurePermission<'id, Station, P::StationCommandPath, P::StationConfigureContext>),
    /// Access-point interface.
    AccessPoint(
        ConfigurePermission<
            'id,
            AccessPoint,
            P::AccessPointCommandPath,
            P::AccessPointConfigureContext,
        >,
    ),
}
impl<'id, P: PhaseTypes<'id>> Configured<'id, P> {
    /// Diagnostic role; command correlation uses the owned permission instead.
    pub fn role(&self) -> VifRole {
        match self {
            Self::Station(_) => VifRole::Station,
            Self::AccessPoint(_) => VifRole::AccessPoint,
        }
    }
}
/// A TSF sample returns the same interface permission that admitted its request.
#[must_use]
pub enum SampledTsfPermission<'id, P: PhaseTypes<'id>> {
    /// Station interface.
    Station(TsfPermission<'id, Station, P::StationCommandPath>),
    /// Access-point interface.
    AccessPoint(TsfPermission<'id, AccessPoint, P::AccessPointCommandPath>),
}
impl<'id, P: PhaseTypes<'id>> SampledTsfPermission<'id, P> {
    /// The clock being sampled, derived from the permission's interface type.
    pub fn role(&self) -> VifRole {
        match self {
            Self::Station(_) => VifRole::Station,
            Self::AccessPoint(_) => VifRole::AccessPoint,
        }
    }
}
