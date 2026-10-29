//! Test workload only: ordinary pinned-fork UDP sockets on the product IP stack.
use crate::transport::CONSOLE;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_embassy_time::EmbassyClock;
use oer_hil_protocol::{
    RequestIdentity,
    base::{Accepted, RejectReason, Rejected},
};
use oer_network_control::*;
use oer_time::{Clock, Duration, Timer};
use owned_embassy_net::{udp::UdpSocket, wire::Ipv4Addr};

oer_hil_agent::requests! {
    pub enum Requests(sessions = false) { Start(StartDatagrams), Stop(StopDatagrams), }
    publishes [DatagramsFinished, Udp]
}
enum Operation {
    Start(RequestIdentity, StartDatagrams),
    Stop(RequestIdentity),
    StationStop,
}
static OPERATIONS: Channel<CriticalSectionRawMutex, Operation, 1> = Channel::new();
static STOPPED: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();
static ACTIVE: AtomicBool = AtomicBool::new(false);
pub(super) fn request(request: RequestIdentity, command: Requests, running: bool) {
    let operation = match command {
        Requests::Start(_) if !running => {
            super::emit(request, &Rejected(RejectReason::InvalidState));
            return;
        }
        Requests::Start(config) if config.valid() => {
            if ACTIVE.swap(true, Ordering::Relaxed) {
                super::emit(request, &Rejected(RejectReason::Busy));
                return;
            }
            Operation::Start(request, config)
        }
        Requests::Start(_) => {
            super::emit(request, &Rejected(RejectReason::InvalidConfiguration));
            return;
        }
        Requests::Stop(_) if ACTIVE.load(Ordering::Relaxed) => Operation::Stop(request),
        _ => {
            super::emit(request, &Rejected(RejectReason::InvalidState));
            return;
        }
    };
    let start = matches!(operation, Operation::Start(..));
    if OPERATIONS.try_send(operation).is_err() {
        if start {
            ACTIVE.store(false, Ordering::Relaxed);
        }
        super::emit(request, &Rejected(RejectReason::Busy));
    } else {
        super::emit(request, &Accepted);
    }
}
/// Join the application before asking the product station to leave.
pub(super) async fn stop_for_station() {
    OPERATIONS.send(Operation::StationStop).await;
    STOPPED.receive().await;
}

pub(super) async fn run(stack: super::ip::Stack) -> ! {
    loop {
        let (start, config) = match OPERATIONS.receive().await {
            Operation::Start(request, config) => (request, config),
            Operation::StationStop => {
                STOPPED.send(()).await;
                continue;
            }
            Operation::Stop(_) => continue,
        };
        let started_us = EmbassyClock.now().as_micros();
        let mut tracker = DatagramTracker::new();
        let mut stopped_by = start;
        let mut station_stop = false;
        let end = match UdpSocket::new(stack) {
            Err(_) => DatagramEnd::SocketCapacity,
            Ok(mut socket) => {
                if socket.bind(config.local_port, 0_u16).is_err() {
                    DatagramEnd::Bind
                } else {
                    let ending = match select(
                        OPERATIONS.receive(),
                        workload(&socket, config, &mut tracker),
                    )
                    .await
                    {
                        Either::First(Operation::Stop(request)) => {
                            stopped_by = request;
                            DatagramEnd::Stopped
                        }
                        Either::First(Operation::StationStop) => {
                            station_stop = true;
                            DatagramEnd::Stopped
                        }
                        Either::First(Operation::Start(..)) => DatagramEnd::Bind,
                        Either::Second(end) => end,
                    };
                    socket.close();
                    ending
                }
            }
        };
        ACTIVE.store(false, Ordering::Relaxed);
        let result = DatagramsFinished {
            start_request: start.request_id,
            started_us,
            ended_us: EmbassyClock.now().as_micros(),
            end,
            counters: tracker.counters(),
        };
        CONSOLE
            .publish_reliably(stopped_by.session_id, stopped_by.request_id, &result)
            .await;
        if station_stop {
            STOPPED.send(()).await;
        }
    }
}
async fn workload(
    socket: &UdpSocket<'static, super::ip::Device>,
    config: StartDatagrams,
    tracker: &mut DatagramTracker,
) -> DatagramEnd {
    let mut bytes = [0x5a; MAX_DATAGRAM_PAYLOAD as usize];
    match config.mode {
        DatagramMode::Echo | DatagramMode::Sink => loop {
            let (length, metadata) = match socket.recv_from(&mut bytes).await {
                Ok(received) => received,
                Err(_) => return DatagramEnd::Receive,
            };
            let Some(payload) = bytes.get(..length) else {
                return DatagramEnd::Receive;
            };
            tracker.received(payload);
            if config.mode == DatagramMode::Echo {
                if socket.send_to(payload, metadata).await.is_err() {
                    return DatagramEnd::Send;
                }
                tracker.sent(length as u16);
            }
            embassy_futures::yield_now().await;
        },
        DatagramMode::Source => {
            let Some(peer) = config.peer else {
                return DatagramEnd::Send;
            };
            let mut sequence = 0_u64;
            let interval = match config.packets_per_second {
                Some(rate) => match 1_000_000_u64.checked_div(u64::from(rate)) {
                    Some(micros) if micros != 0 => Some(Duration::from_micros(micros)),
                    _ => return DatagramEnd::Deadline,
                },
                None => None,
            };
            let Some(payload) = bytes.get_mut(..usize::from(config.payload_bytes)) else {
                return DatagramEnd::Send;
            };
            loop {
                let now = EmbassyClock.now();
                if !(DatagramHeader {
                    sequence,
                    sent_us: now.as_micros(),
                })
                .write(payload)
                {
                    return DatagramEnd::Send;
                }
                if socket
                    .send_to(payload, (Ipv4Addr::from(peer.address), peer.port))
                    .await
                    .is_err()
                {
                    return DatagramEnd::Send;
                }
                tracker.sent(config.payload_bytes);
                sequence = match sequence.checked_add(1) {
                    Some(next) => next,
                    None => return DatagramEnd::SequenceExhausted,
                };
                if let Some(interval) = interval {
                    let Some(deadline) = now.checked_add(interval) else {
                        return DatagramEnd::Deadline;
                    };
                    EmbassyClock.wait_until(deadline).await;
                }
                embassy_futures::yield_now().await;
            }
        }
    }
}
