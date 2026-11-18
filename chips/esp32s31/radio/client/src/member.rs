//! PHY membership as a typestate, and the RF epoch.

use core::marker::PhantomData;

use crate::Client;

/// A client's membership of the PHY domain in state `S`.
///
/// It is affine: it cannot be copied, cloned or built outside the actor.
/// [`Untracked`] permits nothing but `track` and `leave`; only [`Tracked`]
/// permits RF use.
///
/// A [`Tracked`] member holds what the actor tuned it to
/// ([`Client::Tuned`]); the actor alone sets it, at `track` and at every
/// successful `retune`, and reads it back for the requests that need it.
///
/// ```compile_fail
/// use oer_chips_esp32s31_radio_client::{Member, Untracked, wifi::Wifi};
/// fn forge() -> Member<Wifi, Untracked> {
///     Member { client: core::marker::PhantomData, held: () }
/// }
/// ```
///
/// Nothing outside the actor's mint names a tracked member's tuned value:
/// ```compile_fail
/// use oer_chips_esp32s31_radio_client::{Member, Tracked, wifi::Wifi};
/// fn tuned(member: &Member<Wifi, Tracked>) { let _ = member.held; }
/// ```
#[must_use = "a membership must be left through the actor"]
pub struct Member<K: Client, S: State<K>> {
    client: PhantomData<fn() -> (K, S)>,
    held: S::Held,
}

/// What a member holds in each state.
pub trait State<K: Client>: crate::sealed::Sealed {
    type Held: Send;
}

impl<K: Client> State<K> for Untracked {
    type Held = ();
}

impl<K: Client> State<K> for Tracked {
    type Held = K::Tuned;
}

impl crate::sealed::Sealed for Untracked {}
impl crate::sealed::Sealed for Tracked {}

impl<K: Client, S: State<K>> core::fmt::Debug for Member<K, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Member")
    }
}

/// A member whose initial tracking has not run.
#[derive(Debug)]
pub enum Untracked {}

/// A member whose tracking is not overdue: it may use RF.
#[derive(Debug)]
pub enum Tracked {}

/// A member in either state, as `leave` takes it.
#[derive(Debug)]
pub enum Leaving<K: Client> {
    Untracked(Member<K, Untracked>),
    Tracked(Member<K, Tracked>),
}

/// The RF epoch: the actor mints a new one at every PHY registration and
/// every RF wake, and keeps it while RF stays open.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RfEpoch(u32);

impl RfEpoch {
    /// The epoch's number, for diagnostics.
    pub const fn get(self) -> u32 {
        self.0
    }
}

/// The actor's capability to mint and retire memberships and epochs.
pub struct MemberMint(());

impl MemberMint {
    pub(crate) const fn new() -> Self {
        Self(())
    }

    /// A new member of `K`.
    pub fn untracked<K: Client>(&self) -> Member<K, Untracked> {
        Member {
            client: PhantomData,
            held: (),
        }
    }

    /// The member, after its initial tracking tuned it to `tuned`.
    pub fn track<K: Client>(
        &self,
        member: Member<K, Untracked>,
        tuned: K::Tuned,
    ) -> Member<K, Tracked> {
        let Member {
            client: _,
            held: (),
        } = member;
        Member {
            client: PhantomData,
            held: tuned,
        }
    }

    /// The member, after a successful retune tuned it to `tuned`.
    pub fn retune<K: Client>(
        &self,
        member: Member<K, Tracked>,
        tuned: K::Tuned,
    ) -> Member<K, Tracked> {
        let Member { client: _, held: _ } = member;
        Member {
            client: PhantomData,
            held: tuned,
        }
    }

    /// What the actor last tuned `member` to.
    pub fn tuned<K: Client>(&self, member: &Member<K, Tracked>) -> K::Tuned {
        member.held
    }

    /// End a membership.
    pub fn retire<K: Client>(&self, member: Leaving<K>) {
        match member {
            Leaving::Untracked(Member {
                client: _,
                held: (),
            })
            | Leaving::Tracked(Member { client: _, held: _ }) => {}
        }
    }

    /// The epoch after `previous`, or the first.
    pub fn next_epoch(&self, previous: Option<RfEpoch>) -> RfEpoch {
        RfEpoch(previous.map_or(0, |epoch| epoch.0.wrapping_add(1)))
    }
}
