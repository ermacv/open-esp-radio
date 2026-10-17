//! The station's slice beside the real arbiter, on a host port the test
//! plays as the thin service would: scan, join of an open network,
//! connected, disconnect; alone and beside an access point.

use super::*;
use crate::join::test_support::{BSSID, LOCAL, answer_authentication, association_response};
use core::num::NonZeroU16;
use oer_ieee80211_arbiter::ArbiterPool;
use oer_ieee80211_arbiter::{
    AccessPointCommand, AccessPointFact, Arbiter, ArbiterAction, ArbiterPolicy, NoReservation,
};
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, GateIssuer, HardwareServices,
    LowerMacCapabilities, PhyFormatSet, RateSupport, RxMeta, WidthSet,
};
use oer_ieee80211_mac::channel_switch::ChannelSwitchMode;
use oer_ieee80211_mac::phy::{HeMcs, HtMcs, LegacyRate, PhyRate};
use oer_radio_port::{Mint, branded};
use std::vec::Vec;

type TestPools = (
    ArbiterPool,
    oer_ieee80211_lower_mac::InterfaceCommandPool<StationRole>,
);
type TestGatePath = oer_radio_port::Child<Root, 0>;
type TestConfigurationPath = oer_radio_port::Child<Root, 1>;

mod repeated_message3;
mod restart;
mod sae;

fn installed<R>(body: impl for<'id> FnOnce(Mint<'id, TestPools>) -> R) -> R {
    let future = core::pin::pin!(branded(async |mint| body(mint)));
    let mut cx = core::task::Context::from_waker(core::task::Waker::noop());
    match core::future::Future::poll(future, &mut cx) {
        core::task::Poll::Ready(value) => value,
        core::task::Poll::Pending => panic!("this fixture performs no async operation"),
    }
}

/// A 2.4 GHz backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
        vifs: 2,
        tx_queues: 4,
        max_mpdu_length: 2304,
        tx_power_ceiling_min_dbm: Some(0),
        coex_priorities: CoexPrioritySet::only(CoexPriority::Normal),
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER
            .union(ReceiveFilter::OTHER_BSS_MANAGEMENT),
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 7,
        rx_block_ack_max_window: 64,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

/// The test's tokens: numbered, neither `Clone` nor `Copy`; a buffer is
/// its memory.
#[derive(Debug, Eq, PartialEq)]
struct Ticket(u8);

#[derive(Debug)]
struct GroupPermission;

struct Buffer([u8; 512], usize);

impl oer_ieee80211_lower_mac::TxBufferClass for Buffer {
    const CAPACITY: usize = 512;
}

impl TxBufferMemory for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0[..self.1]
    }
}

/// A key slot and an installed key: neither `Clone` nor `Copy`.
#[derive(Debug, Eq, PartialEq)]
struct Slot(u8);
#[derive(Debug, Eq, PartialEq)]
struct Key<S = Slot>(S);

#[derive(Debug, Eq, PartialEq)]
struct Pairwise(Key, oer_ieee80211_mac::security::Spp);
struct Tokens<const G: usize = 2>;

impl<const G: usize> StaTokens for Tokens<G> {
    type AssociationStart = ();
    type Association = oer_ieee80211_mac::security::Spp;
    type VacantKeyBank = (Slot, heapless::Vec<Slot, KEY_BANK_CAPACITY>);
    type VacantAssociation = (Slot, [Slot; G]);
    fn begin_association(_: (), spp: Self::Association) -> Self::Association {
        spp
    }
    fn association(key: &Pairwise) -> &Self::Association {
        &key.1
    }
    fn split_vacant(
        (pairwise, group): Self::VacantAssociation,
    ) -> (
        (),
        Slot,
        oer_ieee80211_lower_mac::association::KeyBankParts<Slot, Slot, KEY_BANK_CAPACITY>,
    ) {
        (
            (),
            pairwise,
            oer_ieee80211_lower_mac::association::KeyBankParts {
                pairwise: heapless::Vec::new(),
                group: group.into_iter().collect(),
            },
        )
    }
    fn collect_key_bank(
        parts: oer_ieee80211_lower_mac::association::KeyBankParts<Slot, Slot, KEY_BANK_CAPACITY>,
    ) -> Result<
        Self::VacantKeyBank,
        oer_ieee80211_lower_mac::association::KeyBankParts<Slot, Slot, KEY_BANK_CAPACITY>,
    > {
        if parts.pairwise.len() != 1 || parts.group.len() != G {
            return Err(parts);
        }
        let oer_ieee80211_lower_mac::association::KeyBankParts {
            mut pairwise,
            group,
        } = parts;
        Ok((pairwise.pop().unwrap(), group))
    }
    fn end_association(
        _: Self::Association,
        (pairwise, group): Self::VacantKeyBank,
    ) -> (
        (),
        Slot,
        oer_ieee80211_lower_mac::association::KeyBankParts<Slot, Slot, KEY_BANK_CAPACITY>,
    ) {
        (
            (),
            pairwise,
            oer_ieee80211_lower_mac::association::KeyBankParts {
                pairwise: heapless::Vec::new(),
                group,
            },
        )
    }
    const PAIRWISE_SLOTS: usize = 1;
    const TOTAL_GROUP_SLOTS: usize = G;
    const GROUP_SLOTS: usize = G;
    type GatePath = TestGatePath;
    type EpochPath = oer_radio_port::Root;
    type ConfigurationPath = TestConfigurationPath;
    type Small = Buffer;
    type Ticket = Ticket;
    type SmallBuffer = Buffer;
    type Sent = Buffer;
    type AckEvidence = ();
    type AirDenied = ();
    fn renew(mpdu: Self::Sent) -> Buffer {
        mpdu
    }
    type PairwiseSlot = Slot;
    type GroupSlot = Slot;
    type PairwiseKey = Pairwise;
    type GroupKey = Key<Slot>;
    type GroupPermission = GroupPermission;
    type RxMeta = RxMeta;
}

const SNONCE: [u8; 32] = [3; 32];

fn nonce() -> [u8; 32] {
    SNONCE
}

const SSID: &[u8] = b"open-net";

static CAPABILITIES: AssociationCapabilities = AssociationCapabilities {
    ht20: [0; 28],
    ht40: [0; 28],
    he20_ht: [0; 28],
    he20: [0; 24],
    he20_extended: [0; 14],
    wmm: [0; 9],
};

fn channel(number: u8) -> SupportedChannel<Caps> {
    SupportedChannel::new(Channel::ghz2_4(number, ChannelWidth::Mhz20).unwrap()).unwrap()
}

fn at(millis: u32) -> Instant {
    Instant::from_micros(u64::from(millis) * 1000)
}

/// The open network's Association Response: the join helper's, without
/// the Privacy capability.
fn open_association_response() -> [u8; 30] {
    let mut frame = association_response(0);
    frame[24..26].copy_from_slice(&0x0421_u16.to_le_bytes());
    frame
}

/// A beacon of the open network on channel 6.
fn beacon() -> [u8; 55] {
    beacon_on(6)
}

/// A beacon of the open network on the channel `number`.
fn beacon_on(number: u8) -> [u8; 55] {
    let mut frame = [0_u8; 55];
    frame[0..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&[0xff; 6]);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame[32..34].copy_from_slice(&100_u16.to_le_bytes());
    frame[34..36].copy_from_slice(&0x0001_u16.to_le_bytes());
    frame[36] = 0;
    frame[37] = SSID.len() as u8;
    frame[38..46].copy_from_slice(SSID);
    frame[46] = 1;
    frame[47] = 4;
    frame[48..52].copy_from_slice(&[0x82, 0x84, 0x8b, 0x96]);
    frame[52] = 3;
    frame[53] = 1;
    frame[54] = number;
    frame
}

/// What the port reports of a received frame: its signal and the noise
/// floor.
fn meta() -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
    meta.rssi_dbm = RxEvidence::HardwareObserved(-40);
    meta.noise_floor_dbm = RxEvidence::HardwareObserved(-95);
    meta
}

fn verified_meta() -> RxMeta {
    let mut meta = meta();
    meta.crypto = RxEvidence::HardwareObserved(
        oer_ieee80211_lower_mac::RxCryptoStatus::DecryptedAndIntegrityVerified,
    );
    meta
}

fn protected_received(mut frame: Vec<u8>, pn: u64) -> Vec<u8> {
    use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpKeyId, CcmpPacketNumber};
    frame[1] |= 0x40;
    let header = CcmpHeader::new(
        CcmpPacketNumber::new(pn).unwrap(),
        CcmpKeyId::new(0).unwrap(),
    );
    frame.splice(24..24, header.encode());
    frame
}

/// What the world saw the station and the arbiter do.
#[derive(Debug, Eq, PartialEq)]
enum Seen {
    Retune(u8),
    Configure(&'static str),
    Transmit { subtype: u16, on: u8 },
    Notify(StaNotice),
    AccessPoint(AccessPointCommand<Caps>),
    Upstream(UpstreamFact<Caps>),
    InstallPairwise([u8; 16]),
    InstallGroup(u8),
    ReplaceGroup(u8),
    SelectGroup,
    RemoveKey,
}

/// The loop the thin service runs, with the test as the port.
struct World<'id, const G: usize = 2> {
    station: Station<'id, Tokens<G>, Caps>,
    arbiter: Arbiter<'id, Caps, NoReservation, TestGatePath>,
    gate_issuer: GateIssuer<'id, TestGatePath>,
    seen: Vec<Seen>,
    completed: Vec<StaCommandTerminal>,
    /// The transmissions in flight, which the port ends in order.
    submitted: Vec<(StaTxContext<'id>, Ticket, Buffer)>,
    /// Let RX run while the current TX still owns its terminal resources.
    hold_transmissions: bool,
    /// The receive configurations the port has yet to confirm.
    configuring: Option<
        ConfigurePermission<'id, StationRole, TestConfigurationPath, StaConfigurationContext<'id>>,
    >,
    /// The configurations wait for the test's confirmation.
    hold_configured: bool,
    hold_key_installs: bool,
    pending_installs: Vec<StaInput<'id, Tokens<G>, Caps>>,
    hold_group_removals: bool,
    pending_removals: Vec<StaInput<'id, Tokens<G>, Caps>>,
    /// The outcomes of the next transmissions; delivered once none is
    /// queued.
    outcomes: std::collections::VecDeque<(Outcome, bool)>,
    /// The reason codes of the Deauthentications sent.
    deauthentications: Vec<u16>,
    /// Each EAPOL frame's Protected bit and whether it went under the key.
    eapol: Vec<(bool, bool)>,
    /// Every frame the station sent, in its buffer.
    frames: Vec<Vec<u8>>,
    /// The next frame does not go: the service hands its tokens back.
    unsend_next: bool,
    /// The frames sent before the station said it associated, and its peer.
    associated: Option<(usize, Peer<RxMeta>)>,
    /// The signal of the frames the world delivers.
    rssi: i8,
}

impl<'id> World<'id> {
    fn new(mint: Mint<'id, TestPools>, skipped_absences: u8) -> Self {
        Self::with_security(mint, skipped_absences, StaSecurity::Open)
    }

    fn with_security(
        mint: Mint<'id, TestPools>,
        skipped_absences: u8,
        security: StaSecurity,
    ) -> Self {
        Self::with_groups(mint, skipped_absences, security, [Slot(1), Slot(2)])
    }

    fn with_groups<const G: usize>(
        mint: Mint<'id, TestPools>,
        skipped_absences: u8,
        security: StaSecurity,
        group_slots: [Slot; G],
    ) -> World<'id, G> {
        Self::with_pools::<2, G>(mint, skipped_absences, security, group_slots)
    }

    fn with_pools<const TX: usize, const G: usize>(
        mint: Mint<'id, TestPools>,
        skipped_absences: u8,
        security: StaSecurity,
        group_slots: [Slot; G],
    ) -> World<'id, G> {
        Self::with_builder::<TX, G>(mint, skipped_absences, group_slots, |config, resources| {
            match security {
                StaSecurity::Open => Station::new_open(config, resources),
                StaSecurity::Personal { password, pmk } => {
                    Station::new_personal(config, password, pmk, resources)
                }
            }
        })
    }

    fn with_builder<const TX: usize, const G: usize>(
        mint: Mint<'id, TestPools>,
        skipped_absences: u8,
        group_slots: [Slot; G],
        build: impl FnOnce(
            StaConfig<Caps>,
            StationResources<'id, Tokens<G>, TX, TX, G>,
        ) -> Station<'id, Tokens<G>, Caps>,
    ) -> World<'id, G> {
        let channels = heapless::Vec::from_slice(&[channel(1), channel(6)]).unwrap();
        let ((gate, gate_issuer, authority), permissions) = mint.into_pools();
        let (arbiter, place, absence) = Arbiter::new(
            ArbiterPolicy::DEFAULT.checked(Duration::ZERO).unwrap(),
            channel(1),
            authority,
            gate,
        );
        let config = StaConfig {
            address: oer_ieee80211_mac::address::OwnAddress::new(
                oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap(),
            ),
            ssid: heapless::Vec::from_slice(SSID).unwrap(),
            channels,
            dwell: Duration::from_millis(10),
            skipped_absences,
            access_point_margin_db: 6,
            supported_rates: [0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24],
            association: &CAPABILITIES,
            listen_interval: StationListenInterval::new(10).unwrap(),
            phy: PhyMode::Legacy,
            nonce,
            he_packet_padding: |_| oer_ieee80211_upper_mac::HePacketPadding::None,
        };
        let resources = StationResources {
            configure: permissions.configure,
            epochs: LinkEpochSource::for_test(0),
            tickets: core::array::from_fn::<_, TX, _>(|index| Ticket(u8::try_from(index).unwrap())),
            buffers: core::array::from_fn::<_, TX, _>(|_| Buffer([0; 512], 512)),
            place,
            absence_place: absence,
            keys: (Slot(0), group_slots),
            group_permission: GroupPermission,
        };
        let station = build(config, resources);
        World {
            station,
            arbiter,
            gate_issuer,
            seen: Vec::new(),
            completed: Vec::new(),
            submitted: Vec::new(),
            configuring: None,
            hold_configured: false,
            hold_key_installs: false,
            pending_installs: Vec::new(),
            hold_group_removals: false,
            pending_removals: Vec::new(),
            outcomes: std::collections::VecDeque::new(),
            hold_transmissions: false,
            deauthentications: Vec::new(),
            eapol: Vec::new(),
            frames: Vec::new(),
            unsend_next: false,
            associated: None,
            rssi: i8::MIN,
        }
    }
}

