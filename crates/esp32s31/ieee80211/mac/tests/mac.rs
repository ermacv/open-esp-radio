use oer_esp32s31_hal::types::MacDelaySlot;
use std::{cell::RefCell, rc::Rc};

use oer_esp32s31_hal::types::{
    MacCcmpKeyIdentity, MacHtTxProgram, MacInterface, MacInterruptMask, MacKeyInstallOutcome,
    MacLegacyTxProgram, MacTxCompletionObservation, MacTxDetachOutcome, MacTxDetachReason,
    MacTxQueueDetached,
};
use oer_esp32s31_ieee80211_dma::descriptor::{
    BIT_30, BIT_31, DESCRIPTOR_BYTES, DMA_LOW, Descriptor, LENGTH_SHIFT, descriptor_address_valid,
    dma_range_valid, length, rx_armed_word, rx_rearm_word, size, tx_owned_word,
};
use oer_esp32s31_ieee80211_mac::{
    crypto::{
        CcmpKeyHardware, CryptoKeyError, clear_sta_ccmp_slots, install_sta_group_ccmp,
        install_sta_pairwise_ccmp,
    },
    init::{
        MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacColdAntennaHardware, MacColdCoexHardware,
        MacColdCoexPti, MacColdCryptoHardware, MacColdEnableHardware, MacColdHalTailHardware,
        MacColdHandshakeHardware, MacColdHeHardware, MacColdLastRxBufferHardware,
        MacColdRxBufferHardware, MacColdRxPolicyHardware, MacColdStartConfig, MacColdStartError,
        MacColdStartOutcome, MacColdTxRxHardware, MacDelayEntropy, MacInterfaceAddressHardware,
        MacLowRateHardware, MacResetHardware, MacSlowClockCalibration,
        MacSlowClockCalibrationSource, MacSnifferHardware, MacTxPowerPair, MacTxPowerSource,
        MacTxPowerTable, StaLinkRxPolicyHardware, activate_promiscuous_receive,
        configure_sta_link_receive_policy, initialize_wifi_mac,
    },
    irq::{IrqWork, consumer::IrqStorage},
    rate::schedule::{RateScheduleKind, RateScheduleRef},
    rx::{
        HeBandwidth, HeGuardIntervalAndLtf, HeMuBandwidth, HeMuSignal, HeSuSignal,
        HeTriggerBasedSignal, HtDuplicateRxClassification, INGRESS_STRICT_DUMP,
        INGRESS_STRICT_RXEND, RX_BUFFER_SENTINEL, RxBasebandFormat, RxDma, RxDmaBinding,
        RxDmaCursorObservation, RxDmaWalkerStopped, RxError, RxHe20MuSigBUsersError,
        RxIngressConfig, RxPhyInfo, RxReloadObservation, RxRingError, RxRingLive, RxRingStopped,
        RxSegment, build_cold_ring, decode_normalized_rx_metadata, decode_rx_he_mu_sig_b,
        decode_rx_local_timestamp, decode_rx_phy_info, disable_receive, enable_receive,
        extract_ccmp_data, extract_control, extract_data, extract_management, first_segment_layout,
        prepare_recycled_buffer, publish_cold_ring, rearm_descriptor, view_normalized_rx_frame,
    },
    tx::{
        AmpduTxConfig, HeAmpduTxConfig, HeBccDcmMcs, HeEdcaTxopLimit, HeFecCoding, HeLdpcDcmMcs,
        HeMcs, HeRate, HeResourceUnit, HeTriggerScheduledRate, HeTriggerScheduledRateError,
        HtAmpduDensity, HtAmpduTxConfig, HtChannelWidth, HtDuplicateCertificationRequest,
        HtDuplicateRate, HtDuplicateTxEvidenceGaps, HtDuplicateTxLinkCapabilities,
        HtDuplicateTxOracleField, HtDuplicateTxOracleGaps, HtDuplicateTxQualificationField,
        HtDuplicateTxQualificationGaps, HtDuplicateTxRejection, HtDuplicateTxSelection,
        HtDuplicateTxUnavailable, HtGuardInterval, HtMcs, HtPeerAmpduParameters,
        HtProtectionSpacing, HtRate, LegacyRate, LegacyTxConfig, LegacyTxQueue, TxCompletion,
        TxError, TxHardware, TxPhyRate, TxSlot, TxSlotState, select_esp32s31_ht_duplicate_tx,
    },
};
use oer_ieee80211_mac::he::{HeMuSigBMimoUser, HeMuSigBNonMimoUser, HeMuSigBUser};
use oer_ieee80211_mac::trigger::{
    parse_trigger_common_info, parse_trigger_frame, parse_trigger_user_spatial_stream,
};
use oer_ieee80211_softmac::{MacRxEvidence, MacRxMetadata};
use oer_memory::{HardwareOwnedTxDma, PreparedTxDma};

mod cases;
mod support;
