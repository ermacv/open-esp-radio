#![forbid(unsafe_code)]

use crate::product_hil::network::sockets::{Stack, accept, listen, new_tcp};
use oer_firmware_hil_agent::traffic::connection;
use oer_tool_data_hil_protocol::{network::SessionFailure, network::SessionVerdict};

use embassy_futures::join::join;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use oer_hil_esp32s31_telemetry::rx_pipeline::RxPipelineCounters;
use oer_tool_data_hil_protocol::{
    network::Completion as HilCompletion, network::Direction as HilDirection,
    network::FlowTransportEvidence, network::ServiceInfo, network::SessionReady,
    network::Transport as HilTransport, network::TransportEvidence, network::fill_stream_pattern,
    network::stream_pattern_matches, wifi::WifiNetworkInterface,
};

use crate::console::{complete_session, publish_event_reliably, runtime_log};
use crate::product_hil::{
    OPEN_RADIO_TASK_POLL_TELEMETRY, OPEN_RADIO_TCP_CHUNK_CAPACITY, QualificationRequester,
    TASK_POLLS, qualification_sample,
};

use super::{
    CpuLoadWindow, SessionChannel, aggregate_tx_evidence, log_open_radio_ampdu_snapshot,
    log_open_radio_task_poll_interval, wait_session_link_requirements,
};

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct TcpBenchmarkConfig {
    pub network_interface: WifiNetworkInterface,
    pub local_port: u16,
    pub maximum_payload_bytes: u16,
    pub receive_buffer_capacity: usize,
    pub transmit_buffer_capacity: usize,
    pub io_buffer_capacity: usize,
    pub idle_timeout: Duration,
}

#[derive(Clone, Copy, Default)]
struct StreamResult {
    bytes: u64,
    units: u64,
    errors: u32,
    eof: bool,
    pattern_ok: bool,
}

static TCP_RX_PATTERN_BUFFER: Mutex<CriticalSectionRawMutex, [u8; OPEN_RADIO_TCP_CHUNK_CAPACITY]> =
    Mutex::new([0; OPEN_RADIO_TCP_CHUNK_CAPACITY]);
static TCP_TX_PATTERN_BUFFER: Mutex<CriticalSectionRawMutex, [u8; OPEN_RADIO_TCP_CHUNK_CAPACITY]> =
    Mutex::new([0; OPEN_RADIO_TCP_CHUNK_CAPACITY]);

