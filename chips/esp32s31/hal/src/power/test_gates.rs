//! A record of the platform gates' edges, for host tests that check their
//! order against other writes.

use std::{boxed::Box, vec::Vec};

use super::platform::{PlatformClock, PlatformGates, PlatformHold, WifiPowerGate};

/// One edge the recording gates saw.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GateEvent {
    Acquire(PlatformClock),
    Release(PlatformClock),
    OpenWifiPower,
    CloseWifiPower,
}

/// Platform gates that tell `sink` of every edge and keep the holds out.
pub(crate) struct RecordingGates {
    sink: Box<dyn FnMut(GateEvent)>,
    held: Vec<PlatformClock>,
}

impl RecordingGates {
    pub(crate) fn new() -> Self {
        Self::logging(|_| {})
    }

    pub(crate) fn logging(sink: impl FnMut(GateEvent) + 'static) -> Self {
        Self {
            sink: Box::new(sink),
            held: Vec::new(),
        }
    }

    /// The holds out now, in the order they were taken.
    pub(crate) fn held(&self) -> &[PlatformClock] {
        &self.held
    }
}

impl PlatformGates for RecordingGates {
    fn acquire(&mut self, clock: PlatformClock) -> PlatformHold {
        (self.sink)(GateEvent::Acquire(clock));
        self.held.push(clock);
        PlatformHold::for_test(clock)
    }
    fn release(&mut self, hold: PlatformHold) {
        let clock = hold.clock();
        (self.sink)(GateEvent::Release(clock));
        let Some(index) = self.held.iter().position(|held| *held == clock) else {
            panic!("{clock:?} is released without its hold");
        };
        self.held.remove(index);
    }
}

impl WifiPowerGate for RecordingGates {
    fn open_wifi_power(&mut self) {
        (self.sink)(GateEvent::OpenWifiPower);
    }
    fn close_wifi_power(&mut self) {
        (self.sink)(GateEvent::CloseWifiPower);
    }
}
