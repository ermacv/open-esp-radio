//! One connect's timeline: when each scan visit asked for its channel, moved
//! there, listened and left, what it heard and when, and when the join's
//! stages began. A new connect replaces the last one's.
//!
//! The station service is the one writer; a reader takes each word on its
//! own, so a reading during a connect may be a write apart. Times are
//! microseconds from the connect's start. A visit past [`VISITS`] is counted,
//! not kept.

use core::sync::atomic::{AtomicU32, Ordering};

use super::bump;

/// The visits kept: every channel of 2.4 GHz, with room for cancelled visits.
pub const VISITS: usize = 16;

const STAGES: usize = 8;
/// A time not reached.
const NONE: u32 = u32::MAX;

/// A step of a scan visit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStep {
    Ask,
    Tune,
    Configure,
    Probe,
    Listen,
}

/// A stage of the join after the scan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Stage {
    Ask,
    Tune,
    Configure,
    Authenticate,
    Associate,
    StartLink,
    Handshake,
    Connected,
}

/// Where a connect is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Step {
    Rest,
    Scan(ScanStep),
    Join(Stage),
}

impl Step {
    const fn code(self) -> u32 {
        match self {
            Self::Rest => 0,
            Self::Scan(step) => 1 + step as u32,
            Self::Join(stage) => 16 + stage as u32,
        }
    }
}

/// One frame a scan heard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Heard {
    /// It names the connect's network.
    pub own: bool,
    pub probe_response: bool,
    /// Addressed to the station, not overheard.
    pub to_us: bool,
    pub rssi: Option<i8>,
}

/// What became of a visit's probe. Neither a sent probe nor an answer says
/// what reached the air: the first is the port's report, the second a frame
/// that reached the scan.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ProbeState {
    /// None yet: the visit has not handed one over.
    #[default]
    None,
    /// Handed to the port.
    Built,
    /// The port reports it did not go out.
    Unsent,
    /// The port reports it went out.
    Sent,
    /// A probe response addressed to the station arrived.
    Answered,
    /// The visit ended with no answer.
    TimedOut,
}

/// One channel visit. Times are from the connect's start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Visit {
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
    /// The strongest frame of the connect's network.
    pub best_own_rssi: Option<i8>,
}

/// When the join's stages began, from the connect's start.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Stages {
    pub ask: Option<u32>,
    pub tune: Option<u32>,
    pub configure: Option<u32>,
    pub authenticate: Option<u32>,
    pub associate: Option<u32>,
    pub start_link: Option<u32>,
    pub handshake: Option<u32>,
    pub connected: Option<u32>,
}

/// The connect as a whole.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Connect {
    /// Visits kept.
    pub visits: u8,
    /// Visits past [`VISITS`].
    pub lost_visits: u32,
    /// When the connect failed or left.
    pub ended: Option<u32>,
    pub stages: Stages,
}

const fn time(word: u32) -> Option<u32> {
    match word {
        NONE => None,
        word => Some(word),
    }
}

struct VisitCells {
    channel: AtomicU32,
    /// `Ask`, `Tune`, `Configure`, `Probe` and `Listen`, by [`ScanStep`].
    steps: [AtomicU32; 5],
    ended: AtomicU32,
    first_heard: AtomicU32,
    first_own: AtomicU32,
    heard: AtomicU32,
    own_beacons: AtomicU32,
    own_probe_responses: AtomicU32,
    responses_to_us: AtomicU32,
    responses_overheard: AtomicU32,
    probe_ended: AtomicU32,
    /// 1 when the port said the probe went out, 2 when it did not.
    probe_sent: AtomicU32,
    /// The level as its `u8` bits, [`NONE`] before one.
    best_own_rssi: AtomicU32,
}

impl VisitCells {
    const fn new() -> Self {
        Self {
            channel: AtomicU32::new(0),
            steps: [const { AtomicU32::new(NONE) }; 5],
            ended: AtomicU32::new(NONE),
            first_heard: AtomicU32::new(NONE),
            first_own: AtomicU32::new(NONE),
            heard: AtomicU32::new(0),
            own_beacons: AtomicU32::new(0),
            own_probe_responses: AtomicU32::new(0),
            responses_to_us: AtomicU32::new(0),
            responses_overheard: AtomicU32::new(0),
            probe_ended: AtomicU32::new(NONE),
            probe_sent: AtomicU32::new(0),
            best_own_rssi: AtomicU32::new(NONE),
        }
    }

