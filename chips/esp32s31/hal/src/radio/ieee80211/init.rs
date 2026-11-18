//! Finite cold preparation of the ESP32-S31 Wi-Fi MAC.
//!
//! The register sequence is recovered from `libpp.a[hal_mac.o]::hal_init`,
//! `mac_txrx_init`, `mac_rxbuf_init`, and `mac_last_rxbuf_init`. It deliberately
//! stops before publishing an RX descriptor base: ownership of the DMA ring
//! remains with the DMA storage, whose prepared/core-ready DMA proofs publish
//! the cold ring. The MAC core stays gated until runtime receives that
//! prepared memory/binding proof; RX stays stopped until the walker is live.

mod cold;
mod interface_address;
mod low_rate;
pub use low_rate::MacLowRateHardware;

use crate::types::MacDelaySlot;
use oer_portable_time::{Duration, Timer};

pub use self::{
    cold::{
        antenna::MacColdAntennaHardware,
        coex::{MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacColdCoexHardware, MacColdCoexPti},
        crypto::MacColdCryptoHardware,
        hal_tail::{MacColdHalTailHardware, MacSlowClockCalibration},
        handshake::{
            MAC_COLD_HANDSHAKE_DEADLINE, MAC_COLD_HANDSHAKE_RESAMPLE, MacColdHandshakeHardware,
            MacColdStartError, MacColdStartOutcome, run_cold_handshake,
        },
        he::{
            MacColdHeHardware, MacTxPowerSource, query_tx_power_table,
            run_tx_power_diagnostic_queries,
        },
        last_rx_buffer::MacColdLastRxBufferHardware,
        prepare::MacColdPrepareHardware,
        rx_buffer::MacColdRxBufferHardware,
        rx_policy::MacColdRxPolicyHardware,
        txrx::MacColdTxRxHardware,
    },
    interface_address::MacInterfaceAddressHardware,
};

use self::interface_address::program_cold_receive_addresses;

pub use crate::types::{MacInterruptMask, MacTxPowerPair, MacTxPowerTable};

/// Station primary event mask, with TX queue collision disabled.
///
/// The radio interrupt handler publishes it when it activates an epoch; the
/// bit policy itself remains part of the MAC lifecycle. Queue collisions are
/// sampled from TX_COMMON. The separate INT1 HE color source is enabled by HE
/// initialization and is not controlled by this mask.
pub const MAC_STATION_INTERRUPT_MASK: MacInterruptMask = MacInterruptMask::STATION;

/// The proof that the Wi-Fi MAC's block was reset, with the Wi-Fi module
/// clocks on, and the foreign words inside it written back: what the cold
/// preparation takes. Only [`RadioDomain::reset_wifi_mac_block`] makes one;
/// it is used once.
///
/// [`RadioDomain::reset_wifi_mac_block`]: crate::radio::domain::RadioDomain::reset_wifi_mac_block
///
/// ```compile_fail
/// use oer_chips_esp32s31_hal::radio::ieee80211::init::MacBlockReset;
/// fn requires_copy<T: Copy>() {}
/// requires_copy::<MacBlockReset>();
/// ```
///
/// ```compile_fail
/// use oer_chips_esp32s31_hal::radio::ieee80211::init::MacBlockReset;
/// fn requires_clone<T: Clone>() {}
/// requires_clone::<MacBlockReset>();
/// ```
///
/// ```compile_fail
/// use oer_chips_esp32s31_hal::radio::ieee80211::init::MacBlockReset;
/// let _made = MacBlockReset { _private: () };
/// ```
///
/// One reset serves one preparation: the proof is moved into it.
///
/// ```compile_fail,E0382
/// use oer_chips_esp32s31_hal::radio::ieee80211::init::MacBlockReset;
/// fn twice(reset: MacBlockReset) {
///     let _first = reset;
///     let _second = reset;
/// }
/// ```
#[must_use = "a reset's proof is taken by the cold preparation"]
pub struct MacBlockReset {
    _private: (),
}

impl MacBlockReset {
    /// The proof of the pulse just made; only the reset operation calls it.
    pub(crate) const fn after_pulse() -> Self {
        Self { _private: () }
    }

    /// A proof for a host model of the preparation, which has no block.
    #[cfg(any(test, feature = "test-support"))]
    pub const fn for_host_model() -> Self {
        Self::after_pulse()
    }
}

/// Platform entropy used by the on-chip branch of `hal_he_set_mac_delay`.
///
/// The vendor OS adapter obtains this from its `_random` callback. Keeping it
/// as a narrow trait prevents the MAC crate from borrowing a chip RNG
/// peripheral or depending on the vendor C ABI.
pub trait MacDelayEntropy {
    fn mac_delay_random(&mut self) -> u32;
}

/// Platform slow-clock calibration used by `hal_timer_update_by_rtc`.
///
/// This is the Rust ownership boundary corresponding to the vendor
/// `_slowclk_cal_get` callback. It keeps the open MAC independent of the
/// vendor function table and of any future platform clock peripheral.
pub trait MacSlowClockCalibrationSource {
    fn mac_slow_clock_calibration(&mut self) -> MacSlowClockCalibration;
}

/// Inputs for the role-neutral Wi-Fi MAC cold transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacColdStartConfig {
    /// How long the ready edge may stay unset, counted from the request.
    pub handshake_deadline: Duration,
    pub station_address: [u8; 6],
    pub access_point_address: [u8; 6],
}

