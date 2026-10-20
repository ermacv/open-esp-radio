use crate::{Answer, Request, projection};
use esp_hal::rng::RngOwner;
use oer_embassy_time::EmbassyClock;
use oer_esp32s31_hal::{
    domain::{CoexAbsent, Coexisting, CommonPower, RadioDomain, WifiClient},
    ieee80211::{
        client::{WifiClocked, WifiCold},
        init::*,
    },
    phy::i2c::{self, PhyI2cAddress},
    root::WifiPartition,
    types::MacTxPowerPair,
};
use oer_esp32s31_phy::{
    concurrent::{ConcurrentAcquire, ConcurrentPhy, PhyClientAccess, phy_ready},
    wifi_client::{WifiPhyMembership, join_wifi, leave_wifi, set_wifi_rx},
    *,
};
use oer_espressif_esp_hal_radio::EspHalClocks;
use oer_hil_protocol::{base::RejectReason, phy::*};
use oer_time::{Clock, Timer};

type Result<T> = core::result::Result<T, RejectReason>;

pub struct Radio {
    domain: RadioDomain<oer_espressif_esp_hal_radio::EspHal, ConcurrentPhy>,
    clocks: EspHalClocks,
    cold: Option<WifiCold>,
    clocked: Option<WifiClocked>,
    absent: Option<CoexAbsent<WifiClient>>,
    coex: Option<Coexisting<WifiClient>>,
    power: Option<CommonPower<WifiClient>>,
    member: Option<WifiPhyMembership>,
    identity: PhyCalibrationIdentity,
    station: [u8; 6],
    access_point: [u8; 6],
    ieee802154: crate::ieee802154::Rx,
    rng: RngOwner<'static>,
    ready: Option<CaptureReady>,
    projection: [u16; projection::WORDS],
    registered: bool,
    poisoned: bool,
    hold: Option<crate::hold::Hold>,
    hold_id: u32,
}

