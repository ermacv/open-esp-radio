//! Blocks of stack memory, moved by value, and the brand of one core.

use core::marker::PhantomData;

/// Who a value belongs to: a type no other network shares.
pub(crate) type Tag<B> = PhantomData<fn() -> B>;

/// A network's identity, a type the [`brand!`](crate::brand) macro makes. A
/// type, not a lifetime: a block or a handle crosses `'static` tasks and
/// queues, which can name it, and another network's value does not compile.
pub trait Brand: Sized + 'static {
    /// Take this brand's one claim; whether it was still free.
    fn claim_once() -> bool;
}

/// Make the brand type `$name`, whose claim holds once per image.
#[macro_export]
macro_rules! brand {
    ($visibility:vis $name:ident) => {
        #[derive(Debug, Eq, PartialEq)]
        $visibility struct $name;

        impl $crate::Brand for $name {
            fn claim_once() -> bool {
                static CLAIMED: core::sync::atomic::AtomicBool =
                    core::sync::atomic::AtomicBool::new(false);
                !CLAIMED.swap(true, core::sync::atomic::Ordering::AcqRel)
            }
        }
    };
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

/// Give `storage` to brand `B`.
///
/// # Errors
/// `B` was claimed before; a final refusal, with `storage` back.
pub fn claim<'m, B: Brand, const SIZE: usize, const BLOCKS: usize>(
    storage: &'m mut [[u8; SIZE]; BLOCKS],
) -> Result<Claimed<'m, B, SIZE, BLOCKS>, &'m mut [[u8; SIZE]; BLOCKS]> {
    if !B::claim_once() {
        return Err(storage);
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
        // This pool made the block, so the others out leave it room.
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
