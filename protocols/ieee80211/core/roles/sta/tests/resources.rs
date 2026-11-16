use oer_protocols_ieee80211_contracts_port::*;
use oer_protocols_ieee80211_core_arbiter::{AbsencePlace, LeasePlace};
use oer_protocols_ieee80211_core_roles_sta::machine::{
    StaConfig, StaTypes, Station, StationResources,
};
use oer_protocols_ieee80211_wire::phy::{HeMcs, HtMcs};
struct Caps;
impl Ieee80211Capabilities for Caps {
    type SppScope = oer_protocols_ieee80211_wire::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: oer_protocols_ieee80211_contracts_port::Support::Supported,
            ofdm: oer_protocols_ieee80211_contracts_port::Support::Supported,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
            he_ldpc: oer_protocols_ieee80211_contracts_port::Support::Unsupported,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 1,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: None,
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER
            .union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        rx_amsdu_in_ampdu: true,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
        edca_max_txop_limit_units_32_us: u16::MAX,
    };
}
struct Types<'id>(core::marker::PhantomData<fn(&'id ()) -> &'id ()>);
impl<'id> StaTypes for Types<'id> {
    type GatePath = oer_protocols_radio_port::Root<oer_protocols_radio_port::LocalId<'id>>;
    type ConfigurationPath = oer_protocols_radio_port::Root<oer_protocols_radio_port::LocalId<'id>>;
    type AckEvidence = ();
    type AirDenied = ();
    type RxMeta = RxMeta;
}
fn construct<'id, const G: usize>(
    config: StaConfig<Caps, Callbacks>,
    place: LeasePlace<'id>,
    absence: AbsencePlace<'id>,
) -> (
    Station<'id, Types<'id>, Caps, Callbacks>,
    oer_protocols_ieee80211_core_roles_sta::machine::StaCommandCredits<'id>,
) {
    Station::new::<G>(
        config,
        StationResources {
            place,
            absence_place: absence,
        },
        oer_protocols_ieee80211_core_roles_sta::machine::GroupCapacity::new(),
    )
}

struct Callbacks(oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128);
impl oer_protocols_ieee80211_core_roles_sta::machine::StaCallbacks for Callbacks {
    type AesEngine = oer_protocols_ieee80211_core_security_cipher::aes::SoftwareAes128;
    fn nonce(&mut self) -> [u8; 32] {
        [0; 32]
    }
    fn aes_engine(&mut self) -> &mut Self::AesEngine {
        &mut self.0
    }
    fn he_packet_padding(
        &self,
        _: &[u8],
    ) -> oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding {
        oer_protocols_ieee80211_core_upper_mac::protection::HePacketPadding::None
    }
}

#[test]
fn a_station_builds_from_its_places_and_one_group_key() {
    let constructor: fn(_, _, _) -> _ = construct::<1>;
    core::hint::black_box(constructor);
}
