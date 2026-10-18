#![forbid(unsafe_code)]

use core::sync::atomic::Ordering;
use oer_tool_data_hil_protocol::{network::SessionFailure, network::SessionVerdict};

use crate::product_hil::network::sockets::{Stack, UdpRxStorage, new_udp, recv_from_with};
use embassy_futures::{
    select::{Either, select},
    yield_now,
};
use embassy_time::Duration;
#[cfg(any(
    feature = "core0-rx-cycle-telemetry",
    feature = "core0-rx-coarse-telemetry"
))]
use oer_esp32s31_ieee80211_system::CORE0_PERFORMANCE;
#[cfg(feature = "core0-rx-cycle-telemetry")]
use oer_esp32s31_ieee80211_system::{
    CORE0_AP_RX_CYCLES, CORE0_REORDER_CYCLES, CORE0_RX_CYCLES, CORE0_RX_SERVICE_HISTOGRAM,
};
#[cfg(feature = "core0-rx-cycle-telemetry")]
use oer_adapters_esp_hal_soc::L1CachePerformanceCounters;
use oer_hil_esp32s31_telemetry::{
    rx_evidence::{RX_HE_MCS_BUCKETS, RX_HT_MCS_BUCKETS, RxPhySnapshot, RxSmpduSnapshot},
    rx_pipeline::RxPipelineCounters,
    task_poll::TaskPollSet,
};
#[cfg(feature = "rx-delivery-telemetry")]
use oer_tool_data_hil_protocol::network::RxReorderDeliveryEvidence;
use oer_tool_data_hil_protocol::{
    network::Direction as HilDirection, network::RadioEvidence, network::RxRadioEvidence,
    network::RxZeroCopyEvidence, network::ServiceInfo, network::SessionReady,
    network::Transport as HilTransport,
};

#[cfg(feature = "core0-rx-coarse-telemetry")]
use crate::product_hil::traffic::log_open_radio_core0_rx_coarse;
use crate::product_hil::traffic::reporting::log_open_radio_task_poll_snapshot;
#[cfg(feature = "core0-rx-cycle-telemetry")]
use crate::product_hil::traffic::{
    log_open_radio_core0_rx_cycles, log_open_radio_core0_rx_service_histogram,
};
use crate::{
    console::{publish_event_reliably, runtime_log},
    product_hil::{
        QualificationRequester, qualification_sample, rx_qualification,
        traffic::{
            BidirectionalResultChannel, BidirectionalSessionChannel,
            OpenRadioBidirectionalDirection, OpenRadioBidirectionalResult,
            complete_open_radio_bidirectional_direction, iperf2_udp_sequence,
            log_open_radio_rx_pipeline_interval,
        },
    },
};

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpRxSessionSource {
    pub sessions: &'static BidirectionalSessionChannel,
    pub results: &'static BidirectionalResultChannel,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpRxBenchmarkConfig {
    pub network_interface: oer_tool_data_hil_protocol::wifi::WifiNetworkInterface,
    pub local_port: u16,
    pub queue_depth: usize,
    pub payload_capacity: usize,
    pub silence_threshold: Duration,
    pub task_poll_telemetry: bool,
    pub code_address: usize,
    pub session_source: UdpRxSessionSource,
}

#[derive(Clone, Copy)]
pub(in crate::product_hil) struct UdpRxTelemetry {
    pub pipeline: &'static RxPipelineCounters,
    pub task_polls: &'static TaskPollSet,
    #[cfg(feature = "core0-rx-cycle-telemetry")]
    pub l1_cache: &'static L1CachePerformanceCounters,
}

mod receive;

