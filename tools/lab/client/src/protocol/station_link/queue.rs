//! The AP's transmit queues as mac80211 reports them in debugfs, as the
//! router answers them: the station's per-TID queues
//! (`stations/<mac>/aqm`) and the radio's own queue (`ieee80211/<phy>/aqm`),
//! each kept verbatim beside its typed values, and what changed between two
//! reads of a link ([`window`]). The router (`oer-tools-lab-router`,
//! `station::link::queue`) reads and parses the files.
//!
//! The router finds every field by the name the file's own header line
//! gives it, never by its position. A column or name it does not know stays
//! in the verbatim lines and is listed as unknown; a known one the file
//! lacks is listed as missing. Neither becomes a value: it is unavailable,
//! never zero.
//!
//! SOURCE: the stand's router runs OpenWrt 25.12.5 r33051-f5dae5ece4, kernel
//! 6.12.94, with `kmod-mac80211-6.12.94.6.18.26-r1`: mac80211 from
//! backports v6.18.26, whose mac80211 is Linux v6.18.26's (`net/mac80211/
//! debugfs_sta.c::sta_aqm_read`, `net/mac80211/debugfs.c::aqm_read`,
//! `include/net/fq_impl.h`, `include/net/codel_impl.h`). OpenWrt's mac80211
//! patches touch neither file's format nor the queue's counting. The meaning
//! of each value below is that source's; [`Counts`] names it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::StationLink;

/// What a value is counted in.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Unit {
    /// One `sk_buff`: one frame as mac80211 queues it (an A-MSDU built in
    /// the queue is one).
    Packet,
    /// Bytes of `skb->len`, or of `skb->truesize` for memory.
    Byte,
    /// One action that may cover several packets: a drop operation, an
    /// enqueue.
    Operation,
    /// One flow of the queue's flow table.
    Flow,
}

/// How a value moves.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Kind {
    /// A total since the queue was created (a station's at its
    /// association, the radio's at its registration). The kernel keeps it
    /// in 32 bits: it wraps at 2^32.
    Counter,
    /// What the queue holds at the read.
    Level,
    /// A configured value.
    Setting,
}

/// Which queue a value counts.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Scope {
    /// The link's station, every TID of it.
    Station,
    /// The link's station's queue of one TID.
    StationTid { tid: u8 },
    /// The whole radio (mac80211's `local->fq`): every station and every
    /// interface of the phy. Never one station's.
    Radio,
}

