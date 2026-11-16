//! What the ESP32-S31 MAC offers the Wi-Fi path: its capabilities, the
//! metadata of a received MPDU, and the register seams of the station's
//! TBTT, the transmit gate, the beacon receive priority, the HE
//! association and the coexistence priorities, which the hardware code
//! implements ([`crate::cooperative_hardware`]).

use crate::mac::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES;
use crate::ordinary_tx::{TX_CCMP_MIC_SIZE, TX_FCS_SIZE, TX_METADATA_SIZE};
use oer_chips_esp32s31_hal::types::{MacHe20Association, MacHeBssColor, MacPti, StaTbttSchedule};
use oer_protocols_ieee80211_contracts_port::{
    BandSet, BeaconTimingCapabilities, BssColorScope, BssColorUse, ConnectionFrame, LinkMetric,
    LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter, RxEvidence, RxMeta, RxMetadata,
    SignalMetadata, VifRoleSet, WidthSet,
};
use oer_protocols_ieee80211_wire::phy::{HeMcs, HtMcs};

pub mod ampdu;
pub use ampdu::{
    ESP32S31_AMPDU_LENGTHS, ESP32S31_AMPDU_MAX_LENGTH, esp32s31_ampdu_capabilities,
    esp32s31_ampdu_length, protection_spacing,
};

/// Number of frame positions carried by the port's `u32` retry/aging masks.
pub const AMPDU_SUBFRAME_MASK_BITS: usize = u32::BITS as usize;

/// The ordinary EDCA queues, each holding one attempt in flight.
pub const LOWER_MAC_TX_QUEUES: usize = 4;

/// Interfaces the core configures at once: the station context (MAC
/// interface zero) and the access-point context (interface one).
pub const LOWER_MAC_VIFS: u8 = 2;

const RESOURCES: oer_protocols_ieee80211_contracts_port::MacResourceLimits =
    ESP32S31_MAC_SERVICE_CAPABILITIES.resources;

/// Keys installed at once: the station's pairwise and group slots and the
/// access point's pairwise and group slots the driver exposes.
pub const LOWER_MAC_KEY_SLOTS: usize = (RESOURCES.station_pairwise_ccmp_slots
    + RESOURCES.station_group_ccmp_slots
    + RESOURCES.access_point_pairwise_ccmp_slots
    + RESOURCES.access_point_group_ccmp_slots) as usize;

/// The longest PSDU (MPDU, MIC and FCS) every transmit program carries:
/// the legacy program's 12-bit length field
/// (the `signal` check of `MacLegacyTxProgram::new`, which
/// `TxSlot::submit_legacy` turns into `TxError::Invalid`); the HE program
/// takes up to 0x3ff7.
const MAX_PROGRAM_PSDU: usize = 0x0fff;

/// The receive rules the station context provides: its link policy (six)
/// for the BSS-member rules, and the ESP-NOW policy (six, mode two) that
/// also admits management frames of other BSSs.
const STATION_RECEIVE_FILTERS: ReceiveFilter =
    ReceiveFilter::BSS_MEMBER.union(ReceiveFilter::OTHER_BSS_MANAGEMENT);

/// One received MPDU's metadata on the ESP32-S31: the portable fields, and
/// the frame's RSSI from its receive prefix and the PHY's noise floor when
/// it was received, which every frame carries. The S31 therefore measures
/// every frame's signal ([`SignalMetadata`]), and a rate selection that
/// needs it can use this port's metadata directly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Esp32s31RxMeta {
    common: RxMeta,
    rssi_dbm: i8,
    noise_floor_dbm: i8,
}

impl Esp32s31RxMeta {
    /// The same metadata with the frame's receive stamp, which the runtime
    /// converts from the MAC local time.
    pub const fn with_timestamp(
        mut self,
        timestamp: RxEvidence<oer_protocols_ieee80211_contracts_port::Ieee80211Stamp>,
    ) -> Self {
        self.common.timestamp = timestamp;
        self
    }
}

impl RxMetadata for Esp32s31RxMeta {
    fn common(&self) -> &RxMeta {
        &self.common
    }
}

