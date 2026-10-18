//! Thin wrappers around exact compiled production entries.
//!
//! Keep ABI conversion and isolated platform construction here. Operation
//! ordering belongs to the production driver function being traced.

use crate::calibration_projection::{
    CALIBRATION_WORDS, PARENT_WORDS, snapshot_calibration, snapshot_committed, snapshot_parent,
};
use core::{
    cell::Cell,
    future::{Future, ready},
};

use oer_time::{Clock, Instant, Timer};

/// Time for the compiled-production comparison: each wait hands its
/// requested interval to the harness `deliver` edge at once, and time then
/// stands at the deadline. Hardware time itself is outside this software
/// comparison.
struct TraceTimer {
    now: Cell<Instant>,
    deliver: fn(u32),
}

impl TraceTimer {
    /// Waits delivered through the plain harness delay edge.
    fn production() -> Self {
        Self {
            now: Cell::new(Instant::EPOCH),
            deliver: |micros| super::ets_delay_us(micros),
        }
    }

    /// Waits delivered through the register-preserving harness delay, as for
    /// RFPLL.
    fn register_preserving() -> Self {
        Self {
            now: Cell::new(Instant::EPOCH),
            deliver: |micros| super::open_phy_trace_preserving_delay(micros),
        }
    }
}

impl Clock for TraceTimer {
    fn now(&self) -> Instant {
        self.now.get()
    }
}

impl Timer for TraceTimer {
    fn wait_until(&self, deadline: Instant) -> impl Future<Output = ()> {
        if let Some(interval) = deadline.checked_duration_since(self.now.get()) {
            (self.deliver)(u32::try_from(interval.as_micros()).unwrap_or(u32::MAX));
            self.now.set(deadline);
        }
        ready(())
    }
}

/// RFPLL's explicit requested-time environment, with captured register-preserving
/// delivery: the ROM short delay relies on the ROM routine's actual register
/// usage, which an ordinary call boundary does not preserve.
struct RfpllTraceDelay;
impl oer_esp32s31_phy::PhyShortDelay for RfpllTraceDelay {
    const MAX_MICROS: u32 = oer_esp32s31_phy::RomShortDelay::MAX_MICROS;
    fn settle_micros(micros: u32) -> bool {
        if micros == 0 || micros > Self::MAX_MICROS {
            return false;
        }
        super::open_phy_trace_preserving_delay(micros);
        true
    }
}

oer_probe_macros::probe! {
    /// Complete production search, including typed I2C transactions and settling.
    /// The wrapper constructs its isolated PHY owner; no search policy lives
    /// in this ABI wrapper.
    pub fn open_phy_rfpll_trace_search() -> i32 {
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        embassy_futures::block_on(oer_esp32s31_phy::target_port::rfpll::search::<
            RfpllTraceDelay,
        >(&mut phy))
        .map_or(i32::MIN, |outcome| i32::from(outcome.delta()))
    }
}

oer_probe_macros::probe! {
    /// Direct synthesizer programming through the production target executor.
    /// Returns the initial capacitance in the high halfword and the final one
    /// in the low halfword, the ROM `phy_rfpll_cap_init_cal` result, or a
    /// negative code for a transition failure (-1) or an executor failure
    /// (`i32::MIN`).
    pub fn open_phy_rfpll_trace_program(frequency_code: u16, crystal_selector: u8, offset: u8) -> i32 {
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        match embassy_futures::block_on(oer_esp32s31_phy::target_port::rfpll::program::<
            RfpllTraceDelay,
        >(
            &TraceTimer::register_preserving(),
            &mut phy,
            oer_esp32s31_phy::analog::rfpll::RfpllFrequencyRequest {
                crystal_selector,
                frequency_code,
                offset,
            },
        )) {
            Ok(Ok(outcome)) => {
                ((u32::from(outcome.initial_cap) << 16) | u32::from(outcome.final_cap)) as i32
            }
            Ok(Err(_)) => -1,
            Err(_) => i32::MIN,
        }
    }
}

oer_probe_macros::probe! {
    /// Production frequency-control envelope; no grant or parent policy is modeled here.
    pub fn open_phy_rfpll_trace_maintain(channel: u16) -> i32 {
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        embassy_futures::block_on(oer_esp32s31_phy::target_port::rfpll::maintain::<
            RfpllTraceDelay,
        >(&mut phy, channel))
        .map_or(i32::MIN, |outcome| i32::from(outcome.search.delta()))
    }
}

