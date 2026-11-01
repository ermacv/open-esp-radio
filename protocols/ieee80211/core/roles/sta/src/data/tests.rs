//! The data machine's encapsulation, tokens and admission.

use super::*;
use crate::events::{DataLoss, StaEvent};
use core::num::NonZeroU16;
use oer_ieee80211_contracts_port::association::KEY_BANK_CAPACITY;
use oer_ieee80211_contracts_port::{
    BandSet, Channel, ChannelWidth, HardwareServices, Ieee80211Capabilities, LowerMacCapabilities,
    PhyFormatSet, RateSupport, ReceiveFilter, RxEvidence, WidthSet,
};
use oer_ieee80211_contracts_port::{BufferMemoryUnavailable, ResponseHeader};
use oer_ieee80211_contracts_port::{IndeterminateCause, WithdrawCause};
use oer_ieee80211_core_upper_mac::qos::AcAdmission;
use oer_ieee80211_core_upper_mac::sequence::SequenceCounter;
use oer_ieee80211_wire::data::MsduParts;
use oer_ieee80211_wire::phy::{HeMcs, HtMcs, LegacyRate, PhyRate};
use oer_ieee80211_wire::sequence::SequenceNumber;
use std::vec::Vec;
use {oer_ieee80211_core_upper_mac::ccmp::CcmpRxReplayEpoch, oer_ieee80211_wire::ccmp::GroupKeyId};

struct Caps;

/// The events each test's calls reported, in order.
#[derive(Default)]
struct Kept(Vec<StaEvent>);

impl EventSink<StaEvent> for Kept {
    fn event(&mut self, event: StaEvent) {
        self.0.push(event);
    }
}

fn losses(events: &Kept, reason: DataLoss) -> u32 {
    events
        .0
        .iter()
        .filter(|event| **event == StaEvent::DataLost(reason))
        .count() as u32
}

fn tokens_lost(events: &Kept) -> u32 {
    events
        .0
        .iter()
        .filter(|event| **event == StaEvent::TokenLost)
        .count() as u32
}

impl Ieee80211Capabilities for Caps {
    type SppScope = oer_ieee80211_wire::security::spp::Association;
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
        individual_no_ack: PhyFormatSet::NON_HT,
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        rx_amsdu_in_ampdu: true,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
        edca_max_txop_limit_units_32_us: u16::MAX,
    };
}

/// Test memory; `false` models memory that is not the station's.
struct Buffer(Vec<u8>, bool);
impl Buffer {
    fn new(bytes: Vec<u8>) -> Self {
        Self(bytes, true)
    }
}

impl oer_ieee80211_contracts_port::TxBufferClass for Buffer {
    const CAPACITY: usize = 1600;
}

impl TxBufferMemory for Buffer {
    fn renew_msdu(&mut self) -> Result<(), BufferMemoryUnavailable> {
        if self.1 {
            Ok(())
        } else {
            Err(BufferMemoryUnavailable)
        }
    }
    fn bytes_mut(&mut self) -> Result<&mut [u8], BufferMemoryUnavailable> {
        if self.1 {
            Ok(&mut self.0)
        } else {
            Err(BufferMemoryUnavailable)
        }
    }
}

fn memory(buffer: &ReturnedMpdu<Buffer, Buffer>) -> &[u8] {
    match buffer {
        ReturnedMpdu::Fresh(buffer) | ReturnedMpdu::Sent(buffer) => &buffer.0,
    }
}
fn fresh_memory(buffer: &mut ReturnedMpdu<Buffer, Buffer>) -> &mut [u8] {
    let ReturnedMpdu::Fresh(buffer) = buffer else {
        panic!("only fresh memory is mutable")
    };
    &mut buffer.0
}

struct Data<'id>(core::marker::PhantomData<fn(&'id ()) -> &'id ()>);

impl<'id> StaDataTokens for Data<'id> {
    type EpochPath = oer_radio_port::Root<oer_radio_port::LocalId<'id>>;
    type Class = Buffer;
    type Ticket = u8;
    type Buffer = Buffer;
    type Sent = Buffer;
    type AckEvidence = ();
    type AirDenied = ();
    fn renew(mpdu: Self::Sent) -> Buffer {
        mpdu
    }
}

struct Station<'id>(core::marker::PhantomData<fn(&'id ()) -> &'id ()>);

