//! The Wi-Fi MAC register owners and their operations.
//!
//! The owners hold the PAC's generated Wi-Fi MAC blocks; every operation is
//! a sequence of the blocks' field accessors, each vendor step with its
//! `SOURCE:`. The owners are generic over where the blocks live
//! ([`MacPlacement`]): the chip, by default, or host memory in a test, which
//! runs the same operations.

use core::marker::PhantomData;

use oer_esp32s31_pac::{
    WifiInterruptPartition, WifiMacPartition, WifiMacPeripherals, wifi_mac_antenna_init,
    wifi_mac_aux_tsf_control, wifi_mac_beacon_ie_crc, wifi_mac_beamforming_feedback_test,
    wifi_mac_beamforming_report, wifi_mac_block_ack, wifi_mac_bssid_policy, wifi_mac_coex_init,
    wifi_mac_coex_runtime, wifi_mac_cold_handshake, wifi_mac_control, wifi_mac_core_enable,
    wifi_mac_crypto_control, wifi_mac_csi_dump, wifi_mac_diagnostic_statistics,
    wifi_mac_he_buffer_status, wifi_mac_he_color_collision, wifi_mac_he_init_prefix,
    wifi_mac_he_init_suffix, wifi_mac_he_mu_edca_timer, wifi_mac_he_obss_narrow_band_ru,
    wifi_mac_he_tb_diagnostics, wifi_mac_he_tb_statistics, wifi_mac_he_trigger_rx_diagnostics,
    wifi_mac_interface_address, wifi_mac_internal_tx_block_ack, wifi_mac_interrupt,
    wifi_mac_key_table, wifi_mac_last_rx_buffer, wifi_mac_local_time, wifi_mac_power_interrupt,
    wifi_mac_regdma_control, wifi_mac_rtc_timer_update, wifi_mac_rx_bssid_list,
    wifi_mac_rx_csi_control, wifi_mac_rx_custom_type, wifi_mac_rx_dma, wifi_mac_rx_end_state,
    wifi_mac_rx_filter, wifi_mac_rx_hang_statistics, wifi_mac_rx_power_save,
    wifi_mac_rx_runtime_results, wifi_mac_rx_statistics, wifi_mac_sta_beacon_filter,
    wifi_mac_sta_tbtt_target, wifi_mac_tsf_load, wifi_mac_tsf_status, wifi_mac_tsf_timer,
    wifi_mac_tx_common, wifi_mac_tx_completion, wifi_mac_tx_power_init,
    wifi_mac_tx_queue_control, wifi_mac_tx_queue_vector, wifi_mac_tx_statistics,
    wifi_mac_txrx_callbacks, wifi_mac_txrx_prefix, wifi_mac_txrx_suffix, wifi_phy_rate_oracle,
};
use oer_mmio::{Fixed, Index, Placement};

