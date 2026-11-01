//! The link's counts: the data MPDUs addressed to the station by their
//! PPDU's rate and A-MPDU containment, the station's single data
//! transmissions by their last rate and end, and its transmit agreements'
//! settlements. The station service, on the boot hart, is their one
//! writer; a reader on any hart takes each word on its own, so counts read
//! together may be a write apart. Counts wrap at 2^32.

use core::sync::atomic::{AtomicU32, Ordering};

use super::bump;

/// A rate as the counts sort it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Rate {
    Ht {
        mcs: u8,
        width40: bool,
        short_gi: bool,
    },
    NonHt,
    He,
    Unknown,
}

/// Counts by rate: HT MCS 0 to 7 by width and guard interval, the other
/// HT MCSs, non-HT, HE and no rate.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rates {
    pub ht20_long_gi: [u32; 8],
    pub ht20_short_gi: [u32; 8],
    pub ht40_long_gi: [u32; 8],
    pub ht40_short_gi: [u32; 8],
    pub ht_other: u32,
    pub non_ht: u32,
    pub he: u32,
    pub unknown: u32,
}

/// The received data MPDUs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RxLink {
    pub mpdus: Rates,
    pub in_ampdu: u32,
    pub single: u32,
    pub containment_unknown: u32,
}

/// How a single data transmission ended.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TxEnd {
    Delivered,
    AckTimeout,
    Other,
    Unsent,
}

/// Whether a received MPDU came in an A-MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Containment {
    Unknown,
    Single,
    InAmpdu,
}

/// The single data transmissions, by their last rate and end, and every
/// transmission they took.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxLink {
    pub mpdus: Rates,
    pub transmissions: u32,
    pub delivered: u32,
    pub ack_timeouts: u32,
    pub other_ends: u32,
    pub unsent: u32,
}

/// Both directions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Link {
    pub rx: RxLink,
    pub tx: TxLink,
}

/// The transmit agreements' settlements: aggregates, those refused, their
/// subframes, the aggregates by size (1, 2, 3 to 4, 5 to 8, 9 to
/// 16, 17 and more subframes), the subframes acknowledged, retried and
/// dropped, and the BlockAckReqs sent.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxAmpdu {
    pub aggregates: u32,
    pub refused: u32,
    pub subframes: u32,
    pub by_size: [u32; SIZE_BINS],
    pub acknowledged: u32,
    pub retried: u32,
    pub dropped: u32,
    pub block_ack_requests: u32,
}

/// One aggregate's settlement.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Settled {
    pub members: u8,
    /// The agreement refused the aggregate.
    pub refused: bool,
    pub acknowledged: u8,
    pub retried: u8,
    pub dropped: u8,
}

/// The aggregates' size bins.
pub const SIZE_BINS: usize = 6;

const RATE_BUCKETS: usize = 4 * 8 + 4;
const HT_OTHER: usize = 32;
const NON_HT: usize = 33;
const HE: usize = 34;
const UNKNOWN_RATE: usize = 35;

/// The bucket of `rate`.
const fn bucket(rate: Rate) -> usize {
    match rate {
        Rate::Ht {
            mcs,
            width40,
            short_gi,
        } => {
            if mcs > 7 {
                return HT_OTHER;
            }
            (width40 as usize * 2 + short_gi as usize) * 8 + mcs as usize
        }
        Rate::NonHt => NON_HT,
        Rate::He => HE,
        Rate::Unknown => UNKNOWN_RATE,
    }
}

/// The size bin of an aggregate of `members`.
const fn size_bin(members: u8) -> usize {
    match members {
        0 | 1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        _ => 5,
    }
}

fn read<const N: usize>(counts: &[AtomicU32; N]) -> [u32; N] {
    counts.each_ref().map(|count| count.load(Ordering::Relaxed))
}

fn rates(buckets: &[u32; RATE_BUCKETS]) -> Rates {
    let at = |bucket: usize| buckets.get(bucket).copied().unwrap_or(0);
    let mcs = |width40, short_gi| {
        core::array::from_fn(|mcs| {
            at(self::bucket(Rate::Ht {
                mcs: u8::try_from(mcs).unwrap_or(u8::MAX),
                width40,
                short_gi,
            }))
        })
    };
    Rates {
        ht20_long_gi: mcs(false, false),
        ht20_short_gi: mcs(false, true),
        ht40_long_gi: mcs(true, false),
        ht40_short_gi: mcs(true, true),
        ht_other: at(HT_OTHER),
        non_ht: at(NON_HT),
        he: at(HE),
        unknown: at(UNKNOWN_RATE),
    }
}

