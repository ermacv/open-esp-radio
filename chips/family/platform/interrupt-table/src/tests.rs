use crate::{Entry, Error, LineExpectation, LineObservation, Router};
use oer_portable_riscv::clic::Control;

crate::__fake_table!();

#[test]
fn install_silences_every_source_and_disables_every_line_before_it_checks() {
    for hart in [0, 1] {
        let controller = match hart {
            0 => {
                Router::<_, 0, _>::install(FakeController::new(0), InterruptTable)
                    .unwrap()
                    .controller
            }
            _ => {
                Router::<_, 1, _>::install(FakeController::new(1), InterruptTable)
                    .unwrap()
                    .controller
            }
        };
        assert!(controller.routes.iter().all(Option::is_none));
        assert!(
            controller
                .lines
                .iter()
                .all(|line| *line == FakeLine::Disabled)
        );
        // Every source silent first: no line is touched while a source
        // still holds it.
        let first_line = controller
            .writes
            .iter()
            .position(|write| matches!(write, Write::Disable { .. }))
            .unwrap();
        assert_eq!(first_line, 8);
        // Nothing but the silences and the disables: a level task's line
        // too waits for its entry's binding.
        assert_eq!(controller.writes.len(), 8 + 48, "hart {hart}");
    }
}

#[test]
fn verify_expects_a_level_task_line_disabled_or_bound_as_a_task_at_its_level() {
    let task = LineExpectation::Task(Control::level::<3, 3>());
    // Disabled before its binding, its task's binding after it.
    let mut router = fake_router::<0>();
    assert_eq!(router.verify(), Ok(()));
    router.controller.lines[34] = FakeLine::Task(Control::level::<3, 3>());
    assert_eq!(router.verify(), Ok(()));
    for (held, observed) in [
        (
            FakeLine::Bound(Control::level::<3, 3>()),
            0x100 | u32::from(Control::level::<3, 3>().bits()),
        ),
        (
            FakeLine::Task(Control::level::<2, 3>()),
            0x200 | u32::from(Control::level::<2, 3>().bits()),
        ),
        (FakeLine::Foreign(0x5f03_0100), 0x5f03_0100),
    ] {
        let mut router = fake_router::<0>();
        router.controller.lines[34] = held;
        assert_eq!(
            router.verify(),
            Err(Error::WrongLine {
                line: 34,
                observed: LineObservation::of_word(observed),
                expected: task,
            })
        );
    }
    // A source routed to the task's line is no entry's there.
    let mut router = fake_router::<0>();
    router.controller.routes[3] = Some(34);
    assert_eq!(
        router.verify(),
        Err(Error::ForeignRoute {
            source: 3,
            line: 34
        })
    );
    // Hart 1 has no task: its line 34 is disabled.
    let mut router = fake_router::<1>();
    router.controller.lines[34] = FakeLine::Task(Control::level::<3, 3>());
    assert_eq!(
        router.verify(),
        Err(Error::WrongLine {
            line: 34,
            observed: LineObservation::of_word(task.record()),
            expected: LineExpectation::Disabled,
        })
    );
}

#[test]
fn a_level_task_is_laid_out_in_its_own_table_and_never_in_the_source_table() {
    assert_eq!(<KeeperToken as Entry>::CLASS, crate::Class::LevelTask);
    assert_eq!(
        <KeeperToken as Entry>::CONTEXT,
        crate::Context { level: 3, hart: 0 }
    );
    // The exported task table: the keeper's task, then the one the build
    // leaves out, with no handler.
    let [keeper, absent] = LEVEL_TASK_TABLE else {
        panic!("two level tasks");
    };
    assert_eq!((keeper.line, keeper.level, keeper.hart), (34, 3, 0));
    assert!(keeper.handler.is_some());
    assert_eq!((absent.line, absent.level, absent.hart), (35, 3, 0));
    assert!(absent.handler.is_none());
    assert_eq!(<InterruptTable as crate::Table>::TASKS.len(), 2);
    // No element of the source table is a task's.
    assert!(
        INTERRUPT_TABLE
            .iter()
            .all(|binding| binding.class != crate::Class::LevelTask && binding.line < 34)
    );
}

#[test]
fn a_binding_fault_records_its_line_and_hart_in_its_first_word() {
    for (line, hart) in [(34, 0), (17, 1), (47, 1)] {
        let fault = crate::BindFault {
            line,
            hart,
            kind: crate::BindFaultKind::HandlerOccupied,
        };
        assert_eq!(
            fault.diagnostic(),
            (
                crate::BindFaultCode::HandlerOccupied,
                [u32::from(line) | u32::from(hart) << 8, 0, 0, 0]
            )
        );
    }
}

