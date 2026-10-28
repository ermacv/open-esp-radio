macro_rules! place_diagnostic_radio {
    ($storage:expr, Self { $($fields:tt)* }) => {{
        #[cfg(not(feature = "diagnostic-radio-health"))]
        { oer_memory::init_in_place!($storage, Self { $($fields)* }) }
        #[cfg(feature = "diagnostic-radio-health")]
        { oer_memory::init_in_place!($storage, Self { $($fields)*
            health => crate::health::Store::place, health_boot: 0,
        }) }
    }};
}

use crate::Rng;
use crate::completion::Boundary;
use crate::{Answer, WireRequest as Request, projection};
use oer_diagnostics::{DiagPath, Note};
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::power::ChipClocks;
use oer_esp32s31_hal::{
    domain::{CoexAbsent, Coexisting, CommonPower, RadioDomain, WifiClient},
    ieee80211::{
        client::{WifiClocked, WifiCold},
        init::*,
    },
    phy::i2c::{self, Address},
    root::WifiPartition,
    types::MacTxPowerPair,
};
use oer_esp32s31_phy::{
    concurrent::{ConcurrentAcquire, ConcurrentPhy, PhyClientAccess, phy_ready},
    wifi_client::{WifiPhyMembership, join_wifi, leave_wifi, set_wifi_rx},
    *,
};
use oer_hil_protocol::{base::RejectReason, phy::*};
use oer_time::{Clock, Timer};

type Result<T> = core::result::Result<T, RejectReason>;

pub struct Radio {
    domain: Option<RadioDomain<ChipClocks, ConcurrentPhy>>,
    radio_fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
    clocks: ChipClocks,
    cold: Option<WifiCold>,
    clocked: Option<WifiClocked>,
    absent: Option<CoexAbsent<WifiClient>>,
    coex: Option<Coexisting<WifiClient>>,
    power: Option<CommonPower<WifiClient>>,
    member: Option<WifiPhyMembership>,
    identity: PhyCalibrationIdentity,
    cache: crate::cache::Cache,
    full_calibration_performed: bool,
    calibration_path: CalibrationPath,
    station: [u8; 6],
    access_point: [u8; 6],
    ieee802154: crate::ieee802154::Rx,
    rng: Rng,
    ready: Option<CaptureReady>,
    projection: [u16; projection::WORDS],
    poisoned: bool,
    hold: Option<crate::hold::Hold>,
    hold_id: u32,
    #[cfg(feature = "diagnostic-radio-health")]
    health: crate::health::Store,
    #[cfg(feature = "diagnostic-radio-health")]
    health_boot: u64,
}

