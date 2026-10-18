//! Payload limits of the product image, independent of whether the product
//! task is linked.

pub(crate) const OPEN_RADIO_TCP_CHUNK_CAPACITY: usize = 32_768;

// The command's per-frame payload policy, independent of radio socket storage.
const MEMORY_BENCHMARK_PAYLOAD_CAPACITY: u16 = 4096;

/// The largest payload one command of this image carries.
pub(crate) const MAXIMUM_PAYLOAD_BYTES: u16 = if cfg!(feature = "memory-benchmark") {
    MEMORY_BENCHMARK_PAYLOAD_CAPACITY
} else {
    OPEN_RADIO_TCP_CHUNK_CAPACITY as u16
};
