use oer_ieee80211_lower_mac::RxEvidence;
use oer_ieee80211_mac::channel::ChannelWidth;
use oer_ieee80211_softmac::MacRxEvidence;

use super::*;

const LEGACY: [LegacyRate; 15] = [
    LegacyRate::Dsss1MLong,
    LegacyRate::Dsss2MLong,
    LegacyRate::Cck5M5Long,
    LegacyRate::Cck11MLong,
    LegacyRate::Dsss2MShort,
    LegacyRate::Cck5M5Short,
    LegacyRate::Cck11MShort,
    LegacyRate::Ofdm48M,
    LegacyRate::Ofdm24M,
    LegacyRate::Ofdm12M,
    LegacyRate::Ofdm6M,
    LegacyRate::Ofdm54M,
    LegacyRate::Ofdm36M,
    LegacyRate::Ofdm18M,
    LegacyRate::Ofdm9M,
];

const GI_LTF: [HeGuardIntervalAndLtf; 4] = [
    HeGuardIntervalAndLtf::OneLtf800Ns,
    HeGuardIntervalAndLtf::TwoLtf800Ns,
    HeGuardIntervalAndLtf::TwoLtf1600Ns,
    HeGuardIntervalAndLtf::FourLtf3200Ns,
];

fn every_tx_rate() -> impl Iterator<Item = TxPhyRate> {
    let legacy = LEGACY.into_iter().map(TxPhyRate::Legacy);
    let ht = (0..=7).flat_map(|index| {
        let mcs = HtMcs::from_index(index).unwrap();
        [
            (HtGuardInterval::Long800Ns, HtChannelWidth::Mhz20),
            (HtGuardInterval::Short400Ns, HtChannelWidth::Mhz20),
            (HtGuardInterval::Long800Ns, HtChannelWidth::Mhz40),
            (HtGuardInterval::Short400Ns, HtChannelWidth::Mhz40),
        ]
        .map(move |(gi, width)| TxPhyRate::Ht(HtRate::new(mcs, gi, width)))
    });
    let he = GI_LTF.into_iter().flat_map(|gi_ltf| {
        let plain = (0..=9).flat_map(move |index| {
            let mcs = HeMcs::from_index(index).unwrap();
            [HeRate::new(mcs, gi_ltf), HeRate::ldpc(mcs, gi_ltf)]
        });
        let bcc_dcm = [HeBccDcmMcs::Mcs0, HeBccDcmMcs::Mcs1, HeBccDcmMcs::Mcs3]
            .map(|mcs| HeRate::bcc_dcm(mcs, gi_ltf));
        let ldpc_dcm = [
            HeLdpcDcmMcs::Mcs0,
            HeLdpcDcmMcs::Mcs1,
            HeLdpcDcmMcs::Mcs3,
            HeLdpcDcmMcs::Mcs4,
        ]
        .map(|mcs| HeRate::ldpc_dcm(mcs, gi_ltf));
        plain.chain(bcc_dcm).chain(ldpc_dcm).map(TxPhyRate::He)
    });
    legacy.chain(ht).chain(he)
}

#[test]
fn every_s31_transmit_rate_round_trips_through_the_portable_rate() {
    let mut count = 0;
    for rate in every_tx_rate() {
        assert_eq!(TxPhyRate::try_from(rate.phy_rate()), Ok(rate), "{rate:?}");
        count += 1;
    }
    assert_eq!(count, 15 + 32 + 4 * (20 + 3 + 4));
}

