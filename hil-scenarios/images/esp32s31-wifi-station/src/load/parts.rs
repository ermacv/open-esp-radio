//! Named parts of the image's work and their polls, from poll boundaries
//! that only an `oer_trace` build emits.
//!
//! A part is a task or a future inside one; [`Polled`] observes the start
//! and end of each of its polls through `oer_trace::observe!`. Each part has
//! one writer, the context of the executor that polls it; the writer
//! publishes under a sequence number, odd while it writes, as in [`super::cpu`].
//! A poll's time includes its preemption by higher executors and interrupts,
//! so nested parts and the parts of different executors overlap. Counts and
//! times wrap at 2^32; a window takes their differences, which hold for a
//! window shorter than some 71 minutes.
//!
//! The station service reports its own spans (`ServiceSpan` of the Wi-Fi
//! facade) through the driver's observer; each span is a part here, nested
//! in `station`, with the same counter.
//!
//! The longest poll belongs to an epoch: the reader starts a new one at the
//! window's start, and a part's longest counts only for the epoch it was
//! written in. A poll that ends while the reader starts an epoch may be kept
//! for the previous one.

use core::sync::atomic::{AtomicU32, Ordering, fence};

use oer_services_load::{MAX_PART_NAME, MAX_PARTS, PartWindow, Parts};

/// The image's named parts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub(crate) enum Part {
    /// The boot hart's application executor's tasks.
    Console,
    StationWork,
    RtcWatchdog,
    Relay,
    /// The boot hart's radio executor's tasks.
    RadioActor,
    Driver,
    /// Hart 1's executor's task.
    Network,
    /// Inside the station work: the station's state machine and its
    /// requests; inside the network: the IP stack and the load server.
    Station,
    Ip,
    Requests,
    Load,
    /// Inside the station: its service's spans (the Wi-Fi facade's
    /// `observation::Span`).
    Step,
    Pending,
    Sources,
    Received,
    Deliver,
    Queue,
    Terminal,
    Transmit,
    Recycle,
}

impl Part {
    pub(crate) const ALL: [Part; COUNT] = [
        Part::Console,
        Part::StationWork,
        Part::RtcWatchdog,
        Part::Relay,
        Part::RadioActor,
        Part::Driver,
        Part::Network,
        Part::Station,
        Part::Ip,
        Part::Requests,
        Part::Load,
        Part::Step,
        Part::Pending,
        Part::Sources,
        Part::Received,
        Part::Deliver,
        Part::Queue,
        Part::Terminal,
        Part::Transmit,
        Part::Recycle,
    ];

    pub(crate) const fn name(self) -> &'static str {
        match self {
            Part::Console => "console",
            Part::StationWork => "station-work",
            Part::RtcWatchdog => "rtc-watchdog",
            Part::Relay => "relay",
            Part::RadioActor => "radio-actor",
            Part::Driver => "driver",
            Part::Network => "network",
            Part::Station => "station",
            Part::Ip => "ip",
            Part::Requests => "requests",
            Part::Load => "load",
            Part::Step => "step",
            Part::Pending => "pending",
            Part::Sources => "sources",
            Part::Received => "received",
            Part::Deliver => "deliver",
            Part::Queue => "queue",
            Part::Terminal => "terminal",
            Part::Transmit => "transmit",
            Part::Recycle => "recycle",
        }
    }

    /// The part of the station service's span `span`.
    #[cfg(all(target_arch = "riscv32", oer_trace))]
    fn of_span(span: oer_esp32s31_wifi::observation::Span) -> Self {
        use oer_esp32s31_wifi::observation::Span;
        match span {
            Span::Step => Part::Step,
            Span::Pending => Part::Pending,
            Span::Sources => Part::Sources,
            Span::Received => Part::Received,
            Span::Deliver => Part::Deliver,
            Span::Queue => Part::Queue,
            Span::Terminal => Part::Terminal,
            Span::Transmit => Part::Transmit,
            Span::Recycle => Part::Recycle,
        }
    }

    #[cfg(any(oer_trace, test))]
    fn from_raw(raw: u8) -> Option<Self> {
        Self::ALL.into_iter().find(|part| *part as u8 == raw)
    }
}

