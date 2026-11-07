//! Blocks of stack memory, moved by value.

/// One block of stack memory and the octets it holds.
#[derive(Debug)]
pub struct Block<'m, const SIZE: usize> {
    bytes: &'m mut [u8; SIZE],
    length: usize,
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

/// The free blocks of the storage the composition placed. It outlives a core,
/// so a block out at a core's end stays out and can still come back.
#[derive(Debug)]
pub struct Pool<'m, const SIZE: usize, const BLOCKS: usize> {
    free: heapless::Vec<Block<'m, SIZE>, BLOCKS>,
}

impl<'m, const SIZE: usize, const BLOCKS: usize> Pool<'m, SIZE, BLOCKS> {
    pub fn new(storage: &'m mut [[u8; SIZE]; BLOCKS]) -> Self {
        let mut free = heapless::Vec::new();
        for bytes in storage {
            // BLOCKS blocks fit a capacity of BLOCKS.
            let _fits = free.push(Block { bytes, length: 0 });
        }
        Self { free }
    }

    /// A free block, emptied.
    pub fn take(&mut self) -> Option<Block<'m, SIZE>> {
        let mut block = self.free.pop()?;
        block.length = 0;
        Some(block)
    }

    /// Take a block back.
    ///
    /// # Errors
    /// The pool is full, so the block is another pool's; it comes back.
    pub fn give(&mut self, block: Block<'m, SIZE>) -> Result<(), Block<'m, SIZE>> {
        self.free.push(block)
    }

    /// How many blocks are free.
    pub fn free(&self) -> usize {
        self.free.len()
    }
}
