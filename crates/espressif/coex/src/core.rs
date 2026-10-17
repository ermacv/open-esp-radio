use crate::{
    COEX_TIMER_COUNT, CoexClient, CoexClientRequest, CoexClockHardware, CoexError,
    CoexEventDurations, CoexEventId, CoexTimerHardware, CoexTimerIndex, model::CoexRequest,
    program_timer, timer_index,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CoexStatus {
    pub enabled: bool,
    /// Successfully programmed requests retained by software, not RF grants.
    pub active_timers: u8,
    /// Timers touched by a failed operation whose effects are not confirmed.
    /// These must be disabled before another request can be programmed.
    pub uncertain_timers: u8,
}

pub struct CoexCore {
    enabled: bool,
    active: [Option<CoexRequest>; COEX_TIMER_COUNT],
    uncertain_timers: u8,
    durations: CoexEventDurations,
}

impl CoexCore {
    /// A disabled core. Event priorities are read from the radio arbiter's
    /// table through [`CoexTimerHardware::pti`] when a request is programmed.
    pub const fn new() -> Self {
        Self {
            enabled: false,
            active: [None; COEX_TIMER_COUNT],
            uncertain_timers: 0,
            durations: CoexEventDurations::reviewed_vendor(),
        }
    }

    pub fn enable(&mut self) {
        self.enabled = true;
    }

    pub fn disable<H: CoexTimerHardware>(&mut self, hardware: &mut H) -> Result<(), CoexError> {
        for index in CoexTimerIndex::ALL {
            if self.active[usize::from(index.value())].is_some()
                || self.uncertain_timers & timer_bit(index) != 0
            {
                self.disable_timer(hardware, index)?;
            }
        }
        self.enabled = false;
        Ok(())
    }

    /// Arm the mapped vendor timer for one event.
    ///
    /// The explicit enabled guard is Rust lifecycle policy. With that
    /// precondition satisfied, unmapped events follow `coex_core_request` and
    /// return the vendor invalid-event status instead of becoming no-ops.
    pub fn request_wifi<H: CoexTimerHardware, C: CoexClockHardware>(
        &mut self,
        hardware: &mut H,
        clock: &mut C,
        request: CoexClientRequest,
    ) -> Result<CoexTimerIndex, CoexError> {
        self.request(hardware, clock, CoexClient::Wifi, request)
    }

    pub fn request_bluetooth<H: CoexTimerHardware, C: CoexClockHardware>(
        &mut self,
        hardware: &mut H,
        clock: &mut C,
        request: CoexClientRequest,
    ) -> Result<CoexTimerIndex, CoexError> {
        self.request(hardware, clock, CoexClient::Bluetooth, request)
    }

    fn request<H: CoexTimerHardware, C: CoexClockHardware>(
        &mut self,
        hardware: &mut H,
        clock: &mut C,
        client: CoexClient,
        request: CoexClientRequest,
    ) -> Result<CoexTimerIndex, CoexError> {
        if !self.enabled {
            return Err(CoexError::Disabled);
        }
        if self.uncertain_timers != 0 {
            return Err(CoexError::RecoveryRequired);
        }
        let index = timer_index(request.event).ok_or(CoexError::InvalidEvent)?;
        // Any fallible backend call may have written hardware before failing.
        // Record the cleanup obligation before the first such call, including
        // clock failures after configuration and failed publication itself.
        self.uncertain_timers |= timer_bit(index);
        let pti = hardware.pti(request.event);
        program_timer(
            hardware,
            clock,
            index,
            client,
            pti,
            request.latency,
            request.duration,
        )?;
        hardware.enable(index)?;
        self.active[usize::from(index.value())] = Some(CoexRequest { client, request });
        self.uncertain_timers &= !timer_bit(index);
        Ok(index)
    }

    pub fn release<H: CoexTimerHardware>(
        &mut self,
        hardware: &mut H,
        event: CoexEventId,
    ) -> Result<CoexTimerIndex, CoexError> {
        let index = timer_index(event).ok_or(CoexError::InvalidEvent)?;
        self.disable_timer(hardware, index)?;
        Ok(index)
    }

    fn disable_timer<H: CoexTimerHardware>(
        &mut self,
        hardware: &mut H,
        index: CoexTimerIndex,
    ) -> Result<(), CoexError> {
        self.uncertain_timers |= timer_bit(index);
        hardware.disable(index)?;
        self.active[usize::from(index.value())] = None;
        self.uncertain_timers &= !timer_bit(index);
        Ok(())
    }

    pub fn status(&self) -> CoexStatus {
        let mut active_timers = 0_u8;
        for (index, request) in self.active.iter().enumerate() {
            if request.is_some() {
                active_timers |= 1 << index;
            }
        }
        CoexStatus {
            enabled: self.enabled,
            active_timers,
            uncertain_timers: self.uncertain_timers,
        }
    }

    pub const fn event_duration(&self, event: CoexEventId) -> Option<u32> {
        self.durations.duration(event)
    }
}

impl Default for CoexCore {
    fn default() -> Self {
        Self::new()
    }
}

const fn timer_bit(index: CoexTimerIndex) -> u8 {
    1 << index.value()
}
