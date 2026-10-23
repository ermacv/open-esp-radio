// Exercise the actual image observer and root wrapper with a virtual clock/UART.
#[path = "../../../../firmware/hil/esp32s31-phy/src/hold/metrics.rs"]
mod metrics;
#[path = "../../../../firmware/hil/esp32s31-phy/src/hold/notes.rs"]
mod notes;
#[path = "../../../../firmware/hil/esp32s31-phy/src/hold/observer.rs"]
mod observer;
#[path = "../../../../firmware/hil/esp32s31-phy/src/hold/recorder.rs"]
mod recorder;
#[path = "../../../../firmware/hil/esp32s31-phy/src/hold/sar_tails.rs"]
mod sar_tails;

use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    pin::pin,
    task::{Context, Poll, Waker},
};
use notes::Notes;
use oer_diagnostics::Note;
use oer_esp32s31_phy::tracking::observation::{
    EngineTiming, Event, GainTiming, Operation, ReadinessTrace, RxGainExecution,
};
use oer_hil_protocol::phy::HoldPolls;
use oer_time::{Clock, Duration};
use oer_time_virtual::VirtualClock;

/// Runs a future that never waits.
fn block<F: Future>(future: F) -> F::Output {
    let mut future = pin!(future);
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(output) => output,
        Poll::Pending => panic!("a report waited"),
    }
}

struct Uart<'a> {
    active: &'a Cell<bool>,
    returned: &'a Cell<bool>,
    bytes: &'a RefCell<String>,
}
impl Notes for Uart<'_> {
    async fn note(&mut self, note: &Note<'static>) {
        assert!(!self.active.get(), "UART inside a measured interval");
        assert!(self.returned.get(), "UART before root return");
        let mut text = self.bytes.borrow_mut();
        oer_diagnostics::render::note(note, &mut *text).expect("renderable note");
        text.push('\n');
    }
}

/// A sink that records how many notes arrived.
struct Counted(usize);
impl Notes for Counted {
    async fn note(&mut self, _: &Note<'static>) {
        self.0 += 1;
    }
}