pub(in crate::product_hil) async fn run_open_radio_tcp_benchmark<'a>(
    stack: Stack<'a>,
    rx_buffer: &'a mut [u8],
    tx_buffer: &'a mut [u8],
    config: TcpBenchmarkConfig,
    pipeline_counters: &RxPipelineCounters,
    sessions: &'static SessionChannel,
) -> ! {
    let mut socket = new_tcp(stack, rx_buffer, tx_buffer);
    let mut listener = listen(stack, config.local_port);
    for direction in [
        HilDirection::Rx,
        HilDirection::Tx,
        HilDirection::Bidirectional,
    ] {
        publish_event_reliably(
            0,
            0,
            oer_tool_data_hil_protocol::network::ServiceReady(ServiceInfo {
                network_interface: config.network_interface,
                transport: HilTransport::Tcp,
                direction,
                local_port: config.local_port,
                maximum_payload_bytes: config.maximum_payload_bytes,
            }),
        )
        .await;
    }
    runtime_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-ready port={} rx_buffer={} \
         tx_buffer={} io_buffer={} session_protocol=required directions=rx,tx,bidirectional",
        config.local_port,
        config.receive_buffer_capacity,
        config.transmit_buffer_capacity,
        config.io_buffer_capacity,
    ));

    loop {
        let session = sessions.receive().await;
        wait_session_link_requirements(session.config.link_requirements, config.network_interface)
            .await;
        let duration_millis = match session.config.completion {
            HilCompletion::DurationMillis(duration) => duration,
            HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                unreachable!("protocol owner accepts only duration-completed sessions")
            }
        };
        let duration = Duration::from_millis(u64::from(duration_millis));
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=tcp-session-start session={} \
             direction={:?} duration_ms={}",
            session.session_id, session.config.direction, duration_millis,
        ));

        let qualification_start = qualification_sample(QualificationRequester::TcpBegin).await;
        #[cfg(feature = "rx-ownership-telemetry")]
        crate::product_hil::rx_ownership::begin();
        let hardware_start = qualification_start.rx_primary;
        let task_poll_start = TASK_POLLS.snapshot();
        let pipeline_start = pipeline_counters.snapshot();
        let aggregate_start = qualification_start.aggregate_tx;
        let connection_timeout = duration + Duration::from_secs(5);
        let connected = match with_timeout(
            connection_timeout,
            connection::before_ready(accept(&mut listener, &mut socket), async {
                publish_event_reliably(
                    session.session_id,
                    0,
                    SessionReady {
                        direction: session.config.direction,
                        tx_block_ack_tid: session.config.link_requirements.tx_block_ack_tid,
                    },
                )
                .await;
            }),
        )
        .await
        {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                runtime_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=tcp-accept error={error:?}"
                ));
                false
            }
            Err(_) => {
                runtime_log(format_args!(
                    "OPEN_RADIO_PHY_HIL result=FAIL stage=tcp-accept error=Timeout"
                ));
                false
            }
        };
        socket.set_nagle_enabled(false);
        let started = Instant::now();
        let cpu_window = CpuLoadWindow::begin();
        let (mut rx, mut tx) = (StreamResult::default(), StreamResult::default());
        let session_flow = session
            .config
            .primary_flow()
            .expect("validated TCP session carries a primary flow");

        if connected {
            match session.config.direction {
                HilDirection::Rx => {
                    let flow = session_flow
                        .target_rx
                        .expect("validated TCP RX session carries an RX flow");
                    rx = receive_stream(
                        &mut socket,
                        usize::from(flow.payload_bytes),
                        config.idle_timeout,
                    )
                    .await;
                }
                HilDirection::Tx => {
                    let flow = session_flow
                        .target_tx
                        .expect("validated TCP TX session carries a TX flow");
                    tx = transmit_stream(
                        &mut socket,
                        usize::from(flow.payload_bytes),
                        flow.offered_rate_bps,
                        started,
                        duration,
                    )
                    .await;
                }
                HilDirection::Bidirectional => {
                    let rx_flow = session_flow
                        .target_rx
                        .expect("validated bidirectional TCP session carries an RX flow");
                    let tx_flow = session_flow
                        .target_tx
                        .expect("validated bidirectional TCP session carries a TX flow");
                    let (mut reader, mut writer) = socket.split();
                    (rx, tx) = join(
                        receive_stream(
                            &mut reader,
                            usize::from(rx_flow.payload_bytes),
                            config.idle_timeout,
                        ),
                        transmit_stream(
                            &mut writer,
                            usize::from(tx_flow.payload_bytes),
                            tx_flow.offered_rate_bps,
                            started,
                            duration,
                        ),
                    )
                    .await;
                }
            }
        } else {
            match session.config.direction {
                HilDirection::Rx => rx.errors = 1,
                HilDirection::Tx => tx.errors = 1,
                HilDirection::Bidirectional => {
                    rx.errors = 1;
                    tx.errors = 1;
                }
            }
        }

        if matches!(
            session.config.direction,
            HilDirection::Tx | HilDirection::Bidirectional
        ) {
            // `close` appends FIN after every byte already accepted by the
            // socket.  The following bounded flush is therefore the target's
            // terminal-delivery frontier: it must observe both the payload
            // ACKs and the FIN ACK before the session can pass.  In
            // particular, an inner connection error is not a successful
            // timeout operation.
            socket.close();
            match with_timeout(config.idle_timeout, socket.flush()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) | Err(_) => {
                    tx.errors = tx.errors.saturating_add(1);
                }
            }
        }
        let cpu_load = cpu_window.finish();
        let elapsed_us = started.elapsed().as_micros().max(1);
        socket.abort();

        super::session_evidence_due();
        let qualification = qualification_sample(QualificationRequester::Tcp).await;
        #[cfg(feature = "rx-ownership-telemetry")]
        let ownership_valid = crate::product_hil::rx_ownership::report().await;
        #[cfg(not(feature = "rx-ownership-telemetry"))]
        let ownership_valid = true;
        let hardware_delta = qualification
            .rx_primary
            .zip(hardware_start)
            .map(|(current, earlier)| current.wrapping_delta_since(earlier))
            .unwrap_or_default();
        let pipeline_end = pipeline_counters.snapshot();
        let enqueued = pipeline_end
            .network_enqueued
            .wrapping_sub(pipeline_start.network_enqueued);
        let queue_dropped = pipeline_end
            .network_dropped
            .wrapping_sub(pipeline_start.network_dropped);
        let health_errors = u32::from(hardware_delta.buffer_full)
            .saturating_add(u32::from(hardware_delta.fifo_overflow))
            .saturating_add(queue_dropped);
        let transport_errors = rx
            .errors
            .saturating_add(tx.errors)
            .saturating_add(health_errors);
        let (units_complete, pattern_checked) = match session.config.direction {
            HilDirection::Rx => (rx.units == 1, true),
            HilDirection::Tx => (tx.units == 1, false),
            HilDirection::Bidirectional => (rx.units == 1 && tx.units == 1, true),
        };
        // The first check that failed, in the order the verdict evaluates.
        let verdict = if !ownership_valid {
            SessionVerdict::Failed(SessionFailure::OwnershipInvalid)
        } else if !connected {
            SessionVerdict::Failed(SessionFailure::NotConnected)
        } else if rx.errors != 0 {
            SessionVerdict::Failed(SessionFailure::ReceiveErrors(rx.errors))
        } else if tx.errors != 0 {
            SessionVerdict::Failed(SessionFailure::TransmitErrors(tx.errors))
        } else if health_errors != 0 {
            SessionVerdict::Failed(SessionFailure::Health {
                buffer_full: u32::from(hardware_delta.buffer_full),
                fifo_overflow: u32::from(hardware_delta.fifo_overflow),
                queue_dropped,
            })
        } else if !units_complete {
            SessionVerdict::Failed(SessionFailure::Incomplete {
                rx_units: u32::try_from(rx.units).unwrap_or(u32::MAX),
                tx_units: u32::try_from(tx.units).unwrap_or(u32::MAX),
            })
        } else if pattern_checked && !rx.pattern_ok {
            SessionVerdict::Failed(SessionFailure::PatternMismatch)
        } else {
            SessionVerdict::Passed
        };
        runtime_log(format_args!(
            "OTCP dir={:?} rb={} tb={} ru={} tu={} u={} e={} bf={} fo={} enq={} drop={} eof={} pat={}",
            session.config.direction,
            rx.bytes,
            tx.bytes,
            rx.units,
            tx.units,
            elapsed_us,
            transport_errors,
            hardware_delta.buffer_full,
            hardware_delta.fifo_overflow,
            enqueued,
            queue_dropped,
            u8::from(rx.eof),
            u8::from(rx.pattern_ok),
        ));
        let aggregate = qualification
            .aggregate_tx
            .zip(aggregate_start)
            .map(|(current, earlier)| current.wrapping_delta_since(earlier));
        let aggregate_evidence = aggregate
            .filter(|_| {
                matches!(
                    session.config.direction,
                    HilDirection::Tx | HilDirection::Bidirectional
                )
            })
            .filter(|aggregate| aggregate.rate_selections != 0)
            .map(aggregate_tx_evidence);
        if let Some(aggregate) = aggregate {
            log_open_radio_ampdu_snapshot(aggregate).await;
        }
        log_open_radio_task_poll_interval(
            task_poll_start,
            OPEN_RADIO_TASK_POLL_TELEMETRY,
            &TASK_POLLS,
        )
        .await;
        let transport = TransportEvidence {
            rx_maximum_silence_micros: None,
            rx_bytes: rx.bytes,
            tx_bytes: tx.bytes,
            rx_units: rx.units,
            tx_units: tx.units,
            rx_late_bytes: 0,
            rx_late_units: 0,
            elapsed_micros: elapsed_us,
            transport_errors,
        };
        complete_session(
            session.session_id,
            [
                Some(FlowTransportEvidence::from_session_total(
                    session_flow.flow_id,
                    transport,
                )),
                None,
            ],
            aggregate_evidence.map(|(radio, _)| radio),
            aggregate_evidence.map(|(_, timing)| timing),
            None,
            None,
            cpu_load,
            verdict,
        )
        .await;
        super::session_evidence_published();
    }
}

