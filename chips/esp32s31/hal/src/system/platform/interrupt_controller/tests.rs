use oer_chips_family_platform_interrupt_table::{LineExpectation, LineObservation};
use oer_portable_riscv::clic::Control;
use std::vec;
use std::vec::Vec;

use oer_chips_esp32s31_pac::{
    clic::{self, Clic, ClicObserver},
    instances,
    interrupt_core::{self, InterruptCore0, InterruptCore0Observer},
};
use oer_chips_family_platform_interrupt_table::{Controller, Error, NoBindings, Router};
use oer_portable_mmio::{Bits, Host, Index, InterruptSource};

use super::{
    ForeignHart, InterruptController, InterruptGrant, InterruptView, Observer,
    facts::{self, LEVEL_BITS, LINES},
};

/// Hart 0's controller over host memory.
type Ours<'a> = InterruptController<InterruptCore0<Host<'a>>, Host<'a>>;

/// Host memory of hart 0's bank and of the CLIC.
struct Memory {
    matrix: Vec<u32>,
    clic: Vec<u32>,
}

impl Memory {
    /// Every word holding `word`.
    fn filled(word: u32) -> Self {
        Self {
            matrix: vec![word; instances::interrupt_core0::SIZE / 4],
            clic: vec![word; instances::clic::SIZE / 4],
        }
    }

    fn controller(&mut self) -> Ours<'_> {
        InterruptGrant::new(
            interrupt_core::interrupt_core0_host(&mut self.matrix)
                .expect("the memory holds the bank")
                .0,
            clic::clic_host(&mut self.clic)
                .expect("the memory holds the CLIC")
                .0,
            None,
        )
        .into_controller_on(0)
        .expect("hart 0 builds its own controller")
    }

    fn matrix(&mut self) -> InterruptCore0<Host<'_>> {
        interrupt_core::interrupt_core0_host(&mut self.matrix)
            .expect("the memory holds the bank")
            .0
    }

    fn clic(&mut self) -> Clic<Host<'_>> {
        clic::clic_host(&mut self.clic)
            .expect("the memory holds the CLIC")
            .0
    }

    fn clic_observer(&mut self) -> ClicObserver<Host<'_>> {
        clic::clic_host(&mut self.clic)
            .expect("the memory holds the CLIC")
            .1
    }

    fn observer(&mut self) -> Observer<Host<'_>, Host<'_>> {
        let routes: InterruptCore0Observer<Host<'_>> =
            interrupt_core::interrupt_core0_host(&mut self.matrix)
                .expect("the memory holds the bank")
                .1;
        let lines = clic::clic_host(&mut self.clic)
            .expect("the memory holds the CLIC")
            .1;
        Observer { routes, lines }
    }
}

/// Inject hardware-owned INT_INFO into private host memory. The field
/// geometry comes from the model; VERSION and unassigned bits are deliberately
/// nonzero, since neither is part of the boot geometry contract.
fn configured_memory(control_bits: u8, lines: u16, level_bits: u8) -> Memory {
    let mut memory = Memory::filled(0);
    memory.clic[clic::int_info::OFFSET / 4] = (u32::from(control_bits)
        << clic::int_info::CTLBITS_MASK.trailing_zeros())
        | (u32::from(lines) << clic::int_info::NUM_INT_MASK.trailing_zeros())
        | clic::int_info::VERSION_MASK
        | !(clic::int_info::CTLBITS_MASK | clic::int_info::NUM_INT_MASK);
    memory.clic_observer().modify_int_config(|config| {
        config.set_mnlbits(Bits::new(u32::from(level_bits)).expect("four-bit fixture"));
    });
    memory
}

