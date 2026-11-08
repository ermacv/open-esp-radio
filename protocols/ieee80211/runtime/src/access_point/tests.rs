//! The runner over the model's port: its start order, its beacons at the
//! TBTTs, a probe's answer, and a stop that returns every owner.

use super::*;
use core::convert::Infallible;
use std::boxed::Box;
use std::rc::Rc;
use std::vec::Vec;

use embassy_futures::select::{Either, select};
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualClock;
use oer_protocols_ieee80211_contracts_port::{Channel, ChannelWidth, RegulatedChannel};
use oer_protocols_ieee80211_core_roles_ap::{
    AccessPointClientLimit, AccessPointInactiveTimeout, AccessPointPeerStorage, AccessPointService,
    beacon::ApBeacon, security_policy::ApSecurityPolicy,
};
use oer_protocols_ieee80211_core_security_rsn::frames::RsnGtk;
use oer_protocols_ieee80211_testing_model::{
    ModelAccessPointTsf, ModelCaps24, ModelContext, ModelInstall, ModelLarge, ModelPort,
    ModelRxMeta,
};
use oer_protocols_ieee80211_wire::{
    address::IndividualAddress,
    ap::profile::{Advertisement, LegacyRates, WmmParameters},
    beacon::AP_BEACON_CAPACITY,
    ccmp::GroupKeyId,
    extensions::wmm::WmmAcParameters,
    ht::HtLocalCapabilities,
    phy::LegacyRate,
    security::spp::Association,
    sequence::SequenceNumber,
    ssid::WifiSsid,
};
use oer_protocols_radio_port::{Installations, LocalId, SlotIdentity, with_local_slot};

use super::network::ApNetworkStorage;

const STATION: [u8; 6] = [0x02, 0, 0, 0, 0, 1];
const ACCESS_POINT: [u8; 6] = [0x02, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [0x02, 0, 0, 0, 0, 9];
const SSID: &[u8] = b"runner";

struct Contexts;

impl ModelContext for Contexts {
    type Body = Infallible;
    type OwnedContext<Id: SlotIdentity> = ();
    type Context<'p, Id: SlotIdentity + 'p> = ();
    type OwnedStationConfigureContext<Id: SlotIdentity> = ();
    type StationConfigureContext<'p, Id: SlotIdentity + 'p> = ();
    type OwnedAccessPointConfigureContext<Id: SlotIdentity> = ();
    type AccessPointConfigureContext<'p, Id: SlotIdentity + 'p> = ();
}

type Port<'s> = ModelPort<'s, LocalId<'s>, ModelCaps24, Contexts>;

const ADVERTISEMENT: Advertisement = match Advertisement::new(
    LegacyRates::new(
        [0x8b, 0x96, 0x82, 0x84, 0x0c, 0x18, 0x30, 0x60],
        [0x6c, 0x12, 0x24, 0x48],
    ),
    HtLocalCapabilities::new(0x100c, 0x03, 0xff, 0x01),
    match WmmParameters::new(
        4,
        false,
        [WmmAcParameters {
            admission_control_mandatory: false,
            aifsn: oer_portable_wire::U8::constant::<3>(),
            ecw_min: oer_portable_wire::U8::constant::<4>(),
            ecw_max: oer_portable_wire::U8::constant::<10>(),
            txop_limit_units_32_us: 0,
        }; 4],
    ) {
        Some(value) => value,
        None => panic!("the WMM parameters fit their fields"),
    },
    0x0421,
) {
    Some(value) => value,
    None => panic!("the advertisement leaves Privacy to the security mode"),
};

struct Nonces;

impl ApNonces for Nonces {
    fn nonce(&mut self) -> Option<[u8; 32]> {
        Some([4; 32])
    }
}

fn channel() -> Channel {
    Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap()
}

fn supported() -> SupportedChannel<ModelCaps24> {
    SupportedChannel::new(
        oer_protocols_radio_regulatory::TEST_DOMAIN
            .permit(RegulatedChannel::from(channel()))
            .unwrap(),
    )
    .unwrap()
}

fn policy() -> ApPolicy {
    ApPolicy {
        management_rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        management_transmissions: NonZeroU8::new(2).unwrap(),
        data_transmissions: NonZeroU8::new(4).unwrap(),
        packet_numbers: CcmpPacketNumberStep::ONE,
        group_generation: 1,
    }
}

fn probe_request() -> Vec<u8> {
    let mut frame = std::vec![0x40, 0, 0, 0];
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&PEER);
    frame.extend_from_slice(&[0xff; 6]);
    frame.extend_from_slice(&[0, 0]);
    frame.extend_from_slice(&[0, SSID.len() as u8]);
    frame.extend_from_slice(SSID);
    frame.extend_from_slice(&[1, 4, 0x82, 0x84, 0x8b, 0x96]);
    frame
}

/// The frame controls of the access point's frames on the air.
fn sent_controls(air: &oer_protocols_ieee80211_testing_model::ModelAir) -> Vec<u8> {
    air.sent()
        .iter()
        .filter(|frame| frame.role == oer_protocols_ieee80211_contracts_port::VifRole::AccessPoint)
        .filter_map(|frame| frame.frame.first().copied())
        .collect()
}

