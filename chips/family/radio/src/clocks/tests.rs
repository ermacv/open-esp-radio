//! Recorded gates and virtual time verify exact edge and refusal behavior.
use super::*;
use std::vec::Vec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PlatformClock {
    Pll160m,
    AnalogI2cMaster,
    Mpll,
    ModemCoexistence,
    ModemLowPowerTimer,
}
impl PlatformClock {
    const ALL: [Self; 5] = [
        Self::Pll160m,
        Self::AnalogI2cMaster,
        Self::Mpll,
        Self::ModemCoexistence,
        Self::ModemLowPowerTimer,
    ];
    fn index(self) -> usize {
        match self {
            Self::Pll160m => 0,
            Self::AnalogI2cMaster => 1,
            Self::Mpll => 2,
            Self::ModemCoexistence => 3,
            Self::ModemLowPowerTimer => 4,
        }
    }
}
#[derive(Default)]
struct References([u16; 5]);
impl ClockReferences for References {
    type Clock = PlatformClock;
    fn get(&self, clock: PlatformClock) -> u16 {
        self.0[clock.index()]
    }
    fn set(&mut self, clock: PlatformClock, count: u16) {
        self.0[clock.index()] = count;
    }
}
struct PermanentMpll;

/// One gate write.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Write {
    Pll160m(bool),
    AnalogI2cMaster(bool),
    Coexistence(bool),
    LowPowerTimer(bool),
    WifiPowerOpen,
}

#[derive(Default)]
struct Recorded(Vec<Write>, oer_time_virtual::VirtualClock);

impl ClockGates for Recorded {
    type References = References;
    fn set_clock(&mut self, clock: PlatformClock, on: bool) {
        match clock {
            PlatformClock::Pll160m => self.set_pll_f160m(on),
            PlatformClock::AnalogI2cMaster => self.set_analog_i2c_master(on),
            PlatformClock::ModemCoexistence => self.set_coexistence(on),
            PlatformClock::ModemLowPowerTimer => self.set_low_power_timer(on),
            PlatformClock::Mpll => {}
        }
    }
    fn open_wifi_power(&mut self) {
        self.record(Write::WifiPowerOpen);
    }
}
impl Recorded {
    fn record(&mut self, write: Write) {
        self.0.push(write);
        self.1
            .advance(oer_time::Duration::from_micros(1))
            .expect("finite test time");
    }
    fn set_pll_f160m(&mut self, on: bool) {
        self.record(Write::Pll160m(on));
    }
    fn set_analog_i2c_master(&mut self, on: bool) {
        self.record(Write::AnalogI2cMaster(on));
    }
    fn set_coexistence(&mut self, on: bool) {
        self.record(Write::Coexistence(on));
    }
    fn set_low_power_timer(&mut self, on: bool) {
        self.record(Write::LowPowerTimer(on));
    }
}

fn adopted() -> ClockCounts<Recorded, PermanentMpll> {
    ClockCounts::adopt(Recorded::default(), PermanentMpll)
}

#[test]
fn adoption_opens_only_the_wifi_power_gate() {
    let counts = adopted();
    assert_eq!(counts.gates.0, [Write::WifiPowerOpen]);
    for clock in PlatformClock::ALL {
        assert_eq!(
            counts.counts.get(clock),
            0,
            "{clock:?} has no radio reference"
        );
    }
}

#[test]
fn only_the_first_and_last_reference_write_a_gate() {
    let mut counts = adopted();
    for clock in [
        PlatformClock::Pll160m,
        PlatformClock::AnalogI2cMaster,
        PlatformClock::Pll160m,
        PlatformClock::ModemCoexistence,
        PlatformClock::ModemLowPowerTimer,
    ] {
        counts.acquire(clock).expect("granted");
    }
    for clock in [
        PlatformClock::Pll160m,
        PlatformClock::ModemLowPowerTimer,
        PlatformClock::AnalogI2cMaster,
        PlatformClock::ModemCoexistence,
        PlatformClock::Pll160m,
    ] {
        counts.release(clock).expect("held");
    }
    assert_eq!(
        counts.gates.0,
        [
            Write::WifiPowerOpen,
            Write::Pll160m(true),
            Write::AnalogI2cMaster(true),
            Write::Coexistence(true),
            Write::LowPowerTimer(true),
            Write::LowPowerTimer(false),
            Write::AnalogI2cMaster(false),
            Write::Coexistence(false),
            Write::Pll160m(false),
        ]
    );
}