impl<'id, const G: usize> World<'id, G> {
    /// Run the loop at `now` until neither machine owes an action; the
    /// port confirms configurations and ends transmissions at once.
    fn run(&mut self, now: Instant) {
        loop {
            let mut acted = false;
            while let Some(action) = self.station.poll_action(now) {
                acted = true;
                match action {
                    StaAction::RequestChannel(request, place) => {
                        let answer = self.arbiter.request(now, request, place);
                        self.station.input(now, StaInput::Answer(answer));
                    }
                    StaAction::ReleaseChannel(lease) => self.arbiter.release(lease),
                    StaAction::RequestAbsence {
                        request,
                        home,
                        place,
                    } => {
                        let answer = self.arbiter.request_absence(now, request, home, place);
                        self.station.input(now, StaInput::AbsenceAnswer(answer));
                    }
                    StaAction::ContractViolation(reason) => {
                        panic!("unexpected contract violation: {reason:?}")
                    }
                    StaAction::Ready(id) => self.arbiter.ready(id),
                    StaAction::InstallPairwiseKey {
                        association,
                        pending,
                        slot,
                        key,
                        ..
                    } => {
                        self.seen.push(Seen::InstallPairwise(*key.as_bytes()));
                        self.installed(
                            now,
                            StaInput::PairwiseKeyInstalled {
                                pending,
                                key: Pairwise(Key(slot), association),
                            },
                        );
                    }
                    StaAction::InstallGroupKey {
                        permission,
                        pending,
                        slot,
                        key_id,
                        ..
                    } => {
                        self.seen.push(Seen::InstallGroup(key_id.get()));
                        self.installed(
                            now,
                            StaInput::GroupKeyInstalled {
                                permission,
                                key: Key(slot),
                                pending,
                            },
                        );
                    }
                    StaAction::RemovePairwiseKey(Pairwise(Key(slot), association)) => {
                        self.seen.push(Seen::RemoveKey);
                        self.station
                            .input(now, StaInput::PairwiseKeyRemoved { slot, association });
                    }
                    StaAction::RemoveGroupKey {
                        pending,
                        key: Key(slot),
                        permission,
                    } => {
                        self.seen.push(Seen::RemoveKey);
                        let removed = StaInput::GroupKeyRemoved {
                            pending,
                            slot,
                            permission,
                        };
                        if self.hold_group_removals {
                            self.pending_removals.push(removed);
                        } else {
                            self.station.input(now, removed);
                        }
                    }
                    StaAction::SelectGroupKey {
                        pending,
                        installed,
                        permission,
                    } => {
                        self.seen.push(Seen::SelectGroup);
                        self.installed(
                            now,
                            StaInput::GroupKeySelected {
                                pending,
                                key: installed,
                                permission,
                            },
                        );
                    }
                    StaAction::ReplaceGroupKey {
                        permission,
                        pending,
                        installed,
                        key_id,
                        ..
                    } => {
                        self.seen.push(Seen::ReplaceGroup(key_id.get()));
                        self.installed(
                            now,
                            StaInput::GroupKeyReplaced {
                                permission,
                                pending,
                                key: installed,
                            },
                        );
                    }
                    StaAction::ExchangeLease(old) => {
                        let new = self.arbiter.exchange(old);
                        self.station.input(now, StaInput::Exchanged(new));
                    }
                    StaAction::Upstream(fact) => {
                        self.seen.push(Seen::Upstream(fact));
                        self.arbiter.upstream(now, fact);
                    }
                    StaAction::Configure(receive, pending) => {
                        self.seen.push(Seen::Configure(match receive {
                            StationReceive::Off => "off",
                            StationReceive::OtherBssManagement => "scan",
                            StationReceive::Bss { filter, .. }
                                if filter.get() == ReceiveFilter::BSS_MEMBER =>
                            {
                                "bss"
                            }
                            StationReceive::Bss { .. } => "bss+scan",
                        }));
                        assert!(self.configuring.replace(pending).is_none());
                    }
                    StaAction::Transmit(transmit) => self.transmitted(transmit, false),
                    // A frame under the key goes as any other; the world
                    // notes the key.
                    StaAction::TransmitProtected { transmit, .. } => {
                        self.transmitted(transmit, true);
                    }
                    StaAction::Notify(notice) => self.seen.push(Seen::Notify(notice)),
                    StaAction::CommandCompleted(terminal) => self.completed.push(terminal),
                    // The frames sent before the station associated.
                    StaAction::Associated(peer) => {
                        self.associated = Some((self.frames.len(), peer))
                    }
                }
            }
            while let Some(action) = self.arbiter.poll_action(now) {
                acted = true;
                match action {
                    ArbiterAction::Station(notice) => {
                        self.station.input(now, StaInput::Arbiter(notice));
                    }
                    ArbiterAction::AccessPoint(command) => {
                        self.seen.push(Seen::AccessPoint(command));
                    }
                    // The gate closes once every transmission ended.
                    ArbiterAction::CloseGate { gate, .. } => {
                        self.end_transmissions(now);
                        self.arbiter.gate_closed(self.gate_issuer.close(gate));
                    }
                    ArbiterAction::Retune(to) => {
                        self.seen
                            .push(Seen::Retune(to.channel().channel().number()));
                        self.arbiter.retuned(to);
                    }
                    ArbiterAction::OpenGate(closed) => {
                        self.arbiter.gate_opened(self.gate_issuer.open(closed))
                    }
                }
            }
            if !self.hold_configured
                && let Some(pending) = self.configuring.take()
            {
                self.station.input(now, StaInput::Configured(pending));
                acted = true;
            }
            if !self.hold_transmissions && !self.submitted.is_empty() {
                self.end_transmissions(now);
                acted = true;
            }
            if !acted {
                return;
            }
        }
    }

    fn installed(&mut self, now: Instant, input: StaInput<'id, Tokens<G>, Caps>) {
        if self.hold_key_installs {
            self.pending_installs.push(input);
        } else {
            self.station.input(now, input);
        }
    }

    fn end_transmissions(&mut self, now: Instant) {
        for (context, ticket, buffer) in self.submitted.drain(..).collect::<Vec<_>>() {
            // The service did not send it: its tokens come back unsent.
            if core::mem::take(&mut self.unsend_next) {
                self.station.input(
                    now,
                    StaInput::Unsent {
                        reason: crate::machine::TxRefusal::NoRate,
                        context,
                        ticket,
                        buffer: ReturnedMpdu::Fresh(buffer),
                    },
                );
                continue;
            }
            let (completion, published) = self
                .outcomes
                .pop_front()
                .unwrap_or((ended(End::Delivered), true));
            self.station.input(
                now,
                StaInput::TxDone {
                    context,
                    ticket,
                    buffer: if published {
                        ReturnedMpdu::Sent(buffer)
                    } else {
                        ReturnedMpdu::Fresh(buffer)
                    },
                    completion,
                },
            );
        }
    }

    /// One frame the station sent: the world records it and holds its
    /// tokens.
    fn transmitted(&mut self, mut transmit: StaTransmit<'id, Tokens<G>, Caps>, keyed: bool) {
        let frame = transmit.buffer.bytes_mut();
        // A Null's PM bit tells its power state apart.
        let subtype = u16::from_le_bytes([frame[0], frame[1]]) & 0x10fc;
        // Authentication, Association and EAPOL are the
        // connection's frames.
        // A protected frame carries the CCMP header's room,
        // a QoS one its QoS Control field.
        let protected = frame[1] & 0x40 != 0;
        assert_eq!(
            protected, keyed,
            "encoded protection and actual key selection agree"
        );
        let qos = subtype & 0x0080 != 0;
        let ether_type = 30 + usize::from(qos) * 2 + usize::from(protected) * 8;
        let eapol = subtype & 0x000c == 0x0008 && frame[ether_type..ether_type + 2] == [0x88, 0x8e];
        if eapol {
            self.eapol.push((protected, keyed));
        }
        let length = match &transmit.frame {
            StaFrame::Individual(frame) => frame.frame().len().get(),
            StaFrame::Group(frame) => frame.frame().len().get(),
        };
        self.frames.push(frame[..length].to_vec());
        let connection = matches!(subtype, 0x00b0 | 0x0000) || eapol;
        assert_eq!(
            transmit.class,
            if connection {
                StaFrameClass::Connection
            } else {
                StaFrameClass::Normal
            }
        );
        if subtype == 0x00c0 {
            let body = 24 + usize::from(protected) * 8;
            self.deauthentications
                .push(u16::from_le_bytes([frame[body], frame[body + 1]]));
        }
        let on = self.arbiter.channel().map_or(0, |c| c.channel().number());
        self.seen.push(Seen::Transmit { subtype, on });
        self.submitted
            .push((transmit.context, transmit.ticket, transmit.buffer));
    }

    fn answer_authentication(&mut self, now: Instant, status: u16) {
        let response = self
            .frames
            .iter()
            .rev()
            .find(|frame| frame[0] & 0xfc == 0xb0)
            .and_then(|request| answer_authentication(request, status))
            .expect("the test AP requires a valid wire Authentication Request");
        self.receive(now, &response);
    }

    fn receive(&mut self, now: Instant, frame: &[u8]) {
        let mut meta = meta();
        if self.rssi != i8::MIN {
            meta.rssi_dbm = RxEvidence::HardwareObserved(self.rssi);
        }
        let _ = self.station.receive(now, frame, meta, |_| {});
        self.run(now);
    }

    fn receive_verified(&mut self, now: Instant, frame: &[u8]) {
        self.station
            .receive(now, frame, verified_meta(), |_| {})
            .unwrap();
        self.run(now);
    }

    fn take(&mut self) -> Vec<Seen> {
        core::mem::take(&mut self.seen)
    }

    /// Run until the next deadline of either machine.
    fn advance(&mut self) -> Instant {
        let next = [self.station.poll_deadline(), self.arbiter.poll_deadline()]
            .into_iter()
            .flatten()
            .min()
            .expect("a machine waits for a deadline");
        self.station.input(next, StaInput::Tick);
        self.run(next);
        next
    }
}

#[test]
fn a_station_alone_scans_joins_an_open_network_and_leaves_it() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        // Channel 1, where the port is: the station listens and probes.
        assert_eq!(
            world.take(),
            [
                Seen::Configure("scan"),
                Seen::Transmit {
                    subtype: 0x0040,
                    on: 1
                },
            ]
        );
        // The lease ends after the dwell; channel 6 is next, where the beacon
        // arrives.
        world.advance();
        assert_eq!(
            world.take(),
            [
                Seen::Retune(6),
                Seen::Configure("scan"),
                Seen::Transmit {
                    subtype: 0x0040,
                    on: 6
                },
            ]
        );
        let now = at(13);
        world.receive(now, &beacon());
        // The scan ends; the join leases channel 6 and authenticates.
        world.advance();
        assert_eq!(
            world.take(),
            [
                Seen::Configure("bss"),
                Seen::Transmit {
                    subtype: 0x00b0,
                    on: 6
                },
            ]
        );
        world.answer_authentication(at(30), 0);
        assert_eq!(
            world.take(),
            [Seen::Transmit {
                subtype: 0x0000,
                on: 6
            }]
        );
        assert!(world.station.peer().is_none());
        // The Association Response, 55 dB over the noise floor.
        let mut meta = meta();
        meta.rssi_dbm = RxEvidence::HardwareObserved(-40);
        meta.noise_floor_dbm = RxEvidence::HardwareObserved(-95);
        let _ = world
            .station
            .receive(at(31), &open_association_response(), meta, |_| {});
        world.run(at(31));
        assert_eq!(
            world.take(),
            [
                Seen::Notify(StaNotice::Connected {
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap()
                    )
                    .unwrap(),
                    channel: 6
                }),
                Seen::Upstream(UpstreamFact::Connected(channel(6))),
            ]
        );
        assert!(world.station.is_connected());
        // What a rate selection starts from: the PHY the association made and
        // the metadata its response arrived with.
        let peer = world.station.peer().unwrap();
        assert_eq!(peer.associated.phy, PhyMode::Legacy);
        assert_eq!(peer.response, meta);

        // The application leaves: Deauthentication, receive off, the lease
        // back, then the report.
        world
            .station
            .command(at(40), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(40));
        assert_eq!(
            world.take(),
            [
                Seen::Transmit {
                    subtype: 0x00c0,
                    on: 6
                },
                Seen::Configure("off"),
                Seen::Notify(StaNotice::Disconnected(StaDisconnectReason::Requested)),
            ]
        );
        // Every token came back.
        assert!(world.submitted.is_empty());
        assert!(world.station.place.is_some());
    });
}

#[test]
fn refused_background_absence_returns_the_original_request_and_exact_reason() {
    use oer_ieee80211_arbiter::AbsenceRefusal;
    for reason in [
        AbsenceRefusal::ScheduleExhausted,
        AbsenceRefusal::NotConnected,
        AbsenceRefusal::PrimaryEnds { at: at(99) },
        AbsenceRefusal::IdentityExhausted,
    ] {
        installed(|mint| {
            let mut world = connected_world(mint);
            world.completed.clear();
            world
                .station
                .command(at(50), StaRequest::new(42, StaCommand::BackgroundScan))
                .unwrap();
            let rejected = world
                .station
                .command(at(50), StaRequest::new(43, StaCommand::BackgroundScan))
                .unwrap_err();
            assert_eq!(rejected.request.correlation(), 43);
            assert_eq!(rejected.reason, CommandRefusal::Busy);
            let Some(StaAction::RequestAbsence { place, .. }) = world.station.poll_action(at(50))
            else {
                panic!()
            };
            world.station.input(
                at(50),
                StaInput::AbsenceAnswer(AbsenceAnswer::Refused(reason, place)),
            );
            let rejected = world
                .station
                .command(at(50), StaRequest::new(44, StaCommand::BackgroundScan))
                .unwrap_err();
            assert_eq!(rejected.reason, CommandRefusal::TerminalPending);
            assert_eq!(rejected.request.correlation(), 44);
            // A second kind of command cannot overwrite the scan's terminal.
            world
                .station
                .command(at(50), StaRequest::new(45, StaCommand::Disconnect))
                .unwrap();
            world.run(at(50));
            assert_eq!(
                world.completed,
                [
                    StaCommandTerminal {
                        request: StaRequest::new(42, StaCommand::BackgroundScan),
                        outcome: StaCommandOutcome::BackgroundScan {
                            interrupted: Some(BackgroundScanEnd::AbsenceRefused(reason))
                        },
                    },
                    StaCommandTerminal {
                        request: StaRequest::new(45, StaCommand::Disconnect),
                        outcome: StaCommandOutcome::Disconnected(StaDisconnectReason::Requested),
                    },
                ]
            );
            assert!(world.station.absence_place.is_some());
            assert!(world.station.poll_action(at(50)).is_none());
            assert!(world.station.poll_deadline().is_none());
        });
    }
}

#[test]
fn absence_schedule_exhaustion_completes_the_accepted_background_scan() {
    use oer_ieee80211_arbiter::AbsenceRefusal;
    installed(|mint| {
        let mut world = connected_world(mint);
        // The dwell itself fits, but the arbiter's preparation and return do not.
        let now = Instant::from_micros(u64::MAX - world.station.config.dwell.as_micros());
        world
            .station
            .command(now, StaRequest::new(61, StaCommand::BackgroundScan))
            .unwrap();
        let Some(StaAction::RequestAbsence {
            request,
            home,
            place,
        }) = world.station.poll_action(now)
        else {
            panic!()
        };
        let answer = world.arbiter.request_absence(now, request, home, place);
        assert!(matches!(
            answer,
            AbsenceAnswer::Refused(AbsenceRefusal::ScheduleExhausted, _)
        ));
        world.station.input(now, StaInput::AbsenceAnswer(answer));
        let Some(StaAction::CommandCompleted(terminal)) = world.station.poll_action(now) else {
            panic!()
        };
        assert_eq!(terminal.request.correlation(), 61);
        assert_eq!(
            terminal.outcome,
            StaCommandOutcome::BackgroundScan {
                interrupted: Some(BackgroundScanEnd::AbsenceRefused(
                    AbsenceRefusal::ScheduleExhausted
                ))
            }
        );
        assert!(world.station.is_connected());
        assert!(world.station.poll_action(now).is_none());
        assert!(world.station.poll_deadline().is_none());
    });
}

#[test]
fn inappropriate_commands_return_without_changing_the_running_operation() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        for command in [StaCommand::BackgroundScan, StaCommand::Disconnect] {
            let refused = world
                .station
                .command(at(0), StaRequest::new(71, command))
                .unwrap_err();
            assert_eq!(refused.request.command(), command);
            assert_eq!(refused.reason, CommandRefusal::NotConnected);
        }
        world
            .station
            .command(at(0), StaRequest::new(72, StaCommand::Connect))
            .unwrap();
        let refused = world
            .station
            .command(at(0), StaRequest::new(73, StaCommand::Connect))
            .unwrap_err();
        assert_eq!(refused.request.correlation(), 73);
        assert_eq!(refused.reason, CommandRefusal::Transitioning);
        assert!(matches!(
            world.station.poll_action(at(0)),
            Some(StaAction::RequestChannel(..))
        ));
        assert!(world.completed.is_empty());
    });
}

#[test]
fn beside_an_access_point_the_scan_goes_in_absences_after_its_tbtts() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world.arbiter.access_point(
            at(0),
            AccessPointFact::Running {
                channel: channel(1),
                next_tbtt: at(50),
                beacon_interval: oer_ieee80211_arbiter::coordinator::BeaconInterval::new(
                    Duration::from_millis(100),
                )
                .unwrap(),
            },
        );
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        // Channel 1 is the access point's: the station scans it at once.
        assert_eq!(
            world.take(),
            [
                Seen::Configure("scan"),
                Seen::Transmit {
                    subtype: 0x0040,
                    on: 1
                },
            ]
        );
        // Channel 6 only in the absence after the TBTT at 50 ms: the port
        // moves at 52 ms, and the station probes there.
        let ended = world.advance();
        assert_eq!(ended, at(12));
        assert_eq!(world.take(), []);
        let started = world.advance();
        assert_eq!(started, at(52));
        assert_eq!(
            world.take(),
            [
                Seen::Retune(6),
                Seen::Configure("scan"),
                Seen::Transmit {
                    subtype: 0x0040,
                    on: 6
                },
            ]
        );
        // The absence ends before the next beacon: the port is back home.
        let back = world.advance();
        assert_eq!(back, at(64));
        assert_eq!(world.take().first(), Some(&Seen::Retune(1)));
    });
}

#[test]
fn a_scan_ends_with_a_partial_result_after_skipped_absences() {
    installed(|mint| {
        let mut world = World::new(mint, 2);
        world.arbiter.access_point(
            at(0),
            AccessPointFact::Running {
                channel: channel(1),
                next_tbtt: at(50),
                beacon_interval: oer_ieee80211_arbiter::coordinator::BeaconInterval::new(
                    Duration::from_millis(100),
                )
                .unwrap(),
            },
        );
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        world.advance();
        world.take();
        // The access point stops before the absence, which cancels it, and
        // runs again before the station asks anew: twice in a row.
        for _ in 0..2 {
            world.arbiter.access_point(at(20), AccessPointFact::Stopped);
            world.arbiter.access_point(
                at(20),
                AccessPointFact::Running {
                    channel: channel(1),
                    next_tbtt: at(50),
                    beacon_interval: oer_ieee80211_arbiter::coordinator::BeaconInterval::new(
                        Duration::from_millis(100),
                    )
                    .unwrap(),
                },
            );
            world.run(at(20));
        }
        assert!(world.take().contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::NotFound { partial: true }
        ))));
    });
}

/// The access point beside the station runs on channel 1.
fn access_point_on_1(world: &mut World<'_>) {
    world.arbiter.access_point(
        at(0),
        AccessPointFact::Running {
            channel: channel(1),
            next_tbtt: at(50),
            beacon_interval: oer_ieee80211_arbiter::coordinator::BeaconInterval::new(
                Duration::from_millis(100),
            )
            .unwrap(),
        },
    );
}

