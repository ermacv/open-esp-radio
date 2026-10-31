//! The window's counters: the serving hart's cycles and retired
//! instructions, what the network bridge did with the station's frames and
//! what the IP stack took.
//!
//! The bridge reports each frame through `oer_trace::observe!`, so only an
//! `oer_trace` build counts them; the station work's task is their one
//! writer and the load server, in the same task, their reader. The hart's
//! counters and the stack's are read at the window's ends. Bridge counts
//! wrap at 2^32; the stack's saturate there.

use oer_services_load::Counters;

/// What the network bridge did with one frame from the station.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build reports the bridge's frames")
)]
pub(crate) enum Bridged {
    /// The bridge took it from the station; a refusal below follows when
    /// the stack does not take it.
    Received,
    /// The stack's receive pool had no packet for it.
    RefusedPool,
    /// The stack's receive queue had no room for it.
    RefusedQueue,
    /// Its length or a down link refused it.
    RefusedOther,
    /// The station lent it in place and the bridge copied it after all.
    Copied,
}

#[cfg_attr(
    not(any(oer_trace, test)),
    allow(dead_code, reason = "only a trace build reports the bridge's frames")
)]
impl Bridged {
    const ALL: [Bridged; 5] = [
        Bridged::Received,
        Bridged::RefusedPool,
        Bridged::RefusedQueue,
        Bridged::RefusedOther,
        Bridged::Copied,
    ];

    #[cfg(any(oer_trace, test))]
    fn from_raw(raw: u32) -> Option<Self> {
        Self::ALL.into_iter().find(|outcome| *outcome as u32 == raw)
    }
}

/// One frame's outcome at the network bridge.
#[cfg(any(oer_trace, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BridgeFrame {
    pub outcome: Bridged,
}

#[cfg(any(oer_trace, test))]
impl oer_trace::Event for BridgeFrame {
    const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Platform, 42);
    // The executors' activity channel, beside the parts' polls.
    const CHANNEL: oer_trace::Channel = const {
        match oer_trace::Channel::new(oer_trace::Domain::Platform, 7) {
            Ok(channel) => channel,
            Err(_) => panic!("the activity channel is in the platform range"),
        }
    };
    fn encode(&self) -> [u32; 2] {
        [self.outcome as u32, 0]
    }
    fn decode([outcome, reserved]: [u32; 2]) -> Option<Self> {
        if reserved != 0 {
            return None;
        }
        Some(Self {
            outcome: Bridged::from_raw(outcome)?,
        })
    }
}

/// The bridge's counts by outcome.
#[cfg(target_arch = "riscv32")]
static BRIDGE: [core::sync::atomic::AtomicU32; Bridged::ALL.len()] =
    [const { core::sync::atomic::AtomicU32::new(0) }; Bridged::ALL.len()];

/// Count `frame`; only the station work's task calls it.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub(crate) fn observe(frame: BridgeFrame) {
    use core::sync::atomic::Ordering;
    if let Some(count) = BRIDGE.get(usize::from(frame.outcome as u8)) {
        count.store(
            count.load(Ordering::Relaxed).wrapping_add(1),
            Ordering::Relaxed,
        );
    }
}

/// What one end of the window read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Snapshot {
    pub cycles: u64,
    pub instructions: u64,
    pub bridge: [u32; Bridged::ALL.len()],
    pub stack_received: u32,
}

/// The counters now, with the stack's received frames `stack_received`.
#[cfg(target_arch = "riscv32")]
pub(crate) fn snapshot(stack_received: u32) -> Snapshot {
    use core::sync::atomic::Ordering;
    Snapshot {
        cycles: oer_riscv::cycle64(),
        instructions: oer_riscv::instret64(),
        bridge: BRIDGE.each_ref().map(|count| count.load(Ordering::Relaxed)),
        stack_received,
    }
}

/// The counts between the window's ends `start` and `end`.
pub(crate) fn window(start: &Snapshot, end: &Snapshot) -> Counters {
    let bridge = |outcome: Bridged| {
        let at = |snapshot: &Snapshot| {
            snapshot
                .bridge
                .get(usize::from(outcome as u8))
                .copied()
                .unwrap_or(0)
        };
        at(end).wrapping_sub(at(start))
    };
    Counters {
        cycles: end.cycles.wrapping_sub(start.cycles),
        instructions: end.instructions.wrapping_sub(start.instructions),
        bridge_received: bridge(Bridged::Received),
        bridge_refused_pool: bridge(Bridged::RefusedPool),
        bridge_refused_queue: bridge(Bridged::RefusedQueue),
        bridge_refused_other: bridge(Bridged::RefusedOther),
        bridge_copied: bridge(Bridged::Copied),
        stack_received: end.stack_received.saturating_sub(start.stack_received),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_trace::Event as _;

    #[test]
    fn a_window_takes_wrapping_bridge_and_hart_differences() {
        let start = Snapshot {
            cycles: u64::MAX - 1,
            instructions: 10,
            bridge: [u32::MAX, 1, 2, 3, 6],
            stack_received: 5,
        };
        let end = Snapshot {
            cycles: 3,
            instructions: 25,
            bridge: [4, 1, 7, 3, 8],
            stack_received: 9,
        };
        assert_eq!(
            window(&start, &end),
            Counters {
                cycles: 5,
                instructions: 15,
                bridge_received: 5,
                bridge_refused_pool: 0,
                bridge_refused_queue: 5,
                bridge_refused_other: 0,
                bridge_copied: 2,
                stack_received: 4,
            }
        );
    }

    #[test]
    fn a_bridge_frame_round_trips_and_refuses_unknown_words() {
        for outcome in Bridged::ALL {
            let frame = BridgeFrame { outcome };
            assert_eq!(BridgeFrame::decode(frame.encode()), Some(frame));
        }
        assert_eq!(BridgeFrame::decode([5, 0]), None);
        assert_eq!(BridgeFrame::decode([0, 1]), None);
    }
}
