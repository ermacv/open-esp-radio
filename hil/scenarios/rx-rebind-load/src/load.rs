//! The UDP sender and the transmit-bitrate sampler. Every operation either
//! thread performs is bounded, and stopping joins both: no exit path leaves
//! the load running, and `halt` always returns.

use std::{
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

/// How often the stop flag is seen while the sampler waits.
const STOP_POLL: Duration = Duration::from_millis(20);

pub struct Load {
    stop: Arc<AtomicBool>,
    sent: Arc<AtomicU64>,
    refused: Arc<AtomicU64>,
    started: Instant,
    sender: Option<JoinHandle<()>>,
    sampler: Option<JoinHandle<Vec<String>>>,
}

#[derive(Debug)]
pub struct LoadReport {
    pub seconds: f64,
    pub datagrams: u64,
    pub refused: u64,
    pub samples: Vec<String>,
}

impl Load {
    /// Start `send`, called until the load stops, and `sample`, called
    /// every `period`. Each call must itself be bounded: a send with a write
    /// timeout, a sample with a deadline.
    pub fn start(
        mut send: impl FnMut() -> io::Result<()> + Send + 'static,
        mut sample: impl FnMut() -> String + Send + 'static,
        period: Duration,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let sent = Arc::new(AtomicU64::new(0));
        let refused = Arc::new(AtomicU64::new(0));
        let sender = {
            let (stop, sent, refused) = (stop.clone(), sent.clone(), refused.clone());
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match send() {
                        Ok(()) => sent.fetch_add(1, Ordering::Relaxed),
                        Err(_) => refused.fetch_add(1, Ordering::Relaxed),
                    };
                }
            })
        };
        let sampler = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut samples = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    samples.push(sample());
                    let next = Instant::now() + period;
                    while !stop.load(Ordering::Relaxed) && Instant::now() < next {
                        std::thread::sleep(STOP_POLL.min(period));
                    }
                }
                samples
            })
        };
        Self {
            stop,
            sent,
            refused,
            started: Instant::now(),
            sender: Some(sender),
            sampler: Some(sampler),
        }
    }

    /// Stop both threads and join them; the samples taken.
    fn halt(&mut self) -> Vec<String> {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(sender) = self.sender.take() {
            let _ = sender.join();
        }
        self.sampler
            .take()
            .and_then(|sampler| sampler.join().ok())
            .unwrap_or_default()
    }

    pub fn finish(mut self) -> LoadReport {
        let samples = self.halt();
        LoadReport {
            seconds: self.started.elapsed().as_secs_f64(),
            datagrams: self.sent.load(Ordering::Relaxed),
            refused: self.refused.load(Ordering::Relaxed),
            samples,
        }
    }
}

impl Drop for Load {
    fn drop(&mut self) {
        let _ = self.halt();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// Reports `name` when the closure owning it is dropped: at the end of
    /// the thread that ran it, before that thread's join returns.
    struct Ended(mpsc::Sender<&'static str>, &'static str);

    impl Drop for Ended {
        fn drop(&mut self) {
            let _ = self.0.send(self.1);
        }
    }

    fn ended(receiver: &mpsc::Receiver<&'static str>) -> Vec<&'static str> {
        let mut names: Vec<_> = receiver.try_iter().collect();
        names.sort_unstable();
        names
    }

    /// A sender inside a send that returns only when `release` ends, as a
    /// send held to its write timeout returns, and a sampler that has taken
    /// its first sample.
    fn blocked_load() -> (Load, mpsc::Sender<()>, mpsc::Receiver<&'static str>) {
        let (entered, in_send) = mpsc::channel();
        let (release, released) = mpsc::channel::<()>();
        let (sampled, first_sample) = mpsc::channel();
        let (done, threads) = mpsc::channel();
        let sender_ended = Ended(done.clone(), "sender");
        let sampler_ended = Ended(done, "sampler");
        let load = Load::start(
            move || {
                let _owned = &sender_ended;
                let _ = entered.send(());
                let _ = released.recv();
                Err(io::Error::from(io::ErrorKind::TimedOut))
            },
            move || {
                let _owned = &sampler_ended;
                let _ = sampled.send(());
                "6.0 MBit/s".to_owned()
            },
            Duration::from_secs(3600),
        );
        in_send.recv().unwrap();
        first_sample.recv().unwrap();
        (load, release, threads)
    }

    #[test]
    fn finish_joins_a_sender_held_in_its_bounded_send_and_the_sampler() {
        let (load, release, threads) = blocked_load();
        let finishing = std::thread::spawn(move || load.finish());
        // The held send returns; every later one returns at once.
        drop(release);
        let report = finishing.join().unwrap();
        assert_eq!(ended(&threads), ["sampler", "sender"]);
        assert!(report.refused >= 1);
        assert_eq!(report.datagrams, 0);
        assert_eq!(report.samples, ["6.0 MBit/s"]);
    }

    #[test]
    fn dropping_the_load_joins_both_threads() {
        let (load, release, threads) = blocked_load();
        let dropping = std::thread::spawn(move || drop(load));
        drop(release);
        dropping.join().unwrap();
        assert_eq!(ended(&threads), ["sampler", "sender"]);
    }

    #[test]
    fn a_long_sample_period_does_not_hold_the_stop() {
        let (sent, first_send) = mpsc::channel();
        let (sampled, first_sample) = mpsc::channel();
        let load = Load::start(
            move || {
                let _ = sent.send(());
                Ok(())
            },
            move || {
                let _ = sampled.send(());
                "54.0 MBit/s".to_owned()
            },
            Duration::from_secs(3600),
        );
        first_send.recv().unwrap();
        first_sample.recv().unwrap();
        // The sampler waits out its hour only while the load runs.
        let report = load.finish();
        assert_eq!(report.samples, ["54.0 MBit/s"]);
        assert!(report.datagrams >= 1);
    }
}
