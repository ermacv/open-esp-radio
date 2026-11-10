//! Diagnostics a test switches, whose clock fails the test when read while
//! they are off, and which fail it when handed a count while off.

extern crate std;

use std::cell::RefCell;
use std::rc::Rc;

use crate::{Diagnosis, Diagnostics};

#[derive(Debug, Default)]
pub(super) struct Probed {
    /// Whether they are on.
    pub(super) on: bool,
    /// How often the service asked whether they are on.
    pub(super) asked: usize,
    /// How often it read the clock; each read moves it on by one tick.
    pub(super) clock: u64,
    /// How often it handed over a count, and their sum.
    pub(super) adds: usize,
    pub(super) total: Diagnosis,
}

/// A test's diagnostics: a handle to what they saw.
#[derive(Clone, Debug, Default)]
pub(super) struct Probe(pub(super) Rc<RefCell<Probed>>);

impl Probe {
    pub(super) fn on() -> Self {
        let probe = Self::default();
        probe.switch(true);
        probe
    }

    pub(super) fn switch(&self, on: bool) {
        self.0.borrow_mut().on = on;
    }

    pub(super) fn total(&self) -> Diagnosis {
        self.0.borrow().total
    }
}

impl Diagnostics for Probe {
    fn on(&mut self) -> bool {
        let mut probed = self.0.borrow_mut();
        probed.asked += 1;
        probed.on
    }

    fn clock(&mut self) -> u64 {
        let mut probed = self.0.borrow_mut();
        assert!(
            probed.on,
            "the clock was read while the diagnostics were off"
        );
        probed.clock += 1;
        probed.clock
    }

    fn add(&mut self, counted: &Diagnosis) {
        let mut probed = self.0.borrow_mut();
        assert!(probed.on, "a count was handed over while off");
        probed.adds += 1;
        probed.total.accumulate(counted);
    }
}