impl<'id> StaTokens for Station<'id> {
    type AssociationStart = ();
    type Association = ();
    type VacantKeyBank =
        oer_ieee80211_contracts_port::association::KeyBankParts<(), (), KEY_BANK_CAPACITY>;
    type VacantAssociation = [(); 0];
    fn begin_association(_: (), _: oer_ieee80211_wire::security::Spp) {}
    fn association(key: &()) -> &() {
        key
    }
    fn split_vacant(group: Self::VacantAssociation) -> ((), (), Self::VacantKeyBank) {
        (
            (),
            (),
            oer_ieee80211_contracts_port::association::KeyBankParts {
                pairwise: heapless::Vec::new(),
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
    const TOTAL_GROUP_SLOTS: usize = 0;
    const GROUP_SLOTS: usize = 0;
    type GatePath = oer_radio_port::Root<oer_radio_port::LocalId<'id>>;
    type EpochPath = oer_radio_port::Root<oer_radio_port::LocalId<'id>>;
    type ConfigurationPath = oer_radio_port::Root<oer_radio_port::LocalId<'id>>;
    type Small = Buffer;
    type Ticket = u8;
    type SmallBuffer = Buffer;
    type Sent = Buffer;
    type AckEvidence = ();
    type AirDenied = ();
    fn renew(mpdu: Self::Sent) -> Buffer {
        mpdu
    }
    type PairwiseSlot = ();
    type GroupSlot = ();
    type PairwiseKey = ();
    type GroupKey = ();
    type GroupPermission = ();
    type RxMeta = RxMeta;
}

const LOCAL: [u8; 6] = [2, 0, 0, 0, 0, 1];
const BSSID: [u8; 6] = [2, 0, 0, 0, 0, 2];
const PEER: [u8; 6] = [2, 0, 0, 0, 0, 3];

fn qos_link<'a>(
    key: &'a (),
    replay: &'a mut CcmpRxReplayEpoch,
) -> LinkView<'a, 'static, Station<'static>> {
    LinkView {
        epoch: LinkEpoch::for_test(1),
        address: oer_ieee80211_wire::address::OwnAddress::new(
            oer_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        association_id: 1,
        // The station's QoS sequence spaces, which the link lends.
        mode: LinkMode::Qos(std::boxed::Box::leak(std::boxed::Box::new(
            [SequenceCounter::new(SequenceNumber::ZERO); 8],
        ))),
        admission: AcAdmission::OPEN,
        protection: LinkProtection::Ccmp { key, replay },
    }
}

fn open_link(counter: &mut SequenceCounter) -> LinkView<'_, 'static, Station<'static>> {
    LinkView {
        epoch: LinkEpoch::for_test(1),
        address: oer_ieee80211_wire::address::OwnAddress::new(
            oer_ieee80211_wire::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: oer_ieee80211_wire::address::Bssid::new(
            oer_ieee80211_wire::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        association_id: 1,
        mode: LinkMode::NonQos(counter),
        admission: AcAdmission::OPEN,
        protection: LinkProtection::Open,
    }
}

fn machine() -> StaData<'static, Data<'static>, Caps, 8> {
    StaData::new(
        [1, 2],
        [
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    )
}

fn msdu(priority: u8) -> Msdu<'static> {
    Msdu {
        destination: PEER,
        ether_type: 0x0800,
        priority: WmmUserPriority::new(priority).unwrap(),
        payload: &[0xab; 100],
        admitted: oer_time::Instant::from_micros(1_000 + u64::from(priority)),
    }
}

fn epoch() -> CcmpRxReplayEpoch {
    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap()
}

fn transmit<const CAP: usize>(
    data: &mut StaData<'static, Data<'static>, Caps, CAP>,
) -> DataTransmit<'static, Data<'static>, Caps> {
    match data.poll_action() {
        Some(transmit) => transmit,
        None => panic!("an attempt is owed"),
    }
}

#[test]
fn stopped_data_bank_reuses_credits_permissions_and_the_order_frontier() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..2 {
        data.input(
            DataInput {
                msdu: msdu(6),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    let next_order = data.next_order;
    let outstanding = transmit(&mut data);
    assert_eq!(data.available(WmmAccessCategory::Voice), 1);
    let bank = data.finish(&mut events);
    assert_eq!(bank.available(WmmAccessCategory::Voice), 1);
    assert_eq!(losses(&events, DataLoss::LeftBehind), 1);
    let mut data = bank.restart();
    assert_eq!(data.next_order, next_order);
    assert_eq!(data.available(WmmAccessCategory::Voice), 1);
    assert_eq!(data.tickets.len(), 1);
    data.returned(
        DataReturn::TxDone {
            context: outstanding.context,
            ticket: outstanding.ticket,
            buffer: outstanding.buffer,
            completion: ended(End::Delivered, 1),
        },
        &mut events,
    );
    assert_eq!(data.available(WmmAccessCategory::Voice), 2);
    assert_eq!(data.tickets.len(), 2);
    assert_eq!(data.buffers.len(), 2);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let transmit = transmit(&mut data);
    assert_eq!(transmit.context.order, next_order);
    assert_eq!(tokens_lost(&events), 0);
}

#[test]
fn a_changed_link_refuses_data_without_reinterpreting_the_encoding_or_losing_resources() {
    for encoded_protected in [false, true] {
        for change_epoch in [false, true] {
            let mut events = Kept::default();
            let mut data = machine();
            let capacity = data.available(WmmAccessCategory::Voice);
            let mut replay = epoch();
            let mut link = qos_link(&(), &mut replay);
            if !encoded_protected {
                link.protection = LinkProtection::Open;
            }
            data.input(
                DataInput {
                    msdu: msdu(6),
                    link: &mut link,
                },
                &mut events,
            )
            .unwrap();
            let DataTransmitParts {
                context,
                ticket,
                buffer,
            } = transmit(&mut data).into_parts();
            let expected = if encoded_protected {
                DataProtection::Ccmp(&())
            } else {
                DataProtection::Open
            };
            assert_eq!(context.protection(&link), Ok(expected));
            let encoded = memory(&buffer).to_vec();
            let mut replacement_replay = epoch();
            if change_epoch {
                link.epoch = LinkEpoch::for_test(2);
            } else if encoded_protected {
                link.protection = LinkProtection::Open;
            } else {
                link.protection = LinkProtection::Ccmp {
                    key: &(),
                    replay: &mut replacement_replay,
                };
            }
            let reason = context.protection(&link).unwrap_err();
            assert_eq!(reason, crate::machine::TxRefusal::LinkChanged);
            assert_eq!(memory(&buffer), encoded);
            assert_eq!(data.available(WmmAccessCategory::Voice), capacity - 1);
            data.returned(
                DataReturn::Refused {
                    context,
                    ticket,
                    buffer,
                    reason,
                },
                &mut events,
            );
            assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
            // The service reported the refusal; the data machine adds no event.
            assert!(events.0.is_empty());
            assert!(data.poll_action().is_none());
            assert_eq!(core::iter::from_fn(|| data.tickets.take()).count(), 2);
            assert_eq!(core::iter::from_fn(|| data.buffers.take()).count(), 2);
        }
    }
}

#[test]
fn interleaved_returns_keep_the_original_tid_order_and_admission_count() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1, 2, 3, 4],
        core::array::from_fn::<_, 4, _>(|_| Buffer::new(std::vec![0; 1600])),
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for priority in [6, 4, 6, 7] {
        data.input(
            DataInput {
                msdu: msdu(priority),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    let mut operations: Vec<_> = (0..4).map(|_| transmit(&mut data)).collect();
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity - 3);
    assert_eq!(data.available(WmmAccessCategory::Video), capacity - 1);
    // Video may finish first. Voice returns in FIFO order, including two
    // frames of TID 6. Destroy the bytes that formerly identified a terminal.
    let video = operations.remove(1);
    operations.insert(0, video);
    for mut operation in operations {
        fresh_memory(&mut operation.buffer)[..26].fill(0);
        data.returned(
            DataReturn::TxDone {
                context: operation.context,
                ticket: operation.ticket,
                buffer: operation.buffer,
                completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
            },
            &mut events,
        );
    }
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert_eq!(data.available(WmmAccessCategory::Video), capacity);
    // Tickets belong to attempts: a requeued MPDU takes a free one afresh.
    for (tid, sequence) in [(6, 0), (4, 0), (6, 1), (7, 0)] {
        let operation = transmit(&mut data);
        assert_eq!(operation.context.tid.value(), tid);
        // A returned MSDU keeps the age it was admitted with.
        assert_eq!(
            operation.context.admitted(),
            oer_time::Instant::from_micros(1_000 + u64::from(tid))
        );
        assert_eq!(
            operation.context.frame.frame().len().get(),
            26 + 8 + 8 + 100
        );
        let spec = operation.context.frame().frame().header().header();
        let mut header = [0; 26];
        AckHeader::new(*spec).unwrap().encode(&mut header).unwrap();
        assert_eq!(tid_and_sequence(&header), (tid, sequence));
        data.returned(
            DataReturn::TxDone {
                context: operation.context,
                ticket: operation.ticket,
                buffer: operation.buffer,
                completion: ended(End::Delivered, 1),
            },
            &mut events,
        );
    }
    assert_eq!(tokens_lost(&events), 0);
    assert_eq!(losses(&events, DataLoss::Overflow), 0);
    assert!(data.accepts());
}

#[test]
fn repeated_unpublished_returns_keep_the_checked_frame_and_payload() {
    let mut events = Kept::default();
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let mut operation = transmit(&mut data);
    let len = operation.context.frame().frame().len().get();
    let payload = memory(&operation.buffer)[34..len].to_vec();
    for _ in 0..3 {
        fresh_memory(&mut operation.buffer)[..26].fill(0);
        data.returned(
            DataReturn::TxDone {
                context: operation.context,
                ticket: operation.ticket,
                buffer: operation.buffer,
                completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
            },
            &mut events,
        );
        operation = transmit(&mut data);
        assert_eq!(operation.context.frame().frame().len().get(), len);
        let spec = operation.context.frame().frame().header().header();
        let mut header = [0; 26];
        AckHeader::new(*spec).unwrap().encode(&mut header).unwrap();
        assert_eq!(tid_and_sequence(&header), (6, 0));
        assert_eq!(&memory(&operation.buffer)[34..len], payload.as_slice());
    }
    data.returned(
        DataReturn::TxDone {
            context: operation.context,
            ticket: operation.ticket,
            buffer: operation.buffer,
            completion: ended(End::Delivered, 1),
        },
        &mut events,
    );
    assert_eq!(core::iter::from_fn(|| data.tickets.take()).count(), 2);
    assert_eq!(core::iter::from_fn(|| data.buffers.take()).count(), 2);
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
}

#[test]
fn a_late_old_link_terminal_does_not_take_a_new_links_metadata() {
    let mut events = Kept::default();
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let old = transmit(&mut data);
    data.leave(&mut events);
    link.epoch = LinkEpoch::for_test(2);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let new = transmit(&mut data);
    assert!(!data.is_current(&old.context));
    assert!(data.is_current(&new.context));
    data.returned(
        DataReturn::TxDone {
            context: new.context,
            ticket: new.ticket,
            buffer: new.buffer,
            completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
        },
        &mut events,
    );
    data.returned(
        DataReturn::TxDone {
            context: old.context,
            ticket: old.ticket,
            buffer: old.buffer,
            completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
        },
        &mut events,
    );
    let again = transmit(&mut data);
    assert_eq!(again.ticket, 2);
    assert_eq!(again.context.order, 1);
    assert!(data.poll_action().is_none());
    assert_eq!(losses(&events, DataLoss::LeftBehind), 1);
    data.returned(
        DataReturn::TxDone {
            context: again.context,
            ticket: again.ticket,
            buffer: again.buffer,
            completion: ended(End::Delivered, 1),
        },
        &mut events,
    );
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert!(data.tickets.take().is_some());
    assert!(data.tickets.take().is_some());
    assert!(data.tickets.take().is_none());
    assert!(data.buffers.take().is_some());
    assert!(data.buffers.take().is_some());
    assert!(data.buffers.take().is_none());
    assert_eq!(tokens_lost(&events), 0);
}

#[test]
fn a_local_refusal_releases_admission_with_the_operation_resources() {
    let mut events = Kept::default();
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..8 {
        data.input(
            DataInput {
                msdu: msdu(6),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
        let operation = transmit(&mut data);
        assert_eq!(data.available(WmmAccessCategory::Voice), capacity - 1);
        data.returned(
            DataReturn::Refused {
                context: operation.context,
                ticket: operation.ticket,
                buffer: operation.buffer,
                reason: crate::machine::TxRefusal::NoRate,
            },
            &mut events,
        );
        assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
        assert!(data.accepts());
        assert!(data.poll_action().is_none());
    }
    assert_eq!(tokens_lost(&events), 0);
}

#[test]
fn preparation_refusal_ends_only_its_msdu_without_a_radio_terminal_or_retry() {
    let mut events = Kept::default();
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for priority in [6, 4] {
        data.input(
            DataInput {
                msdu: msdu(priority),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    let rejected = transmit(&mut data);
    let accepted = transmit(&mut data);
    data.returned(
        DataReturn::Refused {
            context: rejected.context,
            ticket: rejected.ticket,
            buffer: rejected.buffer,
            reason: crate::machine::TxRefusal::Layout(
                oer_ieee80211_contracts_port::LayoutError::BodyOverlap,
            ),
        },
        &mut events,
    );
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert_eq!(data.available(WmmAccessCategory::Video), capacity - 1);
    // The service reported the refusal; the data machine adds no event.
    assert!(events.0.is_empty());
    assert!(data.poll_action().is_none());
    data.returned(
        DataReturn::TxDone {
            context: accepted.context,
            ticket: accepted.ticket,
            buffer: accepted.buffer,
            completion: ended(End::Delivered, 1),
        },
        &mut events,
    );
    assert_eq!(core::iter::from_fn(|| data.tickets.take()).count(), 2);
    assert_eq!(core::iter::from_fn(|| data.buffers.take()).count(), 2);
    assert_eq!(tokens_lost(&events), 0);
}

#[test]
fn a_protected_qos_msdu_leaves_room_for_the_ccmp_header_and_contends_by_its_priority() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let sent = transmit(&mut data);
    assert!(data.poll_action().is_none());
    let frame = memory(&sent.buffer);
    // QoS Data, To DS, Protected.
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x4188);
    assert_eq!(&frame[4..10], &BSSID);
    assert_eq!(&frame[16..22], &PEER);
    // The TID in the QoS Control field.
    assert_eq!(frame[24] & 0x0f, 6);
    // Header, the CCMP header's room, LLC/SNAP, payload; no MIC.
    assert_eq!(sent.context.frame().frame().len().get(), 26 + 8 + 8 + 100);
    assert_eq!(&frame[34..42], &[0xaa, 0xaa, 0x03, 0, 0, 0, 0x08, 0x00]);
    assert_eq!(sent.context.access_category(), WmmAccessCategory::Voice);
    assert_eq!(
        sent.context.protection(&link),
        Ok(DataProtection::Ccmp(&()))
    );
}

#[test]
fn an_open_non_qos_msdu_takes_the_stations_sequence_number_and_no_ccmp_room() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut counter = SequenceCounter::new(SequenceNumber::new(42).unwrap());
    let mut link = open_link(&mut counter);
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let sent = transmit(&mut data);
    let frame = memory(&sent.buffer);
    assert_eq!(u16::from_le_bytes([frame[0], frame[1]]), 0x0108);
    assert_eq!(u16::from_le_bytes([frame[22], frame[23]]) >> 4, 42);
    assert_eq!(sent.context.frame().frame().len().get(), 24 + 8 + 100);
    assert_eq!(sent.context.protection(&link), Ok(DataProtection::Open));
    assert_eq!(counter.peek(), SequenceNumber::new(43).unwrap());
}

#[test]
fn an_msdu_too_long_for_its_buffer_takes_no_sequence_number() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1],
        [Buffer::new(std::vec![0; 200])],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut counter = SequenceCounter::new(SequenceNumber::new(42).unwrap());
    let long = Msdu {
        payload: &[0; 200],
        ..msdu(0)
    };
    assert_eq!(
        data.input(
            DataInput {
                msdu: long,
                link: &mut open_link(&mut counter)
            },
            &mut events
        ),
        Err(DataRefusal::TooLong)
    );
    assert_eq!(counter.peek(), SequenceNumber::new(42).unwrap());
    // The tokens stay, and a short MSDU takes the number.
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut open_link(&mut counter),
        },
        &mut events,
    )
    .unwrap();
    let sent = transmit(&mut data);
    assert_eq!(
        u16::from_le_bytes([memory(&sent.buffer)[22], memory(&sent.buffer)[23]]) >> 4,
        42
    );
}

#[test]
fn a_buffer_whose_memory_is_not_the_stations_is_refused_and_takes_no_sequence_number() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1],
        [Buffer(std::vec![0; 1600], false)],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut counter = SequenceCounter::new(SequenceNumber::new(42).unwrap());
    assert_eq!(
        data.input(
            DataInput {
                msdu: msdu(0),
                link: &mut open_link(&mut counter)
            },
            &mut events
        ),
        Err(DataRefusal::BufferUnavailable)
    );
    assert_eq!(counter.peek(), SequenceNumber::new(42).unwrap());
    assert!(data.accepts(), "the tokens came back");
    assert!(events.0.contains(&StaEvent::BufferMemoryUnavailable));
}

#[test]
fn open_mpdu_fits_exactly_without_a_temporary_ccmp_gap() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1],
        [Buffer::new(std::vec![0; 24 + 8 + 100])],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut counter = SequenceCounter::new(SequenceNumber::new(42).unwrap());
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut open_link(&mut counter),
        },
        &mut events,
    )
    .unwrap();
    let sent = transmit(&mut data);
    let bytes = memory(&sent.buffer);
    assert_eq!(bytes.len(), 132);
    assert_eq!(&bytes[..2], &[0x08, 0x01]);
    assert_eq!(&bytes[4..10], &BSSID);
    assert_eq!(&bytes[10..16], &LOCAL);
    assert_eq!(&bytes[16..22], &PEER);
    assert_eq!(&bytes[22..24], &(42_u16 << 4).to_le_bytes());
    assert_eq!(&bytes[24..32], &[0xaa, 0xaa, 3, 0, 0, 0, 8, 0]);
    assert_eq!(&bytes[32..], &[0xab; 100]);
    assert_eq!(counter.peek(), SequenceNumber::new(43).unwrap());
}