    fn clear(&self) {
        for cell in [
            &self.ended,
            &self.first_heard,
            &self.first_own,
            &self.probe_ended,
            &self.best_own_rssi,
        ]
        .into_iter()
        .chain(&self.steps)
        {
            cell.store(NONE, Ordering::Relaxed);
        }
        for cell in [
            &self.channel,
            &self.heard,
            &self.own_beacons,
            &self.own_probe_responses,
            &self.responses_to_us,
            &self.responses_overheard,
            &self.probe_sent,
        ] {
            cell.store(0, Ordering::Relaxed);
        }
    }

    fn step(&self, step: ScanStep) -> Option<u32> {
        time(self.steps.get(step as usize)?.load(Ordering::Relaxed))
    }

    fn probe_state(&self) -> ProbeState {
        if self.responses_to_us.load(Ordering::Relaxed) != 0 {
            return ProbeState::Answered;
        }
        let ended = time(self.ended.load(Ordering::Relaxed)).is_some();
        match self.probe_sent.load(Ordering::Relaxed) {
            2 => ProbeState::Unsent,
            _ if ended && self.step(ScanStep::Listen).is_some() => ProbeState::TimedOut,
            1 => ProbeState::Sent,
            _ if self.step(ScanStep::Listen).is_some() => ProbeState::Built,
            _ => ProbeState::None,
        }
    }

    fn read(&self) -> Visit {
        let load = |cell: &AtomicU32| time(cell.load(Ordering::Relaxed));
        Visit {
            channel: self.channel.load(Ordering::Relaxed).to_le_bytes()[0],
            asked: self.step(ScanStep::Ask),
            granted: self.step(ScanStep::Tune),
            tuned: self.step(ScanStep::Configure),
            configured: self.step(ScanStep::Probe),
            listening: self.step(ScanStep::Listen),
            ended: load(&self.ended),
            first_heard: load(&self.first_heard),
            first_own: load(&self.first_own),
            heard: self.heard.load(Ordering::Relaxed),
            own_beacons: self.own_beacons.load(Ordering::Relaxed),
            own_probe_responses: self.own_probe_responses.load(Ordering::Relaxed),
            responses_to_us: self.responses_to_us.load(Ordering::Relaxed),
            responses_overheard: self.responses_overheard.load(Ordering::Relaxed),
            probe_ended: load(&self.probe_ended),
            probe: self.probe_state(),
            best_own_rssi: load(&self.best_own_rssi)
                .map(|bits| i8::from_le_bytes([bits.to_le_bytes()[0]])),
        }
    }
}

/// Store `value` unless the cell holds a time already.
fn first(cell: &AtomicU32, value: u32) {
    if cell.load(Ordering::Relaxed) == NONE {
        cell.store(value, Ordering::Relaxed);
    }
}

/// The connect's words.
pub struct ConnectCounter {
    /// The microsecond the connect started.
    start: AtomicU32,
    /// The last step and channel, to keep changes only.
    last: AtomicU32,
    /// 1 between connects.
    resting: AtomicU32,
    /// Visits begun, [`VISITS`] and over counted in `lost`.
    visits: AtomicU32,
    /// 1 while a visit's listening has not ended.
    open: AtomicU32,
    lost: AtomicU32,
    ended: AtomicU32,
    cells: [VisitCells; VISITS],
    stages: [AtomicU32; STAGES],
}

impl ConnectCounter {
    pub const fn new() -> Self {
        Self {
            start: AtomicU32::new(0),
            last: AtomicU32::new(NONE),
            resting: AtomicU32::new(1),
            visits: AtomicU32::new(0),
            open: AtomicU32::new(0),
            lost: AtomicU32::new(0),
            ended: AtomicU32::new(NONE),
            cells: [const { VisitCells::new() }; VISITS],
            stages: [const { AtomicU32::new(NONE) }; STAGES],
        }
    }