/// The link's counts.
pub struct LinkCounter {
    rx_rates: [AtomicU32; RATE_BUCKETS],
    /// Unknown, single, in an A-MPDU.
    rx_containment: [AtomicU32; 3],
    tx_rates: [AtomicU32; RATE_BUCKETS],
    /// Delivered, ACK timeout, other, unsent.
    tx_ends: [AtomicU32; 4],
    tx_transmissions: AtomicU32,
    /// Aggregates, refused, subframes, the size bins, acknowledged,
    /// retried, dropped and BlockAckReqs.
    ampdu: [AtomicU32; AMPDU_COUNTS],
}

const AMPDU_COUNTS: usize = 7 + SIZE_BINS;
const AMPDU_BINS: usize = 3;
const AMPDU_ACKNOWLEDGED: usize = AMPDU_BINS + SIZE_BINS;

impl LinkCounter {
    pub const fn new() -> Self {
        Self {
            rx_rates: [const { AtomicU32::new(0) }; RATE_BUCKETS],
            rx_containment: [const { AtomicU32::new(0) }; 3],
            tx_rates: [const { AtomicU32::new(0) }; RATE_BUCKETS],
            tx_ends: [const { AtomicU32::new(0) }; 4],
            tx_transmissions: AtomicU32::new(0),
            ampdu: [const { AtomicU32::new(0) }; AMPDU_COUNTS],
        }
    }

    /// Count a received data MPDU.
    pub fn received(&self, rate: Rate, containment: Containment) {
        bump_at(&self.rx_rates, bucket(rate), 1);
        let containment = match containment {
            Containment::Unknown => 0,
            Containment::Single => 1,
            Containment::InAmpdu => 2,
        };
        bump_at(&self.rx_containment, containment, 1);
    }

    /// Count a single data transmission's end after `transmissions`.
    pub fn sent(&self, rate: Rate, end: TxEnd, transmissions: u16) {
        bump_at(&self.tx_rates, bucket(rate), 1);
        let end = match end {
            TxEnd::Delivered => 0,
            TxEnd::AckTimeout => 1,
            TxEnd::Other => 2,
            TxEnd::Unsent => 3,
        };
        bump_at(&self.tx_ends, end, 1);
        bump(&self.tx_transmissions, u32::from(transmissions));
    }

    /// Count an aggregate's settlement.
    pub fn settled(&self, settled: Settled) {
        bump_at(&self.ampdu, 0, 1);
        bump_at(&self.ampdu, 1, u32::from(settled.refused));
        bump_at(&self.ampdu, 2, u32::from(settled.members));
        bump_at(
            &self.ampdu,
            AMPDU_BINS.saturating_add(size_bin(settled.members)),
            1,
        );
        bump_at(
            &self.ampdu,
            AMPDU_ACKNOWLEDGED,
            u32::from(settled.acknowledged),
        );
        bump_at(
            &self.ampdu,
            AMPDU_ACKNOWLEDGED.saturating_add(1),
            u32::from(settled.retried),
        );
        bump_at(
            &self.ampdu,
            AMPDU_ACKNOWLEDGED.saturating_add(2),
            u32::from(settled.dropped),
        );
    }

    /// Count a BlockAckReq sent.
    pub fn block_ack_request(&self) {
        bump_at(&self.ampdu, AMPDU_ACKNOWLEDGED.saturating_add(3), 1);
    }

    /// The counts now.
    pub fn link(&self) -> Link {
        let [containment_unknown, single, in_ampdu] = read(&self.rx_containment);
        let [delivered, ack_timeouts, other_ends, unsent] = read(&self.tx_ends);
        Link {
            rx: RxLink {
                mpdus: rates(&read(&self.rx_rates)),
                in_ampdu,
                single,
                containment_unknown,
            },
            tx: TxLink {
                mpdus: rates(&read(&self.tx_rates)),
                transmissions: self.tx_transmissions.load(Ordering::Relaxed),
                delivered,
                ack_timeouts,
                other_ends,
                unsent,
            },
        }
    }

    /// The settlements now.
    pub fn ampdu(&self) -> TxAmpdu {
        let [
            aggregates,
            refused,
            subframes,
            b0,
            b1,
            b2,
            b3,
            b4,
            b5,
            acknowledged,
            retried,
            dropped,
            block_ack_requests,
        ] = read(&self.ampdu);
        TxAmpdu {
            aggregates,
            refused,
            subframes,
            by_size: [b0, b1, b2, b3, b4, b5],
            acknowledged,
            retried,
            dropped,
            block_ack_requests,
        }
    }
}

impl Default for LinkCounter {
    fn default() -> Self {
        Self::new()
    }
}

fn bump_at<const N: usize>(counts: &[AtomicU32; N], index: usize, by: u32) {
    if let Some(count) = counts.get(index) {
        bump(count, by);
    }
}

#[cfg(test)]
mod tests;
