//! PHY membership as a typestate, and the RF epoch.

use core::marker::PhantomData;

/// A client's membership of the PHY domain in state `S`.
///
/// It is affine: it cannot be copied, cloned or built outside the actor.
/// [`Untracked`] permits nothing but `track` and `leave`; only [`Tracked`]
/// permits RF use.
///
/// ```compile_fail
/// use oer_esp32s31_radio_client::{Member, Untracked, wifi::Wifi};
/// fn forge() -> Member<Wifi, Untracked> {
///     Member { client: core::marker::PhantomData }
/// }
/// ```
#[must_use = "a membership must be left through the actor"]
pub struct Member<K, S> {
    client: PhantomData<fn() -> (K, S)>,
}

impl<K, S> core::fmt::Debug for Member<K, S> {
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
pub enum Leaving<K> {
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
    pub fn untracked<K>(&self) -> Member<K, Untracked> {
        Member {
            client: PhantomData,
        }
    }

    /// The member, after its initial tracking.
    pub fn track<K>(&self, member: Member<K, Untracked>) -> Member<K, Tracked> {
        let Member { client: _ } = member;
        Member {
            client: PhantomData,
        }
    }

    /// End a membership.
    pub fn retire<K>(&self, member: Leaving<K>) {
        match member {
            Leaving::Untracked(Member { client: _ }) | Leaving::Tracked(Member { client: _ }) => {}
        }
    }

    /// The epoch after `previous`, or the first.
    pub fn next_epoch(&self, previous: Option<RfEpoch>) -> RfEpoch {
        RfEpoch(previous.map_or(0, |epoch| epoch.0.wrapping_add(1)))
    }
}
