//! The Wi-Fi MAC register owners and their operations.
//!
//! The owners hold the PAC's generated Wi-Fi MAC parts, taken from the
//! partitions of `oer_esp32s31_pac::take()`; every operation is a sequence of
//! the parts' field accessors, each vendor step with its `SOURCE:`. The
//! owners are generic over where the parts live ([`MacPlacement`]): the chip
//! by default, or host memory in a test, which runs the same operations.

use oer_esp32s31_pac::{
    instances,
    modem_pwr::{
        WifiMacAuxTsfControl, WifiMacCoexRuntime, WifiMacLocalTime, WifiMacRegdmaControl,
        WifiMacRtcTimerUpdate, WifiMacStaTbttTarget, WifiMacTsfLoad, WifiMacTsfStatus,
        WifiMacTsfTimer,
    },
    ownership::{
        MacLocalTimePeripherals, WifiInterruptPeripherals, WifiMacInterfacePeripherals,
        WifiMacPeripherals, WifiRxDmaPeripherals,
    },
    wifi_bb::WifiPhyRateOracleWifiBb,
    wifi_bb_brx::WifiPhyRateOracleWifiBbBrx,
    wifi_mac::{
        WifiMacAntennaInit, WifiMacBeaconIeCrc, WifiMacBeamformingFeedbackTest,
        WifiMacBeamformingReport, WifiMacBlockAck, WifiMacBssidPolicy, WifiMacCoexInit,
        WifiMacColdHandshake, WifiMacControl, WifiMacCoreEnable, WifiMacCryptoControl,
        WifiMacCsiDump, WifiMacDiagnosticStatistics, WifiMacHeBufferStatus,
        WifiMacHeColorCollision, WifiMacHeInitPrefix, WifiMacHeInitSuffix, WifiMacHeMuEdcaTimer,
        WifiMacHeObssNarrowBandRu, WifiMacHeTbDiagnostics, WifiMacHeTbStatistics,
        WifiMacHeTriggerRxDiagnostics, WifiMacInterfaceAddress, WifiMacInternalTxBlockAck,
        WifiMacKeyTable, WifiMacLastRxBuffer, WifiMacRxBssidList, WifiMacRxCsiControl,
        WifiMacRxCustomType, WifiMacRxDma, WifiMacRxEndState, WifiMacRxFilter,
        WifiMacRxHangStatistics, WifiMacRxPowerSave, WifiMacRxRuntimeResults, WifiMacRxStatistics,
        WifiMacStaBeaconFilter, WifiMacTxCommon, WifiMacTxCompletion, WifiMacTxPowerInit,
        WifiMacTxQueueControl, WifiMacTxQueueVector, WifiMacTxStatistics, WifiMacTxrxCallbacks,
        WifiMacTxrxPrefix, WifiMacTxrxSuffix,
    },
};
use oer_mmio::{Bits, Fixed, Index, Placement};

pub use values::*;

mod sealed {
    pub trait Sealed {}
}

/// The chip's Wi-Fi MAC parts at their fixed addresses: the owners' default.
pub enum Chip {}

impl sealed::Sealed for Chip {}

/// Every Wi-Fi MAC part in host memory, for tests of the operations.
#[cfg(test)]
pub(crate) struct HostMemory<'a>(core::marker::PhantomData<&'a mut [u32]>);

#[cfg(test)]
impl sealed::Sealed for HostMemory<'_> {}

/// Where the Wi-Fi MAC parts live: [`Chip`], or host memory in a test. One
/// placement per instance the parts lie in. Only this module implements it.
pub trait MacPlacement: sealed::Sealed {
    /// The placement of the parts of `WIFI_MAC`.
    type Mac: Placement;
    /// The placement of the parts of `MODEM_PWR`.
    type Power: Placement;
    /// The placement of the parts of `WIFI_BB`.
    type Baseband: Placement;
    /// The placement of the parts of `WIFI_BB_BRX`.
    type BasebandRx: Placement;
}