#[test]
fn controller_geometry_mismatches_refuse_installation_before_route_or_line_writes() {
    let expected_count = u16::from(LINES.count);
    for (control_bits, lines, level_bits, error) in [
        (
            facts::CONTROL_BITS - 1,
            expected_count,
            LEVEL_BITS,
            Error::WrongControlBits {
                actual: facts::CONTROL_BITS - 1,
                expected: facts::CONTROL_BITS,
            },
        ),
        (
            facts::CONTROL_BITS + 1,
            expected_count,
            LEVEL_BITS,
            Error::WrongControlBits {
                actual: facts::CONTROL_BITS + 1,
                expected: facts::CONTROL_BITS,
            },
        ),
        (
            facts::CONTROL_BITS,
            expected_count - 1,
            LEVEL_BITS,
            Error::WrongInterruptCount {
                actual: expected_count - 1,
                expected: expected_count,
            },
        ),
        (
            facts::CONTROL_BITS,
            expected_count + 1,
            LEVEL_BITS,
            Error::WrongInterruptCount {
                actual: expected_count + 1,
                expected: expected_count,
            },
        ),
        (
            facts::CONTROL_BITS,
            expected_count,
            0,
            Error::WrongLevelBits {
                actual: 0,
                expected: LEVEL_BITS,
            },
        ),
        (
            facts::CONTROL_BITS,
            expected_count,
            LEVEL_BITS + 1,
            Error::WrongLevelBits {
                actual: LEVEL_BITS + 1,
                expected: LEVEL_BITS,
            },
        ),
    ] {
        let mut memory = configured_memory(control_bits, lines, level_bits);
        let matrix_before = memory.matrix.clone();
        let clic_before = memory.clic.clone();
        assert_eq!(
            Router::<_, 0, _>::install(memory.controller(), NoBindings).err(),
            Some(error)
        );
        assert_eq!(memory.matrix, matrix_before, "no route writes on mismatch");
        assert_eq!(memory.clic, clic_before, "no line writes on mismatch");
    }
}

#[test]
fn verification_rechecks_mnlbits_after_installation() {
    let mut memory = configured_memory(facts::CONTROL_BITS, u16::from(LINES.count), LEVEL_BITS);
    let matrix = interrupt_core::interrupt_core0_host(&mut memory.matrix)
        .expect("matrix")
        .0;
    let (clic, mut config) = clic::clic_host(&mut memory.clic).expect("CLIC");
    let controller = InterruptGrant::new(matrix, clic, None)
        .into_controller_on(0)
        .expect("hart 0 builds its own controller");
    let router =
        Router::<_, 0, _>::install(controller, NoBindings).expect("matching geometry installs");
    assert_eq!(router.verify(), Ok(()));
    config.modify_int_config(|config| {
        config.set_mnlbits(Bits::constant::<0>());
    });
    assert_eq!(
        router.verify(),
        Err(Error::WrongLevelBits {
            actual: 0,
            expected: LEVEL_BITS
        })
    );
}

fn source(number: u16) -> Index<{ interrupt_core::route::COUNT }> {
    Index::new(usize::from(number)).expect("a source of the bank")
}

fn line(number: u8) -> Index<{ clic::ip::COUNT }> {
    Index::new(usize::from(number)).expect("a line of the CLIC")
}

/// Write each of a line's four byte registers through the PAC's own
/// accessors, as another writer would leave them.
fn set_line(
    clic: &mut clic::Clic<oer_portable_mmio::Host<'_>>,
    number: u8,
    (pending, enabled, vectored, trigger, control): (bool, bool, bool, clic::attr::Trig, u8),
) {
    clic.modify_ip(line(number), |byte| {
        byte.set_ip(pending);
    });
    clic.modify_attr(line(number), |byte| {
        byte.set_shv(vectored).set_trig(trigger);
    });
    clic.modify_ctl(line(number), |byte| {
        byte.set_ctl(control);
    });
    clic.modify_ie(line(number), |byte| {
        byte.set_ie(enabled);
    });
}

/// The controller's own source `number`.
fn bank_source(number: u16) -> super::Source {
    Ours::SOURCES[usize::from(number)]
}

/// The controller's own line `number`.
fn clic_line(number: u8) -> super::Line {
    Ours::LINES[usize::from(number)]
}

#[test]
fn splitting_the_partition_sets_three_level_bits_and_keeps_the_other_configuration() {
    let mut memory = Memory::filled(u32::MAX);
    super::set_level_bits(&mut memory.clic_observer());
    assert_eq!(super::level_bits(&memory.clic_observer()), LEVEL_BITS);
    let config = memory.clic_observer().read_int_config();
    assert_eq!(config.mnlbits().get(), 3);
    assert!(config.nvbits(), "the vectoring flag stays");
    assert_eq!(config.nmbits().get(), 3, "the mode bits stay");
}

