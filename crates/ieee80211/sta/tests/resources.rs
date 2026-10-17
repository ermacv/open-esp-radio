use oer_ieee80211_arbiter::{AbsencePlace, LeasePlace};
use oer_ieee80211_lower_mac::association::KEY_BANK_CAPACITY;
use oer_ieee80211_lower_mac::*;
use oer_ieee80211_mac::phy::{HeMcs, HtMcs};
use oer_ieee80211_sta::epoch::LinkEpochSource;
use oer_ieee80211_sta::machine::{StaConfig, StaTokens, Station, StationResources};
struct Caps;
impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_mac::security::spp::Association;
    const CAPS: LowerMacCapabilities = LowerMacCapabilities {
        bands: BandSet::GHZ2_4,
        widths: WidthSet::MHZ20,
        rates: RateSupport {
            dsss_cck: true,
            ofdm: true,
            ht_max_mcs: HtMcs::new(7),
            he_max_mcs: HeMcs::new(9),
            he_max_bandwidth_mhz: 20,
            he_dcm: false,
            he_ldpc: false,
            spatial_streams: 1,
        },
        services: HardwareServices::FCS,
        vifs: 1,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: None,
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER
            .union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}
struct Buffer;
impl TxBufferMemory for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut []
    }
}
struct Tokens<const GROUPS: usize>;
impl oer_ieee80211_lower_mac::TxBufferClass for Buffer {
    const CAPACITY: usize = 256;
}
impl<const GROUPS: usize> StaTokens for Tokens<GROUPS> {
    type AssociationStart = ();
    type Association = ();
    type VacantKeyBank =
        oer_ieee80211_lower_mac::association::KeyBankParts<(), (), KEY_BANK_CAPACITY>;
    type VacantAssociation = [(); GROUPS];
    fn begin_association(_: (), _: oer_ieee80211_mac::security::Spp) {}
    fn association(key: &()) -> &() {
        key
    }
    fn split_vacant(group: Self::VacantAssociation) -> ((), (), Self::VacantKeyBank) {
        (
            (),
            (),
            oer_ieee80211_lower_mac::association::KeyBankParts {
                pairwise: core::iter::empty().collect(),
                group: group.into_iter().collect(),
            },
        )
    }
    fn collect_key_bank(
        parts: Self::VacantKeyBank,
    ) -> Result<Self::VacantKeyBank, Self::VacantKeyBank> {
        Ok(parts)
    }
    fn end_association(_: (), mut bank: Self::VacantKeyBank) -> ((), (), Self::VacantKeyBank) {
        bank.pairwise.pop();
        ((), (), bank)
    }
    const PAIRWISE_SLOTS: usize = 1;
    const TOTAL_GROUP_SLOTS: usize = GROUPS;
    const GROUP_SLOTS: usize = GROUPS;
    type GatePath = oer_radio_port::Root;
    type EpochPath = oer_radio_port::Root;
    type ConfigurationPath = oer_radio_port::Root;
    type Small = Buffer;
    type Ticket = u8;
    type SmallBuffer = Buffer;
    type PairwiseSlot = ();
    type Sent = core::convert::Infallible;
    type AckEvidence = ();
    type AirDenied = ();
    fn renew(mpdu: Self::Sent) -> Buffer {
        match mpdu {}
    }
    type GroupSlot = ();
    type PairwiseKey = ();
    type GroupKey = ();
    type GroupPermission = ();
    type RxMeta = RxMeta;
}
fn construct<'id, const T: usize, const B: usize, const G: usize>(
    epochs: LinkEpochSource<'id>,
    config: StaConfig<Caps>,
    place: LeasePlace<'id>,
    absence: AbsencePlace<'id>,
    configure: oer_ieee80211_lower_mac::ConfigurePermission<'id, oer_ieee80211_lower_mac::Station>,
) -> Station<'id, Tokens<G>, Caps> {
    Station::new_open::<T, B, G>(
        config,
        StationResources {
            configure,
            epochs,
            tickets: [0; T],
            buffers: [const { Buffer }; B],
            place,
            absence_place: absence,
            keys: [(); G],
            group_permission: (),
        },
    )
}

#[test]
fn minimum_pools_are_well_formed() {
    let constructor: fn(_, _, _, _, _) -> _ = construct::<1, 1, 1>;
    core::hint::black_box(constructor);
}