#[test]
fn transmit_rate_fields_keep_their_meaning() {
    let PhyRate::Legacy(rate) = TxPhyRate::Legacy(LegacyRate::Cck5M5Short).phy_rate() else {
        panic!("non-HT");
    };
    assert_eq!(rate, phy::LegacyRate::Cck5M5(DsssPreamble::Short));
    assert_eq!(rate.kbps(), LegacyRate::Cck5M5Short.nominal_kbps());

    let ht = HtRate::new(
        HtMcs::Mcs5,
        HtGuardInterval::Short400Ns,
        HtChannelWidth::Mhz40,
    );
    let PhyRate::Ht(portable) = TxPhyRate::Ht(ht).phy_rate() else {
        panic!("HT");
    };
    assert_eq!(portable.mcs().index(), 5);
    assert_eq!(portable.bandwidth(), PpduBandwidth::Mhz40);
    assert!(portable.short_gi());

    let he = HeRate::ldpc_dcm(HeLdpcDcmMcs::Mcs4, HeGuardIntervalAndLtf::TwoLtf1600Ns);
    let PhyRate::He(portable) = TxPhyRate::He(he).phy_rate() else {
        panic!("HE");
    };
    assert_eq!(portable.mcs().index(), 4);
    assert_eq!(portable.gi_ltf(), HeGiLtf::Ltf2xGi1600Ns);
    assert_eq!(portable.fec(), FecCoding::Ldpc);
    assert!(portable.dcm());
    assert_eq!(portable.bandwidth(), PpduBandwidth::Mhz20);
}

#[test]
fn portable_rates_beyond_the_s31_formatter_are_refused() {
    let ht = |mcs, bandwidth| {
        PhyRate::Ht(phy::HtRate::new(phy::HtMcs::new(mcs).unwrap(), bandwidth, false).unwrap())
    };
    assert_eq!(
        TxPhyRate::try_from(ht(8, PpduBandwidth::Mhz20)),
        Err(UnsupportedTxRate::HtMcs)
    );
    let he = |mcs, streams, bandwidth, fec, dcm| {
        PhyRate::He(
            phy::HeRate::new(
                phy::HeMcs::new(mcs).unwrap(),
                SpatialStreams::new(streams).unwrap(),
                bandwidth,
                HeGiLtf::Ltf2xGi800Ns,
                fec,
                dcm,
            )
            .unwrap(),
        )
    };
    assert_eq!(
        TxPhyRate::try_from(he(10, 1, PpduBandwidth::Mhz20, FecCoding::Bcc, false)),
        Err(UnsupportedTxRate::HeMcs)
    );
    assert_eq!(
        TxPhyRate::try_from(he(0, 2, PpduBandwidth::Mhz20, FecCoding::Bcc, false)),
        Err(UnsupportedTxRate::HeSpatialStreams)
    );
    assert_eq!(
        TxPhyRate::try_from(he(0, 1, PpduBandwidth::Mhz40, FecCoding::Bcc, false)),
        Err(UnsupportedTxRate::HeBandwidth)
    );
    assert_eq!(
        TxPhyRate::try_from(he(4, 1, PpduBandwidth::Mhz20, FecCoding::Bcc, true)),
        Err(UnsupportedTxRate::HeDcm)
    );
    assert!(TxPhyRate::try_from(he(4, 1, PpduBandwidth::Mhz20, FecCoding::Ldpc, true)).is_ok());
}

const DOT11B: u8 = 0;
const OFDM: u8 = 1;
const HT: u8 = 2;
const HE_SU: u8 = 4;
const HE_MU: u8 = 5;

fn prefix(bb_format: u8, rate: u8, he_siga1: u32, he_siga2: u16) -> RxPhyInfo {
    RxPhyInfo {
        rate,
        bb_format,
        he_siga1,
        he_siga2,
    }
}

#[test]
fn non_ht_receptions_decode_the_rate_code_of_their_format() {
    for rate in LEGACY {
        let dsss = prefix(DOT11B, rate.code(), 0, 0).phy_rate();
        let ofdm = prefix(OFDM, rate.code(), 0, 0).phy_rate();
        let expected = Some(PhyRate::Legacy(rate.phy_rate()));
        if rate.phy_rate().is_ofdm() {
            assert_eq!((dsss, ofdm), (None, expected), "{rate:?}");
        } else {
            assert_eq!((dsss, ofdm), (expected, None), "{rate:?}");
        }
    }
    assert_eq!(prefix(DOT11B, 0x04, 0, 0).phy_rate(), None);
}