use wifi_mac_antenna_init::WifiMacAntennaInit;
use wifi_mac_aux_tsf_control::WifiMacAuxTsfControl;
use wifi_mac_beacon_ie_crc::WifiMacBeaconIeCrc;
use wifi_mac_beamforming_feedback_test::WifiMacBeamformingFeedbackTest;
use wifi_mac_beamforming_report::WifiMacBeamformingReport;
use wifi_mac_block_ack::WifiMacBlockAck;
use wifi_mac_bssid_policy::WifiMacBssidPolicy;
use wifi_mac_coex_init::WifiMacCoexInit;
use wifi_mac_coex_runtime::WifiMacCoexRuntime;
use wifi_mac_cold_handshake::WifiMacColdHandshake;
use wifi_mac_control::WifiMacControl;
use wifi_mac_core_enable::WifiMacCoreEnable;
use wifi_mac_crypto_control::WifiMacCryptoControl;
use wifi_mac_csi_dump::WifiMacCsiDump;
use wifi_mac_diagnostic_statistics::WifiMacDiagnosticStatistics;
use wifi_mac_he_buffer_status::WifiMacHeBufferStatus;
use wifi_mac_he_color_collision::WifiMacHeColorCollision;
use wifi_mac_he_init_prefix::WifiMacHeInitPrefix;
use wifi_mac_he_init_suffix::WifiMacHeInitSuffix;
use wifi_mac_he_mu_edca_timer::WifiMacHeMuEdcaTimer;
use wifi_mac_he_obss_narrow_band_ru::WifiMacHeObssNarrowBandRu;
use wifi_mac_he_tb_diagnostics::WifiMacHeTbDiagnostics;
use wifi_mac_he_tb_statistics::WifiMacHeTbStatistics;
use wifi_mac_he_trigger_rx_diagnostics::WifiMacHeTriggerRxDiagnostics;
use wifi_mac_interface_address::WifiMacInterfaceAddress;
use wifi_mac_internal_tx_block_ack::WifiMacInternalTxBlockAck;
use wifi_mac_interrupt::WifiMacInterrupt;
use wifi_mac_key_table::WifiMacKeyTable;
use wifi_mac_last_rx_buffer::WifiMacLastRxBuffer;
use wifi_mac_local_time::WifiMacLocalTimeCounter;
use wifi_mac_power_interrupt::WifiMacPowerInterrupt;
use wifi_mac_regdma_control::WifiMacRegdmaControl;
use wifi_mac_rtc_timer_update::WifiMacRtcTimerUpdate;
use wifi_mac_rx_bssid_list::WifiMacRxBssidList;
use wifi_mac_rx_csi_control::WifiMacRxCsiControl;
use wifi_mac_rx_custom_type::WifiMacRxCustomType;
use wifi_mac_rx_dma::WifiMacRxDma;
use wifi_mac_rx_end_state::WifiMacRxEndState;
use wifi_mac_rx_filter::WifiMacRxFilter;
use wifi_mac_rx_hang_statistics::WifiMacRxHangStatistics;
use wifi_mac_rx_power_save::WifiMacRxPowerSave;
use wifi_mac_rx_runtime_results::WifiMacRxRuntimeResults;
use wifi_mac_rx_statistics::WifiMacRxStatistics;
use wifi_mac_sta_beacon_filter::WifiMacStaBeaconFilter;
use wifi_mac_sta_tbtt_target::WifiMacStaTbttTarget;
use wifi_mac_tsf_load::WifiMacTsfLoad;
use wifi_mac_tsf_status::WifiMacTsfStatus;
use wifi_mac_tsf_timer::WifiMacTsfTimer;
use wifi_mac_tx_common::WifiMacTxCommon;
use wifi_mac_tx_completion::WifiMacTxCompletion;
use wifi_mac_tx_power_init::WifiMacTxPowerInit;
use wifi_mac_tx_queue_control::WifiMacTxQueueControl;
use wifi_mac_tx_queue_vector::WifiMacTxQueueVector;
use wifi_mac_tx_statistics::WifiMacTxStatistics;
use wifi_mac_txrx_callbacks::WifiMacTxrxCallbacks;
use wifi_mac_txrx_prefix::WifiMacTxrxPrefix;
use wifi_mac_txrx_suffix::WifiMacTxrxSuffix;
use wifi_phy_rate_oracle::WifiPhyRateOracle;

mod sealed {
    pub trait Sealed {}
}

/// The chip's Wi-Fi MAC blocks at their fixed addresses: the owners' default.
pub enum Chip {}

impl sealed::Sealed for Chip {}

/// Every Wi-Fi MAC block in host memory, for tests of the operations.
#[cfg(test)]
pub(crate) struct HostMemory<'a>(PhantomData<&'a mut [u32]>);

#[cfg(test)]
impl sealed::Sealed for HostMemory<'_> {}