#[test]
fn msdus_wait_while_the_tokens_are_out() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    assert!(!data.accepts());
    assert_eq!(
        data.input(
            DataInput {
                msdu: msdu(0),
                link: &mut link
            },
            &mut events
        ),
        Err(DataRefusal::NoTokens)
    );
    let first = transmit(&mut data);
    data.returned(
        DataReturn::TxDone {
            context: first.context,
            ticket: first.ticket,
            buffer: first.buffer,
            completion: ended(End::Delivered, 1),
        },
        &mut events,
    );
    assert!(data.accepts());
}

#[test]
fn buffers_hold_owed_mpdus_and_tickets_only_attempts() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 16>::new(
        [1, 2],
        core::array::from_fn::<_, 16, _>(|_| Buffer::new(std::vec![0; 1600])),
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for priority in core::iter::repeat_n(5, 16) {
        assert_eq!(
            data.input(
                DataInput {
                    msdu: msdu(priority),
                    link: &mut link,
                },
                &mut events,
            )
            .map(|tid| tid.value()),
            Ok(5),
            "every buffer takes an MSDU while two tickets are free"
        );
    }
    assert!(!data.accepts());
    let first = transmit(&mut data);
    let second = transmit(&mut data);
    assert!(data.poll_action().is_none(), "both tickets are attempts");
    assert_eq!(data.available(WmmAccessCategory::Video), 0);
    data.returned(
        DataReturn::TxDone {
            context: first.context,
            ticket: first.ticket,
            buffer: first.buffer,
            completion: ended(End::Delivered, 1),
        },
        &mut events,
    );
    assert!(data.accepts(), "the delivered MPDU's buffer is free");
    let third = transmit(&mut data);
    assert_eq!(
        third.ticket, 1,
        "the returned ticket goes with the next MPDU"
    );
    assert!(data.poll_action().is_none());
    drop((second, third));
    assert_eq!(tokens_lost(&events), 0);
}

