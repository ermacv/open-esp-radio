//! UDP consumption and timing, independent of radio qualification reporting.

use embassy_futures::yield_now;
use embassy_time::{Duration, Instant, with_timeout};
use oer_hil_agent::traffic::payload::PayloadFillCheck;
use oer_hil_esp32s31_telemetry::task_poll::{TaskPollSet, TaskPollSetSnapshot};
use oer_hil_esp32s31_telemetry::udp_rx_window::RxWindow;
use oer_hil_protocol::{
    network::Completion, network::CpuLoadEvidence, network::FlowTransportEvidence,
    network::SESSION_FLOW_CAPACITY, network::UdpSessionPayloadIdentity,
};

use crate::{
    console::{ActiveSession, runtime_log_reliably},
    product_hil::{
        network::sockets::{Ipv4Addr, SocketAddr, UdpSocket, recv_from_with},
        traffic::{CpuLoadWindow, UdpSequenceEvidence, iperf2_udp_sequence},
    },
};

const STARTUP_MICROS: u64 = 2_000_000;
const TERMINAL_GRACE_MICROS: u64 = 750_000;

pub(super) struct Outcome {
    pub bytes: u64,
    pub datagrams: u64,
    pub receive_errors: u32,
    pub terminal_seen: bool,
    pub sequence: UdpSequenceEvidence,
    pub flow_evidence: [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY],
    pub elapsed_us: u64,
    pub task_poll_end: TaskPollSetSnapshot,
    /// Over the measurement window: from its start, the first datagram or
    /// the end of the startup allowance, whichever is first, to its end.
    pub cpu_load: Option<CpuLoadEvidence>,
}

struct Flow {
    peer: Option<SocketAddr>,
    payload_identity: Option<UdpSessionPayloadIdentity>,
    payload: usize,
    terminal: bool,
    evidence: FlowTransportEvidence,
}

