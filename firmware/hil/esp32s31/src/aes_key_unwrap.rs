//! Host-driven calls of the same composed engine used by the station.
use crate::{services::AesRequests, transport::CONSOLE};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use oer_crypto::{
    Aes128Key, KeyData, KeyUnwrap, KeyUnwrapError, KeyUnwrapRequest, block::BlockError,
};
use oer_esp32s31_compose::{Esp32s31, crypto::WifiKeyUnwrap};
use oer_hil_protocol::{
    RequestIdentity,
    base::{RejectReason, Rejected},
    system::{AesKeyUnwrap, AesKeyUnwrapped, AesUnwrapError},
};

static REQUESTS: Channel<CriticalSectionRawMutex, (RequestIdentity, AesKeyUnwrap), 1> =
    Channel::new();

oer_espressif_executor::tasks! {
    /// Separate from all radio tasks: only the console and the product engine.
    pub(crate) AesTasks: Executor<Esp32s31, 0> {
        console(usb: crate::transport::Usb, boot: u64) = console;
        unwrap(engine: WifiKeyUnwrap) = unwrap;
        watchdog(watchdog: oer_esp32s31_hal::peripherals::watchdog::ArmedRtcWatchdog<oer_esp32s31_hal::peripherals::watchdog::RwdtRegisters>) = crate::feed_rtc_watchdog;
    }
}

pub(crate) fn start(
    handoff: oer_espressif_staged_runtime::Handoff<oer_esp32s31_hal::platform::Port>,
) -> ! {
    let prepared = crate::initialize_runtime();
    let ((timer, software, watchdogs, entropy, aes), _psram, mut adopted) =
        crate::finish_startup(|| {
            prepared.adopt(
                handoff,
                crate::INTERRUPT_TABLE,
                oer_esp32s31_board::BootstrapPsram,
                oer_esp32s31_compose::phy_fault::report_previous,
                |runtime| {
                    crate::system::read_reset_causes(runtime.reset_cause);
                    crate::system::take_platform_panic();
                    crate::fatal::report_previous();
                    (
                        runtime.system_timer,
                        runtime.software_interrupts,
                        runtime.watchdogs,
                        runtime.entropy,
                        runtime.aes,
                    )
                },
            )
        })
        .into_parts();
    crate::exception::report_stack_guard();
    let Some(interrupts) = crate::Interrupts::take() else {
        crate::fail(crate::fatal::Failure::InterruptTable)
    };
    let _timer_ready = oer_espressif_executor::init::<Esp32s31, 0, _, _>(
        oer_espressif_executor::systimer::SystemTimer {
            comparators: timer.comparators,
            counter: timer.counter,
        },
        adopted.router(),
        interrupts.alarm,
        timer.alarm0,
    )
    .unwrap_or_else(|failure| failure.reset());
    let usb = crate::usb::open(adopted.router(), interrupts.console_usb);
    let silent = adopted
        .router()
        .silent::<oer_esp32s31_hal::peripherals::aes::AesInterrupt>()
        .unwrap_or_else(|_| crate::fail(crate::fatal::Failure::InterruptTable));
    let engine = oer_esp32s31_compose::crypto::wifi_key_unwrap(aes, silent);
    let mut entropy = oer_esp32s31_hal::peripherals::entropy::Entropy::new(
        entropy,
        oer_embassy_time::EmbassyClock,
    );
    let boot = u64::from_le_bytes(entropy.random_bytes()).max(1);
    CONSOLE.start(boot);
    let executor = crate::Executor::<Esp32s31, 0>::new(software.line0);
    let (mut router, hart) = adopted
        .enable_interrupts()
        .unwrap_or_else(|error| crate::fail(crate::fatal::Failure::from_boot(error)));
    match executor
        .run(
            &hart,
            &mut router,
            interrupts.wake,
            software.irq0,
            (),
            move |spawner| {
                if AesTasks::console(spawner, usb.0, boot).is_err()
                    || AesTasks::unwrap(spawner, engine).is_err()
                    || AesTasks::watchdog(spawner, watchdogs.rtc).is_err()
                {
                    crate::fail(crate::fatal::Failure::TaskAllocation);
                }
            },
        )
        .unwrap_or_else(|failure| failure.reset()) {}
}

async fn console(usb: crate::transport::Usb, boot: u64) {
    crate::transport::serve(
        usb,
        boot,
        crate::services::AES_KEYS,
        0,
        |identity, AesRequests::Unwrap(request)| {
            if REQUESTS.try_send((identity, request)).is_err() {
                CONSOLE.publish(
                    identity.session_id,
                    identity.request_id,
                    &Rejected(RejectReason::Busy),
                );
            }
        },
    )
    .await
}

fn error(error: KeyUnwrapError) -> AesUnwrapError {
    match error {
        KeyUnwrapError::InvalidLength => AesUnwrapError::InvalidLength,
        KeyUnwrapError::IntegrityCheckFailed => AesUnwrapError::IntegrityCheckFailed,
        KeyUnwrapError::Engine(error) => match error {
            BlockError::Unavailable => AesUnwrapError::Unavailable,
            BlockError::Timeout { observation } => AesUnwrapError::Timeout { observation },
            BlockError::InvalidState { observation } => {
                AesUnwrapError::InvalidState { observation }
            }
            BlockError::TimeOverflow => AesUnwrapError::TimeOverflow,
        },
    }
}

async fn unwrap(mut engine: WifiKeyUnwrap) {
    let (mut completed, mut succeeded, mut refused) = (0u32, 0u32, 0u32);
    loop {
        let (identity, request) = REQUESTS.receive().await;
        let result = match KeyData::<512>::try_copy(&request.wrapped) {
            Ok(wrapped) => {
                let done = engine
                    .unwrap_key(KeyUnwrapRequest::new(
                        Aes128Key::copy_from(&request.kek),
                        wrapped,
                    ))
                    .await;
                engine = done.engine;
                done.response
                    .plaintext()
                    .map_err(error)
                    .and_then(|bytes| bytes.try_into().map_err(|_| AesUnwrapError::InvalidLength))
            }
            Err(_) => Err(AesUnwrapError::InvalidLength),
        };
        completed = completed.saturating_add(1);
        if result.is_ok() {
            succeeded = succeeded.saturating_add(1);
        } else {
            refused = refused.saturating_add(1);
        }
        CONSOLE
            .publish_reliably(
                identity.session_id,
                identity.request_id,
                &AesKeyUnwrapped {
                    operation: request.operation,
                    completed,
                    succeeded,
                    refused,
                    result,
                },
            )
            .await;
    }
}
