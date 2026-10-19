use super::*;
use oer_ieee80211_lower_mac::{QosControl, Tid};
use oer_ieee80211_mac::security::Spp;

// Independent AESCCM wire peer, PN 010203040506, TID 5, one A-MSDU subframe.
const WIRE: &[(Spp, &str, &str)] = &[
    (
        Spp::NotNegotiated,
        "88410000020000000002020000000001020000000003301285000605002004030201f07eede0098f221607e15fba00f6d3700d45aacc36579a5c2fb58aad5da6dac379a5",
        "88420000020000000001020000000002020000000003301285000605002004030201d48908a774db803d957983d3cada8502b9396fef745b09a70a3e9546c93ea961a5e2",
    ),
    (
        Spp::Negotiated,
        "884100000200000000020200000000010200000000033012850006050020040302013edc756662394dbcfb0ae5899079214592511815ae78bf94e73bebfda59517877bdd",
        "88420000020000000001020000000002020000000003301285000605002004030201bcd472f2766f73794735481b94e75ff9bde8c13d1359f9fd5341ddf58e25ff82a425",
    ),
];
fn bytes(hex: &str) -> Vec<u8> {
    hex.as_bytes()
        .chunks_exact(2)
        .map(|p| u8::from_str_radix(core::str::from_utf8(p).unwrap(), 16).unwrap())
        .collect()
}

