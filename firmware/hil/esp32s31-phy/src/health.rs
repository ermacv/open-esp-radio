//! Diagnostic-only adapters. All data is copied under the operation's existing owner.
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_phy::{
    PhyTargetObserver,
    tracking::observation::{Event, Operation},
};
pub use oer_esp32s31_instrumentation_hil::store::Store;
use oer_hil_protocol::RequestIdentity;
use oer_esp32s31_instrumentation_hil::*;
use oer_esp32s31_instrumentation::health::*;
use oer_time::Clock;

pub use crate::services::HealthRequest as Request;
pub enum Answer {
    Rejected(oer_hil_protocol::base::RejectReason),
    Started(HealthStarted),
    Window(HealthWindow),
    Finished(HealthFinished),
    Counters(CounterPage),
    Signals(SignalPage),
    Tracking(TrackingPage),
    Measurements(MeasurementPage),
    Released(HealthReleased),
}
impl Answer {
    pub async fn publish(&self, id: RequestIdentity) {
        macro_rules! send {
            ($v:expr) => {
                crate::transport::CONSOLE
                    .publish_reliably(id.session_id, id.request_id, $v)
                    .await
            };
        }
        match self {
            Self::Rejected(reason) => send!(&oer_hil_protocol::base::Rejected(*reason)),
            Self::Started(v) => send!(v),
            Self::Window(v) => send!(v),
            Self::Finished(v) => send!(v),
            Self::Counters(v) => send!(v),
            Self::Signals(v) => send!(v),
            Self::Tracking(v) => send!(v),
            Self::Measurements(v) => send!(v),
            Self::Released(v) => send!(v),
        };
    }
}
pub struct Observer<'a> {
    pub store: &'a mut Store,
}
impl Observer<'_> {
    fn measurement(&mut self, read: Bracket, value: MeasurementValue) {
        // Cold calibration visits several frequencies. Configured capture channel
        // must not be substituted for the measurement's actual channel or gain.
        self.store
            .push(Record::Measurement(Measurement {
                read,
                channel: None,
                gain: None,
                value,
            }))
            .unwrap_or_else(|fault| crate::health::count_fault(fault));
    }
    fn point(&mut self, value: MeasurementValue) {
        let at = now();
        self.measurement(
            Bracket {
                first_micros: at,
                last_micros: at,
            },
            value,
        );
    }
}
pub fn now() -> u64 {
    EmbassyClock.now().as_micros()
}
fn request(r: oer_esp32s31_phy::tracking::observation::health::Request) -> DcIqRequest {
    DcIqRequest {
        context: context(r.context),
        iteration: r.iteration,
        chain: r.chain,
        control: r.control,
        mode: r.mode,
    }
}
fn context(c: oer_esp32s31_phy::tracking::observation::health::Context) -> MeasurementContext {
    MeasurementContext {
        gain: c.gain,
        measurement: c.measurement,
        shared_radio: c.shared_radio,
        radio_stage: c.radio_stage,
        fine_code: c.fine_code,
        high_level: c.high_level,
        frequency_code: c.frequency_code,
    }
}
impl PhyTargetObserver for Observer<'_> {
    const OBSERVE_HEALTH: bool = true;
    fn pwdet_completed(&mut self, o: oer_esp32s31_phy::tracking::observation::health::Pwdet) {
        use oer_esp32s31_phy::tracking::observation::health::SarPhase as P;
        let phase = match o.phase {
            P::DetectorConfigured => SarPhase::DetectorConfigured,
            P::ToneConfigured => SarPhase::ToneConfigured,
            P::ReferenceControl => SarPhase::ReferenceControl,
            P::ToneArmed => SarPhase::ToneArmed,
            P::Triggered => SarPhase::Triggered,
            P::Ready => SarPhase::Ready,
            P::TimedOut => SarPhase::TimedOut,
            P::Captured => SarPhase::Captured,
            P::ToneCleared => SarPhase::ToneCleared,
            P::ToneStopped => SarPhase::ToneStopped,
        };
        self.measurement(
            Bracket {
                first_micros: o.started_micros,
                last_micros: o.finished_micros,
            },
            MeasurementValue::SarPhase {
                phase,
                measurement: o.measurement,
                sample: o.sample,
                value: o.value,
            },
        );
        if phase == SarPhase::TimedOut {
            self.store.fail();
        }
    }
    fn tracking_operation(&mut self, operation: Operation, event: Event) {
        if let Event::DcIq(observation) = event {
            use oer_esp32s31_phy::tracking::observation::health::Completion as C;
            let value = match observation.completion {
                C::Pending { activity } => MeasurementValue::DcIqPending {
                    request: request(observation.request),
                    activity,
                },
                C::Captured { sums } => MeasurementValue::DcIqCaptured {
                    request: request(observation.request),
                    sums,
                },
            };
            self.measurement(
                Bracket {
                    first_micros: observation.started_micros,
                    last_micros: observation.finished_micros,
                },
                value,
            );
            return;
        }
        if let Event::RxDcCorrection(c) = event {
            self.point(MeasurementValue::RxDcCorrection {
                context: context(c.context),
                configuration: c.configuration,
                iterations: c.iterations,
                converged: c.converged,
                residual: None,
            });
            return;
        }
        let disposition = match event {
            Event::Started => Disposition::Started,
            Event::Completed => Disposition::Ran,
            Event::Failed => Disposition::Failed,
            _ => return,
        };
        let operation = match operation {
            Operation::Tracking => TrackingOperation::Tick,
            Operation::GrantProtectRfpll => TrackingOperation::Rfpll,
            Operation::GrantProtectCommon => TrackingOperation::Common,
            Operation::GrantProtectTransmit => TrackingOperation::Transmit,
            Operation::WifiBasebandOff => TrackingOperation::WifiBasebandOff,
            Operation::Temperature => TrackingOperation::Temperature,
            other => TrackingOperation::Other(other as u8),
        };
        self.store
            .push(Record::Tracking(Tracking {
                condition: None,
                operation,
                due: Due::Unknown,
                disposition,
                at_micros: now(),
                committed: 0,
            }))
            .unwrap_or_else(|fault| crate::health::count_fault(fault));
        if disposition == Disposition::Failed {
            self.store.fail();
        }
    }
    fn tx_sar_wait(&mut self, elapsed_micros: u64, observations: u16, ready: bool) {
        self.point(MeasurementValue::SarWait {
            elapsed_micros,
            observations,
            ready,
        });
    }
    fn tx_dc_comparator(&mut self, gain: u8, iteration: u8, high: [bool; 2]) {
        self.point(MeasurementValue::TxDcComparator {
            gain,
            iteration,
            high,
        });
    }
}

