//! Boot and transport of the separate radio-health image.
use crate::*;

pub(crate) fn main(handoff: oer_espressif_staged_runtime::Handoff) -> ! {
    oer_esp32s31_hal::usb_serial_jtag::early_note(&oer_diagnostics::Note::diagnostic_start("phy"));
    let peripherals = esp_hal::init(esp_hal::Config::default());
    let (psram, adopted) = handoff.adopt(
        INTERRUPT_TABLE,
        oer_esp32s31_board::BootstrapPsram(peripherals.PSRAM),
    );
    let interrupts = Interrupts::take().expect("diagnostic interrupt table claimed once");
    let factory = oer_esp32s31_hal::platform::FactoryIdentity::read(
        oer_espressif_esp_hal_soc::handoff::take_efuse()
            .expect("the image takes the eFuse words once"),
    );
    let identity = PhyCalibrationIdentity {
        rf_cal_version: phy_get_rf_cal_version(),
        base_mac_address: factory.base_mac(),
        mac_extension: factory.mac_extension(),
    };
    let station = factory.address(oer_esp32s31_hal::platform::Interface::Station);
    let access_point = factory.address(oer_esp32s31_hal::platform::Interface::AccessPoint);
    let (hardware, clocks) = take_radio(
        RadioSingletons {
            i2c_ana_mst: peripherals.I2C_ANA_MST,
            modem_syscon: peripherals.MODEM_SYSCON,
            modem_lpcon: peripherals.MODEM_LPCON,
            wifi: peripherals.WIFI,
            bt: peripherals.BT,
            ieee802154: peripherals.IEEE802154,
            lp_tsens: peripherals.LP_TSENS,
        },
        &psram,
    );
    let (domain, partitions) =
        hardware.split::<oer_espressif_esp_hal_radio::EspHal, _>(ConcurrentPhy::new());
    let mut rng = Rng::new(
        oer_espressif_esp_hal_soc::handoff::take_entropy(peripherals.RNG, peripherals.LP_PERI),
        oer_embassy_time::EmbassyClock,
    );
    let boot = (u64::from(rng.random()) << 32 | u64::from(rng.random())).max(1);
    let radio = radio::Radio::place(
        RADIO.uninit(),
        domain,
        partitions.wifi,
        partitions.wifi_coex,
        ieee802154::Rx::new(
            partitions.ieee802154,
            partitions.ieee802154_coex,
            interrupts.mac,
        ),
        clocks,
        identity,
        station,
        access_point,
        rng,
    );
    radio.set_health_boot(boot);
    oer_espressif_executor::init::<2, _>(peripherals.SYSTIMER, interrupts.radio_alarm);
    oer_espressif_executor::bind::<0, _>(interrupts.app_alarm);
    let radio_spawner =
        InterruptExecutor::<2>::new(SoftwareInterrupt::new(peripherals.FROM_CPU_INTR2))
            .start(interrupts.radio, LEVELS);
    RadioTasks::serve_radio(&radio_spawner, radio).expect("radio diagnostic task");
    let poster = SLOT.claim().expect("one diagnostic requester");
    let chip = transport::Chip(oer_esp32s31_hal::platform::ResetCauses::read(
        oer_espressif_esp_hal_soc::handoff::take_reset_causes(peripherals.LP_AON_CLK_RST),
    ));
    let usb = transport::open_usb(
        oer_espressif_esp_hal_soc::handoff::take_usb_serial_jtag(peripherals.USB_DEVICE)
            .expect("no early record holds the USB Serial/JTAG at the console handoff"),
        interrupts.console_usb,
    );
    transport::CONSOLE.start(boot);
    adopted.enable_interrupts();
    Executor::<0>::new(SoftwareInterrupt::new(peripherals.FROM_CPU_INTR0)).run(
        interrupts.wake,
        move |spawner| {
            AppTasks::console(spawner, usb, boot, chip).expect("diagnostic console task");
            AppTasks::dispatch(spawner, poster).expect("diagnostic dispatch task");
        },
    );
}

pub(crate) async fn console(usb: transport::Usb, boot: u64, chip: transport::Chip) {
    let keys = oer_hil_protocol::feature_keys::image_keys(&|feature| {
        feature == "diagnostic-phy" || feature == "diagnostic-radio-health"
    });
    let intake = Intake::new(boot, ImageKeySet::new(&keys), 0);
    transport::CONSOLE
        .run(
            usb,
            intake,
            &chip,
            |id, request: oer_hil_agent::base::Either<WireRequest, health::Request>| {
                let request = match request {
                    oer_hil_agent::base::Either::First(request) => {
                        oer_hil_agent::base::Either::First(Request::Wire(request))
                    }
                    oer_hil_agent::base::Either::Second(request) => {
                        oer_hil_agent::base::Either::Second(request)
                    }
                };
                if REQUESTS.try_send((id, request)).is_err() {
                    transport::CONSOLE.publish(
                        id.session_id,
                        id.request_id,
                        &Rejected(RejectReason::Busy),
                    );
                }
            },
        )
        .await;
}

