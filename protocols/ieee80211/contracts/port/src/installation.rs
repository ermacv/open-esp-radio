//! The installation's transitions: the proofs a port gives at each one and
//! takes at the next.
//!
//! An installation of a port goes through start, stop, clean, retire, and
//! then reinstall or release ([`DataPort`](crate::DataPort)). A call's
//! answer is its acceptance point: before it, a refusal that leaves the
//! port and every proof as they were and names what blocks it; after it,
//! the next state's proof.
//!
//! A proof is one word: the port's [`StateSerial`] at its issue, under the
//! brand `'id` of the port's installations. It is affine (neither `Copy`
//! nor `Clone`), and every transition takes the proof of the one before it
//! by value or by reference: a start takes the stop's proof
//! ([`StartPoll`]), a retirement the stop's, a reinstall or a release the
//! retirement's. The port raises its serial at every transition that ends
//! a proof's state (a start beginning and ending, a stop confirming, a
//! recovery of a running radio, a retirement, a reinstall, a release) and
//! takes a proof only when it equals its serial: otherwise
//! [`StaleProof`], nothing changed. That is the one check the types leave,
//! the same in every call; no call reads the port's state to decide
//! whether a proof holds. A second copy of a proof (a stopped port answers
//! every [`poll_stop`](crate::DataPort::poll_stop) with its stop's proof)
//! is the same proof, and stale with it once the port moved. A proof of
//! another serial is refused; who may issue one (a port's implementation
//! and tests) is held by the layer tests' scan, not by this module.

use core::marker::PhantomData;

/// The serial of a port's state: raised at every transition that ends the
/// state a proof proves. It has no reader: a port compares a proof with
/// its own ([`Stopped::is_of`]). It counts in 32 bits and wraps: a proof
/// held across 2^32 transitions of its port is the one a wrapped serial
/// could take again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StateSerial(u32);

impl StateSerial {
    /// A port's serial at its first installation.
    pub const FIRST: Self = Self(0);

    /// The serial after one more transition.
    #[must_use]
    pub const fn raised(self) -> Self {
        Self(self.0.wrapping_add(1))
    }
}

/// The brand of a port's installations: invariant in `'id`.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

macro_rules! proof {
    ($(#[$doc:meta])* $name:ident) => {
        $(#[$doc])*
        #[must_use = "a proof is the only way to the next transition"]
        #[derive(Debug, Eq, PartialEq)]
        pub struct $name<'id> {
            serial: StateSerial,
            brand: Brand<'id>,
        }

        impl<'id> $name<'id> {
            /// The proof of the port's state at `serial`. A port issues it
            /// at the transition it proves; the layer tests hold that only
            /// a port's implementation and tests call it.
            pub const fn issue(serial: StateSerial) -> Self {
                Self {
                    serial,
                    brand: PhantomData,
                }
            }

            /// Whether it is the proof of the port's state at `serial`: the
            /// port's one check of a proof.
            pub fn is_of(&self, serial: StateSerial) -> bool {
                self.serial == serial
            }
        }
    };
}

proof! {
    /// The installation's radio stopped and nothing moved the port since:
    /// a port gives it at the installation (an installation starts
    /// stopped), at a confirmed stop and at a refused start, and takes it
    /// at a start, a clean, a take of what a sink refused and a
    /// retirement.
    Stopped
}

proof! {
    /// A start in progress: the port gives it when the start goes on
    /// ([`StartPoll::Pending`]) and takes it at the next poll of the start
    /// alone. It is no stop's proof, so nothing else takes it.
    Starting
}

proof! {
    /// The stopped installation holds nothing that blocks its end: a port
    /// gives it at a retirement and takes it at a reinstall or a release.
    Retired
}

proof! {
    /// A fault the port took back so that a stop returns its owners: from
    /// then on the port admits and publishes nothing new, and only its
    /// service of what is out goes on.
    Recovering
}

/// What a start's poll takes: the stop's proof to begin, or the proof of
/// the start in progress to go on.
#[derive(Debug, Eq, PartialEq)]
pub enum StartProof<'id> {
    /// Begin the start.
    Stopped(Stopped<'id>),
    /// Go on with the start.
    Starting(Starting<'id>),
}

impl<'id> StartProof<'id> {
    /// Whether it is the proof of the port's state at `serial`.
    pub fn is_of(&self, serial: StateSerial) -> bool {
        match self {
            Self::Stopped(proof) => proof.is_of(serial),
            Self::Starting(proof) => proof.is_of(serial),
        }
    }
}

impl<'id> From<Stopped<'id>> for StartProof<'id> {
    fn from(proof: Stopped<'id>) -> Self {
        Self::Stopped(proof)
    }
}

impl<'id> From<Starting<'id>> for StartProof<'id> {
    fn from(proof: Starting<'id>) -> Self {
        Self::Starting(proof)
    }
}

/// A proof a port did not take: the port moved since its issue, or it is
/// another installation's. Nothing changed.
#[derive(Debug, Eq, PartialEq)]
pub struct StaleProof<P> {
    /// The proof, unchanged.
    pub proof: P,
}

/// A call that takes proof `P` and did not make its transition: the proof
/// is stale, or the port refused for `R`. Either way the
/// proof comes back, and nothing changed.
#[derive(Debug, Eq, PartialEq)]
pub enum ProofRefused<P, R> {
    /// The proof is not of the port's state.
    Stale(StaleProof<P>),
    /// The port refused the transition for `R`.
    Refused(P, R),
}