#[test]
fn the_fatal_view_reads_exactly_the_routes_and_lines_the_words_hold() {
    let mut memory = Memory::filled(0);
    let mut controller = memory.controller();
    controller.route(bank_source(12), clic_line(20));
    drop(controller);
    // An edge-triggered line no binding writes, pending.
    set_line(
        &mut memory.clic(),
        20,
        (
            true,
            true,
            true,
            clic::attr::Trig::PositiveEdge,
            Control::level::<3, LEVEL_BITS>().bits(),
        ),
    );
    let matrix_before = memory.matrix.clone();
    let clic_before = memory.clic.clone();
    {
        let observer = memory.observer();
        let view = InterruptView {
            observer: &observer,
        };
        assert_eq!(view.routed(12), Some(20));
        assert_eq!(view.routed(11), None, "a silent source");
        assert_eq!(view.routed(u16::MAX), None, "no source of the bank");
        // The line's four bytes in their order: pending 1, enabled 1,
        // attributes 0x03 (vectored, positive edge; the mode as the zeroed
        // memory holds it), level byte 0x5f.
        assert_eq!(view.line(20), Some(0x5f03_0101));
        assert_eq!(view.line(21), Some(0));
        assert_eq!(view.line(u8::MAX), None, "no line of the CLIC");
    }
    assert!(
        memory.matrix == matrix_before && memory.clic == clic_before,
        "the view writes nothing"
    );
}

#[test]
fn a_route_writes_its_source_s_line_alone_and_keeps_the_rest_of_its_word() {
    let mut memory = Memory::filled(u32::MAX);
    // Every other bit set, source 12 on the CLIC's last line.
    memory.matrix().modify_route(source(12), |route| {
        route.set_map(Bits::constant::<47>());
    });
    let before = memory.matrix.clone();
    let mut controller = memory.controller();
    let neighbour = controller.routed(bank_source(11));
    let routed_before = controller.routed(bank_source(12));
    controller.route(bank_source(12), clic_line(20));
    assert_eq!(controller.routed(bank_source(12)), Some(20));
    assert_eq!(
        controller.routed(bank_source(11)),
        neighbour,
        "the next source is untouched"
    );
    drop(controller);
    assert_eq!(
        memory.matrix().read_route(source(12)).pass_level().get(),
        3,
        "the route keeps its source's PASS_LEVEL"
    );
    let mut controller = memory.controller();
    controller.route(
        bank_source(12),
        clic_line(routed_before.expect("the filled word routes the source")),
    );
    drop(controller);
    assert!(
        memory.matrix == before,
        "routing back restores every word, reserved bits included"
    );
}

#[test]
fn a_silenced_source_is_routed_nowhere_and_writes_its_word_alone() {
    let mut memory = Memory::filled(u32::MAX);
    let mut controller = memory.controller();
    controller.silence(bank_source(5));
    assert_eq!(controller.routed(bank_source(5)), None);
    drop(controller);
    let changed = memory
        .matrix
        .iter()
        .filter(|&&word| word != u32::MAX)
        .count();
    assert_eq!(changed, 1, "only source 5's word changed");
    assert!(
        memory.clic.iter().skip(1).all(|&word| word == u32::MAX),
        "no line word changed"
    );
}

#[test]
fn a_bound_and_a_disabled_line_check_as_written_clear_their_pending_byte_and_keep_their_mode() {
    let mut memory = Memory::filled(u32::MAX);
    let mut controller = memory.controller();
    controller.bind(clic_line(30), Control::level::<3, LEVEL_BITS>());
    let level = Control::level::<3, LEVEL_BITS>();
    assert_eq!(
        controller.check(clic_line(30), LineExpectation::Bound(level)),
        Ok(())
    );
    assert!(
        controller
            .check(clic_line(30), LineExpectation::Disabled)
            .is_err()
    );
    drop(controller);
    let clic = memory.clic();
    let attributes = clic.read_attr(line(30));
    assert!(clic.read_ie(line(30)).ie() && attributes.shv());
    assert_eq!(attributes.trig_variant(), clic::attr::Trig::PositiveLevel);
    assert_eq!(clic.read_ctl(line(30)).ctl(), level.bits());
    assert!(
        !clic.read_ip(line(30)).ip(),
        "the bind cleared the pending byte"
    );
    assert_eq!(attributes.mode().get(), 3, "the privilege mode stays");
    drop(clic);
    let mut controller = memory.controller();
    controller.disable(clic_line(30));
    assert_eq!(
        controller.check(clic_line(30), LineExpectation::Disabled),
        Ok(())
    );
    drop(controller);
    let clic = memory.clic();
    assert!(
        !clic.read_ie(line(30)).ie()
            && !clic.read_attr(line(30)).shv()
            && clic.read_ctl(line(30)).ctl() == 0
    );
    assert!(
        !clic.read_ip(line(30)).ip(),
        "the take-down cleared the pending byte"
    );
    assert_eq!(
        clic.read_attr(line(30)).mode().get(),
        3,
        "the privilege mode stays"
    );
    drop(clic);
    assert!(
        memory
            .clic
            .iter()
            .enumerate()
            .skip(1)
            .all(|(index, &word)| index == clic::ip::OFFSET / 4 + 30 || word == u32::MAX),
        "no other line's word changed"
    );
}