/// What a value counts, by the source the module comment names.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Counts {
    /// `iw station get`'s `tx packets`. Where this driver counts it
    /// (mac80211 or mt76, before or after the queue) is not established.
    StationTxPackets,
    /// `backlog-bytes`: `tin.backlog_bytes`, the queued packets' lengths.
    QueuedBytes,
    /// `backlog-packets`: `tin.backlog_packets`, the packets queued.
    QueuedPackets,
    /// `new-flows`: `tin.flows`, packets classified to a flow that no queue
    /// held (`fq_flow_classify`): flow activations, not flows alive.
    FlowActivations,
    /// `drops`: `cstats.drop_count`, packets CoDel dropped at dequeue, one
    /// per packet (`codel_dequeue`).
    CodelDrops,
    /// `marks`: `cstats.ecn_mark`, packets CoDel marked ECN CE instead of
    /// dropping; they stay queued and are dequeued.
    EcnMarks,
    /// `overlimit`: `tin.overlimit`, drop OPERATIONS, not packets. When the
    /// radio's queue is over its packet or memory limit at an enqueue (any
    /// station's), `fq_tin_enqueue` drops from the radio's fattest flow,
    /// up to `max(1, min(32, its length / 2))` packets in one
    /// `fq_flow_drop`, then adds one here, to the TID that owned that flow.
    /// The packets so dropped are counted nowhere; never add this to a
    /// packet count.
    OverlimitDropOperations,
    /// `collisions`: `tin.collisions`, enqueues whose flow hash was taken by
    /// another TID's queue and went to this TID's default flow.
    CollidedEnqueues,
    /// `tx-bytes`: `tin.tx_bytes`, bytes dequeued from the queue.
    DequeuedBytes,
    /// `tx-packets`: `tin.tx_packets`, packets dequeued from the queue
    /// (`fq_tin_dequeue`), not transmissions: a dequeued packet the driver
    /// later drops or fails to send is counted, one CoDel dropped is not.
    DequeuedPackets,
    /// `fq_flows_cnt`: the radio's flow table size.
    RadioFlowSlots,
    /// `fq_backlog`: `fq->backlog`, the packets the radio's queue holds.
    RadioQueuedPackets,
    /// Printed as `fq_overlimit`, it is `fq->overmemory`: `aqm_read` passes
    /// the two counters in the other order than it names them. Drop
    /// operations made while the queue's memory was over its limit.
    RadioOvermemoryDropOperations,
    /// Printed as `fq_overmemory`, it is `fq->overlimit`: every drop
    /// operation of the radio's queue, over memory or over the packet
    /// limit, so never below [`Self::RadioOvermemoryDropOperations`].
    RadioDropOperations,
    /// `fq_collisions`: `fq->collisions`, the radio's collided enqueues.
    RadioCollidedEnqueues,
    /// `fq_memory_usage`: `fq->memory_usage`, the queued packets' truesize.
    RadioMemoryUsage,
    /// `fq_memory_limit`: the truesize above which the queue drops.
    RadioMemoryLimit,
    /// `fq_limit`: the packets above which the queue drops.
    RadioPacketLimit,
    /// `fq_quantum`: the bytes a flow may dequeue per round.
    RadioQuantum,
}

impl Counts {
    pub fn unit(self) -> Unit {
        match self {
            Self::StationTxPackets
            | Self::QueuedPackets
            | Self::CodelDrops
            | Self::EcnMarks
            | Self::DequeuedPackets
            | Self::RadioQueuedPackets
            | Self::RadioPacketLimit => Unit::Packet,
            Self::QueuedBytes
            | Self::DequeuedBytes
            | Self::RadioMemoryUsage
            | Self::RadioMemoryLimit
            | Self::RadioQuantum => Unit::Byte,
            Self::OverlimitDropOperations
            | Self::CollidedEnqueues
            | Self::RadioOvermemoryDropOperations
            | Self::RadioDropOperations
            | Self::RadioCollidedEnqueues => Unit::Operation,
            Self::FlowActivations | Self::RadioFlowSlots => Unit::Flow,
        }
    }

    pub fn kind(self) -> Kind {
        match self {
            Self::QueuedBytes
            | Self::QueuedPackets
            | Self::RadioQueuedPackets
            | Self::RadioMemoryUsage => Kind::Level,
            Self::RadioFlowSlots
            | Self::RadioMemoryLimit
            | Self::RadioPacketLimit
            | Self::RadioQuantum => Kind::Setting,
            Self::StationTxPackets
            | Self::FlowActivations
            | Self::CodelDrops
            | Self::EcnMarks
            | Self::OverlimitDropOperations
            | Self::CollidedEnqueues
            | Self::DequeuedBytes
            | Self::DequeuedPackets
            | Self::RadioOvermemoryDropOperations
            | Self::RadioDropOperations
            | Self::RadioCollidedEnqueues => Kind::Counter,
        }
    }
}

/// One typed value: what it counts, in which unit, of which queue and when
/// it was read.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observed {
    pub value: u64,
    pub unit: Unit,
    pub kind: Kind,
    pub counts: Counts,
    pub scope: Scope,
    /// The router's boot clock just before the read, in milliseconds
    /// (the first field of its `/proc/uptime`, in 10 ms steps).
    pub observed_at_ms: u64,
}

