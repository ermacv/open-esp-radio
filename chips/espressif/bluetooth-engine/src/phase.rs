//! Progress values carry no register, graph, completion or reclamation authority.
//!
//! The unique HAL owners retain publication receipts and generative arena/epoch
//! brands. Changing a progress value cannot manufacture any such receipt.

/// Durable progress of one execution-modify attempt.
#[derive(Debug)]
pub enum ExecutionModifyPhase {
    /// Before the request: the engines must be idle.
    Preamble,
    /// The progress signal may still report a conflict.
    Progress { selected: bool },
    /// Waiting for the request while the scheduler is busy.
    Completion,
    /// Waiting for the settle signal to leave 9.
    Settle { selected: bool },
}

/// Durable stop progress, including each outstanding P3 reply.
#[derive(Debug)]
pub enum StopPhase {
    Initial,
    InitialReply,
    MaskReply,
    Preamble,
    PreambleReply,
    Requested,
    RequestedReply,
}

/// Non-repeating engine installation identity used to reject stale completions.
/// This value grants no register or DMA authority; receipts retain it privately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineEpoch(u32);

impl EngineEpoch {
    /// Allocate from the owner's installation counter without wrapping.
    /// Exhaustion leaves the counter unchanged and refuses every later attempt.
    pub fn allocate(counter: &core::sync::atomic::AtomicU32) -> Option<Self> {
        use core::sync::atomic::Ordering;
        counter
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |epoch| {
                epoch.checked_add(1)
            })
            .ok()
            .map(Self)
    }

    /// Detached observation identity; this does not construct a completion receipt.
    pub const fn from_sequence(sequence: u32) -> Self {
        Self(sequence)
    }

    /// Numeric identity for value-only lifecycle replies.
    pub const fn sequence(self) -> u32 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_exhaustion_is_permanent() {
        let counter = core::sync::atomic::AtomicU32::new(u32::MAX - 1);
        assert_eq!(
            EngineEpoch::allocate(&counter).map(EngineEpoch::sequence),
            Some(u32::MAX - 1)
        );
        assert_eq!(EngineEpoch::allocate(&counter), None);
        assert_eq!(EngineEpoch::allocate(&counter), None);
    }
}