impl Radio {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        domain: RadioDomain<oer_espressif_esp_hal_radio::EspHal, ConcurrentPhy>,
        wifi: WifiPartition,
        coex: CoexAbsent<WifiClient>,
        ieee802154: crate::ieee802154::Rx,
        clocks: EspHalClocks,
        identity: PhyCalibrationIdentity,
        station: [u8; 6],
        access_point: [u8; 6],
        rng: RngOwner<'static>,
    ) -> Self {
        Self {
            ieee802154,
            domain,
            clocks,
            cold: Some(WifiCold::from_partition(wifi)),
            clocked: None,
            absent: Some(coex),
            coex: None,
            power: None,
            member: None,
            identity,
            station,
            access_point,
            rng,
            ready: None,
            projection: [0; projection::WORDS],
            registered: false,
            poisoned: false,
            hold: None,
            hold_id: 0,
        }
    }

    pub async fn request(&mut self, request: Request) -> Result<Answer> {
        if self.poisoned && !matches!(request, Request::HoldGet(_) | Request::HoldStop(_)) {
            return Err(RejectReason::InvalidState);
        }
        let result = self.execute(request).await;
        if matches!(result, Err(RejectReason::Internal)) {
            self.poisoned = true;
            self.ready = None;
        }
        result
    }

    async fn execute(&mut self, request: Request) -> Result<Answer> {
        if self.hold.is_some() && !matches!(request, Request::HoldGet(_) | Request::HoldStop(_)) {
            return Err(RejectReason::InvalidState);
        }
        match request {
            Request::HoldStart(config) => {
                if !config.valid() {
                    return Err(RejectReason::InvalidConfiguration);
                }
                let current = self.current(config.epoch)?;
                if !matches!(current.radio, CaptureRadio::Ieee802154 { .. }) {
                    return Err(RejectReason::InvalidConfiguration);
                }
                let saved = self
                    .domain
                    .attachment()
                    .phy_state()
                    .map_err(|_| RejectReason::InvalidState)?
                    .temperature_tracking_debug();
                let id = self
                    .hold_id
                    .checked_add(1)
                    .ok_or(RejectReason::InvalidState)?;
                self.domain
                    .attachment_mut()
                    .set_temperature_tracking_debug(3, 0)
                    .map_err(|_| RejectReason::InvalidState)?;
                self.hold_id = id;
                let started = HoldStarted {
                    id,
                    epoch: current.epoch,
                    ticks: config.ticks,
                    channel: current.channel,
                    radio: current.radio,
                    force: [3, 0],
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
                let restored = self
                    .domain
                    .attachment_mut()
                    .set_temperature_tracking_debug(hold.saved_debug[0], hold.saved_debug[1])
                    .is_ok();
                Ok(Answer::HoldStopped(HoldStopped {
                    id: config.id,
                    samples: hold.samples.len() as u8,
                    complete: !hold.invalid
                        && hold.samples.len() == usize::from(hold.started.ticks),
                    debug_restored: restored,
                }))
            }
            Request::Prepare(config) => {
                if !config.valid() {
                    return Err(RejectReason::InvalidConfiguration);
                }
                if self.registered {
                    return Err(RejectReason::InvalidState);
                }
                let registration = register_concurrent_phy::<_, (), RomShortDelay, _>(
                    &EmbassyClock,
                    &mut self.domain,
                    &mut (),
                    &self.clocks,
                    PhyRegisterConfig::new(self.identity),
                    NoopPhyTargetObserver,
                )
                .await
                .map_err(|_| {
                    oer_esp32s31_board::rom::line(c"OPEN_RADIO_HIL phy=FAIL step=registration");
                    RejectReason::Internal
                })?;
                let outcome = registration.outcome();
                oer_esp32s31_board::rom::print_fmt(format_args!(
                    "OPEN_RADIO_HIL phy=registration full={} path={:?}\n",
                    outcome.full_calibration_performed, outcome.calibration_path
                ));
                if !outcome.full_calibration_performed
                    || outcome.calibration_path != PhyCalibrationPath::FullForCache
                {
                    return Err(RejectReason::Internal);
                }
                crate::tx_rate_checkpoint::observe(
                    &self.domain,
                    crate::tx_rate_checkpoint::Point::AfterPhyRegistration,
                )?;
                crate::sar2_checkpoint::observe(
                    &mut self.domain,
                    crate::sar2_checkpoint::Point::AfterPhyRegistration,
                )
                .await?;
                let cache = self
                    .domain
                    .attachment()
                    .calibration_cache(self.identity)
                    .map_err(|_| RejectReason::Internal)?;
                let state = self
                    .domain
                    .attachment()
                    .phy_state()
                    .map_err(|_| RejectReason::Internal)?;
                // The vendor reports its full cold-calibration object before Wi-Fi bring-up.
                // Preserve that same immutable registration product across radio restarts.
                self.projection = projection::capture(state, cache.snapshot());
                self.registered = true;
                oer_esp32s31_board::rom::print_fmt(format_args!(
                    "OPEN_RADIO_HIL phy=enter config={config:?}\n"
                ));
                self.enter_capture(config).await?;
                let lifecycle = match config.radio {
                    CaptureRadio::Wifi => CaptureLifecycle::Cold,
                    CaptureRadio::Ieee802154 { .. } => CaptureLifecycle::Ieee802154,
                };
                let ready = self.snapshot(1, lifecycle, config)?;
                Ok(Answer::Ready(ready))
            }
            Request::Restart(RestartCaptureRadio { epoch }) => {
                let current = self.current(epoch)?;
                let next = epoch.checked_add(1).ok_or(RejectReason::InvalidState)?;
                self.ready = None;
                self.exit_capture(current.radio).await?;
                if !self.domain.attachment().rf_closed() {
                    return Err(RejectReason::Internal);
                }
                wake_concurrent_rf::<_, RomShortDelay>(
                    &EmbassyClock,
                    &mut self.domain,
                    &self.clocks,
                )
                .await
                .map_err(|_| RejectReason::Internal)?;
                crate::sar2_checkpoint::observe(
                    &mut self.domain,
                    crate::sar2_checkpoint::Point::AfterRfWake,
                )
                .await?;
                let config = PrepareCapture {
                    channel: current.channel,
                    radio: current.radio,
                };
                self.enter_capture(config).await?;
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
                self.exit_capture(current.radio).await?;
                Ok(Answer::Stopped(CaptureStopped {
                    epoch,
                    rf_closed: self.domain.attachment().rf_closed(),
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
                    let value = self
                        .domain
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
                    let address = PhyI2cAddress::image(index).ok_or(RejectReason::Internal)?;
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

    pub fn hold_deadline(&self) -> Option<oer_time::Instant> {
        self.hold
            .as_ref()
            .filter(|hold| hold.running)
            .map(|hold| hold.deadline)
    }

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
        let mut observer = crate::hold::Observer::new(hold.samples.len() as u8 + 1);
        let tracked = track_concurrent_phy::<_, (), RomShortDelay, _>(
            &mut self.domain,
            &mut (),
            &EmbassyClock,
            &mut observer,
        )
        .await;
        let (committed, failed) = match tracked {
            Ok(ConcurrentTrackingTick::NotDue) => return,
            Ok(ConcurrentTrackingTick::Tracked(outcome)) => {
                let committed = u8::from(outcome.rfpll_corrected)
                    | (u8::from(outcome.calibration.common) << 1)
                    | (u8::from(outcome.calibration.transmit) << 2);
                (
                    committed,
                    outcome.tracking_inhibited
                        || outcome.clients.wifi()
                        || !outcome.clients.bluetooth_ieee802154()
                        || committed != 7,
                )
            }
            failure => {
                // Terminal diagnostic evidence only, outside every measured interval.
                oer_esp32s31_board::rom::print_fmt(format_args!(
                    "OPEN_RADIO_HIL hold=FAIL tick={} result={failure:?}\n",
                    hold.samples.len() + 1,
                ));
                self.poisoned = true;
                (0, true)
            }
        };
        // Print only after tracking returns, outside all measured intervals.
        oer_esp32s31_board::rom::print_fmt(format_args!(
            "OPEN_RADIO_HIL sar_tail tick={} {:?}\n",
            hold.samples.len() + 1,
            observer.1,
        ));
        let sample = observer.0.finish(committed, failed);
        hold.invalid |= sample.invalid;
        if hold.samples.push(sample).is_err() {
            hold.invalid = true;
        }
        hold.running = !hold.invalid && hold.samples.len() < usize::from(hold.started.ticks);
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
            full_calibration_performed: true,
            path: CalibrationPath::FullForCache,
            projection_schema: projection::SCHEMA,
            calibration_words: projection::WORDS as u16,
            registers: self
                .domain
                .phy_register_image_len()
                .try_into()
                .map_err(|_| RejectReason::Internal)?,
            analog_bytes: PhyI2cAddress::IMAGE_LEN
                .try_into()
                .map_err(|_| RejectReason::Internal)?,
            channel: config.channel,
            radio: config.radio,
        };
        self.ready = Some(ready);
        Ok(ready)
    }

    async fn enter_capture(&mut self, config: PrepareCapture) -> Result<()> {
        match config.radio {
            CaptureRadio::Wifi => self.enter(config.channel).await,
            CaptureRadio::Ieee802154 { power_dbm } => {
                self.ieee802154
                    .enter(&mut self.domain, &self.clocks, config.channel, power_dbm)
                    .await
            }
        }
    }

    async fn exit_capture(&mut self, radio: CaptureRadio) -> Result<()> {
        match radio {
            CaptureRadio::Wifi => self.exit().await,
            CaptureRadio::Ieee802154 { .. } => self.ieee802154.exit(&mut self.domain).await,
        }
    }

    async fn enter(&mut self, channel: u8) -> Result<()> {
        let absent = self.absent.take().ok_or(RejectReason::Internal)?;
        let (coex, _) = self.domain.enter_coex(absent);
        self.coex = Some(coex);
        let cold = self.cold.take().ok_or(RejectReason::Internal)?;
        let (powered, power) = cold
            .power_up(&mut self.domain, &self.clocks)
            .map_err(|_| RejectReason::Internal)?;
        self.power = Some(power);
        crate::tx_rate_checkpoint::observe(
            &self.domain,
            crate::tx_rate_checkpoint::Point::AfterWifiCommonPower,
        )?;
        let mut wifi = powered
            .enable_clocks(&mut self.domain, &self.clocks)
            .map_err(|_| RejectReason::Internal)?;
        wifi.set_initialized(&mut self.domain, true)
            .map_err(|_| RejectReason::Internal)?;
        let ready = phy_ready(&mut self.domain).map_err(|_| RejectReason::Internal)?;
        let PhyClientAccess::Available(available) = ready.client::<WifiPhyMembership>() else {
            return Err(RejectReason::Internal);
        };
        let joined =
            join_wifi(available, &wifi, &EmbassyClock).map_err(|_| RejectReason::Internal)?;
        let due = joined.acquired == ConcurrentAcquire::TrackingDue;
        self.member = Some(joined.membership);
        if due {
            maintain_concurrent_phy::<_, (), RomShortDelay, _>(
                &EmbassyClock,
                &mut self.domain,
                &mut (),
                NoopPhyTargetObserver,
            )
            .await
            .map_err(|_| RejectReason::Internal)?;
        }
        use crate::cca_checkpoint::{Point, observe};
        wifi = observe(&self.domain, wifi, Point::BeforeRx)?;
        set_wifi_rx(
            &mut self.domain,
            self.member.as_ref().ok_or(RejectReason::Internal)?,
            true,
        );
        wifi = observe(&self.domain, wifi, Point::AfterRx)?;
        wifi = observe(&self.domain, wifi, Point::BeforeChannel)?;
        {
            let mut platform = ();
            let (mut port, phy) = wifi.channel_hal_with_attachment(&mut platform, &mut self.domain);
            select_concurrent_wifi_channel::<RomShortDelay, _, _>(
                &EmbassyClock,
                phy,
                u16::from(channel),
                0,
                &mut port,
                &mut NoopPhyTargetObserver,
            )
            .await
            .map_err(|_| RejectReason::Internal)?;
        }
        wifi = observe(&self.domain, wifi, Point::AfterChannel)?;
        let tx = self
            .domain
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
                MacCoexPti::new(self.domain.coex_pti(event.coex_event()).value())
                    .expect("generated coexistence PTI fits")
            }),
        };
        wifi = observe(&self.domain, wifi, Point::BeforeMac)?;
        initialize_wifi_mac(
            &mut sources,
            &mut wifi.cold_mac_hal(&mut self.domain),
            &EmbassyClock,
            MacColdStartConfig {
                handshake_deadline: MAC_COLD_HANDSHAKE_DEADLINE,
                station_address: self.station,
                access_point_address: self.access_point,
            },
        )
        .await
        .map_err(|_| RejectReason::Internal)?;
        wifi = observe(&self.domain, wifi, Point::AfterMac)?;
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
            wifi = observe(&self.domain, wifi, Point::Settled(sample))?;
        }
        self.clocked = Some(wifi);
        Ok(())
    }

    async fn exit(&mut self) -> Result<()> {
        let wifi = self.clocked.take().ok_or(RejectReason::Internal)?;
        let member = self.member.take().ok_or(RejectReason::Internal)?;
        set_wifi_rx(&mut self.domain, &member, false);
        if !leave_wifi(&mut self.domain, &wifi, member).map_err(|_| RejectReason::Internal)? {
            return Err(RejectReason::Internal);
        }
        let powered = wifi
            .release_clocks(&mut self.domain)
            .map_err(|_| RejectReason::Internal)?
            .into_powered();
        let (absent, _) = self
            .domain
            .leave_coex(self.coex.take().ok_or(RejectReason::Internal)?);
        self.cold = Some(
            powered
                .power_down(
                    &mut self.domain,
                    self.power.take().ok_or(RejectReason::Internal)?,
                    &absent,
                )
                .map_err(|_| RejectReason::Internal)?,
        );
        self.absent = Some(absent);
        crate::sar2_checkpoint::observe(
            &mut self.domain,
            crate::sar2_checkpoint::Point::BeforeRfClose,
        )
        .await?;
        close_concurrent_rf::<_, (), RomShortDelay>(&EmbassyClock, &mut self.domain, &mut ())
            .await
            .map_err(|_| RejectReason::Internal)?;
        Ok(())
    }

    async fn read_analog(&mut self, address: PhyI2cAddress) -> Result<u8> {
        let mut registers = self.domain.phy_hal();
        let mut started = false;
        for _ in 0..HARDWARE_EDGE_LIMIT {
            if !started {
                started = i2c::try_start_read(&mut registers, address).is_ok();
            }
            if started {
                if let Ok(value) = i2c::try_finish_read(&registers, address) {
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
}

struct Sources<'a> {
    tx: PhyTxTargetPowerProfile,
    pti: [MacCoexPti; 4],
    rng: &'a mut RngOwner<'static>,
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