#[test]
fn a_task_line_s_record_names_its_kind_apart_from_a_routed_line() {
    let control = Control::level::<3, 3>();
    assert_eq!(LineExpectation::Disabled.record(), 0);
    assert_eq!(
        LineExpectation::Bound(control).record(),
        0x100 | u32::from(control.bits())
    );
    assert_eq!(
        LineExpectation::Task(control).record(),
        0x200 | u32::from(control.bits())
    );
}

#[test]
fn verify_refuses_a_foreign_route_a_wrong_line_and_a_stray_enabled_line() {
    let mut router = fake_router::<0>();
    router.controller.routes[7] = Some(30);
    assert_eq!(
        router.verify(),
        Err(Error::ForeignRoute {
            source: 7,
            line: 30
        })
    );

    let mut router = fake_router::<0>();
    // The timer's source on its own line, but the line at another level.
    router.controller.routes[3] = Some(16);
    router.controller.lines[16] = FakeLine::Bound(Control::level::<2, 3>());
    assert_eq!(
        router.verify(),
        Err(Error::WrongLine {
            line: 16,
            observed: LineObservation::of_word(0x100 | u32::from(Control::level::<2, 3>().bits())),
            expected: LineExpectation::Bound(Control::level::<1, 3>()),
        })
    );
    // An attribute no binding writes, as an edge trigger.
    router.controller.lines[16] = FakeLine::Foreign(0x5f02_0100);
    assert!(matches!(
        router.verify(),
        Err(Error::WrongLine { line: 16, .. })
    ));

    // The timer's source on another entry's line of another hart.
    let mut router = fake_router::<0>();
    router.controller.routes[3] = Some(17);
    assert_eq!(
        router.verify(),
        Err(Error::ForeignRoute {
            source: 3,
            line: 17
        })
    );

    let mut router = fake_router::<0>();
    router.controller.lines[40] = FakeLine::Bound(Control::level::<3, 3>());
    assert_eq!(
        router.verify(),
        Err(Error::WrongLine {
            line: 40,
            observed: LineObservation::of_word(0x100 | u32::from(Control::level::<3, 3>().bits())),
            expected: LineExpectation::Disabled,
        })
    );

    // The timer's line bound at its level with its source routed nowhere:
    // a line no routed source holds is disabled, an entry's line too.
    let mut router = fake_router::<0>();
    router.controller.lines[16] = FakeLine::Bound(Control::level::<1, 3>());
    assert_eq!(
        router.verify(),
        Err(Error::WrongLine {
            line: 16,
            observed: LineObservation::of_word(0x100 | u32::from(Control::level::<1, 3>().bits())),
            expected: LineExpectation::Disabled,
        })
    );
}

#[test]
fn a_router_of_one_hart_refuses_the_controller_of_another() {
    assert_eq!(
        Router::<_, 0, _>::install(FakeController::new(1), InterruptTable).err(),
        Some(Error::WrongHart {
            hart: 0,
            current: 1
        })
    );
}

