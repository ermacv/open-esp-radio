//! The installation's transitions: the proofs a port gives at each one and
//! takes at the next.
//!
//! An installation of a port goes through start, stop, clean, retire, and
//! then reinstall or release ([`DataPort`](crate::DataPort)). A call's
//! answer is its acceptance point: before it, a refusal that leaves the
//! port and every proof as they were and names what blocks it; after it,
//! the next state's proof.
//!
//! A proof is one word: the installation's [`InstallationGeneration`], under the brand
//! `'id` of the port's installations. It is affine (neither `Copy` nor
//! `Clone`), and every transition takes the proof of the one before it by
//! value or by reference, so a proof does not outlive the state it proves:
//! a start takes the stop's proof ([`StartPoll`]), a retirement the stop's,
//! a reinstall or a release the retirement's. Only a port issues a proof;
//! a port compares each proof it is given with its own generation and
//! answers one of another installation with [`StaleProof`], the one check
//! the types leave.

use core::marker::PhantomData;
use core::num::NonZeroU32;

/// An installation's place in the count of installations the port made,
/// from one.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InstallationGeneration(NonZeroU32);

impl InstallationGeneration {
    /// The generation numbered `generation`.
    pub const fn new(generation: NonZeroU32) -> Self {
        Self(generation)
    }

    /// Its number.
    pub const fn get(self) -> NonZeroU32 {
        self.0
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
            generation: InstallationGeneration,
            brand: Brand<'id>,
        }

        impl<'id> $name<'id> {
            /// The proof of installation `generation`. Only the port that
            /// made that installation issues it, at the transition it
            /// proves.
            pub const fn issue(generation: InstallationGeneration) -> Self {
                Self {
                    generation,
                    brand: PhantomData,
                }
            }

            /// The installation it proves the state of.
            pub const fn generation(&self) -> InstallationGeneration {
                self.generation
            }
        }
    };
}

proof! {
    /// The installation's radio stopped and nothing started it since: a
    /// port gives it at the installation (an installation starts stopped)
    /// and at a confirmed stop, and takes it at a start, a clean, a take of
    /// what a sink refused and a retirement.
    Stopped
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

/// A proof a port did not take: it names installation `proof`, which is
/// not the port's (`installation`). Nothing changed.
#[derive(Debug, Eq, PartialEq)]
pub struct StaleProof<P> {
    /// The proof, unchanged.
    pub proof: P,
    /// The port's installation.
    pub installation: InstallationGeneration,
}

/// The answer of a start's poll, which takes the stop's proof.
#[derive(Debug, Eq, PartialEq)]
pub enum StartPoll<'id, R> {
    /// The radio runs: the stop's proof ended with the start.
    Started,
    /// The start goes on: the proof, for the next poll.
    Pending(Stopped<'id>),
    /// The start was refused and the radio is stopped: the proof, with why.
    Refused(Stopped<'id>, R),
    /// The start began on the device and failed, and the stop of what it
    /// began is not confirmed (`R` names the cause): the port is stopping,
    /// and its stop gives a new proof.
    Unstopped(R),
}

/// A stop that did not confirm: why, and everything the port still has
/// out (`O`, its [`Outstanding`](crate::Outstanding)).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StopUnconfirmed<F, O> {
    /// Why the stop ended unconfirmed.
    pub cause: crate::StopCause<F>,
    /// What the port has out.
    pub outstanding: O,
}

/// What a stop can do about a port's fault, as
/// [`DataPort::recover_for_stop`](crate::DataPort::recover_for_stop)
/// answers it.
#[derive(Debug, Eq, PartialEq)]
pub enum Recovery<'id, F, O> {
    /// The port holds no fault: there is nothing to take back.
    Clear,
    /// The fault leaves every owner whole: the port took it back, and its
    /// stop returns them.
    Recovered(Recovering<'id>),
    /// The fault holds owners no stop returns: the fault, and what it
    /// leaves out (`O`). The port keeps it.
    Kept(F, O),
}

/// The classes of what an installation has out, read whole: what a
/// conformance ledger compares with its own count.
pub trait OutstandingClasses {
    /// The classes that are out, one bit each
    /// ([`outstanding::class`](crate::outstanding::class)).
    fn classes(&self) -> u32;

    /// The count of `class` (one bit of
    /// [`outstanding::class`](crate::outstanding::class)): units, keys,
    /// items, buffers or attempts; one for an out class without a count
    /// (the phase, the fault, the arenas), zero for a class not out.
    fn count(&self, class: u32) -> usize;
}

impl<F, const QUEUES: usize> OutstandingClasses for crate::Outstanding<F, QUEUES> {
    fn classes(&self) -> u32 {
        crate::Outstanding::classes(self)
    }

    fn count(&self, class: u32) -> usize {
        use crate::outstanding::class as c;
        let flag = |out: bool| usize::from(out);
        match class {
            c::PHASE => flag(self.phase != crate::RunPhase::Stopped),
            c::FAULT => flag(self.fault.is_some()),
            c::RECEIVED => self.received,
            c::REMOVALS => self.removals,
            c::RETAINED_GROUPS => self.retained_groups,
            c::UNCONSUMED => self.unconsumed,
            c::TRANSMIT_OUT => self.transmit_out,
            c::QUEUED => self.queued.iter().sum(),
            c::RUNNING => self.running,
            c::ARENAS => flag(self.arenas != crate::ArenaState::Returnable),
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn first() -> InstallationGeneration {
        InstallationGeneration::new(NonZeroU32::MIN)
    }

    #[test]
    fn a_proof_is_one_word_and_names_its_installation() {
        assert_eq!(core::mem::size_of::<Stopped<'static>>(), 4);
        assert_eq!(core::mem::size_of::<Retired<'static>>(), 4);
        assert_eq!(Stopped::issue(first()).generation(), first());
    }

    #[test]
    fn a_class_counts_what_is_out_and_nothing_else() {
        use crate::outstanding::class;
        let mut outstanding = crate::Outstanding::<(), 2> {
            phase: crate::RunPhase::Stopped,
            fault: None,
            received: 0,
            lent: 3,
            removals: 0,
            retained_groups: 0,
            unconsumed: 0,
            transmit_out: 0,
            queued: [0; 2],
            running: 0,
            arenas: crate::ArenaState::Returnable,
        };
        assert_eq!(OutstandingClasses::classes(&outstanding), 0);
        outstanding.queued = [2, 1];
        outstanding.fault = Some(());
        assert_eq!(outstanding.count(class::QUEUED), 3);
        assert_eq!(outstanding.count(class::FAULT), 1);
        assert_eq!(outstanding.count(class::RECEIVED), 0);
    }
}
