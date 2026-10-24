use crate::{Entry, Error, LineState, Router, Trigger, line_control};

crate::__fake_table!();

#[test]
fn a_line_s_control_byte_holds_its_level_in_the_top_bits() {
    let controls: std::vec::Vec<u8> = (1..=8).map(|level| line_control(level, 3)).collect();
    assert_eq!(controls, [0x1f, 0x3f, 0x5f, 0x7f, 0x9f, 0xbf, 0xdf, 0xff]);
    assert_eq!(line_control(1, 4), 0x0f);
    assert_eq!(line_control(16, 4), 0xff);
}

#[test]
fn install_silences_every_source_and_disables_every_line_before_it_checks() {
    let router = Router::install(FakeController::new(0), INTERRUPT_TABLE).unwrap();
    let controller = &router.controller;
    assert!(controller.routes.iter().all(Option::is_none));
    assert!(
        controller
            .lines
            .iter()
            .all(|line| *line == LineState::DISABLED)
    );
    // Every source silent first: no line is touched while a source still
    // holds it.
    let first_line = controller
        .writes
        .iter()
        .position(|write| matches!(write, Write::Line { .. }))
        .unwrap();
    assert_eq!(first_line, 8);
    assert_eq!(controller.writes.len(), 8 + 48);
}

#[test]
fn verify_refuses_a_foreign_route_a_wrong_line_and_a_stray_enabled_line() {
    let mut router = fake_router(0);
    router.controller.routes[7] = Some(30);
    assert_eq!(
        router.verify(),
        Err(Error::ForeignRoute {
            source: 7,
            line: 30
        })
    );

    let mut router = fake_router(0);
    // The timer's source on its own line, but the line at another level.
    router.controller.routes[3] = Some(16);
    router.controller.lines[16] = LineState::bound(2, 3);
    assert_eq!(
        router.verify(),
        Err(Error::WrongLine {
            line: 16,
            state: LineState::bound(2, 3),
            expected: LineState::bound(1, 3),
        })
    );
    router.controller.lines[16] = LineState {
        trigger: Trigger::PositiveEdge,
        ..LineState::bound(1, 3)
    };
    assert!(matches!(
        router.verify(),
        Err(Error::WrongLine { line: 16, .. })
    ));

    // The timer's source on another entry's line of another hart.
    let mut router = fake_router(0);
    router.controller.routes[3] = Some(17);
    assert_eq!(
        router.verify(),
        Err(Error::ForeignRoute {
            source: 3,
            line: 17
        })
    );

    let mut router = fake_router(0);
    router.controller.lines[40] = LineState::bound(3, 3);
    assert_eq!(
        router.verify(),
        Err(Error::WrongLine {
            line: 40,
            state: LineState::bound(3, 3),
            expected: LineState::DISABLED,
        })
    );
}

// The only test that takes the tokens and binds: both are once per image.
#[test]
fn a_binding_places_the_part_then_writes_the_line_then_the_route() {
    unsafe extern "C" {
        fn __oer_interrupt_h0_l16();
    }
    let tokens = Interrupts::take().unwrap();
    assert!(Interrupts::take().is_none());

    // The radio's entry is hart one's: hart zero's router refuses it and
    // writes nothing.
    let mut zero = fake_router(0);
    let before = zero.controller.writes.len();
    let refused = zero.bind(tokens.radio, RadioIrq).unwrap_err();
    assert_eq!(
        refused.error,
        Error::WrongHart {
            source: 5,
            hart: 1,
            current: 0
        }
    );
    assert_eq!(zero.controller.writes.len(), before);

    // Before its binding the timer's handler finds no part and runs nothing.
    assert!(<TimerToken as Entry>::cell().with(|_| ()).is_none());
    // SAFETY: the table's handler of line 16 takes no arguments and only
    // borrows the timer's cell.
    unsafe { __oer_interrupt_h0_l16() };

    let _bound: crate::Bound<TimerIrq> = zero.bind(tokens.timer, TimerIrq::default()).unwrap();
    assert_eq!(crate::Bound::<TimerIrq>::SOURCE, 3);
    assert_eq!(
        zero.controller.writes[before..],
        [
            Write::Line {
                line: 16,
                state: LineState::bound(1, 3)
            },
            Write::Route {
                source: 3,
                line: 16
            },
        ]
    );
    zero.verify().unwrap();
    assert_eq!(
        <TimerToken as Entry>::cell().with(|part| part.handled),
        Some(0)
    );
    // SAFETY: as above; the cell now lends the part to `on_timer`.
    unsafe {
        __oer_interrupt_h0_l16();
        __oer_interrupt_h0_l16();
    }
    // Each call of the line's symbol ran the entry's handler on the bound
    // part once.
    assert_eq!(
        <TimerToken as Entry>::cell().with(|part| part.handled),
        Some(2)
    );

    let mut one = fake_router(1);
    let _bound = one.bind(refused.token, refused.part).unwrap();
    assert_eq!(one.controller.routes[5], Some(17));
    assert_eq!(one.controller.lines[17], LineState::bound(2, 3));
    one.verify().unwrap();
}

#[test]
fn the_table_lays_out_each_entry_as_the_tools_read_it() {
    let [timer, radio, absent] = INTERRUPT_TABLE else {
        panic!("three entries");
    };
    assert_eq!(
        (
            timer.source,
            timer.line,
            timer.level,
            timer.hart,
            timer.class
        ),
        (3, 16, 1, 0, crate::Class::Hardware)
    );
    assert!(timer.handler.is_some());
    assert_eq!(
        (
            radio.source,
            radio.line,
            radio.level,
            radio.hart,
            radio.class
        ),
        (5, 17, 2, 1, crate::Class::Executor)
    );
    assert!(absent.handler.is_none());
}
