struct DeferredAccessPointRxSink<'storage> {
    frames: crate::datapath::rx::ethernet::PackedEthernetWriter<'storage>,
    exhausted: bool,
}

impl<'storage> DeferredAccessPointRxSink<'storage> {
    fn new(storage: &'storage mut [u8]) -> Self {
        Self {
            frames: crate::datapath::rx::ethernet::PackedEthernetWriter::new(storage),
            exhausted: false,
        }
    }

    const fn used(&self) -> usize {
        self.frames.used()
    }
}

impl ApRxSink for DeferredAccessPointRxSink<'_> {
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_ap_rx_sink")
    )]
    fn publish(&mut self, event: ApRxEvent<'_>) {
        if self.frames.push(event.frame).is_err() {
            self.exhausted = true;
        }
    }
}

/// Captures one ordinary Ethernet view as offsets inside its staging owner.
/// The owner is converted in place only after the AP dispatcher and reorder
/// state have accepted the frame.
struct InPlaceAccessPointRxSink {
    raw_start: usize,
    raw_length: usize,
    publication: Option<StagedEthernetPublication>,
    unsupported: bool,
}

impl InPlaceAccessPointRxSink {
    fn new(raw: &[u8]) -> Self {
        Self {
            raw_start: raw.as_ptr() as usize,
            raw_length: raw.len(),
            publication: None,
            unsupported: false,
        }
    }
}

impl ApRxSink for InPlaceAccessPointRxSink {
    fn publish(&mut self, event: ApRxEvent<'_>) {
        let payload_start = event.frame.payload.as_ptr() as usize;
        let payload_end = match payload_start.checked_add(event.frame.payload.len()) {
            Some(end) => end,
            None => {
                self.unsupported = true;
                return;
            }
        };
        let raw_end = self.raw_start.saturating_add(self.raw_length);
        if event.amsdu
            || self.publication.is_some()
            || payload_start < self.raw_start
            || payload_end > raw_end
        {
            self.unsupported = true;
            return;
        }
        self.publication = Some(StagedEthernetPublication {
            destination: event.frame.destination,
            source: event.frame.source,
            ether_type: event.frame.ether_type,
            payload_offset: payload_start - self.raw_start,
            payload_length: event.frame.payload.len(),
            metadata: event.metadata,
        });
    }
}

#[cfg(test)]
#[path = "rx_dispatch/in_place_rx_sink_tests.rs"]
mod in_place_rx_sink_tests;

fn observe_protected_dispatch(
    dispatch: ApRxDispatch,
    peer: Option<[u8; 6]>,
    #[cfg(any(feature = "diagnostics", test))] report: &mut AccessPointControlObservation,
    activity_peer: &mut Option<[u8; 6]>,
) -> bool {
    match dispatch {
        ApRxDispatch::Data {
            ethernet_frames, ..
        } => {
            if ethernet_frames != 0 {
                *activity_peer = peer;
                true
            } else {
                false
            }
        }
        ApRxDispatch::FragmentBuffered { .. } => false,
        ApRxDispatch::Duplicate => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_duplicates =
                    report.protected_data_duplicates.saturating_add(1);
            }
            false
        }
        ApRxDispatch::ForeignPeer => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_foreign = report.protected_data_foreign.saturating_add(1);
            }
            false
        }
        ApRxDispatch::Unauthorized => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_unauthorized =
                    report.protected_data_unauthorized.saturating_add(1);
            }
            false
        }
        ApRxDispatch::Rejected(ApRxError::Radio(_)) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_radio_rejected =
                    report.protected_data_radio_rejected.saturating_add(1);
            }
            false
        }
        ApRxDispatch::Rejected(ApRxError::Data(_)) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_protocol_rejected =
                    report.protected_data_protocol_rejected.saturating_add(1);
            }
            false
        }
        ApRxDispatch::Rejected(ApRxError::SecurityModeMismatch) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.security_mode_mismatches = report.security_mode_mismatches.saturating_add(1);
            }
            false
        }
        ApRxDispatch::Rejected(
            ApRxError::PeerQosMismatch
            | ApRxError::PairwiseKeyId(_)
            | ApRxError::Replay(_)
            | ApRxError::KeyGenerationMismatch
            | ApRxError::Fragment(_),
        ) => {
            #[cfg(any(feature = "diagnostics", test))]
            {
                report.protected_data_protocol_rejected =
                    report.protected_data_protocol_rejected.saturating_add(1);
            }
            false
        }
    }
}

/// Hot AP data-dispatch leaf shared by direct and retained reorder releases.
/// It owns no hardware capability and reports only value/borrowed protocol
/// outcomes to its caller.
struct AccessPointProtectedFrameDispatch;

