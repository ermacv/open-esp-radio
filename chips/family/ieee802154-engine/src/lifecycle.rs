//! Finite family IEEE 802.15.4 reset and foundation sequence.
//!
//! The sequence starts after the shared modem clock planner established the
//! IEEE 802.15.4 module clocks. PHY/RF ownership stays with the caller; this
//! sequence establishes neither RF readiness, interrupt routing nor DMA storage. The backend is built from
//! the IEEE 802.15.4 partition; the private MAC resets live in the shared
//! modem syscon block and are reached through a separate reset port borrowed
//! from the shared radio lease. Neither exposes raw register or address
//! operations.

use core::marker::PhantomData;

use oer_espressif_ieee802154_mac_values::{
    COEX_DISABLED_PTI, Ieee802154FoundationSnapshot, Ieee802154LifecycleBackend,
    Ieee802154ResetPort, Ieee802154ResetReadback,
};

/// Private reset readback checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154ResetCheckpoint {
    /// The functional MAC reset reads released.
    MacResetReleased,
    /// The APB reset reads released.
    ApbResetReleased,
}

/// Foundation readback checkpoint.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154FoundationCheckpoint {
    /// Every event enable reads masked.
    EventsMasked,
    /// Every RX-abort enable reads masked.
    RxAbortsMasked,
    /// Every TX-abort enable reads masked.
    TxAbortsMasked,
    /// Energy detection reads as average sampling.
    EdSampleAverage,
    /// TX/RX coexistence priority reads disabled.
    TxrxPtiDisabled,
    /// ACK coexistence priority reads disabled.
    AckPtiDisabled,
    /// The sourced MAC initialization delays read back.
    TxOnDelayApplied,
}

/// One failed semantic readback. Register images never escape the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154ReadbackError<Checkpoint> {
    /// First mismatched checkpoint.
    pub checkpoint: Checkpoint,
    /// Expected semantic predicate.
    pub expected: bool,
    /// Observed semantic predicate.
    pub observed: bool,
}

/// Driver validation phases, independent of register ownership.
pub mod state {
    /// The shared modem clock planner holds every IEEE 802.15.4 module clock.
    #[derive(Debug)]
    pub struct Clocked;

    /// Both private MAC reset lines have been pulsed and released.
    #[derive(Debug)]
    pub struct Reset;

    /// Static MAC foundation is configured with every event still masked.
    #[derive(Debug)]
    pub struct FoundationConfigured;
}

/// Exclusive whole-radio backend at one proved IEEE 802.15.4 phase.
#[derive(Debug)]
pub struct Ieee802154Lifecycle<Backend, State> {
    backend: Backend,
    _state: PhantomData<State>,
}

impl<Backend, State> Ieee802154Lifecycle<Backend, State> {
    /// Consume the phase and return the retained hardware owner.
    pub fn into_backend(self) -> Backend {
        self.backend
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Clocked> {
    /// Enter the clocked phase once the shared modem clock planner committed
    /// the IEEE 802.15.4 module clocks for this backend.
    pub const fn clocked(backend: Backend) -> Self {
        Self {
            backend,
            _state: PhantomData,
        }
    }
}

impl<Backend, State> Ieee802154Lifecycle<Backend, State> {
    /// Retreat to the clocked phase before the module clocks are released.
    ///
    /// This performs no MMIO: the MAC keeps whatever reset and foundation
    /// state it had, which no later phase relies on without proving it again.
    pub fn into_clocked(self) -> Ieee802154Lifecycle<Backend, state::Clocked> {
        Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        }
    }
}

/// Affine evidence of the foundation that handed its backend to an operational epoch.
/// It permits only revalidation on return; it proves no producer drain or RF readiness.
#[must_use = "retain the return phase until the operational owners reunite"]
pub struct Ieee802154FoundationReturn<Backend> {
    _backend: PhantomData<Backend>,
}

impl<Backend> Ieee802154Lifecycle<Backend, state::FoundationConfigured> {
    /// Transfer the backend while retaining its prior reset/foundation phase for return.
    pub fn into_operational(self) -> (Backend, Ieee802154FoundationReturn<Backend>) {
        (
            self.backend,
            Ieee802154FoundationReturn {
                _backend: PhantomData,
            },
        )
    }

