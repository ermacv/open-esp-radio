//! The last connect's timeline, as the image observed it: the whole of it
//! ([`GetConnectTimes`]) and its scan's channel visits one by one
//! ([`GetScanVisit`]), since one answer does not hold them all. Times are
//! microseconds from the connect's start, `None` when not reached.

use core::fmt;

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Ask for the last connect as a whole.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetConnectTimes;

/// The answer to [`GetConnectTimes`]: when each stage after the scan began.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ConnectTimes {
    /// Scan visits kept, for [`GetScanVisit`].
    pub visits: u8,
    /// Visits past the image's table.
    pub lost_visits: u32,
    /// The connect failed or left.
    pub ended: Option<u32>,
    pub ask: Option<u32>,
    pub tune: Option<u32>,
    pub configure: Option<u32>,
    pub authenticate: Option<u32>,
    pub associate: Option<u32>,
    pub start_link: Option<u32>,
    pub handshake: Option<u32>,
    pub connected: Option<u32>,
}

/// Ask for one visit of the last connect's scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetScanVisit {
    pub index: u8,
}

/// The answer to [`GetScanVisit`]: `None` past the last visit.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ScanVisitAnswer(pub Option<ScanVisit>);

/// What became of a visit's probe. `Sent` is the port's report and
/// `Answered` a frame that reached the scan; neither observes the air.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum ProbeState {
    #[default]
    None,
    Built,
    Unsent,
    Sent,
    Answered,
    TimedOut,
}

impl ProbeState {
    const fn text(self) -> &'static str {
        match self {
            Self::None => "-",
            Self::Built => "built",
            Self::Unsent => "unsent",
            Self::Sent => "sent",
            Self::Answered => "answer",
            Self::TimedOut => "timeout",
        }
    }
}

/// One channel visit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ScanVisit {
    pub channel: u8,
    pub asked: Option<u32>,
    /// The lease was granted.
    pub granted: Option<u32>,
    /// The port reached the channel.
    pub tuned: Option<u32>,
    /// The receive rules were in place.
    pub configured: Option<u32>,
    /// The probe was handed over.
    pub listening: Option<u32>,
    pub ended: Option<u32>,
    /// The first beacon or probe response of any network.
    pub first_heard: Option<u32>,
    /// The first of the connect's network.
    pub first_own: Option<u32>,
    pub heard: u32,
    pub own_beacons: u32,
    pub own_probe_responses: u32,
    /// Probe responses of any network addressed to the station.
    pub responses_to_us: u32,
    /// Probe responses overheard, addressed to another.
    pub responses_overheard: u32,
    /// The port ended the probe.
    pub probe_ended: Option<u32>,
    pub probe: ProbeState,
    pub best_own_rssi: Option<i8>,
}

/// `to` after `from`.
fn between(from: Option<u32>, to: Option<u32>) -> Option<u32> {
    to?.checked_sub(from?)
}

/// Microseconds as milliseconds, right-aligned in `width`, or a dash.
fn ms(f: &mut fmt::Formatter<'_>, width: usize, micros: Option<u32>) -> fmt::Result {
    match micros {
        Some(micros) => {
            let tenths = micros / 100;
            let (whole, tenth) = (tenths / 10, tenths % 10);
            let pad = width.saturating_sub(digits(whole).saturating_add(2));
            write!(f, "{:pad$}{whole}.{tenth}", "")
        }
        None => write!(f, "{:>width$}", "-"),
    }
}

fn digits(value: u32) -> usize {
    match value.checked_ilog10() {
        Some(log) => (log as usize).saturating_add(1),
        None => 1,
    }
}

impl ScanVisit {
    /// The columns [`ScanVisit`]'s `Display` fills, in milliseconds: from
    /// asking to the grant, to the port on the channel, to the receive rules,
    /// to the probe handed over, listening, the whole time on the channel,
    /// and from reaching the channel to the first frame and the first of the
    /// connect's network, and to the port's end of the probe. Then the frames
    /// heard, the network's beacons and probe responses, its strongest level,
    /// the probe responses addressed to the station and those overheard, and
    /// the probe's state.
    pub const HEADER: &'static str = "ch    ask   tune config  probe listen onchan  first    own  pdone frames beacon  resp  dBm   us over   probe";
}

