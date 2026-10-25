//! Transport and the radio task of the separate radio-health image.
use crate::*;

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
        ieee802154::poll();
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
