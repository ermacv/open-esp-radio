use super::*;

const PARAMETERS: PhyPbusMemoryParameters = PhyPbusMemoryParameters {
    parameter_002: 0xbf,
    parameter_014: 1,
};

#[test]
fn exact_sixty_entry_rom_sequence_is_explicit() {
    let expected_counts = [8_u8, 5, 8, 5, 3, 1, 8, 5, 8, 5, 3, 1];
    let expected_first = [0_u8, 8, 13, 21, 26, 29, 30, 38, 43, 51, 56, 59];
    let expected_last = [7_u8, 12, 20, 25, 28, 29, 37, 42, 50, 55, 58, 59];
    let mut seen_counts = [0_u8; 12];
    let mut seen = 0_u8;
    let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);

    while let PhyPbusMemoryAction::Program(entry) = transition.action() {
        assert_eq!(entry.index(), seen_counts[entry.group() as usize]);
        if entry.index() == 0 {
            let control = entry.boundary().unwrap();
            assert_eq!(control.group(), entry.group());
            assert_eq!(
                control.first_entry(),
                expected_first[entry.group() as usize]
            );
            assert_eq!(control.last_entry(), expected_last[entry.group() as usize]);
        } else {
            assert_eq!(entry.boundary(), None);
        }
        seen_counts[entry.group() as usize] += 1;
        seen += 1;
        transition
            .advance(PhyPbusMemoryCompletion::Programmed(entry))
            .unwrap();
    }

    assert_eq!(seen, PHY_PBUS_MEMORY_ENTRY_COUNT);
    assert_eq!(seen_counts, expected_counts);
    assert_eq!(
        transition.action(),
        PhyPbusMemoryAction::Complete(PhyPbusMemoryOutcome)
    );
}

#[test]
fn retained_wake_regenerates_all_group_boundaries_without_table_entries() {
    let expected_first = [0_u8, 8, 13, 21, 26, 29, 30, 38, 43, 51, 56, 59];
    let expected_last = [7_u8, 12, 20, 25, 28, 29, 37, 42, 50, 55, 58, 59];
    let mut transition = PhyPbusBoundaryRestoreTransition::new();

    for group in 0..12 {
        let PhyPbusBoundaryRestoreAction::Restore(boundary) = transition.action() else {
            panic!("boundary transition completed before all groups");
        };
        assert_eq!(boundary.group(), group);
        assert_eq!(boundary.first_entry(), expected_first[group as usize]);
        assert_eq!(boundary.last_entry(), expected_last[group as usize]);
        assert!(PhyPbusBoundaryRestoreMmioBinding::new(transition.action()).is_ok());
        transition
            .advance(PhyPbusBoundaryRestoreCompletion::Restored(boundary))
            .unwrap();
    }

    assert_eq!(
        transition.action(),
        PhyPbusBoundaryRestoreAction::Complete(PhyPbusBoundaryRestoreOutcome)
    );
    assert_eq!(
        PhyPbusBoundaryRestoreMmioBinding::new(transition.action()),
        Err(PhyPbusMemoryBindingError::UnsupportedAction)
    );
}

#[test]
fn retained_boundary_restore_rejects_an_out_of_order_group() {
    let mut transition = PhyPbusBoundaryRestoreTransition::new();
    let PhyPbusBoundaryRestoreAction::Restore(first) = transition.action() else {
        panic!("first boundary must be available");
    };
    transition
        .advance(PhyPbusBoundaryRestoreCompletion::Restored(first))
        .unwrap();
    assert_eq!(
        transition.advance(PhyPbusBoundaryRestoreCompletion::Restored(first)),
        Err(PhyPbusBoundaryRestoreError::WrongCompletion)
    );
}

#[test]
fn completion_must_match_the_exact_entry() {
    let mut transition = PhyPbusMemoryTransition::new(PARAMETERS);
    let PhyPbusMemoryAction::Program(first) = transition.action() else {
        panic!("first action must program");
    };
    let mut other = PhyPbusMemoryTransition::new(PhyPbusMemoryParameters {
        parameter_002: 0,
        parameter_014: 1,
    });
    let PhyPbusMemoryAction::Program(wrong) = other.action() else {
        panic!("first action must program");
    };
    assert_eq!(
        transition.advance(PhyPbusMemoryCompletion::Programmed(wrong)),
        Err(PhyPbusMemoryTransitionError::WrongCompletion)
    );
    transition
        .advance(PhyPbusMemoryCompletion::Programmed(first))
        .unwrap();
    assert!(matches!(
        transition.action(),
        PhyPbusMemoryAction::Program(_)
    ));
    let _ = &mut other;
}

#[test]
fn mmio_binding_accepts_program_but_not_terminal_state() {
    let transition = PhyPbusMemoryTransition::new(PARAMETERS);
    assert!(PhyPbusMemoryMmioBinding::new(transition.action()).is_ok());
    assert_eq!(
        PhyPbusMemoryMmioBinding::new(PhyPbusMemoryAction::Complete(PhyPbusMemoryOutcome)),
        Err(PhyPbusMemoryBindingError::UnsupportedAction)
    );
}
