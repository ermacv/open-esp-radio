//! Value-only physical RX service observations.

/// One finite DMA service transaction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ServiceObservation {
    pub frontier: usize,
    pub pool_credits: usize,
    pub queue_credits: usize,
    pub completed_units: usize,
    pub completed_descriptors: usize,
    pub admitted: usize,
    pub staged_units: usize,
    pub staged_bytes: usize,
    pub discarded_units: usize,
    pub recycled_descriptors: usize,
    pub overload_discarded: usize,
    pub overload_recycled_descriptors: usize,
    pub critical_reserve_admitted: usize,
    pub stage_capacity_blocked: bool,
    pub critical_admission_blocked: bool,
    pub minimum_pool_credits: usize,
    pub minimum_queue_credits: usize,
    pub micros: u64,
    /// Hardware counter sampled immediately before this service transaction.
    pub hardware_buffer_full_before: Option<u16>,
    /// Hardware counter sampled after descriptor recycling/reload completes.
    pub hardware_buffer_full_after: Option<u16>,
}

/// Length-class discard observed before a malformed unit is recycled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Discard {
    Empty,
    TooLong,
    Chained,
    OverloadBulk,
}