fn check_quiet_root(failed: bool) {
    let clock = VirtualClock::<1>::new();
    let active = Cell::new(false);
    let returned = Cell::new(false);
    let bytes = RefCell::new(String::new());
    let mut uart = Uart {
        active: &active,
        returned: &returned,
        bytes: &bytes,
    };
    let mut aggregate = HoldPolls::default();
    let mut trace = ReadinessTrace::default();
    let metrics = RefCell::new(metrics::Metrics::default());
    let mut observer =
        observer::Observer::new(1, &mut aggregate, Some(&mut trace), &clock, &mut uart);
    observer.metrics = Some(&metrics);
    let mut phase = 0;
    {
        let root = poll_fn(|_| {
            match phase {
                0 => {
                    active.set(true);
                    observer.tracking_operation(Operation::Tracking, Event::Started);
                    for operation in [
                        Operation::GrantProtectRfpll,
                        Operation::GrantProtectCommon,
                        Operation::GrantProtectTransmit,
                        Operation::WifiBasebandOff,
                    ] {
                        observer.tracking_operation(operation, Event::Started);
                        clock
                            .advance(Duration::from_micros(3))
                            .expect("advance clock");
                        observer.tracking_operation(operation, Event::Completed);
                    }
                    assert!(observer.rfpll_engine_trace().is_none());
                    assert!(observer.readiness_trace().is_some());
                    for operation in [
                        Operation::Rfpll,
                        Operation::BluetoothIeee802154Power,
                        Operation::WifiI2c,
                        Operation::WifiPower,
                        Operation::Dcode,
                        Operation::RxGain,
                        Operation::ChannelRestore,
                        Operation::Temperature,
                        Operation::Temperature,
                    ] {
                        observer.engine_completed(
                            operation,
                            EngineTiming {
                                polls: 2,
                                pending: 1,
                                total_micros: 7,
                                maximum_micros: 4,
                                ..EngineTiming::default()
                            },
                        );
                    }
                    for operation in [Operation::BluetoothIeee802154Power, Operation::WifiPower] {
                        observer.power_gain_completed(
                            operation,
                            GainTiming {
                                lower_micros: 2,
                                arithmetic_micros: 3,
                                publication_micros: 4,
                            },
                        );
                    }
                    observer.tx_sar_wait(3, 2, true);
                    observer.rx_gain_execution(RxGainExecution {
                        maximum_parts_micros: [2, 3, 4],
                        maximum_engine_actions: [0x8b, 11],
                        maximum_engine_micros: [10, 7],
                        i2c_poll_micros: [[5, 2], [10, 7]],
                        i2c_samples: [1, 3],
                        ..RxGainExecution::default()
                    });
                    for first in [0, 8] {
                        observer.channel_action_completed(
                            14,
                            &EngineTiming {
                                maximum_micros: 8,
                                maximum_ready_micros: 6,
                                ..EngineTiming::default()
                            },
                            [first, first + 8],
                        );
                    }
                    // Unmeasured callbacks must also stay silent inside Tracking.
                    observer.tracking_operation(Operation::Temperature, Event::Completed);
                    phase = 1;
                    Poll::Pending
                }
                1 => {
                    observer.tracking_operation(
                        Operation::Tracking,
                        if failed {
                            Event::Failed
                        } else {
                            Event::Completed
                        },
                    );
                    active.set(false);
                    phase = 2;
                    // Closing Tracking does not itself authorize UART: root is pending.
                    Poll::Pending
                }
                _ => Poll::Ready(()),
            }
        });
        let mut future = pin!(metrics::observe(root, &clock, &metrics));
        let mut context = Context::from_waker(Waker::noop());
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(bytes.borrow().is_empty());
        clock
            .advance(Duration::from_micros(20))
            .expect("advance clock");
        assert!(future.as_mut().poll(&mut context).is_pending());
        assert!(bytes.borrow().is_empty());
        clock
            .advance(Duration::from_micros(30))
            .expect("advance clock");
        assert!(future.as_mut().poll(&mut context).is_ready());
        assert!(bytes.borrow().is_empty());
    }
    returned.set(true);
    block(observer.report(1));
    let sample = observer.recorder.finish(7, failed);
    block(metrics::report(1, &metrics.borrow(), &mut uart));
    assert_eq!(sample.invalid, failed);
    assert_eq!(sample.intervals.len(), 5);
    assert_eq!(aggregate.polls, 18);
    assert_eq!(aggregate.pending, 9);
    assert!(!metrics.borrow().invalid);
    let output = bytes.borrow();
    assert!(output.contains("engine family=7 polls=4 pending=2 ready=2"));
    assert!(output.contains("gain-parts family=2 lower-us=2 arithmetic-us=3 publication-us=4"));
    assert!(output.contains("gain-parts family=4 lower-us=2 arithmetic-us=3 publication-us=4"));
    assert!(output.contains("hold-poll tick=1 op=0 polls=2 pending=1 resumes=1"));
    // The tails are three kinds of note, in this order, and each prints.
    let tails: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with("OPEN_RADIO_HIL sar_tail"))
        .collect();
    assert_eq!(tails.len(), 3 + 4 + 1);
    assert!(tails[0].starts_with("OPEN_RADIO_HIL sar_tail tick=1 first=0 bins=0,0,1,0,0,0"));
    assert!(tails[2].starts_with("OPEN_RADIO_HIL sar_tail tick=1 first=12 bins="));
    assert!(tails[3].starts_with("OPEN_RADIO_HIL sar_tail tick=1 first=0 largest=3,0"));
    assert_eq!(
        tails[7],
        "OPEN_RADIO_HIL sar_tail tick=1 total=1 expired=0 max_observations=2"
    );
    assert!(output.contains("calibration-atom family=6"));
    assert!(output.contains("calibration-atom family=8"));
    assert!(output.contains("rx-gain-parts lower-us=2 hardware-us=3 commit-us=4"));
    assert!(output.contains("part=6 action=Dc.I2c first=true us=10"));
    assert!(output.contains("part=6 action=Dc.I2c first=false us=7"));
    assert!(output.contains("rx-i2c-samples-packed=4294967299"));
    let groups: Vec<u64> = output
        .lines()
        .filter_map(|line| {
            line.strip_prefix("OPEN_RADIO_HIL channel-action name=PublishTxGain packed=")
                .map(|word| word.parse().expect("packed channel action"))
        })
        .collect();
    assert_eq!(groups.len(), 2);
    assert_eq!(((groups[0] >> 40) & 255, (groups[0] >> 48) & 255), (0, 8));
    assert_eq!(((groups[1] >> 40) & 255, (groups[1] >> 48) & 255), (8, 16));
    assert!(clock.now().as_micros() > 0);
}

