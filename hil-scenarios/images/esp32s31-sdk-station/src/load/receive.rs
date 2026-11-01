//! What the image counts of the datagrams on a run's port: the run's load
//! in the shared [`Tally`], and beside it what the tally does not count.

use oer_services_load::Received;
use oer_services_load::header::Header;
use oer_services_load::tally::{Arrival, Tally};

/// What one datagram on the port is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Datagram {
    /// It asks for an echo, of any run: the image sends it back and counts
    /// it only as echoed or refused, never as load.
    Echo(Header),
    /// The run's load.
    Load(Header),
    /// Another run's, or no load header.
    Foreign,
}

/// `payload` on the port of run `run`.
pub(crate) fn classify(payload: &[u8], run: u32) -> Datagram {
    match Header::read(payload) {
        Some(header) if header.echo_request => Datagram::Echo(header),
        Some(header) if header.run == run => Datagram::Load(header),
        _ => Datagram::Foreign,
    }
}

/// `header` turned into the echo of its datagram.
pub(crate) fn echo(header: Header) -> Header {
    Header {
        echo_request: false,
        echo_reply: true,
        ..header
    }
}

/// The counters of one run's received datagrams that its tally does not
/// keep.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Receiving {
    window_datagrams: u64,
    window_bytes: u64,
    foreign: u64,
    echoed: u64,
    echo_refused: u64,
    first_us: Option<u64>,
    last_us: Option<u64>,
}

impl Receiving {
    /// Count load datagram `sequence` of `bytes` UDP payload bytes, received
    /// at `now_us`, `in_window` or not. A distinct one (new or late) is
    /// received; a duplicate or untracked one only counts in `tally`.
    pub(crate) fn load<const WORDS: usize>(
        &mut self,
        tally: &mut Tally<WORDS>,
        sequence: u64,
        bytes: usize,
        now_us: u64,
        in_window: bool,
    ) {
        let bytes = u64::try_from(bytes).unwrap_or(u64::MAX);
        match tally.count(sequence, bytes) {
            Arrival::New | Arrival::Late => {
                if in_window {
                    self.window_datagrams = self.window_datagrams.saturating_add(1);
                    self.window_bytes = self.window_bytes.saturating_add(bytes);
                }
                self.first_us.get_or_insert(now_us);
                self.last_us = Some(now_us);
            }
            Arrival::Duplicate | Arrival::Untracked => {}
        }
    }

    /// A datagram on the port that is not the run's load.
    pub(crate) fn foreign(&mut self) {
        self.foreign = self.foreign.saturating_add(1);
    }

    /// An echo the socket took, or refused.
    pub(crate) fn echo(&mut self, taken: bool) {
        let counter = if taken {
            &mut self.echoed
        } else {
            &mut self.echo_refused
        };
        *counter = counter.saturating_add(1);
    }

    /// The run's received counters with `tally`'s.
    pub(crate) fn received<const WORDS: usize>(&self, tally: &Tally<WORDS>) -> Received {
        Received {
            datagrams: tally.datagrams,
            bytes: tally.bytes,
            window_datagrams: self.window_datagrams,
            window_bytes: self.window_bytes,
            lost: tally.lost(),
            reordered: tally.reordered,
            duplicates: tally.duplicates,
            untracked: tally.untracked,
            highest: tally.highest,
            foreign: self.foreign,
            echoed: self.echoed,
            echo_refused: self.echo_refused,
            first_us: self.first_us,
            last_us: self.last_us,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_services_load::header::LENGTH;

    /// Each arrival: its sequence and whether it falls in the window.
    fn received(arrivals: &[(u64, bool)]) -> Received {
        let mut tally = Tally::<4>::new();
        let mut receiving = Receiving::default();
        for (at, (sequence, in_window)) in arrivals.iter().enumerate() {
            receiving.load(&mut tally, *sequence, 1400, 1000 + at as u64, *in_window);
        }
        receiving.received(&tally)
    }

    #[test]
    fn the_tally_counts_distinct_lost_late_and_duplicate_load() {
        let counts = received(&[(0, true), (2, true), (2, true), (1, true), (5, true)]);
        assert_eq!((counts.datagrams, counts.bytes), (4, 4 * 1400));
        assert_eq!(
            (counts.lost, counts.reordered, counts.duplicates),
            (2, 1, 1)
        );
        assert_eq!(counts.highest, Some(5));
        assert_eq!((counts.first_us, counts.last_us), (Some(1000), Some(1004)));
    }

    #[test]
    fn only_distinct_load_inside_the_window_counts_as_the_window_s() {
        let counts = received(&[(0, false), (1, true), (1, true), (2, true), (3, false)]);
        assert_eq!(counts.datagrams, 4);
        assert_eq!(
            (counts.window_datagrams, counts.window_bytes),
            (2, 2 * 1400)
        );
    }

    #[test]
    fn untracked_and_duplicate_arrivals_are_not_received() {
        let counts = received(&[(0, true), (256, true), (0, true)]);
        assert_eq!(
            (counts.datagrams, counts.untracked, counts.duplicates),
            (1, 1, 1)
        );
        assert_eq!(counts.window_datagrams, 1);
        assert_eq!(counts.last_us, Some(1000));
    }

    #[test]
    fn echoes_and_foreign_datagrams_are_no_load() {
        let mut payload = [0xa5; 64];
        let load = Header {
            run: 7,
            sequence: 3,
            sent_us: 9,
            echo_request: false,
            echo_reply: false,
        };
        load.write(&mut payload).unwrap();
        assert_eq!(classify(&payload, 7), Datagram::Load(load));
        assert_eq!(classify(&payload, 8), Datagram::Foreign);
        assert_eq!(classify(&payload[..LENGTH - 1], 7), Datagram::Foreign);
        let probe = Header {
            echo_request: true,
            ..load
        };
        probe.write(&mut payload).unwrap();
        assert_eq!(classify(&payload, 7), Datagram::Echo(probe));
        assert_eq!(
            classify(&payload, 8),
            Datagram::Echo(probe),
            "an echo of any run"
        );
        assert_eq!(
            echo(probe),
            Header {
                echo_reply: true,
                ..load
            }
        );

        let tally = Tally::<4>::new();
        let mut receiving = Receiving::default();
        receiving.foreign();
        receiving.echo(true);
        receiving.echo(false);
        let counts = receiving.received(&tally);
        assert_eq!(
            (counts.foreign, counts.echoed, counts.echo_refused),
            (1, 1, 1)
        );
        assert_eq!(
            (counts.datagrams, counts.highest, counts.lost),
            (0, None, 0)
        );
    }
}