macro_rules! mac_blocks {
    ($($field:ident: $block:ident => $assoc:ident,)*) => {
        /// Where each Wi-Fi MAC block lives: [`Chip`] or host memory in a
        /// test. Only this module implements it.
        pub trait MacPlacement: sealed::Sealed {
            $(
                #[doc = concat!("The placement of `", stringify!($field), "`.")]
                type $assoc: Placement;
            )*
            /// The placement of the receive-interface address pairs.
            type InterfaceAddress: Placement;
            /// The placement of the MAC local-time counter.
            type LocalTime: Placement;
            /// The placement of the RX descriptor walker's registers.
            type RxDma: Placement;
            /// The placement of the MAC interrupt bank.
            type Interrupt: Placement;
            /// The placement of the MAC power interrupt bank.
            type PowerInterrupt: Placement;
        }

        impl MacPlacement for Chip {
            $(type $assoc = Fixed<{ $field::BASE }>;)*
            type InterfaceAddress = Fixed<{ wifi_mac_interface_address::BASE }>;
            type LocalTime = Fixed<{ wifi_mac_local_time::BASE }>;
            type RxDma = Fixed<{ wifi_mac_rx_dma::BASE }>;
            type Interrupt = Fixed<{ wifi_mac_interrupt::BASE }>;
            type PowerInterrupt = Fixed<{ wifi_mac_power_interrupt::BASE }>;
        }

        #[cfg(test)]
        impl<'a> MacPlacement for HostMemory<'a> {
            $(type $assoc = oer_mmio::Host<'a>;)*
            type InterfaceAddress = oer_mmio::Host<'a>;
            type LocalTime = oer_mmio::Host<'a>;
            type RxDma = oer_mmio::Host<'a>;
            type Interrupt = oer_mmio::Host<'a>;
            type PowerInterrupt = oer_mmio::Host<'a>;
        }

        /// The Wi-Fi MAC blocks of one register set.
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

        /// Host memory standing in for every Wi-Fi MAC block, each word
        /// filled with one pattern.
        #[cfg(test)]
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
            /// Every block's words, each `fill`.
            pub(crate) fn filled(fill: u32) -> Self {
                Self {
                    $($field: std::vec![fill; $field::SIZE / 4],)*
                    interface_address: std::vec![fill; wifi_mac_interface_address::SIZE / 4],
                    local_time: std::vec![fill; wifi_mac_local_time::SIZE / 4],
                    rx_dma: std::vec![fill; wifi_mac_rx_dma::SIZE / 4],
                    interrupt: std::vec![fill; wifi_mac_interrupt::SIZE / 4],
                    power_interrupt: std::vec![fill; wifi_mac_power_interrupt::SIZE / 4],
                }
            }

            /// The owners over this memory.
            pub(crate) fn owners(&mut self) -> HostOwners<'_> {
                let mac = MacBlocks {
                    $($field: $block::host(&mut self.$field).expect("the block's host words"),)*
                };
                HostOwners {
                    registers: WifiRadioRegisters {
                        mac,
                        interface: WifiMacInterfaceAddress::host(&mut self.interface_address)
                            .expect("the block's host words"),
                        local_time: wifi_mac_local_time::WifiMacLocalTime::host(
                            &mut self.local_time,
                        )
                        .expect("the block's host words")
                        .into_counter(),
                    },
                    rx_dma: WifiRxDmaRegisters {
                        dma: WifiMacRxDma::host(&mut self.rx_dma).expect("the block's host words"),
                    },
                    interrupts: MacInterruptSetup {
                        mac: WifiMacInterrupt::host(&mut self.interrupt)
                            .expect("the block's host words"),
                        power: WifiMacPowerInterrupt::host(&mut self.power_interrupt)
                            .expect("the block's host words"),
                    },
                }
            }
        }
    };
}

