//! Comparison of the A-MPDU BlockAck completion with `ppResortTxAMPDU`.
//!
//! The vendor resorts one queue's completed aggregate after
//! `ppTxqUpdateBitmap` stored the BlockAck's starting sequence and bitmap in
//! the queue context: every MPDU the bitmap acknowledges is recycled, and
//! every other one is kept for the next aggregate with the IEEE 802.11 Retry
//! bit set in its frame. Production observes the same completion through its
//! retained A-MPDU owner and retry state and, when it retains the aggregate,
//! sets the Retry bit of every missing MPDU in that MPDU's own DMA backing.
//! Both sides hold the same encoded MPDUs; each case compares every MPDU's
//! header after the completion.
use crate::harness::{Arg, Result, case, direct, invalid, known, selection};
use crate::mac::{LEAF_FILLS, Mac};
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, ComparisonVerdict,
    EventChannels, ExecutionCase, MemoryPair, MemorySelection, RegionLifetime, ReturnWords,
    SessionReset,
};

/// The vendor function, linked as an extra root.
pub const ROOTS: &[&str] = &[VENDOR];
/// Evidence claims: the vendor resort with the production completion and
/// retry retention, and the vendor protection-failure leaf with the
/// production aggregate republication, and the acknowledgement-timeout leaf
/// with the production aggregate retry.
pub const CLAIMS: &[(&str, &str, &str)] = &[
    ("archive", VENDOR, RESORT_PROBE),
    ("archive", "lmacProcessCtsTimeout", TIMEOUT_STEP_PROBE),
    ("archive", "lmacProcessAckTimeout", TIMEOUT_STEP_PROBE),
];
const VENDOR: &str = "ppResortTxAMPDU";
const LAYOUT_PROBE: &str = "open_libpp_ampdu_trace_layout";
const RESORT_PROBE: &str = "open_libpp_ampdu_trace_resort";

/// The ROM pointer to the transmit context whose queue records the resort
/// reads, one of `QUEUE_STRIDE` bytes per queue.
const TX_RX: &str = "pTxRx";
const QUEUE_STRIDE: u32 = 0x34;
/// Queue-record fields: the ordinary queue's first `esf_buf`, the pending
/// count byte, the queue-valid byte, the
/// BlockAck starting sequence, the TID, the resorted aggregate's first
/// `esf_buf`, and the bitmap's high and low words.
const QUEUE_ORDINARY: u32 = 0x20;
const QUEUE_COUNT: u32 = 0x28;
const QUEUE_VALID: u32 = 0x29;
const QUEUE_STARTING_SEQUENCE: u32 = 0x2a;
const QUEUE_TID: u32 = 0x2c;
const QUEUE_AGGREGATE: u32 = 0x34;
const QUEUE_BITMAP_HIGH: u32 = 0x38;
const QUEUE_BITMAP_LOW: u32 = 0x3c;
/// Bytes of one queue record a case observes.
const QUEUE_RECORD_BYTES: u32 = 0x40;
/// Bytes of the transmit context the queue records occupy.
const TX_RX_BYTES: u32 = 0x200;
/// The queue the aggregate completed on, best effort, and the default TID.
const QUEUE: u32 = 2;
const TID: u8 = 0;

/// `esf_buf` fields: the first and the last DMA descriptor, the frame length,
/// the buffer flags, the station context, the next buffer and the transmit
/// descriptor. Flag 0x2000 marks a buffer that starts with the eight-byte
/// A-MPDU metadata before the frame.
const ESF_BYTES: u32 = 0x40;
const ESF_DMA: usize = 0x04;
const ESF_LAST_DMA: usize = 0x08;
const ESF_LENGTH: usize = 0x14;
const ESF_FLAGS: usize = 0x24;
const ESF_AMPDU_METADATA: u16 = 0x2000;
const ESF_STATION: usize = 0x2c;
const ESF_NEXT: usize = 0x30;
const ESF_DESCRIPTOR: usize = 0x34;
/// DMA descriptor: word 0 the buffer size and length with the end-of-frame
/// flag, word 1 the buffer, word 2 the next descriptor of the aggregate.
const DMA_BYTES: u32 = 0x10;
const DMA_FRAME: usize = 0x04;
const DMA_NEXT: usize = 0x08;
const DMA_LENGTH_SHIFT: u32 = 12;
const DMA_END_OF_FRAME: u32 = 1 << 30;
/// Transmit descriptor word 0 of an aggregate member: the A-MPDU marker,
/// and the first member's additional marker.
/// `ppResortTxAMPDU` copies a whole 0x48-byte descriptor to its stack.
const DESCRIPTOR_BYTES: u32 = 0x48;
const DESCRIPTOR_AMPDU: u32 = 0x0040_0000;
const DESCRIPTOR_FIRST: u32 = 0x0008_0000;
/// Transmit descriptor word 0 flag of an S-MPDU: `ppSelectTxFormat` sets it
/// when it takes the single-MPDU rate (`rcGetSMPDURate`).
const DESCRIPTOR_SMPDU: u32 = 0x4000_0000;
/// The access context `GetAccess` returns and the station context, whose
/// byte at `STATION_BAR_PENDING` holds one BlockAckReq-pending bit per TID.
const ACCESS_BYTES: u32 = 0x80;
/// The access context's end-of-exchange byte: `lmacEndFrameExchangeSequence`
/// writes 3 when a Trigger-based success (`lmacProcessTBSuccess`) ends an
/// S-MPDU, and `ppResortTxAMPDU` then recycles the whole aggregate without
/// reading a BlockAck.
const ACCESS_END: usize = 0x13;
const ACCESS_END_TRIGGER_SUCCESS: u8 = 3;
const STATION_BYTES: u32 = 0x40;
const STATION_BAR_PENDING: usize = 0x28;
/// Vendor arenas.
const ARENA_TX_RX: u32 = 0x3fff_a000;
const ARENA_ACCESS: u32 = 0x3fff_a200;
const ARENA_STATION: u32 = 0x3fff_a300;
const ARENA_ESF: u32 = 0x3fff_a400;
const ARENA_DMA: u32 = 0x3fff_a800;
const ARENA_DESCRIPTOR: u32 = 0x3fff_a900;
const ARENA_FRAMES: u32 = 0x3fff_b000;
/// Production input frames and output words.
const PRODUCTION_FRAMES: u32 = 0x3fff_c000;
const PRODUCTION_OUTPUT: u32 = 0x3fff_c400;
const OUTPUT_WORDS: u32 = 4;
const LAYOUT_OUTPUT: u32 = 0x3fff_c800;

/// Bytes of one encoded MPDU, as the probe commits it, and of the header
/// each case compares: a QoS data header through QoS Control.
const MPDU_BYTES: usize = 32;
/// The vendor A-MPDU metadata before each frame: word 0 carries the PSDU
/// length in bits 0..14 and the MPDU's sequence number low byte in bits
/// 16..24, which `ppResortTxAMPDU` measures against the BlockAck starting
/// sequence; byte 4 the empty delimiters. The PSDU adds the frame check
/// sequence.
const METADATA_BYTES: usize = 8;
const METADATA_SEQUENCE_SHIFT: u32 = 16;
const FCS_BYTES: usize = 4;
const BUFFER_BYTES: usize = METADATA_BYTES + MPDU_BYTES;
const HEADER_BYTES: u32 = 26;
const SLOTS: usize = 8;
/// A QoS data frame to the AP, and the header offsets of Sequence Control.
const FRAME_CONTROL: [u8; 2] = [0x88, 0x01];
const SEQUENCE_CONTROL: usize = 22;
const SEQUENCE_SHIFT: u16 = 4;
/// Frame Control's flags byte and its Retry bit.
const RETRY_BYTE: u32 = 1;
const RETRY: u8 = 0x08;
/// Guest events one resort may record.
const RESORT_EVENTS: u32 = 1 << 12;

