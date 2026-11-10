//! An installation that did not end in a completed retire: everything it
//! held, for its external owner.
//!
//! A Wi-Fi installation (the station's or the access point's) ends either
//! as a completed retire (its radio's stop confirmed, the port cleaned and
//! the radio released, every owner ended with the retired installation),
//! or as an [`Unfinished`] installation: the actor or runner that owns the
//! port, the items a port call could not keep, an attempt a call returned
//! unaccepted, the cleanup's removal receipt, the cause and the phase the
//! end reached. Nothing of it is dropped on the way: the composition
//! returns it to its caller, the application, which holds it. The
//! station's installation has two halves, both returned: the datapath
//! actor's (this value) and the station lifecycle's (its retired
//! lifecycle: station, places, arbiter, issuer and contexts).
//!
//! The lifecycle's half leaves the place its lifecycle ran in before that
//! place is reused: a half still held there keeps the place borrowed, so
//! the next installation cannot take it.
//!
//! ```compile_fail,E0499
//! use core::mem::MaybeUninit;
//! use oer_portable_memory::Placed;
//! let mut place = MaybeUninit::<u32>::uninit();
//! let half = Placed::init(&mut place, |p| p.write(1));
//! // The next installation, while the unfinished half is in the place.
//! let next = Placed::init(&mut place, |p| p.write(2));
//! drop((half, next));
//! ```

/// How far the end of an unfinished installation came.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    /// The run ended before its radio's stop: the actor's or the runner's
    /// own end (a fault, a broken contract, an item a port call could not
    /// keep). The radio's stop was not asked for.
    Running,
    /// The radio's stop was asked for and is not confirmed: the run ended
    /// during the stop (the actor's fault, an unconfirmed stop or retune,
    /// the lifecycle's end), or the access point's stop watch returned with
    /// an item a port call could not keep and its home had no room for it.
    Stopping,
    /// The radio's stop is confirmed and no release ran: items a port call
    /// could not keep are out, which the release would retire with the
    /// port.
    Stopped,
    /// The station's restart stopped the radio: the stopped port's cleanup
    /// or its reinstall refused, and no release ran.
    Reinstalling,
    /// The radio's release refused or its posted leave ended unanswered:
    /// the port keeps its radio, or what is left of it.
    Releasing,
}

impl Phase {
    /// The phase of an actor's end that its radio read
    /// ([`oer_protocols_ieee80211_runtime::datapath::Actor::radio_phase`]): a stop
    /// asked for and not confirmed is [`Self::Stopping`], whatever ended
    /// the actor.
    pub const fn of_radio(radio: oer_protocols_ieee80211_runtime::datapath::RadioPhase) -> Self {
        use oer_protocols_ieee80211_runtime::datapath::RadioPhase;
        match radio {
            RadioPhase::Running => Self::Running,
            RadioPhase::Stopping => Self::Stopping,
            RadioPhase::Stopped => Self::Stopped,
        }
    }
}

/// An installation that did not end in a completed retire, whole.
///
/// `O` is the actor or runner, which owns the port and every owner it did
/// not hand out; `K` the items a port call could neither give it nor keep;
/// `U` an attempt a call returned unaccepted beside such an item; `R` the
/// stopped port's cleanup receipt; `C` the cause.
#[must_use = "an unfinished installation holds the port and its owners"]
pub struct Unfinished<O, K, U, R, C> {
    /// The actor or runner, with its port.
    pub owner: O,
    /// The items a port call could not keep.
    pub unkept: K,
    /// The attempt a call returned unaccepted.
    pub unaccepted: U,
    /// The stopped port's cleanup receipt, when a cleanup ran.
    pub receipt: Option<R>,
    /// Why the installation did not retire.
    pub cause: C,
    /// How far its end came.
    pub phase: Phase,
}

/// Names the cause and the phase, not the owners, which carry no `Debug`.
impl<O, K, U, R, C: core::fmt::Debug> core::fmt::Debug for Unfinished<O, K, U, R, C> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Unfinished")
            .field("cause", &self.cause)
            .field("phase", &self.phase)
            .field("receipt", &self.receipt.is_some())
            .finish_non_exhaustive()
    }
}

/// How many items a port call could not keep the access point's retire
/// holds while it goes on with the stop: a chosen bound, not a hardware
/// value. A further item ends the retire unfinished in
/// [`Phase::Stopping`], every item with it.
pub const UNKEPT_ITEMS: usize = 4;
