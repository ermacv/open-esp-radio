//! Destructive tests of the SoC deadline service, without a radio owner.
use oer_esp32s31_soc_esp_hal::watchdog::DeadlineLease;
/// Deliberately leave the real SoC service armed; no radio implementation is
/// replaced. This verifies service cancellation, not an injected PHY failure.
pub(super) async fn inject(
    mode: oer_hil_protocol::system::WatchdogTestMode,
    lease: DeadlineLease<'static>,
) -> ! {
    use oer_hil_protocol::system::WatchdogTestMode as Mode;
    match mode {
        Mode::Complete => unreachable!("completed before acknowledgement"),
        Mode::BlockedPoll =>
        {
            #[allow(
                clippy::disallowed_methods,
                reason = "explicit diagnostic: watchdog must reset a non-returning synchronous poll"
            )]
            loop {
                core::hint::spin_loop();
            }
        }
        Mode::Cancelled => {
            let pending = async move {
                core::future::pending::<()>().await;
                lease.complete().expect("unreachable completion");
            };
            {
                let mut pending = core::pin::pin!(pending);
                core::future::poll_fn(|cx| {
                    assert!(core::future::Future::poll(pending.as_mut(), cx).is_pending());
                    core::task::Poll::Ready(())
                })
                .await;
            }
        }
        Mode::LostCompletion => {
            core::future::pending::<()>().await;
            lease.complete().expect("unreachable completion");
        }
        Mode::LateRestoration => {
            embassy_time::Timer::after(embassy_time::Duration::from_secs(3)).await;
            // Hardware reset must precede this; no software reset fallback.
            let _ = lease.complete();
        }
    }
    core::future::pending().await
}