impl SignalMetadata for Esp32s31RxMeta {
    fn link_metric(&self) -> LinkMetric {
        LinkMetric::new(self.rssi_dbm, self.noise_floor_dbm)
    }
}

/// What the ESP32-S31 lower MAC accepts through this core, with TX slots of
/// `buffer_size` bytes.
///
/// The rates are those `TxPhyRate::try_from(PhyRate)` accepts: DSSS/CCK and
/// OFDM, HT MCS 0-7 at 20 and 40 MHz, and HE SU MCS 0-9 at 20 MHz with BCC
/// or LDPC and DCM; the DCM MCS limits are checked at submission. The
/// services are exactly the hardware-owned operations of
/// [`ESP32S31_MAC_SERVICE_CAPABILITIES`]. The longest MPDU leaves room for
/// the TX metadata word, the CCMP MIC and the FCS in the four-byte-aligned
/// descriptor capacity, and with the MIC and the FCS fits the transmit
/// programs' PSDU length field (the legacy program's 12 bits). Each ordinary
/// EDCA queue holds one attempt. 5 GHz is absent from the ESP32-S31.
pub const fn esp32s31_lower_mac_capabilities(buffer_size: usize) -> LowerMacCapabilities {
    let buffered =
        (buffer_size & !3).saturating_sub(TX_METADATA_SIZE + TX_CCMP_MIC_SIZE + TX_FCS_SIZE);
    // The transmit programs' length fields bound a PSDU too.
    let programmable = MAX_PROGRAM_PSDU - TX_CCMP_MIC_SIZE - TX_FCS_SIZE;
    let usable = if buffered < programmable {
        buffered
    } else {
        programmable
    };
    LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20.union(WidthSet::MHZ40),
        rates: RateSupport {
            dsss_cck: oer_protocols_ieee80211_contracts_port::Support::Supported,
            ofdm: oer_protocols_ieee80211_contracts_port::Support::Supported,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: oer_protocols_ieee80211_contracts_port::Support::Supported,
            he_ldpc: oer_protocols_ieee80211_contracts_port::Support::Supported,
            spatial_streams: 1,
        },
        services: ESP32S31_MAC_SERVICE_CAPABILITIES
            .operations
            .hardware_services(),
        vifs: LOWER_MAC_VIFS,
        tx_queues: LOWER_MAC_TX_QUEUES as u8,
        max_mpdu_length: if usable > u16::MAX as usize {
            u16::MAX
        } else {
            usable as u16
        },
        tx_power_ceiling_min_dbm: Some(0),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: STATION_RECEIVE_FILTERS,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER
            .union(ReceiveFilter::PROBE_REQUESTS),
        rx_block_ack_max_tid: RESOURCES.rx_block_ack_max_tid,
        rx_block_ack_max_window: RESOURCES.rx_block_ack_max_window,
        // The receive MAC acknowledges an MPDU carrying an A-MSDU inside an
        // agreement's A-MPDU but never completes it in the receive DMA: it
        // writes the whole MPDU, decrypted, into a ring buffer, then sets no
        // end of frame on its descriptor, so no unit is ever completed.
        // SOURCE: stand HIL `wifi-load` on s31-a, OpenWrt mt76 AP, HT40
        // channel 13, WPA2-CCMP, A-MSDUs granted, RX 10 Mbit/s with RTT
        // probes, 2026-11-01 (diagnostic builds, uncommitted). Each 5 s window
        // lost 99 to 100 probes; a scan of the ring's not-completed buffers
        // found 34 to 54 distinct QoS Data MPDUs with A-MSDU Present, none
        // completed: their 802.11 header, CCMP header, decrypted subframes
        // (both subframe headers, LLC/SNAP, IP), MIC and FCS were in place
        // and their RX-control tail was zero. The station's SPP decision was
        // NotNegotiated; programming the interface SPP A-MSDU bit to either
        // value lost the same probes with the payload decrypted the same.
        // No `dbg_read_rx_count` counter (abort, FCS-pass abort, TKIP error,
        // buffer full and the rest) separated these MPDUs, and no RX-control
        // refusal (MIC failure, RX failure) or chained unit was counted.
        // That does not exclude a failed MIC: with every installed key's
        // first word flipped for 200 ms under the same load, the MAC staged
        // no protected unit (12 units, all unprotected), about 200 more
        // datagrams were lost, and neither a MIC nor an RX failure was
        // counted either.
        // SOURCE: stand HIL `wifi-load`, ESP32-S31 station, HT20 channel 13,
        // OpenWrt mt76 AP, WPA2-CCMP, 2026-11-01. An air capture showed
        // every RTT probe aggregated with a load datagram into one A-MSDU
        // MPDU that the station's BlockAck acknowledged; the driver staged
        // no unit for it, no RX statistics or RX-control error counted it,
        // and the reorder skipped its sequence (99 of 99 probes at RX 10).
        // Without probes nothing was lost. 200-byte datagrams aggregated
        // into 2843 short A-MSDU MPDUs lost 12471 of 12500, so length is not
        // the cause; the interface SPP A-MSDU bit set either way changed
        // nothing. With A-MSDU refused in the ADDBA response the AP sent
        // none and nothing was lost. The vendor station's production ADDBA
        // response (`libnet80211[ieee80211_ht.o]::
        // ht_recv_action_ba_addba_request`) grants A-MSDU only under a
        // test-only switch that is zero by default, so it refuses them too.
        rx_amsdu_in_ampdu: false,
        // The bounds `EdcaContentionParameters::new` checks.
        edca_max_aifsn: 0x0f,
        edca_max_cw_exponent: crate::mac::edca::MAX_HARDWARE_ECW_EXPONENT,
        edca_max_txop_limit_units_32_us: u16::MAX,
    }
}