/// Advance until the world saw what `seen` matches; the instant then.
fn advance_until(world: &mut World<'_>, seen: impl Fn(&Seen) -> bool) -> Instant {
    for _ in 0..100 {
        let now = world.advance();
        if world.seen.iter().any(&seen) {
            return now;
        }
    }
    panic!("never seen: {:?}", world.seen);
}

fn announces(seen: &Seen) -> bool {
    matches!(
        seen,
        Seen::AccessPoint(AccessPointCommand::AnnounceSwitch { .. })
    )
}

#[test]
fn beside_an_access_point_a_join_elsewhere_moves_it_and_one_too_soon_after_waits() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        access_point_on_1(&mut world);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        // Channel 6 in the absence at 52 ms, where the network's beacon is.
        world.advance();
        world.advance();
        world.receive(at(53), &beacon_on(6));
        // The join waits for the access point's announced move.
        let asked = advance_until(&mut world, announces);
        assert!(
            world
                .take()
                .contains(&Seen::AccessPoint(AccessPointCommand::AnnounceSwitch {
                    target: channel(6),
                    mode: ChannelSwitchMode::Continue,
                    count: 5,
                }))
        );
        assert_eq!(world.arbiter.channel(), Some(channel(1)));
        // The switch is due: the port moves, and the join authenticates there.
        world
            .arbiter
            .access_point(at(650), AccessPointFact::SwitchDue(channel(6)));
        world.run(at(650));
        assert_eq!(
            world.take(),
            [
                Seen::Retune(6),
                Seen::Configure("bss"),
                Seen::Transmit {
                    subtype: 0x00b0,
                    on: 6
                },
            ]
        );
        // The join fails; the access point stays where it moved.
        world.answer_authentication(at(660), 1);
        assert!(world.take().contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::AuthenticationFailed
        ))));
        assert_eq!(world.arbiter.channel(), Some(channel(6)));

        // The network is now on channel 1, where the access point was: the
        // join there is refused within the follow interval, and the station
        // asks again once it passed.
        world
            .station
            .command(at(700), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(700));
        advance_until(&mut world, |seen| *seen == Seen::Retune(1));
        let now = world.arbiter.poll_deadline().unwrap_or(at(760));
        world.receive(now, &beacon_on(1));
        let retried = advance_until(&mut world, announces);
        assert!(retried >= asked.saturating_add(Duration::from_secs(30)));
        assert_eq!(world.arbiter.channel(), Some(channel(6)));
        assert!(
            world
                .take()
                .contains(&Seen::AccessPoint(AccessPointCommand::AnnounceSwitch {
                    target: channel(1),
                    mode: ChannelSwitchMode::Continue,
                    count: 5,
                }))
        );
    });
}

/// The Null data frame's subtype in the seen frames, with and without PM.
const NULL_ACTIVE: u16 = 0x0048;
const NULL_POWER_SAVE: u16 = 0x1048;

/// A world whose station is connected to the open network on channel 6,
/// alone on the port.
fn connected_world<'id>(mint: Mint<'id, TestPools>) -> World<'id> {
    let world = World::new(mint, 3);
    connect_open_world(world)
}

#[test]
fn refused_lease_exchange_releases_the_original_lease_and_resources() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let Held::Lease(original) = core::mem::replace(&mut world.station.lease, Held::None) else {
            panic!("connected station owns its lease")
        };
        // No move occurred: the public arbiter API returns the same owner in
        // Err. Feed that refusal through the actual station cleanup path.
        let refused = world.arbiter.exchange(original);
        assert!(refused.is_err());
        world.station.input(at(40), StaInput::Exchanged(refused));
        world.run(at(40));
        let events = world.take();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::LeaseExchangeRefused
                    ))
                ))
                .count(),
            1
        );
        assert!(matches!(world.station.state, State::Idle));
        assert!(world.station.place.is_some());
        assert!(world.station.poll_action(at(40)).is_none());
        assert!(world.station.tokens().is_some());
        assert!(world.station.tokens().is_some());
        assert!(world.station.tokens().is_none());
    });
}

fn connect_open_world<const G: usize>(mut world: World<'_, G>) -> World<'_, G> {
    world
        .station
        .command(at(0), StaRequest::new(1, StaCommand::Connect))
        .unwrap();
    world.run(at(0));
    world.advance();
    world.receive(at(13), &beacon());
    world.advance();
    world.answer_authentication(at(30), 0);
    world.receive(at(31), &open_association_response());
    assert!(world.station.is_connected());
    world.take();
    world
}

#[test]
fn association_advertises_the_configured_listen_interval() {
    for beacon_intervals in [1, 13, u16::MAX] {
        installed(|mint| {
            let mut world = World::new(mint, 3);
            world.station.config.listen_interval =
                StationListenInterval::new(beacon_intervals).unwrap();
            let mut world = connect_open_world(world);
            let requests = world
                .frames
                .iter()
                .filter(|frame| frame[0] & 0xfc == 0x00)
                .collect::<Vec<_>>();
            assert_eq!(requests.len(), 1);
            assert_eq!(
                u16::from_le_bytes(requests[0][26..28].try_into().unwrap()),
                beacon_intervals
            );
            assert!(
                world.completed.iter().any(|terminal| matches!(
                    terminal.outcome,
                    StaCommandOutcome::Connected { .. }
                ))
            );
            world
                .station
                .command(at(40), StaRequest::new(2, StaCommand::Disconnect))
                .unwrap();
            world.run(at(40));
            assert!(matches!(world.station.state, State::Idle));
            assert!(world.station.tokens().is_some());
            assert!(world.station.tokens().is_some());
            assert!(world.station.tokens().is_none());
        });
    }
}

#[test]
fn scan_without_a_ds_element_uses_the_received_channel_configuration() {
    fn receive_observation(world: &mut World<'_>, now: Instant, received_channel: u8) {
        let mut frame = beacon().to_vec();
        // This BSS omits DS Parameter Set. Its receive metadata still carries
        // the checked channel configured when this exact frame was captured.
        frame.truncate(52);
        let other_bssid = [0x02, 0, 0, 0, 0, received_channel];
        frame[10..16].copy_from_slice(&other_bssid);
        frame[16..22].copy_from_slice(&other_bssid);
        let mut metadata = meta();
        metadata.channel = channel(received_channel).channel();
        world
            .station
            .receive(now, &frame, metadata, |_| {})
            .unwrap();
        let record = world
            .station
            .scan
            .records()
            .find(|record| record.bssid.get() == other_bssid)
            .unwrap();
        assert_eq!(record.channel, received_channel);
    }
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        // A delayed observation can outlive the lease that captured it.
        receive_observation(&mut world, at(1), 6);
    });
    installed(|mint| {
        let mut world = connected_world(mint);
        world
            .station
            .command(at(40), StaRequest::new(3, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(40));
        let away = world.advance();
        receive_observation(&mut world, away, 11);
        let home = world.advance();
        receive_observation(&mut world, home, 1);
    });
}

#[test]
fn a_null_encoding_failure_leaves_once_and_returns_the_credit() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let mut buffers = Vec::new();
        while let Some(mut buffer) = world.station.buffers.take() {
            buffer.1 = 1;
            buffers.push(buffer);
        }
        for buffer in buffers {
            world.station.buffers.put(buffer);
        }
        let State::Connected(connected) = &mut world.station.state else {
            panic!()
        };
        connected.wake = true;
        world.run(at(40));
        assert!(!world.station.is_connected());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        assert_eq!(
            world
                .seen
                .iter()
                .filter(|event| matches!(
                    event,
                    Seen::Notify(StaNotice::Disconnected(StaDisconnectReason::Encode))
                ))
                .count(),
            1
        );
        assert!(world.station.poll_action(at(41)).is_none());
        assert_eq!(world.station.poll_deadline(), None);
    });
}

#[test]
fn a_terminal_of_the_previous_connection_only_returns_its_resources() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let old = world
            .station
            .encode_null(
                oer_ieee80211_mac::address::Bssid::new(
                    oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                )
                .unwrap(),
                StaPowerManagement::Active,
            )
            .unwrap();
        world
            .station
            .command(at(40), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(40));
        world
            .station
            .command(at(50), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(50));
        world.advance();
        world.receive(at(63), &beacon());
        world.advance();
        world.answer_authentication(at(80), 0);
        world.receive(at(81), &open_association_response());
        assert!(world.station.is_connected());
        let State::Connected(connected) = &mut world.station.state else {
            unreachable!()
        };
        connected.wake = true;
        let Some(StaAction::Transmit(current)) = world.station.poll_action(at(82)) else {
            panic!("the new connection owes its own wake Null");
        };
        assert!(!world.station.is_current(&old.context));
        assert!(world.station.is_current(&current.context));
        world.station.input(
            at(83),
            StaInput::TxDone {
                context: old.context,
                ticket: old.ticket,
                buffer: ReturnedMpdu::Sent(old.buffer),
                completion: ended(End::Delivered),
            },
        );
        let State::Connected(connected) = &world.station.state else {
            unreachable!()
        };
        assert!(
            connected.sending && connected.waking,
            "the new Null is still outstanding"
        );
        world.station.input(
            at(84),
            StaInput::Unsent {
                reason: crate::machine::TxRefusal::NoRate,
                context: current.context,
                ticket: current.ticket,
                buffer: ReturnedMpdu::Fresh(current.buffer),
            },
        );
        let State::Connected(connected) = &world.station.state else {
            unreachable!()
        };
        assert!(!connected.sending && !connected.waking);
        assert!(world.station.tokens().is_some());
        assert!(world.station.tokens().is_some());
        assert!(world.station.tokens().is_none());
        assert_eq!(world.station.lost_tokens(), 0);
    });
}

#[test]
fn a_connected_station_scans_in_an_absence_announced_with_nulls() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world
            .station
            .command(at(40), StaRequest::new(3, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(40));
        // PM=1 first; the absence waits for its readiness and the window.
        assert_eq!(
            world.take(),
            [Seen::Transmit {
                subtype: NULL_POWER_SAVE,
                on: 6
            }]
        );
        // Its start: the port on channel 1, a scan there; then home, the
        // network's receive rules and PM=0.
        world.advance();
        assert_eq!(
            world.take(),
            [
                Seen::Retune(1),
                Seen::Configure("scan"),
                Seen::Transmit {
                    subtype: 0x0040,
                    on: 1
                },
            ]
        );
        world.advance();
        assert_eq!(
            world.take(),
            [
                Seen::Retune(6),
                Seen::Configure("bss"),
                Seen::Transmit {
                    subtype: NULL_ACTIVE,
                    on: 6
                },
                // Channel 6 next, the network's own: no absence, the receive
                // rules widened to the other networks, and a probe.
                Seen::Configure("bss+scan"),
                Seen::Transmit {
                    subtype: 0x0040,
                    on: 6
                },
            ]
        );
        // The dwell ends: the network's rules again, and the scan ends only
        // once the port confirmed them.
        world.hold_configured = true;
        let now = world.advance();
        assert_eq!(world.take(), [Seen::Configure("bss")]);
        assert!(world.station.commands.poll().is_none());
        world.hold_configured = false;
        world.run(now);
        assert!(world.take().is_empty());
        assert_eq!(
            world.completed.last().unwrap(),
            &StaCommandTerminal {
                request: StaRequest::new(3, StaCommand::BackgroundScan),
                outcome: StaCommandOutcome::BackgroundScan { interrupted: None },
            }
        );
    });
}

#[test]
fn an_undelivered_sleep_null_cancels_the_absence_and_the_station_stays() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world.outcomes.push_back((ended(End::AckTimeout), true));
        world
            .station
            .command(at(40), StaRequest::new(3, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(40));
        world.take();
        // No readiness: the arbiter cancels the absence at its start; the port
        // stays on channel 6, and the station wakes its access point again.
        world.advance();
        let seen = world.take();
        assert!(!seen.iter().any(|seen| matches!(seen, Seen::Retune(_))));
        assert!(seen.contains(&Seen::Transmit {
            subtype: NULL_ACTIVE,
            on: 6
        }));
    });
}

#[test]
fn an_unsent_sleep_null_is_undelivered_and_the_station_stays() {
    installed(|mint| {
        let mut world = connected_world(mint);
        // The service cannot send the Null (no rate of its plan on the
        // channel): the station hears it as undelivered.
        world.unsend_next = true;
        world
            .station
            .command(at(40), StaRequest::new(3, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(40));
        world.take();
        world.advance();
        let seen = world.take();
        assert!(!seen.iter().any(|seen| matches!(seen, Seen::Retune(_))));
        assert!(seen.contains(&Seen::Transmit {
            subtype: NULL_ACTIVE,
            on: 6
        }));
        assert!(world.submitted.is_empty());
    });
}

#[test]
fn an_undelivered_wake_null_is_sent_again_when_the_tim_shows_buffered_frames() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world
            .station
            .command(at(40), StaRequest::new(3, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(40));
        // Channel 1's absence; back home, the PM=0 Null is not delivered.
        world.advance();
        world.outcomes.push_back((ended(End::AckTimeout), true));
        world.advance();
        // Channel 6, the network's own, without an absence.
        world.advance();
        assert_eq!(
            world.completed.last(),
            Some(&StaCommandTerminal {
                request: StaRequest::new(3, StaCommand::BackgroundScan),
                outcome: StaCommandOutcome::BackgroundScan { interrupted: None },
            })
        );
        world.take();
        // The access point's beacon shows the station's association identifier
        // (0xc02a & 0x3fff = 42: octet 5, bit 2) in its TIM, a partial virtual
        // bitmap from octet 4: PM=0 again.
        let mut tim_beacon = [0_u8; 62];
        tim_beacon[..55].copy_from_slice(&beacon());
        tim_beacon[55..62].copy_from_slice(&[5, 5, 0, 1, 4, 0, 0b100]);
        world.receive(at(200), &tim_beacon);
        assert_eq!(
            world.take(),
            [Seen::Transmit {
                subtype: NULL_ACTIVE,
                on: 6
            }]
        );
    });
}

/// The WPA2 network's RSN element: CCMP-128 with PSK.
const RSN: [u8; 22] = [
    0x30, 20, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 4, 1, 0, 0, 0x0f, 0xac, 2, 0, 0,
];
const PMK: [u8; 32] = [7; 32];
const ANONCE: [u8; 32] = [4; 32];

/// A beacon of the WPA2 network on channel 6.
fn wpa2_beacon() -> [u8; 77] {
    let mut frame = [0_u8; 77];
    frame[..55].copy_from_slice(&beacon());
    // ESS and Privacy.
    frame[34..36].copy_from_slice(&0x0011_u16.to_le_bytes());
    frame[55..77].copy_from_slice(&RSN);
    frame
}

/// An EAPOL frame from the access point to the station, in a data frame
/// from the DS.
fn eapol_frame(eapol: &[u8]) -> std::vec::Vec<u8> {
    let mut frame = std::vec![0_u8; 24];
    frame[0..2].copy_from_slice(&0x0208_u16.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&BSSID);
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0, 0x88, 0x8e]);
    frame.extend_from_slice(eapol);
    frame
}

#[test]
fn a_station_joins_a_wpa2_network_through_the_four_way_handshake() {
    installed(|mint| {
        use oer_ieee80211_mac::security::rsn::Akm;
        use oer_ieee80211_rsn::{PtkContext, frames::RsnTxFrame};

        let mut world = World::with_security(
            mint,
            3,
            StaSecurity::Personal {
                password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                pmk: Pmk::from_bytes(PMK),
            },
        );
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        world.advance();
        world.receive(at(13), &wpa2_beacon());
        world.advance();
        world.answer_authentication(at(30), 0);
        // The association response with the Privacy capability.
        world.receive(at(31), &association_response(0));
        world.take();

        // Message 1: the station answers with Message 2 in a data frame.
        let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
        world.receive(at(32), &eapol_frame(message1.as_bytes()));
        assert_eq!(
            world.take(),
            [Seen::Transmit {
                subtype: 0x0008,
                on: 6
            }]
        );

        // Message 3, with the access point's RSN element and the GTK in its
        // key data: the station installs both keys, then sends Message 4.
        let ptk = Pmk::from_bytes(PMK).derive_ptk(
            Akm::Psk,
            PtkContext {
                authenticator_address: BSSID,
                supplicant_address: LOCAL,
                authenticator_nonce: ANONCE,
                supplicant_nonce: SNONCE,
            },
        );
        let mut plain = std::vec::Vec::new();
        plain.extend_from_slice(&RSN);
        // The GTK KDE of key 1.
        plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
        plain.extend_from_slice(&[0x99; 16]);
        plain.extend_from_slice(&[0xdd, 0]);
        let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
        let message3 =
            RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
                .unwrap()
                .authenticate(&ptk);
        world.receive(at(33), &eapol_frame(message3.as_bytes()));
        let seen = world.take();
        assert!(matches!(seen[0], Seen::InstallPairwise(_)));
        assert_eq!(
            seen[1..],
            [
                Seen::InstallGroup(1),
                Seen::Transmit {
                    subtype: 0x0008,
                    on: 6
                },
                Seen::Notify(StaNotice::Connected {
                    bssid: oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap()
                    )
                    .unwrap(),
                    channel: 6
                }),
                Seen::Upstream(UpstreamFact::Connected(channel(6))),
            ]
        );
        assert!(world.station.is_connected());

        // Leaving removes both keys before the Deauthentication.
        world
            .station
            .command(at(40), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(40));
        assert_eq!(
            world.take()[..3],
            [
                Seen::RemoveKey,
                Seen::RemoveKey,
                Seen::Transmit {
                    subtype: 0x00c0,
                    on: 6
                },
            ]
        );
    });
}

