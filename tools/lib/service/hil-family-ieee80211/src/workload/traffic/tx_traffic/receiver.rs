//! Event-driven UDP collection, bounded by a correlated target result.

use super::ActiveBurst;
use crate::Result;
use oer_hil_link::transport::events::{EventPoll, deadline_after};
use oer_hil_run_bundle_format::run::fixtures::{self, Burst, Reception, ReceptionEnd as End};
use rustix::net::{RecvFlags, recvfrom};
use std::{
    fs, io,
    net::{Ipv4Addr, SocketAddrV4, UdpSocket},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Only missing packets wait this long after Finished. Complete delivery exits
/// immediately; this deadline never attests stack or radio drain completion.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy)]
enum Stop {
    Finished {
        expected: Option<u64>,
        deadline: Instant,
    },
    Aborted,
}

pub struct Receiver {
    stop: Arc<Mutex<Option<Stop>>>,
    wake: Arc<mio::Waker>,
    worker: Option<thread::JoinHandle<Result<Vec<Burst>>>>,
    progress: std::sync::mpsc::Receiver<u64>,
}

impl Receiver {
    pub fn start(
        socket: &UdpSocket,
        target: Ipv4Addr,
        maximum_wait: Duration,
        output: &Path,
        label: &str,
    ) -> Result<Self> {
        fs::create_dir_all(output)?;
        let (directory, label) = (output.to_owned(), label.to_owned());
        let socket = socket.try_clone()?;
        let poll = EventPoll::new()?;
        poll.register(&socket, false)?;
        let wake = poll.waker();
        let stop = Arc::new(Mutex::new(None));
        let worker_stop = Arc::clone(&stop);
        let deadline = deadline_after(maximum_wait);
        let drops_before = oer_hil_net_traffic::udp::kernel_drops(&socket)?;
        let (progress_tx, progress) = std::sync::mpsc::sync_channel(1);
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(0);
        let worker = thread::spawn(move || {
            let _ = ready_tx.send(());
            collect(
                socket,
                target,
                deadline,
                worker_stop,
                poll,
                ReceptionOutput {
                    directory,
                    label,
                    drops_before,
                },
                Some(progress_tx),
            )
        });
        ready_rx
            .recv()
            .map_err(|_| "UDP collector stopped before readiness")?;
        Ok(Self {
            stop,
            wake,
            worker: Some(worker),
            progress,
        })
    }

    /// Wait for measured traffic, never an assumed post-Start settling delay.
    pub fn wait_started(&self, timeout: Duration) -> Result<u64> {
        self.progress.recv_timeout(timeout).map_err(|error| {
            format!("UDP did not reach 256 received datagrams before pause: {error}").into()
        })
    }

    /// Publish one terminal edge to all flow collectors before joining any of them.
    pub fn target_finished(&self, expected_datagrams: Option<u64>) {
        let mut stop = self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if stop.is_none() {
            *stop = Some(Stop::Finished {
                expected: expected_datagrams,
                deadline: Instant::now() + DELIVERY_TIMEOUT,
            });
        }
        drop(stop);
        let _ = self.wake.wake();
    }

    pub fn finish(mut self, expected_datagrams: Option<u64>) -> Result<Vec<Burst>> {
        self.target_finished(expected_datagrams);
        self.join()
    }

    fn signal(&self, stop: Stop) {
        *self
            .stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stop);
        // The collector may already have stopped on an I/O error or deadline.
        let _ = self.wake.wake();
    }

    fn join(&mut self) -> Result<Vec<Burst>> {
        self.worker
            .take()
            .expect("collector joined once")
            .join()
            .map_err(|_| "UDP collector panicked")?
    }
}

impl Drop for Receiver {
    fn drop(&mut self) {
        if self.worker.is_some() {
            self.signal(Stop::Aborted);
            if let Err(error) = self.join() {
                eprintln!("UDP collector cleanup: {error}");
            }
        }
    }
}

/// Where the reception record goes: the repetition directory and the
/// record's label.
struct ReceptionOutput {
    directory: PathBuf,
    label: String,
    drops_before: Option<u32>,
}