oer_probe_macros::probe! {
    /// Current compiled thermal child, including its conditional hardware path.
    /// Returns the result reference in the low half and performed bit in bit 16,
    /// and writes the reference and `progress` with the RFPLL progress bit of a
    /// performed correction to `output`.
    pub fn open_phy_rfpll_trace_track(
        registers: &mut oer_esp32s31_pac::RadioPhyRegisters,
        current_temperature: i16,
        reference_temperature: i16,
        threshold_override: i32,
        current_channel: u16,
        progress: u16,
        output: &mut [u16; 2],
    ) -> i32 {
        let threshold_override = match threshold_override {
            -1 => None,
            0..=255 => Some(threshold_override as u8),
            _ => return i32::MIN,
        };
        let request = oer_esp32s31_phy::tracking::rfpll::thermal::Request {
            current_temperature,
            reference_temperature,
            threshold_override,
            current_channel,
        };
        embassy_futures::block_on(oer_esp32s31_phy::target_port::rfpll::track::<
            RfpllTraceDelay,
        >(&mut crate::shared_phy(registers), request))
        .map_or(i32::MIN, |outcome| {
            let performed = outcome.correction.is_some();
            *output = [
                outcome.reference_temperature as u16,
                progress | (u16::from(performed) * RFPLL_PROGRESS),
            ];
            i32::from(outcome.reference_temperature as u16) | (i32::from(performed) << 16)
        })
    }
}

oer_probe_macros::probe! {
    /// Complete production temperature transition: DAC read, optional default
    /// prime, one code sample and the conditional range write. Returns the
    /// temperature and writes the sensor index to `output`, or returns
    /// `i32::MIN` when the transition fails closed or its executor fails.
    /// Publishes no temperature observation.
    pub fn open_phy_trace_temperature_sample(output: &mut [u16; 1]) -> i32 {
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        match embassy_futures::block_on(oer_esp32s31_phy::target_port::temperature::sample::<
            RfpllTraceDelay,
        >(&TraceTimer::production(), &mut phy))
        {
            Ok(Ok(outcome)) => {
                output[0] = u16::from(outcome.sensor_index);
                i32::from(outcome.temperature)
            }
            _ => i32::MIN,
        }
    }
}

oer_probe_macros::probe! {
    /// Exact compiled production channel entry used by vendor comparison.
    ///
    /// The wrapper owns only the isolated probe image's peripheral tokens and ABI
    /// conversion. Channel sequencing remains entirely in the production PHY;
    /// platform MMIO is provided by the same ESP-HAL adapter used by firmware.
    pub fn open_phy_production_trace_phy_chip_set_chan(channel_or_frequency: u32, cbw: u32) -> u32 =>
        trace_channel(channel_or_frequency, cbw, [0, 0]).map_or(1, |_| 0);
}

oer_probe_macros::probe! {
    /// Compiled production gain calculation, without channel or hardware effects.
    /// Output groups are digital bytes, baseband halfwords and RF halfwords.
    pub fn open_phy_channel_trace_calculate_tx_gain(
        channel: u16,
        curve: &[u8; 6],
        correction: i8,
        base_and_delta: i8,
        output: &mut [u32; 40],
    ) {
        use oer_esp32s31_phy::channel::{PhyWifiTxGainRequest, calculate_wifi_tx_gain};
        let image = calculate_wifi_tx_gain(PhyWifiTxGainRequest {
            channel,
            calibration_curve: *curve,
            correction,
            base_and_delta,
        });
        output[..8].copy_from_slice(&image.output_32);
        output[8..24].copy_from_slice(&image.output_64);
        output[24..].copy_from_slice(&image.output_72);
    }
}

oer_probe_macros::probe! {
    /// Channel, temperature, bandwidth, 802.11p configuration and temperature-sensor
    /// index committed by
    /// the real production transition from the given 802.11p enable and
    /// configuration bytes. Output is left untouched on failure.
    pub fn open_phy_channel_trace_state(
        channel_or_frequency: u32,
        cbw: u32,
        dot11p: &[u8; 2],
        output: &mut [u16; 5],
    ) -> u32 {
        let Ok(state) = trace_channel(channel_or_frequency, cbw, *dot11p) else {
            return 1;
        };
        let parameters = state.calibration_tracking_parameters(None);
        let dot11p = state.dot11p_configuration();
        let snapshot = oer_esp32s31_phy::validation::calibration_snapshot(&state);
        *output = [
            state.current_wifi_channel(),
            state.temperature_observation().value as u16,
            u16::from(parameters.channel_bandwidth),
            u16::from_le_bytes([dot11p.enabled, dot11p.configuration]),
            u16::from(snapshot.common.sensor_index),
        ];
        0
    }
}