struct WirePeer(Spp, &'static str, &'static str);
impl Work for WirePeer {
    async fn work<'p, 'id: 'p>(
        self,
        phase: &mut ModelPhase<'p, 'id>,
        resources: ModelResources<'id>,
    ) {
        let association =
            ModelKeys::<Station>::begin_association(resources.station_association, self.0);
        let [ptk_slot, unused_pairwise] = resources.station_pairwise;
        let [gtk_slot, unused_group] = resources.station_group;
        let material = core::array::from_fn(|i| i as u8);
        let key = phase.station().install_pairwise_key(
            &association,
            ptk_slot,
            IndividualAddress::new(ACCESS_POINT).unwrap(),
            Ccmp128Key::new(&material),
        );
        phase.station().install_group_key(
            &association,
            resources.station_group_permission,
            gtk_slot,
            GroupKeyId::new(1).unwrap(),
            Ccmp128Key::new(&[0x47; 16]),
        );
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::GroupKeyInstalled(
            InstalledGroup::Station {
                key: group,
                permission,
            },
        )))) = phase.next_event().await
        else {
            panic!("GTK terminal");
        };
        assert_eq!(key.identity.0.role, group.identity.0.role);
        assert_eq!(
            phase
                .held
                .air
                .associations
                .borrow()
                .get(key.identity.0.role)
                .spp(),
            self.0
        );
        assert_eq!(
            phase.held.air.crypto_operations(),
            [
                ModelCryptoOperation::Association {
                    role: VifRole::Station,
                    spp: self.0
                },
                ModelCryptoOperation::Key {
                    role: VifRole::Station,
                    slot: 0
                },
                ModelCryptoOperation::Association {
                    role: VifRole::Station,
                    spp: self.0
                },
                ModelCryptoOperation::Key {
                    role: VifRole::Station,
                    slot: 0
                },
            ]
        );
        // Installing a different mode at the other interface must leave this
        // association's shared PTK/GTK policy and both wire directions intact.
        let ap_spp = match self.0 {
            Spp::Negotiated => Spp::NotNegotiated,
            Spp::NotNegotiated => Spp::Negotiated,
        };
        let ap_association =
            ModelKeys::<AccessPoint>::begin_association(resources.access_point_association, ap_spp);
        let [ap_slot, _unused_ap_slot] = resources.access_point_pairwise;
        let _ap_key = phase.access_point().install_pairwise_key(
            &ap_association,
            ap_slot,
            IndividualAddress::new(STATION).unwrap(),
            Ccmp128Key::new(&[0x48; 16]),
        );
        key.identity.0.last.set(0x0102_0304_0505);
        let mut body = alloc::vec![2, 0, 0, 0, 0, 3, 2, 0, 0, 0, 0, 1, 0, 12];
        body.extend_from_slice(b"\xaa\xaa\x03\0\0\0\x08\0test");
        let header = HeaderSpec::new(
            FrameControl::new(0x4188).unwrap(),
            0,
            IndividualAddress::new(ACCESS_POINT).unwrap(),
            STATION,
            [2, 0, 0, 0, 0, 3],
            0x1230,
            None,
            Some(QosControl {
                tid: Tid::new(5).unwrap(),
                end_of_service_period: false,
                amsdu_present: true,
                high: 0,
            }),
            None,
        )
        .and_then(AckHeader::new)
        .unwrap();
        let frame = ClassFrame::new(TxFrame::Individual {
            frame: SizedFrame::new(header, MpduLength::new(34 + body.len()).unwrap()).unwrap(),
            plan: RetryPlan::new(RetryStage {
                rate: phase.rate(PhyRate::Legacy(LegacyRate::Ofdm6M)).unwrap(),
                transmissions: NonZeroU8::MIN,
                protection: Protection::None,
            }),
        })
        .ok()
        .unwrap();
        let mut tickets = resources.tickets;
        let mut small = resources.small;
        let mut terminal_access = resources.terminal_access;
        submit_frame::<ModelSmall>(
            phase,
            tickets.remove(0),
            TxSource::Fresh {
                buffer: small.remove(0),
                frame,
                body: Some(ModelBody(body)),
                protection: TxProtection::Ccmp(&key),
            },
        );
        let _returned = completion(phase, &mut terminal_access).await;
        assert_eq!(
            phase
                .held
                .air
                .associations
                .borrow()
                .get(VifRole::AccessPoint)
                .spp(),
            ap_spp
        );
        assert_eq!(
            phase
                .held
                .air
                .associations
                .borrow()
                .get(VifRole::Station)
                .spp(),
            self.0
        );
        assert_eq!(
            phase.held.air.sent()[0].frame,
            bytes(self.1),
            "actual wire encryption uses the installed association"
        );
        let meta = ModelRxMeta::new(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap(), -40, -90);
        phase
            .held
            .air
            .receive_ccmp(&bytes(self.2), meta, VifRole::Station)
            .unwrap();
        let Ok(Ok(LowerMacEvent::Received { frame, meta })) = phase.next_event().await else {
            panic!("verified wire reception");
        };
        assert_eq!(
            meta.common.crypto,
            RxEvidence::HardwareObserved(
                oer_ieee80211_lower_mac::RxCryptoStatus::DecryptedAndIntegrityVerified
            )
        );
        let mut subframes = oer_ieee80211_mac::data::amsdu_subframes(
            oer_ieee80211_mac::data::DataInterfaceRole::Station,
            frame.bytes(),
            34,
            26,
        )
        .unwrap();
        assert_eq!(subframes.next().unwrap().unwrap().payload, b"test");
        assert!(subframes.next().is_none());
        phase.recycle(frame);
        let mut tampered = bytes(self.2);
        tampered[24] ^= 0x80;
        let result = phase
            .held
            .air
            .receive_ccmp(&tampered, meta, VifRole::Station);
        if self.0 == Spp::Negotiated {
            assert_eq!(
                result,
                Err(ModelCcmpError::Authentication(
                    oer_ieee80211_rsn::ccmp_mpdu::CcmpMpduError::InvalidMic
                ))
            );
            assert!(phase.held.air.rx.borrow().is_empty());
        } else {
            result.unwrap();
            let Ok(Ok(LowerMacEvent::Received { frame, .. })) = phase.next_event().await else {
                panic!("ordinary CCMP does not authenticate A-MSDU present");
            };
            phase.recycle(frame);
        }
        phase.station().remove_group_key(permission, group);
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(Removed::Station(
            RemovedKey::Group { slot: group, .. },
        ))))) = phase.next_event().await
        else {
            panic!("GTK removal");
        };
        phase.station().remove_pairwise_key(association, key);
        let Ok(Ok(LowerMacEvent::Terminal(LowerMacTerminal::KeyRemoved(Removed::Station(
            RemovedKey::Pairwise {
                slot: pairwise,
                association,
            },
        ))))) = phase.next_event().await
        else {
            panic!("PTK removal");
        };
        let before = phase.held.air.commands.get();
        let bank = VacantKeyBank::new([pairwise, unused_pairwise], [group, unused_group]);
        let vacant = ModelKeys::<Station>::end_association(association, bank);
        assert_eq!(
            phase.held.air.commands.get(),
            before,
            "ending enqueues no hardware command"
        );
        let (start, pairwise, _rest) = ModelKeys::<Station>::split_vacant::<2>(vacant);
        let next = ModelKeys::<Station>::begin_association(start, Spp::NotNegotiated);
        let _next_ptk = phase.station().install_pairwise_key(
            &next,
            pairwise,
            IndividualAddress::new(ACCESS_POINT).unwrap(),
            Ccmp128Key::new(&[3; 16]),
        );
    }
}

#[test]
fn installed_association_controls_wire_amsdu_and_mic_in_both_directions() {
    for &(spp, up, down) in WIRE {
        let (backend, _) = backend();
        assert!(matches!(
            block_on(run_port(backend, Body(WirePeer(spp, up, down)))),
            oer_radio_port::PortEnd::Uninstalled { .. }
        ));
    }
}
