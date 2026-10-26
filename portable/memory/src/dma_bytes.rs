//! Bytes a DMA engine reads and writes behind the CPU's back.
#![allow(
    unsafe_code,
    reason = "memory a DMA engine writes is reached only by volatile copies"
)]

use core::cell::UnsafeCell;

/// `N` bytes, four-byte aligned, whose address a driver publishes to a DMA
/// engine. Every CPU access copies bytes in or out with volatile accesses, so
/// no reference ever aliases memory the engine may write. Reads take `&self`
/// and writes `&mut self`: the only concurrent writer is the hardware, which
/// is outside the program's memory model.
#[repr(C, align(4))]
#[derive(bytemuck::Zeroable)]
pub struct DmaBytes<const N: usize>(UnsafeCell<[u8; N]>);

impl<const N: usize> DmaBytes<N> {
    /// Zeroed bytes.
    pub const fn new() -> Self {
        Self(UnsafeCell::new([0; N]))
    }

    /// The address to publish to the DMA engine.
    pub fn address(&self) -> usize {
        self.0.get() as usize
    }

    /// Copy all `N` bytes out.
    pub fn read(&self) -> [u8; N] {
        // SAFETY: the pointer comes from this live, aligned `UnsafeCell`; a
        // volatile read tolerates the engine writing concurrently.
        unsafe { self.0.get().read_volatile() }
    }

    /// Copy `image` in, zero-filling the rest; bytes beyond `N` are left out.
    pub fn write(&mut self, image: &[u8]) {
        let mut bytes = [0; N];
        for (to, from) in bytes.iter_mut().zip(image) {
            *to = *from;
        }
        // SAFETY: as for `read`; `&mut self` excludes every CPU reader.
        unsafe { self.0.get().write_volatile(bytes) }
    }

    /// Write the bytes of `image` that differ from `original`, one volatile
    /// byte each, into bytes the engine may be reading.
    pub fn write_changed(&mut self, original: &[u8; N], image: &[u8; N]) {
        let base = self.0.get().cast::<u8>();
        for (index, (&old, &new)) in original.iter().zip(image).enumerate() {
            if old != new {
                // SAFETY: `index` is below `N`, so the byte lies inside this
                // live `UnsafeCell`; `&mut self` excludes every CPU reader.
                unsafe { base.add(index).write_volatile(new) }
            }
        }
    }

    /// Replace byte `index` by `update` of it; nothing beyond `N`.
    pub fn update_byte(&mut self, index: usize, update: impl FnOnce(u8) -> u8) {
        if index < N {
            // SAFETY: `index` is below `N`, so the byte lies inside this live
            // `UnsafeCell`; `&mut self` excludes every CPU reader.
            unsafe {
                let byte = self.0.get().cast::<u8>().add(index);
                byte.write_volatile(update(byte.read_volatile()));
            }
        }
    }
}

impl<const N: usize> Default for DmaBytes<N> {
    fn default() -> Self {
        Self::new()
    }
}

// SAFETY: a shared reference only reads, with volatile copies; writing takes
// `&mut self`, so no two CPU contexts race.
unsafe impl<const N: usize> Sync for DmaBytes<N> {}

#[cfg(test)]
mod tests {
    use super::DmaBytes;

    #[test]
    fn writes_copy_in_and_reads_copy_out() {
        let mut bytes = DmaBytes::<4>::new();
        bytes.write(&[1, 2, 3, 4, 5]);
        assert_eq!(bytes.read(), [1, 2, 3, 4]);
        bytes.write(&[9]);
        assert_eq!(bytes.read(), [9, 0, 0, 0]);
        bytes.write_changed(&[9, 0, 0, 0], &[9, 7, 0, 8]);
        assert_eq!(bytes.read(), [9, 7, 0, 8]);
        bytes.update_byte(0, |byte| byte & 0x7);
        bytes.update_byte(4, |_| 0xff);
        assert_eq!(bytes.read(), [1, 7, 0, 8]);
        assert_eq!(bytes.address() % 4, 0);
    }
}