fn trace_channel(
    channel_or_frequency: u32,
    cbw: u32,
    [dot11p_enabled, dot11p_configuration]: [u8; 2],
) -> Result<oer_esp32s31_phy::PhyState, ()> {
    // SAFETY: the verifier executes this entry in an isolated image and never
    // creates a second peripheral owner during the same execution.
    let peripherals = unsafe { esp_hal::peripherals::Peripherals::steal() };
    let platform = oer_esp32s31_radio_esp_hal::EspHalRadioPlatform::new(
        peripherals.MODEM_SYSCON,
        peripherals.MODEM_LPCON,
        peripherals.HP_SYS_CLKRST,
        peripherals.PMU,
        peripherals.LP_AON_CLK_RST,
        peripherals.LP_PERI,
        peripherals.LP_TSENS,
        peripherals.I2C_ANA_MST,
    );
    let (mut radio, partitions) =
        oer_esp32s31_hal::root::RadioHardware::for_validation().into_concurrent(());
    let mut lease = acquire(&mut radio);
    let mut clocked = oer_esp32s31_hal::ieee80211::client::WifiClocked::for_validation(
        oer_esp32s31_hal::ieee80211::client::WifiCold::from_partition(partitions.wifi),
    );
    let mut platform = platform;
    let mut channel = clocked.channel_hal(&mut platform, &mut lease);
    let mut state = oer_esp32s31_phy::PhyState::default();
    state.set_dot11p_configuration(dot11p_enabled, dot11p_configuration);
    let mut observer = oer_esp32s31_phy::target_port::NoopPhyTargetObserver;
    embassy_futures::block_on(oer_esp32s31_phy::validation::select_channel::<
        RfpllTraceDelay,
        _,
        _,
    >(
        &TraceTimer::production(),
        &mut state,
        channel_or_frequency as u16,
        cbw as u8,
        &mut channel,
        &mut observer,
    ))
    .map_err(|_| ())?;
    Ok(state)
}

oer_probe_macros::probe! {
    /// Publish caller-supplied gain components through the production channel binding.
    /// The input contains seed words, packed gain components and configuration;
    /// no calibration table or register encoding is implemented by this wrapper.
    pub fn open_phy_channel_trace_publish_tx_gain(input: &[u32; 47]) -> u32 {
        use oer_esp32s31_phy::channel::{
            PhyChipChannelAction, PhyChipChannelMmioBinding, PhyWifiTxGainImage,
        };
        let image = PhyWifiTxGainImage {
            seed: input[0..6].try_into().unwrap(),
            output_32: input[6..14].try_into().unwrap(),
            output_64: input[14..30].try_into().unwrap(),
            output_72: input[30..46].try_into().unwrap(),
            config: input[46] as u16,
        };
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        let binding =
            PhyChipChannelMmioBinding::new(PhyChipChannelAction::PublishTxGain(image)).unwrap();
        binding.execute_target(&mut (), &mut phy);
        0
    }
}

fn bluetooth_gain_projection(
    input: &[u32; 9],
    output: &mut [u8; 80],
) -> oer_esp32s31_phy::calibration::bluetooth::PhyBluetoothTxGainImage {
    use oer_esp32s31_phy::calibration::bluetooth::{
        PhyBluetoothTxGainParameters, calculate_bluetooth_tx_gain,
    };
    let curve = input[7].to_le_bytes();
    let controls = input[8].to_le_bytes();
    let image = calculate_bluetooth_tx_gain(PhyBluetoothTxGainParameters {
        seed: input[..6].try_into().unwrap(),
        config: input[6] as u16,
        calibration_curve: curve[..3].try_into().unwrap(),
        correction: curve[3] as i8,
        base: controls[0],
        attenuation: controls[1],
    });
    output[..16].copy_from_slice(&image.output_32);
    for (destination, value) in output[16..48].chunks_exact_mut(2).zip(image.output_64) {
        destination.copy_from_slice(&value.to_le_bytes());
    }
    for (destination, value) in output[48..].chunks_exact_mut(2).zip(image.output_72) {
        destination.copy_from_slice(&value.to_le_bytes());
    }
    image
}

oer_probe_macros::probe! {
    /// Pure shipping Bluetooth arithmetic, projected without a peripheral owner.
    pub fn open_phy_bluetooth_trace_calculate_gain(input: &[u32; 9], output: &mut [u8; 80]) {
        let _ = bluetooth_gain_projection(input, output);
    }
}