#[test]
fn a_line_another_writer_left_edge_triggered_is_no_binding() {
    let mut memory = Memory::filled(0);
    set_line(
        &mut memory.clic(),
        20,
        (
            false,
            true,
            true,
            clic::attr::Trig::PositiveEdge,
            Control::level::<3, 3>().bits(),
        ),
    );
    let controller = memory.controller();
    let expected = LineExpectation::Bound(Control::level::<3, LEVEL_BITS>());
    assert_eq!(
        controller.check(clic_line(20), expected),
        Err(LineObservation::of_word(0x5f03_0100)),
        "the router's check sees the line differ from a binding"
    );
}

/// The stand-in part of software interrupt line 0: the interrupt table
/// needs a part that is `'static`, which a host block is not.
#[derive(Default)]
pub struct WakeIrq;

impl InterruptSource for WakeIrq {
    const SOURCE: u16 = <oer_chips_esp32s31_pac::hp_system::FromCpu0 as InterruptSource>::SOURCE;
}

pub fn on_wake(_: &mut WakeIrq) {}

oer_chips_family_platform_interrupt_table::interrupt_table! {
    lines: LINES,
    handler_attributes: [];
    contexts { radio: 3; }
    wake: WakeToken = WakeIrq => on_wake, line 20, context radio, hart 0, Hardware;
}

#[test]
fn the_router_silences_boot_leftovers_then_binds_a_line_on_its_level() {
    // Every source routed to line 20 and every line enabled, as a boot
    // before the table might leave them.
    let mut memory = configured_memory(facts::CONTROL_BITS, u16::from(LINES.count), LEVEL_BITS);
    {
        let mut controller = memory.controller();
        for &source in Ours::SOURCES {
            controller.route(source, clic_line(20));
        }
        for &line in Ours::LINES {
            controller.bind(line, Control::level::<1, LEVEL_BITS>());
        }
    }
    {
        let mut router = Router::<_, 0, _>::install(memory.controller(), InterruptTable)
            .expect("a host controller installs");
        let tokens = Interrupts::take().expect("the test takes the tokens once");
        let _bound = router
            .bind(tokens.wake, WakeIrq)
            .expect("the wake entry is the table's");
        assert_eq!(router.verify(), Ok(()));
    }

    let controller = memory.controller();
    for (&source, number) in Ours::SOURCES.iter().zip(0u16..) {
        let expected = (number == WakeIrq::SOURCE).then_some(20);
        assert_eq!(controller.routed(source), expected, "source {number}");
    }
    for (&line, number) in Ours::LINES.iter().zip(0u8..) {
        let expected = if number == 20 {
            LineExpectation::Bound(Control::level::<3, LEVEL_BITS>())
        } else {
            LineExpectation::Disabled
        };
        assert_eq!(controller.check(line, expected), Ok(()), "line {number}");
    }
    drop(controller);
    let clic = memory.clic();
    let attributes = clic.read_attr(line(20));
    assert!(attributes.shv());
    assert_eq!(attributes.trig_variant(), clic::attr::Trig::PositiveLevel);
    assert_eq!(
        clic.read_ctl(line(20)).ctl(),
        Control::level::<3, 3>().bits()
    );
}

#[test]
fn a_source_is_pending_at_its_bit_of_its_status_word() {
    // SOURCE: ESP-IDF 4d80c1b2 `interrupt_core0_reg.h`: the status words
    // at 0x2a8 + 4n hold sources 32n to 32n + 31.
    let mut words = vec![0; instances::interrupt_core0::SIZE / 4];
    words[0x2a8 / 4 + 2] = 1 << 4; // source 68, FROM_CPU_INTR3
    words[0x2a8 / 4 + 5] = 1 << 8; // source 168, the last
    let status = oer_chips_esp32s31_pac::interrupt_core::InterruptCore0Status::host(&mut words)
        .expect("the memory holds the bank");
    assert!(super::pending_in(&status, 68));
    assert!(super::pending_in(&status, 168));
    assert!(!super::pending_in(&status, 67));
    assert!(!super::pending_in(&status, 169), "no such source");
    assert!(!super::pending_in(&status, u16::MAX));
}