#[test]
fn the_runner_beacons_answers_a_probe_and_stops_with_every_owner() {
    embassy_futures::block_on(with_local_slot(async |claim| {
        let clock = Rc::new(VirtualClock::<8>::new());
        let mut installations = Installations::claim(claim);
        let (install, air) = Port::install(
            &mut installations,
            channel(),
            [
                IndividualAddress::new(STATION).unwrap(),
                IndividualAddress::new(ACCESS_POINT).unwrap(),
            ],
            clock.clone(),
        )
        .unwrap();
        let ModelInstall {
            port,
            commands,
            resources,
        } = install;
        let shared_resources = resources.shared;
        let ap = resources.access_point;
        let mut tickets = heapless::Vec::new();
        for ticket in shared_resources.tickets {
            tickets.push(ticket).ok().unwrap();
        }
        let mut buffers = heapless::Vec::new();
        for buffer in shared_resources.large {
            buffers.push(buffer).ok().unwrap();
        }
        let [pairwise_slot, spare] = ap.pairwise;
        let mut pairwise = heapless::Vec::new();
        pairwise.push(pairwise_slot).ok().unwrap();
        pairwise.push(spare).ok().unwrap();
        let [group, _spare_group] = ap.group;
        let resources = ApResources::<'_, Port<'_>, ModelLarge> {
            tickets,
            access: shared_resources.terminal_access,
            buffers,
            configure: ap.commands.configure,
            association: ap.association,
            pairwise,
            group,
            group_permission: ap.group_permission,
            beacon: ap.beacon,
        };
        let storage = Box::leak(Box::new(AccessPointPeerStorage::new()));
        let pmk = {
            let mut derivation =
                oer_protocols_ieee80211_core_security_rsn::PmkDerivation::new(b"password", SSID)
                    .unwrap();
            loop {
                if let Some(pmk) = derivation.step() {
                    break pmk;
                }
            }
        };
        let service = AccessPointService::<Association>::new(
            ACCESS_POINT,
            pmk,
            RsnGtk::new(GroupKeyId::new(GROUP_KEY_ID).unwrap(), true, &[0x55; 16]),
            1,
            AccessPointClientLimit::new(2).unwrap(),
            AccessPointInactiveTimeout::default(),
            storage.lend(),
        )
        .unwrap();
        let template = Box::leak(Box::new([0; AP_BEACON_CAPACITY]));
        let beacon = ApBeacon::new(
            template,
            &ADVERTISEMENT,
            ACCESS_POINT,
            &WifiSsid::new(SSID).unwrap(),
            channel(),
            100,
            2,
            SequenceNumber::ZERO,
            ApSecurityPolicy::Wpa2Personal,
        )
        .unwrap();
        let engine = ApEngine::new(
            service,
            beacon,
            ADVERTISEMENT,
            channel(),
            GroupKeyId::new(GROUP_KEY_ID).unwrap(),
        );
        let network = Box::leak(Box::new(ApNetworkStorage::new()));
        let (device, _transmit, end) = network.split::<Infallible>();
        let runner = ApRunner::new(
            port,
            commands,
            resources,
            engine,
            policy(),
            supported(),
            [0x55; 16],
            end,
            (),
            Nonces,
        );
        let shared = ApShared::new();
        let script = async {
            for _ in 0..64 {
                embassy_futures::yield_now().await;
            }
            // Started in order: the receive rules, the group key, the TSF.
            let interface = air.access_point();
            assert!(interface.receive.is_some());
            assert!(matches!(interface.tsf, ModelAccessPointTsf::Running { .. }));
            assert!(shared.status().running);
            assert_eq!(
                oer_protocols_network_port::NetDevice::link(&device).state,
                oer_protocols_network_port::LinkState::Up
            );
            // One beacon at the first TBTT, then one at each later one.
            for _ in 0..3 {
                clock.advance(Duration::from_micros(102_400)).unwrap();
                air.clock_advanced();
                for _ in 0..64 {
                    embassy_futures::yield_now().await;
                }
            }
            let beacons = sent_controls(&air)
                .iter()
                .filter(|control| **control == 0x80)
                .count();
            assert!(beacons >= 3, "{beacons} beacons");
            // A probe for the SSID is answered.
            air.receive(&probe_request(), ModelRxMeta::new(channel(), -40, -90));
            for _ in 0..64 {
                embassy_futures::yield_now().await;
            }
            assert!(sent_controls(&air).contains(&0x50), "a probe response");
            shared.request_stop();
            for _ in 0..1024 {
                embassy_futures::yield_now().await;
                clock.advance(Duration::from_millis(1)).unwrap();
                air.clock_advanced();
            }
        };
        let end = match select(runner.run(&shared, &*clock), script).await {
            Either::First(end) => end,
            Either::Second(()) => panic!("the runner stops"),
        };
        let runner = match end {
            ApEnd::Stopped(runner) => runner,
            ApEnd::StartRefused => panic!("the radio starts"),
            ApEnd::StopUnconfirmed(unconfirmed) => panic!("the stop is confirmed: {unconfirmed:?}"),
            ApEnd::Faulted => panic!("the port faulted"),
            ApEnd::Contract(contract) => panic!("the runner's contract: {contract:?}"),
        };
        // Every ticket and buffer is home, the interface closed, its TSF
        // stopped, the beacon permission and the group key's owners back.
        assert_eq!(
            runner.tickets.len(),
            oer_protocols_ieee80211_testing_model::MODEL_TICKETS
        );
        assert_eq!(
            runner.buffers.len(),
            oer_protocols_ieee80211_testing_model::MODEL_LARGE_BUFFERS
        );
        assert!(runner.beacon.is_some());
        assert!(runner.group_slot.is_some() && runner.group_permission.is_some());
        assert!(runner.configure.is_some());
        let interface = air.access_point();
        assert!(interface.receive.is_none());
        assert_eq!(interface.tsf, ModelAccessPointTsf::Stopped);
        assert!(!shared.status().running);
    }));
}