oer_probe_macros::probe! {
    /// Actual Bluetooth gain calculation and publication with caller-owned inputs.
    /// ABI conversion only: `seed[6]`, config, packed curve/correction, base/attenuation.
    pub fn open_phy_bluetooth_trace_tx_gain(input: &[u32; 9], output: &mut [u8; 80]) -> u32 {
        use oer_esp32s31_phy::{
            analog::pbus::PhyForceTxRxDepth,
            calibration::bluetooth::{PhyBluetoothTxGainChild, PhyBluetoothTxGainPublication},
        };
        let image = bluetooth_gain_projection(input, output);
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        // A standalone gain child holds no caller's force-TX/RX level.
        match PhyBluetoothTxGainChild::new(
            PhyBluetoothTxGainPublication::new(image),
            PhyForceTxRxDepth::OUTERMOST,
        )
        .execute_target::<oer_esp32s31_phy::RomShortDelay>(&mut phy)
        {
            Ok(()) => 0,
            Err(_) => 1,
        }
    }
}

oer_probe_macros::probe! {
    /// Complete TX-DC/PWDET executor shared with runtime tracking. The probe only
    /// supplies semantic inputs and exports the measured DC rows.
    pub fn open_phy_calibration_trace_tx_dc_pwdet(
        input: &[[u16; 4]; 3],
        bluetooth: bool,
        tx_path_value: u8,
        clear_tone_after_ready: bool,
        output: &mut [[u16; 4]; 3],
    ) -> u32 {
        use oer_esp32s31_phy::{
            target_port::{NoopPhyTargetObserver, calibration},
            tx::dc_power_detector::*,
        };
        let parameters = PhyTxDcPwdetParameters {
            dco: *input,
            clear_tone_after_ready,
        };
        let mut child = if bluetooth {
            PhyTxDcPwdetTransition::new_bluetooth(parameters, tx_path_value)
        } else {
            PhyTxDcPwdetTransition::new(parameters)
        };
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        let mut observer = NoopPhyTargetObserver;
        match calibration::tx_dc_pwdet_init::<oer_esp32s31_phy::RomShortDelay, _>(
            &mut child,
            &mut phy,
            &core::cell::RefCell::new(&mut observer),
            &TraceTimer::production(),
        ) {
            Ok(()) => {}
            Err(oer_esp32s31_phy::PhyTargetPortError::HardwareEdgeTimedOut) => return 2,
            Err(oer_esp32s31_phy::PhyTargetPortError::RfOperationLimit) => return 6,
            Err(_) => return 3,
        }
        match child.action() {
            PhyTxDcPwdetAction::Complete(result) => {
                *output = result.dco;
                0
            }
            PhyTxDcPwdetAction::Failed(PhyTxDcPwdetFailure::Search(
                PhyTxDcPwdetSearchFailure::ToneSar(
                    oer_esp32s31_phy::tx::calibration::PhyToneSarFailure::ReadyObservationLimit {
                        ..
                    },
                ),
            )) => 5,
            PhyTxDcPwdetAction::Failed(_) => 4,
            _ => 7,
        }
    }
}

oer_probe_macros::probe! {
    /// Execute the same complete PBus-clear child used by runtime RX calibration.
    /// Only the parent request and isolated register capability are supplied here.
    pub fn open_phy_calibration_trace_pbus_clear() -> u32 {
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        let registers = &mut phy;
        use oer_esp32s31_phy::tracking::{calibration::*, parameters::PhyParamTrackRequest};
        let parent = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                clients: PhyParamTrackRequest::new(true, false),
            },
            PhyCalibrationTrackingParameters {
                current_temperature: 100,
                common_reference_temperature: 0,
                transmit_reference_temperature: 100,
                threshold_override: None,
                current_channel: 13,
                channel_bandwidth: 1,
                crystal_selector: 0,
            },
        );
        let mut parent = parent;
        if enter_common_force_level(&mut parent).is_none() {
            return 1;
        }
        let Ok(child) = parent.begin_pbus_clear() else {
            return 1;
        };
        let mut observer = oer_esp32s31_phy::target_port::NoopPhyTargetObserver;
        let result = oer_esp32s31_phy::target_port::calibration::clear_pbus::<
            RfpllTraceDelay,
            _,
        >(child, registers, &mut observer);
        // Distinguish an actual bounded hardware timeout from a binding or
        // executor failure, so the fault scenario cannot accept either by mistake.
        let completion = match result {
            Ok(completion) => completion,
            Err(oer_esp32s31_phy::PhyTargetPortError::HardwareEdgeTimedOut) => return 2,
            Err(_) => return 5,
        };
        if parent.advance(completion).is_err() {
            return 3;
        }
        match parent.action() {
            PhyCalibrationTrackingAction::CalibrateDcode => 0,
            _ => 4,
        }
    }
}