impl Radio {
    #[allow(clippy::too_many_arguments)]
    pub fn place<'storage>(
        storage: &'storage mut core::mem::MaybeUninit<Self>,
        domain: RadioDomain<ChipClocks, ConcurrentPhy>,
        wifi: WifiPartition,
        coex: CoexAbsent<WifiClient>,
        ieee802154: crate::ieee802154::Rx,
        clocks: ChipClocks,
        identity: PhyCalibrationIdentity,
        station: [u8; 6],
        access_point: [u8; 6],
        rng: Rng,
    ) -> &'storage mut Self {
        let radio_fault = domain.fault_handle();
        place_diagnostic_radio!(
            storage,
            Self {
                ieee802154: ieee802154,
                domain: Some(domain),
                radio_fault: radio_fault,
                clocks: clocks,
                cold: Some(WifiCold::from_partition(wifi)),
                clocked: None,
                absent: Some(coex),
                coex: None,
                power: None,
                member: None,
                identity: identity,
                cache => crate::cache::Cache::place,
                full_calibration_performed: false,
                calibration_path: CalibrationPath::FullForCache,
                station: station,
                access_point: access_point,
                rng: rng,
                ready: None,
                projection: [0; projection::WORDS],
                poisoned: false,
                hold: None,
                hold_id: 0,
            }
        )
    }

    // Apply the terminal error policy after the request future is consumed.
    // An async wrapper would retain both that future and another `self` borrow.
    pub fn request_finished(&mut self, result: &Result<Answer>) {
        if matches!(result, Err(RejectReason::Internal)) {
            self.poisoned = true;
            self.ready = None;
            self.cache.fail();
        }
    }

    pub async fn request(
        &mut self,
        request: Request,
        capture_rx: &mut Option<crate::cca_rx::Prepared>,
    ) -> Result<Answer> {
        if self.poisoned
            && !matches!(
                request,
                Request::HoldGet(_)
                    | Request::HoldStop(_)
                    | Request::CacheIdentity(_)
                    | Request::CacheStatus(_)
                    | Request::CacheRead(_)
            )
        {
            return Err(RejectReason::InvalidState);
        }
        if self.hold.is_some()
            && !matches!(
                request,
                Request::HoldGet(_)
                    | Request::HoldStop(_)
                    | Request::CacheIdentity(_)
                    | Request::CacheStatus(_)
                    | Request::CacheRead(_)
            )
        {
            return Err(RejectReason::InvalidState);
        }
        #[cfg(feature = "diagnostic-radio-health")]
        if self.health.active()
            && !matches!(
                request,
                Request::CacheIdentity(_) | Request::CacheStatus(_) | Request::CacheRead(_)
            )
            && !(self.health.armed() && matches!(request, Request::Start(_)))
        {
            return Err(RejectReason::InvalidState);
        }
        match request {
            Request::HoldStart(config) => {
                if !config.valid() {
                    return Err(RejectReason::InvalidConfiguration);
                }
                let current = self.current(config.epoch)?;
                let saved = require_radio(self.domain.as_ref(), self.radio_fault)
                    .attachment()
                    .phy_state()
                    .map_err(|_| RejectReason::InvalidState)?
                    .temperature_tracking_debug();
                let id = self
                    .hold_id
                    .checked_add(1)
                    .ok_or(RejectReason::InvalidState)?;
                if config.policy == HoldPolicy::Forced {
                    require_radio(self.domain.as_mut(), self.radio_fault)
                        .attachment_mut()
                        .set_temperature_tracking_debug(3, 0)
                        .map_err(|_| RejectReason::InvalidState)?;
                }
                self.hold_id = id;
                let started = HoldStarted {
                    id,
                    epoch: current.epoch,
                    ticks: config.ticks,
                    policy: config.policy,
                    channel: current.channel,
                    radio: current.radio,
                    force: if config.policy == HoldPolicy::Forced {
                        [3, 0]
                    } else {
                        [saved.first, saved.second]
                    },
                    period_micros: oer_esp32s31_phy::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
                };
                self.hold = Some(crate::hold::Hold::new(started, [saved.first, saved.second]));
                Ok(Answer::HoldStarted(started))
            }
            Request::HoldGet(config) => {
                let hold = self.hold.as_ref().ok_or(RejectReason::InvalidState)?;
                Ok(Answer::HoldPage(hold.page(config)?))
            }
            Request::HoldStop(config) => {
                let hold = self.hold.as_ref().ok_or(RejectReason::InvalidState)?;
                if config.id != hold.started.id {
                    return Err(RejectReason::InvalidState);
                }
                let mut hold = self.hold.take().ok_or(RejectReason::InvalidState)?;
                hold.running = false;
                let restored = if hold.started.policy == HoldPolicy::Forced {
                    require_radio(self.domain.as_mut(), self.radio_fault)
                        .attachment_mut()
                        .set_temperature_tracking_debug(hold.saved_debug[0], hold.saved_debug[1])
                        .is_ok()
                } else {
                    require_radio(self.domain.as_ref(), self.radio_fault)
                        .attachment()
                        .phy_state()
                        .is_ok_and(|state| {
                            let policy = state.temperature_tracking_debug();
                            [policy.first, policy.second] == hold.saved_debug
                        })
                };
                Ok(Answer::HoldStopped(HoldStopped {
                    id: config.id,
                    samples: hold.samples.len() as u8,
                    complete: !hold.invalid
                        && hold.samples.len() == usize::from(hold.started.ticks),
                    debug_restored: restored,
                }))
            }
            Request::CacheIdentity(_) => {
                Ok(Answer::CacheIdentity(self.cache.identity(self.identity)))
            }
            Request::Start(command) => {
                #[cfg(feature = "diagnostic-radio-health")]
                if self.health.config().is_some_and(|c| {
                    c.channel != command.capture.channel
                        || command.capture.radio != CaptureRadio::Wifi
                        || command.source != CalibrationSource::Full
                }) {
                    return Err(RejectReason::InvalidConfiguration);
                }
                Ok(Answer::Started(self.cache.begin(
                    command,
                    self.identity,
                    EmbassyClock.now(),
                )?))
            }
            Request::CacheWrite(command) => Ok(Answer::CacheWritten(
                self.cache.write(command, EmbassyClock.now())?,
            )),
            Request::CacheComplete(command) => Ok(Answer::CacheCompleted(self.cache.complete(
                command,
                self.identity,
                EmbassyClock.now(),
            )?)),
            Request::CacheStatus(command) => Ok(Answer::CacheStatus(self.cache.status(
                command.ticket,
                self.identity,
                EmbassyClock.now(),
            )?)),
            Request::CacheRead(command) => Ok(Answer::CacheSnapshot(self.cache.read(command)?)),
            Request::Restart(RestartCaptureRadio { epoch }) => {
                let current = self.current(epoch)?;
                let next = epoch.checked_add(1).ok_or(RejectReason::InvalidState)?;
                self.ready = None;
                {
                    let mut exit = core::pin::pin!(self.exit_capture(current.radio));
                    exit.as_mut().await?;
                }
                if !require_radio(self.domain.as_ref(), self.radio_fault)
                    .attachment()
                    .rf_closed()
                {
                    return Err(RejectReason::Internal);
                }
                {
                    let domain = require_radio(self.domain.take(), self.radio_fault);
                    let clocks = &self.clocks;
                    let mut wake =
                        core::pin::pin!(crate::completion::construct(|| wake_concurrent_rf::<
                            _,
                            RomShortDelay,
                        >(
                            &crate::PHY_CLOCK,
                            domain,
                            clocks,
                        )));
                    crate::completion::run(
                        wake.as_mut(),
                        &mut self.domain,
                        crate::completion::restore,
                    )
                    .await
                }
                .map_err(|_| RejectReason::Internal)?;
                crate::sar2_checkpoint::observe(
                    &mut self.domain,
                    self.radio_fault,
                    crate::sar2_checkpoint::Point::AfterRfWake,
                )
                .await?;
                let config = PrepareCapture {
                    channel: current.channel,
                    radio: current.radio,
                };
                {
                    let mut enter = core::pin::pin!(self.enter_capture(config));
                    enter.as_mut().await?;
                }
                if config.radio == CaptureRadio::Wifi {
                    self.prepare_capture_rx(capture_rx).await?;
                }
                let lifecycle = match config.radio {
                    CaptureRadio::Wifi => CaptureLifecycle::Restart,
                    CaptureRadio::Ieee802154 { .. } => CaptureLifecycle::Ieee802154Restart,
                };
                let ready = self.snapshot(next, lifecycle, config)?;
                Ok(Answer::Restarted(CaptureRestarted {
                    ready,
                    rf_closed: true,
                    rf_woken: true,
                }))
            }
            Request::Stop(StopCapture { epoch }) => {
                let current = self.current(epoch)?;
                self.ready = None;
                // The host has finished reading the calibration/MMIO/analog
                // pages. This separate RX experiment cannot affect them.
                let observed = if current.radio == CaptureRadio::Wifi {
                    let wifi = self.clocked.take().ok_or(RejectReason::Internal)?;
                    let prepared = capture_rx.take();
                    let (wifi, result) = crate::cca_rx::run(
                        require_radio(self.domain.as_ref(), self.radio_fault),
                        wifi,
                        current.channel,
                        prepared,
                        #[cfg(feature = "diagnostic-radio-health")]
                        None,
                    )
                    .await;
                    self.clocked = Some(wifi);
                    result
                } else {
                    Ok(())
                };
                let closed = {
                    let mut exit = core::pin::pin!(self.exit_capture(current.radio));
                    exit.as_mut().await
                };
                report_capture_stop(
                    &observed,
                    &closed,
                    require_radio(self.domain.as_ref(), self.radio_fault)
                        .attachment()
                        .rf_closed(),
                )
                .await;
                closed?;
                observed?;
                Ok(Answer::Stopped(CaptureStopped {
                    epoch,
                    rf_closed: require_radio(self.domain.as_ref(), self.radio_fault)
                        .attachment()
                        .rf_closed(),
                }))
            }
            Request::Calibration(ReadCalibrationProjection(window)) => {
                self.current(window.epoch)?;
                let range = window
                    .range(window.epoch, projection::WORDS)
                    .ok_or(RejectReason::InvalidConfiguration)?;
                let values = heapless::Vec::from_slice(&self.projection[range])
                    .map_err(|_| RejectReason::Internal)?;
                Ok(Answer::Calibration(CalibrationProjection {
                    epoch: window.epoch,
                    first: window.first,
                    total: projection::WORDS as u16,
                    values,
                }))
            }
            Request::Registers(ReadRegisterImage(window)) => {
                let ready = self.current(window.epoch)?;
                let range = window
                    .range(ready.epoch, usize::from(ready.registers))
                    .ok_or(RejectReason::InvalidConfiguration)?;
                let mut values = heapless::Vec::new();
                for index in range {
                    let value = require_radio(self.domain.as_ref(), self.radio_fault)
                        .phy_register_image(index)
                        .ok_or(RejectReason::Internal)?;
                    values.push(value).map_err(|_| RejectReason::Internal)?;
                }
                Ok(Answer::Registers(RegisterImage {
                    epoch: ready.epoch,
                    first: window.first,
                    total: ready.registers,
                    values,
                }))
            }
            Request::Analog(ReadAnalogImage(window)) => {
                let ready = self.current(window.epoch)?;
                let range = window
                    .range(ready.epoch, usize::from(ready.analog_bytes))
                    .ok_or(RejectReason::InvalidConfiguration)?;
                let mut values = heapless::Vec::new();
                for index in range {
                    let address = oer_esp32s31_hal::phy::i2c::CAPTURE
                        .get(index)
                        .map(|entry| entry.address)
                        .ok_or(RejectReason::Internal)?;
                    let value = self.read_analog(address).await?;
                    values.push(value).map_err(|_| RejectReason::Internal)?;
                }
                Ok(Answer::Analog(AnalogImage {
                    epoch: ready.epoch,
                    first: window.first,
                    total: ready.analog_bytes,
                    values,
                }))
            }
        }
    }

    pub fn expire_cache(&mut self) {
        self.cache.expire(EmbassyClock.now());
    }
    pub fn release_cache(&mut self) {
        self.cache.release();
    }
    pub fn registration_pending(&self) -> bool {
        self.cache.pending()
    }

    /// Once started, the native registration future runs to its terminal result.
    pub async fn service_registration(&mut self) {
        let result = {
            let mut registration = core::pin::pin!(self.register());
            registration.as_mut().await
        };
        if let Err(_reason) = result {
            self.poisoned = true;
            self.ready = None;
            self.cache.fail();
        }
    }

    async fn register(&mut self) -> Result<()> {
        let (config, candidate) = self.cache.take_registration()?;
        let mut registration_config = PhyRegisterConfig::new(self.identity);
        if let Some(candidate) = candidate {
            registration_config = registration_config.with_calibration_cache(candidate);
        }
        let began = EmbassyClock.now();
        // Consume the native result before awaiting diagnostics: its failure
        // variant retains the complete registration transition.
        let outcome = {
            let mut platform = ();
            let domain = require_radio(self.domain.take(), self.radio_fault);
            let clocks = &self.clocks;
            let observer = capture_observer!(self);
            let mut registration =
                core::pin::pin!(crate::completion::construct(|| register_concurrent_phy::<
                    _,
                    (),
                    RomShortDelay,
                    _,
                >(
                    &crate::PHY_CLOCK,
                    domain,
                    &mut platform,
                    clocks,
                    registration_config,
                    observer,
                )));
            crate::completion::run(registration.as_mut(), &mut self.domain, finish_registration)
                .await
        };
        let outcome = match outcome {
            Ok(outcome) => outcome,
            Err(_) => {
                crate::transport::CONSOLE.note_immediately(&Note::phy_fail("registration"));
                return Err(RejectReason::Internal);
            }
        };

        let registration_us = u32::try_from(
            EmbassyClock
                .now()
                .saturating_duration_since(began)
                .as_micros(),
        )
        .unwrap_or(u32::MAX);
        crate::transport::CONSOLE
            .note_reliably(&Note::registration(
                outcome.full_calibration_performed,
                &DiagPath::of(&outcome.calibration_path),
            ))
            .await;
        self.full_calibration_performed = outcome.full_calibration_performed;
        self.calibration_path = match outcome.calibration_path {
            PhyCalibrationPath::FullForCache => CalibrationPath::FullForCache,
            PhyCalibrationPath::FullUncached => CalibrationPath::FullUncached,
            PhyCalibrationPath::FullAfterRejectedCache => CalibrationPath::FullAfterRejectedCache,
            PhyCalibrationPath::PartialFromCache => CalibrationPath::PartialFromCache,
        };
        crate::tx_rate_checkpoint::observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            crate::tx_rate_checkpoint::Point::AfterPhyRegistration,
        )
        .await?;
        crate::sar2_checkpoint::observe(
            &mut self.domain,
            self.radio_fault,
            crate::sar2_checkpoint::Point::AfterPhyRegistration,
        )
        .await?;
        let cache = require_radio(self.domain.as_ref(), self.radio_fault)
            .attachment()
            .calibration_cache(self.identity)
            .map_err(|_| RejectReason::Internal)?;
        let state = require_radio(self.domain.as_ref(), self.radio_fault)
            .attachment()
            .phy_state()
            .map_err(|_| RejectReason::Internal)?;
        // The vendor reports its full cold-calibration object before Wi-Fi bring-up.
        // Preserve that same immutable registration product across radio restarts.
        self.projection = projection::capture(state, cache.snapshot());
        self.cache.freeze(
            outcome.full_calibration_performed,
            *cache.snapshot(),
            registration_us,
        )?;
        crate::transport::CONSOLE
            .note_reliably(&match config.radio {
                CaptureRadio::Wifi => Note::enter_wifi(config.channel.into()),
                CaptureRadio::Ieee802154 { power_dbm } => {
                    Note::enter_ieee802154(config.channel.into(), power_dbm.into())
                }
            })
            .await;
        {
            let mut enter = core::pin::pin!(self.enter_capture(config));
            enter.as_mut().await?;
        }
        let lifecycle = match config.radio {
            CaptureRadio::Wifi => CaptureLifecycle::Cold,
            CaptureRadio::Ieee802154 { .. } => CaptureLifecycle::Ieee802154,
        };
        let ready = self.snapshot(1, lifecycle, config)?;
        #[cfg(feature = "diagnostic-radio-health")]
        if self.health.config().is_some() {
            self.health.bind_epoch(ready.epoch, ready.channel)?;
        }
        self.cache.ready(ready)
    }

    pub fn deadline(&self) -> Option<oer_time::Instant> {
        let hold = self
            .hold
            .as_ref()
            .filter(|hold| hold.running)
            .map(|hold| hold.deadline);
        [
            hold,
            self.cache.deadline(),
            crate::ieee802154::poll_deadline(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    #[inline(never)]
    pub async fn service_hold(&mut self) {
        let Some(hold) = self.hold.as_mut().filter(|hold| hold.running) else {
            return;
        };
        if EmbassyClock.now() < hold.deadline {
            return;
        }
        hold.deadline = EmbassyClock
            .now()
            .saturating_add(oer_time::Duration::from_micros(
                oer_esp32s31_phy::state::client::DEFAULT_PLL_TRACK_PERIOD_MICROS,
            ));
        if hold.started.policy == HoldPolicy::Forced
            && hold.started.radio == CaptureRadio::Wifi
            && require_radio(self.domain.as_mut(), self.radio_fault)
                .attachment_mut()
                .force_wifi_i2c_tracking_debug()
                .is_err()
        {
            hold.invalid = true;
            hold.running = false;
            return;
        }
        if hold.started.policy == HoldPolicy::Forced
            && require_radio(self.domain.as_mut(), self.radio_fault)
                .attachment_mut()
                .force_power_tracking_debug(hold.started.radio == CaptureRadio::Wifi)
                .is_err()
        {
            hold.invalid = true;
            hold.running = false;
            return;
        }
        let mut trace = oer_esp32s31_phy::tracking::observation::ReadinessTrace::default();
        let metrics = core::cell::RefCell::new(crate::hold::metrics::Metrics::default());
        let mut uart = crate::hold::Uart;
        let mut observer = crate::hold::Observer::new(
            hold.samples.len() as u8 + 1,
            &mut hold.engine,
            Some(&mut trace),
            &crate::PHY_CLOCK,
            &mut uart,
        );
        observer.metrics = Some(&metrics);
        let (domain, tracked) = {
            let mut port = ();
            let domain = require_radio(self.domain.take(), self.radio_fault);

            let observer = &mut observer;
            let mut future =
                core::pin::pin!(crate::completion::construct(|| track_concurrent_phy::<
                    _,
                    (),
                    RomShortDelay,
                    _,
                >(
                    domain,
                    &mut port,
                    &crate::PHY_CLOCK,
                    observer,
                )));
            crate::hold::metrics::observe(
                Boundary::new(future.as_mut()),
                &crate::PHY_CLOCK,
                &metrics,
            )
            .await
        };
        self.domain = Some(domain);
        let (committed, failed) = match tracked {
            Ok(ConcurrentTrackingTick::NotDue) => return,
            Ok(ConcurrentTrackingTick::Tracked(outcome)) => {
                let committed = u8::from(outcome.rfpll_corrected)
                    | (u8::from(outcome.calibration.common) << 1)
                    | (u8::from(outcome.calibration.transmit) << 2);
                (
                    committed,
                    outcome.tracking_inhibited
                        || outcome.clients.wifi() != (hold.started.radio == CaptureRadio::Wifi)
                        || outcome.clients.bluetooth_ieee802154()
                            != (hold.started.radio != CaptureRadio::Wifi)
                        || (hold.started.policy == HoldPolicy::Forced && committed != 7),
                )
            }
            failure => {
                // Terminal diagnostic evidence only, outside every measured interval.
                crate::transport::CONSOLE
                    .note_reliably(&Note::hold_fail(
                        hold.samples.len() as u32 + 1,
                        &DiagPath::of(&failure),
                    ))
                    .await;
                self.poisoned = true;
                (0, true)
            }
        };
        observer.report(hold.samples.len() as u8 + 1).await;
        let recorder = observer.recorder;
        let measured = *metrics.borrow();
        crate::hold::metrics::report(hold.samples.len() as u8 + 1, &measured, &mut uart).await;
        crate::hold::report_preparation(&trace, &hold.engine).await;
        crate::hold::finish_sample(hold, recorder, committed, failed);
    }

    fn current(&self, epoch: u32) -> Result<CaptureReady> {
        self.ready
            .filter(|ready| ready.epoch == epoch && epoch != 0)
            .ok_or(RejectReason::InvalidState)
    }

    fn snapshot(
        &mut self,
        epoch: u32,
        lifecycle: CaptureLifecycle,
        config: PrepareCapture,
    ) -> Result<CaptureReady> {
        let ready = CaptureReady {
            epoch,
            lifecycle,
            full_calibration_performed: self.full_calibration_performed,
            path: self.calibration_path,
            projection_schema: projection::SCHEMA,
            calibration_words: projection::WORDS as u16,
            registers: require_radio(self.domain.as_ref(), self.radio_fault)
                .phy_register_image_len()
                .try_into()
                .map_err(|_| RejectReason::Internal)?,
            analog_bytes: oer_esp32s31_hal::phy::i2c::CAPTURE
                .len()
                .try_into()
                .map_err(|_| RejectReason::Internal)?,
            channel: config.channel,
            radio: config.radio,
        };
        self.ready = Some(ready);
        Ok(ready)
    }

    async fn prepare_capture_rx(
        &mut self,
        capture_rx: &mut Option<crate::cca_rx::Prepared>,
    ) -> Result<()> {
        let wifi = self.clocked.take().ok_or(RejectReason::Internal)?;
        let (wifi, prepared) = crate::cca_rx::prepare(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            capture_rx.take(),
        )
        .await;
        self.clocked = Some(wifi);
        *capture_rx = Some(prepared?);
        Ok(())
    }

    async fn enter_capture(&mut self, config: PrepareCapture) -> Result<()> {
        match config.radio {
            CaptureRadio::Wifi => {
                let mut enter = core::pin::pin!(self.enter(config.channel));
                Boundary::new(enter.as_mut()).await
            }
            CaptureRadio::Ieee802154 { power_dbm } => {
                let mut enter = core::pin::pin!(self.ieee802154.enter(
                    &mut self.domain,
                    self.radio_fault,
                    &self.clocks,
                    config.channel,
                    power_dbm,
                ));
                Boundary::new(enter.as_mut()).await
            }
        }
    }

    async fn exit_capture(&mut self, radio: CaptureRadio) -> Result<()> {
        match radio {
            CaptureRadio::Wifi => {
                let mut exit = core::pin::pin!(self.exit());
                Boundary::new(exit.as_mut()).await
            }
            CaptureRadio::Ieee802154 { .. } => {
                let mut exit =
                    core::pin::pin!(self.ieee802154.exit(&mut self.domain, self.radio_fault));
                Boundary::new(exit.as_mut()).await
            }
        }
    }

    async fn enter(&mut self, channel: u8) -> Result<()> {
        let absent = self.absent.take().ok_or(RejectReason::Internal)?;
        let (coex, _) = require_radio(self.domain.as_mut(), self.radio_fault).enter_coex(absent);
        self.coex = Some(coex);
        let cold = self.cold.take().ok_or(RejectReason::Internal)?;
        let (powered, power) = cold
            .power_up(
                require_radio(self.domain.as_mut(), self.radio_fault),
                &self.clocks,
            )
            .map_err(|_| RejectReason::Internal)?;
        self.power = Some(power);
        crate::tx_rate_checkpoint::observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            crate::tx_rate_checkpoint::Point::AfterWifiCommonPower,
        )
        .await?;
        let mut wifi = powered
            .enable_clocks(
                require_radio(self.domain.as_mut(), self.radio_fault),
                &self.clocks,
            )
            .map_err(|_| RejectReason::Internal)?;
        wifi.set_initialized(require_radio(self.domain.as_mut(), self.radio_fault), true)
            .map_err(|_| RejectReason::Internal)?;
        let ready = phy_ready(require_radio(self.domain.as_mut(), self.radio_fault))
            .map_err(|_| RejectReason::Internal)?;
        let PhyClientAccess::Available(available) = ready.client::<WifiPhyMembership>() else {
            return Err(RejectReason::Internal);
        };
        let joined =
            join_wifi(available, &wifi, &crate::PHY_CLOCK).map_err(|_| RejectReason::Internal)?;
        let due = joined.acquired == ConcurrentAcquire::TrackingDue;
        self.member = Some(joined.membership);
        if due {
            {
                let mut platform = ();
                let domain = require_radio(self.domain.take(), self.radio_fault);
                let observer = capture_observer!(self);

                let mut maintain =
                    core::pin::pin!(crate::completion::construct(|| maintain_concurrent_phy::<
                        _,
                        (),
                        RomShortDelay,
                        _,
                    >(
                        &crate::PHY_CLOCK,
                        domain,
                        &mut platform,
                        observer,
                    )));
                crate::completion::run(
                    maintain.as_mut(),
                    &mut self.domain,
                    crate::completion::restore,
                )
                .await
            }
            .map_err(|_| RejectReason::Internal)?;
        }
        use crate::cca_checkpoint::{Point, observe};
        wifi = observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            Point::BeforeRx,
        )
        .await?;
        set_wifi_rx(
            require_radio(self.domain.as_mut(), self.radio_fault),
            self.member.as_ref().ok_or(RejectReason::Internal)?,
            true,
        );
        wifi = observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            Point::AfterRx,
        )
        .await?;
        wifi = observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            Point::BeforeChannel,
        )
        .await?;
        let transaction = require_radio(self.domain.take(), self.radio_fault)
            .detach(oer_esp32s31_hal::domain::transaction::Operation::Channel);
        let mut platform = ();
        let port = wifi.channel_transaction(&mut platform, transaction);
        let (radio, selected) = select_concurrent_wifi_channel::<RomShortDelay, _, _, _>(
            &crate::PHY_CLOCK,
            u16::from(channel),
            0,
            port,
            &mut capture_observer!(self),
        )
        .await;
        self.domain = Some(radio);
        selected.map_err(|_| RejectReason::Internal)?;
        wifi = observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            Point::AfterChannel,
        )
        .await?;
        let tx = require_radio(self.domain.as_ref(), self.radio_fault)
            .attachment()
            .phy_state()
            .map_err(|_| RejectReason::Internal)?
            .tx_target_power_profile();
        let mut sources = Sources {
            tx,
            rng: &mut self.rng,
            pti: [
                MacCoexEvent::Event1,
                MacCoexEvent::Event3,
                MacCoexEvent::Event10,
                MacCoexEvent::Event15,
            ]
            .map(|event| {
                MacCoexPti::new(
                    require_radio(self.domain.as_ref(), self.radio_fault)
                        .coex_pti(event.coex_event())
                        .value(),
                )
                .expect("generated coexistence PTI fits")
            }),
        };
        wifi = observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            Point::BeforeMac,
        )
        .await?;
        initialize_wifi_mac(
            &mut sources,
            &mut wifi.cold_mac_hal(require_radio(self.domain.as_mut(), self.radio_fault)),
            &crate::PHY_CLOCK,
            MacColdStartConfig {
                handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
                station_address: self.station,
                access_point_address: self.access_point,
            },
        )
        .await
        .map_err(|_| RejectReason::Internal)?;
        wifi = observe(
            require_radio(self.domain.as_ref(), self.radio_fault),
            wifi,
            Point::AfterMac,
        )
        .await?;
        // Diagnostic observation window only: allow live CCA state time to
        // evolve without changing the radio, MAC, calibration or DMA setup.
        for sample in 0..5 {
            EmbassyClock
                .wait_until(
                    EmbassyClock
                        .now()
                        .saturating_add(oer_time::Duration::from_micros(200_000)),
                )
                .await;
            wifi = observe(
                require_radio(self.domain.as_ref(), self.radio_fault),
                wifi,
                Point::Settled(sample),
            )
            .await?;
        }
        self.clocked = Some(wifi);
        Ok(())
    }

    async fn exit(&mut self) -> Result<()> {
        let wifi = self.clocked.take().ok_or(RejectReason::Internal)?;
        let member = self.member.take().ok_or(RejectReason::Internal)?;
        set_wifi_rx(
            require_radio(self.domain.as_mut(), self.radio_fault),
            &member,
            false,
        );
        if !leave_wifi(
            require_radio(self.domain.as_mut(), self.radio_fault),
            &wifi,
            member,
        )
        .map_err(|_| RejectReason::Internal)?
        {
            return Err(RejectReason::Internal);
        }
        let powered = wifi
            .release_clocks(require_radio(self.domain.as_mut(), self.radio_fault))
            .map_err(|_| RejectReason::Internal)?
            .into_powered();
        let (absent, _) = require_radio(self.domain.as_mut(), self.radio_fault)
            .leave_coex(self.coex.take().ok_or(RejectReason::Internal)?);
        self.cold = Some(
            powered
                .power_down(
                    require_radio(self.domain.as_mut(), self.radio_fault),
                    self.power.take().ok_or(RejectReason::Internal)?,
                    &absent,
                )
                .map_err(|_| RejectReason::Internal)?,
        );
        self.absent = Some(absent);
        crate::sar2_checkpoint::observe(
            &mut self.domain,
            self.radio_fault,
            crate::sar2_checkpoint::Point::BeforeRfClose,
        )
        .await?;
        {
            let mut platform = ();
            let domain = require_radio(self.domain.take(), self.radio_fault);
            let mut close =
                core::pin::pin!(crate::completion::construct(|| close_concurrent_rf::<
                    _,
                    (),
                    RomShortDelay,
                >(
                    &crate::PHY_CLOCK,
                    domain,
                    &mut platform,
                )));
            crate::completion::run(close.as_mut(), &mut self.domain, crate::completion::restore)
                .await
        }
        .map_err(|_| RejectReason::Internal)?;
        Ok(())
    }

    async fn read_analog(&mut self, address: Address) -> Result<u8> {
        let mut transaction = require_radio(self.domain.take(), self.radio_fault)
            .detach(oer_esp32s31_hal::domain::transaction::Operation::Maintain);
        let result = async {
            let registers = transaction.hardware_mut();
            let mut started = false;
            for _ in 0..HARDWARE_EDGE_LIMIT {
                if !started {
                    started = i2c::try_start_read(registers, address).is_ok();
                }
                if started {
                    if let Ok(value) = i2c::try_finish_read(registers, address) {
                        return Ok(value);
                    }
                }
                EmbassyClock
                    .wait_until(
                        EmbassyClock
                            .now()
                            .saturating_add(oer_time::Duration::from_micros(1)),
                    )
                    .await;
            }
            Err(RejectReason::Internal)
        }
        .await;
        self.domain = Some(transaction.reunite());
        result
    }
}