pub(crate) const COUNT: usize = 20;
const _: () = {
    if COUNT > u8::MAX as usize {
        panic!("every part has a number for a page's start");
    }
    let mut index = 0;
    let mut rest: &[Part] = &Part::ALL;
    while let [part, tail @ ..] = rest {
        let part = *part;
        if part as usize != index {
            panic!("a part's position is its number");
        }
        if part.name().len() > MAX_PART_NAME {
            panic!("a part's name fits the report");
        }
        index += 1;
        rest = tail;
    }
};

/// One poll boundary of a part at `at_micros`, the executors' activity
/// clock.
#[cfg(any(oer_trace, test))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Boundary {
    pub at_micros: u32,
    pub part: Part,
    pub end: bool,
}

#[cfg(any(oer_trace, test))]
impl oer_trace::Event for Boundary {
    const KIND: oer_trace::Kind = oer_trace::Kind::new(oer_trace::Domain::Platform, 41);
    // The executors' activity channel: a part's polls refine its rounds.
    const CHANNEL: oer_trace::Channel = const {
        match oer_trace::Channel::new(oer_trace::Domain::Platform, 7) {
            Ok(channel) => channel,
            Err(_) => panic!("the activity channel is in the platform range"),
        }
    };
    fn encode(&self) -> [u32; 2] {
        [
            self.at_micros,
            u32::from_le_bytes([self.part as u8, u8::from(self.end), 0, 0]),
        ]
    }
    fn decode([at_micros, context]: [u32; 2]) -> Option<Self> {
        let [part, end, 0, 0] = context.to_le_bytes() else {
            return None;
        };
        let end = match end {
            0 => false,
            1 => true,
            _ => return None,
        };
        Some(Self {
            at_micros,
            part: Part::from_raw(part)?,
            end,
        })
    }
}

/// What one part accumulated: wrapping counts and microseconds, and the
/// longest poll of the current epoch. `poll_us` includes the open poll's
/// time so far; `ended_us` holds only the ended polls, as `polls` does.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct Sample {
    pub polls: u32,
    pub poll_us: u32,
    pub ended_us: u32,
    pub longest_us: Option<u32>,
}

/// The part's writer was writing while it was sampled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Concurrent;

/// The polls of one part.
pub(crate) struct PartCounter {
    sequence: AtomicU32,
    polls: AtomicU32,
    poll_us: AtomicU32,
    /// The start of the open poll, and whether one is open.
    start: AtomicU32,
    open: AtomicU32,
    longest: AtomicU32,
    /// The epoch `longest` belongs to.
    epoch: AtomicU32,
}

impl PartCounter {
    pub(crate) const fn new() -> Self {
        Self {
            sequence: AtomicU32::new(0),
            polls: AtomicU32::new(0),
            poll_us: AtomicU32::new(0),
            start: AtomicU32::new(0),
            open: AtomicU32::new(0),
            longest: AtomicU32::new(0),
            epoch: AtomicU32::new(0),
        }
    }