pub fn temperature(store: &mut Store, state: &oer_esp32s31_phy::PhyState, channel: u8) {
    use oer_esp32s31_phy::tracking::TemperatureAcquisition as A;
    let observed = state.temperature_observation();
    let acquisition = match observed.acquisition {
        A::Unobserved => Acquisition::Unobserved,
        A::Undated => Acquisition::Undated,
        A::Overlong => Acquisition::Overlong,
        A::ClockReversed => Acquisition::ClockReversed,
        A::Window { started, completed } => Acquisition::Window(Bracket {
            first_micros: started,
            last_micros: completed,
        }),
    };
    let at = now();
    store
        .push(Record::Signal(Signal {
            channel,
            gain: None,
            read: Bracket {
                first_micros: at,
                last_micros: at,
            },
            value: SignalValue::Temperature(Temperature {
                sensor_units: observed.value,
                acquisition,
            }),
        }))
        .unwrap_or_else(|fault| crate::health::count_fault(fault));
}

pub fn cut(store: &mut Store, runtime: &oer_esp32s31_hal::owner::RadioAccess, cut: Cut) {
    let start = now();
    let rx = runtime.receive_statistics_snapshot();
    let tx = runtime.transmit_statistics_snapshot();
    let read = Bracket {
        first_micros: start,
        last_micros: now(),
    };
    let epoch = store.identity().epoch;
    let p = rx.primary;
    let d = rx.decode_errors;
    macro_rules! counter {
        ($kind:ident, $value:expr) => {
            store
                .push(Record::Counter(Counter {
                    cut,
                    epoch,
                    read,
                    kind: CounterKind::$kind,
                    value: $value,
                }))
                .unwrap_or_else(|fault| crate::health::count_fault(fault))
        };
    }
    macro_rules! c16 { ($($kind:ident = $field:ident),* $(,)?) => { $(counter!($kind, CounterValue::Modulo16(p.$field));)* }; }
    c16!(
        Mpdu = mpdu_count,
        FcsError = fcs_error,
        Abort = abort,
        AbortFcsPass = abort_fcs_pass,
        PowerDrop = power_drop_error,
        HeSigB = he_sig_b_error,
        SameBm = same_bm_error,
        SignalField = signal_field,
        End = end,
        DataSuccess = data_success,
        OtherUnicast = other_unicast,
        BufferFull = buffer_full,
        FifoOverflow = fifo_overflow,
        Tkip = tkip_error,
        BtBlock = bt_block_error,
        FrequencyHop = frequency_hop_error,
        LastUnmatched = last_unmatched_error,
        AckInterrupt = ack_interrupt,
        RtsInterrupt = rts_interrupt
    );
    counter!(CfoScaled40, CounterValue::SignedScaled40(p.cfo_scaled_40));
    macro_rules! c10 { ($($kind:ident = $field:ident),* $(,)?) => { $(counter!($kind, CounterValue::Modulo10(d.$field));)* }; }
    c10!(
        BrxAgc = brx_agc,
        Brx = brx,
        Nrx = nrx,
        NrxAbort = nrx_abort,
        NrxAgcExit = nrx_agc_exit,
        NrxBasebandOff = nrx_baseband_off,
        NrxFdmWatchdog = nrx_fdm_watchdog,
        NrxRestart = nrx_restart,
        NrxService = nrx_service,
        NrxTxOver = nrx_tx_over,
        NrxUnsupported = nrx_unsupported,
        NrxHeFormat = nrx_he_format,
        NrxHtSig = nrx_ht_sig,
        NrxHeUnsupported = nrx_he_unsupported,
        NrxHeSigACrc = nrx_he_sig_a_crc
    );
    counter!(TxRts, CounterValue::Modulo32(tx.tx_rts));
    counter!(TxCts, CounterValue::Modulo32(tx.tx_cts));
    counter!(TxTrack, CounterValue::Modulo32(tx.track));
    counter!(TxTrcts, CounterValue::Modulo32(tx.trcts));
    signal(store, runtime);
}
pub fn signal(store: &mut Store, runtime: &oer_esp32s31_hal::owner::RadioAccess) {
    let Some(config) = store.config() else {
        return;
    };
    let first_micros = now();
    let (raw, quarter_db) = runtime.noise_floor_snapshot();
    store
        .push(Record::Signal(Signal {
            read: Bracket {
                first_micros,
                last_micros: now(),
            },
            channel: config.channel,
            gain: None,
            value: SignalValue::NoiseFloor(NoiseFloor::SixteenthsDb12 {
                raw,
                quarter_db: quarter_db as i16,
            }),
        }))
        .unwrap_or_else(|fault| crate::health::count_fault(fault));
}
pub fn received(
    store: &mut Store,
    frame: &oer_esp32s31_ieee80211_mac::rx::NormalizedRxFrame<'_>,
    channel: u8,
) {
    let Some(config) = store.config() else {
        return;
    };
    // A normalized management header has transmitter address at octets 10..16.
    // Reject truncated headers and other transmitters instead of attributing ambient RSSI to the peer.
    if frame.mpdu.get(10..16) != Some(config.peer.as_slice()) {
        return;
    }
    let at = now();
    store
        .push(Record::Signal(Signal {
            read: Bracket {
                first_micros: at,
                last_micros: at,
            },
            channel,
            gain: None,
            value: SignalValue::Received {
                peer: config.peer,
                rssi_dbm: frame.rssi_dbm,
                rate: frame.observed_phy().map(|r| ReceivedRate {
                    family: r.bb_format,
                    code: r.rate,
                }),
            },
        }))
        .unwrap_or_else(|fault| crate::health::count_fault(fault));
}