oer_probe_macros::probe! {
    /// Actual RXCAL prefix: PBus clear followed by the complete D-code child.
    pub fn open_phy_calibration_trace_dcode(crystal_selector: u8, output: &mut [u8; 8]) -> u32 {
        use oer_esp32s31_phy::{
            target_port::{NoopPhyTargetObserver, calibration},
            tracking::{calibration::*, parameters::PhyParamTrackRequest},
        };
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        let registers = &mut phy;
        let mut parent = PhyCalibrationTrackingTransition::new(
            PhyCalibrationTrackingRequest {
                clients: PhyParamTrackRequest::new(true, false),
            },
            PhyCalibrationTrackingParameters {
                current_temperature: 100,
                common_reference_temperature: 0,
                transmit_reference_temperature: 100,
                threshold_override: None,
                current_channel: 13,
                channel_bandwidth: 1,
                crystal_selector,
            },
        );
        let mut observer = NoopPhyTargetObserver;
        let result = (|| {
            enter_common_force_level(&mut parent).ok_or(1u32)?;
            let child = parent.begin_pbus_clear().map_err(|_| 1u32)?;
            let completion = calibration::clear_pbus::<
                RfpllTraceDelay,
                _,
            >(child, registers, &mut observer)
            .map_err(|_| 2u32)?;
            parent.advance(completion).map_err(|_| 3u32)?;
            let child = parent.begin_dcode().map_err(|_| 4u32)?;
            let completion = calibration::dcode::<
                RfpllTraceDelay,
                _,
            >(child, &mut (), registers, |_| {})
            .map_err(|error| match error {
                oer_esp32s31_phy::PhyTargetPortError::HardwareEdgeTimedOut => 5u32,
                _ => 9u32,
            })?;
            let PhyCalibrationTrackingCompletion::DcodeCompleted(result) = completion else {
                return Err(6u32);
            };
            let codes = result.result().map_err(|failure| match failure {
                oer_esp32s31_phy::analog::dcode::PhyDcodeFailure::Rfpll {
                    failure: oer_esp32s31_phy::analog::rfpll::RfpllFrequencyFailure::FrequencyReadyDeadlineExceeded { .. },
                    ..
                } => 7u32,
                _ => 10u32,
            })?.codes;
            parent.advance(completion).map_err(|_| 8u32)?;
            *output = codes;
            Ok::<(), u32>(())
        })();
        result.err().unwrap_or(0)
    }
}

oer_probe_macros::probe! {
    /// Build the session arbiter, as production builds its arbiter once at
    /// boot, so a later compared entry does not count its construction.
    pub fn open_phy_validation_arbiter_init() -> i32 {
        session_radio();
        0
    }
}

