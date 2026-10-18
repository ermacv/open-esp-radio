//! Bounded memory-copy diagnostics, independent of radio traffic.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum MemoryBenchmarkMode {
    CpuCopy,
    GdmaBlocking,
    GdmaAsync,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum MemoryBenchmarkSource {
    Sram,
    Psram,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MemoryBenchmarkRequest {
    pub mode: MemoryBenchmarkMode,
    pub source: MemoryBenchmarkSource,
    /// Payload bytes in each frame; excludes guards and storage padding.
    pub bytes: u16,
    /// Frames copied by the CPU loop or one GDMA chain per iteration.
    pub frames: u8,
    pub iterations: u16,
}

impl MemoryBenchmarkRequest {
    pub const fn validate(self) -> bool {
        self.bytes >= 1
            && self.bytes <= 4096
            && self.frames >= 1
            && self.frames <= 32
            && (self.bytes as u32) * (self.frames as u32) <= 49_152
            && self.iterations >= 1
            && self.iterations <= 64
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum MemoryBenchmarkStop {
    Completed,
    PrepareFailed,
    TransferFailed,
    TimedOut,
    DataMismatch,
    GuardCorrupted,
}

/// One case's counters and terminal correctness observation.
///
/// Elapsed counters include activity on the measuring hart between samples.
/// Foreground counters cover the whole CPU/blocking operation, or async
/// preparation/start, transfer polls and cleanup. IRQs inside those windows
/// remain included; executor and IRQ work outside them is excluded. Neither
/// counter scope measures CPU utilization.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct MemoryBenchmarkEvidence {
    pub request: MemoryBenchmarkRequest,
    pub completed_iterations: u16,
    pub elapsed_micros: u64,
    pub elapsed_cycles: u64,
    pub elapsed_instructions: u64,
    pub foreground_cycles: u64,
    pub foreground_instructions: u64,
    pub polls: u32,
    pub stop: MemoryBenchmarkStop,
}