/// Retained native predicates at one instant; actual branch outcomes follow separately.
pub fn inspection(store: &mut Store, phy: &oer_esp32s31_phy::concurrent::ConcurrentPhy) {
    let at_micros = now();
    let Ok(i) = phy.inspect_tracking(at_micros) else {
        store.fail();
        return;
    };
    let due = |due| if due { Due::Due } else { Due::NotDue };
    store
        .push(Record::Tracking(Tracking {
            condition: None,
            operation: TrackingOperation::Tick,
            due: due(matches!(
                i.schedule,
                oer_esp32s31_phy::tracking::schedule::Schedule::Due(_)
            )),
            disposition: Disposition::Inspected,
            at_micros,
            committed: 0,
        }))
        .unwrap_or_else(|fault| crate::health::count_fault(fault));
    if let Some(r) = i.rfpll {
        store
            .push(Record::Tracking(Tracking {
                operation: TrackingOperation::Rfpll,
                condition: Some(ThermalCondition {
                    current: r.current_temperature,
                    reference: r.reference_temperature,
                    threshold: r.threshold(),
                    inhibited: i.inhibited,
                }),
                due: due(r.is_due()),
                disposition: Disposition::Inspected,
                at_micros,
                committed: 0,
            }))
            .unwrap_or_else(|fault| crate::health::count_fault(fault));
    }
    if let Some(cal) = i.wifi.and_then(|w| w.calibration) {
        for (operation, r) in [
            (TrackingOperation::Common, cal.common),
            (TrackingOperation::Transmit, cal.transmit),
        ] {
            store
                .push(Record::Tracking(Tracking {
                    operation,
                    condition: Some(ThermalCondition {
                        current: r.current,
                        reference: r.reference,
                        threshold: r.threshold,
                        inhibited: i.inhibited,
                    }),
                    due: due(r.is_due()),
                    disposition: Disposition::Inspected,
                    at_micros,
                    committed: 0,
                }))
                .unwrap_or_else(|fault| crate::health::count_fault(fault));
        }
    }
}