#[test]
fn ht_receptions_decode_ht_sig() {
    // MCS 7, 40 MHz, aggregated, short GI.
    let siga1 = 7 | (1 << 7) | (1 << 27) | (1 << 31);
    let expected = phy::HtRate::new(phy::HtMcs::new(7).unwrap(), PpduBandwidth::Mhz40, true);
    assert_eq!(
        prefix(HT, 0, siga1, 0).phy_rate(),
        expected.map(PhyRate::Ht)
    );
    // MCS 32 is the duplicate format, not an ordinary rate.
    assert_eq!(prefix(HT, 0, 32 | (1 << 7), 0).phy_rate(), None);
}

#[test]
fn he_su_receptions_decode_he_sig_a() {
    // MCS 3, DCM, 20 MHz, 2x LTF + 0.8 us, one space-time stream; LDPC.
    let siga1 = (3 << 3) | (1 << 7) | (1 << 21);
    let siga2 = 1 << 7;
    let expected = phy::HeRate::new(
        phy::HeMcs::new(3).unwrap(),
        SpatialStreams::ONE,
        PpduBandwidth::Mhz20,
        HeGiLtf::Ltf2xGi800Ns,
        FecCoding::Ldpc,
        true,
    );
    assert_eq!(
        prefix(HE_SU, 0, siga1, siga2).phy_rate(),
        expected.map(PhyRate::He)
    );
    // Two space-time streams under STBC are one spatial stream.
    let stbc = prefix(HE_SU, 0, (9 << 3) | (1 << 23), 1 << 9).phy_rate();
    let Some(PhyRate::He(stbc)) = stbc else {
        panic!("HE SU with STBC");
    };
    assert_eq!(stbc.spatial_streams(), SpatialStreams::ONE);
    // A Doppler midamble hides the stream count.
    assert_eq!(prefix(HE_SU, 0, 0, 1 << 15).phy_rate(), None);
    assert_eq!(prefix(HE_MU, 0, 0, 0).phy_rate(), None);
}

#[test]
fn receive_metadata_keeps_provenance_and_takes_the_configured_channel() {
    let channel = Channel::ghz2_4(11, ChannelWidth::Mhz20).unwrap();
    let chip = MacRxMetadata {
        rate: MacRxEvidence::HardwareObserved(prefix(OFDM, LegacyRate::Ofdm6M.code(), 0, 0)),
        rssi_dbm: MacRxEvidence::HardwareObserved(-61),
        s_mpdu: MacRxEvidence::HardwareObserved(false),
        ampdu: MacRxEvidence::ProtocolValidated(false),
        ..MacRxMetadata::unavailable()
    };
    let meta = rx_meta(chip, channel);
    assert_eq!(meta.channel, channel);
    assert_eq!(
        meta.rate,
        RxEvidence::HardwareObserved(PhyRate::Legacy(phy::LegacyRate::Ofdm6M))
    );
    assert_eq!(meta.rssi_dbm, RxEvidence::HardwareObserved(-61));
    assert_eq!(meta.ampdu, RxEvidence::ProtocolValidated(false));
    assert_eq!(meta.noise_floor_dbm, RxEvidence::Unavailable);
    assert_eq!(meta.timestamp, RxEvidence::Unavailable);

    let mu = MacRxMetadata {
        rate: MacRxEvidence::HardwareObserved(prefix(HE_MU, 0, 0, 0)),
        ..chip
    };
    assert_eq!(rx_meta(mu, channel).rate, RxEvidence::Unavailable);
}

#[test]
fn the_s31_hardware_services_are_the_six_below_the_port() {
    use oer_ieee80211_lower_mac::HardwareServices;

    assert_eq!(
        crate::capabilities::ESP32S31_MAC_SERVICE_CAPABILITIES
            .operations
            .hardware_services(),
        HardwareServices::FCS
            .union(HardwareServices::IMMEDIATE_ACK)
            .union(HardwareServices::BACKOFF_COUNTDOWN)
            .union(HardwareServices::CIPHER_TRANSFORM)
            .union(HardwareServices::RX_BLOCK_ACK_MATCHING)
            .union(HardwareServices::TX_BLOCK_ACK_CAPTURE)
    );
}
