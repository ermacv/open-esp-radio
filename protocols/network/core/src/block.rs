//! Blocks of stack memory, moved by value, and the brand of one core.

use crate::brand::{Brand, NetworkClaim};
use crate::loans::Loans;
use core::marker::PhantomData;
use core::mem::replace;

/// Who a value belongs to: a brand no other network shares. Invariant, so no
/// subtyping between two brands carries a value from one to the other.
pub(crate) type Tag<B> = PhantomData<fn(B) -> B>;

/// The right to make a core of brand `B`: one at a time, as a core retired
/// gives it back. It carries the last serial given, so a later core gives none
/// of the old ones again and an old handle stays stale.
#[derive(Debug)]
pub struct CoreBrand<B> {
    pub(crate) brand: Tag<B>,
    pub(crate) serial: u64,
}

/// What a brand's claim gives: its one pool and the right to its core.
pub type Claimed<'m, B, const SIZE: usize, const BLOCKS: usize> =
    (Pool<'m, B, SIZE, BLOCKS>, CoreBrand<B>);

/// Give `storage` to the brand `right` was made for, and take the right: it
/// is the brand's only one, and only `claim` and a retired core make a
/// [`CoreBrand`], so one brand has one pool and one core at a time. The
/// storage is the pool's for as long as it lasts: a second pool over it does
/// not compile, with the first gone either:
///
/// ```compile_fail,E0499
/// # use oer_protocols_network_core::{BlockPlace, Brand, NetworkClaim, claim};
/// fn twice<A: Brand, B: Brand>(first: NetworkClaim<A>, second: NetworkClaim<B>) {
///     let mut storage = [const { BlockPlace::<64>::new() }; 1];
///     drop(claim(first, &mut storage));
///     drop(claim(second, &mut storage));
/// }
/// ```
///
/// The same with a storage for each compiles:
///
/// ```
/// # use oer_protocols_network_core::{BlockPlace, Brand, NetworkClaim, claim};
/// fn each<A: Brand, B: Brand>(first: NetworkClaim<A>, second: NetworkClaim<B>) {
///     let mut storage = [const { BlockPlace::<64>::new() }; 1];
///     let mut other = [const { BlockPlace::<64>::new() }; 1];
///     drop(claim(first, &mut storage));
///     drop(claim(second, &mut other));
/// }
/// ```
pub fn claim<'m, B: Brand, const SIZE: usize, const BLOCKS: usize>(
    right: NetworkClaim<B>,
    storage: &'m mut [BlockPlace<'m, SIZE>; BLOCKS],
) -> Claimed<'m, B, SIZE, BLOCKS> {
    let NetworkClaim { brand } = right;
    (Pool::new(storage), CoreBrand { brand, serial: 0 })
}

/// One block's place in a pool's storage: its octets, and what its block and
/// its pool know of it. The pool has it while the block is free, and the block
/// while it is out.
#[derive(Debug)]
pub struct BlockPlace<'m, const SIZE: usize> {
    link: Link<'m, SIZE>,
    /// How many octets the block holds, and the group it counts toward: set
    /// when it is taken, and read by the block that was.
    length: usize,
    group: u8,
    bytes: [u8; SIZE],
}

impl<const SIZE: usize> BlockPlace<'_, SIZE> {
    /// A place of no pool yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            link: Link::Off,
            length: 0,
            group: 0,
            bytes: [0; SIZE],
        }
    }
}

impl<const SIZE: usize> Default for BlockPlace<'_, SIZE> {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a place is on its pool's chain of free ones.
#[derive(Debug)]
enum Link<'m, const SIZE: usize> {
    /// Off it: a block holds the place, or no pool has it yet.
    Off,
    /// On it, with the chain behind it.
    On(Chain<'m, SIZE>),
}

/// Free places, each holding those behind it.
#[derive(Debug)]
enum Chain<'m, const SIZE: usize> {
    End,
    Head(&'m mut BlockPlace<'m, SIZE>),
}

impl<'m, const SIZE: usize> Chain<'m, SIZE> {
    /// Put `place` first.
    fn push(&mut self, place: &'m mut BlockPlace<'m, SIZE>) {
        place.link = Link::On(replace(self, Self::End));
        *self = Self::Head(place);
    }

    /// Take the first place, if there is one.
    fn pop(&mut self) -> Option<&'m mut BlockPlace<'m, SIZE>> {
        let Self::Head(place) = replace(self, Self::End) else {
            return None;
        };
        if let Link::On(behind) = replace(&mut place.link, Link::Off) {
            *self = behind;
        }
        Some(place)
    }
}

/// One block of stack memory and the octets it holds: the handle of its place.
#[derive(Debug)]
pub struct Block<'m, B, const SIZE: usize> {
    place: &'m mut BlockPlace<'m, SIZE>,
    brand: Tag<B>,
}

impl<B, const SIZE: usize> Block<'_, B, SIZE> {
    /// The group this block counts toward.
    pub const fn group(&self) -> Group<B> {
        Group {
            index: self.place.group,
            brand: PhantomData,
        }
    }

    /// The whole block, to fill.
    pub fn space(&mut self) -> &mut [u8; SIZE] {
        &mut self.place.bytes
    }

    /// Say how many octets the block holds.
    ///
    /// # Errors
    /// `length` passes the block; what it held stays.
    pub fn set_length(&mut self, length: usize) -> Result<(), BlockTooShort> {
        if length > SIZE {
            return Err(BlockTooShort);
        }
        self.place.length = length;
        Ok(())
    }

    /// The octets the block holds.
    pub fn octets(&self) -> &[u8] {
        self.place
            .bytes
            .get(..self.place.length)
            .unwrap_or_default()
    }

    /// Fill the block from `octets`.
    ///
    /// # Errors
    /// `octets` do not fit; the block stays as it was.
    pub fn fill(&mut self, octets: &[u8]) -> Result<(), BlockTooShort> {
        oer_portable_wire::Writer::new(self.place.bytes.as_mut_slice())
            .write(octets)
            .map_err(|_| BlockTooShort)?;
        self.place.length = octets.len();
        Ok(())
    }
}

