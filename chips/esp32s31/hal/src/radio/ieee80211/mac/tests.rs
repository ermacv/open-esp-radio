//! The runtime start's and stop's writes over a small port, in their order,
//! with the cycle each was made at on a stepping counter.

extern crate std;

use core::cell::Cell;
use std::vec::Vec;

use oer_portable_settle::{SettleCounter, SettleLimit};

use super::{MacRuntimePort, StopRequested, open_core, request_stop, start_receive};

/// A counter of one cycle a microsecond that moves one at every sample.
struct Stepping(Cell<u32>);

impl SettleCounter for Stepping {
    fn cycles(&self, limit: SettleLimit) -> u32 {
        limit.micros()
    }
    fn sample(&self) -> u32 {
        let now = self.0.get();
        self.0.set(now + 1);
        now
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Write {
    StopRequest,
    Fence,
    OpenCore,
    Resume,
    RegdmaLink,
}

/// Every write with the counter's cycle at it.
struct Port<'c> {
    clock: &'c Stepping,
    writes: Vec<(Write, u32)>,
}

impl Port<'_> {
    fn record(&mut self, write: Write) {
        let at = self.clock.0.get();
        self.writes.push((write, at));
    }

    fn kinds(&self) -> Vec<Write> {
        self.writes.iter().map(|(write, _)| *write).collect()
    }
}

impl MacRuntimePort for Port<'_> {
    fn request_stop(&mut self) {
        self.record(Write::StopRequest);
    }
    fn fence(&mut self) {
        self.record(Write::Fence);
    }
    fn open_core(&mut self) {
        self.record(Write::OpenCore);
    }
    fn resume(&mut self) {
        self.record(Write::Resume);
    }
    fn select_regdma_link(&mut self) {
        self.record(Write::RegdmaLink);
    }
}

#[test]
fn the_stop_request_is_written_fenced_then_held_for_twenty_microseconds() {
    let clock = Stepping(Cell::new(1_000));
    let mut port = Port {
        clock: &clock,
        writes: Vec::new(),
    };
    let StopRequested { .. } = request_stop(&mut port, &clock);
    assert_eq!(
        port.kinds(),
        [Write::StopRequest, Write::Fence],
        "the request, the fence, nothing more"
    );
    let requested = port.writes[0].1;
    let returned = clock.0.get();
    assert!(
        returned - requested >= 20,
        "the hold after the request: returned {} µs after it",
        returned - requested
    );
    assert!(
        returned - requested <= 24,
        "held no longer than its length and its reads"
    );
}

#[test]
fn the_core_opens_with_one_write_and_the_receive_starts_with_two() {
    let clock = Stepping(Cell::new(0));
    let mut port = Port {
        clock: &clock,
        writes: Vec::new(),
    };
    let super::MacCoreOpen { .. } = open_core(&mut port);
    assert_eq!(port.kinds(), [Write::OpenCore]);
    port.writes.clear();
    start_receive(&mut port);
    assert_eq!(
        port.kinds(),
        [Write::Resume, Write::RegdmaLink],
        "the restart's writes, nothing read"
    );
}
