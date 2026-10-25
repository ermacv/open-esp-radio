use std::vec;
use std::vec::Vec;

use oer_esp32s31_pac::{
    clic::{self, Clic, ClicObserver},
    instances,
    interrupt_core::{self, InterruptCore0, InterruptCore0Observer},
};
use oer_interrupt_table::{
    Binding, Class, Controller, Error, LineState, Lines, Router, Table, Trigger, line_control,
};
use oer_mmio::{Bits, Host, Index, InterruptSource};

use super::{InterruptController, InterruptView, LEVEL_BITS, LineObservation, Observer};

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

    fn controller(&mut self) -> InterruptController<Host<'_>, Host<'_>> {
        InterruptController::new(
            interrupt_core::interrupt_core0_host(&mut self.matrix)
                .expect("the memory holds the bank")
                .0,
            clic::clic_host(&mut self.clic)
                .expect("the memory holds the CLIC")
                .0,
        )
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

fn source(number: u16) -> Index<{ interrupt_core::route::COUNT }> {
    Index::new(usize::from(number)).expect("a source of the bank")
}

fn line(number: u8) -> Index<{ clic::ctrl::COUNT }> {
    Index::new(usize::from(number)).expect("a line of the CLIC")
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
    controller.route(12, 20).expect("source 12 and line 20");
    controller
        .set_line(
            20,
            LineState {
                enabled: true,
                vectored: true,
                trigger: Trigger::PositiveEdge,
                control: line_control(3, LEVEL_BITS),
            },
        )
        .expect("line 20");
    drop(controller);
    memory.clic().modify_ctrl(line(20), |word| {
        word.set_ip(true);
    });
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
        assert_eq!(
            view.line(20),
            Some(LineObservation {
                pending: true,
                state: LineState {
                    enabled: true,
                    vectored: true,
                    trigger: Trigger::PositiveEdge,
                    control: line_control(3, LEVEL_BITS),
                },
            })
        );
        assert_eq!(
            view.line(21),
            Some(LineObservation {
                pending: false,
                state: LineState::DISABLED,
            })
        );
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
    let neighbour = controller.routed(11);
    let routed_before = controller.routed(12);
    controller
        .route(12, 20)
        .expect("source 12 and line 20 are the controller's");
    assert_eq!(controller.routed(12), Some(20));
    assert_eq!(
        controller.routed(11),
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
    controller
        .route(
            12,
            routed_before.expect("the filled word routes the source"),
        )
        .expect("the filled word's line is a line of the CLIC");
    drop(controller);
    assert!(
        memory.matrix == before,
        "routing back restores every word, reserved bits included"
    );
}

#[test]
fn a_silenced_source_is_routed_nowhere_and_an_index_past_the_bank_is_refused() {
    type Ours = InterruptController<Host<'static>, Host<'static>>;
    let mut memory = Memory::filled(u32::MAX);
    let mut controller = memory.controller();
    controller.silence(5).expect("source 5 is the bank's");
    assert_eq!(controller.routed(5), None);
    assert_eq!(
        controller.route(Ours::SOURCES, 20),
        Err(Error::NoSuchSource {
            source: Ours::SOURCES
        })
    );
    assert_eq!(
        controller.silence(Ours::SOURCES),
        Err(Error::NoSuchSource {
            source: Ours::SOURCES
        })
    );
    assert_eq!(
        controller.routed(InterruptController::<Host<'_>, Host<'_>>::SOURCES),
        None
    );
    // MAP holds 63, the CLIC has 48 lines: line 48 is refused, not written.
    assert_eq!(Ours::LINES, 48);
    assert_eq!(
        controller.route(12, 48),
        Err(Error::NoSuchLine { line: 48 })
    );
    assert_eq!(
        controller.set_line(Ours::LINES, LineState::DISABLED),
        Err(Error::NoSuchLine { line: Ours::LINES })
    );
    assert_eq!(
        controller.line(InterruptController::<Host<'_>, Host<'_>>::LINES),
        LineState::DISABLED
    );
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
fn a_line_reads_back_as_written_and_keeps_its_pending_bit_and_mode() {
    let mut memory = Memory::filled(u32::MAX);
    let mut controller = memory.controller();
    for trigger in [
        Trigger::PositiveLevel,
        Trigger::PositiveEdge,
        Trigger::NegativeLevel,
        Trigger::NegativeEdge,
    ] {
        let state = LineState {
            enabled: false,
            vectored: true,
            trigger,
            control: 0x5f,
        };
        controller
            .set_line(30, state)
            .expect("line 30 is the CLIC's");
        assert_eq!(controller.line(30), state);
    }
    let bound = LineState::bound(3, LEVEL_BITS);
    controller
        .set_line(30, bound)
        .expect("line 30 is the CLIC's");
    assert_eq!(controller.line(30), bound);
    assert_eq!(bound.control, line_control(3, 3));
    drop(controller);
    let word = memory.clic().read_ctrl(line(30));
    assert!(word.ip(), "the pending bit stays");
    assert_eq!(word.attr_mode().get(), 3, "the privilege mode stays");
    let mut controller = memory.controller();
    controller
        .set_line(
            30,
            LineState {
                enabled: true,
                vectored: true,
                trigger: Trigger::NegativeEdge,
                control: 0xff,
            },
        )
        .expect("line 30 is the CLIC's");
    drop(controller);
    assert!(
        memory.clic.iter().skip(1).all(|&word| word == u32::MAX),
        "writing the line back restores its word"
    );
}

#[test]
fn a_line_another_writer_left_edge_triggered_reads_as_such() {
    let mut memory = Memory::filled(0);
    memory.clic().modify_ctrl(line(20), |word| {
        word.set_ie(true)
            .set_attr_shv(true)
            .set_attr_trig(Bits::constant::<0b01>())
            .set_ctl(line_control(3, 3));
    });
    let controller = memory.controller();
    let state = controller.line(20);
    assert_eq!(state.trigger, Trigger::PositiveEdge);
    assert_ne!(
        state,
        LineState::bound(3, LEVEL_BITS),
        "the router's check sees the line differ from a binding"
    );
}

/// The stand-in part of software interrupt line 0: the interrupt table
/// needs a part that is `'static`, which a host block is not.
#[derive(Default)]
pub struct WakeIrq;

impl InterruptSource for WakeIrq {
    const SOURCE: u16 = <oer_esp32s31_pac::hp_system::FromCpu0 as InterruptSource>::SOURCE;
}

pub fn on_wake(_: &mut WakeIrq) {}

const LINES: Lines = Lines {
    first: 16,
    count: 48,
    levels: 8,
    harts: 2,
};

oer_interrupt_table::interrupt_table! {
    lines: LINES,
    handler_attributes: [];
    wake: WakeToken = WakeIrq => on_wake, line 20, level 3, hart 0, Hardware;
}

#[test]
fn the_router_silences_boot_leftovers_then_binds_a_line_on_its_level() {
    // Every source routed to line 20 and every line enabled, as a boot
    // before the table might leave them.
    let mut memory = Memory::filled(0);
    {
        let mut controller = memory.controller();
        for number in 0..InterruptController::<Host<'_>, Host<'_>>::SOURCES {
            controller
                .route(number, 20)
                .expect("every source of the bank routes to line 20");
        }
        for number in 0..InterruptController::<Host<'_>, Host<'_>>::LINES {
            controller
                .set_line(number, LineState::bound(1, LEVEL_BITS))
                .expect("every line of the CLIC takes a binding");
        }
    }
    {
        let mut router = Router::install(memory.controller(), INTERRUPT_TABLE)
            .expect("a host controller installs");
        let tokens = Interrupts::take().expect("the test takes the tokens once");
        let _bound = router
            .bind(tokens.wake, WakeIrq)
            .expect("the wake entry is the table's");
        assert_eq!(router.verify(), Ok(()));
    }

    let controller = memory.controller();
    for number in 0..InterruptController::<Host<'_>, Host<'_>>::SOURCES {
        let expected = (number == WakeIrq::SOURCE).then_some(20);
        assert_eq!(controller.routed(number), expected, "source {number}");
    }
    for number in 0..InterruptController::<Host<'_>, Host<'_>>::LINES {
        let expected = if number == 20 {
            LineState::bound(3, LEVEL_BITS)
        } else {
            LineState::DISABLED
        };
        assert_eq!(
            controller.line(number).enabled,
            expected.enabled,
            "line {number}"
        );
    }
    let state = controller.line(20);
    assert!(state.vectored && state.trigger == Trigger::PositiveLevel);
    assert_eq!(state.control, line_control(3, 3));
}

extern "C" fn no_handler() {}

/// A table of one entry of hart 0, as a table of wider `Lines` than the
/// chip's would hold it.
fn table_of(source: u16, line: u8) -> &'static Table {
    std::boxed::Box::leak(std::boxed::Box::new([Binding {
        source,
        line,
        level: 3,
        class: Class::Hardware,
        hart: 0,
        handler: Some(no_handler),
    }]))
}

#[test]
fn the_router_refuses_a_table_outside_the_matrix_or_the_clic_before_any_write() {
    // A table of `Lines { count: 64, .. }`: line 60 lies outside the CLIC.
    let sources = InterruptController::<Host<'_>, Host<'_>>::SOURCES;
    for (table, error) in [
        (
            table_of(sources, 60),
            Error::NoSuchSource { source: sources },
        ),
        (
            table_of(sources, 20),
            Error::NoSuchSource { source: sources },
        ),
        (table_of(12, 60), Error::NoSuchLine { line: 60 }),
        (table_of(12, 48), Error::NoSuchLine { line: 48 }),
    ] {
        let mut memory = Memory::filled(u32::MAX);
        assert_eq!(
            Router::install(memory.controller(), table).err(),
            Some(error)
        );
        assert!(
            memory.matrix.iter().all(|&word| word == u32::MAX),
            "no route changed"
        );
        assert!(
            memory.clic.iter().skip(1).all(|&word| word == u32::MAX),
            "no line changed"
        );
    }
}

#[test]
fn a_source_is_pending_at_its_bit_of_its_status_word() {
    // SOURCE: ESP-IDF 4d59230d `interrupt_core0_reg.h`: the status words
    // at 0x2a8 + 4n hold sources 32n to 32n + 31.
    let mut words = vec![0; instances::interrupt_core0::SIZE / 4];
    words[0x2a8 / 4 + 2] = 1 << 4; // source 68, FROM_CPU_INTR3
    words[0x2a8 / 4 + 5] = 1 << 8; // source 168, the last
    let status = oer_esp32s31_pac::interrupt_core::InterruptCore0Status::host(&mut words)
        .expect("the memory holds the bank");
    assert!(super::pending_in(&status, 68));
    assert!(super::pending_in(&status, 168));
    assert!(!super::pending_in(&status, 67));
    assert!(!super::pending_in(&status, 169), "no such source");
    assert!(!super::pending_in(&status, u16::MAX));
}
