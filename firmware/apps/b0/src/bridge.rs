//! P2 retains the slot and physical worker; P1 receives only its frontend.
use super::*;
use core::{
    future::poll_fn,
    task::{Context, Poll},
};
use oer_esp32s31_bluetooth_driver::port::{self as port, Device, Fault, mailbox as rpc};
use oer_memory::{Doorbell, HandlerCell, Poster, RequestSlot, Responder};
use oer_radio_port as rp;

pub type Backend<Id> = port::Backend<Id, rpc::MailboxDevice<EmbassyClock>>;
oer_espressif_executor::declare_port_slot!(pub slot: super::Backend<Id>);
pub type Front = rp::SlotFrontend<'static, slot::Id, Backend<slot::Id>>;
type Ready = RequestSlot<Option<rp::Joined<slot::Id>>, Result<Front, Fault>>;
static READY_BELL: Doorbell = Doorbell::new();
static READY: Ready = Ready::new(&READY_BELL, 1);
pub type StopReports =
    oer_memory::AffineSpscReceiver<'static, [u8; 2], 1, oer_memory::CompactSpscCursor>;
pub type StopReporter =
    oer_memory::AffineSpscSender<'static, [u8; 2], 1, oer_memory::CompactSpscCursor>;
static STOP_REPORTS: StaticCell<
    oer_memory::AffineSpscQueue<[u8; 2], 1, oer_memory::CompactSpscCursor>,
> = StaticCell::new();

pub fn stop_reports() -> Option<(StopReporter, StopReports)> {
    Some(
        STOP_REPORTS
            .try_init(oer_memory::AffineSpscQueue::new())?
            .split(),
    )
}

pub struct FrontendChannel(
    pub Poster<Option<rp::Joined<slot::Id>>, Result<Front, Fault>>,
    pub StopReports,
);
pub fn frontend(reports: StopReports) -> FrontendChannel {
    let state = CLOCK_STORE.init(rpc::ClockState::new(
        CLOCKS.claim().expect("one P1 clock caller"),
        &irq::FAULT,
    ));
    let _installed = CLOCK
        .install(state)
        .ok()
        .expect("one P1 clock bookkeeping owner");
    FrontendChannel(READY.claim().expect("one HCI frontend"), reports)
}

static COMMAND_BELL: Doorbell = Doorbell::new();
pub static COMMANDS: rpc::Commands = rpc::Commands::new(&COMMAND_BELL, 1);
static CLOCK_BELL: Doorbell = Doorbell::new();
static CLOCKS: rpc::ClockRequests = rpc::ClockRequests::new(&CLOCK_BELL, 1);
static CLOCK: rpc::ClockStorage = rpc::ClockStorage::new();
static CLOCK_STORE: StaticCell<rpc::ClockState> = StaticCell::new();
pub type Response = Responder<'static, rpc::Command, rpc::Answer>;
pub async fn command() -> (rpc::Command, Response) {
    poll_fn(|cx| {
        let _ = COMMAND_BELL.poll_rung(cx);
        COMMANDS.take_request().map_or(Poll::Pending, Poll::Ready)
    })
    .await
}
pub fn answer(command: rpc::Command, response: Response, result: Result<rpc::Completion, Fault>) {
    response.answer(rpc::Answer {
        epoch: command.epoch,
        result,
    });
}
pub fn complete(
    pending: &mut Option<(rpc::Command, Response)>,
    result: Result<rpc::Completion, Fault>,
) {
    if let Some((command, response)) = pending.take() {
        answer(command, response, result);
    }
}