/// A WPA2 world whose station holds `group_slots` group key slots, up to
/// Message 3 at 33 ms; the PTK of its handshake.
fn wpa2_associated<'id, const G: usize>(
    mint: Mint<'id, TestPools>,
) -> (World<'id, G>, oer_ieee80211_rsn::Ptk) {
    wpa2_associated_by::<G>(mint, &wpa2_beacon())
}

/// [`wpa2_associated`] with the access point's `beacon`.
fn wpa2_associated_by<'id, const G: usize>(
    mint: Mint<'id, TestPools>,
    beacon: &[u8],
) -> (World<'id, G>, oer_ieee80211_rsn::Ptk) {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::{PtkContext, frames::RsnTxFrame};

    let mut world = World::with_groups(
        mint,
        3,
        StaSecurity::Personal {
            password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
            pmk: Pmk::from_bytes(PMK),
        },
        core::array::from_fn::<_, G, _>(|i| Slot(u8::try_from(i + 1).unwrap())),
    );
    world
        .station
        .command(at(0), StaRequest::new(1, StaCommand::Connect))
        .unwrap();
    world.run(at(0));
    world.advance();
    world.receive(at(13), beacon);
    world.advance();
    world.answer_authentication(at(30), 0);
    world.receive(at(31), &association_response(0));
    let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
    world.receive(at(32), &eapol_frame(message1.as_bytes()));
    world.take();
    let ptk = Pmk::from_bytes(PMK).derive_ptk(
        Akm::Psk,
        PtkContext {
            authenticator_address: BSSID,
            supplicant_address: LOCAL,
            authenticator_nonce: ANONCE,
            supplicant_nonce: SNONCE,
        },
    );
    (world, ptk)
}

/// The world of [`wpa2_associated`], connected with group key 1.
fn wpa2_connected<'id, const G: usize>(
    mint: Mint<'id, TestPools>,
) -> (World<'id, G>, oer_ieee80211_rsn::Ptk) {
    wpa2_connected_by::<G>(mint, &wpa2_beacon())
}

/// [`wpa2_connected`] with the access point's `beacon`.
fn wpa2_connected_by<'id, const G: usize>(
    mint: Mint<'id, TestPools>,
    beacon: &[u8],
) -> (World<'id, G>, oer_ieee80211_rsn::Ptk) {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::frames::RsnTxFrame;

    let (mut world, ptk) = wpa2_associated_by::<G>(mint, beacon);
    let mut plain = std::vec::Vec::new();
    plain.extend_from_slice(&RSN);
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[0x99; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message3 =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(&ptk);
    world.receive(at(33), &eapol_frame(message3.as_bytes()));
    assert!(world.station.is_connected());
    world.take();
    (world, ptk)
}

#[test]
fn m4_refused_before_admission_never_completes_the_handshake() {
    installed(|mint| {
        use oer_ieee80211_mac::security::rsn::Akm;
        use oer_ieee80211_rsn::frames::RsnTxFrame;

        let (mut world, ptk) = wpa2_associated::<2>(mint);
        let mut plain = Vec::new();
        plain.extend_from_slice(&RSN);
        plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
        plain.extend_from_slice(&[0x99; 16]);
        plain.extend_from_slice(&[0xdd, 0]);
        let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
        let message3 =
            RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
                .unwrap()
                .authenticate(&ptk);
        world.unsend_next = true;
        world.receive(at(33), &eapol_frame(message3.as_bytes()));
        let seen = world.take();
        assert!(!world.station.is_connected());
        assert!(
            !seen
                .iter()
                .any(|event| matches!(event, Seen::Notify(StaNotice::Connected { .. })))
        );
        assert!(seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::HandshakeFailed
        ))));
        assert_eq!(
            seen.iter()
                .filter(|event| matches!(event, Seen::RemoveKey))
                .count(),
            2
        );
        assert_eq!(
            core::iter::from_fn(|| world.station.tickets.take()).count(),
            2
        );
        assert_eq!(
            core::iter::from_fn(|| world.station.buffers.take()).count(),
            2
        );
        assert_eq!(world.station.lost_tokens(), 0);
    });
}

#[test]
fn a_wpa2_gtk_kde_with_id_zero_uses_a_group_slot() {
    installed(|mint| {
        use oer_ieee80211_mac::security::rsn::Akm;
        use oer_ieee80211_rsn::frames::RsnTxFrame;

        let (mut world, ptk) = wpa2_associated::<1>(mint);
        let mut plain = Vec::from(RSN);
        // GTK KDE from Wireshark wpa2-psk-mfp frame 8, with KeyID
        // changed from 1 to 0 (IEEE 802.11-2020, 12.7.2). The unmodified
        // recorded M3 and its MIC are checked in rsn/frames/captured_m3.rs.
        plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x00, 0x00]);
        plain.extend_from_slice(&[
            0x70, 0xcd, 0xbf, 0x2e, 0x5b, 0xc0, 0xca, 0x22, 0xe5, 0x39, 0x30, 0x81, 0x8a, 0x5d,
            0x80, 0xe4,
        ]);
        plain.extend_from_slice(&[0xdd, 0]);
        let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
        let message3 = RsnTxFrame::<512>::message3(
            Akm::Psk,
            LOCAL,
            2,
            ANONCE,
            [9, 0, 0, 0, 0, 0, 0, 0],
            wrapped.as_bytes(),
        )
        .unwrap()
        .authenticate(&ptk);
        world.receive(at(33), &eapol_frame(message3.as_bytes()));
        assert!(world.station.is_connected());
        assert!(world.take().contains(&Seen::InstallGroup(0)));
        assert_eq!(world.station.keys.group_slots.free.len(), 0);
        world
            .station
            .command(at(34), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(34));
        assert!(matches!(
            world.station.keys.pairwise,
            KeyCell::Free(VacantPairwise { slot: Slot(0), .. })
        ));
        assert_eq!(world.station.keys.group_slots.free.len(), 1);
    });
}

/// Group Message 1 of key `key_id` with replay counter `replay`.
fn group_message1(ptk: &oer_ieee80211_rsn::Ptk, replay: u64, key_id: u8) -> std::vec::Vec<u8> {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::frames::RsnTxFrame;

    let mut plain = std::vec::Vec::new();
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, key_id, 0x00]);
    plain.extend_from_slice(&[0x77; 16]);
    plain.extend_from_slice(&[0xdd, 0, 0, 0, 0, 0, 0, 0]);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message =
        RsnTxFrame::<512>::group_message1(Akm::Psk, LOCAL, replay, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    protected_received(eapol_frame(message.as_bytes()), replay)
}

#[test]
fn a_new_group_key_retains_the_old_installation_after_group_message_2() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        assert_eq!(
            world.take(),
            [
                Seen::InstallGroup(2),
                Seen::Transmit {
                    subtype: 0x0008,
                    on: 6
                },
            ]
        );
        assert!(world.station.is_connected());
        assert!(
            world
                .station
                .keys
                .groups
                .same_material(GroupKeyId::new(1).unwrap(), &[0x99; 16])
        );
        assert_eq!(world.station.keys.group_slots.free.len(), 0);
    });
}

#[test]
fn the_station_says_it_associated_before_the_handshakes_first_message() {
    installed(|mint| {
        let (world, _) = wpa2_connected::<2>(mint);
        // After the Association Request, before Message 2.
        let (sent, peer) = world.associated.expect("the station associated");
        let subtype = |frame: &Vec<u8>| u16::from_le_bytes([frame[0], frame[1]]) & 0x00fc;
        assert_eq!(subtype(&world.frames[sent - 1]), 0x0000);
        assert_eq!(subtype(&world.frames[sent]), 0x0008);
        assert_eq!(peer.response, meta());
    });
}

#[test]
fn the_handshakes_eapol_goes_unprotected_and_group_message_2_under_the_pairwise_key() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        // Messages 2 and 4 of the four-way handshake: no key, no Protected bit.
        assert_eq!(world.eapol, [(false, false), (false, false)]);
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        // Group Message 2, once connected: under the pairwise key, with the
        // Protected bit and the CCMP header's room.
        assert_eq!(world.eapol[2..], [(true, true)]);
    });
}

impl crate::data::StaDataTokens for Tokens {
    type EpochPath = oer_radio_port::Root;
    type Class = Buffer;
    type Ticket = Ticket;
    type Buffer = Buffer;
    type Sent = Buffer;
    type AckEvidence = ();
    type AirDenied = ();
    fn renew(mpdu: Self::Sent) -> Buffer {
        mpdu
    }
}

/// The TID and sequence number of a QoS Data frame.
fn tid_and_sequence(frame: &[u8]) -> (u8, u16) {
    (
        frame[24] & 0x0f,
        u16::from_le_bytes([frame[22], frame[23]]) >> 4,
    )
}

#[test]
fn eapol_and_data_on_tid_7_share_one_counter_and_tid_0_runs_on_across_a_rekey() {
    installed(|mint| {
        use crate::data::{DataInput, Msdu, StaData};
        use oer_ieee80211_mac::qos::WmmUserPriority;

        // A QoS BSS: the beacon advertises WMM.
        let mut beacon = wpa2_beacon().to_vec();
        beacon.extend_from_slice(&[0xdd, 7, 0x00, 0x50, 0xf2, 0x02, 0x00, 0x01, 0x00]);
        let (mut world, ptk) = wpa2_connected_by::<2>(mint, &beacon);
        let mut data = StaData::<Tokens, Caps>::new(
            [Ticket(10)],
            [Buffer([0; 512], 512)],
            core::num::NonZeroU8::MIN,
        );
        fn send<'id>(
            data: &mut StaData<'id, Tokens, Caps>,
            world: &mut World<'id>,
            priority: u8,
        ) -> (u8, u16) {
            let mut link = world.station.link().expect("the link is up");
            let msdu = Msdu {
                destination: BSSID,
                ether_type: 0x0800,
                priority: WmmUserPriority::new(priority).unwrap(),
                payload: &[0xab; 20],
            };
            data.input(DataInput {
                msdu,
                link: &mut link,
            })
            .unwrap();
            let Some(transmit) = data.poll_action() else {
                panic!("an MPDU is owed");
            };
            let crate::data::DataTransmitParts {
                context,
                ticket,
                buffer,
            } = transmit.into_parts();
            let frame = match &buffer {
                ReturnedMpdu::Fresh(buffer) | ReturnedMpdu::Sent(buffer) => &buffer.0,
            };
            let sent = tid_and_sequence(frame);
            data.returned(crate::data::DataReturn::TxDone {
                context,
                ticket,
                buffer,
                completion: ended(End::Delivered),
            });
            sent
        }
        assert_eq!(send(&mut data, &mut world, 7), (7, 0));
        assert_eq!(send(&mut data, &mut world, 0), (0, 0));
        // A rekey: Group Message 2 is QoS Data on TID 7, numbered in its space.
        let before = world.frames.len();
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        let group_message2 = world.frames[before..]
            .iter()
            .find(|frame| frame[0] & 0xfc == 0x88)
            .expect("Group Message 2 went as QoS Data");
        assert_eq!(tid_and_sequence(group_message2), (7, 1));
        // TID 7 goes on after the EAPOL's number; TID 0 never lost one.
        assert_eq!(send(&mut data, &mut world, 7), (7, 2));
        assert_eq!(send(&mut data, &mut world, 0), (0, 1));
    });
}

#[test]
fn a_single_group_slot_is_replaced_without_a_separate_removal() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<1>(mint);
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        assert_eq!(
            world.take(),
            [
                Seen::ReplaceGroup(2),
                Seen::Transmit {
                    subtype: 0x0008,
                    on: 6
                },
            ]
        );
        assert!(world.station.is_connected());
        assert_eq!(world.station.keys.group_slots.free.len(), 0);
        world
            .station
            .command(at(51), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(51));
        assert!(world.station.keys.clear());
    });
}

#[test]
fn identical_gtk_material_keeps_its_installation_and_replay_frontier() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        let mut link = world.station.link().unwrap();
        let LinkProtection::Ccmp { replay, .. } = &mut link.protection else {
            panic!()
        };
        let header = oer_ieee80211_mac::ccmp::CcmpHeader::new(
            oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(900).unwrap(),
            GroupKeyId::new(2).unwrap().key_id(),
        );
        replay
            .check(
                oer_ieee80211_mac::ccmp::CcmpRxProtection::Group,
                None,
                header,
            )
            .unwrap();
        world.take();
        world.receive_verified(at(51), &group_message1(&ptk, 4, 2));
        assert!(!world.seen.iter().any(|event| matches!(
            event,
            Seen::InstallGroup(_) | Seen::ReplaceGroup(_) | Seen::RemoveKey
        )));
        let mut link = world.station.link().unwrap();
        let LinkProtection::Ccmp { replay, .. } = &mut link.protection else {
            panic!()
        };
        assert!(
            replay
                .check(
                    oer_ieee80211_mac::ccmp::CcmpRxProtection::Group,
                    None,
                    header
                )
                .is_err()
        );
        assert_eq!(world.station.keys.group_slots.free.len(), 0);
    });
}

#[test]
fn old_rx_between_replacement_and_its_terminal_cannot_advance_the_successor_epoch() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<1>(mint);
        world.hold_key_installs = true;
        world.receive_verified(at(50), &group_message1(&ptk, 3, 1));
        assert_eq!(world.pending_installs.len(), 1);
        let mut link = world.station.link().unwrap();
        let LinkProtection::Ccmp { replay, .. } = &mut link.protection else {
            panic!()
        };
        let header = |pn| {
            oer_ieee80211_mac::ccmp::CcmpHeader::new(
                oer_ieee80211_mac::ccmp::CcmpPacketNumber::new(pn).unwrap(),
                GroupKeyId::new(1).unwrap().key_id(),
            )
        };
        replay
            .check(
                oer_ieee80211_mac::ccmp::CcmpRxProtection::Group,
                None,
                header(900),
            )
            .unwrap();
        let terminal = world.pending_installs.pop().unwrap();
        world.station.input(at(51), terminal);
        world.run(at(51));
        let mut link = world.station.link().unwrap();
        let LinkProtection::Ccmp { replay, .. } = &mut link.protection else {
            panic!()
        };
        replay
            .check(
                oer_ieee80211_mac::ccmp::CcmpRxProtection::Group,
                None,
                header(1),
            )
            .unwrap();
        assert!(
            replay
                .check(
                    oer_ieee80211_mac::ccmp::CcmpRxProtection::Group,
                    None,
                    header(1)
                )
                .is_err()
        );
        assert_eq!(world.station.keys.group_slots.free.len(), 0);
        assert!(!world.seen.contains(&Seen::RemoveKey));
    });
}

#[test]
fn a_handshake_that_times_out_deauthenticates_with_its_reason() {
    installed(|mint| {
        let (mut world, _) = wpa2_associated::<2>(mint);
        // No Message 3: the handshake's deadline passes.
        world.advance();
        assert_eq!(world.deauthentications, [REASON_HANDSHAKE_TIMEOUT]);
        assert!(world.take().contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::HandshakeFailed
        ))));
    });
}

#[test]
fn a_message_3_whose_key_data_does_not_unwrap_deauthenticates() {
    installed(|mint| {
        use oer_ieee80211_mac::security::rsn::Akm;
        use oer_ieee80211_rsn::frames::RsnTxFrame;

        let (mut world, ptk) = wpa2_associated::<2>(mint);
        let message3 = RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], &[0x55; 24])
            .unwrap()
            .authenticate(&ptk);
        world.receive(at(33), &eapol_frame(message3.as_bytes()));
        assert_eq!(world.deauthentications, [REASON_UNSPECIFIED]);
    });
}

#[test]
fn the_access_points_deauthentication_is_not_answered_with_one() {
    installed(|mint| {
        let (mut world, _) = wpa2_connected::<2>(mint);
        let mut deauthentication = [0_u8; 26];
        deauthentication[0..2].copy_from_slice(&0x00c0_u16.to_le_bytes());
        deauthentication[4..10].copy_from_slice(&LOCAL);
        deauthentication[10..16].copy_from_slice(&BSSID);
        deauthentication[16..22].copy_from_slice(&BSSID);
        deauthentication[24..26].copy_from_slice(&7_u16.to_le_bytes());
        world.receive(at(50), &deauthentication);
        let seen = world.take();
        // Both keys removed, no Deauthentication back.
        assert_eq!(
            seen.iter().filter(|seen| **seen == Seen::RemoveKey).count(),
            2
        );
        assert!(world.deauthentications.is_empty());
        assert!(seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Peer { reason_code: 7 }
        ))));
    });
}

