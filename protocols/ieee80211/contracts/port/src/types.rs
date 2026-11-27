//! The transmit gate's proofs and the arbiter's construction authority.
//!
//! The gate is closed whenever the data is stopped: an installation's mint
//! gives it closed ([`GatePool`]), the port opens it on a running radio
//! ([`DataPort::open_gate`](crate::DataPort::open_gate)) and the close-gate
//! command's end gives it closed again.

use oer_protocols_radio_port::{PoolPath, Root, Slot};

enum GateKind {}

/// The gate component of a lower-MAC backend's pool set.
///
/// The backend receives its gate, closed, and its issuer together when its
/// one [`oer_protocols_radio_port::Mint`] is consumed: the data of an
/// installation is stopped, and the gate is closed whenever the data is
/// stopped. The unique place remains private.
///
/// ```compile_fail,E0432
/// use oer_protocols_ieee80211_contracts_port::GatePlace;
/// ```
///
/// ```
/// use oer_protocols_ieee80211_contracts_port::{GateClosed, GateIssuer, GatePool};
/// use oer_protocols_radio_port::Mint;
/// fn gate<'id>(mint: Mint<'id, GatePool>) -> (GateClosed<'id>, GateIssuer<'id>) {
///     mint.into_pools()
/// }
/// ```
pub struct GatePool;

impl oer_protocols_radio_port::PoolSet for GatePool {
    type Pools<'id, Path: PoolPath> = (GateClosed<'id, Path>, GateIssuer<'id, Path>);

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        GatePlace::mint(builder.into_pools::<oer_protocols_radio_port::Slots<GateKind, 1>>())
    }
}

/// The install's unique gate place, held inside exactly one gate state.
#[must_use = "the unique gate place stays in one gate state"]
struct GatePlace<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    slot: Slot<'id, oer_protocols_radio_port::Slots<GateKind, 1>, Path>,
}

impl<'id, Path: PoolPath> GatePlace<'id, Path> {
    fn mint(
        [slot]: [Slot<'id, oer_protocols_radio_port::Slots<GateKind, 1>, Path>; 1],
    ) -> (GateClosed<'id, Path>, GateIssuer<'id, Path>) {
        (
            GateClosed {
                place: Self { slot },
            },
            GateIssuer {
                brand: core::marker::PhantomData,
            },
        )
    }
}

impl<Path: PoolPath> core::fmt::Debug for GatePlace<'_, Path> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GatePlace")
            .field("slot", &self.slot.index())
            .finish()
    }
}

/// The open transmit gate. Closing transfers its unique place to the backend.
#[derive(Debug)]
#[must_use = "close_gate consumes the open gate and returns its closed evidence"]
pub struct GateOpen<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    place: GatePlace<'id, Path>,
}

/// Evidence that the backend publishes no attempt; it does not prove quiet air.
/// Only the retained backend issuer constructs this state after its barrier.
///
/// ```compile_fail,E0616
/// # use oer_protocols_ieee80211_contracts_port::{GateClosed, GateOpen};
/// fn forge<'id>(gate: GateOpen<'id>) -> GateClosed<'id> {
///     GateClosed { place: gate.place }
/// }
/// ```
#[derive(Debug)]
#[must_use = "open_gate consumes the closed evidence and returns the open gate"]
pub struct GateClosed<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    place: GatePlace<'id, Path>,
}

impl<'id, Path: PoolPath> GateClosed<'id, Path> {
    /// The closed gate lent to a retune: it stays closed for the loan.
    pub const fn lent(&self) -> GateLent<'_, 'id> {
        GateLent {
            brand: core::marker::PhantomData,
        }
    }
}

/// The installation's closed gate, lent for as long as `'g`: a retune is
/// asked and begun under it. Only a [`GateClosed`] lends it.
///
/// ```compile_fail,E0451
/// # use oer_protocols_ieee80211_contracts_port::GateLent;
/// fn forge<'g, 'id>() -> GateLent<'g, 'id> {
///     GateLent { brand: core::marker::PhantomData }
/// }
/// ```
#[derive(Clone, Copy, Debug)]
pub struct GateLent<'g, 'id> {
    brand: core::marker::PhantomData<(&'g (), Brand<'id>)>,
}

/// The brand of one installation, invariant in `'id`.
type Brand<'id> = fn(&'id ()) -> &'id ();

/// The backend's authority to convert its one gate place between states.
///
/// One issuer is retained by the backend per install and never reaches the
/// body. Closing is allowed only after the current publication's result and
/// the withdrawal of queued attempts. Opening consumes that very evidence.
/// Backend endpoints may share this stateless authority immutably: each
/// transition still consumes the installation's one affine gate place.
///
/// ```compile_fail,E0382
/// # use oer_protocols_ieee80211_contracts_port::{GateClosed, GateIssuer, GateOpen};
/// fn both<'id>(issuer: &mut GateIssuer<'id>, gate: GateOpen<'id>)
///     -> (GateOpen<'id>, GateClosed<'id>)
/// {
///     let closed = issuer.close(gate);
///     (gate, closed)
/// }
/// ```
#[must_use = "the backend retains the issuer for the whole install"]
pub struct GateIssuer<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    brand: core::marker::PhantomData<fn(&'id (), Path) -> &'id ()>,
}

impl<'id, Path: PoolPath> GateIssuer<'id, Path> {
    /// Attest the completed hardware barrier, retaining the same place.
    pub fn close(&self, gate: GateOpen<'id, Path>) -> GateClosed<'id, Path> {
        GateClosed { place: gate.place }
    }

    /// Consume the closed evidence to open the same gate.
    pub fn open(&self, gate: GateClosed<'id, Path>) -> GateOpen<'id, Path> {
        GateOpen { place: gate.place }
    }
}

/// One gate and its arbiter's unique construction authority.
///
/// The installation's mint builds this component once. Returning a gate
/// after a cycle does not mint another arbiter or another pair of lease places.
pub struct ArbiterPool;

impl oer_protocols_radio_port::PoolSet for ArbiterPool {
    type Pools<'id, Path: PoolPath> = (
        GateClosed<'id, Path>,
        GateIssuer<'id, Path>,
        ArbiterAuthority<'id, Path>,
    );

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let (closed, issuer) = builder.into_pools::<GatePool>();
        (
            closed,
            issuer,
            ArbiterAuthority {
                brand: core::marker::PhantomData,
            },
        )
    }
}

/// The sole right to construct the arbiter of this installation's gate.
#[derive(Debug)]
#[must_use = "construct the arbiter exactly once with its initial gate"]
pub struct ArbiterAuthority<'id, Path: PoolPath = Root<oer_protocols_radio_port::LocalId<'id>>> {
    brand: core::marker::PhantomData<fn(&'id (), Path) -> (&'id (), Path)>,
}