impl Observed {
    pub fn new(value: u64, counts: Counts, scope: Scope, observed_at_ms: u64) -> Self {
        Self {
            value,
            unit: counts.unit(),
            kind: counts.kind(),
            counts,
            scope,
            observed_at_ms,
        }
    }
}

/// A file as the router printed it: its read's time, its header line and
/// its other lines, verbatim.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Raw {
    pub observed_at_ms: u64,
    pub header: String,
    pub lines: Vec<String>,
}

/// The station's `aqm` file: one row per TID queue the station has.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationQueues {
    pub raw: Raw,
    /// The header's columns this parser does not know; their values stay
    /// in `raw` and in each row's `unknown`.
    pub unknown_columns: Vec<String>,
    /// The columns this parser knows that the header lacks: unavailable in
    /// every row.
    pub missing_columns: Vec<String>,
    pub tids: Vec<TidQueue>,
}

/// One TID's queue of the station.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TidQueue {
    pub tid: u8,
    /// Its access category index (mac80211's: 0 VO, 1 VI, 2 BE, 3 BK).
    pub ac: Option<u8>,
    pub backlog_bytes: Option<Observed>,
    pub backlog_packets: Option<Observed>,
    pub new_flows: Option<Observed>,
    pub drops: Option<Observed>,
    pub marks: Option<Observed>,
    pub overlimit: Option<Observed>,
    pub collisions: Option<Observed>,
    pub tx_bytes: Option<Observed>,
    pub tx_packets: Option<Observed>,
    /// The queue's flags as printed: `0x6(RUN AMPDU NO-AMSDU)`.
    pub flags: Option<String>,
    /// The row's text under each unknown column.
    pub unknown: BTreeMap<String, String>,
}

/// The radio's `aqm` file: one row per value, by name.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RadioQueues {
    pub raw: Raw,
    /// The header's columns other than `access`, `name` and `value`.
    pub unknown_columns: Vec<String>,
    /// The names this parser does not know: no typed value.
    pub unknown_names: Vec<String>,
    /// The names this parser knows that the file lacks.
    pub missing_names: Vec<String>,
    pub values: BTreeMap<String, RadioEntry>,
}

/// One row of the radio's `aqm` file.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RadioEntry {
    /// `R` or `RW`, as printed; absent without an `access` column.
    pub access: Option<String>,
    /// Absent for an unknown name or without a `value` column.
    pub value: Option<Observed>,
}

/// Whether two reads saw one association of the station.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Association {
    /// Both reads name the same `associated at [boottime]`.
    Same,
    /// The station associated again between the reads: its queues and
    /// counters are new.
    Changed,
    /// A read names no association time: continuity is not confirmed.
    Unconfirmed,
}

/// One value's change between two reads.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Change {
    Valid(Delta),
    /// A read lacks the value.
    Unavailable,
    /// Both reads have it, but their difference is not the window's.
    Invalid(Invalid),
}

/// A change between two reads of one value.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Delta {
    /// The end's value less the start's: never negative for a counter;
    /// a level's or a setting's may be.
    pub change: i64,
    pub unit: Unit,
    pub kind: Kind,
    pub counts: Counts,
    pub scope: Scope,
    pub from_ms: u64,
    pub to_ms: u64,
}

/// Why a change between two reads is not the window's.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Invalid {
    /// The router's boot clock went back between the reads: it restarted,
    /// and every count began again, however far it has grown since.
    ClockWentBack,
    /// The station associated again between the reads.
    Reassociated,
    /// A read names no association time.
    AssociationUnconfirmed,
    /// A counter went back: a reset (the radio restarted, the station's
    /// queue was recreated) or a 32-bit wrap.
    WentBack,
    /// The two reads count different things.
    Mismatched,
    /// The difference does not fit an `i64`.
    OutOfRange,
}

/// One TID queue's changes.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TidChange {
    pub backlog_bytes: Change,
    pub backlog_packets: Change,
    pub new_flows: Change,
    pub drops: Change,
    pub marks: Change,
    pub overlimit: Change,
    pub collisions: Change,
    pub tx_bytes: Change,
    pub tx_packets: Change,
}

