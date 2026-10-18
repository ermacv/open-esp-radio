#![expect(
    clippy::result_large_err,
    reason = "TX epoch construction returns the exact DMA owner when preparation fails"
)]

//! Embassy control-TX construction for the executor-independent STA TX epoch.

use core::pin::Pin;

use oer_esp32s31_ieee80211::tx::{
    ControlTxConfig, WifiTxEntropy, WifiTxPowerProfile, WifiTxResources,
};

use oer_esp32s31_ieee80211_mac::tx::{
    TxSlot, protection::RtsLengthThreshold, runtime::WifiTxRuntimePolicy,
};

use oer_esp32s31_ieee80211_sta::{
    control_tx::ControlTransmitter,
    tx_epoch::{StaTxEpoch, StaTxEpochError},
};

/// Runtime construction methods for a STA epoch whose control owner is the
/// Embassy-composed ordinary transmitter.
pub trait StaTxEpochExt<'slot, P, E, T, const BUFFER_SIZE: usize>: Sized {
    fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ControlTxConfig,
    ) -> Self;

    /// Build the radio epoch's TX storage with the local dot11RTSThreshold;
    /// every BSS then installs its own protection facts.
    fn from_slot(
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        power: P,
        entropy: E,
        timer: T,
        config: ControlTxConfig,
        rts_length_threshold: Option<RtsLengthThreshold>,
    ) -> Self;

    fn restore_resources(
        &mut self,
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
    ) -> Result<
        (),
        (
            StaTxEpochError,
            ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>,
        ),
    >;
}

impl<'slot, P, E, T, const BUFFER_SIZE: usize> StaTxEpochExt<'slot, P, E, T, BUFFER_SIZE>
    for StaTxEpoch<ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    fn new(
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
        config: ControlTxConfig,
    ) -> Self {
        Self::from_control(ControlTransmitter::new(resources, config), config)
    }

    fn from_slot(
        slot: Pin<&'slot mut TxSlot<BUFFER_SIZE>>,
        power: P,
        entropy: E,
        timer: T,
        config: ControlTxConfig,
        rts_length_threshold: Option<RtsLengthThreshold>,
    ) -> Self {
        let mut policy = WifiTxRuntimePolicy::vendor_defaults();
        policy.set_rts_length_threshold(rts_length_threshold);
        Self::new(
            WifiTxResources {
                slot,
                policy,
                power,
                entropy,
                timer,
            },
            config,
        )
    }

    fn restore_resources(
        &mut self,
        resources: WifiTxResources<'slot, P, E, T, BUFFER_SIZE>,
    ) -> Result<
        (),
        (
            StaTxEpochError,
            ControlTransmitter<'slot, P, E, T, BUFFER_SIZE>,
        ),
    > {
        // A finished role leaves its BSS: the next role starts from the local
        // length threshold alone and installs its own BSS facts.
        let mut resources = resources;
        resources.policy.clear_bss_protection();
        self.restore_control(ControlTransmitter::new(resources, self.config()))
    }
}