/// Vendor callees answered with a constant: rate control, recycling,
/// the next transmission and the BlockAck request are outside the compared
/// frame state.
const ANSWERED: &[(&str, u32)] = &[
    ("rcUpdateTxDoneAmpdu2", 0),
    (AGED, 0),
    ("lmacRecycleMPDU", 0),
    ("lmacDiscardAgedMSDU", 0),
    ("esp_test_tx_count_retry", 0),
    ("ppResumeTxAMPDU", 0),
    ("ppProcessTxQ", 0),
    ("ppFillAMPDUBar", 0),
    ("ppReSendBar", 0),
    ("ppHEAMPDU2Normal", 0),
    ("wifi_log", 0),
];

/// Reviewed differences: production admits the MPDU left of the starting
/// sequence as acknowledged, and hands a single missing HT MPDU to its
/// ordinary retry owner.
const PREDECESSOR: &str = "Production admits the MPDU immediately left of SSN (within the \
    64-entry window) as acknowledged, because peers use both standard-compliant SSN \
    conventions and one advances SSN to the first unacknowledged MPDU. The vendor measures \
    low-byte distance and retries that MPDU, retransmitting an already delivered frame. \
    Production suppresses that duplicate; a sequence beyond the window stays unacknowledged in \
    both (reviewed with the Wi-Fi owner)";
const HT_SINGLE: &str = "production ends the HT aggregate when one MPDU is missing and hands \
    it to its ordinary retry owner, which sets the Retry bit when it republishes the MPDU; the \
    vendor keeps it in the aggregate and sets the bit here (reviewed with the Wi-Fi owner)";
/// Reviewed difference: where the Retry bit of an MPDU leaving the aggregate
/// is set.
const ORDINARY_RETRY: &str = "both sides hand the missing MPDUs to ordinary transmission once \
    the agreement ended; the vendor sets their Retry bit in the resort, production's ordinary \
    retry owner sets it on the copy it republishes, so the aggregate backing keeps the bit \
    clear (reviewed with the Wi-Fi owner)";
/// Reviewed difference: where an MSDU's lifetime starts.
const LIFETIME_ORIGIN: &str = "production starts an MSDU's lifetime when its aggregate is \
    committed, the vendor at the pp queue enqueue timestamp: a head queued long before the rest \
    of its aggregate expires on the vendor side only, which discards it without the Retry bit \
    while production retries it (reviewed with the Wi-Fi owner)";
/// Known gap: production sends no BlockAckReq.
const BAR_GAP: &str = "the vendor sends a BlockAckReq (ppFillAMPDUBar, ppReSendBar) with the \
    starting sequence after the aggregate head when a resort discards that head as aged, or \
    acknowledges it while the station has a request pending for the TID (set when rate control \
    resumes aggregation); production sends none, so the recipient's window advances only by \
    its own timeout (known gap, reviewed with the Wi-Fi owner)";
/// The vendor BlockAckReq builder: its TID and starting sequence arguments.
const FILL_BAR: &str = "ppFillAMPDUBar";
const FILL_BAR_TID: u16 = 0;
const FILL_BAR_SEQUENCE: u16 = 3;

/// The vendor's handover once the agreement ended.
const AGREEMENT_ENDED: &str = "after a resort whose BlockAck agreement is no longer operational \
    (trc_isTxAmpduOperational, trc_tid_isTxAmpduOperational), the vendor moves the remaining \
    MPDUs to the ordinary queue, converting a missing aggregate head through ppHEAMPDU2Normal";
/// Whether the station's BlockAck agreement for the TID is operational.
const OPERATIONAL: &[&str] = &["trc_isTxAmpduOperational", "trc_tid_isTxAmpduOperational"];
/// The vendor conversion of an aggregate head to an ordinary frame.
const TO_ORDINARY: &str = "ppHEAMPDU2Normal";

/// The vendor aging check, executed after `lmacInit` in the aging cases.
const AGED: &str = "lmacMSDUAged";

/// One completion: the aggregate's MPDUs, the BlockAck the peer sent, the
/// production retry policy for a single missing MPDU, whether the station
/// has a BlockAckReq pending for the TID, and the expected verdict with its
/// reviewed reason when it is a difference.
#[derive(Clone, Copy)]
struct Completion {
    label: &'static str,
    count: usize,
    first_sequence: u16,
    starting_sequence: u16,
    bitmap: u64,
    retain_single: bool,
    /// The aggregate's TID, which selects the vendor access category.
    tid: u8,
    bar_pending: bool,
    /// The starting sequence of the BlockAckReq the vendor sends, if any.
    bar: Option<u16>,
    /// The vendor's MPDU publication and short counters in each transmit
    /// descriptor.
    vendor_attempts: u8,
    /// How the vendor's own `lmacMSDUAged` sees the MPDUs, after `lmacInit`
    /// installed its lifetimes; answered as never aged when absent.
    aging: Option<Aging>,
    /// An expected difference: its reason, the MPDU whose Retry bit
    /// differs and whether the vendor side has the bit.
    difference: Option<Difference>,
    /// Whether the BlockAck agreement is still operational at the resort.
    operational: bool,
    /// An S-MPDU that a Trigger-based success ended: no BlockAck is read.
    trigger_based: bool,
    /// Where the aggregate's MPDUs go afterwards on each side, when the
    /// sides differ, with the reason.
    disposition: Option<(Disposition, Disposition, &'static str)>,
}

/// Where a completion leaves the aggregate's remaining MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Disposition {
    /// Kept in the aggregate for the next A-MPDU.
    Aggregate,
    /// Handed to the ordinary queue as individual frames.
    Ordinary,
    /// None kept: the aggregate ended.
    Finished,
}

#[derive(Clone, Copy)]
struct Difference {
    reason: &'static str,
    mpdu: u16,
    vendor_retry: bool,
}

/// Elapsed time since the MPDUs were queued, against the vendor lifetime.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Aging {
    Fresh,
    Expired,
    /// Only the head was queued a lifetime earlier than the others.
    HeadExpired,
    /// The head and the third MPDU were queued a lifetime earlier.
    HeadAndThirdExpired,
}

const fn completion(
    label: &'static str,
    count: usize,
    first_sequence: u16,
    starting_sequence: u16,
    bitmap: u64,
) -> Completion {
    Completion {
        label,
        count,
        first_sequence,
        starting_sequence,
        bitmap,
        retain_single: true,
        tid: TID,
        bar_pending: false,
        bar: None,
        vendor_attempts: 0,
        aging: None,
        difference: None,
        operational: true,
        trigger_based: false,
        disposition: None,
    }
}

/// Transmit descriptor bytes: the MPDU publication and short counters, and
/// the enqueue timestamp `lmacMSDUAged` measures from.
const DESCRIPTOR_PUBLICATIONS: usize = 5;
const DESCRIPTOR_SHORT: usize = 6;
const DESCRIPTOR_ENQUEUED: usize = 0x18;
/// The rate schedule record `rcReachRetryLimit` reads.
const DESCRIPTOR_RECORD: usize = 0x1c;
const ENQUEUED_US: u32 = 0x0100_0000;
/// Beyond the vendor lifetime by more than its own 1024-microsecond margin.
const EXPIRED_MARGIN_US: u32 = 4096;