    /// Re-enter the foundation phase with an owner returning from an
    /// operational epoch, after the complete foundation read back again.
    ///
    /// The operational epoch rewrote the MAC PIB, so only the foundation is
    /// an invariant of the return: masks cleared by interrupt teardown,
    /// average ED sampling, coexistence-disabled PTIs and the receive-on
    /// delay. A mismatch disproves the foundation and keeps the reset proof.
    pub fn resume(
        mut backend: Backend,
        _return_phase: Ieee802154FoundationReturn<Backend>,
    ) -> Result<Self, Ieee802154FoundationFailure<Backend>>
    where
        Backend: Ieee802154LifecycleBackend,
    {
        match verify_foundation_snapshot(backend.foundation_snapshot()) {
            Ok(()) => Ok(Self {
                backend,
                _state: PhantomData,
            }),
            Err(error) => Err(Ieee802154FoundationFailure {
                lifecycle: Ieee802154Lifecycle {
                    backend,
                    _state: PhantomData,
                },
                error,
            }),
        }
    }

    /// Forget a disproved foundation while retaining the last independent
    /// reset proof and the exact whole-radio backend.
    pub fn forget_foundation(self) -> Ieee802154Lifecycle<Backend, state::Reset> {
        Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        }
    }
}

/// Failed reset transition which remains at the last proved `Clocked` phase.
#[derive(Debug)]
pub struct Ieee802154ResetFailure<Backend> {
    lifecycle: Ieee802154Lifecycle<Backend, state::Clocked>,
    error: Ieee802154ReadbackError<Ieee802154ResetCheckpoint>,
}

impl<Backend> Ieee802154ResetFailure<Backend> {
    /// Return the first failed semantic checkpoint.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.error
    }

    /// Recover the last proved phase with the same hardware owner.
    pub fn into_lifecycle(self) -> Ieee802154Lifecycle<Backend, state::Clocked> {
        self.lifecycle
    }
}

/// Failed foundation transition which remains at the last proved `Reset` phase.
#[derive(Debug)]
pub struct Ieee802154FoundationFailure<Backend> {
    lifecycle: Ieee802154Lifecycle<Backend, state::Reset>,
    error: Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>,
}

impl<Backend> Ieee802154FoundationFailure<Backend> {
    /// Return the first failed semantic checkpoint.
    pub const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.error
    }

    /// Recover the last proved phase with the same hardware owner.
    pub fn into_lifecycle(self) -> Ieee802154Lifecycle<Backend, state::Reset> {
        self.lifecycle
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Clocked> {
    /// Pulse ZBMAC first and ZBMAC APB second, preserving all unrelated resets.
    ///
    /// This is the IEEE 802.15.4 case of the vendor
    /// `modem_clock_module_mac_reset`, which `ieee802154_mac_init` runs first.
    pub fn reset_mac<'a>(
        self,
        port: &mut Backend::ResetPort<'a>,
    ) -> Result<Ieee802154Lifecycle<Backend, state::Reset>, Ieee802154ResetFailure<Backend>>
    where
        Backend: Ieee802154LifecycleBackend + 'a,
    {
        port.set_ieee802154_mac_reset(true);
        port.set_ieee802154_mac_reset(false);
        port.set_ieee802154_apb_reset(true);
        port.set_ieee802154_apb_reset(false);

        if let Err(error) = verify_reset_readback(port.ieee802154_reset_readback()) {
            return Err(Ieee802154ResetFailure {
                lifecycle: self,
                error,
            });
        }

        Ok(Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        })
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Reset>
where
    Backend: Ieee802154LifecycleBackend,
{
    /// Configure only the static, non-operational MAC foundation.
    ///
    /// Event and abort delivery remains masked throughout. `EVENT_STATUS` is
    /// deliberately untouched because foundation setup owns no pending-event
    /// acknowledgement; W1C acknowledgement belongs to the later polled or
    /// hard-IRQ owner. This transition does not claim PHY, RF, IRQ routing,
    /// buffers or an idle/ready hardware state.
    pub fn configure_foundation(
        mut self,
    ) -> Result<
        Ieee802154Lifecycle<Backend, state::FoundationConfigured>,
        Ieee802154FoundationFailure<Backend>,
    > {
        self.backend.mask_all_events();
        self.backend.mask_all_rx_aborts();
        self.backend.mask_all_tx_aborts();
        self.backend.select_average_ed_sampling();
        self.backend.disable_txrx_coex();
        self.backend.disable_ack_coex();
        self.backend.apply_txon_delay();
        self.backend.order_device_accesses();

        if let Err(error) = verify_foundation_snapshot(self.backend.foundation_snapshot()) {
            return Err(Ieee802154FoundationFailure {
                lifecycle: self,
                error,
            });
        }

        Ok(Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        })
    }
}

