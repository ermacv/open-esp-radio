//! Blocks of stack memory, moved by value, and the brand of one core.

use core::marker::PhantomData;

/// Who a value belongs to: an invariant lifetime no other [`scope`] shares.
pub(crate) type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The right to make a core of a [`scope`]: one at a time, as a core retired
/// gives it back. It carries the last serial given, so a later core gives none
/// of the old ones again and an old handle stays stale.
#[derive(Debug)]
pub struct CoreBrand<'id> {
    pub(crate) brand: Brand<'id>,
    pub(crate) serial: u64,
}

/// Give `storage` a pool and one core's brand. Everything of this scope, its
/// pool, core, blocks, sockets and commands, carries one lifetime no other
/// scope has, so a value of another core or pool does not compile here.
pub fn scope<const SIZE: usize, const BLOCKS: usize, R>(
    storage: &mut [[u8; SIZE]; BLOCKS],
    run: impl for<'id> FnOnce(Pool<'id, SIZE, BLOCKS>, CoreBrand<'id>) -> R,
) -> R {
    run(
        Pool::new(storage),
        CoreBrand {
            brand: PhantomData,
            serial: 0,
        },
    )
}

/// [`scope`] for a task: `run` awaits, and its future may hold the pool and
/// the core across its awaits. With `storage` placed for good, the future is
/// `'static`, as an executor's task needs:
///
/// ```
/// # use oer_protocols_network_core::{Core, Interface, scope_async};
/// async fn service(storage: &'static mut [[u8; 64]; 2], interface: Interface) {
///     scope_async(storage, async move |pool, brand| {
///         let core: Core<'_, 64, 1, 1, 1, 1> = Core::new(brand, interface);
///         core::future::ready(()).await;
///         drop((core, pool));
///     })
///     .await;
/// }
/// fn task<F: Future<Output = ()> + 'static>(_: F) {}
/// # fn f(storage: &'static mut [[u8; 64]; 2], interface: Interface) {
/// task(service(storage, interface));
/// # }
/// ```
pub async fn scope_async<const SIZE: usize, const BLOCKS: usize, R>(
    storage: &mut [[u8; SIZE]; BLOCKS],
    run: impl for<'id> AsyncFnOnce(Pool<'id, SIZE, BLOCKS>, CoreBrand<'id>) -> R,
) -> R {
    run(
        Pool::new(storage),
        CoreBrand {
            brand: PhantomData,
            serial: 0,
        },
    )
    .await
}

/// One block of stack memory and the octets it holds.
#[derive(Debug)]
pub struct Block<'id, const SIZE: usize> {
    bytes: &'id mut [u8; SIZE],
    length: usize,
    brand: Brand<'id>,
}

impl<const SIZE: usize> Block<'_, SIZE> {
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

/// The free blocks of one [`scope`]'s storage. Only its own blocks fit its
/// brand, and it holds room for all of them, so a block always has its place.
#[derive(Debug)]
pub struct Pool<'id, const SIZE: usize, const BLOCKS: usize> {
    free: heapless::Vec<Block<'id, SIZE>, BLOCKS>,
}

impl<'id, const SIZE: usize, const BLOCKS: usize> Pool<'id, SIZE, BLOCKS> {
    fn new(storage: &'id mut [[u8; SIZE]; BLOCKS]) -> Self {
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
    pub fn take(&mut self) -> Option<Block<'id, SIZE>> {
        let mut block = self.free.pop()?;
        block.length = 0;
        Some(block)
    }

    /// Take a block back.
    pub fn give(&mut self, block: Block<'id, SIZE>) {
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
