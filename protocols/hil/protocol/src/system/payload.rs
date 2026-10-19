//! The system module's payloads: stack watermarks.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StackWatermark {
    pub capacity_bytes: u32,
    pub free_bytes: u32,
    pub used_bytes: u32,
    pub minimum_free_bytes: u32,
}

impl StackWatermark {
    /// Whether this measured stack retains its nonzero required reserve.
    /// Inconsistent measurements cannot establish headroom.
    pub const fn has_required_headroom(self) -> bool {
        self.minimum_free_bytes > 0
            && self.free_bytes >= self.minimum_free_bytes
            && matches!(self.free_bytes.checked_add(self.used_bytes), Some(total) if total == self.capacity_bytes)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct StackUsage {
    pub cpu0: StackWatermark,
    pub cpu1: StackWatermark,
    /// Own-hart measurements of dedicated IRQ stacks. `None` means this image
    /// shares that hart's task stack; it never means a failed measurement.
    pub cpu0_irq: Option<StackWatermark>,
    pub cpu1_irq: Option<StackWatermark>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_headroom_accepts_the_boundary_and_rejects_exhaustion_or_invalid_evidence() {
        let at_limit = StackWatermark {
            capacity_bytes: 192 * 1024,
            minimum_free_bytes: 16 * 1024,
            free_bytes: 16 * 1024,
            used_bytes: 176 * 1024,
        };
        assert!(at_limit.has_required_headroom());
        assert!(
            !StackWatermark {
                free_bytes: at_limit.free_bytes - 1,
                used_bytes: at_limit.used_bytes + 1,
                ..at_limit
            }
            .has_required_headroom()
        );
        assert!(
            !StackWatermark {
                minimum_free_bytes: 0,
                ..at_limit
            }
            .has_required_headroom()
        );
        assert!(
            !StackWatermark {
                used_bytes: u32::MAX,
                ..at_limit
            }
            .has_required_headroom()
        );
    }
}
