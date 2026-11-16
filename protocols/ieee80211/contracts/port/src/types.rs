//! The types a data port's owners and terminals carry ([`PortTypes`]),
//! and the values that move through it: tokens, attempts, keys and
//! terminals.
//!
//! # Tokens
//!
//! Every token carries the install's brand `'id`, is minted once by the
//! install's resources and travels by value: an operation takes it, and
//! the terminal that ends the operation gives it back. So a portable core
//! holds the tokens of what it may do, and what it holds is what it may
//! do: a transmission it has no ticket for, a key it has no slot for or a
//! gate it has not closed does not compile.
//!
//! | Token | Taken by | Given back by |
//! | --- | --- | --- |
//! | a transmit ticket ([`PortTypes::TicketKind`]) | [`DataPort::submit`](crate::DataPort::submit) | [`LowerMacTerminal::TxCompleted`] |
//! | a transmit buffer of a class ([`TxBufferClass`]), its memory ([`TxBufferMemory`]) | [`DataPort::submit`](crate::DataPort::submit), together with its [`ClassFrame`] | [`LowerMacTerminal::TxCompleted`], every buffer of the attempt |
//! | a key slot of a role's kind | [`PortCommands`](crate::PortCommands)' installations (the installed key, returned by its installation terminal, holds it) | [`LowerMacTerminal::KeyRemoved`], after a removal |
//! | one GTK mutation permission per interface ([`KeyTypes::GroupPermission`]) | install/replace/select/remove GTK | the corresponding FIFO success terminal |
//! | one configure permission per interface ([`crate::ConfigurePermission`]) | [`PortCommand::Configure`](crate::PortCommand::Configure) | [`LowerMacTerminal::Configured`], with the same permission and opaque context |
//! | a Block Ack slot | [`DataPort::add_block_ack`](crate::DataPort::add_block_ack) (the agreement holds it) | [`DataPort::remove_block_ack`](crate::DataPort::remove_block_ack) |
//! | the open gate ([`GateOpen`]) | [`PortCommand::CloseGate`](crate::PortCommand::CloseGate) | [`DataPort::open_gate`](crate::DataPort::open_gate), which takes the closed gate ([`GateClosed`]) that [`LowerMacTerminal::GateClosed`] gave |
//! | a received frame's buffer | [`DataPort::receive`](crate::DataPort::receive) | [`DataPort::recycle`](crate::DataPort::recycle) |
//!
//! A ticket and a buffer are different tokens.
//! A ticket is a place in flight and a place for its terminal; their
//! number is bounded by the hardware. A buffer is memory of one class,
//! sized by the composition, from a pool the caller holds: the caller
//! encodes its frame straight into it. A submission takes one ticket and
//! the buffer of its frame, and its terminal gives both back. So a
//! submission never waits: an empty pool is the caller's own capacity,
//! and what the hardware cannot take at once (an occupied queue) the
//! backend keeps until it can. Queue permits are the backend's own and
//! never reach a caller. How many tickets and buffers each role holds,
//! with a reserve for its management frames, is the composition's
//! choice: the tokens are values it hands out.
//!
//! # Protected frames
//!
//! Preparation derives the Protected bit from [`TxProtection`] and
//! leaves the 8 octets of its CCMP header after its MAC header; its length
//! counts them, not the MIC. At the first transmission of the MPDU the
//! backend writes the CCMP header with the caller-allocated packet number into
//! the buffer, and the hardware appends the MIC; the plan's transmissions
//! keep it. The completion gives the buffer back as `Sent`
//! ([`TxBufferPool::Sent`]): a retransmission of the same MPDU
//! ([`TxSource::Retransmission`]) keeps its packet number, and only a
//! renewed buffer ([`TxBufferPool::renew`]) is numbered anew. A received
//! frame whose crypto status is
//! [`RxCryptoStatus::DecryptedAndIntegrityVerified`](crate::RxCryptoStatus)
//! keeps its CCMP header, for the receiver's replay check, and loses its
//! MIC.
//!
//! # Compile-fail tests
//!
//! No caller mints a token: a gate, a ticket or a buffer comes only from
//! the install's resources.
//!
//! ```compile_fail,E0432
//! use oer_protocols_ieee80211_contracts_port::GatePlace;
//! ```
//!
//! Nor copies one:
//!
//! ```compile_fail,E0382
//! # use oer_protocols_ieee80211_contracts_port::GateOpen;
//! fn twice<'id>(gate: GateOpen<'id>) -> (GateOpen<'id>, GateOpen<'id>) {
//!     (gate, gate)
//! }
//! ```
//!
//! ```compile_fail,E0599
//! # use oer_protocols_radio_port::Slot;
//! fn copy<'id, K>(ticket: &Slot<'id, K>) -> Slot<'id, K> {
//!     ticket.clone()
//! }
//! ```
//!
//! Its compiling twin moves the token on, as an operation takes it:
//!
//! ```
//! # use oer_protocols_ieee80211_contracts_port::{DataPort, GateClosed, GateOpen, PortTypes, PortCommand};
//! fn close<'id, P: DataPort<'id>>(
//!     gate: GateOpen<'id, <P::Types as PortTypes<'id>>::GatePath>,
//! ) -> PortCommand<'id, P::Types, P::Caps> {
//!     PortCommand::CloseGate(gate)
//! }
//! fn open<'id, P: DataPort<'id>>(port: &mut P, gate: GateClosed<'id, <P::Types as PortTypes<'id>>::GatePath>) -> GateOpen<'id, <P::Types as PortTypes<'id>>::GatePath> {
//!     port.open_gate(gate)
//! }
//! ```
//!
//! The open gate does not open a gate:
//!
//! ```compile_fail,E0308
//! # use oer_protocols_ieee80211_contracts_port::{DataPort, GateOpen, PortTypes};
//! fn reopen<'id, P: DataPort<'id>>(port: &mut P, gate: GateOpen<'id, <P::Types as PortTypes<'id>>::GatePath>) {
//!     let _ = port.open_gate(gate);
//! }
//! ```
//!
//! A frame a buffer's class does not fit has no [`ClassFrame`], whose
//! fields no caller sets:
//!
//! ```compile_fail,E0451
//! # use oer_protocols_ieee80211_contracts_port::{ClassFrame, TxFrame};
//! fn forge<'t, C, K>(frame: TxFrame<'t, K>) -> ClassFrame<'t, C, K> {
//!     ClassFrame { frame, class: core::marker::PhantomData }
//! }
//! ```
//!
//! Its compiling twin checks the frame against the class:
//!
//! ```
//! # use oer_protocols_ieee80211_contracts_port::{ClassFrame, Ieee80211Capabilities, TxBufferClass, TxFrame};
//! fn fit<'t, C: TxBufferClass, K: Ieee80211Capabilities>(
//!     frame: TxFrame<'t, K>,
//! ) -> Result<ClassFrame<'t, C, K>, TxFrame<'t, K>> {
//!     ClassFrame::new(frame)
//! }
//! ```
//!
//! An attempt seals its context, ticket and buffer once:
//!
//! ```compile_fail,E0382
//! # use oer_protocols_ieee80211_contracts_port::*;
//! # use oer_protocols_radio_port::PoolPath;
//! fn twice<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath>(attempt: Attempt<'id, T, M, K, X, Path>)
//! where M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X> {
//!     let _ = attempt.admit();
//!     let _ = attempt.admit();
//! }
//! ```
//!
//! Its compiling twin seals each attempt once, with no `Clone` bound:
//!
//! ```
//! # use oer_protocols_ieee80211_contracts_port::*;
//! # use oer_protocols_radio_port::PoolPath;
//! fn one<'id, T, M: PreparedTx<'id>, K, X, Path: PoolPath>(attempt: Attempt<'id, T, M, K, X, Path>)
//! where M::Types: PortTypes<'id, TicketKind = T, TicketPath = Path, Context = X> {
//!     let _ = attempt.admit();
//! }
//! ```