#[test]
fn observer_defers_all_uart_until_successful_root_return() {
    check_quiet_root(false);
}
#[test]
fn observer_defers_all_uart_until_failed_root_return() {
    check_quiet_root(true);
}

#[test]
fn channel_observation_overflow_invalidates_the_hold_without_early_uart() {
    let clock = VirtualClock::<1>::new();
    let mut aggregate = HoldPolls::default();
    let mut output = Counted(0);
    {
        let mut observer = observer::Observer::new(1, &mut aggregate, None, &clock, &mut output);
        for _ in 0..33 {
            observer.channel_action_completed(14, &EngineTiming::default(), [0, 8]);
        }
    }
    assert_eq!(output.0, 0);
    assert!(aggregate.invalid);
}

#[test]
fn root_suspension_is_attributed_only_to_intervals_active_across_pending() {
    let clock = VirtualClock::<1>::new();
    let mut m = metrics::Metrics::default();
    m.enter(clock.now().as_micros());
    m.edge(0, true);
    m.edge(2, true);
    clock
        .advance(Duration::from_micros(10))
        .expect("advance clock");
    m.exit(clock.now().as_micros(), true);
    clock
        .advance(Duration::from_micros(30))
        .expect("advance clock");
    m.enter(clock.now().as_micros());
    m.edge(2, false);
    m.edge(3, true);
    clock
        .advance(Duration::from_micros(10))
        .expect("advance clock");
    m.exit(clock.now().as_micros(), true);
    clock
        .advance(Duration::from_micros(40))
        .expect("advance clock");
    m.enter(clock.now().as_micros());
    m.edge(3, false);
    m.edge(0, false);
    clock
        .advance(Duration::from_micros(10))
        .expect("advance clock");
    m.exit(clock.now().as_micros(), false);
    assert!(!m.invalid);
    assert_eq!(
        (m.rows[0].polls, m.rows[0].pending, m.rows[0].resumes),
        (3, 2, 2)
    );
    assert_eq!(m.rows[0].suspended_us, 70);
    assert_eq!(m.rows[2].suspended_us, 30);
    assert_eq!(m.rows[3].suspended_us, 40);
    assert_eq!((m.poll_us, m.maximum_poll_us), (30, 10));
}
#[test]
fn repeated_baseband_windows_within_a_poll_count_as_one_poll() {
    let clock = VirtualClock::<1>::new();
    let mut m = metrics::Metrics::default();
    m.enter(clock.now().as_micros());
    m.edge(0, true);
    for _ in 0..2 {
        m.edge(4, true);
        m.edge(4, false);
    }
    m.edge(0, false);
    m.exit(clock.now().as_micros(), false);
    assert!(!m.invalid);
    assert_eq!((m.rows[4].polls, m.rows[4].pending), (1, 0));
    m.exit(clock.now().as_micros(), false);
    assert!(m.invalid);
}
