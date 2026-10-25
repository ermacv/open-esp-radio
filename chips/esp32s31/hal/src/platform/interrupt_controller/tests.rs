use std::vec;
use std::vec::Vec;

use oer_esp32s31_pac::platform::{Clic, InterruptCore0, clic, interrupt_core0};
use oer_interrupt_table::{Controller, LineState, Lines, Router, Trigger, line_control};
use oer_mmio::{Bits, Host, Index, InterruptSource};

use super::{InterruptController, LEVEL_BITS};

/// Host memory of hart 0's bank and of the CLIC.
struct Memory {
    matrix: Vec<u32>,
    clic: Vec<u32>,
}

impl Memory {
    /// Every word holding `word`.
    fn filled(word: u32) -> Self {
        Self {
            matrix: vec![word; interrupt_core0::SIZE / 4],
            clic: vec![word; clic::SIZE / 4],
        }
    }

    fn controller(&mut self) -> InterruptController<Host<'_>, Host<'_>> {
        InterruptController::new(
            InterruptCore0::host(&mut self.matrix).expect("the memory holds the bank"),
            Clic::host(&mut self.clic).expect("the memory holds the CLIC"),
        )
    }

    fn matrix(&mut self) -> InterruptCore0<Host<'_>> {
        InterruptCore0::host(&mut self.matrix).expect("the memory holds the bank")
    }

    fn clic(&mut self) -> Clic<Host<'_>> {
        Clic::host(&mut self.clic).expect("the memory holds the CLIC")
    }
}

fn source(number: u16) -> Index<{ interrupt_core0::route::COUNT }> {
    Index::new(usize::from(number)).expect("a source of the bank")
}

fn line(number: u8) -> Index<{ clic::ctrl::COUNT }> {
    Index::new(usize::from(number)).expect("a line of the CLIC")
}

#[test]
fn taking_the_controller_sets_three_level_bits_and_keeps_the_other_configuration() {
    let mut memory = Memory::filled(u32::MAX);
    let controller = memory.controller();
    assert_eq!(controller.level_bits(), LEVEL_BITS);
    drop(controller);
    let config = memory.clic().read_int_config();
    assert_eq!(config.mnlbits().get(), 3);
    assert!(config.nvbits(), "the vectoring flag stays");
    assert_eq!(config.nmbits().get(), 3, "the mode bits stay");
}

#[test]
fn a_route_writes_its_source_s_line_alone_and_keeps_the_rest_of_its_word() {
    let mut memory = Memory::filled(u32::MAX);
    let mut controller = memory.controller();
    let neighbour = controller.routed(11);
    let routed_before = controller.routed(12);
    controller.route(12, 20);
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
    controller.route(
        12,
        routed_before.expect("the filled word routes the source"),
    );
    drop(controller);
    assert!(
        memory.matrix.iter().all(|&word| word == u32::MAX),
        "routing back restores every word, reserved bits included"
    );
}

#[test]
fn a_silenced_source_is_routed_nowhere_and_an_index_past_the_bank_names_no_word() {
    let mut memory = Memory::filled(u32::MAX);
    let mut controller = memory.controller();
    controller.silence(5);
    assert_eq!(controller.routed(5), None);
    controller.route(InterruptController::<Host<'_>, Host<'_>>::SOURCES, 20);
    controller.silence(InterruptController::<Host<'_>, Host<'_>>::SOURCES);
    assert_eq!(
        controller.routed(InterruptController::<Host<'_>, Host<'_>>::SOURCES),
        None
    );
    controller.set_line(
        InterruptController::<Host<'_>, Host<'_>>::LINES,
        LineState::DISABLED,
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
        controller.set_line(30, state);
        assert_eq!(controller.line(30), state);
    }
    let bound = LineState::bound(3, LEVEL_BITS);
    controller.set_line(30, bound);
    assert_eq!(controller.line(30), bound);
    assert_eq!(bound.control, line_control(3, 3));
    drop(controller);
    let word = memory.clic().read_ctrl(line(30));
    assert!(word.ip(), "the pending bit stays");
    assert_eq!(word.attr_mode().get(), 3, "the privilege mode stays");
    let mut controller = memory.controller();
    controller.set_line(
        30,
        LineState {
            enabled: true,
            vectored: true,
            trigger: Trigger::NegativeEdge,
            control: 0xff,
        },
    );
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
    const SOURCE: u16 = <oer_esp32s31_pac::platform::FromCpu0 as InterruptSource>::SOURCE;
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
            controller.route(number, 20);
        }
        for number in 0..InterruptController::<Host<'_>, Host<'_>>::LINES {
            controller.set_line(number, LineState::bound(1, LEVEL_BITS));
        }
    }
    let mut router =
        Router::install(memory.controller(), INTERRUPT_TABLE).expect("a host controller installs");
    let tokens = Interrupts::take().expect("the test takes the tokens once");
    let _bound = router
        .bind(tokens.wake, WakeIrq)
        .expect("the wake entry is the table's");
    assert_eq!(router.verify(), Ok(()));
    drop(router);

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
