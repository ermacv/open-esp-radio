//! The data machine's encapsulation, tokens and admission.

use super::*;
use core::num::NonZeroU16;
use oer_ieee80211_lower_mac::association::KEY_BANK_CAPACITY;
use oer_ieee80211_lower_mac::{
    BandSet, Channel, ChannelWidth, CoexPriority, CoexPrioritySet, HardwareServices,
    Ieee80211Capabilities, LowerMacCapabilities, PhyFormatSet, RateSupport, ReceiveFilter,
    RxEvidence, WidthSet,
};
use oer_ieee80211_lower_mac::{IndeterminateCause, WithdrawCause};
use oer_ieee80211_mac::ccmp::{CcmpRxReplayEpoch, GroupKeyId};
use oer_ieee80211_mac::data::EthernetFrameParts;
use oer_ieee80211_mac::phy::{HeMcs, HtMcs, LegacyRate, PhyRate};
use oer_ieee80211_mac::sequence::SequenceNumber;
use oer_ieee80211_mac::station::StaSequenceCounter;
use std::vec::Vec;

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
        station_receive_filters: ReceiveFilter::BSS_MEMBER,
        access_point_receive_filters: ReceiveFilter::BSS_MEMBER,
        rx_block_ack_max_tid: 0,
        rx_block_ack_max_window: 0,
        edca_max_aifsn: 15,
        edca_max_cw_exponent: 10,
    };
}

struct Buffer(Vec<u8>);

impl oer_ieee80211_lower_mac::TxBufferClass for Buffer {
    const CAPACITY: usize = 1600;
}