#[test]
fn mpll_references_never_write_and_never_cross_the_psram_reference() {
    let mut counts = adopted();
    for _ in 0..3 {
        counts.acquire(PlatformClock::Mpll).expect("granted");
    }
    assert_eq!(counts.counts.get(PlatformClock::Mpll), 3);
    for _ in 0..3 {
        counts.release(PlatformClock::Mpll).expect("held");
    }
    assert_eq!(counts.counts.get(PlatformClock::Mpll), 0);
    assert_eq!(
        counts.release(PlatformClock::Mpll),
        Err(ClockError::NotHeld(PlatformClock::Mpll)),
        "the PSRAM mapping's reference is not the radio's to give back"
    );
    assert_eq!(
        counts.gates.0,
        [Write::WifiPowerOpen],
        "MPLL powers nothing"
    );
}

#[test]
fn a_release_before_its_acquire_is_refused_and_writes_nothing() {
    let mut counts = adopted();
    for clock in PlatformClock::ALL {
        assert_eq!(counts.release(clock), Err(ClockError::NotHeld(clock)));
    }
    counts
        .acquire(PlatformClock::ModemCoexistence)
        .expect("granted");
    counts
        .release(PlatformClock::ModemCoexistence)
        .expect("held");
    assert_eq!(
        counts.release(PlatformClock::ModemCoexistence),
        Err(ClockError::NotHeld(PlatformClock::ModemCoexistence)),
        "a second release has nothing to give back"
    );
    assert_eq!(
        counts.gates.0,
        [
            Write::WifiPowerOpen,
            Write::Coexistence(true),
            Write::Coexistence(false),
        ]
    );
}

#[test]
fn an_exhausted_count_refuses_without_a_write() {
    let mut counts = adopted();
    counts.counts.set(PlatformClock::AnalogI2cMaster, u16::MAX);
    assert_eq!(
        counts.acquire(PlatformClock::AnalogI2cMaster),
        Err(ClockError::CountExhausted(PlatformClock::AnalogI2cMaster))
    );
    assert_eq!(counts.counts.get(PlatformClock::AnalogI2cMaster), u16::MAX);
    assert_eq!(counts.gates.0, [Write::WifiPowerOpen]);
}

#[test]
fn refused_edges_do_not_advance_the_port_clock() {
    use oer_time::Clock;
    let mut counts = adopted();
    let at = counts.gates.1.now();
    assert_eq!(
        counts.release(PlatformClock::Pll160m),
        Err(ClockError::NotHeld(PlatformClock::Pll160m))
    );
    assert_eq!(counts.gates.1.now(), at);
    counts.acquire(PlatformClock::Pll160m).unwrap();
    let at = counts.gates.1.now();
    counts.acquire(PlatformClock::Pll160m).unwrap();
    counts.release(PlatformClock::Pll160m).unwrap();
    assert_eq!(
        counts.gates.1.now(),
        at,
        "no gate atom on interior count transitions"
    );
}

#[test]
fn permanent_owner_stays_with_the_counter() {
    use std::{cell::Cell, rc::Rc};
    struct Permanent(Rc<Cell<bool>>);
    impl Drop for Permanent {
        fn drop(&mut self) {
            self.0.set(true);
        }
    }
    let dropped = Rc::new(Cell::new(false));
    let counts = ClockCounts::adopt(Recorded::default(), Permanent(dropped.clone()));
    assert!(!dropped.get());
    drop(counts);
    assert!(dropped.get());
}

#[test]
fn only_the_rtc_crystal_selector_chooses_wifi_xtal32k() {
    use std::cell::Cell;
    struct Port {
        rtc: RtcSlowClockSource,
        selected: Option<WifiLowPowerClockSource>,
        reads: Cell<u8>,
    }
    impl LowPowerClockPort for Port {
        fn rtc_slow_clock_source(&self) -> RtcSlowClockSource {
            self.reads.set(self.reads.get() + 1);
            self.rtc
        }
        fn select_wifi_low_power_clock(&mut self, source: WifiLowPowerClockSource) {
            self.selected = Some(source);
        }
    }
    for (rtc, expected) in [
        (
            RtcSlowClockSource::Crystal32Khz,
            WifiLowPowerClockSource::Crystal32Khz,
        ),
        (
            RtcSlowClockSource::SlowOscillator,
            WifiLowPowerClockSource::SlowOscillator,
        ),
        (
            RtcSlowClockSource::Invalid(2),
            WifiLowPowerClockSource::SlowOscillator,
        ),
        (
            RtcSlowClockSource::Invalid(u8::MAX),
            WifiLowPowerClockSource::SlowOscillator,
        ),
    ] {
        let mut port = Port {
            rtc,
            selected: None,
            reads: Cell::new(0),
        };
        select_wifi_low_power_clock(&mut port);
        assert_eq!(port.selected, Some(expected));
        assert_eq!(port.reads.get(), 1);
    }
}