impl MacPlacement for Chip {
    type Mac = Fixed<{ instances::wifi_mac::BASE }>;
    type Power = Fixed<{ instances::modem_pwr::BASE }>;
    type Baseband = Fixed<{ instances::wifi_bb::BASE }>;
    type BasebandRx = Fixed<{ instances::wifi_bb_brx::BASE }>;
}

#[cfg(test)]
impl<'a> MacPlacement for HostMemory<'a> {
    type Mac = oer_mmio::Host<'a>;
    type Power = oer_mmio::Host<'a>;
    type Baseband = oer_mmio::Host<'a>;
    type BasebandRx = oer_mmio::Host<'a>;
}

/// The words of one instance in host memory, each `fill`.
#[cfg(test)]
fn host_words(size: usize, fill: u32) -> std::vec::Vec<u32> {
    std::vec![fill; size.div_ceil(4)]
}

macro_rules! mac_blocks {
    ($($field:ident: $block:ident => $assoc:ident / $instance:ident,)*) => {
        /// The Wi-Fi MAC parts of one register set.
        pub(crate) struct MacBlocks<M: MacPlacement = Chip> {
            $(pub(crate) $field: $block<M::$assoc>,)*
        }

        impl MacBlocks {
            fn from_partition(blocks: WifiMacPeripherals) -> Self {
                let WifiMacPeripherals { $($field,)* } = blocks;
                Self { $($field,)* }
            }

            fn into_partition(self) -> WifiMacPeripherals {
                let Self { $($field,)* } = self;
                WifiMacPeripherals { $($field,)* }
            }
        }

        /// Host memory standing in for every Wi-Fi part: one instance's
        /// words per part, so a test compares every word each operation
        /// leaves.
        #[cfg(test)]
        #[derive(Clone, Debug, Eq, PartialEq)]
        pub(crate) struct HostWords {
            $(pub(crate) $field: std::vec::Vec<u32>,)*
            pub(crate) interface_address: std::vec::Vec<u32>,
            pub(crate) local_time: std::vec::Vec<u32>,
            pub(crate) rx_dma: std::vec::Vec<u32>,
            pub(crate) interrupt: std::vec::Vec<u32>,
            pub(crate) power_interrupt: std::vec::Vec<u32>,
        }

        #[cfg(test)]
        impl HostWords {
            /// Every part's words, each `fill`.
            pub(crate) fn filled(fill: u32) -> Self {
                Self {
                    $($field: host_words(instances::$instance::SIZE, fill),)*
                    interface_address: host_words(instances::wifi_mac::SIZE, fill),
                    local_time: host_words(instances::modem_pwr::SIZE, fill),
                    rx_dma: host_words(instances::wifi_mac::SIZE, fill),
                    interrupt: host_words(instances::wifi_mac::SIZE, fill),
                    power_interrupt: host_words(instances::modem_pwr::SIZE, fill),
                }
            }

            /// Every changed word as `(part, byte offset, new value)`, in
            /// part and memory order.
            pub(crate) fn changes(&self, after: &Self) -> std::vec::Vec<(&'static str, usize, u32)> {
                let mut changes = std::vec::Vec::new();
                $(host_changes(stringify!($field), &self.$field, &after.$field, &mut changes);)*
                host_changes("interface_address", &self.interface_address, &after.interface_address, &mut changes);
                host_changes("local_time", &self.local_time, &after.local_time, &mut changes);
                host_changes("rx_dma", &self.rx_dma, &after.rx_dma, &mut changes);
                host_changes("interrupt", &self.interrupt, &after.interrupt, &mut changes);
                host_changes("power_interrupt", &self.power_interrupt, &after.power_interrupt, &mut changes);
                changes
            }

            /// The owners over this memory.
            pub(crate) fn owners(&mut self) -> HostOwners<'_> {
                let mac = MacBlocks {
                    $($field: $block::host(&mut self.$field).expect("the part's host words"),)*
                };
                HostOwners {
                    registers: WifiRadioRegisters {
                        mac,
                        interface: WifiMacInterfaceAddress::host(&mut self.interface_address)
                            .expect("the part's host words"),
                        local_time: MacLocalTime(
                            WifiMacLocalTime::host(&mut self.local_time)
                                .expect("the part's host words"),
                        ),
                    },
                    rx_dma: WifiRxDmaRegisters {
                        dma: WifiMacRxDma::host(&mut self.rx_dma)
                            .expect("the part's host words"),
                    },
                    mac_interrupt: MacInterruptRegisters::new(
                        oer_esp32s31_pac::wifi_mac::WifiMacInterrupt::host(&mut self.interrupt)
                            .expect("the part's host words"),
                    ),
                    power_interrupt: MacPowerInterruptRegisters::new(
                        oer_esp32s31_pac::modem_pwr::WifiMacPowerInterrupt::host(
                            &mut self.power_interrupt,
                        )
                            .expect("the part's host words"),
                    ),
                }
            }
        }
    };
}

