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

use oer_bluetooth_le_radio::{
    EventResult, LeInstant, LeRadio, LeWindow, NonZeroRadioDuration, OutsideEpoch, RadioDuration,
    RadioTiming, TestChannel, TestPhy, TestReceive, TestReport, TestTransmit, TxPower,
};

mod payload;

pub use payload::DtmPayloadPattern;

/// Longest LE Test packet payload.
pub const DTM_MAX_PAYLOAD: usize = u8::MAX as usize;

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
    Running(DtmTest, WindowGeometry),
    Ending(DtmTest, WindowGeometry),
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
        let geometry = WindowGeometry::new(test).ok_or(DtmStartError::UnsupportedPhy)?;
        self.phase = Phase::Running(test, geometry);
        self.last = None;
        self.recurring = false;
        self.counters = DtmCounters::default();
        Ok(())
    }

    /// Stop planning. An event in progress must end first.
    pub fn end(&mut self) -> DtmStop {
        let (test, geometry) = match self.phase {
            Phase::Idle => return DtmStop::Stopped { received: 0 },
            Phase::Running(test, geometry) | Phase::Ending(test, geometry) => (test, geometry),
        };
        match self.in_progress {
            true => {
                self.phase = Phase::Ending(test, geometry);
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
        let Phase::Running(test, geometry) = self.phase else {
            return Ok(None);
        };
        if self.in_progress {
            return Ok(None);
        }
        let admitted = admitted_anchor(now, timing).ok_or(DtmEpochExhausted {
            calculation: DtmCalculation::Admission,
        })?;
        let window = plan_window(geometry, admitted, timing, self.last)?;
        // All geometry is validated before payload or history state changes.
        let request = match test {
            DtmTest::Transmit {
                channel,
                phy,
                pattern,
                length,
            } => {
                // A u8 length is bounded by the 255-byte array. Trim exactly
                // its unused tail, retaining the prefix as a mutable slice.
                let mut bytes = payload.iter_mut();
                let mut unused = usize::from(length)..DTM_MAX_PAYLOAD;
                if unused.next().is_some() {
                    bytes.nth_back(unused.len());
                }
                let bytes = bytes.into_slice();
                pattern.fill(bytes);
                DtmEvent::Transmit(TestTransmit {
                    channel,
                    phy,
                    window,
                    tx_power: self.tx_power,
                    payload_type: pattern.radio_payload_type(),
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
    pub fn report(&mut self, report: TestReport) -> Result<(), DtmCounterExhausted> {
        if !self.in_progress {
            return Ok(());
        }
        match report {
            TestReport::Nothing => increment(&mut self.counters.empty, DtmCounter::Empty)?,
            TestReport::Received { .. } => {
                increment(&mut self.counters.received, DtmCounter::Received)?
            }
            TestReport::Failed => increment(&mut self.counters.failed, DtmCounter::Failed)?,
        }
        Ok(())
    }

    /// Count how the event in progress ended; the session may plan the next.
    pub fn ended(&mut self, result: EventResult) -> Result<(), DtmCounterExhausted> {
        if !self.in_progress {
            return Ok(());
        }
        match result {
            EventResult::Executed { .. } => {
                increment(&mut self.counters.executed, DtmCounter::Executed)?
            }
            EventResult::NotExecuted => {
                increment(&mut self.counters.not_executed, DtmCounter::NotExecuted)?
            }
            EventResult::Aborted => increment(&mut self.counters.aborted, DtmCounter::Aborted)?,
            EventResult::Indeterminate => {
                increment(&mut self.counters.indeterminate, DtmCounter::Indeterminate)?
            }
        }
        self.in_progress = false;
        if let Phase::Ending(..) = self.phase {
            self.phase = Phase::Idle;
        }
        Ok(())
    }

    /// The receiver count once an ending test has drained.
    pub fn drained(&self) -> Option<u16> {
        match (self.phase, self.in_progress) {
            (Phase::Idle, false) => Some(self.counters.received.min(u32::from(u16::MAX)) as u16),
            _ => None,
        }
    }
}

/// Failure of a detached DTM window plan, with no state mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmWindowError {
    /// The selected transmitter PHY has no implemented airtime calculation.
    UnsupportedPhy,
    /// A checked time calculation left this radio epoch.
    OutsideEpoch(DtmEpochExhausted),
}

/// Plan one physical DTM air window without requiring TX authority for RX.
/// Both the portable session and the S31 lower-LL worker use this geometry.
/// Failure changes no session, payload or previous-window state.
pub fn next_window(
    test: DtmTest,
    now: LeInstant,
    timing: RadioTiming,
    previous: Option<LeWindow>,
) -> Result<LeWindow, DtmWindowError> {
    let admitted =
        admitted_anchor(now, timing).ok_or(DtmWindowError::OutsideEpoch(DtmEpochExhausted {
            calculation: DtmCalculation::Admission,
        }))?;
    let geometry = WindowGeometry::new(test).ok_or(DtmWindowError::UnsupportedPhy)?;
    plan_window(geometry, admitted, timing, previous).map_err(DtmWindowError::OutsideEpoch)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WindowGeometry {
    Transmit {
        air: NonZeroRadioDuration<LeRadio>,
        interval: RadioDuration<LeRadio>,
    },
    Receive,
}

impl WindowGeometry {
    fn new(test: DtmTest) -> Option<Self> {
        match test {
            DtmTest::Transmit { phy, length, .. } => Some(Self::Transmit {
                air: transmit_air_time(phy, length)?,
                // Evaluating I(L) over the bounded u8 length gives these
                // exact grid intervals; coded TX is refused above.
                interval: RadioDuration::from_micros(match (phy, length) {
                    (TestPhy::Le1M, 0..=37) | (TestPhy::Le2M, 0..=83) => 625,
                    (TestPhy::Le1M, 38..=115) | (TestPhy::Le2M, 84..=239) => 1_250,
                    (TestPhy::Le1M, 116..=193) | (TestPhy::Le2M, 240..=255) => 1_875,
                    (TestPhy::Le1M, 194..=255) => 2_500,
                    (TestPhy::LeCodedS8 | TestPhy::LeCodedS2, _) => return None,
                }),
            }),
            DtmTest::Receive { .. } => Some(Self::Receive),
        }
    }
}

fn plan_window(
    geometry: WindowGeometry,
    admitted: LeInstant,
    timing: RadioTiming,
    previous: Option<LeWindow>,
) -> Result<LeWindow, DtmEpochExhausted> {
    use DtmCalculation as C;
    let at = |calculation| DtmEpochExhausted { calculation };
    let window = match geometry {
        WindowGeometry::Transmit { air, interval } => {
            let anchor = match previous {
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
                            let slots = ceil_div(late.as_micros(), interval.as_micros())
                                .ok_or(at(C::SlotAlignment))?;
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
        WindowGeometry::Receive => {
            let earliest = admitted
                .checked_add(DTM_PLANNING_SLACK)
                .ok_or(at(C::InitialAnchor))?;
            let anchor = previous.map_or(earliest, |last| last.end().max(earliest));
            LeWindow::nonempty(anchor, DTM_RECEIVE_WINDOW).ok_or(at(C::AirWindow))?
        }
    };
    timing
        .reservation(window)
        .map_err(|OutsideEpoch| at(C::Reservation))?;
    Ok(window)
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
    // A u8 payload keeps the exact sum below 2,200 us.
    Some(NonZeroRadioDuration::from_micros(
        overhead.checked_add(u64::from(length).checked_mul(byte)?)?,
    ))
}

fn ceil_div(value: u64, divisor: u64) -> Option<u64> {
    value
        .checked_div(divisor)?
        .checked_add(u64::from(value.checked_rem(divisor)? != 0))
}

/// The diagnostic counter whose next observation could not be retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DtmCounter {
    Executed,
    NotExecuted,
    Aborted,
    Indeterminate,
    Empty,
    Failed,
    Received,
}

/// A DTM observation would exceed its counter. The session and observation's
/// current counter remain unchanged; the caller retains the rejected input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DtmCounterExhausted {
    pub counter: DtmCounter,
    pub value: u32,
}

fn increment(value: &mut u32, counter: DtmCounter) -> Result<(), DtmCounterExhausted> {
    *value = value.checked_add(1).ok_or(DtmCounterExhausted {
        counter,
        value: *value,
    })?;
    Ok(())
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