const COMPLETIONS: &[Completion] = &[
    completion("all-acknowledged", 4, 100, 100, 0b1111),
    completion("none-acknowledged", 4, 100, 100, 0),
    completion("holes", 4, 100, 100, 0b0101),
    completion("tail-missing", 8, 200, 200, 0b0011_1111),
    // A starting sequence beyond the window acknowledges nothing.
    completion("beyond-window", 4, 100, 170, u64::MAX),
    // Sequence numbers wrap from 4095 to 0 inside the aggregate.
    completion("wrap", 4, 4094, 4094, 0b0110),
    // HE keeps a single missing MPDU in the aggregate.
    completion("single-missing-he", 4, 100, 100, 0b1011),
    Completion {
        retain_single: false,
        difference: Some(Difference {
            reason: HT_SINGLE,
            mpdu: 2,
            vendor_retry: true,
        }),
        disposition: Some((Disposition::Aggregate, Disposition::Ordinary, HT_SINGLE)),
        ..completion("single-missing-ht", 4, 100, 100, 0b1011)
    },
    // The peer advanced SSN past the first MPDU, already delivered.
    Completion {
        difference: Some(Difference {
            reason: PREDECESSOR,
            mpdu: 0,
            vendor_retry: true,
        }),
        ..completion("predecessor", 4, 100, 101, 0b0101)
    },
    // The head is acknowledged while a request is pending: the vendor
    // requests a BlockAck from the sequence after it.
    Completion {
        bar_pending: true,
        bar: Some(101),
        ..completion("bar-pending", 4, 100, 100, 0b0101)
    },
    // The vendor keeps retrying whatever its MPDU counters say.
    Completion {
        vendor_attempts: u8::MAX,
        ..completion("vendor-counters-exhausted", 4, 100, 100, 0b0101)
    },
    // Both sides keep a fresh MPDU and discard an expired one: the vendor
    // by its own aging after lmacInit, production by the same lifetime.
    Completion {
        aging: Some(Aging::Fresh),
        ..completion("lifetime-fresh", 4, 100, 100, 0b0101)
    },
    Completion {
        aging: Some(Aging::Expired),
        ..completion("lifetime-expired", 4, 100, 100, 0b0101)
    },
    // A head queued long before the rest: the vendor discards it alone,
    // production ages the aggregate from its commit and keeps it. Discarding
    // the aged head moves the recipient's window past it with a
    // BlockAckReq, pending request or not.
    Completion {
        aging: Some(Aging::HeadExpired),
        bar: Some(101),
        difference: Some(Difference {
            reason: LIFETIME_ORIGIN,
            mpdu: 0,
            vendor_retry: false,
        }),
        ..completion("lifetime-expired-head", 4, 100, 100, 0b1010)
    },
    // The head and a later MPDU both aged out, as when every MPDU of the
    // aggregate starts its lifetime at the same moment: discarding the later
    // one after the head leaves the vendor without a BlockAckReq.
    Completion {
        aging: Some(Aging::Expired),
        ..completion("lifetime-expired-head-and-later", 4, 100, 100, 0b1010)
    },
    // The head and a later MPDU aged out while the last MPDU stays: the
    // later discard cancels the head's BlockAckReq.
    Completion {
        aging: Some(Aging::HeadAndThirdExpired),
        difference: Some(Difference {
            reason: LIFETIME_ORIGIN,
            mpdu: 0,
            vendor_retry: false,
        }),
        ..completion("lifetime-expired-head-and-third", 4, 100, 100, 0b0010)
    },
    // Only the head is missing and aged and the rest are acknowledged: the
    // aggregate ends empty, and an empty aggregate sends no BlockAckReq.
    Completion {
        aging: Some(Aging::Expired),
        ..completion("lifetime-expired-head-only", 4, 100, 100, 0b1110)
    },
    // The head is acknowledged while a request is pending, and a later
    // MPDU aged out: the later discard likewise cancels the request.
    Completion {
        bar_pending: true,
        aging: Some(Aging::Expired),
        ..completion("bar-pending-later-aged", 4, 100, 100, 0b0101)
    },
    // The agreement ended before the resort: both sides send the missing
    // MPDUs individually.
    Completion {
        operational: false,
        difference: Some(Difference {
            reason: ORDINARY_RETRY,
            mpdu: 1,
            vendor_retry: true,
        }),
        ..completion("agreement-ended", 4, 100, 100, 0b0101)
    },
    // With the head missing too, the vendor converts it to an ordinary frame.
    Completion {
        operational: false,
        difference: Some(Difference {
            reason: ORDINARY_RETRY,
            mpdu: 0,
            vendor_retry: true,
        }),
        ..completion("agreement-ended-head-missing", 4, 100, 100, 0b1010)
    },
    // Each access category the vendor maps a TID to: background, video,
    // voice; a request pending for the TID is answered on it.
    Completion {
        tid: 1,
        ..completion("tid-background", 4, 100, 100, 0b0101)
    },
    Completion {
        tid: 4,
        bar_pending: true,
        bar: Some(101),
        ..completion("tid-video-bar", 4, 100, 100, 0b0101)
    },
    Completion {
        tid: 6,
        ..completion("tid-voice", 4, 100, 100, 0b1001)
    },
    // MPDUs 32 and more past the starting sequence use the bitmap's high
    // word.
    completion("high-bitmap-word", 4, 132, 100, 0b1010 << 32),
    // Nothing is left to hand over when every MPDU was acknowledged.
    Completion {
        operational: false,
        ..completion("agreement-ended-all-acknowledged", 4, 100, 100, 0b1111)
    },
    // A Trigger-based success ends an S-MPDU without a BlockAck: neither
    // side marks the MPDU, and both end the aggregate.
    Completion {
        trigger_based: true,
        ..completion("trigger-based-smpdu", 1, 100, 100, 0)
    },
];

