//! Environment-driven airtime, independent of port commands and event dequeue.
#![allow(
    clippy::indexing_slicing,
    reason = "radio IDs are private, issued by this vector, and never removed"
)]
use alloc::{collections::VecDeque, vec::Vec};
use core::{cell::RefCell, num::NonZeroU32, task::Waker};
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
struct Radio {
    mode: Mode,
    epoch: u64,
    inputs: VecDeque<Stimulus>,
    driver: Option<Waker>,
}
/// Shared, deterministic test ether on `oer-time-virtual`. A caller supplies the
/// packet period as a test-profile input, not as a hardware timing guarantee.
/// Packets are generated at period boundaries strictly after TX starts. Only
/// radios already receiving on a matching channel and PHY collect them.
pub struct Environment {
    clock: VirtualClock,
    period: core::num::NonZeroU64,
    loss: core::cell::Cell<Loss>,
    radios: RefCell<Vec<Radio>>,
}
impl Environment {
    /// Create independent virtual time and a fixed packet cadence in microseconds.
    pub fn new(packet_period_us: core::num::NonZeroU64) -> Self {
        Self {
            clock: VirtualClock::new(),
            period: packet_period_us,
            loss: core::cell::Cell::new(Loss::None),
            radios: RefCell::new(Vec::new()),
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
        let mut radios = self.radios.borrow_mut();
        for tx in 0..radios.len() {
            let Mode::Transmit {
                config,
                last,
                sequence,
            } = radios[tx].mode
            else {
                continue;
            };
            let number = now.saturating_duration_since(last).as_micros() / self.period.get();
            if number == 0 {
                continue;
            }
            let sequence_end = sequence.saturating_add(number);
            let received = match self.loss.get() {
                Loss::None => number,
                Loss::All => 0,
                Loss::EveryNth(n) => number.saturating_sub(
                    sequence_end / u64::from(n.get()) - sequence / u64::from(n.get()),
                ),
            };
            for (rx, radio) in radios.iter_mut().enumerate() {
                if tx == rx {
                    continue;
                }
                if let Mode::Receive {
                    config: receiver,
                    count,
                } = &mut radio.mode
                    && receiver.channel == config.channel
                    && receiver.phy == config.phy
                {
                    *count = count.saturating_add(received);
                }
            }
            // number * period is bounded by the elapsed span used above.
            let elapsed = Duration::from_micros(number.saturating_mul(self.period.get()));
            let last = last.checked_add(elapsed).ok_or(TimeOverflow)?;
            radios[tx].mode = Mode::Transmit {
                config,
                last,
                sequence: sequence_end,
            };
        }
        drop(radios);
        self.clock.advance_to(now);
        Ok(now)
    }
    pub(crate) fn attach(&self) -> usize {
        let mut radios = self.radios.borrow_mut();
        let id = radios.len();
        radios.push(Radio {
            mode: Mode::Off,
            epoch: 1,
            inputs: VecDeque::new(),
            driver: None,
        });
        id
    }
    pub(crate) fn epoch(&self, id: usize) -> u64 {
        self.radios.borrow()[id].epoch
    }
    pub(crate) fn next_epoch(&self, id: usize) -> Result<u64, super::Fault> {
        let mut radios = self.radios.borrow_mut();
        let epoch = radios[id]
            .epoch
            .checked_add(1)
            .ok_or(super::Fault::EpochExhausted)?;
        radios[id].epoch = epoch;
        Ok(epoch)
    }
    pub(crate) fn transmit(&self, id: usize, config: Transmit) {
        self.radios.borrow_mut()[id].mode = Mode::Transmit {
            config,
            last: self.now(),
            sequence: 0,
        };
    }
    pub(crate) fn receive(&self, id: usize, config: Receive) {
        self.radios.borrow_mut()[id].mode = Mode::Receive { config, count: 0 };
    }
    pub(crate) fn stop(&self, id: usize) -> PacketCount {
        let mode = core::mem::replace(&mut self.radios.borrow_mut()[id].mode, Mode::Off);
        let count = match mode {
            Mode::Receive { count, .. } => count,
            _ => 0,
        };
        PacketCount {
            packets: u16::try_from(count).unwrap_or(u16::MAX),
            saturated: count > u64::from(u16::MAX),
        }
    }
    pub(crate) fn is_off(&self, id: usize) -> bool {
        matches!(self.radios.borrow()[id].mode, Mode::Off)
    }
    pub(crate) fn stimulus(&self, id: usize) -> Option<Stimulus> {
        self.radios.borrow_mut()[id].inputs.pop_front()
    }
    pub(crate) fn register(&self, id: usize, waker: &Waker) {
        self.radios.borrow_mut()[id].driver = Some(waker.clone());
    }
    /// Number of modeled radios with TX/RX reachability, independent of P1 reads.
    pub fn active_radios(&self) -> usize {
        self.radios
            .borrow()
            .iter()
            .filter(|r| !matches!(r.mode, Mode::Off))
            .count()
    }
    pub(crate) fn inject(&self, id: usize, stimulus: Stimulus) {
        let wake = {
            let mut radios = self.radios.borrow_mut();
            radios[id].inputs.push_back(stimulus);
            radios[id].driver.clone()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
    pub(crate) fn detach(&self, id: usize) {
        self.stop(id);
        let mut radios = self.radios.borrow_mut();
        radios[id].driver = None;
        radios[id].inputs.clear();
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
        let air = Environment::new(core::num::NonZeroU64::MIN);
        let id = air.attach();
        air.radios.borrow_mut()[id].epoch = u64::MAX;
        assert_eq!(air.next_epoch(id), Err(crate::Fault::EpochExhausted));
        assert_eq!(air.epoch(id), u64::MAX);
    }
}
