#![forbid(unsafe_code)]

use crate::product_hil::network::sockets::{Ipv4Addr, Stack, UdpSocket, UdpTxStorage, new_udp};
use core::sync::atomic::{AtomicU8, Ordering};
use embassy_time::{Duration, Instant, Timer, with_timeout};
#[cfg(feature = "core0-rx-coarse-telemetry")]
use oer_esp32s31_ieee80211_system::CORE0_PERFORMANCE;
#[cfg(feature = "core0-rx-coarse-telemetry")]
use oer_esp32s31_ieee80211_system::TX_PERFORMANCE;
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use oer_adapters_esp_hal_soc::L1CachePerformanceCounters;
use oer_tool_data_hil_protocol::{
    network::Completion as HilCompletion, network::Direction as HilDirection,
    network::FlowTransportEvidence, network::SESSION_FLOW_CAPACITY, network::ServiceInfo,
    network::SessionConfig, network::SessionReady, network::Transport as HilTransport,
    network::TransportEvidence,
};
use oer_tool_data_hil_protocol::{network::SessionFailure, network::SessionVerdict};

use crate::{
    console::{publish_event_reliably, runtime_log},
    product_hil::traffic::{
        BidirectionalResultChannel, BidirectionalSessionChannel, CpuLoadWindow,
        OpenRadioBidirectionalDirection, OpenRadioBidirectionalResult, aggregate_tx_evidence,
        complete_open_radio_bidirectional_direction, wait_session_link_requirements,
    },
    product_hil::{
        OPEN_RADIO_TASK_POLL_TELEMETRY, QualificationRequester, TASK_POLLS, qualification_sample,
    },
};

use crate::product_hil::traffic::reporting::{
    log_open_radio_ampdu_snapshot, log_open_radio_task_poll_snapshot,
};

#[cfg(feature = "core0-rx-coarse-telemetry")]
use crate::product_hil::traffic::log_open_radio_core0_rx_coarse;

const MAX_PACING_CATCH_UP_GROUPS: u32 = 4;
const DEFAULT_MULTI_FLOW_BURST_DATAGRAMS: u8 = 1;
static MULTI_FLOW_BURST_DATAGRAMS: AtomicU8 = AtomicU8::new(DEFAULT_MULTI_FLOW_BURST_DATAGRAMS);

pub(in crate::product_hil) fn configure_multi_flow_burst_datagrams(datagrams: u8) {
    assert!(datagrams != 0, "multi-flow burst cannot be empty");
    MULTI_FLOW_BURST_DATAGRAMS.store(datagrams, Ordering::Release);
}