impl AccessPointProtectedFrameDispatch {
    #[cfg_attr(
        all(target_arch = "riscv32", not(feature = "task-poll-telemetry")),
        unsafe(link_section = ".hot.text.open_radio_ap_rx_dispatch")
    )]
    #[inline(never)]
    fn dispatch(
        data_rx: &mut ApRxDispatcher,
        ordered: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>,
        mut admit: impl FnMut(ApRxAdmissionRequest) -> ApRxAdmission,
        peer: Option<[u8; 6]>,
        current_buffer: usize,
        current_is_amsdu: bool,
        now: oer_time::Instant,
        deferred: &mut DeferredAccessPointRxSink<'_>,
        in_place: &mut InPlaceAccessPointRxSink,
        #[cfg(any(feature = "diagnostics", test))]
        report: &mut AccessPointControlObservation,
        activity_peer: &mut Option<[u8; 6]>,
        produced_data: &mut bool,
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx: &mut crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile,
    ) {
        #[cfg(feature = "task-poll-telemetry")]
        let phase_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        #[cfg(feature = "task-poll-telemetry")]
        let publish_check_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_peer(now.wrapping_sub(phase_started));
            now
        };
        let current = ordered.buffer.as_ptr() as usize == current_buffer;
        let current_can_publish_in_place =
            can_publish_ap_rx_in_place(current, current_is_amsdu, deferred.used());
        let may_publish_in_place = data_rx.may_publish_in_place(ordered);
        #[cfg(feature = "task-poll-telemetry")]
        let deferred_before = deferred.used();
        #[cfg(feature = "task-poll-telemetry")]
        let body_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_publish_check(now.wrapping_sub(publish_check_started));
            now
        };
        #[cfg(feature = "task-poll-telemetry")]
        let mut admission_cycles = 0_u32;
        let mut measured_admit = |request| {
            #[cfg(feature = "task-poll-telemetry")]
            let started = crate::diagnostics::core0_rx_cycles::cycle_count();
            let outcome = admit(request);
            #[cfg(feature = "task-poll-telemetry")]
            {
                admission_cycles = admission_cycles.wrapping_add(
                    crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
                );
            }
            outcome
        };
        let outcome = if may_publish_in_place && current_can_publish_in_place {
            data_rx.dispatch_at(ordered, now, &mut measured_admit, in_place)
        } else {
            data_rx.dispatch_at(ordered, now, &mut measured_admit, deferred)
        };
        #[cfg(feature = "task-poll-telemetry")]
        crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_ingress_path(
            may_publish_in_place && current_can_publish_in_place,
            in_place.publication.is_some(),
            deferred.used() != deferred_before,
            false,
        );
        #[cfg(feature = "task-poll-telemetry")]
        let observe_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_body(now.wrapping_sub(body_started));
            core0_ap_rx.record_leaf_admission(admission_cycles);
            now
        };
        #[cfg(any(feature = "diagnostics", test))]
        {
            report.record_dispatch_rejection(outcome, ordered, now.as_micros());
            if deferred.exhausted {
                report.record_rx_rejection(
                    AccessPointRxRejectionReason::DeferredOutputCapacity,
                    ordered,
                    now.as_micros(),
                );
            } else if in_place.unsupported {
                report.record_rx_rejection(
                    AccessPointRxRejectionReason::InPlaceOutputUnsupported,
                    ordered,
                    now.as_micros(),
                );
            }
        }
        *produced_data |= observe_protected_dispatch(
            outcome,
            peer,
            #[cfg(any(feature = "diagnostics", test))]
            report,
            activity_peer,
        );
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.record_leaf_observe(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(observe_started),
        );
    }

    #[cfg_attr(
        all(target_arch = "riscv32", not(feature = "task-poll-telemetry")),
        unsafe(link_section = ".hot.text.open_radio_ap_rx_dispatch")
    )]
    #[inline(never)]
    fn dispatch_ordinary(
        data_rx: &mut ApRxDispatcher,
        ordered: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>,
        mut admit: impl FnMut(
            oer_esp32s31_ieee80211_ap::rx::ApOrdinaryPairwiseRxRequest,
        ) -> ApRxAdmission,
        _peer: [u8; 6],
        #[cfg(any(feature = "diagnostics", test))] now: oer_time::Instant,
        in_place: &mut InPlaceAccessPointRxSink,
        #[cfg(any(feature = "diagnostics", test))]
        report: &mut AccessPointControlObservation,
        #[cfg(any(feature = "diagnostics", test))] produced_data: &mut bool,
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx: &mut crate::diagnostics::core0_ap_rx_cycles::Core0ApRxCycleProfile,
    ) {
        #[cfg(feature = "task-poll-telemetry")]
        let body_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        #[cfg(feature = "task-poll-telemetry")]
        let mut admission_cycles = 0_u32;
        let mut measured_admit = |request| {
            #[cfg(feature = "task-poll-telemetry")]
            let started = crate::diagnostics::core0_rx_cycles::cycle_count();
            let outcome = admit(request);
            #[cfg(feature = "task-poll-telemetry")]
            {
                admission_cycles = admission_cycles.wrapping_add(
                    crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(started),
                );
            }
            outcome
        };
        let outcome = data_rx
            .try_dispatch_ordinary_pairwise(ordered, &mut measured_admit, in_place)
            .expect("ordinary AP preflight and dispatch share one synchronous owner");
        #[cfg(feature = "task-poll-telemetry")]
        crate::diagnostics::core0_ap_rx_cycles::CORE0_AP_RX_CYCLES.record_ingress_path(
            true,
            in_place.publication.is_some(),
            false,
            false,
        );
        #[cfg(feature = "task-poll-telemetry")]
        let observe_started = {
            let now = crate::diagnostics::core0_rx_cycles::cycle_count();
            core0_ap_rx.record_leaf_body(now.wrapping_sub(body_started));
            core0_ap_rx.record_leaf_admission(admission_cycles);
            now
        };
        #[cfg(any(feature = "diagnostics", test))]
        {
            report.record_dispatch_rejection(outcome, ordered, now.as_micros());
            if in_place.unsupported {
                report.record_rx_rejection(
                    AccessPointRxRejectionReason::InPlaceOutputUnsupported,
                    ordered,
                    now.as_micros(),
                );
            }
            let mut activity_peer = None;
            *produced_data |=
                observe_protected_dispatch(outcome, Some(_peer), report, &mut activity_peer);
        }
        #[cfg(not(any(feature = "diagnostics", test)))]
        let _ = outcome;
        #[cfg(feature = "task-poll-telemetry")]
        core0_ap_rx.record_leaf_observe(
            crate::diagnostics::core0_rx_cycles::cycle_count().wrapping_sub(observe_started),
        );
    }
}