use embedded_io_async::{Read as TcpRead, Write as TcpWrite};

async fn receive_stream(
    reader: &mut impl TcpRead,
    chunk_bytes: usize,
    idle_timeout: Duration,
) -> StreamResult {
    let mut result = StreamResult {
        pattern_ok: true,
        ..StreamResult::default()
    };
    loop {
        let (read, pattern_matches) = {
            let mut buffer = TCP_RX_PATTERN_BUFFER.lock().await;
            let read = with_timeout(idle_timeout, reader.read(&mut buffer[..chunk_bytes])).await;
            let pattern_matches = match read {
                Ok(Ok(length)) => stream_pattern_matches(&buffer[..length], result.bytes),
                Ok(Err(_)) | Err(_) => true,
            };
            (read, pattern_matches)
        };
        match read {
            Ok(Ok(0)) => {
                result.eof = true;
                result.units = 1;
                break;
            }
            Ok(Ok(length)) => {
                result.pattern_ok &= pattern_matches;
                result.bytes = result.bytes.saturating_add(length as u64);
            }
            Ok(Err(_)) | Err(_) => {
                result.errors = result.errors.saturating_add(1);
                break;
            }
        }
    }
    result
}

async fn transmit_stream(
    writer: &mut impl TcpWrite,
    chunk_bytes: usize,
    offered_rate_bps: Option<u64>,
    started: Instant,
    duration: Duration,
) -> StreamResult {
    let mut result = StreamResult::default();
    let mut pending_offset = chunk_bytes;
    while started.elapsed() < duration {
        let elapsed = started.elapsed();
        if elapsed >= duration {
            break;
        }
        let remaining = duration - elapsed;
        let write = {
            let mut buffer = TCP_TX_PATTERN_BUFFER.lock().await;
            if pending_offset == chunk_bytes {
                fill_stream_pattern(&mut buffer[..chunk_bytes], result.bytes);
                pending_offset = 0;
            }
            with_timeout(
                remaining,
                writer.write(&buffer[pending_offset..chunk_bytes]),
            )
            .await
        };
        match write {
            Err(_) if started.elapsed() >= duration => break,
            Ok(Ok(0)) | Ok(Err(_)) | Err(_) => {
                result.errors = result.errors.saturating_add(1);
                break;
            }
            Ok(Ok(length)) => {
                pending_offset += length;
                result.bytes = result.bytes.saturating_add(length as u64);
            }
        }
        if let Some(rate_bps) = offered_rate_bps {
            let elapsed_us = result.bytes.saturating_mul(8_000_000) / rate_bps;
            let deadline = started + Duration::from_micros(elapsed_us);
            if Instant::now() < deadline {
                Timer::at(deadline).await;
            }
        }
    }
    // The socket owner performs one bounded close+flush after both halves of
    // a bidirectional session have joined.  A writer cannot establish that
    // terminal frontier on its own because it does not own the socket state.
    result.units = u64::from(result.bytes != 0 && result.errors == 0);
    result
}