#[test]
fn disconnects_to_other_receivers_never_end_handshake_or_connection() {
    for connected in [false, true] {
        for subtype in [0xa0, 0xc0] {
            installed(|mint| {
                let (mut world, _) = if connected {
                    wpa2_connected::<2>(mint)
                } else {
                    wpa2_associated::<2>(mint)
                };
                world.take();
                let mut base = [0; 26];
                base[0] = subtype;
                base[4..10].copy_from_slice(&LOCAL);
                base[10..16].copy_from_slice(&BSSID);
                base[16..22].copy_from_slice(&BSSID);
                base[24..26].copy_from_slice(&7_u16.to_le_bytes());
                for (offset, byte) in [(4, 4), (4, 1), (10, 4), (16, 4), (1, 0x40)] {
                    let mut frame = base;
                    frame[offset] = byte;
                    // Even descriptor integrity evidence cannot turn a
                    // protected body into unprotected management admission.
                    world
                        .station
                        .receive(at(50), &frame, verified_meta(), |_| {})
                        .unwrap();
                    world.run(at(50));
                    assert!(matches!(
                        world.station.state,
                        State::Handshake(_) | State::Connected(_)
                    ));
                    assert_eq!(world.station.is_connected(), connected);
                    assert!(world.take().is_empty());
                }
                world.receive(at(50), &base[..25]);
                assert!(world.take().is_empty());
                assert!(matches!(
                    world.station.state,
                    State::Handshake(_) | State::Connected(_)
                ));
            });
        }
    }
}

#[test]
fn local_or_broadcast_disconnect_from_selected_bss_ends_the_link() {
    for connected in [false, true] {
        for receiver in [LOCAL, [0xff; 6]] {
            for subtype in [0xa0, 0xc0] {
                installed(|mint| {
                    let (mut world, _) = if connected {
                        wpa2_connected::<2>(mint)
                    } else {
                        wpa2_associated::<2>(mint)
                    };
                    world.take();
                    let mut frame = [0; 26];
                    frame[0] = subtype;
                    frame[4..10].copy_from_slice(&receiver);
                    frame[10..16].copy_from_slice(&BSSID);
                    frame[16..22].copy_from_slice(&BSSID);
                    frame[24..26].copy_from_slice(&7_u16.to_le_bytes());
                    world.receive(at(50), &frame);
                    assert!(world.station.keys.clear());
                    assert!(world.deauthentications.is_empty());
                    assert!(world.take().contains(&Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::Peer { reason_code: 7 }
                    ))));
                });
            }
        }
    }
}

#[test]
fn leaving_on_request_deauthenticates_as_leaving() {
    installed(|mint| {
        let (mut world, _) = wpa2_connected::<2>(mint);
        world
            .station
            .command(at(50), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(50));
        assert_eq!(world.deauthentications, [REASON_LEAVING]);
    });
}

/// A beacon of the open network from `bssid` on the channel `number`.
fn beacon_from(bssid: [u8; 6], number: u8) -> [u8; 55] {
    let mut frame = beacon_on(number);
    frame[10..16].copy_from_slice(&bssid);
    frame[16..22].copy_from_slice(&bssid);
    frame
}

/// The network on channel 1, where the access point beside the station
/// runs, at `beside` dBm and on channel 6 at -56 dBm: the channel the join
/// goes to, and whether the access point announces a move.
fn joined_beside<'id>(mint: Mint<'id, TestPools>, beside: i8) -> (u8, bool) {
    let mut world = World::new(mint, 3);
    access_point_on_1(&mut world);
    world
        .station
        .command(at(0), StaRequest::new(1, StaCommand::Connect))
        .unwrap();
    world.run(at(0));
    world.rssi = beside;
    world.receive(at(1), &beacon_from([2, 0, 0, 0, 0, 0xbb], 1));
    world.advance();
    world.advance();
    world.rssi = -56;
    world.receive(at(53), &beacon_from(BSSID, 6));
    let now = advance_until(&mut world, |seen| {
        matches!(
            seen,
            Seen::Transmit {
                subtype: 0x00b0,
                ..
            }
        ) || announces(seen)
    });
    let _ = now;
    let seen = world.take();
    let announced = seen.iter().any(announces);
    let on = seen
        .iter()
        .find_map(|seen| match seen {
            Seen::Transmit {
                subtype: 0x00b0,
                on,
            } => Some(*on),
            _ => None,
        })
        .unwrap_or(6);
    (on, announced)
}

#[test]
fn a_candidate_on_the_access_points_channel_wins_within_the_margin() {
    installed(|mint| {
        // 4 dB weaker: within 6 dB, the join stays on the access point's
        // channel and nothing moves.
        assert_eq!(joined_beside(mint, -60), (1, false));
    });
    installed(|mint| {
        // 14 dB weaker: the strongest wins, and the access point announces its
        // move.
        assert_eq!(joined_beside(mint, -70), (6, true));
    });
}

#[test]
fn stale_handshake_installations_return_the_association_only_after_all_keys() {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::frames::RsnTxFrame;

    for group_pending in [false, true] {
        installed(|mint| {
            let (mut world, ptk) = wpa2_associated::<2>(mint);
            let mut plain = Vec::new();
            plain.extend_from_slice(&RSN);
            plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
            plain.extend_from_slice(&[0x99; 16]);
            plain.extend_from_slice(&[0xdd, 0]);
            let wrapped =
                oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
            let message3 =
                RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
                    .unwrap()
                    .authenticate(&ptk);
            world.hold_key_installs = true;
            world.receive(at(33), &eapol_frame(message3.as_bytes()));
            assert_eq!(world.pending_installs.len(), 1);
            if group_pending {
                let pairwise = world.pending_installs.pop().unwrap();
                world.station.input(at(33), pairwise);
                world.run(at(33));
                assert_eq!(world.pending_installs.len(), 1);
            }
            world
                .station
                .command(at(34), StaRequest::new(2, StaCommand::Disconnect))
                .unwrap();
            world.run(at(34));
            assert!(world.station.current_epoch().is_none());
            assert!(matches!(world.station.state, State::Leave(_)));
            // Connect cannot replace an installation still reclaiming its keys.
            let refused = world
                .station
                .command(at(34), StaRequest::new(1, StaCommand::Connect))
                .unwrap_err();
            assert_eq!(refused.reason, CommandRefusal::Transitioning);
            assert!(world.station.current_epoch().is_none());
            let returned = core::mem::take(&mut world.pending_installs);
            for input in returned {
                world.station.input(at(35), input);
                world.run(at(35));
                assert_eq!(
                    world
                        .seen
                        .iter()
                        .filter(|event| **event == Seen::RemoveKey)
                        .count(),
                    if group_pending { 2 } else { 1 }
                );
                assert!(!world.station.is_connected());
            }
            assert!(world.station.keys.clear());
            assert!(matches!(world.station.state, State::Idle));
            assert_eq!(world.station.lost_tokens(), 0);
            assert!(
                !world
                    .seen
                    .iter()
                    .any(|event| matches!(event, Seen::Notify(StaNotice::Connected { .. })))
            );
            // M2 was the only EAPOL: the stale installations did not cause M4.
            assert_eq!(world.eapol.len(), 1);
            world
                .station
                .command(at(36), StaRequest::new(1, StaCommand::Connect))
                .unwrap();
            assert!(world.station.current_epoch().is_some());
        });
    }
}

#[test]
fn a_rekey_installed_after_disconnect_is_removed_without_group_message_2() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.hold_key_installs = true;
        let eapol = world.eapol.len();
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        assert_eq!(world.pending_installs.len(), 1);
        world
            .station
            .command(at(51), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(51));
        assert!(matches!(world.station.state, State::Leave(_)));
        let installed = world.pending_installs.pop().unwrap();
        world.station.input(at(52), installed);
        world.run(at(52));
        assert!(world.station.keys.clear());
        assert_eq!(world.eapol.len(), eapol);
        assert_eq!(
            world
                .seen
                .iter()
                .filter(|event| **event == Seen::RemoveKey)
                .count(),
            3
        );
        assert!(matches!(world.station.state, State::Idle));
    });
}

#[test]
fn a_replacement_terminal_after_disconnect_returns_its_slot_through_removal() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<1>(mint);
        world.hold_key_installs = true;
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        assert_eq!(world.pending_installs.len(), 1);
        let responses = world.eapol.len();
        world
            .station
            .command(at(51), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(51));
        assert!(!world.station.keys.clear());
        let terminal = world.pending_installs.pop().unwrap();
        world.station.input(at(52), terminal);
        world.run(at(52));
        assert!(world.station.keys.clear());
        assert_eq!(world.eapol.len(), responses);
        assert_eq!(
            world
                .seen
                .iter()
                .filter(|event| **event == Seen::RemoveKey)
                .count(),
            2
        );
    });
}

#[test]
fn group_removal_returns_permission_before_another_owner_can_leave() {
    let mut keys = Keys::<Tokens>::new::<2>((Slot(0), [Slot(7), Slot(42)]), GroupPermission);
    for (id, slot) in [(1, 7), (2, 42)] {
        let place = keys.groups.reserve().unwrap();
        let free = keys.group_slots.free.take().unwrap();
        assert!(matches!(free, Slot(7) | Slot(42)));
        keys.groups.install(
            InstalledGroup {
                place,
                key_id: GroupKeyId::new(id).unwrap(),
                material: Ccmp128Key::new(&[id; 16]),
                key: Key(Slot(slot)),
                replay: GroupReplay::Retained(
                    oer_ieee80211_mac::ccmp::CcmpRxReplayState::from_receive_sequence([0; 8])
                        .unwrap(),
                ),
            },
            true,
        );
    }
    let (permission, entry) = keys.take_group_key(false).unwrap();
    assert!(keys.take_group_key(false).is_none());
    assert!(!keys.clear());
    keys.group_removed(entry.key.0, permission, entry.place);
    let (permission, entry) = keys.take_group_key(false).unwrap();
    assert!(!keys.clear());
    keys.group_removed(entry.key.0, permission, entry.place);
    assert!(keys.clear());
}

#[test]
fn old_configuration_response_releases_permission_without_completing_leave() {
    installed(|mint| {
        let mut world = World::new(mint, 2);
        world.hold_configured = true;
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        let pending_scan = world.configuring.take().unwrap();
        world
            .station
            .command(at(1), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(1));
        assert!(world.configuring.is_none());
        assert!(!world.seen.contains(&Seen::Configure("off")));
        world
            .station
            .input(at(2), StaInput::Configured(pending_scan));
        world.run(at(2));
        assert!(world.seen.contains(&Seen::Configure("off")));
        assert!(matches!(world.station.state, State::Leave(_)));
        let pending_off = world.configuring.take().unwrap();
        world
            .station
            .input(at(3), StaInput::Configured(pending_off));
        world.run(at(3));
        assert!(matches!(world.station.state, State::Idle));
    });
}

#[test]
fn authentication_transmitter_is_station() {
    installed(|mint| {
        let world = connected_world(mint);
        let auth = world.frames.iter().find(|frame| frame[0] == 0xb0).unwrap();
        assert_eq!(u16::from_le_bytes([auth[0], auth[1]]) & !0x0800, 0x00b0);
        assert_eq!(&auth[4..10], &BSSID);
        assert_eq!(&auth[10..16], &LOCAL);
        assert_eq!(&auth[16..22], &BSSID);
        assert!(answer_authentication(auth, 0).is_some());
        for offset in [4, 10, 16] {
            let mut malformed = auth.clone();
            malformed[offset] ^= 2;
            assert!(answer_authentication(&malformed, 0).is_none());
        }
    });
}

#[test]
fn leave_without_lease_reports_before_waiting() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world
            .station
            .command(at(0), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        let Some(StaAction::Configure(StationReceive::Off, pending)) =
            world.station.poll_action(at(0))
        else {
            panic!("disconnect first disables reception");
        };
        world.station.input(at(0), StaInput::Configured(pending));
        for (correlation, outcome) in [
            (
                1,
                StaCommandOutcome::ConnectFailed(StaDisconnectReason::Requested),
            ),
            (
                2,
                StaCommandOutcome::Disconnected(StaDisconnectReason::Requested),
            ),
        ] {
            let Some(StaAction::CommandCompleted(terminal)) = world.station.poll_action(at(0))
            else {
                panic!()
            };
            assert_eq!(terminal.request.correlation(), correlation);
            assert_eq!(terminal.outcome, outcome);
        }
        assert!(matches!(
            world.station.poll_action(at(0)),
            Some(StaAction::Notify(StaNotice::Disconnected(
                StaDisconnectReason::Requested
            )))
        ));
        assert!(world.station.poll_action(at(0)).is_none());
        assert!(world.station.poll_deadline().is_none());
    });
}

#[test]
fn deauthentication_waits_for_the_ticket_and_buffer_of_an_accepted_transmission() {
    installed(|mint| {
        let mut world =
            connect_open_world(World::with_builder::<1, 0>(mint, 3, [], Station::new_open));
        let mut pending = Vec::new();
        for _ in 0..1 {
            pending.push(
                world
                    .station
                    .encode_null(
                        oer_ieee80211_mac::address::Bssid::new(
                            oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                        )
                        .unwrap(),
                        StaPowerManagement::Active,
                    )
                    .unwrap(),
            );
        }
        world
            .station
            .command(at(50), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        assert!(
            world.station.poll_action(at(50)).is_none(),
            "deauthentication must wait for credit"
        );
        assert!(world.station.poll_deadline().is_none());
        let transmit = pending.pop().unwrap();
        world.station.input(
            at(51),
            StaInput::TxDone {
                context: transmit.context,
                ticket: transmit.ticket,
                buffer: ReturnedMpdu::Fresh(transmit.buffer),
                completion: TxOutcome::Withdrawn(
                    oer_ieee80211_lower_mac::WithdrawCause::GateClosed,
                ),
            },
        );
        let Some(StaAction::Transmit(transmit)) = world.station.poll_action(at(51)) else {
            panic!("returned credit must release deauthentication");
        };
        assert!(matches!(
            transmit.context.purpose,
            TxPurpose::Deauthentication
        ));
        world.transmitted(transmit, false);
        for transmit in pending {
            world.transmitted(transmit, false);
        }
        world.run(at(51));
        assert_eq!(world.station.tickets.len(), 1);
        assert_eq!(world.station.buffers.len(), 1);
        assert!(world.take().contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Requested
        ))));
    });
}

