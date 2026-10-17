//! Direct Test Mode test planning and counting.
//!
//! [`DtmSession`] plans one test event at a time. A transmitter test places
//! its packets on the `I(L)` grid of Core 6.0 Vol 6 Part F section 4.1.6 and
//! skips grid points that are no longer admissible; a receiver test listens
//! in back-to-back windows and counts every packet with a valid CRC. Ending a
//! test first lets the event in progress leave the schedule. The caller
//! correlates the planned event with what the radio reports and hands the
//! session the event's [`TestReport`] and [`EventResult`].

use core::num::NonZeroU64;

use oer_bluetooth_radio::{
    EventResult, LeInstant, LeRadio, LeWindow, NonZeroRadioDuration, OutsideEpoch, RadioDuration,
    RadioTiming, TestChannel, TestPayloadType, TestPhy, TestReceive, TestReport, TestTransmit,
    TxPower,
};

mod payload;

pub use payload::DtmPayloadPattern;

/// Longest LE Test packet payload.
pub const DTM_MAX_PAYLOAD: usize = 255;

/// One planned test event, for the caller to check against its backend's
/// capabilities and submit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmEvent<'payload> {
    /// A transmitter event.
    Transmit(TestTransmit<'payload>),
    /// A receiver event.
    Receive(TestReceive),
}

/// Length of one receiver listening window.
pub const DTM_RECEIVE_WINDOW: NonZeroRadioDuration<LeRadio> =
    NonZeroRadioDuration::from_micros(NonZeroU64::new(5_000).unwrap());

/// Slack between planning the first event of a test and the earliest instant
/// the backend admits, covering the time until the request reaches it.
pub const DTM_PLANNING_SLACK: RadioDuration<LeRadio> = RadioDuration::from_micros(500);

/// Slack of a later transmitter packet, planned as soon as the previous one
/// ended. It covers the request's way to the backend so that a grid slot the
/// request cannot reach in time is skipped at planning rather than refused,
/// which would cost a retry and a further interval.
pub const DTM_RECURRING_TRANSMIT_SLACK: RadioDuration<LeRadio> = RadioDuration::from_micros(100);

/// One test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmTest {
    /// Transmit test packets.
    Transmit {
        /// RF channel.
        channel: TestChannel,
        /// PHY.
        phy: TestPhy,
        /// Payload pattern.
        pattern: DtmPayloadPattern,
        /// Payload length in bytes.
        length: u8,
    },
    /// Receive and count test packets.
    Receive {
        /// RF channel.
        channel: TestChannel,
        /// PHY.
        phy: TestPhy,
    },
}

/// Why a test did not start.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmStartError {
    /// A test is running or ending.
    Active,
    /// Coded transmitter timing is not implemented.
    UnsupportedPhy,
}

/// Result of ending a test.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmStop {
    /// The test stopped; `received` counts the receiver's packets and is zero
    /// for a transmitter.
    Stopped {
        /// Received packets.
        received: u16,
    },
    /// Cancel the event in progress and wait for it to end.
    Draining,
}

/// Counters of the current test.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DtmCounters {
    /// Events that ran.
    pub executed: u32,
    /// Events that left the schedule without running.
    pub not_executed: u32,
    /// Events withdrawn after publication whose execution the hardware
    /// left unconfirmed.
    pub aborted: u32,
    /// Events whose execution and device withdrawal are unknown after a fault.
    pub indeterminate: u32,
    /// Receiver events that returned no packet.
    pub empty: u32,
    /// Receiver events whose packet failed its check.
    pub failed: u32,
    /// Receiver packets with a valid CRC.
    pub received: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    Idle,
    Running(DtmTest),
    Ending(DtmTest),
}

/// Plans the events of one Direct Test Mode test.
#[derive(Debug)]
pub struct DtmSession {
    phase: Phase,
    tx_power: TxPower,
    in_progress: bool,
    last: Option<LeWindow>,
    recurring: bool,
    counters: DtmCounters,
}

/// Calculation that prevented a required DTM continuation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmCalculation {
    /// The earliest anchor admitted by preparation and guard.
    Admission,
    /// First-packet or receive-window planning slack.
    InitialAnchor,
    /// The next point on the previous transmitter's interval grid.
    NextAnchor,
    /// The earliest recurring transmitter point reachable now.
    ReachableAnchor,
    /// The duration of skipped grid slots.
    SlotAlignment,
    /// The air window of a test event.
    AirWindow,
    /// Its preparation reservation.
    Reservation,
}