type ClockResponse = Responder<'static, rpc::ClockRequest, rpc::ClockAnswer>;
struct ClockReplies {
    pending: Option<(rpc::ClockRequest, ClockResponse)>,
}
static REPLIES: StaticCell<ClockReplies> = StaticCell::new();
static REPLY_OWNER: HandlerCell<ClockReplies> = HandlerCell::new();
#[derive(Clone, Copy)]
pub struct RuntimeClock {
    epoch: u64,
}
#[derive(Clone, Copy)]
pub struct ClockToken {
    request: rpc::ClockRequest,
}
impl RuntimeClock {
    pub fn new(epoch: u64) -> Self {
        Self { epoch }
    }
}
impl oer_esp32s31_bluetooth_driver::port::runner::ClockRequests for RuntimeClock {
    type Request = ClockToken;
    fn try_request(&self) -> Option<Self::Request> {
        REPLY_OWNER
            .with(|state| {
                let held = &mut state.pending;
                if held.is_none() {
                    *held = CLOCKS.take_request();
                }
                let request = held.as_ref()?.0;
                if request.epoch != self.epoch {
                    let (_, reply) = held.take().expect("retained stale clock request");
                    reply.answer(rpc::ClockAnswer {
                        request,
                        result: Err(Fault::Accounting),
                    });
                    irq::FAULT.store(true, core::sync::atomic::Ordering::Release);
                    return None;
                }
                Some(ClockToken { request })
            })
            .flatten()
    }
    fn poll_request(&self, cx: &mut Context<'_>) -> Poll<Self::Request> {
        let _ = CLOCK_BELL.poll_rung(cx);
        self.try_request().map_or(Poll::Pending, Poll::Ready)
    }
    fn answer(token: Self::Request, result: Result<oer_bluetooth_le_radio::LeInstant, Fault>) {
        let _ = REPLY_OWNER.with(|state| {
            let held = &mut state.pending;
            if held
                .as_ref()
                .is_none_or(|(request, _)| *request != token.request)
            {
                irq::FAULT.store(true, core::sync::atomic::Ordering::Release);
                return;
            }
            let (request, response) = held.take().expect("matching clock response duty");
            response.answer(rpc::ClockAnswer { request, result });
        });
    }
}
pub fn clock_state() -> RuntimeClock {
    let _installed = REPLY_OWNER
        .install(REPLIES.init(ClockReplies { pending: None }))
        .ok()
        .expect("one P2 clock responder owner");
    RuntimeClock { epoch: 0 }
}
pub fn fail_clock(cause: Fault) {
    let _ = REPLY_OWNER.with(|state| {
        if let Some((request, reply)) = state.pending.take() {
            reply.answer(rpc::ClockAnswer {
                request,
                result: Err(cause),
            });
        }
    });
}

pub async fn supervisor() {
    let mut first = Some(rpc::MailboxDevice::new(
        EmbassyClock,
        COMMANDS.claim().expect("one device caller"),
        &CLOCK,
        &irq::FAULT,
        oer_time::Duration::from_secs(2),
    ));
    let mut slot = oer_espressif_executor::port_slot::StaticPortSlot::new(
        slot::take_static().expect("one B0 port slot"),
    );
    loop {
        poll_fn(|cx| READY_BELL.poll_rung(cx)).await;
        let Some((joined, response)) = READY.take_request() else {
            continue;
        };
        let (frontend, registration) = match joined {
            None => {
                let Some(device) = first.take() else {
                    oer_firmware_apps_b0::handoff::park_failure((), || {
                        response.answer(Err(Fault::Accounting))
                    })
                    .await
                };
                match slot.register(Backend::new(device)) {
                    Ok(registered) => registered,
                    Err(owner) => {
                        oer_firmware_apps_b0::handoff::park_failure(owner, || {
                            response.answer(Err(Fault::Accounting))
                        })
                        .await
                    }
                }
            }
            Some(joined) => match slot.reinstall(joined, |vacant| {
                let device = match vacant.into_device().reinitialize() {
                    Ok(device) => device,
                    Err(mut device) => {
                        device.quarantine();
                        device
                    }
                };
                Backend::new(device)
            }) {
                Ok(registered) => registered,
                Err(resume) => {
                    oer_firmware_apps_b0::handoff::park_failure(resume, || {
                        response.answer(Err(Fault::Accounting))
                    })
                    .await
                }
            },
        };
        response.answer(Ok(frontend));
        if let Some(completion) = registration.run().await {
            completion.unregistered();
        }
    }
}