/// The station's transmit packets in the window, accounted by its queues.
///
/// `entry` (the station's `tx packets`, every TID) less, over every TID
/// queue, the packets dequeued (`tx_packets`), dropped by CoDel (`drops`)
/// and the backlog's change (`backlog_packets`) leaves `remainder`. All are
/// packets; TID 0 counts whole, probes and load alike. What the remainder
/// holds is not established: the packets of overlimit drop operations
/// (each TID's `overlimit` counts operations, never added here), queue
/// purges and filters (counted nowhere), packets counted at the entry that
/// never reached a queue, and what moved between the entry's read and the
/// queues' (`skew_ms`). Where the driver counts `entry` is itself not
/// established. The radio's values are never part of it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Balance {
    pub entry: Change,
    pub remainder: Remainder,
    /// The larger of the two reads' gaps between the station's read and
    /// its queues' read; absent without both queue reads.
    pub skew_ms: Option<u64>,
}

/// What a balance leaves.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Remainder {
    /// The entry's packets the queues' packet counters do not account.
    Packets(i64),
    /// No packet remainder: a term is not the window's, and why.
    Open(String),
}

/// What changed in the AP's queues between two reads of one station's
/// link.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QueueWindow {
    pub association: Association,
    /// Every TID either read names; a TID only one names is unavailable.
    pub tids: BTreeMap<u8, TidChange>,
    /// The radio's values by name: the whole radio's, not the station's.
    pub radio: BTreeMap<String, Change>,
    pub balance: Balance,
}

/// What two reads of a link share: whether the station kept its
/// association and whether the router's clock went back between them.
#[derive(Clone, Copy)]
struct Reads {
    association: Association,
    /// The end's station read is strictly earlier on the router's boot
    /// clock than the start's. Equal times stand: the clock steps 10 ms.
    clock_went_back: bool,
}

/// The change of one value from `start` to `end`: none across a router
/// restart (its own reads' clock, or the link's, went back), a station's
/// value only within one association.
fn change(start: Option<&Observed>, end: Option<&Observed>, reads: Reads) -> Change {
    let (Some(start), Some(end)) = (start, end) else {
        return Change::Unavailable;
    };
    if start.counts != end.counts || start.scope != end.scope {
        return Change::Invalid(Invalid::Mismatched);
    }
    if reads.clock_went_back || end.observed_at_ms < start.observed_at_ms {
        return Change::Invalid(Invalid::ClockWentBack);
    }
    if start.scope != Scope::Radio {
        match reads.association {
            Association::Same => {}
            Association::Changed => return Change::Invalid(Invalid::Reassociated),
            Association::Unconfirmed => return Change::Invalid(Invalid::AssociationUnconfirmed),
        }
    }
    if end.kind == Kind::Counter && end.value < start.value {
        return Change::Invalid(Invalid::WentBack);
    }
    match i64::try_from(i128::from(end.value) - i128::from(start.value)) {
        Ok(change) => Change::Valid(Delta {
            change,
            unit: end.unit,
            kind: end.kind,
            counts: end.counts,
            scope: end.scope,
            from_ms: start.observed_at_ms,
            to_ms: end.observed_at_ms,
        }),
        Err(_) => Change::Invalid(Invalid::OutOfRange),
    }
}

fn tid_change(start: Option<&TidQueue>, end: Option<&TidQueue>, reads: Reads) -> TidChange {
    let c = |field: fn(&TidQueue) -> Option<&Observed>| {
        change(start.and_then(field), end.and_then(field), reads)
    };
    TidChange {
        backlog_bytes: c(|queue| queue.backlog_bytes.as_ref()),
        backlog_packets: c(|queue| queue.backlog_packets.as_ref()),
        new_flows: c(|queue| queue.new_flows.as_ref()),
        drops: c(|queue| queue.drops.as_ref()),
        marks: c(|queue| queue.marks.as_ref()),
        overlimit: c(|queue| queue.overlimit.as_ref()),
        collisions: c(|queue| queue.collisions.as_ref()),
        tx_bytes: c(|queue| queue.tx_bytes.as_ref()),
        tx_packets: c(|queue| queue.tx_packets.as_ref()),
    }
}

