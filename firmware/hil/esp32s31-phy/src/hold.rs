//! Forced due tracking in the diagnostic image only, with bounded coarse edges.
mod recorder;
mod sar_tails;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_phy::{
    PhyTargetObserver,
    state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
    tracking::observation::{Event, Operation},
};
use oer_hil_protocol::{base::RejectReason, phy::*};
use oer_time::{Clock, Duration, Instant};

pub struct Hold {
    pub started: HoldStarted,
    pub samples: heapless::Vec<HoldSample, HOLD_TICKS>,
    pub invalid: bool,
    pub running: bool,
    pub deadline: Instant,
    pub saved_debug: [u8; 2],
}
impl Hold {
    pub fn new(started: HoldStarted, saved_debug: [u8; 2]) -> Self {
        Self {
            started,
            samples: heapless::Vec::new(),
            invalid: false,
            running: true,
            deadline: EmbassyClock
                .now()
                .saturating_add(Duration::from_micros(DEFAULT_PLL_TRACK_PERIOD_MICROS)),
            saved_debug,
        }
    }
    pub fn page(&self, request: GetTrackingHold) -> Result<TrackingHold, RejectReason> {
        if request.id != self.started.id || request.first > self.started.ticks {
            return Err(RejectReason::InvalidState);
        }
        Ok(TrackingHold {
            id: request.id,
            first: request.first,
            available: self.samples.len() as u8,
            terminal: !self.running,
            invalid: self.invalid,
            sample: self.samples.get(usize::from(request.first)).cloned(),
        })
    }
}

pub struct Observer(pub recorder::Recorder, pub sar_tails::Tails);
impl Observer {
    pub fn new(tick: u8) -> Self {
        Self(recorder::Recorder::new(tick), sar_tails::Tails::default())
    }
}
impl PhyTargetObserver for &mut Observer {
    fn tx_sar_wait(&mut self, elapsed: u64, observations: u16, ready: bool) {
        self.1.observe(elapsed, observations, ready);
    }
    fn tracking_operation(&mut self, operation: Operation, event: Event) {
        let operation = match operation {
            Operation::Tracking => HoldOperation::Tracking,
            Operation::GrantProtectRfpll => HoldOperation::Rfpll,
            Operation::GrantProtectCommon => HoldOperation::Common,
            Operation::GrantProtectTransmit => HoldOperation::Transmit,
            Operation::WifiBasebandOff => HoldOperation::WifiBasebandOff,
            _ => return,
        };
        match event {
            Event::Started => {
                self.0
                    .observe(operation, true, false, EmbassyClock.now().as_micros())
            }
            Event::Completed | Event::Failed => self.0.observe(
                operation,
                false,
                event == Event::Failed,
                EmbassyClock.now().as_micros(),
            ),
            _ => {}
        }
    }
}