impl fmt::Display for ScanVisit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:2}", self.channel)?;
        for micros in [
            between(self.asked, self.granted),
            between(self.granted, self.tuned),
            between(self.tuned, self.configured),
            between(self.configured, self.listening),
            between(self.listening, self.ended),
            between(self.tuned, self.ended),
            between(self.tuned, self.first_heard),
            between(self.tuned, self.first_own),
            between(self.tuned, self.probe_ended),
        ] {
            f.write_str(" ")?;
            ms(f, 6, micros)?;
        }
        write!(
            f,
            " {:6} {:6} {:5}",
            self.heard, self.own_beacons, self.own_probe_responses
        )?;
        match self.best_own_rssi {
            Some(rssi) => write!(f, " {rssi:4}")?,
            None => f.write_str("    -")?,
        }
        write!(
            f,
            " {:4} {:4} {:>7}",
            self.responses_to_us,
            self.responses_overheard,
            self.probe.text()
        )
    }
}

impl fmt::Display for ConnectTimes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "visits {} lost {}", self.visits, self.lost_visits)?;
        for (name, micros) in [
            ("scan-end", self.ask.or(self.ended)),
            ("tune", self.tune),
            ("configure", self.configure),
            ("authenticate", self.authenticate),
            ("associate", self.associate),
            ("start-link", self.start_link),
            ("handshake", self.handshake),
            ("connected", self.connected),
            ("ended", self.ended),
        ] {
            write!(f, " | {name} ")?;
            ms(f, 0, micros)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;
    use std::format;

    fn visit() -> ScanVisit {
        ScanVisit {
            channel: 13,
            asked: Some(1_000),
            granted: Some(1_500),
            tuned: Some(3_000),
            configured: Some(3_200),
            listening: Some(3_300),
            ended: Some(253_300),
            first_heard: Some(10_000),
            first_own: None,
            heard: 4,
            own_beacons: 0,
            own_probe_responses: 0,
            best_own_rssi: None,
            ..ScanVisit::default()
        }
    }

    #[test]
    fn a_visit_row_holds_each_span_under_its_column() {
        let row = format!("{}", visit());
        assert_eq!(row.len(), ScanVisit::HEADER.len());
        let columns: std::vec::Vec<_> = row.split_whitespace().collect();
        assert_eq!(
            columns,
            [
                "13", "0.5", "1.5", "0.2", "0.1", "250.0", "250.3", "7.0", "-", "-", "4", "0", "0",
                "-", "0", "0", "-"
            ]
        );
    }

    #[test]
    fn a_visit_that_never_tuned_shows_dashes() {
        let row = format!(
            "{}",
            ScanVisit {
                channel: 1,
                ..ScanVisit::default()
            }
        );
        assert_eq!(row.len(), ScanVisit::HEADER.len());
    }

    #[test]
    fn a_visit_row_names_the_probes_state() {
        for (state, text) in [
            (ProbeState::Built, "built"),
            (ProbeState::Unsent, "unsent"),
            (ProbeState::Sent, "sent"),
            (ProbeState::Answered, "answer"),
            (ProbeState::TimedOut, "timeout"),
        ] {
            let row = format!(
                "{}",
                ScanVisit {
                    probe: state,
                    responses_to_us: 2,
                    responses_overheard: 3,
                    ..visit()
                }
            );
            assert_eq!(row.len(), ScanVisit::HEADER.len());
            let columns: std::vec::Vec<_> = row.split_whitespace().rev().take(3).collect();
            assert_eq!(columns, [text, "3", "2"]);
        }
    }

    #[test]
    fn a_failed_connect_ends_its_scan_at_its_end() {
        let times = ConnectTimes {
            visits: 13,
            ended: Some(9_269_000),
            ..ConnectTimes::default()
        };
        let line = format!("{times}");
        assert!(line.contains("scan-end 9269.0"), "{line}");
        assert!(line.contains("connected -"), "{line}");
    }

    #[test]
    fn the_longest_answers_fit_one_message() {
        let full = ScanVisit {
            channel: 13,
            asked: Some(u32::MAX - 1),
            granted: Some(u32::MAX - 1),
            tuned: Some(u32::MAX - 1),
            configured: Some(u32::MAX - 1),
            listening: Some(u32::MAX - 1),
            ended: Some(u32::MAX - 1),
            first_heard: Some(u32::MAX - 1),
            first_own: Some(u32::MAX - 1),
            heard: u32::MAX,
            own_beacons: u32::MAX,
            own_probe_responses: u32::MAX,
            responses_to_us: u32::MAX,
            responses_overheard: u32::MAX,
            probe_ended: Some(u32::MAX - 1),
            probe: ProbeState::TimedOut,
            best_own_rssi: Some(-128),
        };
        let mut buffer = [0; 256];
        let encoded = postcard::to_slice(&ScanVisitAnswer(Some(full)), &mut buffer).unwrap();
        assert!(
            encoded.len() <= oer_protocols_hil_protocol::MAX_POSTCARD_BYTES,
            "{} bytes",
            encoded.len()
        );
    }
}