    fn since(&self, at: u32) -> u32 {
        at.wrapping_sub(self.start.load(Ordering::Relaxed))
    }

    /// The visit in progress, kept or not.
    fn current(&self) -> Option<&VisitCells> {
        if self.open.load(Ordering::Relaxed) == 0 {
            return None;
        }
        let visits = self.visits.load(Ordering::Relaxed) as usize;
        self.cells.get(visits.checked_sub(1)?)
    }

    fn begin(&self, at: u32) {
        self.start.store(at, Ordering::Relaxed);
        self.resting.store(0, Ordering::Relaxed);
        self.visits.store(0, Ordering::Relaxed);
        self.open.store(0, Ordering::Relaxed);
        self.lost.store(0, Ordering::Relaxed);
        self.ended.store(NONE, Ordering::Relaxed);
        for cell in &self.stages {
            cell.store(NONE, Ordering::Relaxed);
        }
        for cell in &self.cells {
            cell.clear();
        }
    }

    /// End the visit in progress.
    fn close(&self, at: u32) {
        if let Some(cell) = self.current() {
            first(&cell.ended, self.since(at));
        }
        self.open.store(0, Ordering::Relaxed);
    }

    /// The next visit, on `channel`.
    fn open_visit(&self, channel: Option<u8>) {
        let visits = self.visits.load(Ordering::Relaxed) as usize;
        match self.cells.get(visits) {
            Some(cell) => {
                cell.channel
                    .store(u32::from(channel.unwrap_or(0)), Ordering::Relaxed);
                bump(&self.visits, 1);
                self.open.store(1, Ordering::Relaxed);
            }
            None => bump(&self.lost, 1),
        }
    }

    /// The connect is at `step`, on `channel`, at microsecond `at`.
    pub fn step(&self, at: u32, step: Step, channel: Option<u8>) {
        let word = step.code() | u32::from(channel.unwrap_or(0)) << 8;
        if self.last.load(Ordering::Relaxed) == word {
            return;
        }
        self.last.store(word, Ordering::Relaxed);
        if step == Step::Rest {
            self.close(at);
            if self.resting.load(Ordering::Relaxed) == 0 {
                self.resting.store(1, Ordering::Relaxed);
                self.ended.store(self.since(at), Ordering::Relaxed);
            }
            return;
        }
        if self.resting.load(Ordering::Relaxed) != 0 {
            self.begin(at);
        }
        match step {
            Step::Rest => {}
            Step::Scan(scan) => {
                if scan == ScanStep::Ask {
                    self.close(at);
                    self.open_visit(channel);
                }
                if let Some(cell) = self
                    .current()
                    .and_then(|cell| cell.steps.get(scan as usize))
                {
                    first(cell, self.since(at));
                }
            }
            Step::Join(stage) => {
                self.close(at);
                if let Some(cell) = self.stages.get(stage as usize) {
                    first(cell, self.since(at));
                }
                if stage == Stage::Connected {
                    self.resting.store(1, Ordering::Relaxed);
                }
            }
        }
    }

    /// The port ended the visit's probe at microsecond `at`.
    pub fn probe_ended(&self, at: u32, sent: bool) {
        let Some(cell) = self.current() else {
            return;
        };
        first(&cell.probe_ended, self.since(at));
        cell.probe_sent
            .store(if sent { 1 } else { 2 }, Ordering::Relaxed);
    }

    /// The scan heard `heard` at microsecond `at`.
    pub fn heard(&self, at: u32, heard: Heard) {
        let Some(cell) = self.current() else {
            return;
        };
        let since = self.since(at);
        first(&cell.first_heard, since);
        bump(&cell.heard, 1);
        if heard.probe_response {
            bump(
                if heard.to_us {
                    &cell.responses_to_us
                } else {
                    &cell.responses_overheard
                },
                1,
            );
        }
        if !heard.own {
            return;
        }
        first(&cell.first_own, since);
        bump(
            if heard.probe_response {
                &cell.own_probe_responses
            } else {
                &cell.own_beacons
            },
            1,
        );
        if let Some(rssi) = heard.rssi {
            let best = time(cell.best_own_rssi.load(Ordering::Relaxed))
                .map(|bits| i8::from_le_bytes([bits.to_le_bytes()[0]]));
            if best.is_none_or(|best| rssi > best) {
                cell.best_own_rssi
                    .store(u32::from(rssi.to_le_bytes()[0]), Ordering::Relaxed);
            }
        }
    }