#[test]
fn disconnect_during_an_absence_sends_deauthentication_only_after_returning_home() {
    installed(|mint| {
        let mut world = connected_world(mint);
        world
            .station
            .command(at(40), StaRequest::new(3, StaCommand::BackgroundScan))
            .unwrap();
        world.run(at(40));
        let now = world.advance();
        assert_eq!(world.arbiter.channel().unwrap().channel().number(), 1);
        world.take();
        world
            .station
            .command(now, StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(now);
        assert!(
            world.take().is_empty(),
            "no leave frame or configuration while away"
        );
        world.advance();
        let seen = world.take();
        assert!(seen.contains(&Seen::Transmit {
            subtype: 0x00c0,
            on: 6
        }));
        assert!(!seen.contains(&Seen::Transmit {
            subtype: 0x00c0,
            on: 1
        }));
        assert!(seen.contains(&Seen::Notify(StaNotice::Disconnected(
            StaDisconnectReason::Requested
        ))));
    });
}

#[test]
fn associated_is_the_first_action_after_the_association_response() {
    for protected in [false, true] {
        installed(|mint| {
            let security = if protected {
                StaSecurity::Personal {
                    password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                    pmk: Pmk::from_bytes(PMK),
                }
            } else {
                StaSecurity::Open
            };
            let mut world = World::with_security(mint, 3, security);
            world
                .station
                .command(at(0), StaRequest::new(1, StaCommand::Connect))
                .unwrap();
            world.run(at(0));
            world.advance();
            if protected {
                world.receive(at(13), &wpa2_beacon());
            } else {
                world.receive(at(13), &beacon());
            }
            world.advance();
            world.answer_authentication(at(30), 0);
            let response = if protected {
                association_response(0)
            } else {
                open_association_response()
            };
            world
                .station
                .receive(at(31), &response, meta(), |_| {})
                .unwrap();
            assert!(matches!(
                world.station.poll_action(at(31)),
                Some(StaAction::Associated(_))
            ));
            if protected {
                assert!(world.station.poll_action(at(31)).is_none());
                assert!(matches!(
                    world.station.state,
                    State::Handshake(Handshake {
                        step: HandshakeStep::Await,
                        ..
                    })
                ));
                assert!(
                    world.station.poll_deadline().is_some(),
                    "waiting for M1 has a deadline"
                );
            } else {
                assert!(matches!(
                    world.station.poll_action(at(31)),
                    Some(StaAction::CommandCompleted(StaCommandTerminal {
                        outcome: StaCommandOutcome::Connected { .. },
                        ..
                    }))
                ));
                assert!(matches!(
                    world.station.poll_action(at(31)),
                    Some(StaAction::Notify(StaNotice::Connected { .. }))
                ));
            }
        });
    }
}

#[test]
fn irrelevant_management_while_authentication_waits_for_credit_preserves_join() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        world.run(at(0));
        world.advance();
        world.receive(at(13), &beacon());
        world.hold_configured = true;
        let now = world.advance();
        let pending = world.configuring.take().unwrap();
        let first = world.station.tickets.take().unwrap();
        let second = world.station.tickets.take().unwrap();
        world.station.input(now, StaInput::Configured(pending));
        let before = world.station.sequence;
        let mut after_one_attempt = before;
        after_one_attempt.take();
        assert!(world.station.poll_action(now).is_none());
        assert!(matches!(world.station.state, State::Join(_)));
        assert_eq!(world.station.sequence, before);
        assert!(world.station.poll_action(now).is_none());
        assert_eq!(world.station.sequence, before);
        let mut probe = beacon();
        probe[0] = 0x50;
        let mut foreign_auth = crate::join::test_support::authentication_response(0);
        foreign_auth[4..10].copy_from_slice(&[2, 0, 0, 0, 0, 0x7a]);
        for input in [&beacon()[..], &probe[..], &foreign_auth[..]] {
            world.station.receive(now, input, meta(), |_| {}).unwrap();
            assert!(matches!(
                world.station.state,
                State::Join(Join {
                    step: JoinStep::Authenticate(Authenticate::NeedCredit(_, _)),
                    ..
                })
            ));
            assert!(world.station.poll_action(now).is_none());
            assert_eq!(world.station.sequence, before);
        }
        world.station.tickets.put(first);
        world.station.tickets.put(second);
        let Some(StaAction::Transmit(transmit)) = world.station.poll_action(now) else {
            panic!("returning credits must continue the unsent authentication attempt");
        };
        assert_eq!(world.station.sequence, after_one_attempt);
        world.transmitted(transmit, false);
        world.end_transmissions(now);
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

#[test]
fn association_waits_for_credit_without_consuming_a_request_and_still_expires() {
    installed(|mint| {
        let mut world = connected_world(mint);
        let mut held = Vec::new();
        while let Some(ticket) = world.station.tickets.take() {
            held.push(ticket);
        }
        let access_point = *world.station.scan.records().next().unwrap();
        let join = || Join {
            rsn: select_association_rsn(&access_point, world.station.policy()).unwrap(),
            access_point,
            channel: world.station.config.channels[0],
            step: JoinStep::Associate(
                StaAssociationRuntime::new(
                    oer_ieee80211_mac::address::OwnAddress::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap(),
                    ),
                    oer_ieee80211_mac::address::Bssid::new(
                        oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
                    )
                    .unwrap(),
                    oer_ieee80211_mac::security::LinkProtection::Open,
                    at(100),
                )
                .unwrap(),
                AssociationKey::Open,
            ),
        };
        world.station.state = State::Join(join());
        let before = world.station.sequence;
        assert!(world.station.poll_action(at(100)).is_none());
        assert_eq!(world.station.sequence, before);
        assert_eq!(world.station.poll_deadline(), Some(at(1100)));
        world.station.tickets.put(held.pop().unwrap());
        let Some(StaAction::Transmit(transmit)) = world.station.poll_action(at(100)) else {
            panic!("returning credit resumes the first association request");
        };
        // This TX owns the only available credit while the next retry becomes
        // due. The association's absolute deadline must remain armed.
        let after_first = world.station.sequence;
        let mut after_one_attempt = before;
        after_one_attempt.take();
        assert_eq!(after_first, after_one_attempt);
        assert!(world.station.poll_action(at(300)).is_none());
        assert_eq!(world.station.sequence, after_first);
        assert_eq!(world.station.poll_deadline(), Some(at(1100)));
        world.station.input(at(1100), StaInput::Tick);
        let Some(StaAction::Configure(StationReceive::Off, pending)) =
            world.station.poll_action(at(1100))
        else {
            panic!("association must leave at its absolute deadline")
        };
        world.station.input(at(1100), StaInput::Configured(pending));
        world.station.input(
            at(1100),
            StaInput::TxDone {
                context: transmit.context,
                ticket: transmit.ticket,
                buffer: ReturnedMpdu::Fresh(transmit.buffer),
                completion: TxOutcome::Withdrawn(
                    oer_ieee80211_lower_mac::WithdrawCause::GateClosed,
                ),
            },
        );
        for ticket in held {
            world.station.tickets.put(ticket);
        }
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        world.run(at(1100));
        world.station.input(at(1101), StaInput::Tick);
        world.run(at(1101));
        assert_eq!(
            world
                .take()
                .iter()
                .filter(|event| matches!(
                    event,
                    Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::AssociationFailed
                    ))
                ))
                .count(),
            1
        );
    });
}

#[test]
fn handshake_deadline_covers_every_message_key_and_tx_wait() {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::{PtkContext, frames::RsnTxFrame};
    for point in 0..8 {
        installed(|mint| {
            let mut world = World::with_security(
                mint,
                3,
                StaSecurity::Personal {
                    password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                    pmk: Pmk::from_bytes(PMK),
                },
            );
            world
                .station
                .command(at(0), StaRequest::new(1, StaCommand::Connect))
                .unwrap();
            world.run(at(0));
            world.advance();
            world.receive(at(13), &wpa2_beacon());
            world.advance();
            world.answer_authentication(at(30), 0);
            world.receive(at(31), &association_response(0));
            let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
            let _ =
                world
                    .station
                    .receive(at(32), &eapol_frame(message1.as_bytes()), meta(), |_| {});
            let mut transmissions = Vec::new();
            if point >= 1 {
                let Some(StaAction::Transmit(tx)) = world.station.poll_action(at(32)) else {
                    panic!("M2")
                };
                transmissions.push(tx);
            }
            if point >= 2 {
                let tx = transmissions.pop().unwrap();
                world.transmitted(tx, false);
                world.end_transmissions(at(32));
            }
            if point >= 3 {
                let ptk = Pmk::from_bytes(PMK).derive_ptk(
                    Akm::Psk,
                    PtkContext {
                        authenticator_address: BSSID,
                        supplicant_address: LOCAL,
                        authenticator_nonce: ANONCE,
                        supplicant_nonce: SNONCE,
                    },
                );
                let mut plain = RSN.to_vec();
                plain.extend_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0]);
                plain.extend_from_slice(&[0x99; 16]);
                plain.extend_from_slice(&[0xdd, 0]);
                let wrapped =
                    oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
                let message3 = RsnTxFrame::<512>::message3(
                    Akm::Psk,
                    LOCAL,
                    2,
                    ANONCE,
                    [0; 8],
                    wrapped.as_bytes(),
                )
                .unwrap()
                .authenticate(&ptk);
                let _ = world.station.receive(
                    at(33),
                    &eapol_frame(message3.as_bytes()),
                    meta(),
                    |_| {},
                );
            }
            if point >= 4 {
                let Some(StaAction::InstallPairwiseKey {
                    association,
                    pending,
                    slot,
                    ..
                }) = world.station.poll_action(at(33))
                else {
                    panic!("PTK")
                };
                world.pending_installs.push(StaInput::PairwiseKeyInstalled {
                    pending,
                    key: Pairwise(Key(slot), association),
                });
            }
            if point >= 5 {
                let pairwise = world.pending_installs.pop().unwrap();
                world.station.input(at(33), pairwise);
                let Some(StaAction::InstallGroupKey {
                    pending,
                    slot,
                    permission,
                    ..
                }) = world.station.poll_action(at(33))
                else {
                    panic!("GTK")
                };
                world.pending_installs.push(StaInput::GroupKeyInstalled {
                    permission,
                    pending,
                    key: Key(slot),
                });
            }
            if point >= 6 {
                for installed in core::mem::take(&mut world.pending_installs) {
                    world.station.input(at(33), installed);
                }
            }
            if point >= 7 {
                let Some(StaAction::Transmit(tx)) = world.station.poll_action(at(33)) else {
                    panic!("M4")
                };
                transmissions.push(tx);
            }
            assert_eq!(
                world.station.poll_deadline(),
                Some(at(6032)),
                "handshake step {point}"
            );
            world.station.input(at(6032), StaInput::Tick);
            assert!(
                matches!(world.station.state, State::Leave(_)),
                "step {point} must expire"
            );
            assert!(world.station.poll_deadline().is_none());
            for installed in core::mem::take(&mut world.pending_installs) {
                world.station.input(at(6032), installed);
            }
            for tx in transmissions {
                world.transmitted(tx, false);
            }
            world.run(at(6032));
            world.station.input(at(6033), StaInput::Tick);
            world.run(at(6033));
            assert_eq!(world.station.tickets.len(), 2);
            assert_eq!(world.station.buffers.len(), 2);
            assert!(world.station.keys.clear());
            let seen = world.take();
            assert_eq!(
                seen.iter()
                    .filter(|event| matches!(
                        event,
                        Seen::Notify(StaNotice::Disconnected(
                            StaDisconnectReason::HandshakeFailed
                        ))
                    ))
                    .count(),
                1
            );
            assert!(
                !seen
                    .iter()
                    .any(|event| matches!(event, Seen::Notify(StaNotice::Connected { .. })))
            );
        });
    }
}

#[test]
fn every_control_encoder_reserves_credit_before_consuming_its_sequence() {
    for hold_buffers in [false, true] {
        for encoding in 0..7 {
            installed(|mint| {
                let mut world = connected_world(mint);
                let mut tickets = Vec::new();
                let mut buffers = Vec::new();
                if hold_buffers {
                    while let Some(buffer) = world.station.buffers.take() {
                        buffers.push(buffer);
                    }
                } else {
                    while let Some(ticket) = world.station.tickets.take() {
                        tickets.push(ticket);
                    }
                }
                let mut qos = [StaSequenceCounter::new(SequenceNumber::ZERO); 8];
                fn encode<'id>(
                    station: &mut Station<'id, Tokens, Caps>,
                    qos: &mut [StaSequenceCounter; 8],
                    encoding: u8,
                ) -> Option<StaTransmit<'id, Tokens, Caps>> {
                    match encoding {
                        0 => station
                            .encode_eapol(
                                oer_ieee80211_mac::address::Bssid::new(
                                    oer_ieee80211_mac::address::IndividualAddress::new(BSSID)
                                        .unwrap(),
                                )
                                .unwrap(),
                                &[],
                                TxPurpose::HandshakeResponse,
                            )
                            .ok(),
                        1 => station
                            .encode_null(
                                oer_ieee80211_mac::address::Bssid::new(
                                    oer_ieee80211_mac::address::IndividualAddress::new(BSSID)
                                        .unwrap(),
                                )
                                .unwrap(),
                                StaPowerManagement::Active,
                            )
                            .ok(),
                        2 => station
                            .encode_protected_eapol(
                                oer_ieee80211_mac::address::Bssid::new(
                                    oer_ieee80211_mac::address::IndividualAddress::new(BSSID)
                                        .unwrap(),
                                )
                                .unwrap(),
                                None,
                                &[],
                                TxPurpose::Connected(Sending::GroupResponse),
                            )
                            .ok(),
                        3 => station
                            .encode_protected_eapol(
                                oer_ieee80211_mac::address::Bssid::new(
                                    oer_ieee80211_mac::address::IndividualAddress::new(BSSID)
                                        .unwrap(),
                                )
                                .unwrap(),
                                Some(qos),
                                &[],
                                TxPurpose::Connected(Sending::GroupResponse),
                            )
                            .ok(),
                        4 => station.encode_probe(TxPurpose::Probe).ok(),
                        5 | 6 => station
                            .encode_management(
                                StaManagementSubtype::Deauthentication,
                                oer_ieee80211_mac::address::Bssid::new(
                                    oer_ieee80211_mac::address::IndividualAddress::new(BSSID)
                                        .unwrap(),
                                )
                                .unwrap(),
                                &REASON_UNSPECIFIED.to_le_bytes(),
                                TxPurpose::Deauthentication,
                                encoding == 6,
                            )
                            .ok(),
                        _ => unreachable!(),
                    }
                }
                let before = world.station.sequence;
                let qos_before = qos;
                for _ in 0..2 {
                    assert!(encode(&mut world.station, &mut qos, encoding).is_none());
                    assert_eq!(world.station.sequence, before, "encoding {encoding}");
                    assert_eq!(qos, qos_before, "encoding {encoding}");
                    assert_eq!(
                        world.station.tickets.len(),
                        if hold_buffers { 2 } else { 0 }
                    );
                    assert_eq!(
                        world.station.buffers.len(),
                        if hold_buffers { 0 } else { 2 }
                    );
                }
                for ticket in tickets {
                    world.station.tickets.put(ticket);
                }
                for buffer in buffers {
                    world.station.buffers.put(buffer);
                }
                let transmit = encode(&mut world.station, &mut qos, encoding).unwrap();
                let mut expected = before;
                let mut expected_qos = qos_before;
                if encoding == 3 {
                    expected_qos[7].take();
                } else {
                    expected.take();
                }
                assert_eq!(world.station.sequence, expected);
                assert_eq!(qos, expected_qos);
                world.transmitted(transmit, matches!(encoding, 2 | 3 | 6));
                world.end_transmissions(at(50));
                assert_eq!(world.station.tickets.len(), 2);
                assert_eq!(world.station.buffers.len(), 2);
            });
        }
    }
}

#[test]
fn unsuccessful_scan_disables_reception_before_reporting() {
    for unsupported_channel in [false, true] {
        installed(|mint| {
            let mut world = World::new(mint, 3);
            world
                .station
                .command(at(0), StaRequest::new(1, StaCommand::Connect))
                .unwrap();
            world.run(at(0));
            if unsupported_channel {
                world.receive(at(1), &beacon_on(3));
            }
            world.advance();
            world.advance();
            let seen = world.take();
            let off = seen
                .iter()
                .position(|event| *event == Seen::Configure("off"));
            let reported = seen.iter().position(|event| {
                matches!(
                    event,
                    Seen::Notify(StaNotice::Disconnected(
                        StaDisconnectReason::NotFound { .. }
                    ))
                )
            });
            assert!(off.is_some(), "scan reception must be disabled: {seen:?}");
            assert!(off < reported, "Off precedes NotFound: {seen:?}");
            assert!(matches!(world.station.state, State::Idle));
            assert!(world.station.poll_action(at(100)).is_none());
        });
    }
}

#[test]
fn refused_scan_disables_reception_before_reporting() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        let Some(StaAction::RequestChannel(_, place)) = world.station.poll_action(at(0)) else {
            panic!("the scan requests its first channel");
        };
        world.station.input(
            at(0),
            StaInput::Answer(Answer::Refused(Refusal::Unservable, place)),
        );
        let Some(StaAction::Configure(StationReceive::Off, pending)) =
            world.station.poll_action(at(0))
        else {
            panic!("a refused scan still disables reception before reporting");
        };
        assert!(world.station.poll_action(at(0)).is_none());
        world.station.input(at(0), StaInput::Configured(pending));
        let Some(StaAction::CommandCompleted(terminal)) = world.station.poll_action(at(0)) else {
            panic!()
        };
        assert_eq!(
            terminal.outcome,
            StaCommandOutcome::ConnectFailed(StaDisconnectReason::Refused(Refusal::Unservable))
        );
        assert!(matches!(
            world.station.poll_action(at(0)),
            Some(StaAction::Notify(StaNotice::Disconnected(
                StaDisconnectReason::Refused(Refusal::Unservable)
            )))
        ));
        assert!(world.station.poll_action(at(0)).is_none());
    });
}

#[test]
fn a_lease_terminal_during_leave_preserves_deauthentication_and_configuration() {
    for protected in [false, true] {
        installed(|mint| {
            let mut world = if protected {
                wpa2_connected::<2>(mint).0
            } else {
                connected_world(mint)
            };
            world
                .station
                .command(at(50), StaRequest::new(2, StaCommand::Disconnect))
                .unwrap();
            // The lease terminal races the existing Leave, before the key
            // removals or the deauthentication have been taken as actions.
            let leaving = world.station.take_state();
            world.station.state = world.station.lease_ended(leaving, false);
            world.run(at(50));
            let seen = world.take();
            let sent = seen.iter().position(|event| {
                matches!(
                    event,
                    Seen::Transmit {
                        subtype: 0x00c0,
                        ..
                    }
                )
            });
            let off = seen
                .iter()
                .position(|event| *event == Seen::Configure("off"));
            let reported = seen.iter().position(|event| {
                matches!(
                    event,
                    Seen::Notify(StaNotice::Disconnected(StaDisconnectReason::Requested))
                )
            });
            assert!(
                sent.is_some(),
                "the leave must retain its deauthentication: {seen:?}"
            );
            assert!(
                sent < off && off < reported,
                "deauthentication, Off, notice: {seen:?}"
            );
            assert!(world.station.keys.clear());
            assert_eq!(world.station.tickets.len(), 2);
            assert_eq!(world.station.buffers.len(), 2);
        });
    }
}

/// The terminal kinds the station's tests return.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum End {
    Delivered,
    AckTimeout,
    ContentionLimit,
}

/// A terminal on the tests' port, whose ACK evidence is empty.
type Outcome = oer_ieee80211_lower_mac::SingleOutcome<(), ()>;

fn ended(status: End) -> Outcome {
    let air = oer_ieee80211_lower_mac::Air {
        transmissions: NonZeroU16::MIN,
        rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
    };
    TxOutcome::Ended(match status {
        End::Delivered => SingleEnd::Delivered {
            air,
            response: oer_ieee80211_lower_mac::SingleResponse::Ack(()),
        },
        End::AckTimeout => SingleEnd::AckTimeout(air),
        End::ContentionLimit => SingleEnd::ContentionLimit(Some(air)),
    })
}