    /// Account the boundary at `at_us` in `epoch`; only the part's executor
    /// calls it. An end without its start adds nothing.
    #[cfg(any(oer_trace, test))]
    pub(crate) fn observe(&self, at_us: u32, end: bool, epoch: u32) {
        let sequence = self.sequence.load(Ordering::Relaxed);
        self.sequence
            .store(sequence.wrapping_add(1), Ordering::Relaxed);
        fence(Ordering::Release);
        if !end {
            self.start.store(at_us, Ordering::Relaxed);
            self.open.store(1, Ordering::Relaxed);
        } else if self.open.load(Ordering::Relaxed) != 0 {
            let span = at_us.wrapping_sub(self.start.load(Ordering::Relaxed));
            bump(&self.polls, 1);
            bump(&self.poll_us, span);
            let longest = if self.epoch.load(Ordering::Relaxed) == epoch {
                self.longest.load(Ordering::Relaxed).max(span)
            } else {
                span
            };
            self.longest.store(longest, Ordering::Relaxed);
            self.epoch.store(epoch, Ordering::Relaxed);
            self.open.store(0, Ordering::Relaxed);
        }
        self.sequence
            .store(sequence.wrapping_add(2), Ordering::Release);
    }

    /// The totals at `now_us` (activity microseconds), the open poll's time
    /// included, and the longest poll of `epoch`.
    pub(crate) fn sample(&self, now_us: u32, epoch: u32) -> Result<Sample, Concurrent> {
        let sequence = self.sequence.load(Ordering::Acquire);
        if sequence & 1 != 0 {
            return Err(Concurrent);
        }
        let polls = self.polls.load(Ordering::Relaxed);
        let poll_us = self.poll_us.load(Ordering::Relaxed);
        let start = self.start.load(Ordering::Relaxed);
        let open = self.open.load(Ordering::Relaxed) != 0;
        let longest = self.longest.load(Ordering::Relaxed);
        let longest_epoch = self.epoch.load(Ordering::Relaxed);
        fence(Ordering::Acquire);
        if self.sequence.load(Ordering::Relaxed) != sequence {
            return Err(Concurrent);
        }
        let open_us = if open { now_us.wrapping_sub(start) } else { 0 };
        Ok(Sample {
            polls,
            poll_us: poll_us.wrapping_add(open_us),
            ended_us: poll_us,
            longest_us: (polls != 0 && longest_epoch == epoch).then_some(longest),
        })
    }
}

#[cfg(any(oer_trace, test))]
fn bump(total: &AtomicU32, by: u32) {
    total.store(
        total.load(Ordering::Relaxed).wrapping_add(by),
        Ordering::Relaxed,
    );
}

/// Every part's window, in the order of its number.
pub(crate) type Windows = heapless::Vec<PartWindow, COUNT>;

/// The windows from the part numbered `from`, as many as one answer holds.
pub(crate) fn page(windows: &Windows, from: u8) -> Parts {
    Parts {
        parts: windows
            .iter()
            .skip(usize::from(from))
            .take(MAX_PARTS)
            .cloned()
            .collect(),
    }
}

/// Every part between the samples `start` and `end` of one window, the
/// longest polls from `end`.
pub(crate) fn window(start: &[Sample; COUNT], end: &[Sample; COUNT]) -> Windows {
    let mut parts = Windows::new();
    for ((part, start), end) in Part::ALL.iter().zip(start).zip(end) {
        // There is room for every part and every name fits: neither push
        // nor name is refused.
        let _room = parts.push(PartWindow {
            name: heapless::String::try_from(part.name()).unwrap_or_default(),
            polls: end.polls.wrapping_sub(start.polls),
            poll_us: end.poll_us.wrapping_sub(start.poll_us),
            ended_poll_us: end.ended_us.wrapping_sub(start.ended_us),
            longest_poll_us: end.longest_us,
        });
    }
    parts
}

/// Every part's counter, and the current epoch of the longest polls.
#[cfg(target_arch = "riscv32")]
pub(crate) static COUNTERS: [PartCounter; COUNT] = [const { PartCounter::new() }; COUNT];
#[cfg(target_arch = "riscv32")]
pub(crate) static EPOCH: AtomicU32 = AtomicU32::new(0);

/// The image's observer: the parts' boundaries, the station service's
/// spans, the network bridge's frames and the PC sampler's samples and
/// their return addresses.
#[cfg(all(target_arch = "riscv32", oer_trace))]
#[derive(Clone, Copy, Default)]
pub struct Observer;