    pub fn read(&self) -> Connect {
        let stage = |stage: Stage| {
            self.stages
                .get(stage as usize)
                .and_then(|cell| time(cell.load(Ordering::Relaxed)))
        };
        Connect {
            visits: self.visits.load(Ordering::Relaxed).to_le_bytes()[0],
            lost_visits: self.lost.load(Ordering::Relaxed),
            ended: time(self.ended.load(Ordering::Relaxed)),
            stages: Stages {
                ask: stage(Stage::Ask),
                tune: stage(Stage::Tune),
                configure: stage(Stage::Configure),
                authenticate: stage(Stage::Authenticate),
                associate: stage(Stage::Associate),
                start_link: stage(Stage::StartLink),
                handshake: stage(Stage::Handshake),
                connected: stage(Stage::Connected),
            },
        }
    }

    /// Visit `index`, if it began.
    pub fn visit(&self, index: usize) -> Option<Visit> {
        (index < self.visits.load(Ordering::Relaxed) as usize)
            .then(|| self.cells.get(index).map(VisitCells::read))
            .flatten()
    }
}

impl Default for ConnectCounter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWN_BEACON: Heard = Heard {
        own: true,
        probe_response: false,
        to_us: false,
        rssi: Some(-40),
    };

    fn scan(counter: &ConnectCounter, start: u32, channel: u8) {
        for (offset, step) in [
            ScanStep::Ask,
            ScanStep::Tune,
            ScanStep::Configure,
            ScanStep::Probe,
            ScanStep::Listen,
        ]
        .into_iter()
        .enumerate()
        {
            counter.step(
                start.wrapping_add(offset as u32),
                Step::Scan(step),
                Some(channel),
            );
        }
    }

    #[test]
    fn a_visit_keeps_its_steps_from_the_connects_start_and_ends_at_the_next_ask() {
        let counter = ConnectCounter::new();
        counter.step(900, Step::Rest, None);
        scan(&counter, 1000, 1);
        counter.heard(1100, OWN_BEACON);
        scan(&counter, 2000, 2);
        assert_eq!(counter.read().visits, 2);
        let first = counter.visit(0).unwrap();
        assert_eq!(first.channel, 1);
        assert_eq!(first.asked, Some(0));
        assert_eq!(first.granted, Some(1));
        assert_eq!(first.tuned, Some(2));
        assert_eq!(first.configured, Some(3));
        assert_eq!(first.listening, Some(4));
        assert_eq!(first.first_heard, Some(100));
        assert_eq!(first.ended, Some(1000));
        assert_eq!(counter.visit(1).unwrap().channel, 2);
        assert_eq!(counter.visit(2), None);
    }

    #[test]
    fn a_repeated_step_changes_nothing() {
        let counter = ConnectCounter::new();
        scan(&counter, 1000, 6);
        counter.step(1500, Step::Scan(ScanStep::Listen), Some(6));
        assert_eq!(counter.visit(0).unwrap().listening, Some(4));
    }

    #[test]
    fn heard_frames_split_by_network_and_kind_and_keep_the_strongest() {
        let counter = ConnectCounter::new();
        scan(&counter, 0, 13);
        counter.heard(
            10,
            Heard {
                own: false,
                probe_response: false,
                to_us: false,
                rssi: Some(-20),
            },
        );
        counter.heard(20, OWN_BEACON);
        counter.heard(
            30,
            Heard {
                own: true,
                probe_response: true,
                to_us: true,
                rssi: Some(-30),
            },
        );
        counter.heard(40, OWN_BEACON);
        let visit = counter.visit(0).unwrap();
        assert_eq!(visit.heard, 4);
        assert_eq!(visit.first_heard, Some(10));
        assert_eq!(visit.first_own, Some(20));
        assert_eq!(visit.own_beacons, 2);
        assert_eq!(visit.own_probe_responses, 1);
        assert_eq!(visit.best_own_rssi, Some(-30));
    }

    #[test]
    fn a_failed_scan_ends_at_rest_and_the_next_connect_starts_over() {
        let counter = ConnectCounter::new();
        scan(&counter, 1000, 1);
        counter.step(1250, Step::Rest, None);
        assert_eq!(counter.read().ended, Some(250));
        assert_eq!(counter.visit(0).unwrap().ended, Some(250));
        scan(&counter, 5000, 2);
        let connect = counter.read();
        assert_eq!(connect.visits, 1);
        assert_eq!(connect.ended, None);
        assert_eq!(counter.visit(0).unwrap().channel, 2);
        assert_eq!(counter.visit(0).unwrap().asked, Some(0));
    }

    #[test]
    fn the_join_keeps_its_stages_and_a_later_connect_replaces_them() {
        let counter = ConnectCounter::new();
        scan(&counter, 1000, 13);
        for (at, stage) in [
            (2000, Stage::Ask),
            (2100, Stage::Authenticate),
            (2200, Stage::Associate),
            (2300, Stage::Handshake),
            (2400, Stage::Connected),
        ] {
            counter.step(at, Step::Join(stage), Some(13));
        }
        let stages = counter.read().stages;
        assert_eq!(stages.ask, Some(1000));
        assert_eq!(stages.connected, Some(1400));
        assert_eq!(counter.visit(0).unwrap().ended, Some(1000));
        // A step repeated at the link does not begin another connect.
        counter.step(9000, Step::Join(Stage::Connected), Some(13));
        assert_eq!(counter.read().stages.connected, Some(1400));
        counter.step(9100, Step::Rest, None);
        scan(&counter, 9200, 1);
        assert_eq!(counter.read().stages.connected, None);
    }

    #[test]
    fn visits_past_the_table_are_counted_and_times_survive_the_clocks_wrap() {
        let counter = ConnectCounter::new();
        let start = u32::MAX - 500;
        for visit in 0..VISITS as u32 + 2 {
            scan(
                &counter,
                start.wrapping_add(visit * 100),
                (visit % 13) as u8 + 1,
            );
        }
        let connect = counter.read();
        assert_eq!(usize::from(connect.visits), VISITS);
        assert_eq!(connect.lost_visits, 2);
        assert_eq!(counter.visit(15).unwrap().asked, Some(1500));
    }

    #[test]
    fn a_probe_goes_from_built_to_answered_or_timed_out() {
        let counter = ConnectCounter::new();
        scan(&counter, 0, 13);
        assert_eq!(counter.visit(0).unwrap().probe, ProbeState::Built);
        counter.probe_ended(10, true);
        let visit = counter.visit(0).unwrap();
        assert_eq!(visit.probe, ProbeState::Sent);
        assert_eq!(visit.probe_ended, Some(10));
        counter.heard(
            20,
            Heard {
                own: false,
                probe_response: true,
                to_us: false,
                rssi: None,
            },
        );
        let visit = counter.visit(0).unwrap();
        assert_eq!(visit.responses_overheard, 1);
        assert_eq!(
            visit.probe,
            ProbeState::Sent,
            "an overheard response is no answer"
        );
        // The next visit ends this one with no answer.
        scan(&counter, 500, 1);
        assert_eq!(counter.visit(0).unwrap().probe, ProbeState::TimedOut);
        counter.probe_ended(510, true);
        counter.heard(
            520,
            Heard {
                own: true,
                probe_response: true,
                to_us: true,
                rssi: None,
            },
        );
        let visit = counter.visit(1).unwrap();
        assert_eq!(visit.responses_to_us, 1);
        assert_eq!(visit.probe, ProbeState::Answered);
    }

    #[test]
    fn a_probe_the_port_refused_stays_unsent() {
        let counter = ConnectCounter::new();
        scan(&counter, 0, 13);
        counter.probe_ended(10, false);
        scan(&counter, 500, 1);
        assert_eq!(counter.visit(0).unwrap().probe, ProbeState::Unsent);
    }
}
