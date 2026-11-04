//! Affine interface commands: one receive configuration per interface.
//!
//! The install's `Mint` creates each permission once. A configuration moves
//! its permission into the port's command order and its terminal returns the
//! same owner, so no configuration replaces a previous one.
//!
//! Configuration also carries the composition's opaque context. The backend
//! returns it unchanged with the permission: a station's connection epoch does
//! not wait in a second table indexed by a runtime role. Unconsumed context
//! cannot be overwritten; consuming `into_parts` recovers both values.
use crate::{AccessPoint, InterfaceRole, PhaseTypes, Station, VifRole};
use core::marker::PhantomData;
use oer_protocols_radio_port::{BodyLoan, LoanedContext, PortContext};
use oer_protocols_radio_port::{PoolPath, PoolSet, Root, Slot, Slots};

const CONFIGURE_PER_INTERFACE: usize = 1;

struct ConfigureKind<R>(PhantomData<R>);

/// One interface's installation component; never construct it outside `Mint`.
pub struct InterfaceCommandPool<R>(PhantomData<R>);
impl<R: InterfaceRole> InterfaceCommandPool<R> {
    /// Unique FIFO permissions minted by this component.
    pub const COMMAND_CAPACITY: usize = CONFIGURE_PER_INTERFACE;
    /// Each permission can owe exactly one unconsumed terminal.
    pub const TERMINAL_CAPACITY: usize = Self::COMMAND_CAPACITY;
}
impl<R: InterfaceRole> PoolSet for InterfaceCommandPool<R> {
    type Pools<'id, Path: PoolPath> = InterfacePermissions<'id, R, Path>;
    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let [configure] = builder.into_pools::<Slots<ConfigureKind<R>, CONFIGURE_PER_INTERFACE>>();
        InterfacePermissions {
            configure: ConfigurePermission {
                slot: configure,
                context: (),
            },
        }
    }
}
/// The installation gives this command owner to the interface.
#[must_use]
pub struct InterfacePermissions<
    'id,
    R: InterfaceRole,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>,
> {
    /// One receive-configuration FIFO credit.
    pub configure: ConfigurePermission<'id, R, Path>,
}
/// The sole configuration command credit of an interface and installation.
/// Context travels inside the command owner and returns at `Configured`.
#[must_use = "configuration admission returns only at its terminal"]
pub struct ConfigurePermission<
    'id,
    R: InterfaceRole,
    Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>,
    C = (),
> {
    slot: Slot<'id, ConfigureKind<R>, Path>,
    context: C,
}

impl<'p, R: InterfaceRole, Path: PoolPath, C: LoanedContext<'p, Path::Identity>>
    ConfigurePermission<'p, R, Path, C>
{
    /// Transfer the same permission and opaque context to owned queue custody.
    pub fn into_owned<S>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> ConfigurePermission<'static, R, Path, C::Owned> {
        ConfigurePermission {
            slot: loan.reclaim_slot(self.slot),
            context: self.context.enclose().into_owned(loan),
        }
    }
}

impl<R: InterfaceRole, Path: PoolPath, C> ConfigurePermission<'static, R, Path, C> {
    /// Return the complete permission with the consumer's context loan.
    pub fn lend<'p, S, X: LoanedContext<'p, Path::Identity, Owned = C>>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> ConfigurePermission<'p, R, Path, X> {
        ConfigurePermission {
            slot: loan.lend_slot(self.slot),
            context: X::restore(PortContext::from_owned(self.context, loan)),
        }
    }
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
impl<R: InterfaceRole, Path: PoolPath> InterfacePermissions<'static, R, Path> {
    /// Lend the original permission without minting a replacement.
    pub fn lend<'p, S>(
        self,
        loan: &BodyLoan<'p, Path::Identity, S>,
    ) -> InterfacePermissions<'p, R, Path> {
        InterfacePermissions {
            configure: self.configure.lend(loan),
        }
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