/// The station TSF is read, set and scheduled; the access-point TSF only
/// restarts from zero.
pub const ESP32S31_BEACON_TIMING_CAPABILITIES: BeaconTimingCapabilities =
    BeaconTimingCapabilities {
        tsf_read: VifRoleSet::STATION,
        tsf_set: VifRoleSet::STATION,
        tsf_restart: VifRoleSet::STATION.union(VifRoleSet::ACCESS_POINT),
        tbtt: VifRoleSet::STATION,
    };

/// The station TBTT schedule of the MAC's interface-zero timer, whose event
/// fires on the power interrupt (`MacPowerInterruptObservation::sta_tbtt`).
pub trait StationTbttHardware {
    fn start_station_tbtt(&mut self, schedule: StaTbttSchedule);
    fn stop_station_tbtt(&mut self);
}

/// The power-save block of every ordinary TX queue.
pub trait TxGateHardware {
    fn set_power_save_tx_block(&mut self, blocked: bool);
}

/// The beacon receive priority registers (`hal_set_rx_beacon_pti`,
/// `hal_clear_rx_beacon_pti`).
pub trait RxBeaconPriorityHardware {
    /// Receive beacons at `pti`, the beacon and the shared priority alike.
    fn set_rx_beacon_pti(&mut self, pti: MacPti);
    /// Withdraw the beacon receive priority request.
    fn clear_rx_beacon_pti(&mut self);
}

/// The station interface's HE association registers
/// (`libnet80211.a[wl_cnx.o]::cnx_connect_to_bss` and `ic_set_sta`).
pub trait StaHeHardware {
    /// Begin an HE BSS's connection before authentication: HE BSSID
    /// matching, the color bitmap cleared, the scanned BSS color.
    fn connect_he20_bss(&mut self, color: MacHeBssColor, enabled: bool, partial: bool);
    /// Install the interface-zero HE20 association in the vendor's order.
    fn install_he20_association(&mut self, association: MacHe20Association);
    /// Remove the interface-zero HE association.
    fn clear_he20_association(&mut self);
}

/// An HE setting whose values the MAC's fields do not hold; the port
/// contract's ranges lie within them, so a correct contract never produces
/// one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeAssociationRange;

