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

    #[test]
    fn finish_joins_a_sender_blocked_in_its_bounded_send() {
        let load = Load::start(
            || {
                // A send held to its write timeout.
                std::thread::sleep(Duration::from_millis(50));
                Err(io::Error::from(io::ErrorKind::WouldBlock))
            },
            || "1.0 MBit/s".to_owned(),
            Duration::from_millis(10),
        );
        std::thread::sleep(Duration::from_millis(120));
        let started = Instant::now();
        let report = load.finish();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(report.refused >= 1);
        assert_eq!(report.datagrams, 0);
        assert!(!report.samples.is_empty());
    }

    #[test]
    fn a_long_sample_period_does_not_delay_the_stop() {
        let load = Load::start(
            || Ok(()),
            || "54.0 MBit/s".to_owned(),
            Duration::from_secs(3600),
        );
        std::thread::sleep(Duration::from_millis(30));
        let started = Instant::now();
        let report = load.finish();
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(report.samples, ["54.0 MBit/s"]);
        assert!(report.datagrams > 0);
    }

    #[test]
    fn dropping_the_load_stops_and_joins_both_threads() {
        let calls = Arc::new(AtomicU64::new(0));
        {
            let calls = calls.clone();
            let _load = Load::start(
                move || {
                    calls.fetch_add(1, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(2));
                    Ok(())
                },
                String::new,
                Duration::from_millis(5),
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        // The drop joined the sender: no call follows it.
        let after_drop = calls.load(Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(30));
        assert!(after_drop > 0);
        assert_eq!(calls.load(Ordering::Relaxed), after_drop);
    }
}
