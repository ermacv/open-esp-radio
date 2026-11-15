//! A set over the members of one enum: interrupt causes, lanes and buses
//! that callers combine and test.
//!
//! A member's bit is its place in its enum, given by [`Set::of_place`]
//! alone; no site writes a mask. An enum becomes a set's members through
//! [`members!`], which declares it, lists its members in order and checks
//! at compile time that each has a bit.

use core::marker::PhantomData;

/// The members of one enum, in their order.
pub trait Member: Copy + 'static {
    /// Every member, in its place's order.
    const ALL: &'static [Self];

    /// The member's place in its enum.
    fn place(self) -> u8;
}

/// A set of the members of `M`.
#[derive(Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Set<M> {
    bits: u32,
    members: PhantomData<M>,
}

impl<M> Clone for Set<M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M> Copy for Set<M> {}

impl<M> Default for Set<M> {
    fn default() -> Self {
        Self::empty()
    }
}

impl<M> Set<M> {
    /// The set of no member.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            bits: 0,
            members: PhantomData,
        }
    }

    /// The set of the one member at `place`: the one function that gives a
    /// member its bit. [`members!`] proves `place` below 32 for every member
    /// at compile time, so the shift is exact.
    #[doc(hidden)]
    #[must_use]
    pub const fn of_place(place: u8) -> Self {
        Self {
            bits: 1u32.wrapping_shl(place as u32),
            members: PhantomData,
        }
    }

    /// The members of either set.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self {
            bits: self.bits | other.bits,
            members: PhantomData,
        }
    }

    /// The members of both sets.
    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self {
            bits: self.bits & other.bits,
            members: PhantomData,
        }
    }

    /// The members of this set that are not in `other`.
    #[must_use]
    pub const fn without(self, other: Self) -> Self {
        Self {
            bits: self.bits & !other.bits,
            members: PhantomData,
        }
    }

    /// Whether every member of `other` is in this set.
    #[must_use]
    pub const fn includes(self, other: Self) -> bool {
        self.bits & other.bits == other.bits
    }

    /// Whether the set has no member.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bits == 0
    }
}

impl<M: Member> Set<M> {
    /// The set of every member.
    #[must_use]
    pub const fn all() -> Self {
        Self {
            // [`members!`] declares one to 32 members: the shift is exact.
            bits: u32::MAX.wrapping_shr(32u32.wrapping_sub(M::ALL.len() as u32)),
            members: PhantomData,
        }
    }

    /// The set of `member`.
    #[must_use]
    pub fn of(member: M) -> Self {
        Self::of_place(member.place())
    }

    /// The set of `member` when `bit`, a generated reader's value, is set;
    /// else the empty set.
    #[must_use]
    pub fn of_bit(bit: bool, member: M) -> Self {
        if bit { Self::of(member) } else { Self::empty() }
    }

    /// Whether `member` is in the set.
    #[must_use]
    pub fn contains(self, member: M) -> bool {
        self.includes(Self::of(member))
    }

    /// The set's members, in their order.
    pub fn members(self) -> impl Iterator<Item = M> {
        M::ALL
            .iter()
            .copied()
            .filter(move |member| self.contains(*member))
    }
}

impl<M: Member + core::fmt::Debug> core::fmt::Debug for Set<M> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.debug_set().entries(self.members()).finish()
    }
}

impl<M> core::ops::BitOr for Set<M> {
    type Output = Self;

    fn bitor(self, other: Self) -> Self {
        self.union(other)
    }
}

/// Declare an enum whose members a [`Set`] holds: its members in order,
/// their places, their sets, and the proof that each has a bit.
macro_rules! members {
    (
        $(#[$meta:meta])*
        $vis:vis enum $name:ident {
            $($(#[$member_meta:meta])* $member:ident),+ $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        #[repr(u8)]
        $vis enum $name {
            $($(#[$member_meta])* $member),+
        }

        impl $crate::set::Member for $name {
            const ALL: &'static [Self] = &[$(Self::$member),+];

            fn place(self) -> u8 {
                self as u8
            }
        }

        impl $name {
            /// The set of this member alone.
            #[must_use]
            pub const fn set(self) -> $crate::set::Set<Self> {
                const {
                    if <$name as $crate::set::Member>::ALL.len() > 32 {
                        panic!("a set's members have one bit each of 32");
                    }
                }
                $crate::set::Set::of_place(self as u8)
            }
        }
    };
}
pub(crate) use members;

#[cfg(test)]
mod tests {
    use super::{Member, Set};

    members! {
        enum Three {
            First,
            Second,
            Third,
        }
    }

    #[test]
    fn each_member_has_its_own_bit_by_its_place() {
        let mut seen = Set::<Three>::empty();
        for (place, member) in Three::ALL.iter().copied().enumerate() {
            assert_eq!(usize::from(member.place()), place);
            assert!(!seen.contains(member));
            assert_eq!(member.set(), Set::of(member));
            seen = seen.union(member.set());
        }
        assert_eq!(seen, Set::all());
    }

    #[test]
    fn a_set_holds_what_was_put_in_and_lists_it_in_order() {
        let set = Three::Third.set().union(Three::First.set());
        assert!(set.contains(Three::First) && set.contains(Three::Third));
        assert!(!set.contains(Three::Second));
        assert!(set.includes(Three::Third.set()));
        assert!(!set.includes(Set::all()));
        assert!(set.without(Three::First.set()).includes(Three::Third.set()));
        assert!(!set.without(Three::First.set()).contains(Three::First));
        assert_eq!(set.intersection(Three::First.set()), Three::First.set());
        assert!(Set::<Three>::empty().is_empty() && !set.is_empty());
        assert!(set.members().eq([Three::First, Three::Third].into_iter()));
    }
}