fn retry_message3(ptk: &oer_ieee80211_rsn::Ptk, counter: u64, material: u8) -> Vec<u8> {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::frames::RsnTxFrame;
    let mut plain = Vec::from(RSN);
    plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
    plain.extend_from_slice(&[material; 16]);
    plain.extend_from_slice(&[0xdd, 0]);
    let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
    let message =
        RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, counter, ANONCE, [0; 8], wrapped.as_bytes())
            .unwrap()
            .authenticate(ptk);
    eapol_frame(message.as_bytes())
}

#[test]
fn connected_duplicate_m3_answers_without_reinstalling_or_resetting_replay() {
    use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpPacketNumber, CcmpRxProtection};
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let header = CcmpHeader::new(
            CcmpPacketNumber::new(900).unwrap(),
            GroupKeyId::new(1).unwrap().key_id(),
        );
        if let State::Connected(Connected { rsn: Some(rsn), .. }) = &mut world.station.state {
            rsn.replay
                .check(CcmpRxProtection::Group, None, header)
                .unwrap();
        } else {
            panic!("protected connected station");
        }
        let before = world.frames.len();
        world.receive(at(50), &retry_message3(&ptk, 2, 0x99));
        assert!(world.station.is_connected());
        assert_eq!(world.frames.len(), before + 1);
        assert_eq!(world.eapol.last(), Some(&(false, false)));
        assert!(!world.take().iter().any(|event| matches!(
            event,
            Seen::InstallPairwise(_)
                | Seen::InstallGroup(_)
                | Seen::ReplaceGroup(_)
                | Seen::RemoveKey
        )));
        let State::Connected(Connected { rsn: Some(rsn), .. }) = &mut world.station.state else {
            panic!()
        };
        assert_eq!(rsn.supplicant.replay_counter(), 2);
        assert!(
            rsn.replay
                .check(CcmpRxProtection::Group, None, header)
                .is_err()
        );
    });
}

#[test]
fn duplicate_m3_waits_for_credit_and_coalesces_across_group_key_installation() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.hold_key_installs = true;
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        let first = world.station.tickets.take().unwrap();
        let second = world.station.tickets.take().unwrap();
        let before = world.frames.len();
        for _ in 0..3 {
            world.receive(at(51), &retry_message3(&ptk, 2, 0x99));
        }
        assert_eq!(world.frames.len(), before);
        world
            .station
            .input(at(52), world.pending_installs.pop().unwrap());
        world.station.tickets.put(first);
        let Some(StaAction::Transmit(transmit)) = world.station.poll_action(at(52)) else {
            panic!()
        };
        assert_eq!(
            transmit.context.purpose,
            TxPurpose::Connected(Sending::DuplicateMessage4)
        );
        world.transmitted(transmit, false);
        // The same authenticated request while its response is in flight
        // cannot displace the group response or grow a queue.
        world
            .station
            .receive(at(52), &retry_message3(&ptk, 2, 0x99), meta(), |_| {})
            .unwrap();
        assert!(world.station.poll_action(at(52)).is_none());
        world.end_transmissions(at(53));
        let Some(StaAction::TransmitProtected { transmit, .. }) = world.station.poll_action(at(53))
        else {
            panic!()
        };
        assert_eq!(
            transmit.context.purpose,
            TxPurpose::Connected(Sending::GroupResponse)
        );
        world.transmitted(transmit, true);
        world.station.tickets.put(second);
        world.run(at(53));
        assert_eq!(world.frames.len(), before + 2);
        assert!(world.station.is_connected());
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
    });
}

#[test]
fn duplicate_m3_survives_an_in_flight_group_response() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        world.hold_key_installs = true;
        world.receive_verified(at(50), &group_message1(&ptk, 3, 2));
        world
            .station
            .input(at(51), world.pending_installs.pop().unwrap());
        let Some(StaAction::TransmitProtected { transmit, .. }) = world.station.poll_action(at(51))
        else {
            panic!()
        };
        assert_eq!(
            transmit.context.purpose,
            TxPurpose::Connected(Sending::GroupResponse)
        );
        world.transmitted(transmit, true);
        let before = world.frames.len();
        world
            .station
            .receive(at(51), &retry_message3(&ptk, 2, 0x99), meta(), |_| {})
            .unwrap();
        assert!(world.station.poll_action(at(51)).is_none());
        world.end_transmissions(at(52));
        let Some(StaAction::Transmit(transmit)) = world.station.poll_action(at(52)) else {
            panic!()
        };
        assert_eq!(
            transmit.context.purpose,
            TxPurpose::Connected(Sending::DuplicateMessage4)
        );
        world.transmitted(transmit, false);
        world.run(at(52));
        assert_eq!(world.frames.len(), before + 1);
        assert!(world.station.is_connected());
    });
}

#[test]
fn connected_eapol_rejects_changed_m3_and_unprotected_group_message1() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let before = world.frames.len();
        let mut forged = retry_message3(&ptk, 2, 0x99);
        *forged.last_mut().unwrap() ^= 1;
        let mut unprotected_group = group_message1(&ptk, 3, 2);
        unprotected_group[1] &= !0x40;
        unprotected_group.drain(24..32);
        for (frame, reason) in [
            (forged, RsnConnectedSupplicantError::InvalidMic),
            (
                retry_message3(&ptk, 1, 0x99),
                RsnConnectedSupplicantError::StaleReplayCounter,
            ),
            (
                retry_message3(&ptk, 2, 0x55),
                RsnConnectedSupplicantError::RetainedMessage3Mismatch,
            ),
            (
                unprotected_group,
                RsnConnectedSupplicantError::UnsupportedMessage,
            ),
        ] {
            assert_eq!(
                world.station.receive(at(50), &frame, meta(), |_| {}),
                Err(StaRxError::Security(RsnConnectedProcessError::Supplicant(
                    reason
                )))
            );
            world.run(at(50));
            assert!(world.station.is_connected());
            assert!(world.take().is_empty());
        }
        assert_eq!(world.frames.len(), before);
    });
}

#[test]
fn only_delivered_m4_connects_and_other_terminals_keep_the_original_deadline() {
    use oer_ieee80211_lower_mac::{IndeterminateCause, WithdrawCause};
    for (outcome, published) in [
        (ended(End::Delivered), true),
        (ended(End::AckTimeout), true),
        (ended(End::ContentionLimit), true),
        (TxOutcome::Withdrawn(WithdrawCause::GateClosed), false),
        (TxOutcome::Withdrawn(WithdrawCause::GateClosed), true),
        (
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
            true,
        ),
        (TxOutcome::Withdrawn(WithdrawCause::Disabled), false),
    ] {
        installed(|mint| {
            let (mut world, ptk) = wpa2_associated::<2>(mint);
            let deadline = world.station.poll_deadline().unwrap();
            world.outcomes.push_back((outcome, published));
            world.receive(at(33), &retry_message3(&ptk, 2, 0x99));
            assert_eq!(world.station.tickets.len(), 2);
            assert_eq!(world.station.buffers.len(), 2);
            if outcome == ended(End::Delivered) {
                assert!(world.station.is_connected());
                return;
            }
            assert!(!world.station.is_connected(), "{outcome:?}");
            assert!(
                !world
                    .seen
                    .iter()
                    .any(|s| matches!(s, Seen::Notify(StaNotice::Connected { .. })))
            );
            if outcome == TxOutcome::Withdrawn(WithdrawCause::Disabled) {
                assert!(matches!(
                    world.station.state,
                    State::Leave(Leave {
                        step: LeaveStep::Disabled,
                        ..
                    })
                ));
                assert!(world.station.poll_action(at(34)).is_none());
                let (bank, _places) = world.station.finish(StaDisconnectReason::Requested);
                assert_eq!(bank.frame_credits(), (2, 2));
                assert_eq!(bank.key_owners(), (1, 2, true));
                return;
            }
            assert_eq!(world.station.poll_deadline(), Some(deadline));
            let first = world.take();
            assert_eq!(
                first
                    .iter()
                    .filter(|s| matches!(s, Seen::InstallPairwise(_)))
                    .count(),
                1
            );
            assert_eq!(
                first
                    .iter()
                    .filter(|s| matches!(s, Seen::InstallGroup(_)))
                    .count(),
                1
            );
            world.run(at(34));
            assert!(world.take().is_empty());
            if matches!(outcome, TxOutcome::Indeterminate(_)) {
                world.station.input(deadline, StaInput::Tick);
                world.run(deadline);
                assert_eq!(
                    world
                        .take()
                        .iter()
                        .filter(|s| **s
                            == Seen::Notify(StaNotice::Disconnected(
                                StaDisconnectReason::HandshakeFailed
                            )))
                        .count(),
                    1
                );
                world.station.input(deadline, StaInput::Tick);
                world.run(deadline);
                assert!(world.take().is_empty());
                assert!(world.station.keys.clear());
            } else {
                let mut forged = retry_message3(&ptk, 100, 0x99);
                *forged.last_mut().unwrap() ^= 1;
                world.receive(at(34), &forged);
                assert_eq!(world.station.poll_deadline(), Some(deadline));
                assert!(world.take().is_empty());
                world.receive(at(35), &retry_message3(&ptk, 3, 0x99));
                assert!(world.station.is_connected());
                assert!(!world.take().iter().any(|s| matches!(
                    s,
                    Seen::InstallPairwise(_)
                        | Seen::InstallGroup(_)
                        | Seen::ReplaceGroup(_)
                        | Seen::RemoveKey
                )));
            }
            assert_eq!(world.station.tickets.len(), 2);
            assert_eq!(world.station.buffers.len(), 2);
        });
    }
}

#[test]
fn a_repeated_m3_can_replace_group_material_without_reinstalling_the_ptk() {
    fn exercise<const G: usize>() {
        installed(|mint| {
            let (mut world, ptk) = wpa2_associated::<G>(mint);
            world.outcomes.push_back((ended(End::AckTimeout), true));
            world.receive(at(33), &retry_message3(&ptk, 2, 0x99));
            assert!(!world.station.is_connected());
            assert_eq!(
                world
                    .take()
                    .iter()
                    .filter(|s| matches!(s, Seen::InstallPairwise(_)))
                    .count(),
                1
            );
            world.receive(at(34), &retry_message3(&ptk, 3, 0x55));
            assert!(world.station.is_connected());
            let seen = world.take();
            assert!(!seen.iter().any(|s| matches!(s, Seen::InstallPairwise(_))));
            if G == 1 {
                assert_eq!(
                    seen.iter()
                        .filter(|s| matches!(s, Seen::ReplaceGroup(1)))
                        .count(),
                    1
                );
                assert!(!seen.contains(&Seen::RemoveKey));
            } else {
                assert!(seen.iter().any(|s| matches!(s, Seen::InstallGroup(1))));
                assert!(!seen.contains(&Seen::RemoveKey));
                assert!(
                    world
                        .station
                        .keys
                        .groups
                        .same_material(GroupKeyId::new(1).unwrap(), &[0x99; 16])
                );
            }
            world
                .station
                .command(at(35), StaRequest::new(2, StaCommand::Disconnect))
                .unwrap();
            world.run(at(35));
            assert!(world.station.keys.clear());
            assert_eq!(world.station.tickets.len(), 2);
            assert_eq!(world.station.buffers.len(), 2);
        });
    }
    exercise::<1>();
    exercise::<2>();
}

#[test]
fn probe_encoding_failure_returns_credit_and_missing_epoch_reports_link_change() {
    installed(|mint| {
        let mut world = World::new(mint, 3);
        world
            .station
            .command(at(0), StaRequest::new(1, StaCommand::Connect))
            .unwrap();
        let State::Scan(scan) = &mut world.station.state else {
            panic!("scan");
        };
        scan.step = ScanStep::NeedProbe;
        let mut buffers = Vec::new();
        while let Some(mut buffer) = world.station.buffers.take() {
            buffer.1 = 0;
            buffers.push(buffer);
        }
        for buffer in buffers {
            world.station.buffers.put(buffer);
        }
        assert!(matches!(
            world.station.poll_action(at(0)),
            Some(StaAction::Configure(StationReceive::Off, _))
        ));
        assert_eq!(world.station.tickets.len(), 2);
        assert_eq!(world.station.buffers.len(), 2);
        world.station.epoch = None;
        assert!(matches!(
            world.station.encode_probe(TxPurpose::Probe),
            Err(Unsent::NoConnection)
        ));
        let state = world.station.take_state();
        let State::Leave(leave) = state else {
            panic!("permanent encoding failure leaves");
        };
        assert_eq!(leave.reason, StaDisconnectReason::Encode);
    });
}

#[test]
fn pending_group_material_is_redacted_and_moves_into_the_installed_record() {
    installed(|_mint| {
        let mut keys = Keys::<Tokens<1>>::new::<1>((Slot(0), [Slot(1)]), GroupPermission);
        let mut epochs = LinkEpochSource::for_test(0);
        let id = GroupKeyId::new(1).unwrap();
        let material = Ccmp128Key::new(&[173; 16]);
        let slot = keys.group_slots.free.take().unwrap();
        let pending = PendingGroupKey {
            epoch: epochs.begin(),
            key_id: id,
            material: Ccmp128Key::new(material.as_bytes()),
            place: keys.groups.reserve().unwrap(),
            replay: GroupReplay::Retained(
                oer_ieee80211_mac::ccmp::CcmpRxReplayState::from_receive_sequence([0; 8]).unwrap(),
            ),
        };
        let printed = std::format!("{pending:?}");
        assert!(printed.contains("Ccmp128Key(..)"));
        assert!(!printed.contains("173"));
        let permission = keys.group_permission.take().unwrap();
        keys.group_installed(pending, Key(slot), permission, true, None, false);
        assert!(keys.groups.same_material(id, material.as_bytes()));
        let (permission, entry) = keys.take_group_key(false).unwrap();
        assert!(!keys.groups.same_material(id, material.as_bytes()));
        keys.group_removed(entry.key.0, permission, entry.place);
        assert!(keys.clear());
    });
}

#[test]
fn repeated_m3_selects_retained_material_and_restores_each_live_replay_frontier() {
    use oer_ieee80211_mac::ccmp::{CcmpHeader, CcmpPacketNumber, CcmpRxProtection};
    installed(|mint| {
        let (mut world, ptk) = wpa2_associated::<2>(mint);
        let id = GroupKeyId::new(1).unwrap();
        let header = |pn| CcmpHeader::new(CcmpPacketNumber::new(pn).unwrap(), id.key_id());
        let lanes = core::iter::once(None)
            .chain((0..16).map(Some))
            .collect::<Vec<_>>();
        world.outcomes.push_back((ended(End::AckTimeout), true));
        world.receive(at(33), &retry_message3(&ptk, 2, 0x99));
        for &tid in &lanes {
            active_replay(&mut world.station.state)
                .unwrap()
                .check(CcmpRxProtection::Group, tid, header(500))
                .unwrap();
        }
        world.hold_key_installs = true;
        world.outcomes.push_back((ended(End::AckTimeout), true));
        world.receive(at(34), &retry_message3(&ptk, 3, 0x55));
        assert_eq!(world.pending_installs.len(), 1);
        assert!(world.station.keys.groups.current_material(id, &[0x99; 16]));
        // RX A after install(B) was admitted but before its terminal.
        for &tid in &lanes {
            active_replay(&mut world.station.state)
                .unwrap()
                .check(CcmpRxProtection::Group, tid, header(700))
                .unwrap();
        }
        world
            .station
            .input(at(35), world.pending_installs.pop().unwrap());
        world.run(at(35));
        assert!(!world.station.is_connected());
        assert!(world.station.keys.groups.current_material(id, &[0x55; 16]));
        for &tid in &lanes {
            active_replay(&mut world.station.state)
                .unwrap()
                .check(CcmpRxProtection::Group, tid, header(1))
                .unwrap();
        }
        world.take();
        world.receive(at(36), &retry_message3(&ptk, 4, 0x99));
        assert_eq!(world.pending_installs.len(), 1);
        assert_eq!(
            world
                .seen
                .iter()
                .filter(|seen| **seen == Seen::SelectGroup)
                .count(),
            1
        );
        assert!(!world.seen.iter().any(|seen| matches!(
            seen,
            Seen::InstallGroup(_) | Seen::ReplaceGroup(_) | Seen::InstallPairwise(_)
        )));
        assert!(world.station.keys.groups.current_material(id, &[0x55; 16]));
        // RX B still advances B while select(A) waits. A must retain 700.
        for &tid in &lanes {
            active_replay(&mut world.station.state)
                .unwrap()
                .check(CcmpRxProtection::Group, tid, header(900))
                .unwrap();
        }
        world
            .station
            .input(at(37), world.pending_installs.pop().unwrap());
        world.run(at(37));
        assert!(world.station.is_connected());
        assert!(world.station.keys.groups.current_material(id, &[0x99; 16]));
        assert!(world.station.keys.groups.same_material(id, &[0x55; 16]));
        for &tid in &lanes {
            let replay = active_replay(&mut world.station.state).unwrap();
            assert!(
                replay
                    .check(CcmpRxProtection::Group, tid, header(700))
                    .is_err()
            );
            replay
                .check(CcmpRxProtection::Group, tid, header(701))
                .unwrap();
        }
        assert!(world.station.keys.group_permission.is_some());
    });
}