// Collapse the large registration result in its retained return place before
// diagnostics or radio bring-up; failed transitions are still dropped there.
#[inline(never)]
fn finish_registration(
    destination: &mut Option<RadioDomain<ChipClocks, ConcurrentPhy>>,
    returned: oer_memory::Placed<
        '_,
        (
            RadioDomain<ChipClocks, ConcurrentPhy>,
            core::result::Result<ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure>,
        ),
    >,
) -> Result<PhyRegisterOutcome> {
    let (domain, result) = returned.split();
    *destination = Some(domain.read());
    match &*result {
        Ok(registration) => Ok(registration.outcome()),
        Err(_) => Err(RejectReason::Internal),
    }
}

/// Keep operation scratch frames off unrelated branches of the radio task.
#[inline(never)]
pub fn poll_operation<F: core::future::Future>(
    future: core::pin::Pin<&mut F>,
    context: &mut core::task::Context<'_>,
) -> core::task::Poll<F::Output> {
    future.poll(context)
}

#[inline(never)]
async fn report_capture_stop(observed: &Result<()>, closed: &Result<()>, rf_closed: bool) {
    crate::transport::CONSOLE
        .note_reliably(&Note::capture_stop(
            outcome_name(observed),
            outcome_name(closed),
            rf_closed,
        ))
        .await;
}