#[cfg(all(target_arch = "riscv32", oer_trace))]
impl Observer {
    fn decoded<E: oer_trace::Event + Copy, D: oer_trace::Event>(event: E) -> Option<D> {
        (E::KIND == D::KIND)
            .then(|| D::decode(event.encode()))
            .flatten()
    }
}

#[cfg(all(target_arch = "riscv32", oer_trace))]
impl oer_trace::Observer for Observer {
    fn event<E: oer_trace::Event + Copy + 'static>(&self, event: E) {
        let epoch = EPOCH.load(Ordering::Relaxed);
        if let Some(boundary) = Self::decoded::<E, Boundary>(event)
            && let Some(counter) = COUNTERS.get(usize::from(boundary.part as u8))
        {
            counter.observe(boundary.at_micros, boundary.end, epoch);
        }
        type SpanEvent = oer_esp32s31_wifi::observation::ServiceSpan;
        if let Some(span) = Self::decoded::<E, SpanEvent>(event)
            && let Some(counter) = COUNTERS.get(usize::from(Part::of_span(span.span) as u8))
        {
            counter.observe(super::serve::activity_now(), span.end, epoch);
        }
        if let Some(frame) = Self::decoded::<E, super::counters::BridgeFrame>(event) {
            super::counters::observe(frame);
        }
        if let Some(sample) = Self::decoded::<E, oer_trace::profile::PcSample>(event) {
            super::sampler::record(sample);
        }
        if let Some(ret) = Self::decoded::<E, oer_trace::profile::ReturnSample>(event) {
            super::sampler::record_return(ret);
        }
    }
}

/// The observer the image gives the station's driver and service.
#[cfg(all(target_arch = "riscv32", oer_trace))]
pub type DriverObserver = Observer;
#[cfg(all(target_arch = "riscv32", not(oer_trace)))]
pub type DriverObserver = oer_trace::Noop;

#[cfg(all(target_arch = "riscv32", oer_trace))]
const OBSERVER: Observer = Observer;
#[cfg(all(target_arch = "riscv32", not(oer_trace)))]
const OBSERVER: oer_trace::Noop = oer_trace::Noop;

/// Report what the network bridge did with one frame.
#[cfg(target_arch = "riscv32")]
#[cfg_attr(
    not(oer_trace),
    allow(
        unused_variables,
        reason = "only a trace build counts the bridge's frames"
    )
)]
pub(crate) fn bridged(outcome: super::counters::Bridged) {
    let observer = OBSERVER;
    oer_trace::observe!(observer, super::counters::BridgeFrame { outcome });
}

/// `future`, observed as part `part`: each poll's start and end.
#[cfg(target_arch = "riscv32")]
pub(crate) struct Polled<F> {
    #[cfg_attr(
        not(oer_trace),
        allow(dead_code, reason = "only a trace build observes the part")
    )]
    pub part: Part,
    pub future: F,
}

