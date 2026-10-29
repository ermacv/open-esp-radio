//! A finished-list fence belongs to the engine epoch that captured it.

use crate::bluetooth;
use oer_espressif_bluetooth_engine::phase::EngineEpoch;

#[derive(Debug, Eq, PartialEq)]
pub struct EngineFinishedListObservation {
    observation: bluetooth::BluetoothSchedulerFinishedListObservation,
    epoch: EngineEpoch,
}

#[derive(Debug, Eq, PartialEq)]
pub struct EngineFinishedHardwareListObserved {
    observed: bluetooth::BluetoothSchedulerFinishedHardwareListObserved,
    pub(super) epoch: EngineEpoch,
}

pub enum EngineFinishedListPop {
    Complete,
    List {
        observed: EngineFinishedHardwareListObserved,
        remaining: EngineFinishedListObservation,
    },
}

impl EngineFinishedHardwareListObserved {
    pub const fn index(&self) -> bluetooth::BluetoothSchedulerHardwareListIndex {
        self.observed.index()
    }
}

impl EngineFinishedListObservation {
    pub(super) const fn new(
        observation: bluetooth::BluetoothSchedulerFinishedListObservation,
        epoch: EngineEpoch,
    ) -> Self {
        Self { observation, epoch }
    }
    pub fn is_empty(&self) -> bool {
        self.observation.is_empty()
    }
    pub fn pop_lowest(self) -> EngineFinishedListPop {
        match self.observation.pop_lowest() {
            bluetooth::BluetoothSchedulerFinishedListPop::Complete => {
                EngineFinishedListPop::Complete
            }
            bluetooth::BluetoothSchedulerFinishedListPop::List {
                observed,
                remaining,
            } => EngineFinishedListPop::List {
                observed: EngineFinishedHardwareListObserved {
                    observed,
                    epoch: self.epoch,
                },
                remaining: Self {
                    observation: remaining,
                    epoch: self.epoch,
                },
            },
        }
    }
    /// A detached fixture for testing the bounded drain worker.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn from_lists_for_host_test(lists: &[u8]) -> Option<Self> {
        Some(Self::new(
            bluetooth::BluetoothSchedulerFinishedListObservation::from_lists_for_host_test(lists)?,
            EngineEpoch::from_sequence(u32::MAX),
        ))
    }
}

#[derive(Debug)]
pub enum CompletionError {
    Quarantined(EngineFinishedHardwareListObserved),
    ForeignEpoch(EngineFinishedHardwareListObserved),
    WrongList(EngineFinishedHardwareListObserved),
    TransactionActive(EngineFinishedHardwareListObserved),
    Pool(super::SchedulerPoolError),
}