/// `Ok` or the rejection's name, for the capture-stop note.
fn outcome_name(result: &Result<()>) -> &'static str {
    match result {
        Ok(()) => "Ok",
        Err(RejectReason::BootId) => "BootId",
        Err(RejectReason::SessionId) => "SessionId",
        Err(RejectReason::InvalidState) => "InvalidState",
        Err(RejectReason::InvalidConfiguration) => "InvalidConfiguration",
        Err(RejectReason::Unsupported) => "Unsupported",
        Err(RejectReason::Busy) => "Busy",
        Err(RejectReason::Internal) => "Internal",
    }
}

struct Sources<'a> {
    tx: PhyTxTargetPowerProfile,
    pti: [MacCoexPti; 4],
    rng: &'a mut Rng,
}
impl MacCoexPtiSource for Sources<'_> {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        self.pti[match event {
            MacCoexEvent::Event1 => 0,
            MacCoexEvent::Event3 => 1,
            MacCoexEvent::Event10 => 2,
            MacCoexEvent::Event15 => 3,
        }]
    }
}
impl MacTxPowerSource for Sources<'_> {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let pair = self.tx.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}
impl MacDelayEntropy for Sources<'_> {
    fn mac_delay_random(&mut self) -> u32 {
        self.rng.random()
    }
}
impl MacSlowClockCalibrationSource for Sources<'_> {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration {
        MacSlowClockCalibration::Unavailable
    }
}

