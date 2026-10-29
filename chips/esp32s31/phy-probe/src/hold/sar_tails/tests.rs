extern crate std;
use super::*;
use crate::hold::notes::Notes;
use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use std::string::String;

struct Output(String);
impl Notes for Output {
    async fn note(&mut self, note: &Note<'static>) {
        oer_diagnostics::render::note(note, &mut self.0).unwrap();
        self.0.push('\n');
    }
}

#[test]
fn readiness_boundaries_largest_waits_and_notes_keep_their_order() {
    let mut tails = Tails::default();
    for (index, limit) in LIMITS.into_iter().enumerate() {
        tails.observe(limit, index as u16, true).unwrap();
    }
    tails.observe(100_001, 20, false).unwrap();
    assert_eq!(tails.bins, [[1; 6]; 3]);
    assert_eq!(
        tails.largest,
        [100_001, 100_000, 10_000, 1000, 500, 200, 100, 50]
    );
    assert_eq!(
        (tails.total, tails.expired, tails.max_observations),
        (18, 1, 20)
    );
    let mut output = Output(String::new());
    {
        let mut report = pin!(tails.report(3, &mut output));
        assert!(matches!(
            report
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Ready(())
        ));
    }
    let lines: std::vec::Vec<_> = output.0.lines().collect();
    assert_eq!(lines.len(), 8);
    assert_eq!(
        lines[0],
        "OPEN_RADIO_HIL sar_tail tick=3 first=0 bins=1,1,1,1,1,1"
    );
    assert_eq!(
        lines[2],
        "OPEN_RADIO_HIL sar_tail tick=3 first=12 bins=1,1,1,1,1,1"
    );
    assert_eq!(
        lines[3],
        "OPEN_RADIO_HIL sar_tail tick=3 first=0 largest=100001,100000"
    );
    assert_eq!(
        lines[6],
        "OPEN_RADIO_HIL sar_tail tick=3 first=6 largest=100,50"
    );
    assert_eq!(
        lines[7],
        "OPEN_RADIO_HIL sar_tail tick=3 total=18 expired=1 max_observations=20"
    );
}

#[test]
fn unrepresentable_observations_return_an_error_without_wrapping() {
    let mut tails = Tails {
        total: u32::MAX,
        ..Tails::default()
    };
    assert_eq!(
        tails.observe(1, 1, true),
        Err(ObservationError::CounterOverflow)
    );
    assert_eq!(tails.total, u32::MAX);
    let mut tails = Tails {
        expired: u32::MAX,
        ..Tails::default()
    };
    assert_eq!(
        tails.observe(1, 1, false),
        Err(ObservationError::CounterOverflow)
    );
    assert_eq!(tails.expired, u32::MAX);
    let mut tails = Tails::default();
    tails.bins[0][0] = u32::MAX;
    assert_eq!(
        tails.observe(1, 1, true),
        Err(ObservationError::CounterOverflow)
    );
    assert_eq!(tails.bins[0][0], u32::MAX);
}
