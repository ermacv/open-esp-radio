//! Environment-driven airtime, independent of port commands and event dequeue.
use super::storage::{Local, WakerSlot};
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    cell::Cell,
    num::{NonZeroU32, NonZeroU64},
    task::Waker,
};
use oer_bluetooth_le_port::{PacketCount, Receive, Transmit};
use oer_time::{Clock, Duration, Instant, TimeOverflow, Timer};
use oer_time_virtual::VirtualClock;

/// Explicit propagation input. Loss applies to numbered packets per transmitter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Loss {
    /// Every matching packet reaches the receiver.
    #[default]
    None,
    /// No packets reach receivers.
    All,
    /// Drop every Nth generated packet, preserving phase across advances.
    EveryNth(NonZeroU32),
}
/// Fault/coexistence input at the next P2 publication boundary. This is separate
/// from propagation loss: an RF packet loss never manufactures a port refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stimulus {
    /// Air admission is denied before publication.
    AirDenied,
    /// A published operation was withdrawn with confirmed memory exclusion.
    Withdrawn,
    /// Hardware may still reach the operation; quarantine its owners.
    Indeterminate,
    /// Fault before publication; settle every admitted command, then Drained.
    DrainedFault,
    /// Change the radio generation before P2 reaches queued work.
    StaleEpoch,
}
#[derive(Clone, Copy)]
enum Mode {
    Off,
    Transmit {
        config: Transmit,
        last: Instant,
        sequence: u64,
    },
    Receive {
        config: Receive,
        count: u64,
    },
}
pub(crate) struct Radio {
    mode: Cell<Mode>,
    epoch: Cell<u64>,
    inputs: Local<VecDeque<Stimulus>>,
    driver: WakerSlot,
}
struct Propagation {
    radio: Rc<Radio>,
    config: Transmit,
    last: Instant,
    sequence: u64,
    received: u64,
}
/// Shared, deterministic test ether on `oer-time-virtual`. A caller supplies the
/// packet period as a test-profile input, not as a hardware timing guarantee.
/// Packets are generated at period boundaries strictly after TX starts. Only
/// radios already receiving on a matching channel and PHY collect them.
pub struct Environment {
    clock: VirtualClock,
    period: NonZeroU64,
    loss: Cell<Loss>,
    radios: Local<Vec<Rc<Radio>>>,
}
impl Environment {
    /// Create independent virtual time and a fixed packet cadence in microseconds.
    pub fn new(packet_period_us: NonZeroU64) -> Self {
        Self {
            clock: VirtualClock::new(),
            period: packet_period_us,
            loss: Cell::new(Loss::None),
            radios: Local::new(Vec::new()),
        }
    }
    /// Current monotonic coordinate; starts at the common wrap-exercising epoch.
    pub fn now(&self) -> Instant {
        self.clock.now()
    }
    /// Replace loss for packets generated after this call.
    pub fn set_loss(&self, loss: Loss) {
        self.loss.set(loss);
    }
    /// Advance virtual time and propagate whole packets in the elapsed span.
    /// Overflow leaves both time and the medium unchanged.
    pub fn advance(&self, duration: Duration) -> Result<Instant, TimeOverflow> {
        let now = self.now().checked_add(duration).ok_or(TimeOverflow)?;
        // Validate the entire batch before changing any radio or waking a task.
        let plans = self.radios.with(|radios| {
            let mut plans = Vec::new();
            for radio in radios.iter() {
                let Mode::Transmit {
                    config,
                    last,
                    sequence,
                } = radio.mode.get()
                else {
                    continue;
                };
                let number = now
                    .checked_duration_since(last)
                    .ok_or(TimeOverflow)?
                    .as_micros()
                    / self.period;
                if number == 0 {
                    continue;
                }
                let sequence_end = sequence.checked_add(number).ok_or(TimeOverflow)?;
                let received = match self.loss.get() {
                    Loss::None => number,
                    Loss::All => 0,
                    Loss::EveryNth(n) => {
                        let divisor = NonZeroU64::from(n);
                        let lost = (sequence_end / divisor)
                            .checked_sub(sequence / divisor)
                            .ok_or(TimeOverflow)?;
                        number.checked_sub(lost).ok_or(TimeOverflow)?
                    }
                };
                let elapsed = Duration::from_micros(
                    number.checked_mul(self.period.get()).ok_or(TimeOverflow)?,
                );
                plans.push(Propagation {
                    radio: radio.clone(),
                    config,
                    last: last.checked_add(elapsed).ok_or(TimeOverflow)?,
                    sequence: sequence_end,
                    received,
                });
            }
            Ok::<_, TimeOverflow>(plans)
        })?;
        self.radios.with(|radios| {
            for plan in plans {
                for radio in radios.iter() {
                    if let Mode::Receive { config, count } = radio.mode.get()
                        && config.channel == plan.config.channel
                        && config.phy == plan.config.phy
                    {
                        // PacketCount explicitly reports saturation. Several
                        // transmitters may contribute to one receiver.
                        radio.mode.set(Mode::Receive {
                            config,
                            count: count.saturating_add(plan.received),
                        });
                    }
                }
                plan.radio.mode.set(Mode::Transmit {
                    config: plan.config,
                    last: plan.last,
                    sequence: plan.sequence,
                });
            }
        });
        self.clock.advance_to(now);
        Ok(now)
    }
    pub(crate) fn attach(&self) -> Rc<Radio> {
        let radio = Rc::new(Radio {
            mode: Cell::new(Mode::Off),
            epoch: Cell::new(1),
            inputs: Local::new(VecDeque::new()),
            driver: WakerSlot::new(),
        });
        self.radios.with(|radios| radios.push(radio.clone()));
        radio
    }
    pub(crate) fn epoch(&self, radio: &Radio) -> u64 {
        radio.epoch.get()
    }
    pub(crate) fn next_epoch(&self, radio: &Radio) -> Result<u64, super::Fault> {
        let epoch = radio
            .epoch
            .get()
            .checked_add(1)
            .ok_or(super::Fault::EpochExhausted)?;
        radio.epoch.set(epoch);
        Ok(epoch)
    }
    pub(crate) fn transmit(&self, radio: &Radio, config: Transmit) {
        radio.mode.set(Mode::Transmit {
            config,
            last: self.now(),
            sequence: 0,
        });
    }
    pub(crate) fn receive(&self, radio: &Radio, config: Receive) {
        radio.mode.set(Mode::Receive { config, count: 0 });
    }
    pub(crate) fn stop(&self, radio: &Radio) -> PacketCount {
        let count = match radio.mode.replace(Mode::Off) {
            Mode::Receive { count, .. } => count,
            _ => 0,
        };
        PacketCount {
            packets: u16::try_from(count).unwrap_or(u16::MAX),
            saturated: count > u64::from(u16::MAX),
        }
    }
    pub(crate) fn is_off(&self, radio: &Radio) -> bool {
        matches!(radio.mode.get(), Mode::Off)
    }
    pub(crate) fn stimulus(&self, radio: &Radio) -> Option<Stimulus> {
        radio.inputs.with(VecDeque::pop_front)
    }
    pub(crate) fn register(&self, radio: &Radio, waker: &Waker) {
        radio.driver.register(waker);
    }
    /// Number of modeled radios with TX/RX reachability, independent of P1 reads.
    pub fn active_radios(&self) -> usize {
        self.radios.with(|radios| {
            radios
                .iter()
                .filter(|r| !matches!(r.mode.get(), Mode::Off))
                .count()
        })
    }
    pub(crate) fn inject(&self, radio: &Radio, stimulus: Stimulus) {
        radio.inputs.with(|inputs| inputs.push_back(stimulus));
        radio.driver.wake();
    }
    pub(crate) fn detach(&self, radio: &Radio) {
        self.stop(radio);
        radio.inputs.with(VecDeque::clear);
        radio.driver.clear();
    }
}
impl Clock for Environment {
    fn now(&self) -> Instant {
        self.clock.now()
    }
}
impl Timer for Environment {
    fn wait_until(&self, deadline: Instant) -> impl core::future::Future<Output = ()> {
        self.clock.wait_until(deadline)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn installation_epoch_refuses_to_wrap() {
        let air = Environment::new(NonZeroU64::MIN);
        let radio = air.attach();
        radio.epoch.set(u64::MAX);
        assert_eq!(air.next_epoch(&radio), Err(crate::Fault::EpochExhausted));
        assert_eq!(air.epoch(&radio), u64::MAX);
    }
    #[test]
    fn a_transmitter_timestamp_ahead_of_the_clock_refuses_the_batch() {
        let air = Environment::new(NonZeroU64::MIN);
        let radio = air.attach();
        let now = air.now();
        radio.mode.set(Mode::Transmit {
            config: crate::tests::tx(19),
            last: now.checked_add(Duration::from_micros(2)).unwrap(),
            sequence: 0,
        });
        assert_eq!(air.advance(Duration::from_micros(1)), Err(TimeOverflow));
        assert_eq!(air.now(), now);
        assert_eq!(air.active_radios(), 1);
    }
    #[test]
    fn sequence_overflow_keeps_the_entire_medium_and_time_unchanged() {
        let air = Environment::new(NonZeroU64::MIN);
        let first = air.attach();
        let overflowing = air.attach();
        let receiver = air.attach();
        let config = crate::tests::tx(19);
        air.transmit(&first, config);
        air.transmit(&overflowing, config);
        air.receive(&receiver, crate::tests::rx(19));
        let now = air.now();
        overflowing.mode.set(Mode::Transmit {
            config,
            last: now,
            sequence: u64::MAX,
        });
        assert_eq!(air.advance(Duration::from_micros(1)), Err(TimeOverflow));
        assert_eq!(air.now(), now);
        assert!(
            matches!(first.mode.get(), Mode::Transmit { last, sequence: 0, .. } if last == now)
        );
        assert_eq!(air.stop(&receiver), PacketCount::default());
    }
}
