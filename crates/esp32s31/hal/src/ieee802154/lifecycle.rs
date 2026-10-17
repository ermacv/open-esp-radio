//! Finite ESP32-S31 IEEE 802.15.4 reset and foundation sequence.
//!
//! The sequence starts after the shared modem clock planner established the
//! IEEE 802.15.4 module clocks and stops before PHY/RF ownership, interrupt
//! routing, DMA buffers or an operational MAC state. The backend is built from
//! the IEEE 802.15.4 partition; the private MAC resets live in the shared
//! modem syscon block and are reached through a separate reset port borrowed
//! from the shared radio lease. Neither exposes raw register or address
//! operations.

#![forbid(unsafe_code)]
#![cfg_attr(not(test), allow(dead_code))]

use core::marker::PhantomData;

use oer_esp32s31_pac::{Ieee802154FoundationSnapshot, Ieee802154Pti};

/// PTI value used by the public vendor LL when coexistence is disabled.
pub(crate) const COEX_DISABLED_PTI: u8 = 3;

pub use oer_espressif_ieee802154_engine::channel::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
};

/// Private MAC reset lines in the shared modem syscon block.
///
/// An implementation borrows the shared radio registers for one reset
/// transition; it never retains them.
pub(crate) trait Ieee802154ResetPort {
    fn set_ieee802154_mac_reset(&mut self, asserted: bool);
    fn set_ieee802154_apb_reset(&mut self, asserted: bool);
    fn ieee802154_reset_readback(&self) -> Ieee802154ResetReadback;
}

/// Closed semantic MAC backend for the finite foundation sequence.
///
/// An implementation must retain exclusive access to the IEEE 802.15.4
/// partition for the lifetime of the returned typestate value. In
/// particular, implementations must not reconstruct the IEEE 802.15.4 block
/// from an address or independently claim a second peripheral singleton.
pub(crate) trait Ieee802154LifecycleBackend {
    /// Prevent every peripheral event from reaching the future MAC IRQ route.
    fn mask_all_events(&mut self);

    /// Keep every RX-abort source masked until the receive dataplane exists.
    fn mask_all_rx_aborts(&mut self);

    /// Keep every TX-abort source masked until the transmit dataplane exists.
    fn mask_all_tx_aborts(&mut self);

    fn select_average_ed_sampling(&mut self);
    fn set_txrx_pti(&mut self, pti: Ieee802154Pti);
    fn set_ack_pti(&mut self, pti: Ieee802154Pti);

    /// Apply the vendor MAC-initialization receive-on delay.
    fn apply_txon_delay(&mut self);

    /// Order completed foundation writes before publishing the next typestate.
    fn order_device_accesses(&mut self);

    /// Sample the safe, non-operational foundation image.
    fn foundation_snapshot(&mut self) -> Ieee802154FoundationSnapshot;
}

/// Semantic readback after the two private reset pulses.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154ResetReadback {
    pub mac_reset_released: bool,
    pub apb_reset_released: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154ResetCheckpoint {
    MacResetReleased,
    ApbResetReleased,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ieee802154FoundationCheckpoint {
    EventsMasked,
    RxAbortsMasked,
    TxAbortsMasked,
    EdSampleAverage,
    TxrxPtiDisabled,
    AckPtiDisabled,
    TxOnDelayApplied,
}

/// One failed semantic readback. Register images never escape the backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154ReadbackError<Checkpoint> {
    pub checkpoint: Checkpoint,
    pub expected: bool,
    pub observed: bool,
}

pub(crate) mod state {
    /// The shared modem clock planner holds every IEEE 802.15.4 module clock.
    #[derive(Debug)]
    pub(crate) struct Clocked;

    /// Both private MAC reset lines have been pulsed and released.
    #[derive(Debug)]
    pub(crate) struct Reset;

    /// Static MAC foundation is configured with every event still masked.
    #[derive(Debug)]
    pub(crate) struct FoundationConfigured;
}

/// Exclusive whole-radio backend at one proved IEEE 802.15.4 phase.
#[derive(Debug)]
pub(crate) struct Ieee802154Lifecycle<Backend, State> {
    backend: Backend,
    _state: PhantomData<State>,
}

impl<Backend, State> Ieee802154Lifecycle<Backend, State> {
    pub(crate) const fn backend(&self) -> &Backend {
        &self.backend
    }