mac_blocks! {
    wifi_phy_rate_oracle: WifiPhyRateOracle => PhyRateOracle,
    wifi_mac_antenna_init: WifiMacAntennaInit => AntennaInit,
    wifi_mac_aux_tsf_control: WifiMacAuxTsfControl => AuxTsfControl,
    wifi_mac_beacon_ie_crc: WifiMacBeaconIeCrc => BeaconIeCrc,
    wifi_mac_beamforming_feedback_test: WifiMacBeamformingFeedbackTest => BeamformingFeedbackTest,
    wifi_mac_beamforming_report: WifiMacBeamformingReport => BeamformingReport,
    wifi_mac_block_ack: WifiMacBlockAck => BlockAck,
    wifi_mac_bssid_policy: WifiMacBssidPolicy => BssidPolicy,
    wifi_mac_coex_init: WifiMacCoexInit => CoexInit,
    wifi_mac_coex_runtime: WifiMacCoexRuntime => CoexRuntime,
    wifi_mac_cold_handshake: WifiMacColdHandshake => ColdHandshake,
    wifi_mac_control: WifiMacControl => Control,
    wifi_mac_core_enable: WifiMacCoreEnable => CoreEnable,
    wifi_mac_crypto_control: WifiMacCryptoControl => CryptoControl,
    wifi_mac_csi_dump: WifiMacCsiDump => CsiDump,
    wifi_mac_diagnostic_statistics: WifiMacDiagnosticStatistics => DiagnosticStatistics,
    wifi_mac_he_buffer_status: WifiMacHeBufferStatus => HeBufferStatus,
    wifi_mac_he_color_collision: WifiMacHeColorCollision => HeColorCollision,
    wifi_mac_he_init_prefix: WifiMacHeInitPrefix => HeInitPrefix,
    wifi_mac_he_init_suffix: WifiMacHeInitSuffix => HeInitSuffix,
    wifi_mac_he_mu_edca_timer: WifiMacHeMuEdcaTimer => HeMuEdcaTimer,
    wifi_mac_he_obss_narrow_band_ru: WifiMacHeObssNarrowBandRu => HeObssNarrowBandRu,
    wifi_mac_he_tb_diagnostics: WifiMacHeTbDiagnostics => HeTbDiagnostics,
    wifi_mac_he_tb_statistics: WifiMacHeTbStatistics => HeTbStatistics,
    wifi_mac_he_trigger_rx_diagnostics: WifiMacHeTriggerRxDiagnostics => HeTriggerRxDiagnostics,
    wifi_mac_internal_tx_block_ack: WifiMacInternalTxBlockAck => InternalTxBlockAck,
    wifi_mac_key_table: WifiMacKeyTable => KeyTable,
    wifi_mac_last_rx_buffer: WifiMacLastRxBuffer => LastRxBuffer,
    wifi_mac_regdma_control: WifiMacRegdmaControl => RegdmaControl,
    wifi_mac_rtc_timer_update: WifiMacRtcTimerUpdate => RtcTimerUpdate,
    wifi_mac_rx_bssid_list: WifiMacRxBssidList => RxBssidList,
    wifi_mac_rx_csi_control: WifiMacRxCsiControl => RxCsiControl,
    wifi_mac_rx_custom_type: WifiMacRxCustomType => RxCustomType,
    wifi_mac_rx_end_state: WifiMacRxEndState => RxEndState,
    wifi_mac_rx_filter: WifiMacRxFilter => RxFilter,
    wifi_mac_rx_hang_statistics: WifiMacRxHangStatistics => RxHangStatistics,
    wifi_mac_rx_power_save: WifiMacRxPowerSave => RxPowerSave,
    wifi_mac_rx_runtime_results: WifiMacRxRuntimeResults => RxRuntimeResults,
    wifi_mac_rx_statistics: WifiMacRxStatistics => RxStatistics,
    wifi_mac_sta_beacon_filter: WifiMacStaBeaconFilter => StaBeaconFilter,
    wifi_mac_sta_tbtt_target: WifiMacStaTbttTarget => StaTbttTarget,
    wifi_mac_tsf_load: WifiMacTsfLoad => TsfLoad,
    wifi_mac_tsf_status: WifiMacTsfStatus => TsfStatus,
    wifi_mac_tsf_timer: WifiMacTsfTimer => TsfTimer,
    wifi_mac_txrx_callbacks: WifiMacTxrxCallbacks => TxrxCallbacks,
    wifi_mac_txrx_prefix: WifiMacTxrxPrefix => TxrxPrefix,
    wifi_mac_txrx_suffix: WifiMacTxrxSuffix => TxrxSuffix,
    wifi_mac_tx_common: WifiMacTxCommon => TxCommon,
    wifi_mac_tx_completion: WifiMacTxCompletion => TxCompletion,
    wifi_mac_tx_power_init: WifiMacTxPowerInit => TxPowerInit,
    wifi_mac_tx_queue_control: WifiMacTxQueueControl => TxQueueControl,
    wifi_mac_tx_queue_vector: WifiMacTxQueueVector => TxQueueVector,
    wifi_mac_tx_statistics: WifiMacTxStatistics => TxStatistics,
}

/// The Wi-Fi owners over [`HostWords`].
#[cfg(test)]
pub(crate) struct HostOwners<'a> {
    pub(crate) registers: WifiRadioRegisters<HostMemory<'a>>,
    pub(crate) rx_dma: WifiRxDmaRegisters<HostMemory<'a>>,
    pub(crate) interrupts: MacInterruptSetup<HostMemory<'a>>,
}

/// The Wi-Fi MAC register set: every Wi-Fi MAC block but the RX descriptor
/// walker's and the two interrupt banks, with a copy of the MAC local-time
/// read.
///
/// Its operations are the vendor's register leaves, each over the blocks'
/// field accessors; it has no generic address or value write.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiRadioRegisters<M: MacPlacement = Chip> {
    pub(crate) mac: MacBlocks<M>,
    pub(crate) interface: WifiMacInterfaceAddress<M::InterfaceAddress>,
    local_time: WifiMacLocalTimeCounter<M::LocalTime>,
}

impl WifiRadioRegisters {
    /// Assemble the Wi-Fi register set. This performs no MMIO.
    pub fn new(partition: WifiMacPartition) -> Self {
        let WifiMacPartition {
            blocks,
            interface,
            local_time,
        } = partition;
        Self {
            mac: MacBlocks::from_partition(blocks),
            interface,
            local_time,
        }
    }