mac_blocks! {
    wifi_phy_rate_oracle_wifi_bb: WifiPhyRateOracleWifiBb => Baseband / wifi_bb,
    wifi_phy_rate_oracle_wifi_bb_brx: WifiPhyRateOracleWifiBbBrx => BasebandRx / wifi_bb_brx,
    wifi_mac_antenna_init: WifiMacAntennaInit => Mac / wifi_mac,
    wifi_mac_aux_tsf_control: WifiMacAuxTsfControl => Power / modem_pwr,
    wifi_mac_beacon_ie_crc: WifiMacBeaconIeCrc => Mac / wifi_mac,
    wifi_mac_beamforming_feedback_test: WifiMacBeamformingFeedbackTest => Mac / wifi_mac,
    wifi_mac_beamforming_report: WifiMacBeamformingReport => Mac / wifi_mac,
    wifi_mac_block_ack: WifiMacBlockAck => Mac / wifi_mac,
    wifi_mac_bssid_policy: WifiMacBssidPolicy => Mac / wifi_mac,
    wifi_mac_coex_init: WifiMacCoexInit => Mac / wifi_mac,
    wifi_mac_coex_runtime: WifiMacCoexRuntime => Power / modem_pwr,
    wifi_mac_cold_handshake: WifiMacColdHandshake => Mac / wifi_mac,
    wifi_mac_control: WifiMacControl => Mac / wifi_mac,
    wifi_mac_core_enable: WifiMacCoreEnable => Mac / wifi_mac,
    wifi_mac_crypto_control: WifiMacCryptoControl => Mac / wifi_mac,
    wifi_mac_csi_dump: WifiMacCsiDump => Mac / wifi_mac,
    wifi_mac_diagnostic_statistics: WifiMacDiagnosticStatistics => Mac / wifi_mac,
    wifi_mac_he_buffer_status: WifiMacHeBufferStatus => Mac / wifi_mac,
    wifi_mac_he_color_collision: WifiMacHeColorCollision => Mac / wifi_mac,
    wifi_mac_he_init_prefix: WifiMacHeInitPrefix => Mac / wifi_mac,
    wifi_mac_he_init_suffix: WifiMacHeInitSuffix => Mac / wifi_mac,
    wifi_mac_he_mu_edca_timer: WifiMacHeMuEdcaTimer => Mac / wifi_mac,
    wifi_mac_he_obss_narrow_band_ru: WifiMacHeObssNarrowBandRu => Mac / wifi_mac,
    wifi_mac_he_tb_diagnostics: WifiMacHeTbDiagnostics => Mac / wifi_mac,
    wifi_mac_he_tb_statistics: WifiMacHeTbStatistics => Mac / wifi_mac,
    wifi_mac_he_trigger_rx_diagnostics: WifiMacHeTriggerRxDiagnostics => Mac / wifi_mac,
    wifi_mac_internal_tx_block_ack: WifiMacInternalTxBlockAck => Mac / wifi_mac,
    wifi_mac_key_table: WifiMacKeyTable => Mac / wifi_mac,
    wifi_mac_last_rx_buffer: WifiMacLastRxBuffer => Mac / wifi_mac,
    wifi_mac_regdma_control: WifiMacRegdmaControl => Power / modem_pwr,
    wifi_mac_rtc_timer_update: WifiMacRtcTimerUpdate => Power / modem_pwr,
    wifi_mac_rx_bssid_list: WifiMacRxBssidList => Mac / wifi_mac,
    wifi_mac_rx_csi_control: WifiMacRxCsiControl => Mac / wifi_mac,
    wifi_mac_rx_custom_type: WifiMacRxCustomType => Mac / wifi_mac,
    wifi_mac_rx_end_state: WifiMacRxEndState => Mac / wifi_mac,
    wifi_mac_rx_filter: WifiMacRxFilter => Mac / wifi_mac,
    wifi_mac_rx_hang_statistics: WifiMacRxHangStatistics => Mac / wifi_mac,
    wifi_mac_rx_power_save: WifiMacRxPowerSave => Mac / wifi_mac,
    wifi_mac_rx_runtime_results: WifiMacRxRuntimeResults => Mac / wifi_mac,
    wifi_mac_rx_statistics: WifiMacRxStatistics => Mac / wifi_mac,
    wifi_mac_sta_beacon_filter: WifiMacStaBeaconFilter => Mac / wifi_mac,
    wifi_mac_sta_tbtt_target: WifiMacStaTbttTarget => Power / modem_pwr,
    wifi_mac_tsf_load: WifiMacTsfLoad => Power / modem_pwr,
    wifi_mac_tsf_status: WifiMacTsfStatus => Power / modem_pwr,
    wifi_mac_tsf_timer: WifiMacTsfTimer => Power / modem_pwr,
    wifi_mac_txrx_callbacks: WifiMacTxrxCallbacks => Mac / wifi_mac,
    wifi_mac_txrx_prefix: WifiMacTxrxPrefix => Mac / wifi_mac,
    wifi_mac_txrx_suffix: WifiMacTxrxSuffix => Mac / wifi_mac,
    wifi_mac_tx_common: WifiMacTxCommon => Mac / wifi_mac,
    wifi_mac_tx_completion: WifiMacTxCompletion => Mac / wifi_mac,
    wifi_mac_tx_power_init: WifiMacTxPowerInit => Mac / wifi_mac,
    wifi_mac_tx_queue_control: WifiMacTxQueueControl => Mac / wifi_mac,
    wifi_mac_tx_queue_vector: WifiMacTxQueueVector => Mac / wifi_mac,
    wifi_mac_tx_statistics: WifiMacTxStatistics => Mac / wifi_mac,
}

