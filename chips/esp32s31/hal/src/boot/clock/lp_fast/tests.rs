use core::cell::{Cell, RefCell};

use oer_time::Instant;
use oer_time_virtual::VirtualClock;

use super::*;

struct Ticking<'a>(&'a VirtualClock);

impl Clock for Ticking<'_> {
    fn now(&self) -> Instant {
        self.0.advance(Duration::from_micros(1)).expect("test time")
    }
}

#[test]
fn facts_selection_replaces_inherited_codes_and_preserves_all_neighbor_fields() {
    assert_eq!(LP_FAST_SOURCE, root_clk_conf::FastClkSel::Xtal);
    // The fills exercise all four inherited selector codes and both states
    // of every neighboring bit, including opaque lanes.
    for fill in [0, u32::MAX, 0x5555_5555, 0xaaaa_aaaa] {
        let mut words = std::vec![fill; instances::lp_aon_clkrst::SIZE / 4];
        let part = LpAonRadioClock::host(&mut words).expect("host root clock");
        let before = part.read_root_clk_conf();
        let mut clock = LpFastClock(part);
        let time = VirtualClock::new();
        let ticking = Ticking(&time);
        let deadline = BootDeadline::start(&ticking, crate::boot::BOOT_BUDGET);
        assert_eq!(clock.select(&deadline), Ok(()));
        let after = clock.0.read_root_clk_conf();
        assert_eq!(after.fast_clk_sel_variant(), Some(LP_FAST_SOURCE));
        assert_eq!(before.bits_0_22_opaque(), after.bits_0_22_opaque());
        assert_eq!(before.slow_clk_sel(), after.slow_clk_sel());
        assert_eq!(before.pll80m_clk_force_on(), after.pll80m_clk_force_on());
        assert_eq!(before.xtal_clk_force_on(), after.xtal_clk_force_on());
        assert_eq!(before.fosc_clk_force_on(), after.fosc_clk_force_on());
        assert_eq!(before.ana_sel_ref_pll8m(), after.ana_sel_ref_pll8m());
        drop(clock);
        assert_eq!(
            words.iter().filter(|&&word| word != fill).count(),
            usize::from(before.fast_clk_sel().get() != LP_FAST_SOURCE.bits())
        );
    }
}

struct Model<'a> {
    time: &'a VirtualClock,
    observed: Bits<2>,
    writes: std::vec::Vec<root_clk_conf::FastClkSel>,
    written_at: Cell<Instant>,
    read_at: RefCell<std::vec::Vec<Instant>>,
}

impl<'a> Model<'a> {
    fn new(time: &'a VirtualClock, observed: Bits<2>) -> Self {
        Self {
            time,
            observed,
            writes: std::vec::Vec::new(),
            written_at: Cell::new(time.now()),
            read_at: RefCell::new(std::vec::Vec::new()),
        }
    }
}

impl Registers for Model<'_> {
    fn write_source(&mut self, source: root_clk_conf::FastClkSel) {
        self.writes.push(source);
        self.written_at.set(self.time.now());
    }

    fn source(&self) -> Bits<2> {
        self.read_at.borrow_mut().push(self.time.now());
        self.observed
    }
}

#[test]
fn a_readback_mismatch_fails_once_after_the_switch_settle_without_fallback() {
    for observed in [0, 2, 3] {
        let time = VirtualClock::new();
        let ticking = Ticking(&time);
        let deadline = BootDeadline::start(&ticking, crate::boot::BOOT_BUDGET);
        let mut registers = Model::new(&time, Bits::new(observed).expect("two-bit code"));
        assert_eq!(
            select(&mut registers, &deadline),
            Err(ClockFailure::LpFastReadback {
                expected: LP_FAST_SOURCE.bits(),
                observed,
            }
            .into())
        );
        assert_eq!(registers.writes, [LP_FAST_SOURCE]);
        let reads = registers.read_at.borrow();
        assert_eq!(reads.len(), 1);
        assert!(
            reads[0].saturating_duration_since(registers.written_at.get()) >= SWITCH_SETTLE.bound()
        );
    }
}

#[test]
fn the_switch_settle_stays_under_the_existing_boot_deadline() {
    let time = VirtualClock::new();
    let ticking = Ticking(&time);
    let deadline = BootDeadline::start(&ticking, crate::boot::BOOT_BUDGET);
    time.advance(crate::boot::BOOT_BUDGET.duration())
        .expect("exhaust budget");
    let mut registers = Model::new(&time, Bits::truncate(LP_FAST_SOURCE.bits()));
    assert_eq!(
        select(&mut registers, &deadline),
        Err(oer_espressif_boot::WaitFailure::DeadlineExceeded("lp-fast-switch").into())
    );
    assert_eq!(registers.writes, [LP_FAST_SOURCE]);
    assert!(registers.read_at.borrow().is_empty());
}