/// Establish the role-neutral reset-state MAC register configuration.
///
/// The modem clock fields come from the pinned S31 register descriptions and
/// reproduce `esp-radio`'s S31 `enable_wifi(true)` operation. The remaining
/// ordered MAC writes come from the complete pinned `libpp.a` bodies named in
/// this module's documentation.
///
/// It runs after the radio system's reset of the block (`reset`), in the
/// Wi-Fi owner's own task, on the Wi-Fi partition alone. This function owns
/// no descriptor storage and leaves all common MAC gates closed. It prepares IRQ policy without routing a CPU interrupt or opening
/// the RX walker. The runtime owner selects its role policy, prepares the ring
/// and supplies its memory/binding and live proofs to `WifiMacHal::prepare_mac_runtime` and
/// `WifiMacHal::start_mac_runtime`.
pub async fn initialize_wifi_mac<
    M: MacColdAntennaHardware
        + MacColdCoexHardware
        + MacColdCryptoHardware
        + MacColdPrepareHardware
        + MacColdHalTailHardware
        + MacColdHeHardware
        + MacColdHandshakeHardware
        + MacColdLastRxBufferHardware
        + MacColdRxBufferHardware
        + MacColdRxPolicyHardware
        + MacColdTxRxHardware
        + MacInterfaceAddressHardware
        + MacLowRateHardware,
    P: MacCoexPtiSource + MacDelayEntropy + MacSlowClockCalibrationSource + MacTxPowerSource,
    T: Timer,
>(
    reset: MacBlockReset,
    platform: &mut P,
    mmio: &mut M,
    timer: &T,
    config: MacColdStartConfig,
) -> Result<MacColdStartOutcome, MacColdStartError> {
    // The vendor lifecycle's `wifi_reset_mac()` after `wifi_clock_enable()`
    // is the radio system's (`RadioDomain::reset_wifi_mac_block`, which
    // `reset` proves): only it writes the shared reset word. A reset under a
    // gated MAC clock does not make a cold MAC, which is why it takes the
    // clocks' proof. The preparation consumes the proof: one reset, one
    // preparation.
    let MacBlockReset { _private: () } = reset;

    let outcome = run_cold_handshake(mmio, timer, config.handshake_deadline).await?;
    // The handshake is complete, but no receive addresses or ring exist yet.
    // Keep the core held throughout configuration and the first BASE write.
    mmio.hold_mac_runtime();

    // Direct prefix, all three exact on-chip HE callback paths, then suffix.
    mmio.initialize_txrx_prefix();
    let delay_slot = MacDelaySlot::from_random(platform.mac_delay_random());
    mmio.initialize_txrx_callbacks(delay_slot);
    mmio.initialize_txrx_suffix();

    // Complete four-queue direct/leaf transaction from `hal_init`.
    mmio.initialize_cold_receive_policy();

    // `mac_rxbuf_init`, with descriptor publication left to the ring owner.
    mmio.initialize_rx_buffer_prefix();

    mmio.initialize_he_prefix();
    // Complete hal_init_tx_pwr queries all 43 calibrated PHY pairs before its
    // first MAC table write. The fixed Rust value owns that snapshot; no MAC
    // code retains a pointer into PHY or vendor global state.
    let tx_power = query_tx_power_table(platform);
    mmio.initialize_tx_power(&tx_power);
    // Complete dbg_read_tx_power is not semantically needed for logging, but
    // its 25 discarded ROM queries have 50 hardware-visible PHY RMW edges.
    run_tx_power_diagnostic_queries(platform);
    mmio.initialize_he_suffix();

    // Complete `mac_last_rxbuf_init`, including its three separate enable RMWs.
    mmio.initialize_last_rx_buffer_table();

    // `phy_disable_low_rate`, invoked by `hal_mac_disable_low_rate`.
    mmio.disable_phy_low_rate();

    // `hal_crypto_init`: even unencrypted promiscuous frames traverse the
    // common RX crypto bypass block.
    mmio.initialize_crypto_bypass();

    // Complete `hal_attenna_init`: 34 RMW edges, including both reverse
    // traversals of the eight queue/vector-bank words.
    mmio.initialize_mac_antenna();

    // Complete direct hal_init tail before its first COEX operation. The OSI
    // callback result retains whether the platform actually supplied a
    // calibration. The hardware mapping remains explicit in the HAL adapter.
    let slow_clock_calibration = platform.mac_slow_clock_calibration();
    mmio.initialize_hal_tail(slow_clock_calibration);

    // Complete seventeen-edge COEX/PTI tail. Query values in the blob's exact
    // callback order before handing the finite program to its PAC owner.
    let coex_pti = MacColdCoexPti::query(platform);
    mmio.initialize_cold_coex(coex_pti);

    // The IRQ suffix of `hal_enable_mac`, the station mask's publication,
    // belongs to the radio interrupt handler, which publishes it when it
    // activates an epoch; the core-enable edge belongs to the runtime owner
    // before its first BASE publication.

    // `wifi_set_rx_policy(0)` publishes both valid interface addresses after
    // `hal_init`; the address-valid bits are part of the S31 RX start gate even
    // when the sniffer subsequently disables address filtering.
    program_cold_receive_addresses(mmio, config.station_address, config.access_point_address);

    Ok(outcome)
}