fn collect(
    socket: UdpSocket,
    target: Ipv4Addr,
    session_deadline: Instant,
    stop: Arc<Mutex<Option<Stop>>>,
    mut poll: EventPoll,
    output: ReceptionOutput,
    mut progress: Option<std::sync::mpsc::SyncSender<u64>>,
) -> Result<Vec<Burst>> {
    let ReceptionOutput {
        directory,
        label,
        drops_before,
    } = output;
    let mut packet = [0_u8; 2048];
    let mut active: Option<ActiveBurst> = None;
    let started = Instant::now();
    let mut expected = None;
    let mut error = None;
    let mut end = 'collect: loop {
        if let Err(cause) = oer_process::check_cancelled() {
            error = Some(cause);
            break End::Cancelled;
        }
        let mut drained = false;
        {
            // Bound CPU residence while preserving edge-triggered readiness:
            // if the budget expires, resume draining without another poll wait.
            for _ in 0..64 {
                match receive(&socket, &mut packet) {
                    Ok((length, source)) if source == target && length >= 4 => {
                        // A retried preflight response may arrive after Start. It is
                        // control traffic and never part of delivery accounting.
                        if oer_hil_protocol::network::UdpProbe::decode(&packet[..length]).is_some()
                        {
                            continue;
                        }
                        let sequence =
                            u32::from_be_bytes(packet[..4].try_into().expect("four-byte sequence"));
                        let now = Instant::now();
                        match &mut active {
                            Some(active) => active.push(sequence, length, now),
                            None => active = Some(ActiveBurst::new(sequence, length, now)),
                        }
                    }
                    Ok(_) => {}
                    Err(cause) if cause.kind() == io::ErrorKind::WouldBlock => {
                        drained = true;
                        break;
                    }
                    Err(cause) if cause.kind() == io::ErrorKind::Interrupted => {}
                    Err(cause) => {
                        error = Some(cause.into());
                        break 'collect End::ReceiveError;
                    }
                }
            }
        }
        if progress.is_some() {
            let received = active
                .as_ref()
                .map_or(0, |active| active.seen_sequences.len() as u64);
            if received >= 256
                && let Some(progress) = progress.take()
            {
                let _ = progress.send(received);
            }
        }
        let command = *stop
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let deadline = match command {
            Some(Stop::Aborted) => break End::Aborted,
            Some(Stop::Finished { expected: None, .. }) => break End::TargetUnavailable,
            Some(Stop::Finished {
                expected: Some(count),
                deadline,
            }) => {
                expected = Some(count);
                let unique = active
                    .as_ref()
                    .map_or(0, |active| active.seen_sequences.len() as u64);
                if unique >= count {
                    break End::Delivered;
                }
                if Instant::now() >= deadline {
                    break End::DeliveryDeadline;
                }
                deadline
            }
            None => {
                if Instant::now() >= session_deadline {
                    error = Some("UDP collection deadline expired before target Finished".into());
                    break End::SessionDeadline;
                }
                session_deadline
            }
        };
        if !drained {
            continue;
        }
        if let Err(cause) = poll.wait(Some(deadline)) {
            error = Some(cause.into());
            break End::ReceiveError;
        }
        // Check the socket on both I/O and control edges. No shared descriptor
        // flags change, so the concurrent sender keeps its blocking contract.
    };
    let unique = active
        .as_ref()
        .map_or(0, |active| active.seen_sequences.len() as u64);
    let bursts: Vec<_> = active.into_iter().map(ActiveBurst::finish).collect();
    let kernel_drops = match oer_hil_net_traffic::udp::kernel_drops(&socket) {
        Ok(after) => after
            .zip(drops_before)
            .map(|(after, before)| after.wrapping_sub(before)),
        Err(cause) => {
            error = Some(cause.into());
            None
        }
    };
    if let Some(dropped) = kernel_drops
        && dropped != 0
    {
        end = End::HostOverflow;
        error = Some(io::Error::other(format!("host UDP socket dropped {dropped} datagrams in the kernel; delivery is not a valid radio-only measurement")).into());
    }
    fixtures::write_labelled(
        &directory,
        &label,
        &Reception {
            schema: 2,
            host_kernel_drops: kernel_drops,
            completion: end,
            expected_datagrams: expected,
            received_unique_datagrams: unique,
            undelivered_datagrams: expected.map(|count| count.saturating_sub(unique)),
            elapsed_micros: u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX),
            bursts: bursts.clone(),
            error: error.as_ref().map(|error| error.to_string()),
        },
    )?;
    match error {
        Some(error) => Err(error),
        None => Ok(bursts),
    }
}

/// MSG_DONTWAIT is local to this receive, unlike O_NONBLOCK on a cloned socket.
fn receive(socket: &UdpSocket, packet: &mut [u8]) -> io::Result<(usize, Ipv4Addr)> {
    let (received, _, source) = recvfrom(socket, packet, RecvFlags::DONTWAIT)?;
    let source = source
        .and_then(|source| SocketAddrV4::try_from(source).ok())
        .ok_or_else(|| io::Error::other("UDP collector requires an IPv4 peer"))?;
    Ok((received, *source.ip()))
}

#[cfg(test)]
mod tests;