pub fn retained_detector(store: &mut Store, state: &oer_esp32s31_phy::PhyState) {
    let p = state.pwdet_parameters();
    let at = now();
    store
        .push(Record::Measurement(Measurement {
            read: Bracket {
                first_micros: at,
                last_micros: at,
            },
            channel: None,
            gain: None,
            value: MeasurementValue::RetainedPowerDetector {
                tx_path: p.pbus_tx_path_value,
                rx_path: p.pbus_rx_path_value,
                dco: p.dco,
                clear_tone_after_ready: p.clear_tone_after_ready,
                reference_codes: p.reference_codes,
                calibrated: p.already_calibrated,
            },
        }))
        .unwrap_or_else(|fault| crate::health::count_fault(fault));
}

pub(crate) mod startup;

/// An impossible exact-count overflow keeps the recording owner's borrow
/// alive through the image's retained reset.
pub(crate) fn count_fault(fault: oer_esp32s31_instrumentation_hil::store::CountOverflow) -> ! {
    oer_espressif_staged_runtime::panic::reset_fault::<oer_esp32s31_hal::platform::Port>(
        oer_espressif_staged_runtime::panic::FaultReason::HilHealthCount,
        [
            fault.kind as u32,
            u32::from(fault.count),
            fault.stored as u32,
            0,
        ],
    )
}