// The only test that takes the tokens and binds: both are once per image.
#[test]
fn a_binding_places_the_part_then_writes_the_line_then_the_route() {
    let timer_handler = INTERRUPT_TABLE[0].handler.unwrap();
    let tokens = Interrupts::take().unwrap();
    assert!(Interrupts::take().is_none());

    // The radio's entry is hart one's: binding it on hart zero's router
    // fails to compile (`Router::bind`).
    let mut zero = fake_router::<0>();
    let before = zero.controller.writes.len();
    let (token, radio_part) = (tokens.radio, RadioIrq);

    // Before its binding the timer's handler finds no part and runs nothing.
    assert!(<TimerToken as Entry>::PROOF.cell().with(|_| ()).is_none());
    timer_handler();

    let _bound: crate::Bound<TimerToken> = zero.bind(tokens.timer, TimerIrq::default()).unwrap();
    assert_eq!(crate::Bound::<TimerToken>::SOURCE, 3);
    assert_eq!(
        zero.controller.writes[before..],
        [
            Write::Bind {
                line: 16,
                control: Control::level::<1, 3>()
            },
            Write::Route {
                source: 3,
                line: 16
            },
        ]
    );
    zero.verify().unwrap();
    assert_eq!(
        <TimerToken as Entry>::PROOF
            .cell()
            .with(|part| part.handled),
        Some(0)
    );
    timer_handler();
    timer_handler();
    // Each call of the line's symbol ran the entry's handler on the bound
    // part once.
    assert_eq!(
        <TimerToken as Entry>::PROOF
            .cell()
            .with(|part| part.handled),
        Some(2)
    );

    // The pair's entry writes its line once, then routes both sources to
    // it; its line's symbol runs its handler on the bound part.
    let pair_handler = INTERRUPT_TABLE[2].handler.unwrap();
    let before = zero.controller.writes.len();
    let _bound: crate::Bound<PairToken> = zero.bind(tokens.pair, PairIrq::default()).unwrap();
    assert_eq!(
        zero.controller.writes[before..],
        [
            Write::Bind {
                line: 18,
                control: Control::level::<1, 3>()
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
    pair_handler();
    assert_eq!(
        <PairToken as Entry>::PROOF.cell().with(|part| part.handled),
        Some(1)
    );

    // Bypassing the once gate cannot replace a bound part or write a route.
    #[allow(clippy::disallowed_methods, reason = "regression for a forged token")]
    let forged = TimerToken(crate::__private::Unique::new());
    let before = zero.controller.writes.len();
    let fault = zero.bind(forged, TimerIrq::default()).unwrap_err();
    assert_eq!(fault.fault.kind, crate::BindFaultKind::StorageClaimed);
    assert_eq!(zero.controller.writes.len(), before);
    timer_handler();
    assert_eq!(TimerToken::PROOF.cell().with(|part| part.handled), Some(3));

    // The keeper's level task: its part placed, its line bound as a task's
    // and nothing routed; before its binding its handler runs nothing.
    let keeper_handler = LEVEL_TASK_TABLE[0].handler.unwrap();
    keeper_handler();
    assert!(<KeeperToken as Entry>::PROOF.cell().with(|_| ()).is_none());
    let before = zero.controller.writes.len();
    let routes = zero.controller.routes;
    let _bound: crate::Bound<KeeperToken> =
        zero.bind(tokens.keeper, KeeperPart::default()).unwrap();
    assert_eq!(crate::Bound::<KeeperToken>::LEVEL, 3);
    assert_eq!(
        zero.controller.writes[before..],
        [Write::BindTask {
            line: 34,
            control: Control::level::<3, 3>()
        }]
    );
    assert_eq!(zero.controller.routes, routes);
    zero.verify().unwrap();
    keeper_handler();
    keeper_handler();
    assert_eq!(
        <KeeperToken as Entry>::PROOF
            .cell()
            .with(|part| part.handled),
        Some(2)
    );

    let mut one = fake_router::<1>();
    let _bound = one.bind(token, radio_part).unwrap();
    assert_eq!(one.controller.routes[5], Some(17));
    assert_eq!(
        one.controller.lines[17],
        FakeLine::Bound(Control::level::<2, 3>())
    );
    one.verify().unwrap();
}

#[test]
fn the_table_lays_out_each_entry_as_the_tools_read_it() {
    let [timer, radio, pair_4, pair_7, occupied, absent] = INTERRUPT_TABLE else {
        panic!("six bindings");
    };
    assert_eq!((occupied.source, occupied.line), (2, 19));
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

#[test]
fn only_a_silent_source_no_entry_binds_gets_the_polling_proof() {
    let mut router = fake_router::<0>();
    // The absent entry's build leaves it out: nothing binds source 6.
    let _silent: crate::Silent<AbsentIrq> = router.silent().unwrap();
    // A part an entry binds has no silence: it fails to compile
    // (`Router::silent`).
    router.controller.routes[6] = Some(30);
    assert_eq!(
        router.silent::<AbsentIrq>().err(),
        Some(Error::ForeignRoute {
            source: 6,
            line: 30
        })
    );
}

#[test]
fn a_binding_whose_handler_cell_is_occupied_keeps_its_owners_and_writes_nothing() {
    use crate::BindFaultCode;
    let mut router = fake_router::<0>();
    // A part another context installed in the entry's cell before it.
    let previous = std::boxed::Box::leak(std::boxed::Box::new(OccupiedIrq::default()));
    let _installed = OccupiedToken::PROOF.cell().install(previous).ok().unwrap();
    #[allow(clippy::disallowed_methods, reason = "regression for an occupied cell")]
    let token = OccupiedToken(crate::__private::Unique::new());
    let before = router.controller.writes.len();
    let error = router.bind(token, OccupiedIrq::default()).unwrap_err();
    assert_eq!((error.fault.line, error.fault.hart), (19, 0));
    assert_eq!(
        error.fault.diagnostic(),
        (BindFaultCode::HandlerOccupied, [19, 0, 0, 0])
    );
    drop(error);
    // Nothing written and nothing routed; the installed part stays.
    assert_eq!(router.controller.writes.len(), before);
    assert_eq!(router.controller.routes[2], None);
    let vector = INTERRUPT_TABLE[4].handler.unwrap();
    vector();
    assert_eq!(
        OccupiedToken::PROOF.cell().with(|part| part.handled),
        Some(1)
    );
}
