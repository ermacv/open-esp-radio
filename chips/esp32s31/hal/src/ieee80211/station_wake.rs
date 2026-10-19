//! Station TBTT schedule ownership.
//!
//! The restricted PAC programs the reviewed station TBTT fields. This module
//! records whether a schedule runs for the Wi-Fi route, so an interval change
//! reaches only a running schedule.

use crate::ieee80211::MacBorrow;
use core::cell::RefMut;
use oer_esp32s31_pac::WifiRadioRegisters;

use oer_esp32s31_pac::StaTbttSchedule;

/// Why a station TBTT schedule operation was refused.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaTbttScheduleError {
    /// No station TBTT schedule runs.
    NotScheduled,
}

/// Station TBTT state of one Wi-Fi route.
#[derive(Debug, Default)]
pub(crate) struct StationWakeState {
    tbtt_scheduled: bool,
}

struct StationWakeRegisters<'registers> {
    mac: MacBorrow<'registers>,
    state: StateBorrow<'registers>,
}

enum StateBorrow<'registers> {
    Owned(&'registers mut StationWakeState),
    Published(RefMut<'registers, StationWakeState>),
}

impl StationWakeRegisters<'_> {
    fn parts_mut(&mut self) -> (&mut WifiRadioRegisters, &mut StationWakeState) {
        let state = match &mut self.state {
            StateBorrow::Owned(state) => &mut **state,
            StateBorrow::Published(state) => &mut **state,
        };
        (&mut self.mac, state)
    }

    fn state(&self) -> &StationWakeState {
        match &self.state {
            StateBorrow::Owned(state) => state,
            StateBorrow::Published(state) => state,
        }
    }
}

/// Closed HAL authority for the station TBTT transactions.
///
/// This type exposes neither the Wi-Fi register set nor `Deref`.
pub struct StationWakeHal<'registers> {
    registers: StationWakeRegisters<'registers>,
}

impl<'registers> StationWakeHal<'registers> {
    pub(crate) fn from_owned(
        registers: &'registers mut WifiRadioRegisters,
        state: &'registers mut StationWakeState,
    ) -> Self {
        Self {
            registers: StationWakeRegisters {
                mac: MacBorrow::Owned(registers),
                state: StateBorrow::Owned(state),
            },
        }
    }

    pub(crate) fn from_published(
        registers: RefMut<'registers, WifiRadioRegisters>,
        state: RefMut<'registers, StationWakeState>,
    ) -> Self {
        Self {
            registers: StationWakeRegisters {
                mac: MacBorrow::Published(registers),
                state: StateBorrow::Published(state),
            },
        }
    }

    /// Start the station TBTT schedule, as the vendor power manager does at
    /// each beacon that moves the schedule. The event fires on the route's
    /// power interrupt.
    pub fn start_station_tbtt(&mut self, schedule: StaTbttSchedule) {
        let (registers, state) = self.registers.parts_mut();
        registers.start_station_tbtt(schedule);
        state.tbtt_scheduled = true;
    }

    /// Replace the running station TBTT interval.
    ///
    /// # Errors
    ///
    /// No station TBTT schedule runs.
    pub fn set_station_tbtt_interval(
        &mut self,
        interval_micros: u32,
    ) -> Result<(), StaTbttScheduleError> {
        let (registers, state) = self.registers.parts_mut();
        if !state.tbtt_scheduled {
            return Err(StaTbttScheduleError::NotScheduled);
        }
        registers.set_station_tbtt_interval(interval_micros);
        Ok(())
    }

    /// Replace the station TBTT lead and the wake lead beside it.
    pub fn set_station_tbtt_ahead(&mut self, ahead_micros: u16, wake_ahead_micros: u16) {
        let (registers, _) = self.registers.parts_mut();
        registers.set_station_tbtt_ahead(ahead_micros, wake_ahead_micros);
    }

    /// Stop the station TBTT schedule. Stopping an idle schedule is a no-op
    /// on the vendor disable leaf and leaves it idle.
    pub fn stop_station_tbtt(&mut self) {
        let (registers, state) = self.registers.parts_mut();
        registers.stop_station_tbtt();
        state.tbtt_scheduled = false;
    }

    /// Whether a station TBTT schedule runs.
    pub fn station_tbtt_scheduled(&self) -> bool {
        self.registers.state().tbtt_scheduled
    }
}

#[cfg(test)]
mod tests;