oer_probe_macros::probe! {
    /// Complete RX-gain root with explicit semantic calibration inputs. Flags
    /// select the existing DC/table guards; no child completion is synthesized.
    /// It uses the session arbiter, which a preceding
    /// `open_phy_validation_arbiter_init` builds.
    pub fn open_phy_calibration_trace_rx_gain(
        input: &[u16; 49],
        flags: u8,
        crystal_selector: u8,
        pbus_rx_path: u8,
        output: &mut [u16; 50],
    ) -> u32 {
        use oer_esp32s31_phy::{
            calibration::baseband::PhyRxGainMemoryParameters,
            rx::{gain::*, gain_calibration::PhyRxGainDcParameters},
        };
        let mut lease = acquire(session_radio());
        let mut phy = lease.phy_hal();
        let mut child = if flags & 2 != 0 {
            PhyRxGainInitTransition::with_initialized_tables()
        } else {
            PhyRxGainInitTransition::new(PhyRxGainInitParameters {
                dc_calibrated: flags & 1 != 0,
                tables_initialized: false,
                dc: PhyRxGainDcParameters {
                    crystal_selector,
                    pbus_rx_path_value: pbus_rx_path,
                    rx_saturation_detected: flags & 4 != 0,
                },
                memory: PhyRxGainMemoryParameters {
                    parameter_002: pbus_rx_path,
                    wifi_index_dc: core::array::from_fn(|i| [input[2 * i], input[2 * i + 1]]),
                    wifi_fine_dc: core::array::from_fn(|i| [input[16 + 2 * i], input[17 + 2 * i]]),
                    shared_index_dc: core::array::from_fn(|i| [input[26 + 2 * i], input[27 + 2 * i]]),
                    wifi_auxiliary: input[48],
                },
            })
        };
        match oer_esp32s31_phy::target_port::calibration::rx_gain_init::<
            oer_esp32s31_phy::RomShortDelay,
            _,
        >(&mut child, &mut (), &mut phy, |_, _| {}, |_| {})
        {
            Ok(()) => {}
            Err(oer_esp32s31_phy::PhyTargetPortError::HardwareEdgeTimedOut) => return 2,
            Err(oer_esp32s31_phy::PhyTargetPortError::RfOperationLimit) => return 5,
            Err(_) => return 3,
        }
        let Some(Ok(result)) = child.terminal() else {
            return 4;
        };
        if let Some(dc) = result.dc {
            for (destination, source) in output[..16].chunks_exact_mut(2).zip(dc.wifi_index_dc) {
                destination.copy_from_slice(&source);
            }
            for (destination, source) in output[16..26].chunks_exact_mut(2).zip(dc.wifi_fine_dc) {
                destination.copy_from_slice(&source);
            }
            for (destination, source) in output[26..48].chunks_exact_mut(2).zip(dc.shared_index_dc) {
                destination.copy_from_slice(&source);
            }
        }
        output[48] = u16::from(result.wifi_last_index);
        output[49] = u16::from(result.shared_last_index);
        0
    }
}

oer_probe_macros::probe! {
    /// Complete combined RXCAL/TXCAL with real child executors and state commit.
    /// Input: current/RX/TX temperatures, channel, bandwidth, crystal selector,
    /// threshold (256 selects the default). Output: semantic references and DC banks.
    pub fn open_phy_calibration_trace_combined(
        input: &[u16; 8],
        wifi: bool,
        bluetooth: bool,
        output: &mut [u16; 85],
    ) -> u32 {
        use oer_esp32s31_phy::{
            tracking::{calibration::*, parameters::*},
            validation,
        };
        let mut state = validation::calibration_tracking_state(PhyCalibrationTrackingParameters {
            current_temperature: input[0] as i16,
            common_reference_temperature: input[1] as i16,
            transmit_reference_temperature: input[2] as i16,
            current_channel: input[3],
            channel_bandwidth: input[4] as u8,
            crystal_selector: input[5] as u8,
            threshold_override: (input[6] < 256).then_some(input[6] as u8),
        });
        let [shared_last, wifi_last] = input[7].to_le_bytes();
        validation::seed_rx_table_last_indices(&mut state, shared_last, wifi_last);
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        let mut observer = oer_esp32s31_phy::NoopPhyTargetObserver;
        let mut platform = ();
        // The PHY archive is compared without the coexistence archive, so its
        // weak grant-protect hooks return without effect.
        let mut grant = oer_esp32s31_phy::WeakPhyGrantProtect;
        let timer = TraceTimer::production();
        let mut port = oer_esp32s31_phy::TargetPhyCalibrationTrackingPort::<
            _,
            _,
            _,
            RfpllTraceDelay,
            _,
            _,
        >::new(&mut platform, &mut phy, &mut grant, &timer, &mut observer);
        let mut progress = 0;
        let status = {
            let mut child = validation::calibration_tracking(
                &mut state,
                PhyParamTrackRequest::new(wifi, bluetooth),
            );
            if embassy_futures::block_on(oer_esp32s31_phy::executor::run_phy_calibration_tracking(
                &mut child, &mut port,
            ))
            .is_err()
            {
                1
            } else {
                match child.commit() {
                    Ok(PhyParamTrackingCompletion::CalibrationTracked(completion)) => {
                        progress = calibration_progress(
                            completion.common_updated(),
                            completion.transmit_updated(),
                        );
                        0
                    }
                    _ => 2,
                }
            }
        };
        snapshot_calibration(&state, (&mut output[..CALIBRATION_WORDS]).try_into().unwrap());
        snapshot_committed(
            &state,
            progress,
            (&mut output[CALIBRATION_WORDS..]).try_into().unwrap(),
        );

        status
    }
}