/// The MAC's form of `association`.
///
/// The contract's AID (1..=2007), MMSS code (0..=7), padding (0..=16 us),
/// default PE code (0..=7) and color (0..=63) lie within the MAC's fields, which the
/// inline check below proves for each maximum.
pub fn mac_he20_association(
    association: &oer_protocols_ieee80211_contracts_port::He20Association,
) -> Result<MacHe20Association, HeAssociationRange> {
    use oer_chips_esp32s31_hal::types::{
        MacAssociationId, MacHe20PeerConfig, MacHeBssColor, MacHeDefaultPacketExtensionDuration,
        MacHePacketPaddingDuration, MacMinimumMpduStartSpacing,
    };
    use oer_protocols_ieee80211_contracts_port::He20Association;
    const {
        if (He20Association::MAX_ASSOCIATION_ID as u32) > MacAssociationId::MAX
            || MacAssociationId::MIN > 1
            || (He20Association::MAX_MINIMUM_MPDU_START_SPACING as u32)
                > MacMinimumMpduStartSpacing::MAX
            || (He20Association::MAX_PACKET_PADDING_EIGHT_US as u32) * 8
                > MacHePacketPaddingDuration::MAX
            || (He20Association::MAX_DEFAULT_PACKET_EXTENSION as u32)
                > MacHeDefaultPacketExtensionDuration::MAX
            || MacHeBssColor::MAX < 63
        {
            panic!("the HE association's ranges lie within the MAC's fields");
        }
    }
    let (color, color_enabled, partial_color) = association.color().get();
    Ok(MacHe20Association {
        peer: MacHe20PeerConfig {
            packet_padding_duration: MacHePacketPaddingDuration::new(
                u32::from(association.packet_padding_eight_us()) * 8,
            )
            .ok_or(HeAssociationRange)?,
            default_packet_extension_duration: MacHeDefaultPacketExtensionDuration::new(u32::from(
                association.default_packet_extension(),
            ))
            .ok_or(HeAssociationRange)?,
            bss_color: MacHeBssColor::new(u32::from(color.get())).ok_or(HeAssociationRange)?,
            bss_color_enabled: matches!(color_enabled, BssColorUse::Enabled),
            partial_bss_color: matches!(partial_color, BssColorScope::Partial),
            max_co_hosted_bssid_indicator: association.max_co_hosted_bssid_indicator(),
            extended_range_single_user_disabled: association.extended_range_single_user_disabled(),
        },
        association_id: MacAssociationId::new(u32::from(association.association_id()))
            .ok_or(HeAssociationRange)?,
        minimum_mpdu_start_spacing: MacMinimumMpduStartSpacing::new(u32::from(
            association.minimum_mpdu_start_spacing(),
        ))
        .ok_or(HeAssociationRange)?,
    })
}

/// The coexistence priorities of the shared radio the core shares its RF
/// with, as it last published them.
/// The core retains a concrete `C: RadioCoexPriorities` chosen by its image;
/// this policy is statically dispatched, including on the driver's publication path.
pub trait RadioCoexPriorities {
    /// The priority of the beacon-window event (`coex_pti_get(0)`), which
    /// beacon reception asks for the air with.
    fn beacon_window_pti(&self) -> MacPti;

    /// The priorities `frame` carries: while the coexistence owner's
    /// reconnect policy is on, the priorities of its event (46's as the
    /// packet priority, and the lesser of it and the slice event's as the
    /// scheduler priority); `None` keeps the ordinary priorities of the
    /// frame's access category, as outside the policy and for a Probe
    /// Request, which only requests the air.
    ///
    /// SOURCE(esp32s31): complete pinned `libpp[pp.o]::pp_coex_tx_request`
    /// and `libpp[hal_mac.o]::mac_tx_set_pti`.
    fn connection_frame_priorities(
        &self,
        frame: ConnectionFrame,
    ) -> Option<ConnectionFramePriorities>;
}

/// The packet and scheduler priorities of a connection frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConnectionFramePriorities {
    pub packet: u8,
    pub scheduler: u8,
}

/// The station's pairwise key slots.
pub const STATION_PAIRWISE_SLOTS: usize = 1;
/// The station's receive-only group key slots.
pub const STATION_GROUP_SLOTS: usize = 1;
/// The access point's pairwise key slots, one per association.
pub const ACCESS_POINT_PAIRWISE_SLOTS: usize = 15;
/// The access point's group key slots.
pub const ACCESS_POINT_GROUP_SLOTS: usize = 3;

#[cfg(test)]
mod tests;