pub(in crate::product_hil) fn multi_flow_burst_datagrams() -> u8 {
    MULTI_FLOW_BURST_DATAGRAMS.load(Ordering::Acquire)
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpTxSessionSource {
    pub sessions: &'static BidirectionalSessionChannel,
    pub results: &'static BidirectionalResultChannel,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpTxBenchmarkConfig {
    pub network_interface: oer_tool_data_hil_protocol::wifi::WifiNetworkInterface,
    pub source_port: u16,
    pub payload_capacity: usize,
    /// Maximum application datagrams admitted before enforcing the next
    /// absolute offered-rate deadline. The composition root derives this from
    /// the active plus prepared-ahead A-MPDU arenas, not an arbitrary poll
    /// batch.
    pub pacing_group_datagrams: u8,
    /// Number of consecutive successful publications offered to one flow
    /// before rotating to the next ready flow. A blocked socket ends its
    /// burst immediately; other flows retain independent pacing and progress.
    pub multi_flow_burst_datagrams: u8,
    pub code_address: usize,
    pub session_source: UdpTxSessionSource,
}

use oer_firmware_hil_agent::traffic::multi_tx;

async fn transmit_multi_flow(
    sockets: [&mut UdpSocket<'_>; SESSION_FLOW_CAPACITY],
    session_config: SessionConfig,
    started: Instant,
    duration: Duration,
    pacing_group_datagrams: u8,
    multi_flow_burst_datagrams: u8,
) -> [Option<FlowTransportEvidence>; SESSION_FLOW_CAPACITY] {
    use core::{
        future::{Future, poll_fn},
        task::Poll,
    };
    let mut producer = multi_tx::Producer::new(
        session_config,
        started.as_micros(),
        duration.as_micros(),
        pacing_group_datagrams,
        multi_flow_burst_datagrams,
    );
    poll_fn(|cx| {
        let result = producer.poll(
            cx,
            || Instant::now().as_micros(),
            |index, publication, cx| {
                // The pinned stack implementations register their socket waker on
                // Pending before constructing/publishing the payload. No lease or
                // partially built datagram escapes this one cancellation-safe poll.
                let send = sockets[index].send_to_with(
                    publication.payload_bytes,
                    (
                        Ipv4Addr::from(publication.peer.address),
                        publication.peer.port,
                    ),
                    |payload| {
                        payload[..4].copy_from_slice(&publication.sequence.to_be_bytes());
                        if let Some(identity) = session_config.flows[index]
                            .expect("validated TX session retains its flow")
                            .payload_identity
                        {
                            assert!(identity.write_to(payload));
                            assert!(
                                oer_tool_data_hil_protocol::network::UdpSessionPayloadIdentity::fill_after_header(
                                    payload
                                )
                            );
                        }
                        (publication.payload_bytes, ())
                    },
                );
                let result = core::pin::pin!(send).as_mut().poll(cx);
                #[cfg(feature = "station-exit-evidence")]
                if index == 1 {
                    let counters = &crate::product_hil::AGGREGATE_TX.secondary_socket;
                    let now = Instant::now().as_micros() as u32;
                    match &result {
                        Poll::Pending => counters.blocked(now),
                        Poll::Ready(Ok(())) => counters.admitted(publication.sequence, now),
                        Poll::Ready(Err(_)) => counters.failed(now),
                    }
                }
                result
            },
        );
        if result.is_ready() {
            return result;
        }
        let timer = Timer::at(Instant::from_micros(producer.deadline()));
        if core::pin::pin!(timer).as_mut().poll(cx).is_ready() {
            // A pacing/session edge elapsed while servicing the sockets.
            cx.waker().wake_by_ref();
        }
        Poll::Pending
    })
    .await;
    producer.evidence(Instant::now().as_micros())
}

/// Device-to-host UDP load through Embassy and the open TX scheduler.
pub(in crate::product_hil) async fn run_open_radio_udp_tx_benchmark<'a>(
    stack: Stack<'a>,
    storage: &'a mut [UdpTxStorage; SESSION_FLOW_CAPACITY],
    config: UdpTxBenchmarkConfig,
    #[cfg(any(
        feature = "core0-rx-cycle-telemetry",
        feature = "core0-rx-coarse-telemetry"
    ))]
    l1_cache: &'static L1CachePerformanceCounters,
) -> ! {
    // ServiceReady attests a bound socket, so host firewall/neighbor probes
    // may arrive before Start without hitting an unopened UDP port.
    let [primary_storage, secondary_storage] = storage;
    let mut socket = new_udp(stack, primary_storage);
    let mut secondary_socket = new_udp(stack, secondary_storage);
    secondary_socket
        .bind(
            config
                .source_port
                .checked_add(1)
                .expect("second UDP source port"),
            embassy_net::wire::ListenSocketAddr::UNSPECIFIED,
        )
        .unwrap_or_else(|error| panic!("secondary UDP TX socket bind failed: {error:?}"));
    socket
        .bind(
            config.source_port,
            embassy_net::wire::ListenSocketAddr::UNSPECIFIED,
        )
        .unwrap_or_else(|error| panic!("production UDP TX socket bind failed: {error:?}"));
    for source_port in [config.source_port, config.source_port + 1] {
        publish_event_reliably(
            0,
            0,
            oer_tool_data_hil_protocol::network::ServiceReady(ServiceInfo {
                network_interface: config.network_interface,
                transport: HilTransport::Udp,
                direction: HilDirection::Tx,
                local_port: source_port,
                maximum_payload_bytes: config.payload_capacity as u16,
            }),
        )
        .await;
    }
    runtime_log(format_args!(
        "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-ready \
         source_port={} payload_capacity={} tx_mode=ampdu session_protocol=required",
        config.source_port, config.payload_capacity,
    ));
    loop {
        let session = loop {
            let mut probe = [0_u8; oer_tool_data_hil_protocol::network::UdpProbe::LENGTH + 1];
            let mut secondary_probe = [0_u8; oer_tool_data_hil_protocol::network::UdpProbe::LENGTH + 1];
            match embassy_futures::select::select3(
                config.session_source.sessions.receive(),
                socket.recv_from(&mut probe),
                secondary_socket.recv_from(&mut secondary_probe),
            )
            .await
            {
                embassy_futures::select::Either3::First(session) => break session,
                embassy_futures::select::Either3::Second(Ok((length, peer))) => {
                    if let Some(mut request) =
                        oer_tool_data_hil_protocol::network::UdpProbe::decode(&probe[..length])
                        && !request.response
                    {
                        request.response = true;
                        // This is unmeasured traffic on the bound TX socket.
                        // Only its receipt at the host establishes reverse readiness.
                        let _ = socket.send_to(&request.encode(), peer).await;
                    }
                }
                embassy_futures::select::Either3::Third(Ok((length, peer))) => {
                    if let Some(mut request) =
                        oer_tool_data_hil_protocol::network::UdpProbe::decode(&secondary_probe[..length])
                        && !request.response
                    {
                        request.response = true;
                        let _ = secondary_socket.send_to(&request.encode(), peer).await;
                    }
                }
                embassy_futures::select::Either3::Second(Err(_))
                | embassy_futures::select::Either3::Third(Err(_)) => {}
            }
        };
        wait_session_link_requirements(session.config.link_requirements, config.network_interface)
            .await;
        publish_event_reliably(
            session.session_id,
            0,
            SessionReady {
                direction: HilDirection::Tx,
                tx_block_ack_tid: session.config.link_requirements.tx_block_ack_tid,
            },
        )
        .await;
        let session_flow = session
            .config
            .primary_flow()
            .expect("validated TX session carries a primary flow");
        let peer = session_flow
            .peer
            .expect("validated TX session carries a peer");
        let flow = session_flow
            .target_tx
            .expect("validated TX session carries a target TX flow");
        let duration_millis = match session.config.completion {
            HilCompletion::DurationMillis(duration) => duration,
            HilCompletion::TransferBytes(_) | HilCompletion::HostStop => {
                unreachable!("protocol owner accepts only duration-completed sessions")
            }
        };
        let server = Ipv4Addr::from(peer.address);
        let server_port = peer.port;
        let payload_bytes = usize::from(flow.payload_bytes);
        let duration = Duration::from_millis(u64::from(duration_millis));
        let offered_rate_bps = flow.offered_rate_bps;
        // Group pacing is a CPU-side load-generator property, not evidence of
        // a negotiated radio capability. The bounded socket/DATAPATH queues own
        // admission while interval telemetry independently proves whether TX
        // actually used BlockAck/A-MPDU. Waiting after every datagram makes
        // the Embassy timer itself the throughput ceiling at high offered
        // rates.
        let pacing_group_datagrams = flow
            .pacing_group_datagrams
            .unwrap_or(config.pacing_group_datagrams);
        runtime_log(format_args!(
            "OPEN_RADIO_PHY_HIL result=PASS stage=udp-tx-session-start \
             session={} target={server}:{server_port} payload={payload_bytes} \
             duration_ms={} offered_bps={offered_rate_bps:?}",
            session.session_id,
            duration.as_millis(),
        ));
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        if crate::product_hil::L1_CACHE_COUNTERS_ENABLED.load(Ordering::Relaxed) {
            l1_cache.enable();
        }
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        let cache_start = l1_cache.snapshot();
        #[cfg(feature = "mac-irq-telemetry")]
        let ingress_start = (
            qualification_sample(QualificationRequester::UdpTx)
                .await
                .rx_primary,
            crate::product_hil::RX_PIPELINE.snapshot(),
        );
        let started = Instant::now();
        let cpu_window = CpuLoadWindow::begin();
        let task_poll_start = TASK_POLLS.snapshot();
        #[cfg(feature = "mac-irq-telemetry")]
        let irq_start = crate::product_hil::MAC_IRQ.snapshot();
        #[cfg(feature = "task-poll-telemetry")]
        let network_start =
            crate::product_hil::network::observation::counters(config.network_interface).snapshot();
        #[cfg(feature = "task-poll-telemetry")]
        let send_wait = oer_hil_esp32s31_telemetry::wait::Counters::new();
        #[cfg(feature = "task-poll-telemetry")]
        let pacing_wait = oer_hil_esp32s31_telemetry::wait::Counters::new();
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let core0_performance_start = CORE0_PERFORMANCE.snapshot();
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        let tx_promotion_start = TX_PERFORMANCE.snapshot();
        // TX owns this diagnostic interval. Socket admission does not prove
        // radio completion; the terminal role report owns final MAC outcomes.
        let aggregate_start = if crate::product_hil::OPEN_RADIO_DRIVER_OBSERVATION
            && session.config.direction != HilDirection::Rx
        {
            qualification_sample(QualificationRequester::UdpTxBegin)
                .await
                .aggregate_tx
        } else {
            None
        };
        let (bytes, datagrams, send_errors, flow_evidence) = if session.config.active_flow_count()
            == 1
        {
            let mut next_send = started;
            let mut bytes = 0_u64;
            let mut datagrams = 0_u64;
            let mut send_errors = 0_u32;
            // One session timer bounds a permanently blocked socket admission.
            // Installing and cancelling a timeout for every datagram added about
            // 80 us to the 700-us 16-Mbit/s packet interval and made HIL itself
            // the measured throughput ceiling.
            let _session_elapsed = with_timeout(duration, async {
                loop {
                    let sequence = (datagrams as u32).to_be_bytes();
                    let publication =
                        socket.send_to_with(payload_bytes, (server, server_port), |packet| {
                            packet[..sequence.len()].copy_from_slice(&sequence);
                            if let Some(identity) = session_flow.payload_identity {
                                assert!(identity.write_to(packet));
                                assert!(
                                    oer_tool_data_hil_protocol::network::UdpSessionPayloadIdentity::fill_after_header(
                                        packet
                                    )
                                );
                            }
                            (payload_bytes, ())
                        });
                    #[cfg(feature = "task-poll-telemetry")]
                    let publication = send_wait.observe(publication, || Instant::now().as_micros());
                    match publication.await {
                        Ok(()) => {
                            bytes = bytes.saturating_add(payload_bytes as u64);
                            datagrams = datagrams.saturating_add(1);
                        }
                        Err(_) => {
                            send_errors = send_errors.saturating_add(1);
                            // A refused datagram completes without waiting;
                            // retrying it at once would never yield this core.
                            embassy_futures::yield_now().await;
                            continue;
                        }
                    }
                    if datagrams.is_multiple_of(u64::from(pacing_group_datagrams)) {
                        let Some(rate_bps) = offered_rate_bps else {
                            // An unpaced sender whose socket always admits
                            // must still let the network core's other tasks
                            // run once per group.
                            embassy_futures::yield_now().await;
                            continue;
                        };
                        // Enforce one byte-budget deadline per bounded socket-queue
                        // group. Keep small timer/executor lateness on the absolute
                        // schedule. A four-group token-bucket horizon prevents a
                        // genuine pause from becoming an unbounded line-rate burst.
                        let group_nanos = u64::from(pacing_group_datagrams)
                            .saturating_mul(u64::try_from(payload_bytes).unwrap_or(u64::MAX))
                            .saturating_mul(8_000_000_000)
                            .saturating_add(rate_bps - 1)
                            / rate_bps;
                        let group_duration = Duration::from_nanos(group_nanos);
                        next_send += group_duration;
                        let now = Instant::now();
                        if now < next_send {
                            let pacing = Timer::at(next_send);
                            #[cfg(feature = "task-poll-telemetry")]
                            let pacing = pacing_wait.observe(pacing, || Instant::now().as_micros());
                            pacing.await;
                        } else {
                            // An offer above the link keeps the sender behind
                            // its schedule, so no pacing timer is awaited:
                            // yield once per group so the network core's
                            // other tasks still run.
                            if now - next_send > group_duration * MAX_PACING_CATCH_UP_GROUPS {
                                next_send = now;
                            }
                            embassy_futures::yield_now().await;
                        }
                    }
                }
            })
            .await;
            let elapsed_micros = started.elapsed().as_micros().max(1);
            let transport = TransportEvidence {
                rx_maximum_silence_micros: None,
                rx_bytes: 0,
                tx_bytes: bytes,
                rx_units: 0,
                tx_units: datagrams,
                rx_late_bytes: 0,
                rx_late_units: 0,
                elapsed_micros,
                transport_errors: send_errors,
            };
            (
                bytes,
                datagrams,
                send_errors,
                [
                    Some(FlowTransportEvidence::from_session_total(
                        session_flow.flow_id,
                        transport,
                    )),
                    None,
                ],
            )
        } else {
            let flows = transmit_multi_flow(
                [&mut socket, &mut secondary_socket],
                session.config,
                started,
                duration,
                pacing_group_datagrams,
                config.multi_flow_burst_datagrams,
            )
            .await;
            let aggregate = TransportEvidence::from_flows(flows);
            (
                aggregate.tx_bytes,
                aggregate.tx_units,
                aggregate.transport_errors,
                flows,
            )
        };
        let cpu_load = cpu_window.finish();
        let elapsed_us = started.elapsed().as_micros().max(1);
        let task_poll_interval = OPEN_RADIO_TASK_POLL_TELEMETRY
            .then(|| TASK_POLLS.snapshot().wrapping_delta_since(task_poll_start));
        #[cfg(feature = "mac-irq-telemetry")]
        let irq_interval = crate::product_hil::MAC_IRQ
            .snapshot()
            .wrapping_delta_since(irq_start);
        #[cfg(feature = "task-poll-telemetry")]
        let network_interval =
            crate::product_hil::network::observation::counters(config.network_interface)
                .snapshot()
                .delta(network_start);
        let throughput_kbps = bytes
            .saturating_mul(8)
            .saturating_mul(1_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        // Snapshot the requested interval without guessing when stack/radio
        // queues have drained. Delivery is independently measured by the host.
        super::super::session_evidence_due();
        let qualification_end = qualification_sample(QualificationRequester::UdpTx).await;
        let tx_vector = qualification_end.tx_vector;
        #[cfg(feature = "mac-irq-telemetry")]
        let ingress_interval = (
            ingress_start
                .0
                .zip(qualification_end.rx_primary)
                .map(|(start, end)| end.wrapping_delta_since(start)),
            crate::product_hil::RX_PIPELINE
                .snapshot()
                .wrapping_delta_since(ingress_start.1),
        );
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        let cache_interval = l1_cache.snapshot().wrapping_delta_since(cache_start);
        // Freeze terminal evidence before any report can wait for USB capacity.
        // Reuse this same aggregate snapshot for text and structured evidence.
        let aggregate = qualification_end
            .aggregate_tx
            .zip(aggregate_start)
            .map(|(current, earlier)| current.wrapping_delta_since(earlier));
        // This live link vector belongs to the associated-STA datapath. AP
        // rate/A-MPDU evidence is owned by its terminal role report instead.
        // A station session that explicitly required BlockAck must never
        // succeed without the associated-link evidence it requested.
        assert!(
            !crate::product_hil::OPEN_RADIO_DRIVER_OBSERVATION
                || session.config.link_requirements.tx_block_ack_tid.is_none()
                || tx_vector.is_some(),
            "BlockAck-qualified TX session retains its associated link vector",
        );
        runtime_log(format_args!(
            "OTX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} \
             e={send_errors} p={} pg={} w={} r={} code={}",
            offered_rate_bps.unwrap_or(0) / 1_000,
            pacing_group_datagrams,
            tx_vector.map_or(0, |vector| vector.bandwidth_mhz),
            tx_vector.map_or(0, |vector| vector.aggregate_rate_kbps),
            config.code_address,
        ));
        #[cfg(feature = "mac-irq-telemetry")]
        crate::product_hil::traffic::reporting::log_tx_ingress(
            ingress_interval.0,
            ingress_interval.1,
        )
        .await;
        if let Some(aggregate) = aggregate {
            log_open_radio_ampdu_snapshot(aggregate).await;
        }
        if let Some(interval) = task_poll_interval {
            log_open_radio_task_poll_snapshot(interval).await;
        }
        #[cfg(feature = "task-poll-telemetry")]
        if session.config.active_flow_count() == 1 {
            crate::product_hil::network::observation::log(
                config.network_interface,
                network_interval,
                send_wait.snapshot(),
                pacing_wait.snapshot(),
            )
            .await;
        }
        #[cfg(feature = "mac-irq-telemetry")]
        crate::product_hil::traffic::reporting::log_mac_irq_interval(irq_interval).await;
        #[cfg(feature = "tx-wait-probe")]
        crate::product_hil::traffic::reporting::log_tx_wait_trace(
            &crate::product_hil::AGGREGATE_TX.wait_trace,
        )
        .await;
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        log_open_radio_core0_rx_coarse(core0_performance_start).await;
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        crate::product_hil::traffic::log_open_radio_tx_promotion(tx_promotion_start).await;
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        crate::product_hil::traffic::reporting::log_open_radio_l1_cache_interval(cache_interval)
            .await;
        let aggregate_evidence = aggregate
            .filter(|aggregate| aggregate.rate_selections != 0)
            .map(aggregate_tx_evidence);
        complete_open_radio_bidirectional_direction(
            config.session_source.results,
            OpenRadioBidirectionalResult::new(
                session.session_id,
                OpenRadioBidirectionalDirection::Tx,
                flow_evidence,
                aggregate_evidence.map(|(radio, _)| radio),
                aggregate_evidence.map(|(_, timing)| timing),
                None,
                None,
                cpu_load,
                if send_errors == 0 {
                    SessionVerdict::Passed
                } else {
                    SessionVerdict::Failed(SessionFailure::TransmitErrors(send_errors))
                },
            ),
        )
        .await;
    }
}