impl TxBufferMemory for Buffer {
    fn bytes_mut(&mut self) -> &mut [u8] {
        &mut self.0
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

struct Data;

impl StaDataTokens for Data {
    type EpochPath = oer_radio_port::Root;
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

struct Station;

impl StaTokens for Station {
    type AssociationStart = ();
    type Association = ();
    type VacantKeyBank =
        oer_ieee80211_lower_mac::association::KeyBankParts<(), (), KEY_BANK_CAPACITY>;
    type VacantAssociation = [(); 0];
    fn begin_association(_: (), _: oer_ieee80211_mac::security::Spp) {}
    fn association(key: &()) -> &() {
        key
    }
    fn split_vacant(group: Self::VacantAssociation) -> ((), (), Self::VacantKeyBank) {
        (
            (),
            (),
            oer_ieee80211_lower_mac::association::KeyBankParts {
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
    type GatePath = oer_radio_port::Root;
    type EpochPath = oer_radio_port::Root;
    type ConfigurationPath = oer_radio_port::Root;
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

fn qos_link<'a>(key: &'a (), replay: &'a mut CcmpRxReplayEpoch) -> LinkView<'a, 'static, Station> {
    LinkView {
        epoch: LinkEpoch::for_test(1),
        address: oer_ieee80211_mac::address::OwnAddress::new(
            oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: oer_ieee80211_mac::address::Bssid::new(
            oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        association_id: 1,
        // The station's QoS sequence spaces, which the link lends.
        mode: LinkMode::Qos(std::boxed::Box::leak(std::boxed::Box::new(
            [StaSequenceCounter::new(SequenceNumber::ZERO); 8],
        ))),
        protection: LinkProtection::Ccmp { key, replay },
    }
}

fn open_link(counter: &mut StaSequenceCounter) -> LinkView<'_, 'static, Station> {
    LinkView {
        epoch: LinkEpoch::for_test(1),
        address: oer_ieee80211_mac::address::OwnAddress::new(
            oer_ieee80211_mac::address::IndividualAddress::new(LOCAL).unwrap(),
        ),
        bssid: oer_ieee80211_mac::address::Bssid::new(
            oer_ieee80211_mac::address::IndividualAddress::new(BSSID).unwrap(),
        )
        .unwrap(),
        association_id: 1,
        mode: LinkMode::NonQos(counter),
        protection: LinkProtection::Open,
    }
}

fn machine() -> StaData<'static, Data, Caps> {
    StaData::new(
        [1, 2],
        [Buffer(std::vec![0; 1600]), Buffer(std::vec![0; 1600])],
        core::num::NonZeroU8::new(8).unwrap(),
    )
}

fn msdu(priority: u8) -> Msdu<'static> {
    Msdu {
        destination: PEER,
        ether_type: 0x0800,
        priority: WmmUserPriority::new(priority).unwrap(),
        payload: &[0xab; 100],
    }
}

fn epoch() -> CcmpRxReplayEpoch {
    CcmpRxReplayEpoch::new([0; 8], GroupKeyId::new(1).unwrap(), [0; 8]).unwrap()
}

fn transmit(data: &mut StaData<'static, Data, Caps>) -> DataTransmit<'static, Data, Caps> {
    match data.poll_action() {
        Some(transmit) => transmit,
        None => panic!("an attempt is owed"),
    }
}

#[test]
fn stopped_data_bank_reuses_credits_permissions_and_the_order_frontier() {
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..2 {
        data.input(DataInput {
            msdu: msdu(6),
            link: &mut link,
        })
        .unwrap();
    }
    let next_order = data.next_order;
    let outstanding = transmit(&mut data);
    assert_eq!(data.available(WmmAccessCategory::Voice), 1);
    let bank = data.finish();
    assert_eq!(bank.available(WmmAccessCategory::Voice), 1);
    assert_eq!(bank.counters().left_behind, 1);
    let mut data = bank.restart();
    assert_eq!(data.next_order, next_order);
    assert_eq!(data.available(WmmAccessCategory::Voice), 1);
    assert_eq!(data.tickets.len(), 1);
    data.returned(DataReturn::TxDone {
        context: outstanding.context,
        ticket: outstanding.ticket,
        buffer: outstanding.buffer,
        completion: ended(End::Delivered, 1),
    });
    assert_eq!(data.available(WmmAccessCategory::Voice), 2);
    assert_eq!(data.tickets.len(), 2);
    assert_eq!(data.buffers.len(), 2);
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    let transmit = transmit(&mut data);
    assert_eq!(transmit.context.order, next_order);
    assert_eq!(data.counters().lost_tokens, 0);
}

#[test]
fn a_changed_link_refuses_data_without_reinterpreting_the_encoding_or_losing_resources() {
    for encoded_protected in [false, true] {
        for change_epoch in [false, true] {
            let mut data = machine();
            let capacity = data.available(WmmAccessCategory::Voice);
            let mut replay = epoch();
            let mut link = qos_link(&(), &mut replay);
            if !encoded_protected {
                link.protection = LinkProtection::Open;
            }
            data.input(DataInput {
                msdu: msdu(6),
                link: &mut link,
            })
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
            data.returned(DataReturn::Refused {
                context,
                ticket,
                buffer,
                reason,
            });
            assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
            assert_eq!(data.counters().preparation_refused, 1);
            assert!(data.poll_action().is_none());
            assert_eq!(core::iter::from_fn(|| data.tickets.take()).count(), 2);
            assert_eq!(core::iter::from_fn(|| data.buffers.take()).count(), 2);
        }
    }
}

#[test]
fn interleaved_returns_keep_the_original_tid_order_and_admission_count() {
    let mut data = StaData::<Data, Caps>::new(
        [1, 2, 3, 4],
        core::array::from_fn::<_, 4, _>(|_| Buffer(std::vec![0; 1600])),
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for priority in [6, 4, 6, 7] {
        data.input(DataInput {
            msdu: msdu(priority),
            link: &mut link,
        })
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
        data.returned(DataReturn::TxDone {
            context: operation.context,
            ticket: operation.ticket,
            buffer: operation.buffer,
            completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
        });
    }
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert_eq!(data.available(WmmAccessCategory::Video), capacity);
    for (ticket, tid, sequence) in [(1, 6, 0), (2, 4, 0), (3, 6, 1), (4, 7, 0)] {
        let operation = transmit(&mut data);
        assert_eq!(operation.ticket, ticket);
        assert_eq!(operation.context.tid.value(), tid);
        assert_eq!(
            operation.context.frame.frame().len().get(),
            26 + 8 + 8 + 100
        );
        let spec = operation.context.frame().frame().header().header();
        let mut header = [0; 26];
        oer_ieee80211_lower_mac::HeaderLayout::new(AckHeader::new(*spec).unwrap(), &mut header)
            .unwrap()
            .encode();
        assert_eq!(tid_and_sequence(&header), (tid, sequence));
        data.returned(DataReturn::TxDone {
            context: operation.context,
            ticket: operation.ticket,
            buffer: operation.buffer,
            completion: ended(End::Delivered, 1),
        });
    }
    assert_eq!(data.counters().lost_tokens, 0);
    assert_eq!(data.counters().overflows, 0);
    assert!(data.accepts());
}

#[test]
fn repeated_unpublished_returns_keep_the_checked_frame_and_payload() {
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    let mut operation = transmit(&mut data);
    let len = operation.context.frame().frame().len().get();
    let payload = memory(&operation.buffer)[34..len].to_vec();
    for _ in 0..3 {
        fresh_memory(&mut operation.buffer)[..26].fill(0);
        data.returned(DataReturn::TxDone {
            context: operation.context,
            ticket: operation.ticket,
            buffer: operation.buffer,
            completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
        });
        operation = transmit(&mut data);
        assert_eq!(operation.context.frame().frame().len().get(), len);
        let spec = operation.context.frame().frame().header().header();
        let mut header = [0; 26];
        oer_ieee80211_lower_mac::HeaderLayout::new(AckHeader::new(*spec).unwrap(), &mut header)
            .unwrap()
            .encode();
        assert_eq!(tid_and_sequence(&header), (6, 0));
        assert_eq!(&memory(&operation.buffer)[34..len], payload.as_slice());
    }
    data.returned(DataReturn::TxDone {
        context: operation.context,
        ticket: operation.ticket,
        buffer: operation.buffer,
        completion: ended(End::Delivered, 1),
    });
    assert_eq!(core::iter::from_fn(|| data.tickets.take()).count(), 2);
    assert_eq!(core::iter::from_fn(|| data.buffers.take()).count(), 2);
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
}

#[test]
fn a_late_old_link_terminal_does_not_take_a_new_links_metadata() {
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    let old = transmit(&mut data);
    data.leave();
    link.epoch = LinkEpoch::for_test(2);
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    let new = transmit(&mut data);
    assert!(!data.is_current(&old.context));
    assert!(data.is_current(&new.context));
    data.returned(DataReturn::TxDone {
        context: new.context,
        ticket: new.ticket,
        buffer: new.buffer,
        completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
    });
    data.returned(DataReturn::TxDone {
        context: old.context,
        ticket: old.ticket,
        buffer: old.buffer,
        completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
    });
    let again = transmit(&mut data);
    assert_eq!(again.ticket, 2);
    assert_eq!(again.context.order, 1);
    assert!(data.poll_action().is_none());
    assert_eq!(data.counters().left_behind, 1);
    data.returned(DataReturn::TxDone {
        context: again.context,
        ticket: again.ticket,
        buffer: again.buffer,
        completion: ended(End::Delivered, 1),
    });
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert!(data.tickets.take().is_some());
    assert!(data.tickets.take().is_some());
    assert!(data.tickets.take().is_none());
    assert!(data.buffers.take().is_some());
    assert!(data.buffers.take().is_some());
    assert!(data.buffers.take().is_none());
    assert_eq!(data.counters().lost_tokens, 0);
}

#[test]
fn a_local_refusal_releases_admission_with_the_operation_resources() {
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..8 {
        data.input(DataInput {
            msdu: msdu(6),
            link: &mut link,
        })
        .unwrap();
        let operation = transmit(&mut data);
        assert_eq!(data.available(WmmAccessCategory::Voice), capacity - 1);
        data.returned(DataReturn::Refused {
            context: operation.context,
            ticket: operation.ticket,
            buffer: operation.buffer,
            reason: crate::machine::TxRefusal::NoRate,
        });
        assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
        assert!(data.accepts());
        assert!(data.poll_action().is_none());
    }
    assert_eq!(data.counters().lost_tokens, 0);
}

#[test]
fn preparation_refusal_ends_only_its_msdu_without_a_radio_terminal_or_retry() {
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for priority in [6, 4] {
        data.input(DataInput {
            msdu: msdu(priority),
            link: &mut link,
        })
        .unwrap();
    }
    let rejected = transmit(&mut data);
    let accepted = transmit(&mut data);
    data.returned(DataReturn::Refused {
        context: rejected.context,
        ticket: rejected.ticket,
        buffer: rejected.buffer,
        reason: crate::machine::TxRefusal::Layout(
            oer_ieee80211_lower_mac::LayoutError::BodyOverlap,
        ),
    });
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert_eq!(data.available(WmmAccessCategory::Video), capacity - 1);
    assert_eq!(data.counters().preparation_refused, 1);
    assert!(data.poll_action().is_none());
    data.returned(DataReturn::TxDone {
        context: accepted.context,
        ticket: accepted.ticket,
        buffer: accepted.buffer,
        completion: ended(End::Delivered, 1),
    });
    assert_eq!(core::iter::from_fn(|| data.tickets.take()).count(), 2);
    assert_eq!(core::iter::from_fn(|| data.buffers.take()).count(), 2);
    assert_eq!(data.counters().lost_tokens, 0);
}

#[test]
fn a_protected_qos_msdu_leaves_room_for_the_ccmp_header_and_contends_by_its_priority() {
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
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
    let mut data = machine();
    let mut counter = StaSequenceCounter::new(SequenceNumber::new(42).unwrap());
    let mut link = open_link(&mut counter);
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut link,
    })
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
    let mut data = StaData::<Data, Caps>::new(
        [1],
        [Buffer(std::vec![0; 200])],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut counter = StaSequenceCounter::new(SequenceNumber::new(42).unwrap());
    let long = Msdu {
        payload: &[0; 200],
        ..msdu(0)
    };
    assert_eq!(
        data.input(DataInput {
            msdu: long,
            link: &mut open_link(&mut counter)
        }),
        Err(DataRefusal::TooLong)
    );
    assert_eq!(counter.peek(), SequenceNumber::new(42).unwrap());
    // The tokens stay, and a short MSDU takes the number.
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut open_link(&mut counter),
    })
    .unwrap();
    let sent = transmit(&mut data);
    assert_eq!(
        u16::from_le_bytes([memory(&sent.buffer)[22], memory(&sent.buffer)[23]]) >> 4,
        42
    );
}

#[test]
fn open_mpdu_fits_exactly_without_a_temporary_ccmp_gap() {
    let mut data = StaData::<Data, Caps>::new(
        [1],
        [Buffer(std::vec![0; 24 + 8 + 100])],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut counter = StaSequenceCounter::new(SequenceNumber::new(42).unwrap());
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut open_link(&mut counter),
    })
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
    let mut data = machine();
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut link,
    })
    .unwrap();
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut link,
    })
    .unwrap();
    assert!(!data.accepts());
    assert_eq!(
        data.input(DataInput {
            msdu: msdu(0),
            link: &mut link
        }),
        Err(DataRefusal::NoTokens)
    );
    let first = transmit(&mut data);
    data.returned(DataReturn::TxDone {
        context: first.context,
        ticket: first.ticket,
        buffer: first.buffer,
        completion: ended(End::Delivered, 1),
    });
    assert!(data.accepts());
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
    replay
        .install_group(GroupKeyId::new(2).unwrap(), [0; 8])
        .unwrap();
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
    let air = NonZeroU16::new(transmissions).map(|transmissions| oer_ieee80211_lower_mac::Air {
        transmissions,
        rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
    });
    TxOutcome::Ended(match status {
        End::Delivered => SingleEnd::Delivered {
            air: air.unwrap(),
            response: oer_ieee80211_lower_mac::SingleResponse::Ack(()),
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
    let mut data = StaData::<Data, Caps>::new(
        [1, 2, 3],
        [
            Buffer(std::vec![0; 1600]),
            Buffer(std::vec![0; 1600]),
            Buffer(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // Two MSDUs of TID 6: the first is submitted, then a newer one waits.
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    let first = transmit(&mut data);
    let sent = tid_and_sequence(memory(&first.buffer));
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    // A closing gate aborts the first before any transmission.
    data.returned(DataReturn::TxDone {
        context: first.context,
        ticket: first.ticket,
        buffer: first.buffer,
        completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
    });
    // It goes again at the head of its TID's queue, before the newer MSDU,
    // with the same sequence number, in its buffer.
    let again = transmit(&mut data);
    assert_eq!(tid_and_sequence(memory(&again.buffer)), sent);
    let newer = transmit(&mut data);
    assert_eq!(tid_and_sequence(memory(&newer.buffer)), (6, sent.1 + 1));
    assert!(data.poll_action().is_none());
    // One that made a transmission ends: its tokens come back.
    data.returned(DataReturn::TxDone {
        context: again.context,
        ticket: again.ticket,
        buffer: again.buffer,
        completion: ended(End::AckTimeout, 3),
    });
    assert!(data.poll_action().is_none());
}

#[test]
fn a_link_that_ends_drains_its_queue_and_its_submitted_attempts_never_go_again() {
    let mut data = StaData::<Data, Caps>::new(
        [1, 2, 3],
        [
            Buffer(std::vec![0; 1600]),
            Buffer(std::vec![0; 1600]),
            Buffer(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // One MPDU submitted and two owed on the link.
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut link,
    })
    .unwrap();
    let submitted = transmit(&mut data);
    data.input(DataInput {
        msdu: msdu(0),
        link: &mut link,
    })
    .unwrap();
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    assert!(!data.accepts());
    // The link ends: the owed ones drain into their tokens, counted.
    data.leave();
    assert!(data.poll_action().is_none());
    assert_eq!(data.counters().left_behind, 2);
    // The submitted one, aborted before the air, returns its tokens and
    // does not go again: the retry rule holds within one link.
    data.returned(DataReturn::TxDone {
        context: submitted.context,
        ticket: submitted.ticket,
        buffer: submitted.buffer,
        completion: TxOutcome::Withdrawn(WithdrawCause::GateClosed),
    });
    assert!(data.poll_action().is_none());
    assert_eq!(data.counters().left_behind, 3);
    assert_eq!(data.counters().lost_tokens, 0);
    // Every token is back: a new link takes three MSDUs again.
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    for _ in 0..3 {
        data.input(DataInput {
            msdu: msdu(0),
            link: &mut link,
        })
        .unwrap();
    }
    assert!(!data.accepts());
}

#[test]
fn a_category_the_service_does_not_admit_waits_and_the_others_go_in_their_order() {
    let mut data = StaData::<Data, Caps>::new(
        [1, 2, 3],
        [
            Buffer(std::vec![0; 1600]),
            Buffer(std::vec![0; 1600]),
            Buffer(std::vec![0; 1600]),
        ],
        core::num::NonZeroU8::new(8).unwrap(),
    );
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    // Voice (TID 6), then best effort (TID 0), then Voice again.
    for priority in [6, 0, 7] {
        data.input(DataInput {
            msdu: msdu(priority),
            link: &mut link,
        })
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
fn indeterminate_and_contention_limit_retire_the_msdu_and_release_admission() {
    for (outcome, indeterminate, contention) in [
        (
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout),
            1,
            0,
        ),
        (ended(End::ContentionLimit, 0), 0, 1),
    ] {
        let mut data = StaData::<Data, Caps>::new(
            [1],
            [Buffer(std::vec![0; 1600])],
            core::num::NonZeroU8::new(8).unwrap(),
        );
        let capacity = data.available(WmmAccessCategory::Voice);
        let mut replay = epoch();
        let mut link = qos_link(&(), &mut replay);
        data.input(DataInput {
            msdu: msdu(6),
            link: &mut link,
        })
        .unwrap();
        let operation = transmit(&mut data);
        let buffer = match operation.buffer {
            ReturnedMpdu::Fresh(buffer) => buffer,
            ReturnedMpdu::Sent(_) => panic!("a new MSDU is fresh"),
        };
        data.returned(DataReturn::TxDone {
            context: operation.context,
            ticket: operation.ticket,
            buffer: ReturnedMpdu::Sent(buffer),
            completion: outcome,
        });
        assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
        assert_eq!(data.counters().indeterminate, indeterminate);
        assert_eq!(data.counters().contention_limited, contention);
        assert_eq!(data.counters().lost_tokens, 0);
        assert!(
            data.poll_action().is_none(),
            "final outcomes never retry the MSDU"
        );
        data.input(DataInput {
            msdu: msdu(6),
            link: &mut link,
        })
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
    let mut data = machine();
    let capacity = data.available(WmmAccessCategory::Voice);
    let mut replay = epoch();
    let mut link = qos_link(&(), &mut replay);
    data.input(DataInput {
        msdu: msdu(6),
        link: &mut link,
    })
    .unwrap();
    let operation = transmit(&mut data);
    let ReturnedMpdu::Fresh(buffer) = operation.buffer else {
        unreachable!()
    };
    data.returned(DataReturn::TxDone {
        context: operation.context,
        ticket: operation.ticket,
        buffer: ReturnedMpdu::Sent(buffer),
        completion: TxOutcome::Withdrawn(WithdrawCause::KeyRemoved),
    });
    assert!(
        data.poll_action().is_none(),
        "the removed key's MSDU is not retried"
    );
    assert_eq!(data.available(WmmAccessCategory::Voice), capacity);
    assert_eq!(data.counters().lost_tokens, 0);
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
    impl Iterator<Item = Result<EthernetFrameParts<'f>, oer_ieee80211_mac::receive::RxDropReason>> + 'f,
    oer_ieee80211_mac::receive::RxDropReason,
> {
    use oer_ieee80211_mac::receive::{AdmittedData, RxDropReason};
    let protected_link = matches!(link.protection, LinkProtection::Ccmp { .. });
    let replay = match &mut link.protection {
        LinkProtection::Ccmp { replay, .. } => Some(&mut **replay),
        LinkProtection::Open => None,
    };
    let admitted = AdmittedData::admit(
        oer_ieee80211_mac::frame::DecodedHeader::decode(frame).ok_or(RxDropReason::Header)?,
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