/// A required DTM continuation lies outside the radio epoch. The session
/// retains its test, history and counters; Test End remains available.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmEpochExhausted {
    /// The calculation whose result left the epoch.
    pub calculation: DtmCalculation,
}

impl DtmSession {
    /// A session whose events use `tx_power`.
    pub const fn new(tx_power: TxPower) -> Self {
        Self {
            phase: Phase::Idle,
            tx_power,
            in_progress: false,
            last: None,
            recurring: false,
            counters: DtmCounters {
                executed: 0,
                not_executed: 0,
                aborted: 0,
                indeterminate: 0,
                empty: 0,
                failed: 0,
                received: 0,
            },
        }
    }

    /// Whether a test is running or ending.
    pub const fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::Idle)
    }

    /// Whether a planned event has not ended yet.
    pub const fn in_progress(&self) -> bool {
        self.in_progress
    }

    /// Counters of the current or last test.
    pub const fn counters(&self) -> DtmCounters {
        self.counters
    }

    /// Start `test`.
    pub fn start(&mut self, test: DtmTest) -> Result<(), DtmStartError> {
        if self.is_active() {
            return Err(DtmStartError::Active);
        }
        if let DtmTest::Transmit { phy, length, .. } = test
            && transmit_air_time(phy, length).is_none()
        {
            return Err(DtmStartError::UnsupportedPhy);
        }
        self.phase = Phase::Running(test);
        self.last = None;
        self.recurring = false;
        self.counters = DtmCounters::default();
        Ok(())
    }

    /// Stop planning. An event in progress must end first.
    pub fn end(&mut self) -> DtmStop {
        let test = match self.phase {
            Phase::Idle => return DtmStop::Stopped { received: 0 },
            Phase::Running(test) | Phase::Ending(test) => test,
        };
        match self.in_progress {
            true => {
                self.phase = Phase::Ending(test);
                DtmStop::Draining
            }
            false => {
                self.phase = Phase::Idle;
                DtmStop::Stopped {
                    received: received(test, self.counters),
                }
            }
        }
    }

    /// The next event request, when the session needs one.
    ///
    /// `now` is a fresh backend time and `timing` its admission rule. A
    /// transmitter request borrows its payload from `payload`.
    pub fn next_request<'payload>(
        &mut self,
        now: LeInstant,
        timing: RadioTiming,
        payload: &'payload mut [u8; DTM_MAX_PAYLOAD],
    ) -> Result<Option<DtmEvent<'payload>>, DtmEpochExhausted> {
        use DtmCalculation as C;
        let Phase::Running(test) = self.phase else {
            return Ok(None);
        };
        if self.in_progress {
            return Ok(None);
        }
        let at = |calculation| DtmEpochExhausted { calculation };
        let admitted = admitted_anchor(now, timing).ok_or(at(C::Admission))?;
        let window = match test {
            DtmTest::Transmit { phy, length, .. } => {
                // Start admits only transmitter PHYs with a bounded airtime.
                let air =
                    transmit_air_time(phy, length).expect("start validated the transmitter PHY");
                let interval = packet_interval(air.get());
                let anchor = match self.last {
                    None => admitted
                        .checked_add(DTM_PLANNING_SLACK)
                        .ok_or(at(C::InitialAnchor))?,
                    Some(last) => {
                        let first = last
                            .start()
                            .checked_add(interval)
                            .ok_or(at(C::NextAnchor))?;
                        let reachable = admitted
                            .checked_add(DTM_RECURRING_TRANSMIT_SLACK)
                            .ok_or(at(C::ReachableAnchor))?;
                        // A grid point the request still reaches is the next
                        // one; otherwise the first grid point after it.
                        match reachable.checked_duration_since(first) {
                            None => first,
                            Some(late) => {
                                let slots = late.as_micros().div_ceil(interval.as_micros());
                                interval
                                    .checked_mul(slots)
                                    .and_then(|skipped| first.checked_add(skipped))
                                    .ok_or(at(C::SlotAlignment))?
                            }
                        }
                    }
                };
                LeWindow::nonempty(anchor, air).ok_or(at(C::AirWindow))?
            }
            DtmTest::Receive { .. } => {
                let earliest = admitted
                    .checked_add(DTM_PLANNING_SLACK)
                    .ok_or(at(C::InitialAnchor))?;
                let anchor = self.last.map_or(earliest, |last| last.end().max(earliest));
                LeWindow::nonempty(anchor, DTM_RECEIVE_WINDOW).ok_or(at(C::AirWindow))?
            }
        };
        timing
            .reservation(window)
            .map_err(|OutsideEpoch| at(C::Reservation))?;
        // All geometry is validated before payload or history state changes.
        let request = match test {
            DtmTest::Transmit {
                channel,
                phy,
                pattern,
                length,
            } => {
                let bytes = &mut payload[..usize::from(length)];
                pattern.fill(bytes);
                DtmEvent::Transmit(TestTransmit {
                    channel,
                    phy,
                    window,
                    tx_power: self.tx_power,
                    payload_type: TestPayloadType::new(pattern.payload_type())
                        .expect("every pattern has a test payload type"),
                    payload: bytes,
                })
            }
            DtmTest::Receive { channel, phy } => {
                let recurring = core::mem::replace(&mut self.recurring, true);
                DtmEvent::Receive(TestReceive {
                    channel,
                    phy,
                    window,
                    recurring,
                    tx_power: self.tx_power,
                })
            }
        };
        self.last = Some(window);
        self.in_progress = true;
        Ok(Some(request))
    }

    /// The backend refused the planned request; plan again later.
    pub fn refused(&mut self) {
        self.in_progress = false;
    }

    /// Count the receiver report of the event in progress.
    pub fn report(&mut self, report: TestReport) {
        if !self.in_progress {
            return;
        }
        match report {
            TestReport::Nothing => self.counters.empty += 1,
            TestReport::Received { .. } => self.counters.received += 1,
            TestReport::Failed => self.counters.failed += 1,
        }
    }

    /// Count how the event in progress ended; the session may plan the next.
    pub fn ended(&mut self, result: EventResult) {
        if !self.in_progress {
            return;
        }
        match result {
            EventResult::Executed { .. } => self.counters.executed += 1,
            EventResult::NotExecuted => self.counters.not_executed += 1,
            EventResult::Aborted => self.counters.aborted += 1,
            EventResult::Indeterminate => self.counters.indeterminate += 1,
        }
        self.in_progress = false;
        if let Phase::Ending(_) = self.phase {
            self.phase = Phase::Idle;
        }
    }

    /// The receiver count once an ending test has drained.
    pub fn drained(&self) -> Option<u16> {
        match (self.phase, self.in_progress) {
            (Phase::Idle, false) => Some(self.counters.received.min(u32::from(u16::MAX)) as u16),
            _ => None,
        }
    }
}