pub(in crate::product_hil) async fn run_open_radio_udp_rx_benchmark<'a>(
    stack: Stack<'a>,
    storage: &'a mut UdpRxStorage,
    config: UdpRxBenchmarkConfig,
    telemetry: UdpRxTelemetry,
) -> ! {
    let mut socket = new_udp(stack, storage);
    socket
        .bind(
            config.local_port,
            embassy_net::wire::ListenSocketAddr::UNSPECIFIED,
        )
        .unwrap_or_else(|error| panic!("production UDP RX socket bind failed: {error:?}"));
    publish_event_reliably(
        0,
        0,
        oer_tool_data_hil_protocol::network::ServiceReady(ServiceInfo {
            network_interface: config.network_interface,
            transport: HilTransport::Udp,
            direction: HilDirection::Rx,
            local_port: config.local_port,
            maximum_payload_bytes: config.payload_capacity as u16,
        }),
    )
    .await;
    runtime_log(format_args!(
        "OPEN_RADIO_HIL result=PASS stage=udp-rx-ready port={} queue={} payload={}",
        config.local_port, config.queue_depth, config.payload_capacity,
    ));

    loop {
        // A bound UDP socket is not proof that the host neighbor entry and the
        // complete Wi-Fi/IP ingress path are live. Before admitting a measured
        // session, consume an out-of-band negative-sequence probe and publish
        // a second typed readiness edge. Positive packets outside a session
        // are deliberately discarded instead of contaminating evidence.
        let session = loop {
            match select(
                receive_rx_session(config.session_source),
                recv_from_with(&mut socket, |packet, _| iperf2_udp_sequence(packet)),
            )
            .await
            {
                Either::First(session) => break session,
                Either::Second(Ok(Some(sequence))) if sequence < 0 => {
                    publish_event_reliably(
                        0,
                        0,
                        oer_tool_data_hil_protocol::network::ServiceReady(ServiceInfo {
                            network_interface: config.network_interface,
                            transport: HilTransport::Udp,
                            direction: HilDirection::Rx,
                            local_port: config.local_port,
                            maximum_payload_bytes: config.payload_capacity as u16,
                        }),
                    )
                    .await;
                }
                Either::Second(_) => {}
            }
        };
        #[cfg(feature = "rx-delivery-telemetry")]
        if session.config.active_flow_count() == 1 {
            rx_qualification::HilConnectedRxObserver::begin_delivery_session(session.session_id);
        }
        #[cfg(feature = "station-exit-evidence")]
        rx_qualification::begin_anomalies(session.session_id);
        publish_event_reliably(
            session.session_id,
            0,
            SessionReady {
                direction: HilDirection::Rx,
                tx_block_ack_tid: None,
            },
        )
        .await;
        yield_now().await;

        #[cfg(feature = "core0-rx-cycle-telemetry")]
        if crate::product_hil::L1_CACHE_COUNTERS_ENABLED.load(Ordering::Relaxed) {
            telemetry.l1_cache.enable();
        }
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        let cache_start = telemetry.l1_cache.snapshot();
        let qualification_start = qualification_sample(QualificationRequester::UdpRx).await;
        let hardware_start = qualification_start.rx_primary;
        let irq_start = qualification_start.rx_interrupt_posts;
        let irq_classification_start = crate::product_hil::MAC_IRQ.snapshot();
        let _ = crate::product_hil::MAC_IRQ.take_auxiliary_entries();
        let _ = crate::product_hil::MAC_IRQ.take_unhandled_entries();
        let pipeline_start = telemetry.pipeline.snapshot();
        let zero_copy_start = zero_copy::start();
        let task_poll_start = telemetry.task_polls.snapshot();
        #[cfg(feature = "pc-profile")]
        crate::pc_profile::window_begin();
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        let core0_rx_cycle_start = CORE0_RX_CYCLES.snapshot();
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        let core0_ap_rx_cycle_start = CORE0_AP_RX_CYCLES.snapshot();
        #[cfg(any(
            feature = "core0-rx-cycle-telemetry",
            feature = "core0-rx-coarse-telemetry"
        ))]
        let core0_performance_start = CORE0_PERFORMANCE.snapshot();
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        let core0_reorder_start = CORE0_REORDER_CYCLES.snapshot();
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        let core0_rx_service_start = CORE0_RX_SERVICE_HISTOGRAM.snapshot();
        rx_qualification::LAST_FORMAT.store(u32::MAX, Ordering::Relaxed);
        rx_qualification::LAST_PHY.store(u32::MAX, Ordering::Relaxed);
        let phy_start = rx_qualification::RX_PHY.snapshot();
        let s_mpdu_start = rx_qualification::RX_S_MPDU.snapshot();
        let beacon_s_mpdu_start = rx_qualification::BEACON_S_MPDU.snapshot();
        let ampdu_start = rx_qualification::RX_AMPDU.snapshot();
        let outcome = receive::run(
            &mut socket,
            session,
            config.silence_threshold,
            telemetry.task_polls,
        )
        .await;
        let receive::Outcome {
            bytes,
            datagrams,
            mut receive_errors,
            terminal_seen,
            sequence,
            mut flow_evidence,
            elapsed_us,
            task_poll_end,
            cpu_load,
        } = outcome;
        super::super::session_evidence_due();
        let qualification_end = qualification_sample(QualificationRequester::UdpRx).await;
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        let cache_interval = telemetry
            .l1_cache
            .snapshot()
            .wrapping_delta_since(cache_start);
        let hardware = qualification_end
            .rx_primary
            .zip(hardware_start)
            .map(|(current, earlier)| current.wrapping_delta_since(earlier))
            .unwrap_or_default();
        let rx_irq_posts = qualification_end.rx_interrupt_posts.wrapping_sub(irq_start);
        let irq_classification = crate::product_hil::MAC_IRQ
            .snapshot()
            .wrapping_delta_since(irq_classification_start);
        let mac_irq_entries = irq_classification
            .spurious_entries
            .saturating_add(irq_classification.rx_only_entries)
            .saturating_add(irq_classification.rx_mixed_entries)
            .saturating_add(irq_classification.tx_only_entries)
            .saturating_add(irq_classification.tx_mixed_entries)
            .saturating_add(irq_classification.other_only_entries);
        let irq_auxiliary_entries = crate::product_hil::MAC_IRQ.take_auxiliary_entries();
        let irq_unhandled_entries = crate::product_hil::MAC_IRQ.take_unhandled_entries();
        #[cfg(feature = "station-exit-evidence")]
        let anomalies = rx_qualification::end_anomalies(session.session_id);
        let pipeline_end = telemetry.pipeline.snapshot();
        let zero_copy = zero_copy::finish(zero_copy_start);
        let pipeline_interval = pipeline_end.wrapping_delta_since(pipeline_start);
        #[cfg(feature = "rx-delivery-telemetry")]
        let rx_delivery = (session.config.active_flow_count() == 1)
            .then(|| {
                let reorder = pipeline_interval;
                rx_qualification::HilConnectedRxObserver::finish_delivery_session(
                    session.session_id,
                    RxReorderDeliveryEvidence {
                        ingress: reorder.reorder_ingress,
                        ingress_retries: reorder.reorder_ingress_retries,
                        direct: reorder.reorder_direct,
                        buffered: reorder.reorder_buffered,
                        released: reorder.reorder_released,
                        missing: reorder.reorder_missing,
                        stale: reorder.reorder_stale,
                        gap_expiries: reorder.reorder_gap_expiries,
                        maximum_occupied: reorder.reorder_maximum_occupied,
                        discarded: reorder.reorder_discarded,
                    },
                )
            })
            .flatten();
        #[cfg(not(feature = "rx-delivery-telemetry"))]
        let rx_delivery = None;
        let queue_dropped = pipeline_end
            .network_dropped
            .wrapping_sub(pipeline_start.network_dropped);
        receive_errors = receive_errors.saturating_add(queue_dropped);
        if let Some(primary) = flow_evidence.iter_mut().flatten().next() {
            primary.transport_errors = primary.transport_errors.saturating_add(queue_dropped);
        }
        let throughput_kbps = bytes
            .saturating_mul(8_000)
            .checked_div(elapsed_us)
            .unwrap_or(0);
        runtime_log(format_args!(
            "ORX b={bytes} d={datagrams} u={elapsed_us} k={throughput_kbps} e={receive_errors} \
             terminal={} first={} highest={} missing={} backward={} duplicate={} gap_us={} \
             mpdu={} success={} fcs={} full={} overflow={} irq={} code={}",
            u8::from(terminal_seen),
            sequence.first.unwrap_or(u32::MAX),
            sequence.highest,
            sequence.forward_missing,
            sequence.backward,
            sequence.adjacent_duplicates,
            sequence.maximum_interarrival_micros,
            hardware.mpdu_count,
            hardware.data_success,
            hardware.fcs_error,
            hardware.buffer_full,
            hardware.fifo_overflow,
            rx_irq_posts,
            config.code_address,
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXHW abort={} abort_fcs_pass={} power_drop={} he_sig_b={} same_bm={} \
             signal={} end={} other_unicast={} tkip={} bt_block={} frequency_hop={} \
             unmatched={} ack_irq={} rts_irq={}",
            hardware.abort,
            hardware.abort_fcs_pass,
            hardware.power_drop_error,
            hardware.he_sig_b_error,
            hardware.same_bm_error,
            hardware.signal_field,
            hardware.end,
            hardware.other_unicast,
            hardware.tkip_error,
            hardware.bt_block_error,
            hardware.frequency_hop_error,
            hardware.last_unmatched_error,
            hardware.ack_interrupt,
            hardware.rts_interrupt,
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXDEC brx_agc={} brx={} nrx={} abort={} agc_exit={} bb_off={} fdm_watchdog={} \
             restart={} service={} tx_over={} unsupported={} he_format={} ht_sig={} \
             he_unsupported={} he_sig_a_crc={}",
            hardware.brx_agc_error,
            hardware.brx_error,
            hardware.nrx_error,
            hardware.nrx_abort,
            hardware.nrx_agc_exit,
            hardware.nrx_baseband_off,
            hardware.nrx_fdm_watchdog,
            hardware.nrx_restart,
            hardware.nrx_service,
            hardware.nrx_tx_over,
            hardware.nrx_unsupported,
            hardware.nrx_he_format,
            hardware.nrx_ht_sig,
            hardware.nrx_he_unsupported,
            hardware.nrx_he_sig_a_crc,
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXHANG rx={} tx={} combined={} panic={}",
            hardware.rx_hang, hardware.tx_hang, hardware.rx_tx_hang, hardware.rx_tx_panic,
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXP f={} p={}",
            rx_qualification::LAST_FORMAT.load(Ordering::Relaxed),
            rx_qualification::LAST_PHY.load(Ordering::Relaxed),
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXQ first={} highest={} next={} gap_events={} forward_missing={} \
             maximum_gap={} maximum_gap_at={} first_gap_at={} last_gap_at={} backward={} \
             adjacent_duplicates={} unsequenced={} maximum_interarrival_us={} \
             maximum_interarrival_at={}",
            sequence.first.unwrap_or(u32::MAX),
            sequence.first.map(|_| sequence.highest).unwrap_or(u32::MAX),
            sequence
                .first
                .map(|_| sequence.expected)
                .unwrap_or(u32::MAX),
            sequence.gap_events,
            sequence.forward_missing,
            sequence.maximum_gap,
            sequence.maximum_gap_at.unwrap_or(u32::MAX),
            sequence.first_gap_at.unwrap_or(u32::MAX),
            sequence.last_gap_at.unwrap_or(u32::MAX),
            sequence.backward,
            sequence.adjacent_duplicates,
            sequence.unsequenced,
            sequence.maximum_interarrival_micros,
            sequence.maximum_interarrival_at.unwrap_or(u32::MAX),
        ));
        yield_now().await;
        let (s_mpdu_end, phy_end) = settled_benchmark_counters().await;
        let rx_s_mpdu = s_mpdu_end.wrapping_delta_since(s_mpdu_start);
        let beacon_s_mpdu = rx_qualification::BEACON_S_MPDU
            .snapshot()
            .wrapping_delta_since(beacon_s_mpdu_start);
        let rx_ampdu = rx_qualification::RX_AMPDU
            .snapshot()
            .wrapping_delta_since(ampdu_start);
        runtime_log(format_args!(
            "ORXSM s_mpdu={} not_s_mpdu={} unavailable={} beacon_s_mpdu={} \
             beacon_not_s_mpdu={} beacon_unavailable={}",
            rx_s_mpdu.s_mpdu_frames,
            rx_s_mpdu.not_s_mpdu_frames,
            rx_s_mpdu.unavailable_frames,
            beacon_s_mpdu.s_mpdu_frames,
            beacon_s_mpdu.not_s_mpdu_frames,
            beacon_s_mpdu.unavailable_frames,
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXAG ampdu={} not_ampdu={} hardware_ampdu={} hardware_not_ampdu={} \
             protocol_ampdu={} protocol_not_ampdu={} unavailable={}",
            rx_ampdu.ampdu_frames,
            rx_ampdu.not_ampdu_frames,
            rx_ampdu.hardware_ampdu_frames,
            rx_ampdu.hardware_not_ampdu_frames,
            rx_ampdu.protocol_ampdu_frames,
            rx_ampdu.protocol_not_ampdu_frames,
            rx_ampdu.unavailable_frames,
        ));
        yield_now().await;
        let mcs = core::array::from_fn::<_, RX_HE_MCS_BUCKETS, _>(|index| {
            phy_end.he_mcs[index].wrapping_sub(phy_start.he_mcs[index])
        });
        runtime_log(format_args!(
            "ORXM m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={} m8={} m9={} \
             m10={} m11={} other={}",
            mcs[0],
            mcs[1],
            mcs[2],
            mcs[3],
            mcs[4],
            mcs[5],
            mcs[6],
            mcs[7],
            mcs[8],
            mcs[9],
            mcs[10],
            mcs[11],
            phy_end.other.wrapping_sub(phy_start.other),
        ));
        yield_now().await;
        let ht40_long_gi_mcs = core::array::from_fn::<_, RX_HT_MCS_BUCKETS, _>(|index| {
            phy_end.ht40_long_gi_mcs[index].wrapping_sub(phy_start.ht40_long_gi_mcs[index])
        });
        let ht40_short_gi_mcs = core::array::from_fn::<_, RX_HT_MCS_BUCKETS, _>(|index| {
            phy_end.ht40_short_gi_mcs[index].wrapping_sub(phy_start.ht40_short_gi_mcs[index])
        });
        let ht20_mcs = core::array::from_fn::<_, RX_HT_MCS_BUCKETS, _>(|index| {
            phy_end.ht20_mcs[index].wrapping_sub(phy_start.ht20_mcs[index])
        });
        let ht_other = phy_end.ht_other.wrapping_sub(phy_start.ht_other);
        let ht40_long_gi_frames = ht40_long_gi_mcs.iter().copied().sum();
        let ht40_short_gi_frames = ht40_short_gi_mcs.iter().copied().sum();
        let ht40_below_mcs7_frames = ht40_long_gi_mcs[..7]
            .iter()
            .chain(&ht40_short_gi_mcs[..7])
            .copied()
            .sum();
        let ht_invalid_frames = ht20_mcs
            .iter()
            .chain(&mcs)
            .copied()
            .sum::<u32>()
            .saturating_add(ht_other)
            .saturating_add(phy_end.other.wrapping_sub(phy_start.other));
        runtime_log(format_args!(
            "ORXHTL m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={}",
            ht40_long_gi_mcs[0],
            ht40_long_gi_mcs[1],
            ht40_long_gi_mcs[2],
            ht40_long_gi_mcs[3],
            ht40_long_gi_mcs[4],
            ht40_long_gi_mcs[5],
            ht40_long_gi_mcs[6],
            ht40_long_gi_mcs[7],
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXHTS m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={}",
            ht40_short_gi_mcs[0],
            ht40_short_gi_mcs[1],
            ht40_short_gi_mcs[2],
            ht40_short_gi_mcs[3],
            ht40_short_gi_mcs[4],
            ht40_short_gi_mcs[5],
            ht40_short_gi_mcs[6],
            ht40_short_gi_mcs[7],
        ));
        yield_now().await;
        runtime_log(format_args!(
            "ORXHTW m0={} m1={} m2={} m3={} m4={} m5={} m6={} m7={} other={}",
            ht20_mcs[0],
            ht20_mcs[1],
            ht20_mcs[2],
            ht20_mcs[3],
            ht20_mcs[4],
            ht20_mcs[5],
            ht20_mcs[6],
            ht20_mcs[7],
            ht_other,
        ));
        yield_now().await;
        log_open_radio_rx_pipeline_interval(
            pipeline_start,
            rx_irq_posts,
            mac_irq_entries,
            irq_classification,
            irq_auxiliary_entries,
            irq_unhandled_entries,
            telemetry.pipeline,
        )
        .await;
        if config.task_poll_telemetry {
            log_open_radio_task_poll_snapshot(task_poll_end.wrapping_delta_since(task_poll_start))
                .await;
        }
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        log_open_radio_core0_rx_cycles(
            core0_rx_cycle_start,
            core0_ap_rx_cycle_start,
            core0_performance_start,
            core0_reorder_start,
            cache_interval,
        )
        .await;
        #[cfg(feature = "core0-rx-cycle-telemetry")]
        log_open_radio_core0_rx_service_histogram(&core0_rx_service_start).await;
        #[cfg(feature = "core0-rx-coarse-telemetry")]
        log_open_radio_core0_rx_coarse(core0_performance_start).await;
        // The first check that failed, in the order the verdict evaluates.
        let verdict = if datagrams == 0 {
            SessionVerdict::Failed(SessionFailure::NoDatagrams)
        } else if !terminal_seen {
            SessionVerdict::Failed(SessionFailure::NoTerminal {
                received: u32::try_from(datagrams).unwrap_or(u32::MAX),
                highest_sequence: sequence.highest,
            })
        } else if receive_errors != 0 {
            SessionVerdict::Failed(SessionFailure::ReceiveErrors(receive_errors))
        } else {
            SessionVerdict::Passed
        };
        let radio = crate::product_hil::OPEN_RADIO_DRIVER_OBSERVATION.then_some(RadioEvidence {
            rx: Some(RxRadioEvidence {
                phy_format: u8::try_from(rx_qualification::LAST_FORMAT.load(Ordering::Relaxed))
                    .unwrap_or(u8::MAX),
                ht40_long_gi_frames,
                ht40_short_gi_frames,
                ht40_below_mcs7_frames,
                ht_invalid_frames,
                dma_buffer_full: u32::from(hardware.buffer_full),
                dma_fifo_overflow: u32::from(hardware.fifo_overflow),
                network_dropped: queue_dropped,
                irq_drain_saturated: irq_classification.saturated_entries,
                unhandled_irq_entries: irq_unhandled_entries,
                sequence_first: sequence.first,
                sequence_highest: sequence.first.map(|_| sequence.highest),
                sequence_gap_events: sequence.gap_events,
                sequence_forward_missing: sequence.forward_missing,
                sequence_backward: sequence.backward,
                sequence_duplicates: sequence.adjacent_duplicates,
                sequence_unsequenced: sequence.unsequenced,
                s_mpdu_datagrams: rx_s_mpdu.s_mpdu_frames,
                not_s_mpdu_datagrams: rx_s_mpdu.not_s_mpdu_frames,
                s_mpdu_unavailable_datagrams: rx_s_mpdu.unavailable_frames,
                s_mpdu_beacons: beacon_s_mpdu.s_mpdu_frames,
                not_s_mpdu_beacons: beacon_s_mpdu.not_s_mpdu_frames,
                s_mpdu_unavailable_beacons: beacon_s_mpdu.unavailable_frames,
                ampdu_datagrams: rx_ampdu.ampdu_frames,
                not_ampdu_datagrams: rx_ampdu.not_ampdu_frames,
                hardware_ampdu_datagrams: rx_ampdu.hardware_ampdu_frames,
                hardware_not_ampdu_datagrams: rx_ampdu.hardware_not_ampdu_frames,
                protocol_ampdu_datagrams: rx_ampdu.protocol_ampdu_frames,
                protocol_not_ampdu_datagrams: rx_ampdu.protocol_not_ampdu_frames,
                ampdu_unavailable_datagrams: rx_ampdu.unavailable_frames,
                reorder_tid: u8::try_from(pipeline_interval.reorder_last_start >> 26 & 0x07)
                    .unwrap_or(u8::MAX),
                reorder_window: u16::try_from(pipeline_interval.reorder_last_start >> 16 & 0x03ff)
                    .unwrap_or(u16::MAX),
                reorder_first_samples: pipeline_interval.reorder_first_samples,
                reorder_first_tid: u8::try_from(pipeline_interval.reorder_last_first >> 24 & 0x0f)
                    .unwrap_or(u8::MAX),
                reorder_first_start: u16::try_from(
                    pipeline_interval.reorder_last_first >> 12 & 0x0fff,
                )
                .unwrap_or(u16::MAX),
                reorder_first_sequence: u16::try_from(
                    pipeline_interval.reorder_last_first & 0x0fff,
                )
                .unwrap_or(u16::MAX),
                reorder_first_distance: u16::try_from(
                    pipeline_interval.reorder_last_first_distance,
                )
                .unwrap_or(u16::MAX),
                reorder_current_occupied: pipeline_interval.reorder_current_occupied,
                reorder_maximum_occupied: pipeline_interval.reorder_maximum_occupied,
                rx_service_calls: pipeline_interval.service_calls,
                rx_frontier_histogram_samples: pipeline_interval
                    .frontier_zero_services
                    .saturating_add(pipeline_interval.frontier_one_services)
                    .saturating_add(pipeline_interval.frontier_two_three_services)
                    .saturating_add(pipeline_interval.frontier_four_seven_services)
                    .saturating_add(pipeline_interval.frontier_eight_fifteen_services)
                    .saturating_add(pipeline_interval.frontier_sixteen_thirty_one_services)
                    .saturating_add(pipeline_interval.frontier_thirty_two_plus_services),
                mac_irq_entries,
                mac_irq_classified_entries: mac_irq_entries,
            }),
            tx: None,
        });
        #[cfg(feature = "station-exit-evidence")]
        rx_qualification::report_anomalies(session.session_id, anomalies).await;
        complete_open_radio_bidirectional_direction(
            config.session_source.results,
            OpenRadioBidirectionalResult::new(
                session.session_id,
                OpenRadioBidirectionalDirection::Rx,
                flow_evidence,
                radio,
                None,
                rx_delivery,
                zero_copy,
                cpu_load,
                verdict,
            ),
        )
        .await;
    }
}