fn received_frame(control: u16, ether_type: u16, packet_number: u8, key_id: u8) -> Vec<u8> {
    let mut frame = std::vec![0_u8; 26];
    frame[0..2].copy_from_slice(&control.to_le_bytes());
    frame[4..10].copy_from_slice(&LOCAL);
    frame[10..16].copy_from_slice(&BSSID);
    frame[16..22].copy_from_slice(&PEER);
    if control & 0x4000 != 0 {
        frame.extend_from_slice(&[packet_number, 0, 0, 0x20 | (key_id << 6), 0, 0, 0, 0]);
    }
    frame.extend_from_slice(&[0xaa, 0xaa, 0x03, 0, 0, 0]);
    frame.extend_from_slice(&ether_type.to_be_bytes());
    frame.extend_from_slice(&[0xcd; 20]);
    frame
}

fn meta(crypto: RxEvidence<RxCryptoStatus>) -> RxMeta {
    let mut meta = RxMeta::unavailable(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
    meta.crypto = crypto;
    meta
}

fn decrypted() -> RxMeta {
    meta(RxEvidence::HardwareObserved(
        RxCryptoStatus::DecryptedAndIntegrityVerified,
    ))
}

#[test]
fn a_decrypted_data_mpdu_gives_its_msdu_and_an_unverified_or_eapol_one_none() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 1, 0);
    let parts: Vec<_> = received(&mut link, &frame, &decrypted())
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(parts.len(), 1);
    assert_eq!(parts[0].source, PEER);
    assert_eq!(parts[0].destination, LOCAL);
    assert_eq!(parts[0].payload, &[0xcd; 20]);
    // Protected but not verified: nothing for the network.
    let frame = received_frame(0x4288, 0x0800, 2, 0);
    assert!(received(&mut link, &frame, &meta(RxEvidence::Unavailable)).is_err());
    // EAPOL is the station's.
    let eapol = received_frame(0x4288, 0x888e, 3, 0);
    assert_eq!(
        received(&mut link, &eapol, &decrypted()).unwrap().count(),
        0
    );
}

