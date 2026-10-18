use super::*;

#[cfg(feature = "task-poll-telemetry")]
use crate::diagnostics::core0_rx_cycles::{Core0ProtocolCycleProfile, Core0ProtocolPath};

impl<
    'queue,
    'pool,
    'scratch,
    'irq,
    M: RawMutex,
    S,
    K,
    const CAPACITY: usize,
    const SLOTS: usize,
    const REORDER_SLOTS: usize,
> ConnectedRxProcessor<'queue, 'pool, 'scratch, 'irq, M, S, K, CAPACITY, SLOTS, REORDER_SLOTS>
where
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
    K: Timer,
{
    pub(super) async fn dispatch_retained_frame(
        &mut self,
        frame: RetainedRxFrame<'pool, CAPACITY, SLOTS, REORDER_SLOTS>,
    ) -> ConnectedRxDispatch {
        match frame {
            RetainedRxFrame::Hot(frame) => self.dispatch_owned_frame(frame).await,
            RetainedRxFrame::Cold(frame) => self.dispatch_reordered_frame(frame).await,
        }
    }

    async fn dispatch_reordered_frame(
        &mut self,
        frame: RxReorderFrame<'pool, CAPACITY, REORDER_SLOTS>,
    ) -> ConnectedRxDispatch {
        let locked = frame.segment();
        let source = locked.as_segment();
        let ordinary = !self.runtime.dispatcher.may_publish_amsdu(source);
        let result = if ordinary {
            if let Some(scratch) = self.reorder_scratch.as_deref_mut() {
                let length = source.buffer.len();
                scratch[..length].copy_from_slice(source.buffer);
                let segment = oer_esp32s31_ieee80211_mac::rx::RxSegment {
                    descriptor_address: source.descriptor_address,
                    descriptor_word0: source.descriptor_word0,
                    buffer: &scratch[..length],
                    next_descriptor_address: source.next_descriptor_address,
                };
                dispatch_non_amsdu_segment(
                    &mut self.runtime.dispatcher,
                    &mut self.sink,
                    self.mpdu,
                    segment,
                    None,
                    #[cfg(any(feature = "diagnostics", test))]
                    self.pipeline_observer,
                )
                .await
            } else {
                self.dispatch_segment(source, None).await
            }
        } else {
            self.dispatch_segment(source, None).await
        };
        drop(locked);
        drop(frame);
        result
    }

    /// Dispatch one ordinary retained frame without constructing an async
    /// capacity edge.
    ///
    /// The caller has already proved that this is a single-MSDU Ethernet
    /// publication and that the sink's retained staging slot is the complete
    /// output credit. Keeping this as a synchronous function avoids building
    /// and polling a nested Future for the production common case. Reorder,
    /// A-MSDU and capacity-dependent frames never enter this function.
    #[cfg_attr(
        target_arch = "riscv32",
        unsafe(link_section = ".hot.text.open_radio_sta_rx_direct")
    )]
    #[inline(never)]
    pub(super) fn dispatch_owned_frame_direct(
        &mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> ConnectedRxDispatch {
        #[cfg(feature = "task-poll-telemetry")]
        let mut core0_cycles = Core0ProtocolCycleProfile::begin();
        debug_assert_eq!(
            self.sink.staged_rx_admission(),
            StagedRxAdmission::Immediate
        );
        let times = frame.times();
        let segment = frame.segment();
        debug_assert!(!self.runtime.dispatcher.may_publish_amsdu(segment));
        debug_assert!(self.runtime.dispatcher.may_publish_ethernet(segment));
        #[cfg(feature = "task-poll-telemetry")]
        core0_cycles.preflight_completed();

        let (result, ethernet, _callback_started, _callback_ended, _data_cycles) = {
            let mut capture = StagedEthernetCapture::new(&mut self.sink, segment.buffer);
            let result = self.runtime.dispatcher.dispatch_received(
                segment,
                self.mpdu,
                &mut [],
                times,
                &mut capture,
            );
            (
                result,
                capture.captured,
                capture.callback_started,
                capture.callback_ended,
                #[cfg(feature = "task-poll-telemetry")]
                capture.data_cycles,
                #[cfg(not(feature = "task-poll-telemetry"))]
                (),
            )
        };
        #[cfg(feature = "task-poll-telemetry")]
        {
            if _callback_started == 0 {
                core0_cycles.dispatch_completed();
            } else {
                core0_cycles.dispatch_split_completed(_callback_started, _callback_ended);
            }
            if let Some(profile) = _data_cycles {
                crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES.record_data_profile(profile);
            }
        }

        let disposition = if let Some(ethernet) = ethernet {
            self.sink.publish_staged(frame, ethernet)
        } else {
            drop(frame);
            StagedRxDisposition::Released
        };
        if disposition == StagedRxDisposition::Released {
            self.irq.notify_rx_capacity();
        }
        #[cfg(feature = "task-poll-telemetry")]
        core0_cycles.finish(Core0ProtocolPath::Ordinary);
        result
    }

    pub(super) async fn dispatch_owned_frame(
        &mut self,
        frame: StagedRxFrame<'pool, CAPACITY, SLOTS>,
    ) -> ConnectedRxDispatch {
        #[cfg(feature = "task-poll-telemetry")]
        let mut core0_cycles = Core0ProtocolCycleProfile::begin();
        let times = frame.times();
        #[cfg(any(feature = "diagnostics", test))]
        let preflight_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let dispatch_via_scratch = self.runtime.dispatcher.may_publish_amsdu(frame.segment())
            || !self
                .runtime
                .dispatcher
                .may_publish_ethernet(frame.segment());
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, preflight_started) {
            observer.observe(RxPipelineObservation::ProtocolPreflightCompleted {
                micros: observer.elapsed_micros_since(started),
            });
        }
        #[cfg(feature = "task-poll-telemetry")]
        core0_cycles.preflight_completed();
        if dispatch_via_scratch {
            let result = self.dispatch_segment(frame.segment(), times).await;
            #[cfg(feature = "task-poll-telemetry")]
            core0_cycles.dispatch_completed();
            drop(frame);
            self.irq.notify_rx_capacity();
            #[cfg(feature = "task-poll-telemetry")]
            core0_cycles.finish(Core0ProtocolPath::Scratch);
            return result;
        }

        if self.sink.staged_rx_admission() == StagedRxAdmission::AwaitCapacity {
            #[cfg(any(feature = "diagnostics", test))]
            let wait_started = self.pipeline_observer.map(|observer| observer.now_micros());
            self.sink.wait_staged_ready().await;
            #[cfg(any(feature = "diagnostics", test))]
            if let (Some(observer), Some(started)) = (self.pipeline_observer, wait_started) {
                observer.observe(RxPipelineObservation::NetworkReadyWait {
                    micros: observer.elapsed_micros_since(started),
                });
            }
            #[cfg(feature = "task-poll-telemetry")]
            core0_cycles.wait_completed();
        }

        #[cfg(any(feature = "diagnostics", test))]
        let dispatch_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let segment = frame.segment();
        let (result, ethernet, _callback_started, _callback_ended, _data_cycles) = {
            let mut capture = StagedEthernetCapture::new(&mut self.sink, segment.buffer);
            let result = self.runtime.dispatcher.dispatch_received(
                segment,
                self.mpdu,
                &mut [],
                times,
                &mut capture,
            );
            (
                result,
                capture.captured,
                capture.callback_started,
                capture.callback_ended,
                #[cfg(feature = "task-poll-telemetry")]
                capture.data_cycles,
                #[cfg(not(feature = "task-poll-telemetry"))]
                (),
            )
        };
        #[cfg(feature = "task-poll-telemetry")]
        {
            core0_cycles.dispatch_split_completed(_callback_started, _callback_ended);
            if let Some(profile) = _data_cycles {
                crate::diagnostics::core0_rx_cycles::CORE0_RX_CYCLES.record_data_profile(profile);
            }
        }
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, dispatch_started) {
            let (data, amsdu, amsdu_subframes) = match result {
                ConnectedRxDispatch::Data {
                    ethernet_frames,
                    amsdu,
                } => (true, amsdu, ethernet_frames),
                _ => (false, false, 0),
            };
            observer.observe(RxPipelineObservation::ProtocolDispatched {
                data,
                amsdu,
                amsdu_subframes,
                unit_bytes: segment.buffer.len(),
                micros: observer.elapsed_micros_since(started),
            });
        }

        let disposition = if let Some(ethernet) = ethernet {
            self.sink.publish_staged(frame, ethernet)
        } else {
            drop(frame);
            StagedRxDisposition::Released
        };
        if disposition == StagedRxDisposition::Released {
            self.irq.notify_rx_capacity();
        }
        #[cfg(feature = "task-poll-telemetry")]
        core0_cycles.finish(Core0ProtocolPath::Ordinary);
        result
    }

    async fn dispatch_segment(
        &mut self,
        segment: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>,
        times: Option<oer_esp32s31_ieee80211_mac::rx::pool::RxTimes>,
    ) -> ConnectedRxDispatch {
        if self.runtime.dispatcher.may_publish_amsdu(segment) {
            return self.dispatch_amsdu(segment).await;
        }
        dispatch_non_amsdu_segment(
            &mut self.runtime.dispatcher,
            &mut self.sink,
            self.mpdu,
            segment,
            times,
            #[cfg(any(feature = "diagnostics", test))]
            self.pipeline_observer,
        )
        .await
    }

    async fn dispatch_amsdu(
        &mut self,
        segment: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>,
    ) -> ConnectedRxDispatch {
        #[cfg(any(feature = "diagnostics", test))]
        let dispatch_started = self.pipeline_observer.map(|observer| observer.now_micros());
        let (result, used, metadata, power_save_data) = {
            let wants_power_save_data = self.sink.wants_power_save_data();
            let mut deferred = DeferredEthernetFrames::new(self.ethernet, wants_power_save_data);
            let result =
                self.runtime
                    .dispatcher
                    .dispatch(segment, self.mpdu, &mut [], &mut deferred);
            (
                result,
                deferred.used(),
                deferred.metadata,
                deferred.power_save_data,
            )
        };
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (self.pipeline_observer, dispatch_started) {
            let (data, amsdu, amsdu_subframes) = match result {
                ConnectedRxDispatch::Data {
                    ethernet_frames,
                    amsdu,
                } => (true, amsdu, ethernet_frames),
                _ => (false, false, 0),
            };
            observer.observe(RxPipelineObservation::ProtocolDispatched {
                data,
                amsdu,
                amsdu_subframes,
                unit_bytes: segment.buffer.len(),
                micros: observer.elapsed_micros_since(started),
            });
        }
        let raw = segment.buffer;
        let metadata = metadata.unwrap_or_else(MacRxMetadata::unavailable);
        if let Some(data) = power_save_data {
            self.sink.publish(ConnectedRxEvent::PowerSaveData(data));
        }
        let mut offset = 0_usize;
        while let Some(record) =
            crate::datapath::rx::ethernet::record_at(self.ethernet, used, offset)
                .expect("the private A-MSDU writer produces valid records")
        {
            #[cfg(any(feature = "diagnostics", test))]
            let wait_started = self.pipeline_observer.map(|observer| observer.now_micros());
            self.sink.wait_ready().await;
            #[cfg(any(feature = "diagnostics", test))]
            if let (Some(observer), Some(started)) = (self.pipeline_observer, wait_started) {
                observer.observe(RxPipelineObservation::NetworkReadyWait {
                    micros: observer.elapsed_micros_since(started),
                });
            }
            self.sink.publish(ConnectedRxEvent::Ethernet {
                frame: record.frame,
                raw,
                amsdu: true,
                metadata,
            });
            offset = record.next_offset;
        }
        result
    }
}

