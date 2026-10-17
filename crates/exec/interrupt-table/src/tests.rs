use crate::{
    Error, Matrix, Route, disable, disable_route, enable, enable_route, install, verify,
    verify_required,
};

crate::__fake_matrix!();

#[test]
fn install_silences_the_current_core_s_sources_and_checks_their_slots() {
    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, INTERRUPT_TABLE).unwrap();
    // The timer is core zero's entry; core one's radio is not core zero's to
    // touch.
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);
    assert_eq!(matrix.routed(Core::One, Source::Radio), Some(Level::Two));

    let mut matrix = FakeMatrix::new(Core::Zero);
    matrix.slots[Source::Timer as usize] = 0x1234;
    assert_eq!(
        install(&mut matrix, INTERRUPT_TABLE),
        Err(Error::ForeignHandler {
            source: Source::Timer
        })
    );
}

#[test]
fn a_token_routes_its_source_to_its_level_on_its_core_alone() {
    let tokens = Interrupts::take().unwrap();
    assert!(Interrupts::take().is_none());

    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, INTERRUPT_TABLE).unwrap();
    enable(&mut matrix, INTERRUPT_TABLE, &tokens.timer).unwrap();
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), Some(Level::One));
    verify(&matrix, INTERRUPT_TABLE).unwrap();
    disable(&mut matrix, &tokens.timer);
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);

    // The radio belongs to core one.
    assert_eq!(
        enable(&mut matrix, INTERRUPT_TABLE, &tokens.radio),
        Err(Error::WrongCore {
            source: Source::Radio,
            core: Core::One,
            current: Core::Zero
        })
    );
    assert_eq!(matrix.routed(Core::Zero, Source::Radio), Some(Level::Two));

    // A token whose source the table does not list enables nothing.
    assert_eq!(
        enable(&mut matrix, &[], &tokens.timer),
        Err(Error::NotInTable {
            source: Source::Timer
        })
    );
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);

    // A route enables and disables as its token did.
    let timer = Route::new(tokens.timer);
    assert_eq!(timer.source(), Source::Timer);
    enable_route(&mut matrix, INTERRUPT_TABLE, &timer).unwrap();
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), Some(Level::One));
    disable_route(&mut matrix, &timer);
    assert_eq!(matrix.routed(Core::Zero, Source::Timer), None);
    let radio = Route::new(tokens.radio);
    assert!(matches!(
        enable_route(&mut matrix, INTERRUPT_TABLE, &radio),
        Err(Error::WrongCore { .. })
    ));
}

#[test]
fn a_required_source_must_be_listed_and_routed_on_its_core() {
    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, INTERRUPT_TABLE).unwrap();
    // Installed, the timer is silent until its owner routes it.
    assert_eq!(
        verify_required(&matrix, INTERRUPT_TABLE, [Source::Timer]),
        Err(Error::NotRouted {
            source: Source::Timer
        })
    );
    matrix.route(Source::Timer, Level::One);
    assert_eq!(
        verify_required(&matrix, INTERRUPT_TABLE, [Source::Timer]),
        Ok(())
    );
    // Core one's radio is checked on core one.
    matrix.silence(Core::One, Source::Radio);
    assert_eq!(
        verify_required(&matrix, INTERRUPT_TABLE, [Source::Radio]),
        Ok(())
    );
    assert_eq!(
        verify_required(&matrix, INTERRUPT_TABLE, [Source::Absent]),
        Err(Error::NotInTable {
            source: Source::Absent
        })
    );
}

#[test]
fn verify_rejects_a_source_routed_to_another_level() {
    let mut matrix = FakeMatrix::new(Core::Zero);
    install(&mut matrix, INTERRUPT_TABLE).unwrap();
    matrix.route(Source::Timer, Level::Two);
    assert_eq!(
        verify(&matrix, INTERRUPT_TABLE),
        Err(Error::WrongLevel {
            source: Source::Timer,
            level: Level::Two
        })
    );
}

#[test]
fn a_table_lists_each_source_once() {
    let matrix = FakeMatrix::new(Core::Zero);
    let twice = [
        crate::Binding {
            source: Source::Timer,
            level: Level::One,
            core: Core::Zero,
            handler: Some(Timer as unsafe extern "C" fn()),
        },
        crate::Binding {
            source: Source::Timer,
            level: Level::One,
            core: Core::Zero,
            handler: Some(Timer as unsafe extern "C" fn()),
        },
    ];
    assert_eq!(
        verify(&matrix, &twice),
        Err(Error::Duplicate {
            source: Source::Timer
        })
    );
}

#[test]
fn an_entry_its_cfg_leaves_out_has_no_binding() {
    assert_eq!(INTERRUPT_TABLE.len(), 3);
    assert_eq!(INTERRUPT_TABLE[2].source, Source::Absent);
    assert!(INTERRUPT_TABLE[2].handler.is_none());
}