/// More octets than a block holds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BlockTooShort;

/// How many groups a pool keeps, the shared one included.
pub const GROUPS: usize = 8;

/// A pool's group: what its blocks count toward, whichever socket or task holds
/// them. A block counts toward the group it was taken for from then until the
/// pool has it back, so moving it, or dropping whoever handed it over, changes
/// nothing. A limit bounds the group and pins no block: what it does not use
/// stays shared.
#[derive(Debug)]
pub struct Group<B> {
    index: u8,
    brand: Tag<B>,
}

impl<B> Group<B> {
    /// The group every pool has, with no limit.
    #[must_use]
    pub const fn shared() -> Self {
        Self {
            index: 0,
            brand: PhantomData,
        }
    }
}

impl<B> Clone for Group<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> Copy for Group<B> {}

impl<B> PartialEq for Group<B> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl<B> Eq for Group<B> {}

/// A pool has as many groups as it can keep, [`GROUPS`]. A final refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GroupsFull;

#[derive(Clone, Copy, Debug)]
struct Account {
    limit: usize,
    held: usize,
}

/// The free blocks of brand `B`'s storage, chained through their own places,
/// so one that comes back always has its place. The last to come back is the
/// next taken; the first taken of all is the storage's last.
#[derive(Debug)]
pub struct Pool<'m, B, const SIZE: usize, const BLOCKS: usize> {
    free: Chain<'m, SIZE>,
    /// By group, the shared one first.
    groups: heapless::Vec<Account, GROUPS>,
    /// What receive loans hold of the driver's buffers. It stays here because
    /// loans outlive a core, as a pool does.
    loans: Loans<B, GROUPS>,
}

impl<'m, B, const SIZE: usize, const BLOCKS: usize> Pool<'m, B, SIZE, BLOCKS> {
    fn new(storage: &'m mut [BlockPlace<'m, SIZE>; BLOCKS]) -> Self {
        let mut free = Chain::End;
        for place in storage {
            free.push(place);
        }
        // The shared group.
        let groups = heapless::Vec::from_array([Account {
            limit: usize::MAX,
            held: 0,
        }]);
        Self {
            free,
            groups,
            loans: Loans::new(),
        }
    }

    /// The one account of this network's receive loans. A socket is numbered
    /// here below [`GROUPS`].
    pub const fn loans(&mut self) -> &mut Loans<B, GROUPS> {
        &mut self.loans
    }

    /// A new group of at most `limit` blocks. A group has one limit, here.
    ///
    /// # Errors
    /// [`GroupsFull`].
    pub fn group(&mut self, limit: usize) -> Result<Group<B>, GroupsFull> {
        let index = u8::try_from(self.groups.len()).map_err(|_| GroupsFull)?;
        self.groups
            .push(Account { limit, held: 0 })
            .map_err(|_| GroupsFull)?;
        Ok(Group {
            index,
            brand: PhantomData,
        })
    }

    fn account(&self, group: Group<B>) -> Option<&Account> {
        self.groups.get(usize::from(group.index))
    }

    /// A free block, emptied, for the shared group.
    pub fn take(&mut self) -> Option<Block<'m, B, SIZE>> {
        self.take_as(Group::shared())
    }

    /// A free block, emptied, counted toward `group`: none if the group is at
    /// its limit, however many are free.
    pub fn take_as(&mut self, group: Group<B>) -> Option<Block<'m, B, SIZE>> {
        if !self.may_take(group) {
            return None;
        }
        let place = self.free.pop()?;
        place.length = 0;
        place.group = group.index;
        if let Some(account) = self.groups.get_mut(usize::from(group.index)) {
            account.held = account.held.saturating_add(1);
        }
        Some(Block {
            place,
            brand: PhantomData,
        })
    }

    /// Take a block back: only now does it stop counting toward its group.
    pub fn give(&mut self, block: Block<'m, B, SIZE>) {
        let Block { place, .. } = block;
        if let Some(account) = self.groups.get_mut(usize::from(place.group)) {
            account.held = account.held.saturating_sub(1);
        }
        self.free.push(place);
    }

    /// Whether `group` is under its limit.
    pub fn may_take(&self, group: Group<B>) -> bool {
        self.account(group)
            .is_some_and(|account| account.held < account.limit)
    }

    /// The group holding the most blocks, wherever they are, with their count.
    pub fn heaviest(&self) -> Option<(Group<B>, usize)> {
        let (index, held) = (0_u8..)
            .zip(self.groups.iter().map(|account| account.held))
            .max_by_key(|(_, held)| *held)?;
        let group = Group {
            index,
            brand: PhantomData,
        };
        (held != 0).then_some((group, held))
    }

    /// Blocks out of the pool that count toward `group`.
    pub fn held(&self, group: Group<B>) -> usize {
        self.account(group).map_or(0, |account| account.held)
    }

    /// How many blocks are free.
    pub fn free(&self) -> usize {
        BLOCKS.saturating_sub(self.outstanding())
    }

    /// How many blocks are out: a retired core's outstanding obligations,
    /// which an application returns by [`Self::give`] whenever it does.
    pub fn outstanding(&self) -> usize {
        self.groups
            .iter()
            .fold(0, |out: usize, account| out.saturating_add(account.held))
    }
}