fn take_group_removal<'id>(
    world: &mut World<'id>,
    ptk: &oer_ieee80211_rsn::Ptk,
    counter: u64,
    key_id: u8,
    now: Instant,
) -> (PendingGroupRemoval<'id>, Slot, GroupPermission) {
    world.hold_key_installs = true;
    world.receive_verified(now, &group_message1(ptk, counter, key_id));
    assert_eq!(world.pending_installs.len(), 1);
    world
        .station
        .input(now, world.pending_installs.pop().unwrap());
    let Some(StaAction::TransmitProtected { transmit, .. }) = world.station.poll_action(now) else {
        panic!("the installed GTK owes group message 2");
    };
    world.transmitted(transmit, true);
    world.end_transmissions(now);
    assert!(
        world.station.poll_action(now).is_none(),
        "GM2 retains old installations"
    );
    let Some(Owed::RemoveGroup {
        pending,
        key: Key(slot),
        permission,
    }) = world.station.group_removal(true)
    else {
        panic!("the ledger lends a non-current installation for explicit removal");
    };
    (pending, slot, permission)
}

#[test]
fn each_removal_preserves_its_stations_current_replay() {
    installed(|mint_a| {
        let (mut a, ptk_a) = wpa2_connected::<2>(mint_a);
        // A first completed one rekey: its next removal owns a different
        // private record place and wire KeyID from B's first removal.
        a.receive_verified(at(50), &group_message1(&ptk_a, 3, 2));
        installed(|mint_b| {
            let (mut b, ptk_b) = wpa2_connected::<2>(mint_b);
            let (pending_a, slot_a, permission_a) =
                take_group_removal(&mut a, &ptk_a, 4, 3, at(51));
            let (pending_b, slot_b, permission_b) =
                take_group_removal(&mut b, &ptk_b, 3, 2, at(51));
            let current_b = GroupKeyId::new(2).unwrap();
            assert!(
                active_replay(&mut b.station.state)
                    .unwrap()
                    .has_group(current_b)
            );
            // Every physical slot and permission returns to its own Station.
            // Each private pending token also returns to its original station.
            a.station.input(
                at(52),
                StaInput::GroupKeyRemoved {
                    pending: pending_a,
                    slot: slot_a,
                    permission: permission_a,
                },
            );
            b.station.input(
                at(52),
                StaInput::GroupKeyRemoved {
                    pending: pending_b,
                    slot: slot_b,
                    permission: permission_b,
                },
            );
            assert!(
                b.station
                    .keys
                    .groups
                    .current_material(current_b, &[0x77; 16])
            );
            assert!(
                active_replay(&mut b.station.state)
                    .unwrap()
                    .has_group(current_b),
                "returning a retiring record preserves current replay"
            );
        });
    });
}

#[test]
fn returned_removal_places_are_reused_without_losing_installed_slots() {
    installed(|mint_a| {
        let (mut a, ptk_a) = wpa2_connected::<2>(mint_a);
        a.receive_verified(at(50), &group_message1(&ptk_a, 3, 2));
        installed(|mint_b| {
            let (mut b, ptk_b) = wpa2_connected::<2>(mint_b);
            let (pending_a, slot_a, permission_a) =
                take_group_removal(&mut a, &ptk_a, 4, 3, at(51));
            let (pending_b, slot_b, permission_b) =
                take_group_removal(&mut b, &ptk_b, 3, 2, at(51));
            a.station.input(
                at(52),
                StaInput::GroupKeyRemoved {
                    pending: pending_a,
                    slot: slot_a,
                    permission: permission_a,
                },
            );
            b.station.input(
                at(52),
                StaInput::GroupKeyRemoved {
                    pending: pending_b,
                    slot: slot_b,
                    permission: permission_b,
                },
            );
            // A subsequent installation reuses the returned vacant place without
            // overwriting the still-current record or losing its slot.
            a.hold_key_installs = false;
            a.receive_verified(at(53), &group_message1(&ptk_a, 5, 0));
            a.station
                .command(at(54), StaRequest::new(2, StaCommand::Disconnect))
                .unwrap();
            a.run(at(54));
            assert!(
                a.station.keys.clear(),
                "every physical key slot returns before Leave completes"
            );
            assert!(matches!(a.station.state, State::Idle));
        });
    });
}

#[test]
fn a_removal_issued_before_leave_returns_its_record_and_permission_after_leave() {
    installed(|mint| {
        let (mut world, ptk) = wpa2_connected::<2>(mint);
        let (pending, slot, permission) = take_group_removal(&mut world, &ptk, 3, 2, at(51));
        world
            .station
            .command(at(52), StaRequest::new(2, StaCommand::Disconnect))
            .unwrap();
        world.run(at(52));
        assert!(matches!(world.station.state, State::Leave(_)));
        world.station.input(
            at(53),
            StaInput::GroupKeyRemoved {
                pending,
                slot,
                permission,
            },
        );
        world.run(at(53));
        assert!(world.station.keys.clear());
        assert!(matches!(world.station.state, State::Idle));
    });
}

#[test]
fn delivered_m4_retains_all_installed_groups_until_leave() {
    fn exercise<const G: usize>(leave: bool) {
        use oer_ieee80211_mac::security::rsn::Akm;
        use oer_ieee80211_rsn::{PtkContext, frames::RsnTxFrame};
        installed(|mint| {
            let mut world = World::with_groups(
                mint,
                3,
                StaSecurity::Personal {
                    password: oer_ieee80211_rsn::sae::SaePassword::new(b"password").unwrap(),
                    pmk: Pmk::from_bytes(PMK),
                },
                core::array::from_fn::<_, G, _>(|i| Slot(u8::try_from(i + 1).unwrap())),
            );
            world
                .station
                .command(at(0), StaRequest::new(1, StaCommand::Connect))
                .unwrap();
            world.run(at(0));
            world.advance();
            world.receive(at(13), &wpa2_beacon());
            world.advance();
            world.answer_authentication(at(30), 0);
            world.receive(at(31), &association_response(0));
            let message1 = RsnTxFrame::<512>::message1(Akm::Psk, LOCAL, 1, ANONCE).unwrap();
            world.receive(at(32), &eapol_frame(message1.as_bytes()));
            let ptk = Pmk::from_bytes(PMK).derive_ptk(
                Akm::Psk,
                PtkContext {
                    authenticator_address: BSSID,
                    supplicant_address: LOCAL,
                    authenticator_nonce: ANONCE,
                    supplicant_nonce: SNONCE,
                },
            );
            world.take();
            world.hold_group_removals = true;
            for index in 0..G {
                if index + 1 < G {
                    world.outcomes.push_back((ended(End::AckTimeout), true));
                }
                world.receive(
                    at(33),
                    &retry_message3(
                        &ptk,
                        u64::try_from(index + 2).unwrap(),
                        u8::try_from(index + 1).unwrap(),
                    ),
                );
                assert_eq!(world.station.is_connected(), index + 1 == G);
            }
            assert!(world.pending_removals.is_empty());
            assert!(world.station.keys.group_permission.is_some());
            assert_eq!(world.station.keys.group_slots.free.len(), 0);
            assert_eq!(world.station.keys.groups.len(), G);
            if leave {
                world
                    .station
                    .command(at(34), StaRequest::new(2, StaCommand::Disconnect))
                    .unwrap();
                world.run(at(34));
                for completed in 1..=G {
                    assert_eq!(world.pending_removals.len(), 1);
                    let terminal = world.pending_removals.pop().unwrap();
                    world.station.input(at(35), terminal);
                    world.run(at(35));
                    assert_eq!(world.station.keys.group_slots.free.len(), completed);
                }
            }
            assert!(world.pending_removals.is_empty());
            assert!(world.station.keys.group_permission.is_some());
            if leave {
                assert!(world.station.keys.clear());
                assert!(matches!(world.station.state, State::Idle));
            } else {
                assert!(world.station.keys.groups.current_material(
                    GroupKeyId::new(1).unwrap(),
                    &[u8::try_from(G).unwrap(); 16]
                ));
                assert!(matches!(
                    world.station.state,
                    State::Connected(Connected {
                        rsn: Some(ConnectedRsn {
                            rekey: Rekey::None,
                            ..
                        }),
                        ..
                    })
                ));
            }
        });
    }
    for leave in [false, true] {
        exercise::<3>(leave);
        exercise::<4>(leave);
    }
}

#[test]
fn previous_auth_terminal_cannot_finish_new_attempt() {
    use oer_ieee80211_lower_mac::{IndeterminateCause, WithdrawCause};
    for outcome in [
        Some(ended(End::Delivered)),
        Some(ended(End::AckTimeout)),
        Some(TxOutcome::Withdrawn(WithdrawCause::GateClosed)),
        Some(TxOutcome::Indeterminate(
            IndeterminateCause::HardwareTimeout,
        )),
        None, // A delayed refusal before admission also belongs to attempt 1.
    ] {
        for rx_first in [false, true] {
            installed(|mint| {
                let mut world = World::with_pools::<2, 1>(mint, 3, StaSecurity::Open, [Slot(1)]);
                world
                    .station
                    .command(at(0), StaRequest::new(92, StaCommand::Connect))
                    .unwrap();
                world.run(at(0));
                world.advance();
                world.receive(at(13), &beacon());
                world.hold_configured = true;
                let now = world.advance();
                let pending = world.configuring.take().unwrap();
                world.station.input(now, StaInput::Configured(pending));
                let Some(StaAction::Transmit(first_auth)) = world.station.poll_action(now) else {
                    panic!("first authentication");
                };
                let mut peer_reset = [0; 26];
                peer_reset[..2].copy_from_slice(&0x00c0_u16.to_le_bytes());
                peer_reset[4..10].copy_from_slice(&LOCAL);
                peer_reset[10..16].copy_from_slice(&BSSID);
                peer_reset[16..22].copy_from_slice(&BSSID);
                peer_reset[24..26].copy_from_slice(&7_u16.to_le_bytes());
                world
                    .station
                    .receive(now, &peer_reset, meta(), |_| {})
                    .unwrap();
                let Some(StaAction::Transmit(second_auth)) = world.station.poll_action(now) else {
                    panic!("retry authentication");
                };
                assert_eq!(world.station.tickets.len(), 0);
                assert_eq!(world.station.buffers.len(), 0);
                world.transmitted(second_auth, false);
                let response = answer_authentication(world.frames.last().unwrap(), 0).unwrap();
                if rx_first {
                    world
                        .station
                        .receive(now, &response, meta(), |_| {})
                        .unwrap();
                }
                let input = match outcome {
                    Some(completion) => StaInput::TxDone {
                        context: first_auth.context,
                        ticket: first_auth.ticket,
                        buffer: ReturnedMpdu::Fresh(first_auth.buffer),
                        completion,
                    },
                    None => StaInput::Unsent {
                        context: first_auth.context,
                        ticket: first_auth.ticket,
                        buffer: ReturnedMpdu::Fresh(first_auth.buffer),
                        reason: TxRefusal::NoRate,
                    },
                };
                world.station.input(now, input);
                assert_eq!(world.station.tickets.len(), 1);
                assert_eq!(world.station.buffers.len(), 1);
                if rx_first {
                    assert!(matches!(
                        &world.station.state,
                        State::Join(Join {
                            step: JoinStep::Associate(_, _),
                            ..
                        })
                    ));
                } else {
                    assert!(
                        matches!(
                            &world.station.state,
                            State::Join(Join {
                                step: JoinStep::Authenticate(Authenticate::Exchange {
                                    stage: AuthStage::AwaitSent { .. },
                                    ..
                                }),
                                ..
                            })
                        ),
                        "old {outcome:?} changed the second attempt"
                    );
                }
                // The current terminal returns the second owner's resources once.
                world.end_transmissions(now);
                assert_eq!(world.station.tickets.len(), 2);
                assert_eq!(world.station.buffers.len(), 2);
                if !rx_first {
                    world
                        .station
                        .receive(now, &response, meta(), |_| {})
                        .unwrap();
                }
                assert!(matches!(
                    &world.station.state,
                    State::Join(Join {
                        step: JoinStep::Associate(_, _),
                        ..
                    })
                ));
                assert!(world.completed.is_empty());
            });
        }
    }
}

#[test]
fn review_negotiated_pmf_rejects_unprotected_deauthentication() {
    use oer_ieee80211_mac::security::rsn::Akm;
    use oer_ieee80211_rsn::frames::RsnTxFrame;
    installed(|mint| {
        let mut advertised_rsn = RSN;
        advertised_rsn[20] |= 0x80;
        let mut advertised_beacon = wpa2_beacon();
        advertised_beacon[55..].copy_from_slice(&advertised_rsn);
        let (mut world, ptk) = wpa2_associated_by::<2>(mint, &advertised_beacon);
        match &world.station.state {
            State::Handshake(handshake) => assert!(handshake.supplicant.management_protection()),
            _ => panic!("expected handshake"),
        }
        let mut plain = Vec::from(advertised_rsn);
        plain.extend_from_slice(&[0xdd, 22, 0x00, 0x0f, 0xac, 0x01, 0x01, 0x00]);
        plain.extend_from_slice(&[0x99; 16]);
        plain.extend_from_slice(&[0xdd, 28, 0, 0x0f, 0xac, 9, 4, 0, 1, 0, 0, 0, 0, 0]);
        plain.extend_from_slice(&[0x77; 16]);
        // RFC3394 padding to an eight-octet boundary.
        while plain.len() % 8 != 0 {
            plain.push(if plain.len() == 76 { 0xdd } else { 0 });
        }
        let wrapped = oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
        let m3 =
            RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
                .unwrap()
                .authenticate(&ptk);
        world.receive(at(33), &eapol_frame(m3.as_bytes()));
        assert!(
            world.station.is_connected(),
            "PMF-capable peer completed handshake"
        );
        let mut deauthentication = [0; 26];
        deauthentication[..2].copy_from_slice(&0x00c0_u16.to_le_bytes());
        deauthentication[4..10].copy_from_slice(&LOCAL);
        deauthentication[10..16].copy_from_slice(&BSSID);
        deauthentication[16..22].copy_from_slice(&BSSID);
        deauthentication[24..26].copy_from_slice(&7_u16.to_le_bytes());
        world.receive(at(50), &deauthentication);
        assert!(
            world.station.is_connected(),
            "unprotected deauth must not tear down negotiated PMF association"
        );
    });
}

mod deadlines;
mod management;

#[test]
fn negotiated_spp_from_the_association_is_retained_by_the_ptk_owner() {
    use oer_ieee80211_mac::security::{
        Spp,
        rsn::{Akm, RSN_CAPABILITY_SPP_AMSDU_CAPABLE, RSN_CAPABILITY_SPP_AMSDU_REQUIRED},
    };
    use oer_ieee80211_rsn::frames::RsnTxFrame;
    for (capabilities, expected) in [
        (0, Spp::NotNegotiated),
        (RSN_CAPABILITY_SPP_AMSDU_CAPABLE, Spp::Negotiated),
        (
            RSN_CAPABILITY_SPP_AMSDU_CAPABLE | RSN_CAPABILITY_SPP_AMSDU_REQUIRED,
            Spp::Negotiated,
        ),
    ] {
        installed(|mint| {
            let mut rsn = RSN;
            rsn[20..22].copy_from_slice(&capabilities.to_le_bytes());
            let mut beacon = wpa2_beacon();
            beacon[55..].copy_from_slice(&rsn);
            let (mut world, ptk) = wpa2_associated_by::<2>(mint, &beacon);
            let mut plain = rsn.to_vec();
            plain.extend_from_slice(&[0xdd, 22, 0, 0x0f, 0xac, 1, 1, 0]);
            plain.extend_from_slice(&[0x99; 16]);
            plain.extend_from_slice(&[0xdd, 0]);
            let wrapped =
                oer_ieee80211_rsn::aes::software_aes128_key_wrap(ptk.kek(), &plain).unwrap();
            let message3 =
                RsnTxFrame::<512>::message3(Akm::Psk, LOCAL, 2, ANONCE, [0; 8], wrapped.as_bytes())
                    .unwrap()
                    .authenticate(&ptk);
            world.hold_key_installs = true;
            world.receive(at(33), &eapol_frame(message3.as_bytes()));
            assert_eq!(
                world.pending_installs.len(),
                1,
                "GTK waits for the PTK association owner"
            );
            let installed = world.pending_installs.pop().unwrap();
            let StaInput::PairwiseKeyInstalled { ref key, .. } = installed else {
                panic!("PTK installation");
            };
            assert_eq!(*Tokens::<2>::association(key), expected);
            world.hold_key_installs = false;
            world.station.input(at(33), installed);
            world.run(at(33));
            assert!(world.station.is_connected());
            world
                .station
                .command(at(34), StaRequest::new(2, StaCommand::Disconnect))
                .unwrap();
            world.run(at(34));
            assert!(
                world.station.keys.clear(),
                "the complete bank permits another association"
            );
        });
    }
}