#[test]
fn a_frame_sent_again_is_dropped_by_the_replay_check() {
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let frame = received_frame(0x4288, 0x0800, 7, 0);
    assert_eq!(
        received(&mut link, &frame, &decrypted()).unwrap().count(),
        1
    );
    // The same packet number again: dropped before it reaches the network.
    assert!(received(&mut link, &frame, &decrypted()).is_err());
}

#[test]
fn the_old_and_the_new_group_key_are_both_taken_until_the_old_is_removed() {
    let mut replay = epoch();
    let group = |packet_number, key_id| {
        let mut frame = received_frame(0x4288, 0x0800, packet_number, key_id);
        frame[4..10].copy_from_slice(&[0xff; 6]);
        frame
    };
    {
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(
            received(&mut link, &group(200, 1), &decrypted())
                .unwrap()
                .count(),
            1
        );
    }
    // The access point's new group key, index 2, from packet number 1.
    assert!(
        replay
            .exchange_group(
                GroupKeyId::new(2).unwrap(),
                oer_ieee80211_core_upper_mac::ccmp::CcmpRxReplayState::default(),
            )
            .is_none()
    );
    {
        // Both keys are taken while both are installed.
        let mut link = qos_link(&(), &mut replay);
        assert_eq!(
            received(&mut link, &group(1, 2), &decrypted())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(
            received(&mut link, &group(201, 1), &decrypted())
                .unwrap()
                .count(),
            1
        );
    }
    replay.remove_group(GroupKeyId::new(1).unwrap());
    let mut link = qos_link(&(), &mut replay);
    assert!(received(&mut link, &group(202, 1), &decrypted()).is_err());
    assert_eq!(
        received(&mut link, &group(2, 2), &decrypted())
            .unwrap()
            .count(),
        1
    );
}

/// The terminal kinds the data machine's tests return.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum End {
    Delivered,
    AckTimeout,
    ContentionLimit,
}

/// The terminal of an attempt that made `transmissions`.
fn ended(status: End, transmissions: u16) -> SingleOutcome<(), ()> {
    let air =
        NonZeroU16::new(transmissions).map(|transmissions| oer_ieee80211_contracts_port::Air {
            transmissions,
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        });
    TxOutcome::Ended(match status {
        End::Delivered => SingleEnd::Delivered {
            air: air.unwrap(),
            response: oer_ieee80211_contracts_port::SingleResponse::Ack(()),
        },
        End::AckTimeout => SingleEnd::AckTimeout(air.unwrap()),
        End::ContentionLimit => SingleEnd::ContentionLimit(air),
    })
}

/// The TID and sequence number of a QoS Data MPDU in its buffer.
fn tid_and_sequence(frame: &[u8]) -> (u8, u16) {
    (
        frame[24] & 0x0f,
        u16::from_le_bytes([frame[22], frame[23]]) >> 4,
    )
}

#[test]
fn an_attempt_aborted_before_the_air_goes_again_first_with_its_sequence_number() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1, 2, 3],
        [
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // Two MSDUs of TID 6: the first is submitted, then a newer one waits.
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let first = transmit(&mut data);
    let sent = tid_and_sequence(memory(&first.buffer));
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    // A closing gate aborts the first before any transmission.
    data.returned(
        DataReturn::TxDone {
            context: first.context,
            ticket: first.ticket,
            buffer: first.buffer,
            completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
        },
        &mut events,
    );
    // It goes again at the head of its TID's queue, before the newer MSDU,
    // with the same sequence number, in its buffer.
    let again = transmit(&mut data);
    assert_eq!(tid_and_sequence(memory(&again.buffer)), sent);
    let newer = transmit(&mut data);
    assert_eq!(tid_and_sequence(memory(&newer.buffer)), (6, sent.1 + 1));
    assert!(data.poll_action().is_none());
    // One that made a transmission ends: its tokens come back.
    data.returned(
        DataReturn::TxDone {
            context: again.context,
            ticket: again.ticket,
            buffer: again.buffer,
            completion: ended(End::AckTimeout, 3),
        },
        &mut events,
    );
    assert!(data.poll_action().is_none());
}

