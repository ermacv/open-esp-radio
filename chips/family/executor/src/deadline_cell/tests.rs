extern crate std;

use std::{sync::Arc, thread, vec::Vec};

use super::*;

#[test]
fn a_quiet_cell_reads_what_was_published() {
    let cell = DeadlineCell::new(u64::MAX);
    assert_eq!(
        cell.read(),
        Some(Published {
            value: u64::MAX,
            generation: 0
        })
    );
    cell.publish(0x0000_0001_FFFF_FFFF);
    assert_eq!(
        cell.read(),
        Some(Published {
            value: 0x0000_0001_FFFF_FFFF,
            generation: 2
        })
    );
}

/// Every published value has equal halves, so a mixed read is visible.
fn value(round: u32) -> u64 {
    (u64::from(round) << 32) | u64::from(round)
}

#[test]
fn a_read_racing_a_write_is_whole_or_refused() {
    const ROUNDS: u32 = 200_000;
    let cell = Arc::new(DeadlineCell::new(value(0)));
    let writer = {
        let cell = Arc::clone(&cell);
        thread::spawn(move || {
            for round in 1..=ROUNDS {
                cell.publish(value(round));
            }
        })
    };
    let mut refused = 0_u64;
    let mut seen = Vec::new();
    let mut last = 0_u32;
    while !writer.is_finished() || seen.last() != Some(&ROUNDS) {
        match cell.read() {
            None => refused += 1,
            Some(published) => {
                let high = (published.value >> 32) as u32;
                assert_eq!(high, published.value as u32, "a torn value was accepted");
                assert!(high >= last, "a published value went backwards");
                assert!(published.generation.is_multiple_of(2));
                last = high;
                seen.push(high);
            }
        }
    }
    writer.join().unwrap();
    assert_eq!(
        cell.read().map(|published| published.value),
        Some(value(ROUNDS))
    );
    // The race is real on a multi-core host; either outcome is correct, the
    // count only shows that refusals happen and are survived.
    std::println!("refused {refused} of {} reads", refused + seen.len() as u64);
}
