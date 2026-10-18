//! Bounded DMA ownership model for cancellation cuts, not an MMIO simulator.
//! Each published address must retain its exact buffer in the backing's queue.
use super::*;

#[derive(Clone, Default)]
pub(super) struct Audit {
    pub descriptors: Count,
    pub queued: Count,
    pub buffer_drops: Count,
}
struct Buffer {
    bytes: [u8; 16],
    drops: Count,
}
impl Drop for Buffer {
    fn drop(&mut self) {
        self.drops.increment();
    }
}

#[derive(Default)]
pub(super) struct DmaQueue {
    buffers: [Option<Box<Buffer>>; 2],
    descriptors: [Option<usize>; 2],
    audit: Audit,
}
impl DmaQueue {
    pub fn populated() -> (Self, Audit) {
        let mut work = Self::default();
        for index in 0..work.buffers.len() {
            let buffer = Box::new(Buffer {
                bytes: [index as u8; 16],
                drops: work.audit.buffer_drops.clone(),
            });
            work.descriptors[index] = Some(buffer.bytes.as_ptr() as usize);
            work.buffers[index] = Some(buffer);
            work.audit.descriptors.increment();
            work.audit.queued.increment();
        }
        let audit = work.audit.clone();
        (work, audit)
    }

    pub fn assert_owned(&self) {
        for (descriptor, buffer) in self.descriptors.iter().zip(&self.buffers) {
            assert_eq!(
                *descriptor,
                buffer.as_ref().map(|buffer| buffer.bytes.as_ptr() as usize),
                "each published DMA address retains its precise queued owner"
            );
        }
    }

    pub fn drain(&mut self) {
        self.assert_owned();
        for (descriptor, buffer) in self.descriptors.iter_mut().zip(&mut self.buffers) {
            if descriptor.take().is_some() {
                self.audit.descriptors.0.fetch_sub(1, Ordering::SeqCst);
            }
            if let Some(buffer) = buffer.take() {
                self.audit.queued.0.fetch_sub(1, Ordering::SeqCst);
                drop(buffer);
            }
        }
        self.assert_owned();
    }
}