fn verify_reset_readback(
    readback: Ieee802154ResetReadback,
) -> Result<(), Ieee802154ReadbackError<Ieee802154ResetCheckpoint>> {
    verify(
        Ieee802154ResetCheckpoint::MacResetReleased,
        readback.mac_reset_released,
    )?;
    verify(
        Ieee802154ResetCheckpoint::ApbResetReleased,
        readback.apb_reset_released,
    )
}

fn verify_foundation_snapshot(
    snapshot: Ieee802154FoundationSnapshot,
) -> Result<(), Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>> {
    verify(
        Ieee802154FoundationCheckpoint::EventsMasked,
        snapshot.events_masked(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::RxAbortsMasked,
        snapshot.rx_aborts_masked(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::TxAbortsMasked,
        snapshot.tx_aborts_masked(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::EdSampleAverage,
        snapshot.ed_uses_average(),
    )?;
    verify(
        Ieee802154FoundationCheckpoint::TxrxPtiDisabled,
        snapshot.txrx_pti() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154FoundationCheckpoint::AckPtiDisabled,
        snapshot.ack_pti() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154FoundationCheckpoint::TxOnDelayApplied,
        snapshot.txon_delay_applied(),
    )
}

fn verify<Checkpoint: Copy>(
    checkpoint: Checkpoint,
    observed: bool,
) -> Result<(), Ieee802154ReadbackError<Checkpoint>> {
    if observed {
        Ok(())
    } else {
        Err(Ieee802154ReadbackError {
            checkpoint,
            expected: true,
            observed,
        })
    }
}

#[cfg(test)]
#[allow(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros
)]
mod tests;

impl<B: oer_espressif_ieee802154_mac_values::Ieee802154MacPolicyWrites>
    oer_espressif_ieee802154_mac_values::Ieee802154MacPolicyWrites
    for Ieee802154Lifecycle<B, state::FoundationConfigured>
{
    fn set_channel(&mut self, v: oer_espressif_ieee802154_mac_values::Ieee802154Channel) {
        self.backend.set_channel(v)
    }
    fn set_cca_mode(&mut self, v: oer_espressif_ieee802154_mac_values::Ieee802154CcaMode) {
        self.backend.set_cca_mode(v)
    }
    fn set_cca_threshold_code(&mut self, v: i8) {
        self.backend.set_cca_threshold_code(v)
    }
    fn set_mac_control(&mut self, v: oer_espressif_ieee802154_mac_values::Ieee802154MacControl) {
        self.backend.set_mac_control(v)
    }
    fn set_ack_timeout(&mut self, v: oer_espressif_ieee802154_mac_values::Ieee802154AckTimeout) {
        self.backend.set_ack_timeout(v)
    }
    fn set_primary_pan_identity(
        &mut self,
        v: oer_espressif_ieee802154_mac_values::Ieee802154PanIdentity,
    ) {
        self.backend.set_primary_pan_identity(v)
    }
    fn order_device_accesses(&mut self) {
        self.backend.order_device_accesses()
    }
}
impl<B: oer_espressif_ieee802154_mac_values::Ieee802154MacPolicyBackend>
    oer_espressif_ieee802154_mac_values::Ieee802154MacPolicyBackend
    for Ieee802154Lifecycle<B, state::FoundationConfigured>
{
    fn mac_policy_readback(
        &mut self,
    ) -> oer_espressif_ieee802154_mac_values::Ieee802154MacPolicyReadback {
        self.backend.mac_policy_readback()
    }
}