impl<P, R> ProofRefused<P, R> {
    /// The proof, back.
    pub fn proof(self) -> P {
        match self {
            Self::Stale(stale) => stale.proof,
            Self::Refused(proof, _) => proof,
        }
    }
}

/// The answer of a start's poll, which takes the stop's proof to begin and
/// the start's to go on. `F` is the port's fault.
#[derive(Debug, Eq, PartialEq)]
pub enum StartPoll<'id, F> {
    /// The radio runs: the start's proof ended with the start.
    Started,
    /// The start goes on: its proof, for the next poll. It carries the
    /// serial the start's beginning raised, and it is no [`Stopped`], so
    /// no retirement, clean or take can be asked with it.
    Pending(Starting<'id>),
    /// The start failed before it began on the device, or its stop
    /// confirmed: the radio is stopped, and its new stop's proof comes
    /// with why.
    Refused(Stopped<'id>, crate::StartFailure<F>),
    /// The start began on the device or the MAC and failed for `cause`, and
    /// the stop of what it began is not confirmed (`stop`): the port is
    /// stopping, and its stop gives a new proof.
    Unstopped {
        /// Why the start failed.
        cause: crate::StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: StopUnconfirmed<F>,
    },
    /// The proof is not of the port's state: nothing started.
    Stale(StaleProof<StartProof<'id>>),
}

/// Why a reinstall installed nothing; the retirement's proof comes back,
/// and every refusal but [`Self::Resources`] leaves the port as it was.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ReinstallRefused {
    /// The next installation's TSF epoch repeats the current one.
    SameEpoch,
    /// The port's channel names no channel it installs on.
    ChannelUnset,
    /// The installations refused the receipt (another installation's, or
    /// their generations are used up): the port stays as it was.
    Receipt,
    /// The retired installation kept a resource the next one needs: the
    /// port keeps its fault.
    Resources,
}

/// Why a start did not leave the radio running, as the layers above the
/// port carry it once the stop's proof came back ([`StartPoll`]): `F` is
/// the port's fault, `O` its [`Outstanding`](crate::Outstanding).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartRefused<F> {
    /// The radio runs, or its stop has not confirmed: the caller holds no
    /// stop's proof to start with, so no start was asked of the port.
    Running,
    /// The radio front end did not tune the channel, for the port's cause.
    Retune(F),
    /// The start did not end within the backend's command bound.
    Deadline,
    /// The device refused the start.
    Device,
    /// The start began and failed for `cause`, and the stop of what it
    /// began did not confirm (`stop`): the port is stopping.
    Unstopped {
        /// Why the start failed.
        cause: crate::StartFailure<F>,
        /// What the stop of the begun start observed.
        stop: StopUnconfirmed<F>,
    },
}

impl<F> From<crate::StartFailure<F>> for StartRefused<F> {
    fn from(failure: crate::StartFailure<F>) -> Self {
        match failure {
            crate::StartFailure::Retune(cause) => Self::Retune(cause),
            crate::StartFailure::Deadline => Self::Deadline,
            crate::StartFailure::Device => Self::Device,
        }
    }
}

/// A stop that did not confirm: why, and everything the port still has
/// out (`O`, its [`Outstanding`](crate::Outstanding)).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopUnconfirmed<F> {
    /// Why the stop ended unconfirmed.
    pub cause: crate::StopCause<F>,
    /// What the port has out.
    pub outstanding: crate::Outstanding<F>,
}

/// What a stop can do about a port's fault, as
/// [`DataPort::recover_for_stop`](crate::DataPort::recover_for_stop)
/// answers it.
#[derive(Debug, Eq, PartialEq)]
pub enum Recovery<'id, F> {
    /// The port holds no fault: there is nothing to take back.
    Clear,
    /// The fault leaves every owner whole: the port took it back, and its
    /// stop returns them.
    Recovered(Recovering<'id>),
    /// The fault holds owners no stop returns: the fault, and what it
    /// leaves out (`O`). The port keeps it.
    Kept(F, crate::Outstanding<F>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_proof_is_one_word_and_holds_at_its_serial_alone() {
        assert_eq!(core::mem::size_of::<Stopped<'static>>(), 4);
        assert_eq!(core::mem::size_of::<Retired<'static>>(), 4);
        let first = StateSerial::FIRST;
        let proof = Stopped::issue(first);
        assert!(proof.is_of(first));
        assert!(!proof.is_of(first.raised()));
        // The serial wraps: the one after the last is the first.
        assert_eq!(StateSerial(u32::MAX).raised(), first);
    }

    #[test]
    fn a_class_counts_what_is_out_and_nothing_else() {
        use crate::outstanding::class;
        let mut outstanding = crate::Outstanding::<()> {
            phase: crate::RunPhase::Stopped,
            fault: None,
            received: 0,
            lent: 3,
            removals: 0,
            retained_groups: 0,
            unconsumed: 0,
            transmit_out: 0,
            queued: [0; crate::ACCESS_CATEGORIES],
            running: 0,
            arenas: crate::ArenaState::Returnable,
        };
        assert_eq!(outstanding.classes(), 0);
        outstanding.queued = [2, 1, 0, 0];
        assert_eq!(
            outstanding.queued_in(oer_protocols_ieee80211_wire::qos::WmmAccessCategory::Background),
            1
        );
        outstanding.fault = Some(());
        assert_eq!(outstanding.count(class::QUEUED), 3);
        assert_eq!(outstanding.count(class::FAULT), 1);
        assert_eq!(outstanding.count(class::RECEIVED), 0);
    }
}