struct StagedEthernetCapture<'sink, S> {
    sink: &'sink mut S,
    raw_start: usize,
    raw_length: usize,
    captured: Option<StagedEthernetPublication>,
    callback_started: u32,
    callback_ended: u32,
    #[cfg(feature = "task-poll-telemetry")]
    data_cycles: Option<ConnectedRxDataCycleProfile>,
}

impl<'sink, S> StagedEthernetCapture<'sink, S> {
    fn new(sink: &'sink mut S, raw: &[u8]) -> Self {
        Self {
            sink,
            raw_start: raw.as_ptr() as usize,
            raw_length: raw.len(),
            captured: None,
            callback_started: 0,
            callback_ended: 0,
            #[cfg(feature = "task-poll-telemetry")]
            data_cycles: None,
        }
    }
}

impl<S: ConnectedRxSink> ConnectedRxSink for StagedEthernetCapture<'_, S> {
    #[cfg(feature = "task-poll-telemetry")]
    fn observe_data_cycle_profile(&mut self, profile: ConnectedRxDataCycleProfile) {
        debug_assert!(self.data_cycles.is_none());
        self.data_cycles = Some(profile);
    }

    fn wants_power_save_data(&self) -> bool {
        self.sink.wants_power_save_data()
    }

    fn publish(&mut self, event: ConnectedRxEvent<'_>) {
        let ConnectedRxEvent::Ethernet {
            frame,
            amsdu: false,
            metadata,
            ..
        } = event
        else {
            self.sink.publish(event);
            return;
        };
        #[cfg(feature = "task-poll-telemetry")]
        let callback_started = crate::diagnostics::core0_rx_cycles::cycle_count();
        let payload_start = frame.payload.as_ptr() as usize;
        let payload_offset = payload_start
            .checked_sub(self.raw_start)
            .filter(|offset| {
                offset
                    .checked_add(frame.payload.len())
                    .is_some_and(|end| end <= self.raw_length)
            })
            .expect("ordinary staged Ethernet payload belongs to its raw frame");
        assert!(
            self.captured.is_none(),
            "one ordinary MPDU publishes exactly one Ethernet frame"
        );
        self.captured = Some(StagedEthernetPublication {
            destination: frame.destination,
            source: frame.source,
            ether_type: frame.ether_type,
            payload_offset,
            payload_length: frame.payload.len(),
            metadata,
        });
        #[cfg(feature = "task-poll-telemetry")]
        {
            self.callback_started = callback_started;
            self.callback_ended = crate::diagnostics::core0_rx_cycles::cycle_count();
        }
    }

    fn supports_esp_now_v2(&self) -> bool {
        self.sink.supports_esp_now_v2()
    }

    fn publish_esp_now_v2(
        &mut self,
        received: oer_ieee80211_softmac::EspNowReceivedV2<'_>,
        metadata: oer_ieee80211_softmac::MacRxMetadata<oer_esp32s31_ieee80211_mac::rx::RxPhyInfo>,
    ) {
        self.sink.publish_esp_now_v2(received, metadata);
    }
}

