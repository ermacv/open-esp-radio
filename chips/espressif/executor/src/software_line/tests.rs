use core::cell::Cell;

use super::{SoftwareLine, serve};

/// A level line whose handler runs while it is raised.
struct Line {
    raised: Cell<bool>,
}

impl SoftwareLine for Line {
    fn raise(&self) {
        self.raised.set(true);
    }
    fn lower(&self) {
        self.raised.set(false);
    }
}

#[test]
fn a_raise_during_the_drain_leads_to_another_round() {
    let line = Line {
        raised: Cell::new(false),
    };
    let pending = Cell::new(1_u32);
    line.raise();
    let mut rounds = 0;
    // The hardware runs the handler while the line is raised.
    while line.raised.get() {
        rounds += 1;
        serve(&line, || {
            let work = pending.replace(0);
            if rounds == 1 {
                // A waker posts more work and raises while the first round
                // takes the work it found.
                pending.set(pending.get() + 1);
                line.raise();
            }
            work
        });
        assert!(rounds < 4, "the line settles");
    }
    assert_eq!(rounds, 2, "the second raise ran a second round");
    assert_eq!(pending.get(), 0, "no work was left behind");
}

#[test]
fn a_raise_after_the_drain_is_not_lowered_by_it() {
    let line = Line {
        raised: Cell::new(true),
    };
    serve(&line, || ());
    line.raise();
    assert!(line.raised.get());
}
