//! Blocks of stack memory, moved by value, and the brand of one core.

use crate::brand::{Brand, NetworkClaim};
use crate::loans::Loans;
use core::marker::PhantomData;

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
/// [`CoreBrand`], so one brand has one pool and one core at a time.
pub fn claim<B: Brand, const SIZE: usize, const BLOCKS: usize>(
    right: NetworkClaim<B>,
    storage: &mut [[u8; SIZE]; BLOCKS],
) -> Claimed<'_, B, SIZE, BLOCKS> {
    let NetworkClaim { brand } = right;
    (Pool::new(storage), CoreBrand { brand, serial: 0 })
}

/// One block of stack memory and the octets it holds.
#[derive(Debug)]
pub struct Block<'m, B, const SIZE: usize> {
    bytes: &'m mut [u8; SIZE],
    length: usize,
    /// The group this block counts toward, until the pool has it back.
    group: u8,
    brand: Tag<B>,
}

impl<B, const SIZE: usize> Block<'_, B, SIZE> {
    /// The group this block counts toward.
    pub const fn group(&self) -> Group<B> {
        Group {
            index: self.group,
            brand: PhantomData,
        }
    }

    /// The whole block, to fill.
    pub fn space(&mut self) -> &mut [u8; SIZE] {
        self.bytes
    }

    /// Say how many octets the block holds.
    ///
    /// # Errors
    /// `length` passes the block; what it held stays.
    pub fn set_length(&mut self, length: usize) -> Result<(), BlockTooShort> {
        if length > SIZE {
            return Err(BlockTooShort);
        }
        self.length = length;
        Ok(())
    }

    /// The octets the block holds.
    pub fn octets(&self) -> &[u8] {
        self.bytes.get(..self.length).unwrap_or_default()
    }

    /// Fill the block from `octets`.
    ///
    /// # Errors
    /// `octets` do not fit; the block stays as it was.
    pub fn fill(&mut self, octets: &[u8]) -> Result<(), BlockTooShort> {
        oer_portable_wire::Writer::new(&mut *self.bytes)
            .write(octets)
            .map_err(|_| BlockTooShort)?;
        self.length = octets.len();
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

/// The free blocks of brand `B`'s storage. Only its own blocks fit the brand,
/// and it holds room for all of them, so a block always has its place.
#[derive(Debug)]
pub struct Pool<'m, B, const SIZE: usize, const BLOCKS: usize> {
    free: heapless::Vec<Block<'m, B, SIZE>, BLOCKS>,
    /// By group, the shared one first.
    groups: heapless::Vec<Account, GROUPS>,
    /// What receive loans hold of the driver's buffers. It stays here because
    /// loans outlive a core, as a pool does.
    loans: Loans<B, GROUPS>,
}

impl<'m, B, const SIZE: usize, const BLOCKS: usize> Pool<'m, B, SIZE, BLOCKS> {
    fn new(storage: &'m mut [[u8; SIZE]; BLOCKS]) -> Self {
        let mut free = heapless::Vec::new();
        for bytes in storage {
            // BLOCKS blocks fit a capacity of BLOCKS.
            let _fits = free.push(Block {
                bytes,
                length: 0,
                group: 0,
                brand: PhantomData,
            });
        }
        let mut groups = heapless::Vec::new();
        // One entry fits a capacity of GROUPS, which a const check holds above 0.
        let _fits = groups.push(Account {
            limit: usize::MAX,
            held: 0,
        });
        const {
            if GROUPS == 0 {
                panic!("a pool has its shared group");
            }
        };
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
        let mut block = self.free.pop()?;
        block.length = 0;
        block.group = group.index;
        if let Some(account) = self.groups.get_mut(usize::from(group.index)) {
            account.held = account.held.saturating_add(1);
        }
        Some(block)
    }

    /// Take a block back: only now does it stop counting toward its group.
    pub fn give(&mut self, block: Block<'m, B, SIZE>) {
        if let Some(account) = self.groups.get_mut(usize::from(block.group)) {
            account.held = account.held.saturating_sub(1);
        }
        // A brand has one pool, which made this block and has its place.
        let _fits = self.free.push(block);
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
        self.free.len()
    }

    /// How many blocks are out: a retired core's outstanding obligations,
    /// which an application returns by [`Self::give`] whenever it does.
    pub fn outstanding(&self) -> usize {
        BLOCKS.saturating_sub(self.free.len())
    }
}
