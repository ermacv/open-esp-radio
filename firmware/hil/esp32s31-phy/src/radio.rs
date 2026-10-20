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
    domain: RadioDomain<ConcurrentPhy>,
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
}

impl Radio {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        domain: RadioDomain<ConcurrentPhy>,
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
        }
    }

    pub async fn request(&mut self, request: Request) -> Result<Answer> {
        if self.poisoned {
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
        match request {
            Request::Prepare(config) => {
                if !config.valid() {
                    return Err(RejectReason::InvalidConfiguration);
                }
                if self.registered {
                    return Err(RejectReason::InvalidState);
                }
                let registration = register_concurrent_phy::<(), RomShortDelay, _>(
                    &EmbassyClock,
                    &mut self.domain,
                    &mut (),
                    &self.clocks,
                    PhyRegisterConfig::new(self.identity),
                    NoopPhyTargetObserver,
                )
                .await
                .map_err(|_| RejectReason::Internal)?;
                let outcome = registration.outcome();
                if !outcome.full_calibration_performed
                    || outcome.calibration_path != PhyCalibrationPath::FullForCache
                {
                    return Err(RejectReason::Internal);
                }
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
                wake_concurrent_rf::<RomShortDelay>(&EmbassyClock, &mut self.domain, &self.clocks)
                    .await
                    .map_err(|_| RejectReason::Internal)?;
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
            maintain_concurrent_phy::<(), RomShortDelay, _>(
                &EmbassyClock,
                &mut self.domain,
                &mut (),
                NoopPhyTargetObserver,
            )
            .await
            .map_err(|_| RejectReason::Internal)?;
        }
        set_wifi_rx(
            &mut self.domain,
            self.member.as_ref().ok_or(RejectReason::Internal)?,
            true,
        );
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
        close_concurrent_rf::<(), RomShortDelay>(&EmbassyClock, &mut self.domain, &mut ())
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