fn received(test: DtmTest, counters: DtmCounters) -> u16 {
    match test {
        DtmTest::Transmit { .. } => 0,
        DtmTest::Receive { .. } => counters.received.min(u32::from(u16::MAX)) as u16,
    }
}

/// The earliest anchor the backend admits at `now`.
fn admitted_anchor(now: LeInstant, timing: RadioTiming) -> Option<LeInstant> {
    now.checked_add(timing.preparation_lead)?
        .checked_add(timing.admission_guard)
}

/// Air time of one LE Test packet: preamble, access address, header,
/// payload and CRC; `None` for a PHY without a transmitter airtime.
fn transmit_air_time(phy: TestPhy, length: u8) -> Option<NonZeroRadioDuration<LeRadio>> {
    // The packet overhead, and the airtime of one payload byte.
    let (overhead, byte) = match phy {
        TestPhy::Le1M => (const { NonZeroU64::new((1 + 4 + 2 + 3) * 8).unwrap() }, 8),
        TestPhy::Le2M => (const { NonZeroU64::new((2 + 4 + 2 + 3) * 4).unwrap() }, 4),
        TestPhy::LeCodedS8 | TestPhy::LeCodedS2 => return None,
    };
    // A u8 payload keeps the sum below 2,200 us: it never saturates.
    Some(NonZeroRadioDuration::from_micros(
        overhead.saturating_add(u64::from(length) * byte),
    ))
}

/// `I(L) = ceil((L + 249) / 625) * 625` microseconds for a packet of `L`
/// microseconds.
fn packet_interval(air: RadioDuration<LeRadio>) -> RadioDuration<LeRadio> {
    // Only the LE 1M/2M airtime of a u8 payload reaches this scalar formula:
    // air <= (1 + 4 + 2 + 255 + 3) * 8 = 2120 us; the interval is <= 2500 us.
    RadioDuration::from_micros((air.as_micros() + 249).div_ceil(625) * 625)
}

#[cfg(test)]
mod tests;
