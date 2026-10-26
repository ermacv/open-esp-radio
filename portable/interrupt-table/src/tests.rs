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
    // SAFETY: the test table defines this safe C ABI handler.
    unsafe extern "C" {
        safe fn __oer_interrupt_h0_l16();
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
    __oer_interrupt_h0_l16();

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
    __oer_interrupt_h0_l16();
    __oer_interrupt_h0_l16();
    // Each call of the line's symbol ran the entry's handler on the bound
    // part once.
    assert_eq!(
        <TimerToken as Entry>::cell().with(|part| part.handled),
        Some(2)
    );

    // The pair's entry writes its line once, then routes both sources to
    // it; its line's symbol runs its handler on the bound part.
    // SAFETY: the test table defines this safe C ABI handler.
    unsafe extern "C" {
        safe fn __oer_interrupt_h0_l18();
    }
    let before = zero.controller.writes.len();
    let _bound: crate::Bound<PairIrq> = zero.bind(tokens.pair, PairIrq::default()).unwrap();
    assert_eq!(
        zero.controller.writes[before..],
        [
            Write::Line {
                line: 18,
                state: LineState::bound(1, 3)
            },
            Write::Route {
                source: 4,
                line: 18
            },
            Write::Route {
                source: 7,
                line: 18
            },
        ]
    );
    zero.verify().unwrap();
    __oer_interrupt_h0_l18();
    assert_eq!(
        <PairToken as Entry>::cell().with(|part| part.handled),
        Some(1)
    );

    let mut one = fake_router(1);
    let _bound = one.bind(refused.token, refused.part).unwrap();
    assert_eq!(one.controller.routes[5], Some(17));
    assert_eq!(one.controller.lines[17], LineState::bound(2, 3));
    one.verify().unwrap();
}

#[test]
fn the_table_lays_out_each_entry_as_the_tools_read_it() {
    let [timer, radio, pair_4, pair_7, absent] = INTERRUPT_TABLE else {
        panic!("five bindings");
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
    // The pair's entry lays out one binding per source: the same line,
    // level, hart and handler.
    for (binding, source) in [(pair_4, 4), (pair_7, 7)] {
        assert_eq!(
            (binding.source, binding.line, binding.level, binding.hart),
            (source, 18, 1, 0)
        );
    }
    let (Some(first), Some(second)) = (pair_4.handler, pair_7.handler) else {
        panic!("both bindings of the pair have its handler");
    };
    assert!(core::ptr::fn_addr_eq(first, second));
    assert!(absent.handler.is_none());
}

extern "C" fn no_handler() {}

/// A table whose one entry of hart 0 is `binding`, as a table of wider
/// `Lines` than the controller's would hold it.
fn table_of(source: u16, line: u8, level: u8) -> &'static crate::Table {
    std::boxed::Box::leak(std::boxed::Box::new([crate::Binding {
        source,
        line,
        level,
        class: crate::Class::Hardware,
        hart: 0,
        handler: Some(no_handler),
    }]))
}

#[test]
fn install_refuses_an_entry_outside_the_controller_before_any_write() {
    for (table, error) in [
        (table_of(84, 20, 3), Error::NoSuchSource { source: 84 }),
        (table_of(3, 60, 3), Error::NoSuchLine { line: 60 }),
        (table_of(3, 48, 3), Error::NoSuchLine { line: 48 }),
        (table_of(3, 20, 9), Error::NoSuchLevel { level: 9 }),
        (table_of(3, 20, 0), Error::NoSuchLevel { level: 0 }),
    ] {
        assert_eq!(
            Router::install(FakeController::new(0), table).err(),
            Some(error)
        );
    }
    // Another hart's entry is its own router's to check.
    let other = std::boxed::Box::leak(std::boxed::Box::new([crate::Binding {
        hart: 1,
        ..crate::Binding {
            source: 84,
            line: 60,
            level: 3,
            class: crate::Class::Hardware,
            hart: 0,
            handler: Some(no_handler),
        }
    }]));
    assert!(Router::install(FakeController::new(0), other).is_ok());
}

#[test]
fn a_controller_refuses_a_source_or_line_it_lacks_and_writes_nothing() {
    use crate::Controller;
    let mut controller = FakeController::new(0);
    assert_eq!(controller.route(3, 48), Err(Error::NoSuchLine { line: 48 }));
    assert_eq!(
        controller.route(8, 20),
        Err(Error::NoSuchSource { source: 8 })
    );
    assert_eq!(
        controller.silence(8),
        Err(Error::NoSuchSource { source: 8 })
    );
    assert_eq!(
        controller.set_line(48, LineState::DISABLED),
        Err(Error::NoSuchLine { line: 48 })
    );
    assert!(controller.writes.is_empty());
}

#[test]
fn only_a_silent_source_no_entry_binds_gets_the_polling_proof() {
    let mut router = fake_router(0);
    // The absent entry's build leaves it out: nothing binds source 6.
    let _silent: crate::Silent<AbsentIrq> = router.silent().unwrap();
    // The timer's entry binds source 3, the radio's (hart one) source 5.
    assert_eq!(
        router.silent::<TimerIrq>().err(),
        Some(Error::Bindable { source: 3 })
    );
    assert_eq!(
        router.silent::<RadioIrq>().err(),
        Some(Error::Bindable { source: 5 })
    );
    // A part of several sources is silent only when each of them is.
    assert_eq!(
        router.silent::<PairIrq>().err(),
        Some(Error::Bindable { source: 4 })
    );
    router.controller.routes[6] = Some(30);
    assert_eq!(
        router.silent::<AbsentIrq>().err(),
        Some(Error::ForeignRoute {
            source: 6,
            line: 30
        })
    );
}
