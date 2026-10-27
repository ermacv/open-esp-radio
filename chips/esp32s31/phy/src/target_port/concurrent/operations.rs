//! The production lifecycle future constructors. Hardware execution lives
//! in the parent kernels; host tests bind these same entrypoints to virtual time.
use super::*;
use oer_esp32s31_hal::domain::transaction::Operation;

// The constructor writes its async block directly into the caller's future.
// A fixed poll boundary keeps registration temporaries out of enclosing radio
// polls without an allocation or another copy of the retained transition.
pin_project_lite::pin_project! {
    pub(crate) struct RegisterFuture<F> {
        #[pin]
        pub(crate) future: F,
    }
}
impl<F: core::future::Future> core::future::Future for RegisterFuture<F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(
        self: core::pin::Pin<&mut Self>,
        cx: &mut core::task::Context<'_>,
    ) -> core::task::Poll<Self::Output> {
        self.project().future.poll(cx)
    }
}

/// Register the shared PHY domain through an owned PHY transaction.
///
/// This is the calibrating first `esp_phy_enable`: `PHY` and
/// `PHY_CALIBRATION` are enabled through the radio domain before calibration, and
/// `PHY_CALIBRATION` is released after it. `PHY` stays held while RF is open.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
#[must_use = "PHY registration must be driven to a terminal result"]
#[allow(
    clippy::result_large_err,
    reason = "fail-stop error retains the allocation-free PHY transition"
)]
pub fn register_concurrent_phy<'a, B: PlatformClockBackend + 'a, P, D, O>(
    timer: &'a impl oer_cooperate::CooperativeTimer,
    radio: RadioDomain<B, ConcurrentPhy>,
    platform: &'a mut P,
    clocks: &'a impl PlatformClockProvider<Backend = B>,
    config: PhyRegisterConfig,
    observer: O,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<ConcurrentPhyRegistration, ConcurrentPhyRegisterFailure>,
    ),
> + 'a
where
    D: PhyShortDelay + 'a,
    O: PhyTargetObserver + 'a,
{
    let initial = radio.detach(Operation::Register);
    RegisterFuture {
        future: execute_register::<B, P, D, O>(timer, initial, platform, clocks, config, observer),
    }
}

/// Close RF after the last client left the shared domain, and release its
/// `PHY` modem clock module.
///
/// This is the last `esp_phy_disable`: the temperature preflight and
/// `phy_close_rf` run as on every route, the temperature sensor powers down
/// (`phy_xpd_tsens`), then the common PHY clock is released. The registered calibration epoch is retained for
/// [`wake_concurrent_rf`].
///
/// # Errors
///
/// Rejected before any register access when the domain is not RF-open,
/// clients are active or the owner describes another registration. A
/// preparation failure after completed transactions keeps RF open; an
/// ambiguous hardware failure poisons the domain.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn close_concurrent_rf<B: PlatformClockBackend, P, D>(
    timer: &impl oer_cooperate::CooperativeTimer,
    radio: RadioDomain<B, ConcurrentPhy>,
    platform: &mut P,
) -> impl core::future::Future<Output = (RadioDomain<B, ConcurrentPhy>, Result<(), ConcurrentRfError>)>
where
    D: PhyShortDelay,
{
    let initial = radio.detach(Operation::Close);
    execute_close::<B, P, D>(timer, initial, platform)
}

/// Wake RF of the closed shared domain for its next client.
///
/// This is a later first `esp_phy_enable`: `PHY` and `PHY_CALIBRATION` are
/// enabled, the retained registration is restored by `phy_wakeup_init`
/// without calibration, and `PHY_CALIBRATION` is released. No client is
/// acquired; the next `acquire_client`
/// evaluates tracking.
///
/// # Errors
///
/// Rejected before any register access when the domain is not RF-closed, the
/// owner describes another registration or the clocks could not be
/// enabled. A started wake that fails poisons the domain.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn wake_concurrent_rf<B: PlatformClockBackend, D>(
    timer: &impl oer_cooperate::CooperativeTimer,
    radio: RadioDomain<B, ConcurrentPhy>,
    clocks: &impl PlatformClockProvider<Backend = B>,
) -> impl core::future::Future<Output = (RadioDomain<B, ConcurrentPhy>, Result<(), ConcurrentRfError>)>
where
    D: PhyShortDelay,
{
    let initial = radio.detach(Operation::Wake);
    execute_wake::<B, D>(timer, initial, clocks)
}

/// Run the pending tracking request with the protocols running, as the
/// vendor's tracking does under its PHY lock: the owned PHY transaction serializes
/// the radio domain, and the
/// tracking graph brackets its RF-sensitive regions with the grant-protect
/// request.
///
/// A domain without pending tracking is rejected before any register
/// access.
///
/// # Cancellation
///
/// Cancellation, including before the first poll, quarantines the owned
/// transaction and records its fault before reset.
pub fn maintain_concurrent_phy<B: PlatformClockBackend, P, D, O>(
    timer: &impl oer_cooperate::CooperativeTimer,
    radio: RadioDomain<B, ConcurrentPhy>,
    platform: &mut P,
    observer: O,
    pilot: &mut impl crate::sar_pilot::Service,
) -> impl core::future::Future<
    Output = (
        RadioDomain<B, ConcurrentPhy>,
        Result<PhyParamTrackingOutcome, ConcurrentPhyTrackingError>,
    ),
>
where
    D: PhyShortDelay,
    O: PhyTargetObserver,
{
    let initial = radio.detach(Operation::Maintain);
    execute_maintain::<B, P, D, O>(timer, initial, platform, observer, pilot)
}