async fn dispatch_non_amsdu_segment<
    const CAPACITY: usize,
    const SLOTS: usize,
    S: ConnectedRxProtocolSink<CAPACITY, SLOTS>,
>(
    dispatcher: &mut ConnectedRxDispatcher,
    sink: &mut S,
    mpdu: &mut [u8],
    segment: oer_esp32s31_ieee80211_mac::rx::RxSegment<'_>,
    times: Option<oer_esp32s31_ieee80211_mac::rx::pool::RxTimes>,
    #[cfg(any(feature = "diagnostics", test))] pipeline_observer: Option<&dyn RxPipelineObserver>,
) -> ConnectedRxDispatch {
    if dispatcher.may_publish_ethernet(segment) || dispatcher.may_complete_fragment(segment) {
        // Keep the staging lease until the single-MSDU path owns its one
        // network output slot. A-MSDU uses the deferred streaming path
        // above and acquires one slot per decoded subframe.
        #[cfg(any(feature = "diagnostics", test))]
        let wait_started = pipeline_observer.map(|observer| observer.now_micros());
        sink.wait_ready().await;
        #[cfg(any(feature = "diagnostics", test))]
        if let (Some(observer), Some(started)) = (pipeline_observer, wait_started) {
            observer.observe(RxPipelineObservation::NetworkReadyWait {
                micros: observer.elapsed_micros_since(started),
            });
        }
    }
    #[cfg(any(feature = "diagnostics", test))]
    let dispatch_started = pipeline_observer.map(|observer| observer.now_micros());
    let result = dispatcher.dispatch_received(segment, mpdu, &mut [], times, sink);
    #[cfg(any(feature = "diagnostics", test))]
    if let (Some(observer), Some(started)) = (pipeline_observer, dispatch_started) {
        let (data, amsdu, amsdu_subframes) = match result {
            ConnectedRxDispatch::Data {
                ethernet_frames,
                amsdu,
            } => (true, amsdu, ethernet_frames),
            _ => (false, false, 0),
        };
        observer.observe(RxPipelineObservation::ProtocolDispatched {
            data,
            amsdu,
            amsdu_subframes,
            unit_bytes: segment.buffer.len(),
            micros: observer.elapsed_micros_since(started),
        });
    }
    result
}