/// Append the changed words of one part's memory.
#[cfg(test)]
fn host_changes(
    part: &'static str,
    before: &[u32],
    after: &[u32],
    changes: &mut std::vec::Vec<(&'static str, usize, u32)>,
) {
    for (word, (before, after)) in before.iter().zip(after).enumerate() {
        if before != after {
            changes.push((part, word * 4, *after));
        }
    }
}

/// The Wi-Fi owners over [`HostWords`].
#[cfg(test)]
pub(crate) struct HostOwners<'a> {
    pub(crate) registers: WifiRadioRegisters<HostMemory<'a>>,
    pub(crate) rx_dma: WifiRxDmaRegisters<HostMemory<'a>>,
    pub(crate) mac_interrupt: MacInterruptRegisters<HostMemory<'a>>,
    pub(crate) power_interrupt: MacPowerInterruptRegisters<HostMemory<'a>>,
}

/// The read of the free-running Wi-Fi MAC local-time counter.
///
/// The generated part is a `Copy` reader: reading the counter has no side
/// effect and no part writes it, so the radio PHY (its SDM-stability
/// deadline) and the Wi-Fi MAC (the time its receive timestamps count in)
/// each hold a copy.
pub struct MacLocalTime<P: Placement = Fixed<{ instances::modem_pwr::BASE }>>(WifiMacLocalTime<P>);