    /// Mutably borrow the retained whole-owner backend inside a later closed
    /// semantic transition. This does not expose it outside the crate.
    pub(crate) fn backend_mut(&mut self) -> &mut Backend {
        &mut self.backend
    }

    pub(crate) fn into_backend(self) -> Backend {
        self.backend
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Clocked> {
    /// Enter the clocked phase once the shared modem clock planner committed
    /// the IEEE 802.15.4 module clocks for this backend.
    pub(crate) const fn clocked(backend: Backend) -> Self {
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
    pub(crate) fn into_clocked(self) -> Ieee802154Lifecycle<Backend, state::Clocked> {
        Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        }
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::FoundationConfigured> {
    /// Re-enter the foundation phase with an owner returning from an
    /// operational epoch, after the complete foundation read back again.
    ///
    /// The operational epoch rewrote the MAC PIB, so only the foundation is
    /// an invariant of the return: masks cleared by interrupt teardown,
    /// average ED sampling, coexistence-disabled PTIs and the receive-on
    /// delay. A mismatch disproves the foundation and keeps the reset proof.
    pub(crate) fn resume(mut backend: Backend) -> Result<Self, Ieee802154FoundationFailure<Backend>>
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
    pub(crate) fn forget_foundation(self) -> Ieee802154Lifecycle<Backend, state::Reset> {
        Ieee802154Lifecycle {
            backend: self.backend,
            _state: PhantomData,
        }
    }
}

/// Failed reset transition which remains at the last proved `Clocked` phase.
#[derive(Debug)]
pub(crate) struct Ieee802154ResetFailure<Backend> {
    lifecycle: Ieee802154Lifecycle<Backend, state::Clocked>,
    error: Ieee802154ReadbackError<Ieee802154ResetCheckpoint>,
}

impl<Backend> Ieee802154ResetFailure<Backend> {
    pub(crate) const fn error(&self) -> Ieee802154ReadbackError<Ieee802154ResetCheckpoint> {
        self.error
    }

    pub(crate) fn into_lifecycle(self) -> Ieee802154Lifecycle<Backend, state::Clocked> {
        self.lifecycle
    }
}

/// Failed foundation transition which remains at the last proved `Reset` phase.
#[derive(Debug)]
pub(crate) struct Ieee802154FoundationFailure<Backend> {
    lifecycle: Ieee802154Lifecycle<Backend, state::Reset>,
    error: Ieee802154ReadbackError<Ieee802154FoundationCheckpoint>,
}

impl<Backend> Ieee802154FoundationFailure<Backend> {
    pub(crate) const fn error(&self) -> Ieee802154ReadbackError<Ieee802154FoundationCheckpoint> {
        self.error
    }

    pub(crate) fn into_lifecycle(self) -> Ieee802154Lifecycle<Backend, state::Reset> {
        self.lifecycle
    }
}

impl<Backend> Ieee802154Lifecycle<Backend, state::Clocked> {
    /// Pulse ZBMAC first and ZBMAC APB second, preserving all unrelated resets.
    ///
    /// This is the IEEE 802.15.4 case of the vendor
    /// `modem_clock_module_mac_reset`, which `ieee802154_mac_init` runs first.
    pub(crate) fn reset_mac(
        self,
        port: &mut impl Ieee802154ResetPort,
    ) -> Result<Ieee802154Lifecycle<Backend, state::Reset>, Ieee802154ResetFailure<Backend>> {
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
    pub(crate) fn configure_foundation(
        mut self,
    ) -> Result<
        Ieee802154Lifecycle<Backend, state::FoundationConfigured>,
        Ieee802154FoundationFailure<Backend>,
    > {
        self.backend.mask_all_events();
        self.backend.mask_all_rx_aborts();
        self.backend.mask_all_tx_aborts();
        self.backend.select_average_ed_sampling();
        let disabled_pti = Ieee802154Pti::new(COEX_DISABLED_PTI)
            .expect("reviewed coexistence-disabled PTI fits five bits");
        self.backend.set_txrx_pti(disabled_pti);
        self.backend.set_ack_pti(disabled_pti);
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
        snapshot.txrx_pti().value() == COEX_DISABLED_PTI,
    )?;
    verify(
        Ieee802154FoundationCheckpoint::AckPtiDisabled,
        snapshot.ack_pti().value() == COEX_DISABLED_PTI,
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
mod tests;