#[test]
fn a_link_that_ends_drains_its_queue_and_its_submitted_attempts_never_go_again() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1, 2, 3],
        [
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // One MPDU submitted and two owed on the link.
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let submitted = transmit(&mut data);
    data.input(
        DataInput {
            msdu: msdu(0),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    assert!(!data.accepts());
    // The link ends: the owed ones drain into their tokens, counted.
    data.leave(&mut events);
    assert!(data.poll_action().is_none());
    assert_eq!(losses(&events, DataLoss::LeftBehind), 2);
    // The submitted one, aborted before the air, returns its tokens and
    // does not go again: the retry rule holds within one link.
    data.returned(
        DataReturn::TxDone {
            context: submitted.context,
            ticket: submitted.ticket,
            buffer: submitted.buffer,
            completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
        },
        &mut events,
    );
    assert!(data.poll_action().is_none());
    assert_eq!(losses(&events, DataLoss::LeftBehind), 3);
    assert_eq!(tokens_lost(&events), 0);
    // Every token is back: a new link takes three MSDUs again.
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..3 {
        data.input(
            DataInput {
                msdu: msdu(0),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    assert!(!data.accepts());
}

#[test]
fn a_category_the_service_does_not_admit_waits_and_the_others_go_in_their_order() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1, 2, 3],
        [
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
            Buffer::new(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // Voice (TID 6), then best effort (TID 0), then Voice again.
    for priority in [6, 0, 7] {
        data.input(
            DataInput {
                msdu: msdu(priority),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    // Voice is full at the port: best effort goes, Voice waits.
    let voice_permits: Vec<_> =
        core::iter::from_fn(|| data.admission.take(WmmAccessCategory::Voice)).collect();
    let first = match data.poll_action() {
        Some(transmit) => transmit,
        None => panic!("best effort goes"),
    };
    assert_eq!(
        first.context.access_category(),
        WmmAccessCategory::BestEffort
    );
    assert!(data.poll_action().is_none());
    assert_eq!(data.available(WmmAccessCategory::BestEffort), capacity - 1);
    assert_eq!(data.available(WmmAccessCategory::Voice), 0);
    for permit in voice_permits {
        data.admission.put(WmmAccessCategory::Voice, permit);
    }
    // Voice admitted: its MPDUs go in their order.
    let voice = transmit(&mut data);
    assert_eq!(tid_and_sequence(memory(&voice.buffer)).0, 6);
    let voice = transmit(&mut data);
    assert_eq!(tid_and_sequence(memory(&voice.buffer)).0, 7);
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity - 2);
}

#[test]
fn terminal_failures_retire_the_msdu_and_release_admission() {
    for (outcome, indeterminate, contention, expired) in [
        (
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
            1,
            0,
            0,
        ),
        (ended(End::ContentionLimit, 0), 0, 1, 0),
        (TxOutcome::Expired { msdus: 1 }, 0, 0, 1),
    ] {
        let mut events = Kept::default();
        let mut data = StaData::<Data, Caps, 8>::new(
            [1],
            [Buffer::new(std::vec![0; 1600])],
            core::num::NonZeroU8::new(8).unwrap(),
        );
        let capacity = data.available(WmmAccessCategory::Voice);
        let mut replay = epoch();
        let mut link = qos_link(&(), &mut replay);
        data.input(
            DataInput {
                msdu: msdu(6),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
        let operation = transmit(&mut data);
        let buffer = match operation.buffer {
            ReturnedMpdu::Fresh(buffer) => buffer,
            ReturnedMpdu::Sent(_) => panic!("a new MSDU is fresh"),
        };
        data.returned(
            DataReturn::TxDone {
                context: operation.context,
                ticket: operation.ticket,
                buffer: ReturnedMpdu::Sent(buffer),
                completion: outcome,
            },
            &mut events,
        );
        assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
        assert_eq!(losses(&events, DataLoss::Indeterminate), indeterminate);
        assert_eq!(losses(&events, DataLoss::ContentionLimited), contention);
        assert_eq!(losses(&events, DataLoss::Expired), expired);
        assert_eq!(tokens_lost(&events), 0);
        assert!(
            data.poll_action().is_none(),
            "final outcomes never retry the MSDU"
        );
        data.input(
            DataInput {
                msdu: msdu(6),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
        assert_eq!(
            transmit(&mut data).ticket,
            1,
            "the original credit is reusable once"
        );
    }
}

#[test]
fn key_removal_drops_a_published_msdu_and_returns_admission_once() {
    let mut events = Kept::default();
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let operation = transmit(&mut data);
    let ReturnedMpdu::Fresh(buffer) = operation.buffer else {
        unreachable!()
    };
    data.returned(
        DataReturn::TxDone {
            context: operation.context,
            ticket: operation.ticket,
            buffer: ReturnedMpdu::Sent(buffer),
            completion: TxOutcome::Withdrawn(WithdrawCause::KeyRemoved),
        },
        &mut events,
    );
    assert!(
        data.poll_action().is_none(),
        "the removed key's MSDU is not retried"
    );
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert_eq!(tokens_lost(&events), 0);
    for _ in 0..2 {
        assert!(data.tickets.take().is_some());
        assert!(data.buffers.take().is_some());
    }
    assert!(data.tickets.take().is_none());
    assert!(data.buffers.take().is_none());
}

const EAPOL_ETHER_TYPE: u16 = 0x888e;
/// The MSDUs of a data MPDU of `link`'s BSS to the station, which `meta`
/// describes: a protected one only once the hardware decrypted and verified
/// it and the replay check passed it. An EAPOL frame is the station's, not
/// the network's.
fn received<'f, T: StaTokens>(
    link: &mut LinkView<'_, '_, T>,
    frame: &'f [u8],
    meta: &RxMeta,
) -> Result<
    impl Iterator<Item = Result<MsduParts<'f>, oer_ieee80211_core_upper_mac::receive::RxDropReason>>
    + 'f,
    oer_ieee80211_core_upper_mac::receive::RxDropReason,
> {
    use oer_ieee80211_core_upper_mac::receive::{AdmittedData, RxDropReason};
    let protected_link = matches!(link.protection, LinkProtection::Ccmp { .. });
    let replay = match &mut link.protection {
        LinkProtection::Ccmp { replay, .. } => Some(&mut **replay),
        LinkProtection::Open => None,
    };
    let admitted = AdmittedData::admit(
        oer_ieee80211_wire::frame::DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?,
        link.address,
        link.bssid,
        integrity(meta),
        replay,
    )?;
    if protected_link != admitted.protected() {
        return Err(RxDropReason::Protection);
    }
    Ok(admitted.msdus()?.filter(|parts| match parts {
        Ok(parts) => parts.ether_type != EAPOL_ETHER_TYPE,
        Err(_) => true,
    }))
}

#[test]
fn data_losses_arrive_in_the_order_they_happen() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..2 {
        data.input(
            DataInput {
                msdu: msdu(6),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    let first = transmit(&mut data);
    let second = transmit(&mut data);
    for (operation, completion) in [
        (first, TxOutcome::Expired { msdus: 1 }),
        (
            second,
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
        ),
    ] {
        data.returned(
            DataReturn::TxDone {
                context: operation.context,
                ticket: operation.ticket,
                buffer: operation.buffer,
                completion,
            },
            &mut events,
        );
    }
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    data.leave(&mut events);
    assert_eq!(
        events.0,
        [
            StaEvent::DataLost(DataLoss::Expired),
            StaEvent::DataLost(DataLoss::Indeterminate),
            StaEvent::DataLost(DataLoss::LeftBehind),
        ]
    );
}

/// `restricted` closed to the station's QoS data.
fn closed(restricted: &[WmmAccessCategory]) -> AcAdmission {
    let defaults = oer_ieee80211_wire::extensions::wmm::WmmParameterSet::STANDARD_DEFAULT;
    let record = |category| oer_ieee80211_wire::extensions::wmm::WmmAcParameters {
        admission_control_mandatory: restricted.contains(&category),
        ..defaults.access_category(category)
    };
    AcAdmission::from_edca(oer_ieee80211_wire::extensions::wmm::WmmParameterSet::new(
        1,
        false,
        WmmAccessCategory::ALL.map(record),
    ))
}

#[test]
fn a_closed_voice_category_sends_voice_msdus_as_video_in_tid_five() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    link.admission = closed(&[WmmAccessCategory::Voice]);
    if let LinkMode::Qos(counters) = &mut link.mode {
        counters[5] = SequenceCounter::new(SequenceNumber::new(77).unwrap());
    }
    for priority in [6, 7] {
        data.input(
            DataInput {
                msdu: msdu(priority),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    for sequence in [77, 78] {
        let sent = transmit(&mut data);
        let frame = memory(&sent.buffer);
        // The TID, its sequence space and the category agree on UP5.
        assert_eq!(tid_and_sequence(frame), (5, sequence));
        assert_eq!(sent.context.access_category(), WmmAccessCategory::Video);
    }
    // Neither MSDU took a number in TID 6 or 7.
    if let LinkMode::Qos(counters) = &mut link.mode {
        assert_eq!(counters[6].take().get(), 0);
        assert_eq!(counters[7].take().get(), 0);
    }
}

#[test]
fn an_msdu_without_an_open_category_is_refused_before_taking_an_owner() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    link.admission = closed(&WmmAccessCategory::ALL);
    let available = data.available(WmmAccessCategory::Voice);
    assert_eq!(
        data.input(
            DataInput {
                msdu: msdu(6),
                link: &mut link,
            },
            &mut events,
        ),
        Err(DataRefusal::NoAdmittedAccessCategory)
    );
    assert!(data.poll_action().is_none());
    assert_eq!(data.available(WmmAccessCategory::Voice), available);
    assert!(events.0.is_empty());
    // A non-QoS link has no categories to close: its data goes as before.
    let mut counter = SequenceCounter::new(SequenceNumber::ZERO);
    let mut open = open_link(&mut counter);
    open.admission = closed(&WmmAccessCategory::ALL);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut open,
        },
        &mut events,
    )
    .unwrap();
    assert_eq!(
        transmit(&mut data).context.access_category(),
        WmmAccessCategory::BestEffort
    );
}

#[test]
fn a_changed_admission_applies_to_the_next_msdu_and_keeps_a_numbered_one() {
    let mut events = Kept::default();
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    // The access point closes Voice after the first MSDU took its number.
    link.admission = closed(&[WmmAccessCategory::Voice]);
    data.input(
        DataInput {
            msdu: msdu(6),
            link: &mut link,
        },
        &mut events,
    )
    .unwrap();
    let mut sent = [transmit(&mut data), transmit(&mut data)];
    sent.sort_by_key(|operation| tid_and_sequence(memory(&operation.buffer)).0);
    assert_eq!(tid_and_sequence(memory(&sent[0].buffer)), (5, 0));
    assert_eq!(sent[0].context.access_category(), WmmAccessCategory::Video);
    assert_eq!(tid_and_sequence(memory(&sent[1].buffer)), (6, 0));
    assert_eq!(sent[1].context.access_category(), WmmAccessCategory::Voice);
}

fn window(start: u16, subframes: usize, max_length: u32) -> Option<AggregateWindow> {
    Some(AggregateWindow {
        start: SequenceNumber::new(start).unwrap(),
        size: 32,
        subframes,
        max_length,
    })
}

const RETRY: AggregateRetry = AggregateRetry {
    lifetime: oer_time::Duration::from_secs(1),
    attempts: 2,
};

fn block_ack(start: u16, bitmap: u64) -> oer_ieee80211_contracts_port::AggregateOutcome<(), ()> {
    TxOutcome::Ended(oer_ieee80211_contracts_port::AggregateEnd::Delivered {
        air: oer_ieee80211_contracts_port::Air {
            transmissions: NonZeroU16::MIN,
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
        },
        report: oer_ieee80211_contracts_port::BlockAckReport::new(
            SequenceNumber::new(start).unwrap(),
            bitmap,
        ),
        ack: (),
    })
}

fn aggregate(
    attempt: Option<DataAttempt<'static, Data<'static>, Caps>>,
) -> DataAggregate<'static, Data<'static>, Caps> {
    match attempt {
        Some(DataAttempt::Aggregate(aggregate)) => aggregate,
        _ => panic!("an aggregate"),
    }
}

#[test]
fn an_agreement_aggregates_consecutive_owed_mpdus_within_its_limits() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1, 2],
        core::array::from_fn::<_, 8, _>(|_| Buffer::new(std::vec![0; 1600])),
        core::num::NonZeroU8::new(2).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..5 {
        data.input(
            DataInput {
                msdu: msdu(0),
                link: &mut link,
            },
            &mut events,
        )
        .unwrap();
    }
    // Without an agreement the first MPDU goes alone.
    let alone = match data.poll_attempt(|_| None, &mut events) {
        Some(DataAttempt::Single(single)) => single,
        _ => panic!("a single MPDU"),
    };
    // Three subframes at most: 1, 2 and 3; 4 waits.
    let first = aggregate(data.poll_attempt(|_| window(1, 3, u32::MAX), &mut events));
    assert_eq!((first.context.len(), first.mpdus.len()), (3, 3));
    assert_eq!(first.context.tid().value(), 0);
    assert!(
        data.poll_attempt(|_| window(1, 3, u32::MAX), &mut events)
            .is_none(),
        "both tickets are out"
    );
    // The BlockAck misses 2: it goes back, 1 and 3 retire.
    let marks = data.aggregate_returned(
        DataAggregateReturn::TxDone {
            context: first.context,
            ticket: first.ticket,
            mpdus: first.mpdus,
            completion: block_ack(1, 0b101),
        },
        oer_time::Instant::from_micros(2_000),
        Some(RETRY),
        &mut events,
    );
    assert_eq!(
        marks,
        Some(BlockAckMarks {
            tid: WmmUserPriority::UP0,
            start: SequenceNumber::new(1).unwrap(),
            delivered: 0b101,
            expired: 0,
        })
    );
    // One subframe's octets: only 2 fits a length below two of them.
    let short = aggregate(data.poll_attempt(|_| window(2, 8, 200), &mut events));
    assert_eq!(short.context.len(), 1);
    assert!(matches!(short.mpdus[0], ReturnedMpdu::Fresh(_)));
    // 2 misses its second aggregate on the air: the policy drops it.
    let marks = data.aggregate_returned(
        DataAggregateReturn::TxDone {
            context: short.context,
            ticket: short.ticket,
            mpdus: short.mpdus,
            completion: block_ack(2, 0),
        },
        oer_time::Instant::from_micros(3_000),
        Some(RETRY),
        &mut events,
    );
    assert_eq!(marks.map(|marks| marks.expired), Some(0b1));
    assert_eq!(losses(&events, DataLoss::RetryLimit), 1);
    // A sequence number outside the window goes alone.
    assert!(matches!(
        data.poll_attempt(|_| window(100, 8, u32::MAX), &mut events),
        Some(DataAttempt::Single(_))
    ));
    drop(alone);
    assert_eq!(tokens_lost(&events), 0);
}

#[test]
fn every_return_gives_the_members_places_back() {
    let mut events = Kept::default();
    let mut data = StaData::<Data, Caps, 8>::new(
        [1, 2],
        core::array::from_fn::<_, 8, _>(|_| Buffer::new(std::vec![0; 1600])),
        core::num::NonZeroU8::new(2).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    let mut input =
        |data: &mut StaData<'static, Data<'static>, Caps, 8>, events: &mut Kept, count: usize| {
            for _ in 0..count {
                data.input(
                    DataInput {
                        msdu: msdu(0),
                        link: &mut link,
                    },
                    events,
                )
                .unwrap();
            }
        };
    // Ten aggregates of four members pass through eight member places.
    for round in 0..10_u16 {
        input(&mut data, &mut events, 4);
        let sent = aggregate(data.poll_attempt(|_| window(round * 4, 4, u32::MAX), &mut events));
        assert_eq!((sent.context.len(), sent.mpdus.len()), (4, 4));
        let marks = data.aggregate_returned(
            DataAggregateReturn::TxDone {
                context: sent.context,
                ticket: sent.ticket,
                mpdus: sent.mpdus,
                completion: block_ack(round * 4, 0b1111),
            },
            oer_time::Instant::from_micros(1_000),
            Some(RETRY),
            &mut events,
        );
        assert_eq!(marks.map(|marks| marks.delivered), Some(0b1111));
    }
    // Two members the builder did not take go back with their places; the
    // next aggregate holds them again.
    input(&mut data, &mut events, 4);
    let mut sent = aggregate(data.poll_attempt(|_| window(40, 4, u32::MAX), &mut events));
    let mut tail = heapless::Vec::new();
    for _ in 0..2 {
        let mpdu = sent.mpdus.pop().unwrap();
        assert!(tail.insert(0, mpdu).is_ok());
    }
    data.unbuilt(&mut sent.context, tail, &mut events);
    assert_eq!(sent.context.len(), 2);
    let marks = data.aggregate_returned(
        DataAggregateReturn::TxDone {
            context: sent.context,
            ticket: sent.ticket,
            mpdus: sent.mpdus,
            completion: block_ack(40, 0b11),
        },
        oer_time::Instant::from_micros(1_000),
        Some(RETRY),
        &mut events,
    );
    assert_eq!(marks.map(|marks| marks.delivered), Some(0b11));
    let again = aggregate(data.poll_attempt(|_| window(42, 4, u32::MAX), &mut events));
    assert_eq!(again.context.len(), 2);
    // The bank describes exactly the members the context names.
    assert!((0..2).all(|index| data.aggregate_frame(&again.context, index).is_some()));
    assert!(data.aggregate_frame(&again.context, 2).is_none());
    assert_eq!(tokens_lost(&events), 0);
}