pub(crate) async fn dispatch(mut poster: Poster<Request, Answer>) {
    let mut health_poster = HEALTH_SLOT.claim().expect("one health requester");
    loop {
        let (id, request) = REQUESTS.receive().await;
        let request = match request {
            oer_hil_agent::base::Either::First(request) => request,
            oer_hil_agent::base::Either::Second(request) => {
                let mut pending = health_poster.post(request);
                let (returned, answer) = poll_fn(|cx| pending.poll_answer(cx)).await;
                health_poster = returned;
                answer.publish(id).await;
                continue;
            }
        };
        let mut pending = poster.post(request);
        let (returned, answer) = poll_fn(|cx| pending.poll_answer(cx)).await;
        poster = returned;
        macro_rules! send {
            ($value:expr) => {
                transport::CONSOLE
                    .publish_reliably(id.session_id, id.request_id, &$value)
                    .await
            };
        }
        let release = matches!(&answer, Answer::Started(_) | Answer::CacheCompleted(_));
        let sequence = match answer {
            Answer::CacheIdentity(value) => {
                send!(value)
            }
            Answer::Started(value) => {
                send!(value)
            }
            Answer::CacheWritten(value) => {
                send!(value)
            }
            Answer::CacheCompleted(value) => {
                send!(value)
            }
            Answer::CacheStatus(value) => {
                send!(value)
            }
            Answer::CacheSnapshot(value) => {
                send!(value)
            }
            Answer::Proceeded => continue,

            Answer::HoldStarted(value) => {
                send!(value)
            }
            Answer::HoldPage(value) => {
                send!(value)
            }
            Answer::HoldStopped(value) => {
                send!(value)
            }
            Answer::Restarted(value) => {
                send!(value)
            }
            Answer::Stopped(value) => {
                send!(value)
            }
            Answer::Calibration(value) => {
                send!(value)
            }
            Answer::Registers(value) => {
                send!(value)
            }
            Answer::Analog(value) => {
                send!(value)
            }
            Answer::Rejected(reason) => {
                send!(Rejected(reason))
            }
        };
        if release {
            transport::CONSOLE.written(sequence).await;
            let mut pending = poster.post(Request::Proceed);
            let (returned, _answer) = poll_fn(|cx| pending.poll_answer(cx)).await;
            poster = returned;
        }
    }
}

pub(crate) async fn serve_radio(radio: &'static mut radio::Radio) {
    // RX ring authority is core-local and is created only after the P2 spawn.
    let mut capture_rx = None;
    loop {
        radio.expire_cache();
        if radio.registration_pending() {
            let mut registration = core::pin::pin!(radio.service_registration());
            poll_fn(|cx| radio::poll_registration(registration.as_mut(), cx)).await;
            continue;
        }
        if let Some(deadline) = radio.deadline() {
            use core::future::Future;
            use oer_time::Timer;
            let mut timer = core::pin::pin!(oer_embassy_time::EmbassyClock.wait_until(deadline));
            poll_fn(|cx| {
                if DOORBELL.poll_rung(cx).is_ready() {
                    core::task::Poll::Ready(())
                } else {
                    timer.as_mut().poll(cx)
                }
            })
            .await;
        } else {
            poll_fn(|cx| DOORBELL.poll_rung(cx)).await;
        }
        // A started PHY future always reaches its terminal result before a command.
        {
            let mut hold = core::pin::pin!(radio.service_hold());
            hold.as_mut().await;
        }
        if let Some((request, response)) = SLOT.take_request() {
            radio.expire_cache();
            match request {
                Request::Proceed => {
                    radio.release_cache();
                    response.answer(Answer::Proceeded);
                }
                Request::Wire(request) => {
                    let result = radio.request(request, &mut capture_rx).await;
                    radio.request_finished(&result);
                    response.answer(result.unwrap_or_else(Answer::Rejected));
                }
            }
        }
        if let Some((request, response)) = HEALTH_SLOT.take_request() {
            let answer = radio
                .health_request(request, &mut capture_rx)
                .await
                .unwrap_or_else(health::Answer::Rejected);
            response.answer(answer);
        }
    }
}
