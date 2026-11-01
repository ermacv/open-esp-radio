use std::vec::Vec;

use oer_diagnostics::note::Topic;

use super::*;

const TRAP: TrapIdentity = TrapIdentity {
    mcause: 0x3800_0002,
    mepc: 0x5013_1378,
    mtval: 0,
    ra: 0x5003_0aa4,
    sp: 0x5019_f080,
};

fn interrupt(stopped: bool) -> Observed {
    Observed {
        hart: 0,
        trap: TrapIdentity {
            mcause: 0x8000_0017,
            ..TRAP
        },
        stopped,
        mtvec: 0x4000_0003,
        mtvt: 0x4000_0400,
        pending: [1, 0, 0, 0, 0, 1 << 31],
        line: Some(LineView {
            vector: 23,
            slot: 0x4000_1234,
            state: 0x0201,
            routed: [0, 1 << 4, 0, 0, 0, 0],
        }),
    }
}

fn exception() -> Observed {
    Observed {
        hart: 1,
        trap: TRAP,
        stopped: true,
        mtvec: 0x4000_0003,
        mtvt: 0x4000_0400,
        pending: [0, 0, 1 << 7, 0, 0, 0],
        line: None,
    }
}

/// The notes a whole report emits, and the record after it.
fn reported(record: &mut Record) -> Result<Vec<&'static str>, ()> {
    let mut notes = Vec::new();
    report(record, |note| {
        notes.push(match note.topic {
            Topic::FatalTrapVectors => "vectors",
            Topic::FatalIrqVector => "vector",
            Topic::FatalIrqPending => "pending",
            Topic::FatalIrqRouted => "routed",
            _ => "other",
        });
        Ok(())
    })
    .map(|()| notes)
}

#[test]
fn the_observations_survive_their_record_unchanged() {
    for observed in [interrupt(true), interrupt(false), exception()] {
        let record = observed.record().unwrap();
        assert_eq!(
            Observed::from_record(&record.peek().unwrap()),
            Some(observed)
        );
    }
}

#[test]
fn an_interrupt_under_the_stop_reports_its_line_pending_and_routes_then_clears() {
    let mut record = interrupt(true).record().unwrap();
    assert_eq!(
        reported(&mut record),
        Ok(vec!["vectors", "vector", "pending", "routed"])
    );
    assert!(record.peek().is_err());
}

#[test]
fn routes_read_without_the_stop_are_not_reported() {
    let mut record = interrupt(false).record().unwrap();
    assert_eq!(
        reported(&mut record),
        Ok(vec!["vectors", "vector", "pending"])
    );
}

#[test]
fn an_exception_names_no_line_and_reports_its_vector_registers_and_pending_sources() {
    let mut record = exception().record().unwrap();
    assert_eq!(reported(&mut record), Ok(vec!["vectors", "pending"]));
}

fn previous(
    hart: u32,
    trap: TrapIdentity,
) -> oer_espressif_staged_runtime::trap::PreviousException {
    oer_espressif_staged_runtime::trap::PreviousException {
        hart,
        mcause: trap.mcause,
        mepc: trap.mepc,
        mtval: trap.mtval,
        ra: trap.ra,
        sp: trap.sp,
        mscratch: 0x5019_f080,
    }
}

#[test]
fn observations_follow_only_the_trap_whose_registers_they_copied() {
    let observed = exception();
    assert!(observed.follows(&previous(1, TRAP)));
    // Another hart, or another trap that left an older record behind.
    assert!(!observed.follows(&previous(0, TRAP)));
    for other in [
        TrapIdentity {
            mepc: 0x5013_0000,
            ..TRAP
        },
        TrapIdentity {
            sp: 0x5019_f000,
            ..TRAP
        },
        TrapIdentity {
            mcause: 0x3800_0005,
            ..TRAP
        },
        TrapIdentity { mtval: 4, ..TRAP },
        TrapIdentity { ra: 0, ..TRAP },
    ] {
        assert!(!observed.follows(&previous(1, other)), "{other:?}");
    }
}

#[test]
fn a_refused_note_keeps_the_record_until_a_whole_report() {
    for refused in 0..3 {
        let mut record = interrupt(true).record().unwrap();
        let mut count = 0;
        assert!(
            report(&mut record, |_| {
                let accept = count != refused;
                count += 1;
                if accept { Ok(()) } else { Err(()) }
            })
            .is_err()
        );
        assert_eq!(
            Observed::from_record(&record.peek().unwrap()),
            Some(interrupt(true))
        );
        assert_eq!(
            reported(&mut record),
            Ok(vec!["vectors", "vector", "pending", "routed"])
        );
        assert!(record.peek().is_err());
    }
}

#[test]
fn an_empty_foreign_or_unknown_record_is_neither_reported_nor_cleared() {
    // A corrupt envelope fails `Record::peek` before any decoding
    // (`oer-diagnostics`' own tests); these pass it and fail here.
    let foreign = Record::encode(
        Class::UnexpectedHardware,
        Domain::Wifi,
        1,
        0,
        0,
        &0x1234_u32.to_le_bytes(),
    )
    .unwrap();
    let mut flags = interrupt(true).payload();
    flags.flags |= 1 << 7;
    let unknown = Record::encode(
        Class::CpuException,
        Domain::ExceptionObservation,
        OBSERVED,
        0,
        0,
        &flags,
    )
    .unwrap();
    for record in [Record::EMPTY, foreign, unknown] {
        let mut kept = record;
        let mut emitted = 0;
        assert!(
            report(&mut kept, |_| {
                emitted += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(emitted, 0);
        assert_eq!(kept, record);
    }
}