#[cfg(feature = "diagnostic-radio-health")]
impl Radio {
    pub fn set_health_boot(&mut self, boot: u64) {
        self.health_boot = boot;
    }
    pub async fn health_request(
        &mut self,
        request: crate::health::Request,
        capture_rx: &mut Option<crate::cca_rx::Prepared>,
    ) -> Result<crate::health::Answer> {
        use crate::health::{self, Answer as A, Observer, Request};
        use oer_hil_protocol::health::*;
        let result = match request {
            Request::Begin(config) => {
                if self.cache.identity(self.identity).active_ticket.is_some() || self.poisoned {
                    return Err(RejectReason::InvalidState);
                }
                A::Started(self.health.begin(config, self.health_boot)?)
            }
            Request::Window(RunHealthWindow(identity)) => {
                self.health.validate_window(identity)?;
                if self.poisoned {
                    return Err(RejectReason::InvalidState);
                }
                let current = self.current(identity.epoch)?;
                let started_micros = health::now();
                if let Ok(state) = require_radio(self.domain.as_ref(), self.radio_fault)
                    .attachment()
                    .phy_state()
                {
                    health::retained_detector(&mut self.health, state);
                    health::temperature(&mut self.health, state, current.channel);
                }
                let wifi = self.clocked.take().ok_or(RejectReason::InvalidState)?;
                let (wifi, rx) = crate::cca_rx::run(
                    require_radio(self.domain.as_ref(), self.radio_fault),
                    wifi,
                    current.channel,
                    capture_rx.take(),
                    Some(&mut self.health),
                )
                .await;
                self.clocked = Some(wifi);
                // The finite RX transaction has already stopped/reclaimed its ring.
                // Natural tracking now uses its ordinary exclusive PHY path.
                health::inspection(
                    &mut self.health,
                    require_radio(self.domain.as_ref(), self.radio_fault).attachment(),
                );
                let tracking = if rx.is_ok() {
                    {
                        let (domain, result) = track_concurrent_phy::<_, (), RomShortDelay, _>(
                            require_radio(self.domain.take(), self.radio_fault),
                            &mut (),
                            &crate::PHY_CLOCK,
                            Observer {
                                store: &mut self.health,
                            },
                        )
                        .await;
                        self.domain = Some(domain);
                        result
                    }
                } else {
                    self.health.fail();
                    return Ok(A::Window(
                        self.health_failed_window(identity, started_micros)?,
                    ));
                };
                let at = health::now();
                let (due, disposition, committed, tracking_failed) = match tracking {
                    Ok(ConcurrentTrackingTick::NotDue) => {
                        (Due::NotDue, Disposition::Skipped, 0, false)
                    }
                    Ok(ConcurrentTrackingTick::Tracked(outcome)) => {
                        let committed = u8::from(outcome.rfpll_corrected)
                            | (u8::from(outcome.calibration.common) << 1)
                            | (u8::from(outcome.calibration.transmit) << 2);
                        for (operation, bit) in [
                            (TrackingOperation::Rfpll, 1),
                            (TrackingOperation::Common, 2),
                            (TrackingOperation::Transmit, 4),
                        ] {
                            self.health.push(Record::Tracking(Tracking {
                                condition: None,
                                operation,
                                due: Due::Unknown,
                                disposition: if committed & bit != 0 {
                                    Disposition::Ran
                                } else {
                                    Disposition::Skipped
                                },
                                at_micros: at,
                                committed,
                            }));
                        }
                        (Due::Due, Disposition::Ran, committed, false)
                    }
                    Ok(ConcurrentTrackingTick::Unavailable(_)) | Err(_) => {
                        self.poisoned = true;
                        (Due::Unknown, Disposition::Failed, 0, true)
                    }
                };
                self.health.push(Record::Tracking(Tracking {
                    condition: None,
                    operation: TrackingOperation::Tick,
                    due,
                    disposition,
                    at_micros: at,
                    committed,
                }));
                if let Ok(state) = require_radio(self.domain.as_ref(), self.radio_fault)
                    .attachment()
                    .phy_state()
                {
                    health::temperature(&mut self.health, state, current.channel);
                }
                let window = HealthWindow {
                    identity,
                    started_micros,
                    finished_micros: health::now(),
                    rx_cleaned_up: true,
                    tracking_failed,
                };
                self.health.window_finished(window)?;
                A::Window(window)
            }
            Request::Finish(FinishHealth(identity)) => {
                self.health.validate_finish(identity)?;
                // Do not start the legacy StopCapture RX experiment a second time.
                if let Some(current) = self.ready.take() {
                    if self.exit_capture(current.radio).await.is_err() {
                        self.health.fail();
                        self.poisoned = true;
                    }
                }
                A::Finished(
                    self.health.finish(
                        identity,
                        require_radio(self.domain.as_ref(), self.radio_fault)
                            .attachment()
                            .rf_closed(),
                    )?,
                )
            }
            Request::Counters(ReadHealthCounters(r)) => A::Counters(self.health.counters(r)?),
            Request::Signals(ReadHealthSignals(r)) => A::Signals(self.health.signals(r)?),
            Request::Tracking(ReadHealthTracking(r)) => A::Tracking(self.health.tracking(r)?),
            Request::Measurements(ReadHealthMeasurements(r)) => {
                A::Measurements(self.health.measurements(r)?)
            }
            Request::Release(ReleaseHealth(identity)) => {
                A::Released(self.health.release(identity)?)
            }
        };
        Ok(result)
    }
    fn health_failed_window(
        &mut self,
        identity: oer_hil_protocol::health::HealthIdentity,
        started_micros: u64,
    ) -> Result<oer_hil_protocol::health::HealthWindow> {
        let window = oer_hil_protocol::health::HealthWindow {
            identity,
            started_micros,
            finished_micros: crate::health::now(),
            rx_cleaned_up: false,
            tracking_failed: true,
        };
        self.health.window_finished(window)?;
        self.poisoned = true;
        Ok(window)
    }
}

pub(crate) fn require_radio<T>(
    owner: Option<T>,
    fault: oer_esp32s31_hal::domain::transaction::FaultHandle,
) -> T {
    fault.require_owner(owner)
}