/// Bits of the vendor tracking progress word at `phy_param[0x1fe]`: the RFPLL
/// capacitance correction, the Wi-Fi and Bluetooth/802.15.4 TX-power gain
/// updates, and the common and transmit calibrations.
const RFPLL_PROGRESS: u16 = 0x1;
const WIFI_POWER_PROGRESS: u16 = 0x2;
const BLUETOOTH_POWER_PROGRESS: u16 = 0x4;
const COMMON_CALIBRATION_PROGRESS: u16 = 0x8;
const TRANSMIT_CALIBRATION_PROGRESS: u16 = 0x10;

const fn calibration_progress(common: bool, transmit: bool) -> u16 {
    ((common as u16) * COMMON_CALIBRATION_PROGRESS)
        | ((transmit as u16) * TRANSMIT_CALIBRATION_PROGRESS)
}

oer_probe_macros::probe! {
    /// Complete current parameter parent, including real power/I2C/calibration and
    /// temperature children. RFPLL defaults to registered policy, with an explicit
    /// validation-only override; neither path grants physical access.
    /// Inputs extend the combined probe with signed gain adjustment, relaxed
    /// power threshold, RFPLL enable, the retained Wi-Fi I2C band code, the
    /// Bluetooth/802.15.4 TX-power tracking enable, the tracking inhibit and the
    /// calibration tracking enable. Outputs extend its 77 words with power
    /// temperature/cache, Wi-Fi and BT gain bases, retained adjustment, the vendor
    /// I2C-band code and RFPLL reference temperature.
    /// Status: 0 terminal owner, 1 contained hardware failure, 2 incomplete success,
    /// 3 erroneous ordinary-owner recovery after an executor failure.
    pub fn open_phy_tracking_trace_parent(
        input: &[u16; 15],
        wifi: bool,
        bluetooth: bool,
        output: &mut [u16; 92],
    ) -> u32 {
        use oer_esp32s31_phy::{
            tracking::{calibration::*, parameters::*},
            validation,
        };
        let mut state = validation::parameter_tracking_state(
            PhyCalibrationTrackingParameters {
                current_temperature: input[0] as i16,
                common_reference_temperature: input[1] as i16,
                transmit_reference_temperature: input[2] as i16,
                current_channel: input[3],
                channel_bandwidth: input[4] as u8,
                crystal_selector: input[5] as u8,
                threshold_override: (input[6] < 256).then_some(input[6] as u8),
            },
            input[7] as i8,
            input[8] != 0,
        );
        let [shared_last, wifi_last] = input[10].to_le_bytes();
        validation::seed_rx_table_last_indices(&mut state, shared_last, wifi_last);
        {
            use oer_esp32s31_phy::tracking::i2c::{PhyWifiI2cTrackingBand, PhyWifiI2cTrackingOutcome};
            // Vendor band code at `phy_param[0x4d]`, as the band output encodes it.
            let band = match input[11] {
                1 => PhyWifiI2cTrackingBand::Cold,
                2 => PhyWifiI2cTrackingBand::Elevated,
                3 => PhyWifiI2cTrackingBand::Hot,
                _ => PhyWifiI2cTrackingBand::Nominal,
            };
            state.apply_wifi_i2c_tracking_outcome(PhyWifiI2cTrackingOutcome {
                band,
                changed: false,
            });
        }
        state.set_bt_power_tracking(input[12] as u8);
        let clients = PhyParamTrackRequest::new(wifi, bluetooth);
        let mut pending = validation::parameter_tracking(
            &state,
            clients,
            validation::ParameterTrackingChoices {
                rfpll: input[9] != 0,
                inhibited: input[13] != 0,
                calibration: input[14] != 0,
            },
        );
        let mut radio = validation_radio();
        let mut lease = acquire(&mut radio);
        let mut phy = lease.phy_hal();
        let mut platform = ();
        // The PHY archive is compared without the coexistence archive, so its
        // weak grant-protect hooks return without effect.
        let mut grant = oer_esp32s31_phy::WeakPhyGrantProtect;
        let timer = TraceTimer::production();
        let mut port =
            oer_esp32s31_phy::TargetPhyParamTrackingPort::<_, _, _, RfpllTraceDelay, _, _>::new(
                &mut platform,
                &mut phy,
                &mut grant,
                &timer,
                oer_esp32s31_phy::NoopPhyTargetObserver,
            );
        let run = embassy_futures::block_on(oer_esp32s31_phy::executor::run_phy_param_tracking(
            &mut pending,
            &mut state,
            &mut port,
        ));
        // The vendor progress word `phy_param_track_tot` returns.
        let progress = run.as_ref().map_or(0, |outcome| {
            (u16::from(outcome.rfpll_corrected) * RFPLL_PROGRESS)
                | (u16::from(outcome.tx_power.wifi) * WIFI_POWER_PROGRESS)
                | (u16::from(outcome.tx_power.bluetooth_ieee802154) * BLUETOOTH_POWER_PROGRESS)
                | calibration_progress(
                    outcome.calibration.common,
                    outcome.calibration.transmit,
                )
        });
        let status = if run.is_err() {
            match pending.into_owner() {
                Ok(_) => 3, // A failed parent must never release an ordinary owner.
                Err(pending) => {
                    drop(pending.fail());
                    1
                }
            }
        } else {
            match pending.into_owner() {
                Ok(_) => 0,
                Err(pending) => {
                    drop(pending.fail());
                    2
                }
            }
        };
        snapshot_calibration(&state, (&mut output[..CALIBRATION_WORDS]).try_into().unwrap());
        let parent = &mut output[CALIBRATION_WORDS..];
        snapshot_parent(&state, (&mut parent[..PARENT_WORDS]).try_into().unwrap());
        snapshot_committed(&state, progress, (&mut parent[PARENT_WORDS..]).try_into().unwrap());
        status
    }
}