pub(super) async fn run(
    socket: &mut UdpSocket<'_>,
    session: ActiveSession,
    silence_threshold: Duration,
    task_polls: &TaskPollSet,
) -> Outcome {
    let Completion::DurationMillis(duration) = session.config.completion else {
        unreachable!("protocol owner accepts only duration-completed sessions")
    };
    let elapsed_us = u64::from(duration) * 1_000;
    let mut delivery_start = oer_hil_esp32s31_telemetry::udp_rx_window::DeliveryStart::default();
    let single_flow = session.config.active_flow_count() == 1;
    let mut flows = session.config.flows.map(|flow| {
        flow.map(|flow| Flow {
            // Single-flow host pacing uses an ephemeral source port. Multiple
            // flows have independently configured, fixed peer endpoints.
            peer: (!single_flow).then(|| {
                let peer = flow.peer.expect("validated multi-flow peer");
                (Ipv4Addr::from(peer.address), peer.port).into()
            }),
            payload_identity: flow.payload_identity,
            payload: usize::from(flow.target_rx.expect("validated RX flow").payload_bytes),
            terminal: false,
            evidence: FlowTransportEvidence {
                rx_maximum_silence_micros: None,
                flow_id: flow.flow_id,
                rx_bytes: 0,
                tx_bytes: 0,
                rx_units: 0,
                tx_units: 0,
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros: elapsed_us,
                transport_errors: 0,
            },
        })
    });
    let payload_fill =
        PayloadFillCheck::for_flows(flows.iter().flatten().map(|flow| flow.payload_identity));
    let mut cpu_window = None;
    let mut cpu_window_opened = false;
    let mut cpu_load = None;
    let mut window = RxWindow::new(
        Instant::now().as_micros(),
        elapsed_us,
        STARTUP_MICROS,
        TERMINAL_GRACE_MICROS,
        silence_threshold.as_micros(),
    );
    let mut sequence = UdpSequenceEvidence::default();
    let mut last_data = None;
    let mut socket_errors = 0_u32;
    let mut last_socket_error = None;
    let mut unknown_packets = 0_u32;
    let mut late_datagrams = 0_u64;
    let mut maximum_deadline_lateness = 0_u64;
    let mut task_poll_end = None;
    loop {
        let now = Instant::now().as_micros();
        if !cpu_window_opened && now >= window.start() {
            // No datagram before the startup allowance ended: the
            // measurement window starts there.
            cpu_window = Some(CpuLoadWindow::begin());
            cpu_window_opened = true;
        }
        if now >= window.end() && task_poll_end.is_none() {
            task_poll_end = Some(task_polls.snapshot());
            cpu_load = cpu_window.take().and_then(CpuLoadWindow::finish);
            #[cfg(feature = "pc-profile")]
            crate::pc_profile::window_end();
        }
        let terminal = flows.iter().flatten().all(|flow| flow.terminal);
        if window.finished(now, terminal) {
            break;
        }
        let mut deadline = window.next_deadline(now);
        if !cpu_window_opened {
            // Wake at the window's start to open the CPU window there.
            deadline = deadline.min(window.start());
        }
        let received = with_timeout(
            Duration::from_micros(deadline.saturating_sub(now)),
            recv_from_with(socket, |packet, metadata| {
                (
                    packet.len(),
                    iperf2_udp_sequence(packet),
                    metadata.remote_addr,
                    UdpSessionPayloadIdentity::from_payload(packet),
                    payload_fill.fill_matches(packet),
                )
            }),
        )
        .await;
        let received_at = Instant::now().as_micros();
        maximum_deadline_lateness =
            maximum_deadline_lateness.max(received_at.saturating_sub(deadline));
        let (length, packet_sequence, endpoint, packet_identity, payload_fill_matches) =
            match received {
                Ok(Ok(packet)) => packet,
                Ok(Err(error)) => {
                    socket_errors = socket_errors.saturating_add(1);
                    last_socket_error = Some(error);
                    // A failed receive is not a reason to abandon the observation
                    // window, nor to monopolize the executor with immediate retries.
                    yield_now().await;
                    continue;
                }
                Err(_) => continue,
            };
        let Some(flow) = flows
            .iter_mut()
            .flatten()
            .find(|flow| flow.peer.is_none_or(|peer| peer == endpoint))
        else {
            unknown_packets = unknown_packets.saturating_add(1);
            continue;
        };
        if flow
            .payload_identity
            .is_some_and(|expected| packet_identity != Some(expected) || !payload_fill_matches)
        {
            unknown_packets = unknown_packets.saturating_add(1);
            continue;
        }
        #[cfg(feature = "rx-delivery-telemetry")]
        if single_flow && let Some(sequence) = packet_sequence {
            crate::product_hil::rx_qualification::HilConnectedRxObserver::observe_udp_consumer(
                session.session_id,
                sequence,
            );
        }
        if packet_sequence.is_some_and(|value| value < 0) {
            // Readiness probes use the same negative wire marker outside a
            // session. A marker before this flow's first data is not completion.
            flow.terminal |= flow.evidence.rx_units != 0;
            continue;
        }
        if flow.payload_identity.is_some_and(|expected| {
            !expected.accepts_next_data(
                packet_identity,
                payload_fill_matches,
                packet_sequence,
                flow.evidence.rx_units,
            )
        }) {
            unknown_packets = unknown_packets.saturating_add(1);
            continue;
        }
        if !window.data(received_at) {
            // After the measurement window and before the peer's terminal
            // marker ends the session: delivered, so it counts toward the
            // delivery and its ordering, not toward the throughput.
            late_datagrams = late_datagrams.saturating_add(1);
            flow.evidence.rx_late_bytes = flow.evidence.rx_late_bytes.saturating_add(length as u64);
            flow.evidence.rx_late_units = flow.evidence.rx_late_units.saturating_add(1);
            if single_flow {
                sequence.observe(packet_sequence);
            }
            continue;
        }
        if !cpu_window_opened {
            // The measurement window starts at its first datagram, here
            // before the startup allowance ended.
            cpu_window = Some(CpuLoadWindow::begin());
            cpu_window_opened = true;
        }
        flow.evidence.rx_bytes = flow.evidence.rx_bytes.saturating_add(length as u64);
        flow.evidence.rx_units = flow.evidence.rx_units.saturating_add(1);
        if single_flow && delivery_start.observe(length == flow.payload, packet_sequence) {
            crate::console::publish_event_reliably(
                session.session_id,
                0,
                oer_hil_protocol::network::UdpRxStarted { datagrams: 256 },
            )
            .await;
        }
        flow.evidence.transport_errors = flow
            .evidence
            .transport_errors
            .saturating_add(u32::from(length != flow.payload));
        if single_flow {
            sequence.observe(packet_sequence);
            if let Some(last) = last_data {
                sequence.observe_interarrival(packet_sequence, received_at.saturating_sub(last));
            }
            last_data = Some(received_at);
        }
    }
    let silence = window.summary();
    if single_flow {
        for flow in flows.iter_mut().flatten() {
            flow.evidence.rx_maximum_silence_micros = Some(silence.maximum_silence_micros);
        }
    }
    runtime_log_reliably(format_args!(
        "ORX_WINDOW session={} end=duration no_data={} start_us={} duration_us={} \
         first_delay_us={:?} terminal={} socket_errors={} unknown_packets={} \
         late_datagrams={} deadline_late_us={}",
        session.session_id,
        u8::from(silence.first_delay_micros.is_none()),
        window.start(),
        elapsed_us,
        silence.first_delay_micros,
        u8::from(flows.iter().flatten().all(|flow| flow.terminal)),
        socket_errors,
        unknown_packets,
        late_datagrams,
        maximum_deadline_lateness,
    ))
    .await;
    runtime_log_reliably(format_args!(
        "ORX_SILENCE session={} threshold_us={} pauses={} max_us={} \
         max_start_offset_us={} tail_us={} cause=unlocalized",
        session.session_id,
        silence_threshold.as_micros(),
        silence.pauses,
        silence.maximum_silence_micros,
        silence.maximum_silence_start_micros,
        silence.trailing_silence_micros,
    ))
    .await;
    if let Some(error) = last_socket_error {
        runtime_log_reliably(format_args!(
            "ORX_SOCKET session={} last_error={error:?}",
            session.session_id
        ))
        .await;
    }
    if let Some(primary) = flows.iter_mut().flatten().next() {
        primary.evidence.transport_errors = primary
            .evidence
            .transport_errors
            .saturating_add(socket_errors)
            .saturating_add(unknown_packets);
    }
    let terminal_seen = flows.iter().flatten().all(|flow| flow.terminal);
    let flow_evidence = flows.map(|flow| flow.map(|flow| flow.evidence));
    let aggregate = oer_hil_protocol::network::TransportEvidence::from_flows(flow_evidence);
    Outcome {
        bytes: aggregate.rx_bytes,
        datagrams: aggregate.rx_units,
        receive_errors: aggregate.transport_errors,
        terminal_seen,
        sequence,
        flow_evidence,
        elapsed_us,
        task_poll_end: task_poll_end.expect("completed RX window closes task observation"),
        cpu_load,
    }
}