fn answered(name: &str, address: u32, boundary: CallBoundary, value: u32) -> CallDeclaration {
    CallDeclaration {
        id: name.into(),
        applicability: "a resort callee outside the compared frame state".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary,
            allow_tail: true,
        },
        argument_words: 4,
        responses: vec![CallResponse {
            return_words: [Some(value), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// One vendor aggregate: an `esf_buf`, DMA descriptor, transmit descriptor
/// and buffer with A-MPDU metadata and frame per MPDU, linked in order.
struct Aggregate {
    count: usize,
    first_sequence: u16,
    /// Transmit-descriptor word 0 flags beyond the A-MPDU markers.
    descriptor_flags: u32,
    vendor_attempts: u8,
    /// The rate schedule record every descriptor names, with its address.
    record: Option<u32>,
}

impl Aggregate {
    /// The aggregate's bytes, as (address, bytes) regions.
    fn regions(&self) -> Vec<(u32, Vec<u8>)> {
        let count = self.count;
        let mut esf = vec![0u8; count * ESF_BYTES as usize];
        let mut dma = vec![0u8; count * DMA_BYTES as usize];
        let mut descriptors = vec![0u8; count * DESCRIPTOR_BYTES as usize];
        let mut buffers = vec![0u8; count * BUFFER_BYTES];
        for index in 0..count {
            let buffer = &mut esf[index * ESF_BYTES as usize..(index + 1) * ESF_BYTES as usize];
            word(buffer, ESF_DMA, ARENA_DMA + index as u32 * DMA_BYTES);
            word(buffer, ESF_LAST_DMA, ARENA_DMA + index as u32 * DMA_BYTES);
            buffer[ESF_LENGTH..ESF_LENGTH + 2].copy_from_slice(&(MPDU_BYTES as u16).to_le_bytes());
            buffer[ESF_FLAGS..ESF_FLAGS + 2].copy_from_slice(&ESF_AMPDU_METADATA.to_le_bytes());
            word(buffer, ESF_STATION, ARENA_STATION);
            let next = if index + 1 < count {
                ARENA_ESF + (index as u32 + 1) * ESF_BYTES
            } else {
                0
            };
            word(buffer, ESF_NEXT, next);
            word(
                buffer,
                ESF_DESCRIPTOR,
                ARENA_DESCRIPTOR + index as u32 * DESCRIPTOR_BYTES,
            );
            let descriptor = &mut dma[index * DMA_BYTES as usize..];
            let size = BUFFER_BYTES as u32;
            let end = if index + 1 == count {
                DMA_END_OF_FRAME
            } else {
                0
            };
            word(descriptor, 0, size | size << DMA_LENGTH_SHIFT | end);
            word(
                descriptor,
                DMA_FRAME,
                ARENA_FRAMES + (index * BUFFER_BYTES) as u32,
            );
            let next_dma = if index + 1 < count {
                ARENA_DMA + (index as u32 + 1) * DMA_BYTES
            } else {
                0
            };
            word(descriptor, DMA_NEXT, next_dma);
            let flags = DESCRIPTOR_AMPDU
                | self.descriptor_flags
                | if index == 0 { DESCRIPTOR_FIRST } else { 0 };
            let descriptor = &mut descriptors[index * DESCRIPTOR_BYTES as usize..];
            word(descriptor, 0, flags);
            descriptor[DESCRIPTOR_PUBLICATIONS] = self.vendor_attempts;
            descriptor[DESCRIPTOR_SHORT] = self.vendor_attempts;
            word(descriptor, DESCRIPTOR_ENQUEUED, ENQUEUED_US);
            if let Some(record) = self.record {
                word(descriptor, DESCRIPTOR_RECORD, record);
            }
            let buffer = &mut buffers[index * BUFFER_BYTES..(index + 1) * BUFFER_BYTES];
            let sequence = u32::from(self.first_sequence.wrapping_add(index as u16) & 0xff);
            let metadata = (MPDU_BYTES + FCS_BYTES) as u32 | sequence << METADATA_SEQUENCE_SHIFT;
            word(buffer, 0, metadata);
            buffer[METADATA_BYTES..].copy_from_slice(&frame(self.first_sequence, index));
        }
        vec![
            (ARENA_ESF, esf),
            (ARENA_DMA, dma),
            (ARENA_DESCRIPTOR, descriptors),
            (ARENA_FRAMES, buffers),
        ]
    }
}

/// Where the vendor side keeps MPDU `index`'s frame.
const fn vendor_frame(index: usize) -> u32 {
    ARENA_FRAMES + (index * BUFFER_BYTES + METADATA_BYTES) as u32
}

/// The encoded MPDU at `index` of an aggregate starting at `first`.
fn frame(first: u16, index: usize) -> Vec<u8> {
    let mut frame = vec![0u8; MPDU_BYTES];
    frame[..2].copy_from_slice(&FRAME_CONTROL);
    for (offset, byte) in frame[4..22].iter_mut().enumerate() {
        *byte = 0x10 + offset as u8;
    }
    let sequence = first.wrapping_add(index as u16) & 0x0fff;
    frame[SEQUENCE_CONTROL..SEQUENCE_CONTROL + 2]
        .copy_from_slice(&(sequence << SEQUENCE_SHIFT).to_le_bytes());
    for (offset, byte) in frame[26..].iter_mut().enumerate() {
        *byte = index as u8 * 0x10 + offset as u8;
    }
    frame
}

fn word(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

/// `lmacConfMib`, which `lmacInit` fills: the MSDU lifetimes `lmacMSDUAged`
/// reads, in units of 1024 microseconds, of an aggregate member (word 0)
/// and of an ordinary MPDU (word 2).
const CONF: &str = "lmacConfMib";
const CONF_LIFETIMES_BYTES: u32 = 0x16;
/// The short retry limit byte `lmacProcessShortRetryFail` compares.
const CONF_SHORT_LIMIT: usize = 0x15;
const LIFETIME_AGGREGATE: usize = 0;
const LIFETIME_ORDINARY: usize = 8;
const LIFETIME_UNIT_SHIFT: u32 = 10;

/// What the vendor's `lmacInit` installs: the aggregate and ordinary MSDU
/// lifetimes, in microseconds, and the short retry limit.
struct VendorConf {
    aggregate_lifetime: u32,
    ordinary_lifetime: u32,
    short_limit: u8,
}

fn vendor_conf(ctx: &mut Mac) -> Result<VendorConf> {
    let image = ctx.image_symbols()?;
    let address = ctx.symbol_address(&image, CONF)?;
    let init =
        crate::retry::lmac_init(ctx, &image, vec![selection(address, CONF_LIFETIMES_BYTES)])?;
    let mut row = case("lmac-init-lifetimes", init, None, SessionReset::Cold, false);
    row.relation = None;
    row.stack_fill = Some(LEAF_FILLS[0]);
    let vendor = ctx.vendor.clone();
    let records = ctx
        .submit(
            "lmac-init-lifetimes",
            &crate::session::request(&vendor, None, None, vec![row], RESORT_EVENTS),
            None,
        )?
        .records
        .clone();
    let conf = crate::evidence::output(&records, 0, false);
    let at = |offset: usize| -> Result<u32> {
        let bytes = conf
            .get(offset..offset + 4)
            .ok_or_else(|| invalid("lmacInit left no MSDU lifetimes"))?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) << LIFETIME_UNIT_SHIFT)
    };
    Ok(VendorConf {
        aggregate_lifetime: at(LIFETIME_AGGREGATE)?,
        ordinary_lifetime: at(LIFETIME_ORDINARY)?,
        short_limit: *conf
            .get(CONF_SHORT_LIMIT)
            .ok_or_else(|| invalid("lmacInit left no short retry limit"))?,
    })
}

/// The production layout entry, which also serves as the production side of
/// a setup phase: it claims and returns every pool slot.
fn layout_invocation(ctx: &Mac) -> Result<blobray_domain::Invocation> {
    let mut production = ctx.session.probes.invoke(
        LAYOUT_PROBE,
        vec![("output", Arg::Word(Some(i64::from(LAYOUT_OUTPUT))))],
        vec![],
        vec![selection(LAYOUT_OUTPUT, SLOTS as u32 * 4)],
    )?;
    production
        .memory
        .push(known(LAYOUT_OUTPUT, SLOTS as u32 * 4, &[0; SLOTS * 4])?);
    production.arguments.resize(8, Some(0));
    Ok(production)
}

/// Where the production probe keeps each MPDU, from its layout entry.
fn layout(ctx: &mut Mac) -> Result<Vec<u32>> {
    let production = layout_invocation(ctx)?;
    let memcpy = u32::try_from(
        crate::harness::symbol(
            &ctx.session.inventory,
            crate::layout::ROM_INPUT as usize,
            "memcpy",
        )?
        .value,
    )?;
    let vendor = direct(
        memcpy,
        &[LAYOUT_OUTPUT, LAYOUT_OUTPUT, 0],
        vec![],
        vec![],
        vec![],
    );
    let mut row = crate::harness::setup(
        "ampdu-resort-layout",
        vendor,
        production,
        SessionReset::Cold,
    );
    row.stack_fill = Some(LEAF_FILLS[0]);
    let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
    let records = ctx
        .submit(
            "ampdu-resort-layout",
            &crate::session::request(&vendor, Some(&production), None, vec![row], RESORT_EVENTS),
            Some(ComparisonVerdict::Match),
        )?
        .records
        .clone();
    if crate::i2c::returned_low(&records, 0, true) != Some(0) {
        return Err(invalid("the A-MPDU layout entry failed"));
    }
    Ok(crate::evidence::output(&records, 0, true)
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .collect())
}

/// The compared case of `completion`.
fn case_rows(
    ctx: &mut Mac,
    frames: &[u32],
    lifetime: u32,
    completion: Completion,
) -> Result<Vec<ExecutionCase>> {
    let image = ctx.image_symbols()?;
    let symbol = |name: &str| ctx.symbol_address(&image, name);
    let count = completion.count;
    let encoded: Vec<Vec<u8>> = (0..count)
        .map(|index| frame(completion.first_sequence, index))
        .collect();

    // The vendor transmit context, access and station contexts and one
    // `esf_buf`, DMA descriptor, transmit descriptor and queue word per MPDU.
    let mut tx_rx = vec![0u8; TX_RX_BYTES as usize];
    let record = (QUEUE * QUEUE_STRIDE) as usize;
    tx_rx[record + QUEUE_COUNT as usize] = count as u8;
    tx_rx[record + QUEUE_VALID as usize] = 1;
    tx_rx[record + QUEUE_STARTING_SEQUENCE as usize..record + QUEUE_STARTING_SEQUENCE as usize + 2]
        .copy_from_slice(&completion.starting_sequence.to_le_bytes());
    tx_rx[record + QUEUE_TID as usize] = completion.tid;
    word(&mut tx_rx, record + QUEUE_AGGREGATE as usize, ARENA_ESF);
    word(
        &mut tx_rx,
        record + QUEUE_BITMAP_HIGH as usize,
        (completion.bitmap >> 32) as u32,
    );
    word(
        &mut tx_rx,
        record + QUEUE_BITMAP_LOW as usize,
        completion.bitmap as u32,
    );
    let mut access_context = vec![0u8; ACCESS_BYTES as usize];
    if completion.trigger_based {
        access_context[ACCESS_END] = ACCESS_END_TRIGGER_SUCCESS;
    }
    let mut station = vec![0u8; STATION_BYTES as usize];
    if completion.bar_pending {
        station[STATION_BAR_PENDING] = 1 << completion.tid;
    }
    let mut memory = vec![
        known(symbol(TX_RX)?, 4, &ARENA_TX_RX.to_le_bytes())?,
        known(ARENA_TX_RX, TX_RX_BYTES, &tx_rx)?,
        known(ARENA_ACCESS, ACCESS_BYTES, &access_context)?,
        known(ARENA_STATION, STATION_BYTES, &station)?,
    ];
    let aggregate = Aggregate {
        count,
        first_sequence: completion.first_sequence,
        descriptor_flags: if completion.trigger_based {
            DESCRIPTOR_SMPDU
        } else {
            0
        },
        vendor_attempts: completion.vendor_attempts,
        record: None,
    };
    for (address, mut bytes) in aggregate.regions() {
        if address == ARENA_DESCRIPTOR {
            let aged: &[usize] = match completion.aging {
                Some(Aging::HeadExpired) => &[0],
                Some(Aging::HeadAndThirdExpired) => &[0, 2],
                _ => &[],
            };
            for index in aged {
                word(
                    &mut bytes,
                    index * DESCRIPTOR_BYTES as usize + DESCRIPTOR_ENQUEUED,
                    ENQUEUED_US - lifetime - EXPIRED_MARGIN_US,
                );
            }
        }
        memory.push(known(address, bytes.len() as u32, &bytes)?);
    }
    let header = |name: String, address: u32| MemorySelection {
        name,
        address,
        length: HEADER_BYTES,
    };
    // After the headers, the vendor queue record: where its MPDUs went.
    let mut vendor_observed: Vec<MemorySelection> = (0..count)
        .map(|index| header(format!("mpdu-{index}-header"), vendor_frame(index)))
        .collect();
    vendor_observed.push(MemorySelection {
        name: "queue-record".into(),
        address: ARENA_TX_RX + QUEUE * QUEUE_STRIDE,
        length: QUEUE_RECORD_BYTES,
    });
    let mut vendor = direct(symbol(VENDOR)?, &[QUEUE], memory, vec![], vendor_observed);
    for name in OPERATIONAL {
        vendor.calls.push(answered(
            name,
            symbol(name)?,
            crate::mac::call_boundary(&image, name),
            u32::from(completion.operational),
        ));
    }
    let access = symbol("GetAccess")?;
    vendor.calls.push(answered(
        "GetAccess",
        access,
        crate::mac::call_boundary(&image, "GetAccess"),
        ARENA_ACCESS,
    ));
    let elapsed = match completion.aging {
        None | Some(Aging::Fresh) | Some(Aging::HeadExpired) | Some(Aging::HeadAndThirdExpired) => {
            0
        }
        Some(Aging::Expired) => lifetime + EXPIRED_MARGIN_US,
    };
    if completion.aging.is_some() {
        vendor.calls.push(answered(
            "hal_now",
            symbol("hal_now")?,
            crate::mac::call_boundary(&image, "hal_now"),
            ENQUEUED_US + elapsed,
        ));
    }
    for (name, value) in ANSWERED {
        if *name == AGED && completion.aging.is_some() {
            continue;
        }
        vendor.calls.push(answered(
            name,
            symbol(name)?,
            crate::mac::call_boundary(&image, name),
            *value,
        ));
    }

    // After the headers, the production decision words.
    let mut production_observed: Vec<MemorySelection> = (0..count)
        .map(|index| header(format!("mpdu-{index}-header"), frames[index]))
        .collect();
    production_observed.push(MemorySelection {
        name: "decision".into(),
        address: PRODUCTION_OUTPUT,
        length: OUTPUT_WORDS * 4,
    });
    let mut production = ctx.session.probes.invoke(
        RESORT_PROBE,
        vec![
            ("frames", Arg::Word(Some(i64::from(PRODUCTION_FRAMES)))),
            ("count", Arg::Word(Some(count as i64))),
            (
                "first_sequence",
                Arg::Word(Some(i64::from(completion.first_sequence))),
            ),
            (
                "starting_sequence",
                Arg::Word(Some(i64::from(completion.starting_sequence))),
            ),
            (
                "bitmap_low",
                Arg::Word(Some(i64::from(completion.bitmap as u32))),
            ),
            (
                "bitmap_high",
                Arg::Word(Some(i64::from((completion.bitmap >> 32) as u32))),
            ),
            ("received", Arg::Word(Some(1))),
            ("lifetime_micros", Arg::Word(Some(i64::from(lifetime)))),
            ("elapsed_micros", Arg::Word(Some(i64::from(elapsed)))),
            (
                "retain_single",
                Arg::Word(Some(i64::from(completion.retain_single))),
            ),
            (
                "trigger_flow",
                Arg::Word(Some(i64::from(completion.trigger_based))),
            ),
            (
                "block_ack_operational",
                Arg::Word(Some(i64::from(completion.operational))),
            ),
            ("output", Arg::Word(Some(i64::from(PRODUCTION_OUTPUT)))),
        ],
        vec![],
        production_observed,
    )?;
    let joined: Vec<u8> = encoded.concat();
    production.memory.extend([
        known(PRODUCTION_FRAMES, joined.len() as u32, &joined)?,
        known(
            PRODUCTION_OUTPUT,
            OUTPUT_WORDS * 4,
            &vec![0; (OUTPUT_WORDS * 4) as usize],
        )?,
    ]);
    // The aging cases start from the vendor's own lmacInit, which installs
    // the lifetimes `lmacMSDUAged` reads.
    let mut rows = vec![];
    if completion.aging.is_some() {
        let init = crate::retry::lmac_init(ctx, &image, vec![])?;
        let noop = layout_invocation(ctx)?;
        let mut setup = crate::harness::setup(
            format!("ampdu-resort-{}-init", completion.label),
            init,
            noop,
            SessionReset::Cold,
        );
        setup.stack_fill = Some(LEAF_FILLS[0]);
        rows.push(setup);
    }
    let reset = if rows.is_empty() {
        SessionReset::Cold
    } else {
        SessionReset::Warm
    };
    let mut row = case(
        format!("ampdu-resort-{}", completion.label),
        vendor,
        Some(production),
        reset,
        false,
    );
    let relation = row.relation.as_mut().expect("a compared case");
    relation.events = EventChannels {
        timeline: crate::harness::TIMELINE,
        mmio_read: false,
        mmio_write: false,
        fence: false,
        delay: false,
    };
    relation.returns = ReturnWords {
        low: false,
        high: false,
    };
    relation.memory = (0..count as u16)
        .map(|index| MemoryPair {
            vendor: index,
            replacement: index,
        })
        .collect();
    row.stack_fill = Some(LEAF_FILLS[0]);
    rows.push(row);
    Ok(rows)
}

/// The production decision that ends an aggregate through the Trigger-based
/// completion path.
const DECISION_TRIGGER: u32 = 4;
/// The production decision that hands the missing MPDUs to ordinary
/// transmission.
const DECISION_UNAGGREGATE: u32 = 5;

fn read_word(bytes: &[u8], offset: u32) -> Result<u32> {
    let offset = offset as usize;
    bytes
        .get(offset..offset + 4)
        .map(|b| u32::from_le_bytes(b.try_into().expect("four bytes")))
        .ok_or_else(|| invalid("an observed selection is too short"))
}

/// Where each side left the aggregate's MPDUs: the vendor by its queue
/// record, production by its decision.
fn dispositions(
    records: &[blobray_domain::ExecutionEvidence],
    case: u32,
    count: usize,
) -> Result<(Disposition, Disposition)> {
    let queue = crate::evidence::selection(records, case, false, count as u16);
    let vendor = match (
        read_word(&queue, QUEUE_AGGREGATE)?,
        read_word(&queue, QUEUE_ORDINARY)?,
    ) {
        (0, 0) => Disposition::Finished,
        (0, _) => Disposition::Ordinary,
        _ => Disposition::Aggregate,
    };
    let decision = read_word(
        &crate::evidence::selection(records, case, true, count as u16),
        0,
    )?;
    let production = match decision {
        STEP_RETAIN => Disposition::Aggregate,
        STEP_FINISH | DECISION_TRIGGER => Disposition::Finished,
        DECISION_UNAGGREGATE => Disposition::Ordinary,
        other => return Err(invalid(format!("production decided {other}"))),
    };
    Ok((vendor, production))
}

/// Compare every completion: each matches, or differs for its reviewed
/// reason; both sides leave the MPDUs in the same place unless a reviewed
/// difference or known gap says otherwise; production succeeds in every
/// case, and the vendor sends its BlockAckReq exactly when the station has
/// one pending and converts the aggregate head exactly when it hands a
/// missing head to the ordinary queue.
pub fn exercise(ctx: &mut Mac) -> Result<()> {
    let frames = layout(ctx)?;
    let conf = vendor_conf(ctx)?;
    let aggregate = conf.aggregate_lifetime;
    println!(
        "vendor MSDU lifetimes: aggregate {aggregate} us, ordinary {} us",
        conf.ordinary_lifetime
    );
    let image = ctx.image_symbols()?;
    let request_bar = ctx.symbol_address(&image, "ppReSendBar")?;
    let to_ordinary = ctx.symbol_address(&image, TO_ORDINARY)?;
    let fill_bar = ctx.symbol_address(&image, FILL_BAR)?;
    for completion in COMPLETIONS {
        let rows = case_rows(ctx, &frames, aggregate, *completion)?;
        let compared = rows.len() as u32 - 1;
        let label = format!("ampdu-resort-{}", completion.label);
        let expected = match completion.difference {
            Some(_) => ComparisonVerdict::Diff,
            None => ComparisonVerdict::Match,
        };
        let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
        let records = ctx
            .submit(
                &label,
                &crate::session::request(&vendor, Some(&production), None, rows, RESORT_EVENTS),
                Some(expected),
            )?
            .records
            .clone();
        let status = crate::i2c::returned_low(&records, compared, true);
        if status != Some(0) {
            return Err(invalid(format!("{label}: production reported {status:?}")));
        }
        if let Some(difference) = completion.difference {
            let at = records.iter().find_map(|record| match record {
                blobray_domain::ExecutionEvidence::Comparison { result, .. } => {
                    result.difference.clone()
                }
                _ => None,
            });
            let (with, without) = (FRAME_CONTROL[1] | RETRY, FRAME_CONTROL[1]);
            let (vendor_byte, replacement_byte) = if difference.vendor_retry {
                (with, without)
            } else {
                (without, with)
            };
            let expected_at = blobray_domain::ComparisonDifference::Memory {
                pair: difference.mpdu,
                offset: RETRY_BYTE,
                vendor: vendor_byte,
                replacement: replacement_byte,
            };
            if at.as_ref() != Some(&expected_at) {
                return Err(invalid(format!(
                    "{label}: differs at {at:x?}, not at MPDU {}'s Retry bit: {}",
                    difference.mpdu, difference.reason
                )));
            }
        }
        let found = dispositions(&records, compared, completion.count)?;
        let expected = match completion.disposition {
            Some((vendor, production, _)) => (vendor, production),
            None => (found.0, found.0),
        };
        if found != expected {
            return Err(invalid(format!(
                "{label}: the vendor left the MPDUs {:?} and production {:?}, expected {expected:?}{}",
                found.0,
                found.1,
                completion
                    .disposition
                    .map(|(_, _, reason)| format!(": {reason}"))
                    .unwrap_or_default()
            )));
        }
        let called = |address: u32| {
            crate::evidence::events(&records, compared, false)
                .iter()
                .any(|event| {
                    matches!(event, blobray_domain::ExecutionEvent::ModeledCall { target, .. } if *target == address)
                })
        };
        let head_missing = completion.bitmap & 1 == 0;
        let converted = found.0 == Disposition::Ordinary && head_missing;
        if called(to_ordinary) != converted {
            return Err(invalid(format!(
                "{label}: the vendor {}converted the aggregate head: {AGREEMENT_ENDED}",
                if converted { "has not " } else { "" }
            )));
        }
        let bar = called(request_bar)
            .then(|| {
                crate::evidence::modeled_calls(
                    &crate::evidence::events(&records, compared, false),
                    fill_bar,
                )
                .into_iter()
                .next()
            })
            .flatten()
            .map(|words| {
                let word = |index: u16| words.get(usize::from(index)).copied().flatten();
                (word(FILL_BAR_TID), word(FILL_BAR_SEQUENCE))
            });
        let expected_bar = completion
            .bar
            .map(|sequence| (Some(u32::from(completion.tid)), Some(u32::from(sequence))));
        if bar != expected_bar {
            return Err(invalid(format!(
                "{label}: the vendor BlockAckReq (TID, starting sequence) was {bar:?}, expected {expected_bar:?}: {BAR_GAP}"
            )));
        }
    }
    Ok(())
}

/// One kind of completion without a BlockAck: the vendor leaf, the
/// completion status production observes and the decision it makes while
/// the aggregate continues.
#[derive(Clone, Copy)]
struct Timeout {
    label: &'static str,
    leaf: &'static str,
    status: u32,
    continuing: u32,
    /// Whether the vendor's rate record limit bounds it besides the short
    /// retry limit.
    record_bounded: bool,
    /// Where the vendor ends an RTS-protected aggregate at its limit.
    protected_end: (&'static str, Option<u32>),
    /// Production's decision at the limit of an RTS-protected aggregate.
    protected_decision: u32,
    /// Whether the timeout occurs without RTS protection: a CTS timeout
    /// answers an RTS, so the vendor's every RTS decision marks the
    /// descriptor with [`DESCRIPTOR_RTS`] before it.
    unprotected: bool,
}

/// How the vendor ends an aggregate at its limit: the frame exchange ends
/// without a retry (third argument zero) and the aggregate is recycled,
/// which production's Finish matches.
const END_EXCHANGE: (&str, Option<u32>) = ("lmacEndFrameExchangeSequence", Some(0));

const TIMEOUTS: [Timeout; 2] = [
    Timeout {
        label: "ack-timeout",
        leaf: "lmacProcessAckTimeout",
        status: 5,
        continuing: STEP_RETAIN,
        record_bounded: true,
        protected_end: END_EXCHANGE,
        protected_decision: STEP_FINISH,
        unprotected: true,
    },
    // A protection failure sent no MPDU: neither side sets the Retry bit.
    // At the short retry limit the vendor's lmacEndRetryAMPDUFail keeps the
    // aggregate and requests its BlockAck from the head (ppFillAMPDUBar,
    // ppReSendBar); production requests it too. Whether the aggregate is
    // longer than the RTS threshold only selects the retry-limit byte
    // lmacProcessShortRetryFail records, not this continuation.
    Timeout {
        label: "cts-timeout",
        leaf: "lmacProcessCtsTimeout",
        status: 2,
        continuing: STEP_REPUBLISH,
        record_bounded: false,
        protected_end: ("lmacEndRetryAMPDUFail", None),
        protected_decision: STEP_REQUEST_BLOCK_ACK,
        unprotected: false,
    },
];
/// The production entries of a timeout sequence.
const TIMEOUT_BEGIN_PROBE: &str = "open_libpp_ampdu_trace_timeout_begin";
const TIMEOUT_STEP_PROBE: &str = "open_libpp_ampdu_trace_timeout_step";
/// The lmac queue contexts `lmacInit` builds at the address the retry
/// scenario gives it, and the fields the timeout sequence installs: the
/// transmitting `esf_buf` and the exchange state, transmitting.
const LMAC_QUEUES: u32 = 0x3fff_2000;
const LMAC_QUEUE_BYTES: u32 = 0x38;
const LMAC_QUEUE_BUFFER: u32 = 0x00;
const LMAC_QUEUE_STATE: u32 = 0x12;
const LMAC_TRANSMITTING: u8 = 1;
/// Source of the queue-context patches, and the rate schedule record.
const PATCH: u32 = 0x3fff_ad00;
const ARENA_RECORD: u32 = 0x3fff_ae00;
/// An HT MCS 0 long-guard-interval aggregate, the frames' rate.
const HT_MCS0: u32 = 0x10;
/// Transmit-descriptor flag of an RTS-protected frame: lmacTxFrame and
/// ppCheckTxRTS set it with every RTS decision, and the limits of an
/// aggregate carrying it end in lmacEndRetryAMPDUFail.
const DESCRIPTOR_RTS: u32 = 0x100;
/// Decisions the production step reports.
const STEP_RETAIN: u32 = 1;
const STEP_FINISH: u32 = 2;
const STEP_REPUBLISH: u32 = 3;
const STEP_REQUEST_BLOCK_ACK: u32 = 6;
/// The step's answer once the aggregate left the sequence.
const STEP_NONE: u32 = 9;
/// The publication-limit byte of a rate schedule record.
const RECORD_PUBLICATION_LIMIT: usize = 8;
/// Vendor callees a timeout phase answers: the continuations it chooses
/// between, and callees outside the compared frame state.
const TIMEOUT_CONTINUATIONS: &[&str] = &[
    "lmacEndFrameExchangeSequence",
    "lmacEndRetryAMPDUFail",
    "lmacDiscardFrameExchangeSequence",
    "lmacRetryTxFrame",
];
const TIMEOUT_QUIET: &[&str] = &[
    "lmacMSDUAged",
    "is_use_muedca",
    "esp_test_tx_count_retry",
    "lmacProcessTBSuccess",
    "lmacProcessTxopQComplete",
    "lmacProcessShortFrameSuccess",
    "lmacProcessLongFrameSuccess",
    "wifi_assert",
    "wifi_log",
];
/// Compared MPDUs of a timeout sequence.
const TIMEOUT_MPDUS: usize = 4;

/// The vendor continuation a phase reached: the modeled callee and its third
/// argument word.
fn reached(
    records: &[blobray_domain::ExecutionEvidence],
    case: u32,
    targets: &[(u32, &'static str)],
) -> Option<(&'static str, Option<u32>)> {
    let mut found = None;
    let mut current: Option<&'static str> = None;
    for event in crate::evidence::events(records, case, false) {
        match event {
            blobray_domain::ExecutionEvent::ModeledCall { target, .. } => {
                current = targets.iter().find(|(a, _)| *a == target).map(|(_, n)| *n);
                if let Some(name) = current {
                    found = Some((name, None));
                }
            }
            blobray_domain::ExecutionEvent::CallArgument { word: 2, value }
                if current.is_some() =>
            {
                found = Some((current.expect("a continuation"), value));
            }
            _ => {}
        }
    }
    found
}

/// One timeout sequence: the vendor `lmacInit` with the aggregate and the
/// production begin, the queue-context patch, and `phases` compared
/// acknowledgement timeouts.
fn timeout_rows(
    ctx: &mut Mac,
    frames: &[u32],
    lifetime: u32,
    timeout: Timeout,
    descriptor_flags: u32,
    phases: usize,
) -> Result<Vec<ExecutionCase>> {
    let image = ctx.image_symbols()?;
    let symbol = |name: &str| ctx.symbol_address(&image, name);
    let boundary = |name: &str| crate::mac::call_boundary(&image, name);
    let record = ctx
        .context::<crate::mac::RateTables>()?
        .record(crate::mac::RateArena::Ht, HT_MCS0)?;
    let aggregate = Aggregate {
        count: TIMEOUT_MPDUS,
        first_sequence: 100,
        descriptor_flags,
        vendor_attempts: 0,
        record: Some(ARENA_RECORD),
    };
    let mut init = crate::retry::lmac_init(ctx, &image, vec![])?;
    for (address, bytes) in aggregate
        .regions()
        .into_iter()
        .chain([(ARENA_RECORD, record)])
    {
        init.memory.push(crate::harness::region(
            address,
            bytes.len() as u32,
            &bytes,
            None,
            RegionLifetime::Session,
        )?);
    }
    let joined: Vec<u8> = (0..TIMEOUT_MPDUS).flat_map(|i| frame(100, i)).collect();
    let mut begin = ctx.session.probes.invoke(
        TIMEOUT_BEGIN_PROBE,
        vec![
            ("frames", Arg::Word(Some(i64::from(PRODUCTION_FRAMES)))),
            ("count", Arg::Word(Some(TIMEOUT_MPDUS as i64))),
            ("first_sequence", Arg::Word(Some(100))),
            ("lifetime_micros", Arg::Word(Some(i64::from(lifetime)))),
        ],
        vec![],
        vec![],
    )?;
    begin
        .memory
        .push(known(PRODUCTION_FRAMES, joined.len() as u32, &joined)?);
    begin.arguments.resize(8, Some(0));
    let label = format!("ampdu-{}-{descriptor_flags:x}", timeout.label);
    let mut rows = vec![crate::harness::setup(
        format!("{label}-init"),
        init,
        begin,
        SessionReset::Cold,
    )];
    // The vendor's transmit path installs the aggregate and marks the queue
    // transmitting; production has no counterpart.
    let memcpy = u32::try_from(
        crate::harness::symbol(
            &ctx.session.inventory,
            crate::layout::ROM_INPUT as usize,
            "memcpy",
        )?
        .value,
    )?;
    let context = LMAC_QUEUES + QUEUE * LMAC_QUEUE_BYTES;
    let patches: [(u32, Vec<u8>); 2] = [
        (
            context + LMAC_QUEUE_BUFFER,
            ARENA_ESF.to_le_bytes().to_vec(),
        ),
        (context + LMAC_QUEUE_STATE, vec![LMAC_TRANSMITTING]),
    ];
    for (index, (address, bytes)) in patches.iter().enumerate() {
        let length = bytes.len() as u32;
        rows.push(crate::harness::setup(
            format!("{label}-patch-{index}"),
            direct(
                memcpy,
                &[*address, PATCH, length],
                vec![known(PATCH, length, bytes)?],
                vec![],
                vec![],
            ),
            direct(memcpy, &[PATCH, PATCH, 0], vec![], vec![], vec![]),
            SessionReset::Warm,
        ));
    }
    let header = |name: String, address: u32| MemorySelection {
        name,
        address,
        length: HEADER_BYTES,
    };
    for phase in 0..phases {
        let observed_vendor: Vec<MemorySelection> = (0..TIMEOUT_MPDUS)
            .map(|index| header(format!("mpdu-{index}-header"), vendor_frame(index)))
            .collect();
        let mut vendor = direct(
            symbol(timeout.leaf)?,
            &[QUEUE, 0],
            vec![],
            vec![],
            observed_vendor,
        );
        for name in TIMEOUT_QUIET.iter().chain(TIMEOUT_CONTINUATIONS) {
            vendor
                .calls
                .push(answered(name, symbol(name)?, boundary(name), 0));
        }
        let observed_production: Vec<MemorySelection> = (0..TIMEOUT_MPDUS)
            .map(|index| header(format!("mpdu-{index}-header"), frames[index]))
            .collect();
        let mut production = ctx.session.probes.invoke(
            TIMEOUT_STEP_PROBE,
            vec![("status", Arg::Word(Some(i64::from(timeout.status))))],
            vec![],
            observed_production,
        )?;
        production.arguments.resize(8, Some(0));
        let mut row = case(
            format!("{label}-{phase}"),
            vendor,
            Some(production),
            SessionReset::Warm,
            false,
        );
        let relation = row.relation.as_mut().expect("a compared case");
        relation.events = EventChannels {
            timeline: crate::harness::TIMELINE,
            mmio_read: false,
            mmio_write: false,
            fence: false,
            delay: false,
        };
        relation.memory = (0..TIMEOUT_MPDUS as u16)
            .map(|index| MemoryPair {
                vendor: index,
                replacement: index,
            })
            .collect();
        rows.push(row);
    }
    for row in &mut rows {
        row.stack_fill = Some(LEAF_FILLS[0]);
    }
    Ok(rows)
}

/// Compare acknowledgement timeouts without a BlockAck: in every phase both
/// sides mark the same MPDUs for retry, and the vendor retries exactly while
/// production retains the aggregate.
pub fn exercise_timeouts(ctx: &mut Mac) -> Result<()> {
    let frames = layout(ctx)?;
    let image = ctx.image_symbols()?;
    let targets: Vec<(u32, &'static str)> = TIMEOUT_CONTINUATIONS
        .iter()
        .map(|name| Ok((ctx.symbol_address(&image, name)?, *name)))
        .collect::<Result<_>>()?;
    let record_limit = *ctx
        .context::<crate::mac::RateTables>()?
        .record(crate::mac::RateArena::Ht, HT_MCS0)?
        .get(RECORD_PUBLICATION_LIMIT)
        .ok_or_else(|| invalid("the HT record has no publication limit"))?;
    let conf = vendor_conf(ctx)?;
    let (short_limit, lifetime) = (conf.short_limit, conf.aggregate_lifetime);
    for timeout in TIMEOUTS {
        // The vendor ends the aggregate at the first limit its counters
        // reach; production at its own limit.
        let vendor_attempts = usize::from(if timeout.record_bounded {
            record_limit.min(short_limit)
        } else {
            short_limit
        });
        let protections: &[u32] = if timeout.unprotected {
            &[0, DESCRIPTOR_RTS]
        } else {
            &[DESCRIPTOR_RTS]
        };
        for &flags in protections {
            let final_decision = if flags == DESCRIPTOR_RTS {
                timeout.protected_decision
            } else {
                STEP_FINISH
            };
            let phases = vendor_attempts;
            let rows = timeout_rows(ctx, &frames, lifetime, timeout, flags, phases)?;
            let first = rows.len() - phases;
            let label = format!("ampdu-{}-{flags:x}", timeout.label);
            let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
            let records = ctx
                .submit(
                    &label,
                    &crate::session::request(&vendor, Some(&production), None, rows, RESORT_EVENTS),
                    Some(ComparisonVerdict::Match),
                )?
                .records
                .clone();
            let mut production_attempts = None;
            for phase in 0..phases {
                let case = (first + phase) as u32;
                let decision = crate::i2c::returned_low(&records, case, true);
                let continuation = reached(&records, case, &targets);
                let vendor_continues = phase + 1 < vendor_attempts;
                let retried = continuation == Some(("lmacEndFrameExchangeSequence", Some(1)));
                let production_continues = decision == Some(timeout.continuing);
                if decision == Some(final_decision) {
                    production_attempts = Some(phase + 1);
                }
                let production_valid = match production_attempts {
                    None => production_continues,
                    Some(attempts) if attempts == phase + 1 => true,
                    Some(_) => decision == Some(STEP_NONE),
                };
                if retried != vendor_continues || continuation.is_none() || !production_valid {
                    return Err(invalid(format!(
                        "{label} phase {phase}: production decided {decision:?}, the vendor reached {continuation:?}"
                    )));
                }
            }
            let expected_end = if flags == DESCRIPTOR_RTS {
                timeout.protected_end
            } else {
                END_EXCHANGE
            };
            let end = reached(&records, (first + phases - 1) as u32, &targets);
            if end != Some(expected_end) {
                return Err(invalid(format!(
                    "{label}: the vendor ended at {end:?}, expected {expected_end:?}"
                )));
            }
            if production_attempts != Some(vendor_attempts) {
                return Err(invalid(format!(
                    "{label}: production ended after {production_attempts:?} completions, the vendor after {vendor_attempts}"
                )));
            }
        }
    }
    Ok(())
}