#[cfg(target_arch = "riscv32")]
impl<F: core::future::Future + Unpin> core::future::Future for Polled<F> {
    type Output = F::Output;
    fn poll(
        mut self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<F::Output> {
        let observer = OBSERVER;
        oer_trace::observe!(
            observer,
            Boundary {
                at_micros: super::serve::activity_now(),
                part: self.part,
                end: false,
            }
        );
        let poll = core::pin::Pin::new(&mut self.future).poll(cx);
        oer_trace::observe!(
            observer,
            Boundary {
                at_micros: super::serve::activity_now(),
                part: self.part,
                end: true,
            }
        );
        poll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_trace::Event as _;

    #[test]
    fn a_part_counts_its_polls_and_their_time_with_the_open_one() {
        let counter = PartCounter::new();
        assert_eq!(counter.sample(5, 0), Ok(Sample::default()), "nothing seen");
        counter.observe(10, false, 0);
        counter.observe(25, true, 0);
        counter.observe(30, false, 0);
        counter.observe(70, true, 0);
        counter.observe(100, false, 0);
        assert_eq!(
            counter.sample(104, 0),
            Ok(Sample {
                polls: 2,
                poll_us: 15 + 40 + 4,
                ended_us: 15 + 40,
                longest_us: Some(40),
            })
        );
    }

    #[test]
    fn the_longest_poll_counts_only_in_its_epoch() {
        let counter = PartCounter::new();
        counter.observe(0, false, 0);
        counter.observe(500, true, 0);
        assert_eq!(counter.sample(600, 1).unwrap().longest_us, None);
        counter.observe(600, false, 1);
        counter.observe(620, true, 1);
        counter.observe(700, false, 1);
        counter.observe(710, true, 1);
        assert_eq!(counter.sample(800, 1).unwrap().longest_us, Some(20));
    }

    #[test]
    fn an_end_without_its_start_adds_nothing_and_spans_cross_the_wrap() {
        let counter = PartCounter::new();
        counter.observe(10, true, 0);
        assert_eq!(counter.sample(20, 0), Ok(Sample::default()));
        counter.observe(u32::MAX - 4, false, 0);
        counter.observe(5, true, 0);
        assert_eq!(counter.sample(6, 0).unwrap().poll_us, 10);
    }

    #[test]
    fn a_sample_during_a_write_is_refused() {
        let counter = PartCounter::new();
        counter.sequence.store(1, Ordering::Relaxed);
        assert_eq!(counter.sample(0, 0), Err(Concurrent));
    }

    #[test]
    fn pages_cover_every_part_once_in_order() {
        let windows = window(&[Sample::default(); COUNT], &[Sample::default(); COUNT]);
        let first = page(&windows, 0);
        let second = page(&windows, MAX_PARTS as u8);
        assert_eq!(first.parts.len(), MAX_PARTS);
        assert_eq!(second.parts.len(), COUNT - MAX_PARTS);
        let names: Vec<_> = first
            .parts
            .iter()
            .chain(&second.parts)
            .map(|part| part.name.as_str())
            .collect();
        let expected: Vec<_> = Part::ALL.iter().map(|part| part.name()).collect();
        assert_eq!(names, expected);
        assert!(page(&windows, COUNT as u8).parts.is_empty());
    }

    #[test]
    fn a_window_names_every_part_and_takes_differences_across_the_wrap() {
        let mut start = [Sample::default(); COUNT];
        let mut end = [Sample::default(); COUNT];
        start[1] = Sample {
            polls: u32::MAX,
            poll_us: u32::MAX - 9,
            ended_us: u32::MAX - 19,
            longest_us: Some(7),
        };
        end[1] = Sample {
            polls: 4,
            poll_us: 20,
            ended_us: 5,
            longest_us: Some(3),
        };
        let parts = window(&start, &end);
        assert_eq!(parts.len(), COUNT);
        let station_work = &parts[1];
        assert_eq!(station_work.name.as_str(), "station-work");
        assert_eq!(
            (
                station_work.polls,
                station_work.poll_us,
                station_work.ended_poll_us
            ),
            (5, 30, 25),
            "differences across the wrap"
        );
        assert_eq!(station_work.longest_poll_us, Some(3), "from the end");
        assert_eq!(parts[0].longest_poll_us, None);
    }

    #[test]
    fn a_boundary_round_trips_and_refuses_unknown_parts_and_phases() {
        for part in Part::ALL {
            for end in [false, true] {
                let boundary = Boundary {
                    at_micros: u32::MAX,
                    part,
                    end,
                };
                assert_eq!(Boundary::decode(boundary.encode()), Some(boundary));
            }
        }
        assert_eq!(Boundary::decode([0, COUNT as u32]), None);
        assert_eq!(Boundary::decode([0, 2 << 8]), None);
        assert_eq!(Boundary::decode([0, 1 << 16]), None);
    }
}