#[test]
fn pending_reads_select_only_the_named_harts_bank() {
    use crate::system::platform::{HARTS, Hart};
    for active in Hart::ALL {
        let mut words: [Vec<u32>; HARTS] =
            core::array::from_fn(|_| vec![0; instances::interrupt_core0::SIZE / 4]);
        active.at_mut(&mut words)[interrupt_core::int_status_reg_2::OFFSET / 4] = 1 << 4;
        let banks = host_pending!(&mut words);
        for observed in Hart::ALL {
            assert_eq!(banks.is_pending(observed, 68), observed == active);
            assert!(!banks.is_pending(observed, 67));
            assert!(!banks.is_pending(observed, u16::MAX));
        }
    }
}

#[test]
fn a_grant_builds_its_controller_only_on_its_own_hart() {
    let mut bank = vec![0; instances::interrupt_core0::SIZE / 4];
    let mut clic_words = vec![0; instances::clic::SIZE / 4];
    let grant = InterruptGrant::new(
        interrupt_core::interrupt_core0_host(&mut bank)
            .expect("hart 0's bank")
            .0,
        clic::clic_host(&mut clic_words).expect("CLIC").0,
        None,
    );
    assert_eq!(grant.hart(), 0);
    assert_eq!(
        grant.into_controller_on(1).err(),
        Some(ForeignHart {
            actual: 1,
            expected: 0
        })
    );
}

#[test]
fn each_banks_controller_names_its_own_hart() {
    let mut memory = Memory::filled(0);
    assert_eq!(memory.controller().hart(), 0);
    let mut bank = vec![0; instances::interrupt_core1::SIZE / 4];
    let mut clic_words = vec![0; instances::clic::SIZE / 4];
    let (lines, config) = clic::clic_host(&mut clic_words).expect("CLIC");
    let controller = InterruptGrant::new(
        interrupt_core::InterruptCore1::host(&mut bank).expect("hart 1's bank"),
        lines,
        Some(config),
    )
    .into_controller_on(1)
    .expect("hart 1 builds its own controller");
    assert_eq!(controller.hart(), 1);
}

#[test]
fn a_bound_line_with_any_one_of_its_four_facts_wrong_is_no_binding() {
    let level = Control::level::<3, LEVEL_BITS>();
    let expected = LineExpectation::Bound(level);
    // Each fact of the binding undone alone, with the bytes it leaves in
    // their order: level byte 0x5f, attributes 0x01 (vectored), enable 1.
    type Undo = fn(&mut clic::Clic<oer_portable_mmio::Host<'_>>);
    let cases: [(&str, Undo, u32); 4] = [
        (
            "enable",
            |clic| {
                clic.modify_ie(line(20), |byte| {
                    byte.set_ie(false);
                });
            },
            0x5f01_0000,
        ),
        (
            "vectoring",
            |clic| {
                clic.modify_attr(line(20), |byte| {
                    byte.set_shv(false);
                });
            },
            0x5f00_0100,
        ),
        (
            "trigger",
            |clic| {
                clic.modify_attr(line(20), |byte| {
                    byte.set_trig(clic::attr::Trig::PositiveEdge);
                });
            },
            0x5f03_0100,
        ),
        (
            "level",
            |clic| {
                clic.modify_ctl(line(20), |byte| {
                    byte.set_ctl(Control::level::<1, LEVEL_BITS>().bits());
                });
            },
            0x1f01_0100,
        ),
    ];
    for (fact, undo, word) in cases {
        let mut memory = Memory::filled(0);
        let mut controller = memory.controller();
        controller.bind(clic_line(20), level);
        assert_eq!(controller.check(clic_line(20), expected), Ok(()), "{fact}");
        drop(controller);
        undo(&mut memory.clic());
        let controller = memory.controller();
        assert_eq!(
            controller.check(clic_line(20), expected),
            Err(LineObservation::of_word(word)),
            "{fact}"
        );
    }
}
