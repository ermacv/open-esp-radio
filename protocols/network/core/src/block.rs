//! Blocks of stack memory, moved by value, and the brand of one core.

use core::any::TypeId;
use core::hash::{Hash, Hasher};
use core::marker::PhantomData;
use core::sync::atomic::{AtomicU32, Ordering};

/// Who a value belongs to: a type no other network shares. Invariant, so no
/// subtyping between two brand types carries a value from one to the other.
pub(crate) type Tag<B> = PhantomData<fn(B) -> B>;

/// How many brands one image can claim.
const BRANDS: usize = 64;

/// The brands claimed, by mark. This crate holds it, so nothing outside can
/// make a brand claim twice. An entry is never removed.
static CLAIMED: [AtomicU32; BRANDS] = [const { AtomicU32::new(0) }; BRANDS];

/// FNV-1a over 32 bits, for a type's mark.
struct MarkHasher(u32);

impl Hasher for MarkHasher {
    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 ^ u32::from(*byte)).wrapping_mul(0x0100_0193);
        }
    }

    fn finish(&self) -> u64 {
        u64::from(self.0)
    }
}

/// The mark of type `B`, never the empty entry's zero.
fn mark_of<B: 'static>() -> u32 {
    let mut hasher = MarkHasher(0x811c_9dc5);
    TypeId::of::<B>().hash(&mut hasher);
    hasher.0.max(1)
}

/// Why a brand was not claimed; both are final.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BrandRefusal {
    /// This brand was claimed before.
    Claimed,
    /// The image holds as many brands as it can.
    NoPlace,
}

/// A claim refused, with the memory back.
#[derive(Debug)]
pub struct ClaimRefused<'m, const SIZE: usize, const BLOCKS: usize> {
    pub reason: BrandRefusal,
    pub storage: &'m mut [[u8; SIZE]; BLOCKS],
}

/// Enter `mark`. All claims scan in one order and an entry stays, so a mark
/// entered is met before any empty entry.
fn enter(table: &[AtomicU32], mark: u32) -> Result<(), BrandRefusal> {
    for entry in table {
        match entry.compare_exchange(0, mark, Ordering::AcqRel, Ordering::Acquire) {
            Ok(_) => return Ok(()),
            Err(found) if found == mark => return Err(BrandRefusal::Claimed),
            Err(_) => {}
        }
    }
    Err(BrandRefusal::NoPlace)
}

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

/// Give `storage` to brand `B`, any type: a type is a network's identity, and
/// a type rather than a lifetime, so `'static` tasks and queues can name its
/// blocks. This crate lets each type claim once, and only `claim` and a retired
/// core make a [`CoreBrand`], so one brand has one pool and one core at a time.
///
/// # Errors
/// [`ClaimRefused`], final. Marks are 32 bits, so two types could share one:
/// the later one is then refused, and the two never mix.
pub fn claim<'m, B: 'static, const SIZE: usize, const BLOCKS: usize>(
    storage: &'m mut [[u8; SIZE]; BLOCKS],
) -> Result<Claimed<'m, B, SIZE, BLOCKS>, ClaimRefused<'m, SIZE, BLOCKS>> {
    if let Err(reason) = enter(&CLAIMED, mark_of::<B>()) {
        return Err(ClaimRefused { reason, storage });
    }
    Ok((
        Pool::new(storage),
        CoreBrand {
            brand: PhantomData,
            serial: 0,
        },
    ))
}

/// One block of stack memory and the octets it holds.
#[derive(Debug)]
pub struct Block<'m, B, const SIZE: usize> {
    bytes: &'m mut [u8; SIZE],
    length: usize,
    brand: Tag<B>,
}

impl<B, const SIZE: usize> Block<'_, B, SIZE> {
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

/// The free blocks of brand `B`'s storage. Only its own blocks fit the brand,
/// and it holds room for all of them, so a block always has its place.
#[derive(Debug)]
pub struct Pool<'m, B, const SIZE: usize, const BLOCKS: usize> {
    free: heapless::Vec<Block<'m, B, SIZE>, BLOCKS>,
}

impl<'m, B, const SIZE: usize, const BLOCKS: usize> Pool<'m, B, SIZE, BLOCKS> {
    fn new(storage: &'m mut [[u8; SIZE]; BLOCKS]) -> Self {
        let mut free = heapless::Vec::new();
        for bytes in storage {
            // BLOCKS blocks fit a capacity of BLOCKS.
            let _fits = free.push(Block {
                bytes,
                length: 0,
                brand: PhantomData,
            });
        }
        Self { free }
    }

    /// A free block, emptied.
    pub fn take(&mut self) -> Option<Block<'m, B, SIZE>> {
        let mut block = self.free.pop()?;
        block.length = 0;
        Some(block)
    }

    /// Take a block back.
    pub fn give(&mut self, block: Block<'m, B, SIZE>) {
        // A brand has one pool, which made this block and has its place.
        let _fits = self.free.push(block);
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

#[cfg(test)]
mod tests {
    use core::sync::atomic::AtomicU32;

    use super::{BrandRefusal, enter, mark_of};

    const MARKS: [u32; 2] = [7, 9];

    #[test]
    fn a_mark_enters_once() {
        let table = [const { AtomicU32::new(0) }; 2];
        assert_eq!(enter(&table, MARKS[0]), Ok(()));
        assert_eq!(enter(&table, MARKS[1]), Ok(()));
        assert_eq!(enter(&table, MARKS[0]), Err(BrandRefusal::Claimed));
        assert_eq!(enter(&table, MARKS[1]), Err(BrandRefusal::Claimed));
    }

    #[test]
    fn a_full_table_refuses_a_new_mark_and_still_knows_its_own() {
        let table = [const { AtomicU32::new(0) }; 1];
        assert_eq!(enter(&table, MARKS[0]), Ok(()));
        assert_eq!(enter(&table, MARKS[1]), Err(BrandRefusal::NoPlace));
        assert_eq!(enter(&table, MARKS[0]), Err(BrandRefusal::Claimed));
    }

    #[test]
    fn two_types_have_two_marks_and_a_type_keeps_its_own() {
        struct First;
        struct Second;
        assert_ne!(mark_of::<First>(), mark_of::<Second>());
        assert_eq!(mark_of::<First>(), mark_of::<First>());
        assert_ne!(mark_of::<First>(), 0);
    }
}