fn remainder(entry: Change, tids: &BTreeMap<u8, TidChange>) -> Remainder {
    let Change::Valid(entry) = entry else {
        return Remainder::Open(format!("the station's tx packets: {entry:?}"));
    };
    if tids.is_empty() {
        return Remainder::Open("no station queue at both reads".to_owned());
    }
    let mut accounted = 0_i64;
    for (tid, queue) in tids {
        for (name, term) in [
            ("tx-packets", queue.tx_packets),
            ("drops", queue.drops),
            ("backlog-packets", queue.backlog_packets),
        ] {
            let Change::Valid(term) = term else {
                return Remainder::Open(format!("TID {tid} `{name}`: {term:?}"));
            };
            let Some(sum) = accounted.checked_add(term.change) else {
                return Remainder::Open("the queues' packets overflow".to_owned());
            };
            accounted = sum;
        }
    }
    match entry.change.checked_sub(accounted) {
        Some(packets) => Remainder::Packets(packets),
        None => Remainder::Open("the remainder overflows".to_owned()),
    }
}

fn tid_queue(link: &StationLink, tid: u8) -> Option<&TidQueue> {
    link.station_queues
        .as_ref()
        .and_then(|queues| queues.tids.iter().find(|queue| queue.tid == tid))
}

fn radio_value<'a>(link: &'a StationLink, name: &str) -> Option<&'a Observed> {
    link.radio_queues
        .as_ref()
        .and_then(|queues| queues.values.get(name))
        .and_then(|entry| entry.value.as_ref())
}

/// What changed in the AP's queues from `start` to `end`, two reads of one
/// station's link.
pub fn window(start: &StationLink, end: &StationLink) -> QueueWindow {
    let association = match (
        start.associated_at_boottime_ms,
        end.associated_at_boottime_ms,
    ) {
        (Some(start), Some(end)) if start == end => Association::Same,
        (Some(_), Some(_)) => Association::Changed,
        _ => Association::Unconfirmed,
    };
    let reads = Reads {
        association,
        clock_went_back: end.observed_at_ms < start.observed_at_ms,
    };
    let mut tid_numbers = [start, end]
        .iter()
        .filter_map(|link| link.station_queues.as_ref())
        .flat_map(|queues| queues.tids.iter().map(|queue| queue.tid))
        .collect::<Vec<_>>();
    tid_numbers.sort_unstable();
    tid_numbers.dedup();
    let tids = tid_numbers
        .into_iter()
        .map(|tid| {
            (
                tid,
                tid_change(tid_queue(start, tid), tid_queue(end, tid), reads),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let radio = [start, end]
        .iter()
        .filter_map(|link| link.radio_queues.as_ref())
        .flat_map(|queues| queues.values.keys())
        .map(|name| {
            (
                name.clone(),
                change(radio_value(start, name), radio_value(end, name), reads),
            )
        })
        .collect();
    let entry = |link: &StationLink| {
        Observed::new(
            link.tx_packets,
            Counts::StationTxPackets,
            Scope::Station,
            link.observed_at_ms,
        )
    };
    let entry = change(Some(&entry(start)), Some(&entry(end)), reads);
    let skew = |link: &StationLink| {
        link.station_queues
            .as_ref()
            .map(|queues| queues.raw.observed_at_ms.abs_diff(link.observed_at_ms))
    };
    QueueWindow {
        association,
        balance: Balance {
            entry,
            remainder: remainder(entry, &tids),
            skew_ms: skew(start)
                .zip(skew(end))
                .map(|(start, end)| start.max(end)),
        },
        tids,
        radio,
    }
}

#[cfg(test)]
mod tests;