/// The common calibration branch first enters its force-TX/RX level. Probes
/// that compare only a later common child supply that child's completions as
/// state, without hardware.
fn enter_common_force_level(
    parent: &mut oer_esp32s31_phy::tracking::calibration::PhyCalibrationTrackingTransition,
) -> Option<()> {
    use oer_esp32s31_phy::analog::pbus::{PhyForceTxRxAction, PhyForceTxRxCompletion};
    use oer_esp32s31_phy::tracking::calibration::{
        PhyCalibrationTrackingAction, PhyCalibrationTrackingCompletion,
    };
    // The PHY archive is compared without the coexistence archive, so its
    // weak grant-protect hook returns without effect.
    if parent.action() == (PhyCalibrationTrackingAction::SetGrantProtect { enabled: true }) {
        parent
            .advance(PhyCalibrationTrackingCompletion::GrantProtectSet { enabled: true })
            .ok()?;
    }
    let mut force = parent.begin_force_txrx().ok()?;
    let completion = loop {
        let completion = match force.action() {
            PhyForceTxRxAction::Configure { enabled, phase } => {
                PhyForceTxRxCompletion::Configured { enabled, phase }
            }
            PhyForceTxRxAction::DelayMicros {
                enabled,
                completed_phase,
                micros,
            } => PhyForceTxRxCompletion::DelayElapsed {
                enabled,
                completed_phase,
                micros,
            },
            PhyForceTxRxAction::Complete { .. } => break force.commit().ok()?,
        };
        force.advance(completion).ok()?;
    };
    parent.advance(completion).ok()
}

/// The arbiter of an isolated validation radio.
fn validation_radio() -> oer_esp32s31_hal::shared_radio::SharedRadio<()> {
    oer_esp32s31_hal::root::RadioHardware::for_validation()
        .into_concurrent(())
        .0
}

/// The validation arbiter of the current verifier session.
///
/// It lives in image memory and is built on first use, so an entry that runs
/// after `open_phy_validation_arbiter_init` only takes its lease, as a
/// production transaction does.
fn session_radio() -> &'static mut oer_esp32s31_hal::shared_radio::SharedRadio<()> {
    static mut SESSION_RADIO: Option<oer_esp32s31_hal::shared_radio::SharedRadio<()>> = None;
    // SAFETY: the verifier executes one entry at a time on one hart, and
    // each entry calls this once and drops the reference before it returns,
    // so no two references to the arbiter are alive together.
    unsafe { (*core::ptr::addr_of_mut!(SESSION_RADIO)).get_or_insert_with(validation_radio) }
}

/// The lease of a validation arbiter; nothing else can hold it.
///
/// The unique borrow takes the lease without an atomic exchange, whose word
/// the verifier rejects beside uninitialized stack bytes. The vendor leaves
/// run inside their caller's `phy_lock` and never release it, so the lease is
/// not released either: its release store would add a fence the compared leaf
/// does not execute. The arbiter is local to the entry and ends with it.
fn acquire(
    radio: &mut oer_esp32s31_hal::shared_radio::SharedRadio<()>,
) -> core::mem::ManuallyDrop<oer_esp32s31_hal::shared_radio::SharedRadioLease<'_, ()>> {
    core::mem::ManuallyDrop::new(radio.lease_for_validation())
}