async fn receive_rx_session(source: UdpRxSessionSource) -> crate::console::ActiveSession {
    source.sessions.receive().await
}

/// Idle intervals a settled counter pair must survive, and at most how many
/// intervals to wait for one.
const COUNTER_SETTLE_INTERVAL: Duration = Duration::from_micros(200);
const COUNTER_SETTLE_ATTEMPTS: u32 = 50;

/// The benchmark datagram and PHY-vector counters, read once neither moves.
///
/// RX dispatch counts each benchmark datagram in both, one after the other,
/// on another task. A pair read while a late datagram is being observed can
/// hold it in one counter and not the other, so the interval check compares
/// a pair read across an idle interval.
async fn settled_benchmark_counters() -> (RxSmpduSnapshot, RxPhySnapshot) {
    let read = || {
        (
            rx_qualification::RX_S_MPDU.snapshot(),
            rx_qualification::RX_PHY.snapshot(),
        )
    };
    let mut previous = read();
    for _ in 0..COUNTER_SETTLE_ATTEMPTS {
        embassy_time::Timer::after(COUNTER_SETTLE_INTERVAL).await;
        let current = read();
        if current == previous {
            return current;
        }
        previous = current;
    }
    previous
}

/// Zero-copy RX accounting of one measured session.
///
/// The session restarts the origin's peak so `peak_held` is its own; the
/// counts are the interval's.
mod zero_copy {
    use super::RxZeroCopyEvidence;

    pub(super) fn start() -> oer_adapters_embassy_net::ExternalRxCounters {
        let origin = oer_esp32s31_ieee80211_system::rx_zero_copy_origin();
        let start = origin.counters();
        origin.restart_peak();
        start
    }

    pub(super) fn finish(
        start: oer_adapters_embassy_net::ExternalRxCounters,
    ) -> Option<RxZeroCopyEvidence> {
        let origin = oer_esp32s31_ieee80211_system::rx_zero_copy_origin();
        let interval = origin.counters().wrapping_delta_since(start);
        let level = |slots: usize| u16::try_from(slots).unwrap_or(u16::MAX);
        Some(RxZeroCopyEvidence {
            cap: level(origin.cap()),
            adopted: interval.adopted,
            copied_over_cap: interval.copied_over_cap,
            copied_unfit: interval.copied_unfit,
            dropped: interval.dropped,
            held_at_end: level(interval.held),
            peak_held: level(interval.peak_held),
        })
    }
}