    /// Return the partition. This performs no MMIO.
    pub fn into_partition(self) -> WifiMacPartition {
        WifiMacPartition {
            blocks: self.mac.into_partition(),
            interface: self.interface,
            local_time: self.local_time,
        }
    }
}

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// The MAC local time: the free-running microsecond counter whose
    /// readings receive timestamps carry, at the system timer's rate.
    pub fn mac_local_time(&self) -> u32 {
        self.local_time.read_counter().value()
    }

    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        oer_mmio::fence();
    }
}

/// The Wi-Fi MAC RX descriptor walker's registers: `RX_CONTROL`,
/// `RX_DESCRIPTOR_BASE`, `RX_NEXT_DESCRIPTOR`, `RX_LAST_DESCRIPTOR`, the high
/// address window and the buffer geometry words. No other register owner
/// reaches them: the HAL's RX DMA engine holds this one, and a MAC
/// initialization that writes the walker's control borrows it.
#[must_use = "dropping a radio partition permanently loses its register authority"]
pub struct WifiRxDmaRegisters<M: MacPlacement = Chip> {
    pub(crate) dma: WifiMacRxDma<M::RxDma>,
}

impl WifiRxDmaRegisters {
    /// Take the walker's registers. This performs no MMIO.
    pub const fn new(dma: WifiMacRxDma) -> Self {
        Self { dma }
    }

    /// Return the walker's registers. This performs no MMIO.
    pub fn into_block(self) -> WifiMacRxDma {
        self.dma
    }
}

impl<M: MacPlacement> WifiRxDmaRegisters<M> {
    /// Order descriptor memory and MMIO at a hardware ownership boundary.
    pub fn order_device_accesses(&mut self) {
        oer_mmio::fence();
    }
}

/// Task-side setup token for one MAC interrupt handoff epoch.
///
/// This token exists after the cold owner has been consumed but before the
/// interrupt is routed to a CPU. Activating it publishes the final mask,
/// clears stale events and consumes all task-side enable/clear access.
pub struct MacInterruptSetup<M: MacPlacement = Chip> {
    pub(crate) mac: WifiMacInterrupt<M::Interrupt>,
    pub(crate) power: WifiMacPowerInterrupt<M::PowerInterrupt>,
}

impl MacInterruptSetup {
    /// Take the interrupt banks. This performs no MMIO.
    pub fn new(partition: WifiInterruptPartition) -> Self {
        let WifiInterruptPartition { mac, power } = partition;
        Self { mac, power }
    }

    /// Return the interrupt banks. This performs no MMIO.
    pub fn into_partition(self) -> WifiInterruptPartition {
        WifiInterruptPartition {
            mac: self.mac,
            power: self.power,
        }
    }
}

/// The four ordinary EDCA transmit queues: logical queue `q` lives in the
/// physical bank `3 - q` of the queue-control and queue-vector arrays.
const ORDINARY_QUEUE_COUNT: u8 = 4;

/// The physical bank of ordinary logical queue `queue`, if it is one.
const fn ordinary_bank(queue: u8) -> Option<Index<4>> {
    match queue {
        0 => Some(Index::constant::<3>()),
        1 => Some(Index::constant::<2>()),
        2 => Some(Index::constant::<1>()),
        3 => Some(Index::constant::<0>()),
        _ => None,
    }
}

/// Every element of an `N`-element register array, in ascending order.
const fn every_index<const N: usize>() -> [Index<N>; N] {
    let mut indices = [Index::constant::<0>(); N];
    let mut position = 0;
    while position < N {
        indices[position] = match Index::new(position) {
            Ok(index) => index,
            Err(_) => panic!("a position below the count is an index"),
        };
        position += 1;
    }
    indices
}

/// The low `WIDTH` bits of `value`, for a value already bounded to them.
const fn bits<const WIDTH: u32>(value: u32) -> oer_mmio::Bits<WIDTH> {
    oer_mmio::Bits::truncate(value)
}

/// A field of at most eight bits, as the byte it holds.
const fn byte<const WIDTH: u32>(value: oer_mmio::Bits<WIDTH>) -> u8 {
    const { assert!(WIDTH <= 8, "the field fits a byte") };
    value.get().to_le_bytes()[0]
}

/// A field of at most sixteen bits, as the halfword it holds.
const fn halfword<const WIDTH: u32>(value: oer_mmio::Bits<WIDTH>) -> u16 {
    const { assert!(WIDTH <= 16, "the field fits a halfword") };
    let [low, high, _, _] = value.get().to_le_bytes();
    u16::from_le_bytes([low, high])
}

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

#[cfg(test)]
mod golden_tests;