impl<P: Placement> Clone for MacLocalTime<P> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<P: Placement> Copy for MacLocalTime<P> {}

impl MacLocalTime {
    /// Take the counter's read from its partition. This performs no MMIO.
    pub fn new(partition: MacLocalTimePeripherals) -> Self {
        let MacLocalTimePeripherals {
            wifi_mac_local_time,
        } = partition;
        Self(wifi_mac_local_time)
    }
}

impl<P: Placement> MacLocalTime<P> {
    /// The counter now: a full-width wrapping value in microseconds, at the
    /// system timer's rate (observed on hardware).
    ///
    /// SOURCE: ROM_REV0_PHY_WAIT_I2C_SDM_STABLE, BLOB_LIBPP_MAC_LOCAL_TIME.
    pub fn now(self) -> u32 {
        self.0.read_counter().value()
    }
}

impl<P: Placement> core::fmt::Debug for MacLocalTime<P> {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str("MacLocalTime")
    }
}

/// The Wi-Fi MAC register set: every Wi-Fi MAC part but the RX descriptor
/// walker's and the two interrupt banks, with a copy of the MAC local-time
/// read.
///
/// Its operations are the vendor's register leaves, each over the parts'
/// field accessors; it has no generic address or value write.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiRadioRegisters<M: MacPlacement = Chip> {
    pub(crate) mac: MacBlocks<M>,
    pub(crate) interface: WifiMacInterfaceAddress<M::Mac>,
    local_time: MacLocalTime<M::Power>,
}

/// The Wi-Fi MAC partitions of the PAC root with a copy of the MAC
/// local-time read: what a concurrent split hands the Wi-Fi client.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiMacPartition {
    mac: WifiMacPeripherals,
    interface: WifiMacInterfacePeripherals,
    local_time: MacLocalTime,
}

impl WifiMacPartition {
    /// Take the partitions of `oer_esp32s31_pac::take()`. This performs no
    /// MMIO.
    pub fn new(
        mac: WifiMacPeripherals,
        interface: WifiMacInterfacePeripherals,
        local_time: MacLocalTime,
    ) -> Self {
        Self {
            mac,
            interface,
            local_time,
        }
    }

    /// The MAC local-time read this partition holds a copy of.
    pub const fn mac_local_time(&self) -> MacLocalTime {
        self.local_time
    }

    /// Return the partitions. This performs no MMIO.
    pub fn into_partitions(self) -> (WifiMacPeripherals, WifiMacInterfacePeripherals) {
        (self.mac, self.interface)
    }
}