use oer_protocols_radio_port::{PoolPath, Root, Slot};

use crate::{
    Protection, TxOperation,
    owner::{
        AccessPoint, Ieee80211Capabilities, InterfaceRole, PortRate, Station, SupportedPower,
        TxFrame,
    },
};

enum GateKind {}

/// The gate component of a lower-MAC backend's pool set.
///
/// The backend receives its open gate and issuer together when its one
/// [`oer_protocols_radio_port::Mint`] is consumed. The unique place remains private.
///
/// ```compile_fail,E0432
/// use oer_protocols_ieee80211_contracts_port::GatePlace;
/// ```
///
/// ```
/// use oer_protocols_ieee80211_contracts_port::{GateIssuer, GateOpen, GatePool};
/// use oer_protocols_radio_port::Mint;
/// fn gate<'id>(mint: Mint<'id, GatePool>) -> (GateOpen<'id>, GateIssuer<'id>) {
///     mint.into_pools()
/// }
/// ```
pub struct GatePool;

impl oer_protocols_radio_port::PoolSet for GatePool {
    type Pools<'id, Path: PoolPath> = (GateOpen<'id, Path>, GateIssuer<'id, Path>);

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
    ) -> (GateOpen<'id, Path>, GateIssuer<'id, Path>) {
        (
            GateOpen {
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

impl<Path: PoolPath> GateOpen<'static, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn lend<'p, S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateOpen<'p, Path> {
        GateOpen {
            place: GatePlace {
                slot: loan.lend_slot(self.place.slot),
            },
        }
    }
}

impl<'p, Path: PoolPath> GateOpen<'p, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn into_owned<S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateOpen<'static, Path> {
        GateOpen {
            place: GatePlace {
                slot: loan.reclaim_slot(self.place.slot),
            },
        }
    }
}

impl<Path: PoolPath> GateClosed<'static, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn lend<'p, S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateClosed<'p, Path> {
        GateClosed {
            place: GatePlace {
                slot: loan.lend_slot(self.place.slot),
            },
        }
    }
}

impl<'p, Path: PoolPath> GateClosed<'p, Path> {
    /// Move the same gate state across its backend's private body boundary.
    pub fn into_owned<S>(
        self,
        loan: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> GateClosed<'static, Path> {
        GateClosed {
            place: GatePlace {
                slot: loan.reclaim_slot(self.place.slot),
            },
        }
    }
}

/// One gate and its arbiter's unique construction authority.
///
/// The installation's mint builds this component once. Returning a gate
/// after a cycle does not mint another arbiter or another pair of lease places.
pub struct ArbiterPool;

impl oer_protocols_radio_port::PoolSet for ArbiterPool {
    type Pools<'id, Path: PoolPath> = (
        GateOpen<'id, Path>,
        GateIssuer<'id, Path>,
        ArbiterAuthority<'id, Path>,
    );

    fn build<'id, Path: PoolPath>(
        builder: oer_protocols_radio_port::PoolBuilder<'id, Path>,
    ) -> Self::Pools<'id, Path> {
        let (open, issuer) = builder.into_pools::<GatePool>();
        (
            open,
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

impl<Path: PoolPath> ArbiterAuthority<'static, Path> {
    /// Lend the one construction authority without copying or reissuing it.
    pub fn lend<'p, S>(
        self,
        _: &oer_protocols_radio_port::BodyLoan<'p, Path::Identity, S>,
    ) -> ArbiterAuthority<'p, Path> {
        ArbiterAuthority {
            brand: core::marker::PhantomData,
        }
    }
}