impl WifiRadioRegisters {
    /// Assemble the Wi-Fi register set. This performs no MMIO.
    pub fn new(partition: WifiMacPartition) -> Self {
        let WifiMacPartition {
            mac,
            interface:
                WifiMacInterfacePeripherals {
                    wifi_mac_interface_address,
                },
            local_time,
        } = partition;
        Self {
            mac: MacBlocks::from_partition(mac),
            interface: wifi_mac_interface_address,
            local_time,
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> WifiMacPartition {
        WifiMacPartition {
            mac: self.mac.into_partition(),
            interface: WifiMacInterfacePeripherals {
                wifi_mac_interface_address: self.interface,
            },
            local_time: self.local_time,
        }
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// The MAC local-time read the Wi-Fi registers hold a copy of.
    pub fn mac_local_time(&self) -> MacLocalTime<M::Power> {
        self.local_time
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        oer_riscv::fence();
    }
}

/// The Wi-Fi MAC RX descriptor walker's registers: `RX_CONTROL`,
/// `RX_DESCRIPTOR_BASE`, `RX_NEXT_DESCRIPTOR`, `RX_LAST_DESCRIPTOR`, the high
/// address window and the buffer geometry words. No other register owner
/// reaches them: the HAL's RX DMA engine holds this one, and a MAC
/// initialization that writes the walker's control borrows it.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiRxDmaRegisters<M: MacPlacement = Chip> {
    pub(crate) dma: WifiMacRxDma<M::Mac>,
}

impl WifiRxDmaRegisters {
    /// Take the walker's registers. This performs no MMIO.
    pub fn new(partition: WifiRxDmaPeripherals) -> Self {
        let WifiRxDmaPeripherals { wifi_mac_rx_dma } = partition;
        Self {
            dma: wifi_mac_rx_dma,
        }
    }

    /// Return the walker's registers. This performs no MMIO.
    pub fn into_partition(self) -> WifiRxDmaPeripherals {
        WifiRxDmaPeripherals {
            wifi_mac_rx_dma: self.dma,
        }
    }
}

impl<M: MacPlacement> WifiRxDmaRegisters<M> {
    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        oer_riscv::fence();
    }
}

/// The Wi-Fi MAC's two interrupt banks, as the root hands them out.
///
/// Each bank's enable, status and clear words belong to the one radio
/// interrupt handler that the image's interrupt table binds them to.
pub(crate) fn interrupt_banks(
    partition: WifiInterruptPeripherals,
) -> (MacInterruptRegisters, MacPowerInterruptRegisters) {
    let WifiInterruptPeripherals {
        wifi_mac_interrupt,
        wifi_mac_power_interrupt,
    } = partition;
    (
        MacInterruptRegisters::new(wifi_mac_interrupt),
        MacPowerInterruptRegisters::new(wifi_mac_power_interrupt),
    )
}

/// The four ordinary EDCA transmit queues: logical queue `q` lives in the
/// physical bank `3 - q` of the queue-control and queue-vector arrays.
pub(crate) const ORDINARY_QUEUE_COUNT: u8 = oer_esp32s31_pac::wifi_mac::control_n::COUNT as u8;

/// The physical bank of ordinary logical queue `queue`, if it is one.
pub(crate) const fn ordinary_bank(queue: u8) -> Option<Index<4>> {
    match queue {
        0 => Some(Index::constant::<3>()),
        1 => Some(Index::constant::<2>()),
        2 => Some(Index::constant::<1>()),
        3 => Some(Index::constant::<0>()),
        _ => None,
    }
}

/// Element `position` of an `N`-element register array; `position` is below
/// `N` by its caller's loop or table.
pub(crate) const fn element<const N: usize>(position: usize) -> Index<N> {
    match Index::new(position) {
        Ok(index) => index,
        Err(_) => panic!("a position below the count is an index"),
    }
}

/// The low `WIDTH` bits of `value`, for a value already bounded to them.
pub(crate) const fn bits<const WIDTH: u32>(value: u32) -> Bits<WIDTH> {
    Bits::truncate(value)
}

/// A field of at most eight bits, as the byte it holds.
pub(crate) const fn byte<const WIDTH: u32>(value: Bits<WIDTH>) -> u8 {
    const { assert!(WIDTH <= 8, "the field fits a byte") };
    value.get().to_le_bytes()[0]
}

/// A field of at most sixteen bits, as the halfword it holds.
pub(crate) const fn halfword<const WIDTH: u32>(value: Bits<WIDTH>) -> u16 {
    const { assert!(WIDTH <= 16, "the field fits a halfword") };
    let [low, high, _, _] = value.get().to_le_bytes();
    u16::from_le_bytes([low, high])
}

mod values;

pub mod antenna_init;
pub mod block_ack;
pub mod channel;
pub mod coex;
pub mod cold_start;
pub mod crypto;
pub mod enable;
pub mod hal_init_tail;
pub mod he;
pub mod interface_address;
pub mod interrupt;
pub mod last_rx_buffer;
pub mod phy_rate;
pub mod rx;
pub mod sniffer;
pub mod softap_tsf;
pub mod tsf;
pub mod tx;
pub mod txrx_init;

pub use block_ack::{
    ExtraSoftApRxBlockAckEntrySnapshot, InternalTxBlockAckSnapshot, RxBlockAckEntrySnapshot,
    TxBlockAckDiagnosticSnapshot, TxBlockAckPayload,
};
pub use coex::init::MacCoexPrioritySnapshot;
pub use crypto::{MacCcmpKeyIdentity, MacKeyInstallOutcome, MacSppMode};
pub use hal_init_tail::MacRtcCalibration;
pub use he::{
    beamforming::{
        MacHeBeamformingReportProfile, MacHeBeamformingReportProfileError, MacHeErSuAckRateProfile,
    },
    init_suffix::MacHeTxMpduLengthLink,
    ofdma::{
        MacBeamformingAverageSnr, MacHeBeamformingConfigurationSnapshot,
        MacHeBeamformingDiagnostics, MacHeBufferStatusSnapshot, MacHeCustomReceiveType,
        MacHeEdcaQueueConfiguration, MacHeMuEdcaTimerSnapshot, MacHeQueueSchedulingSnapshot,
        MacHeReceiveConfigurationSnapshot, MacHeRxPowerSaveSnapshot, MacHeTbLinkReservation,
        MacHeTbProgramError, MacHeTbTidLimit, MacHeTid, MacHeTriggerQueueConfiguration,
        MacHeTriggerRxDiagnostics, MacHeTriggerTxQueueSnapshot,
    },
    peer::{MacHe20PeerConfig, MacHe20PeerError},
    tb::{MacHeTbStatistics, MacHeTbTxDiagnostics},
};
pub use interrupt::{
    ConnectedStaWithoutPowerSavePrepared, MacInterruptEnableState, MacInterruptEvents,
    MacInterruptObservation, MacInterruptRegisters, MacInterruptSnapshot,
    MacPowerInterruptObservation, MacPowerInterruptRegisters, MacPowerInterruptSnapshot,
};
pub use rx::{
    dma::{MacRxDmaSnapshot, MacRxNextDescriptorObservation},
    policy::{
        MacApReceivePolicySnapshot, MacRoleReceivePolicy, MacStaApReceivePlan, MacStaPolicyMode,
        MacStaReceivePolicySnapshot,
    },
    statistics::{
        MacHeColorCollisionSnapshot, MacRxDecodeErrorStatistics, MacRxDecodeErrorStatisticsDelta,
        MacRxHangStatistics, MacRxHangStatisticsDelta, MacRxPrimaryStatistics,
        MacRxPrimaryStatisticsDelta, MacRxStatisticsSnapshot,
    },
};
pub use tsf::StaTbttSchedule;
pub use tx::{
    MacHeFecCoding, MacHeGuardIntervalAndLtf, MacHeMcs, MacHeRate, MacHeTxFormat,
    MacHeTxParameters, MacHeTxProgram, MacHtAmpduCompletionObservation, MacHtChannelWidth,
    MacHtGuardInterval, MacHtMcs, MacHtProtectionSpacing, MacHtRate, MacHtTxFormat,
    MacHtTxParameters, MacHtTxProgram, MacLegacyRate, MacLegacyTxParameters, MacLegacyTxProgram,
    MacLegacyTxResponse, MacOrdinaryTxQueueSnapshot, MacTxCompletionObservation, MacTxControlFrame,
    MacTxDetachOutcome, MacTxDetachReason, MacTxProtection, MacTxPtiProgram, MacTxQueueDetached,
    power_init::{
        MAC_TX_POWER_RATE_COUNT, MacPartialRuPowerSelector, MacTxPowerIndex, MacTxPowerPair,
        MacTxPowerTable,
    },
    statistics::MacTxStatisticsSnapshot,
};
pub use txrx_init::MacDelaySlot;
