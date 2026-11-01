//! The L1 cache's access counters and the cache geometry the ROM and boot
//! configured. No product owner uses them; a HIL load image reads them at
//! its measuring windows' ends.
//!
//! What each counter counts, as the cache's register header states it
//! (SOURCE: ESP_IDF_4D59230D_SOC_ESP32S31_REGISTER_SOC_CACHE_REG_H,
//! `L1_IBUSn_ACS_*_CNT` and `L1_DBUSn_ACS_*_CNT`):
//!
//! - *hit* and *miss*: the hits and misses of bus *n*'s accesses to its
//!   cache (instruction bus *n* to L1-ICache*n*, data bus *n* to the one
//!   L1-DCache);
//! - *conflict*: the access conflicts of those accesses;
//! - *next-level read*: the times the L1 cache read the L2 cache because of
//!   those accesses; a data bus also counts its *next-level writes*, the
//!   write-backs its accesses caused.
//!
//! Each count is 32 bits wide. Overflow of any count of a bus sets that
//! bus's raw overflow bit (`L1_CACHE_ACS_CNT_INT_RAW`); whether a count
//! wraps or stops there is not stated. The overflow interrupt is not
//! enabled: only its raw bit is read.
//!
//! Which hart a bus serves: core 0 fetches through instruction bus 0 and
//! core 1 through instruction bus 1; ESP-IDF requests data bus 0 for every
//! core's data and never data bus 1 (SOURCE:
//! ESP_IDF_FFF9895C_S31_CACHE_LL_BUSES). Which hart's data accesses each
//! data bus carries is not established. Only accesses that reach the L1
//! cache count.
//!
//! The counters count only while the L1 cache's trace is on
//! (`TRACE_ENA.L1_CACHE_TRACE_ENA`, "L1-Cache trace for the performance
//! counter and fail tracer"; reset 0). Adopting the owner writes nothing;
//! [`CacheCounters::start`] turns the trace on, clears the selected buses'
//! counters and then enables them.

use oer_esp32s31_pac::{
    cache::{self, CacheAccessCounters, CacheGeometry},
    instances,
    ownership::CacheCounterPeripherals,
};
use oer_mmio::{Fixed, Placement};

#[cfg(test)]
mod tests;

/// Where the cache block lies.
pub type CachePlacement = Fixed<{ instances::cache::BASE }>;

/// The buses whose counters [`CacheCounters::start`] runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Buses {
    /// Instruction bus 0: core 0's fetches through L1-ICache0.
    pub ibus0: bool,
    /// Instruction bus 1: core 1's fetches through L1-ICache1.
    pub ibus1: bool,
    /// Data bus 0 of L1-DCache, the one ESP-IDF requests for every core.
    pub dbus0: bool,
    /// Data bus 1 of L1-DCache.
    pub dbus1: bool,
}

impl Buses {
    /// Every bus the cache counts.
    pub const ALL: Self = Self {
        ibus0: true,
        ibus1: true,
        dbus0: true,
        dbus1: true,
    };
}

/// An instruction bus's counts since its [`CacheCounters::start`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct InstructionBus {
    pub hits: u32,
    pub misses: u32,
    pub conflicts: u32,
    /// Reads of the L2 cache its accesses caused.
    pub next_level_reads: u32,
    /// A count of the bus overflowed: the counts are not exact.
    pub overflowed: bool,
}

/// A data bus's counts since its [`CacheCounters::start`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DataBus {
    pub hits: u32,
    pub misses: u32,
    pub conflicts: u32,
    /// Reads of the L2 cache its accesses caused.
    pub next_level_reads: u32,
    /// Write-backs to the L2 cache its accesses caused.
    pub next_level_writes: u32,
    /// A count of the bus overflowed: the counts are not exact.
    pub overflowed: bool,
}

/// Every bus's counts, read one word after another.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub ibus0: InstructionBus,
    pub ibus1: InstructionBus,
    pub dbus0: DataBus,
    pub dbus1: DataBus,
}

/// One cache's configured size and line (block) size in bytes; `None`
/// when its one-hot word does not select exactly one size.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Level {
    pub size_bytes: Option<u32>,
    pub line_bytes: Option<u32>,
}

/// The configured caches. L1 associativity is fixed: no register selects
/// it (SOURCE: S31_CACHE_WAY_GEOMETRY in the register model).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Geometry {
    /// The L1 instruction caches' size word.
    pub l1_icache: Level,
    pub l1_icache_ways: u32,
    pub l1_dcache: Level,
    pub l1_dcache_ways: u32,
    pub l2: Level,
    /// The L2 cache is bypassed (`BYPASS_L2_CACHE_EN`).
    pub l2_bypassed: bool,
}

/// The access counters of instruction buses 0 and 1 and data buses 0 and
/// 1 with their enable, clear and overflow words and the trace enable, and
/// an observer of the cache geometry.
#[must_use = "dropping the counters permanently loses their register authority"]
pub struct CacheCounters<P: Placement = CachePlacement> {
    counters: CacheAccessCounters<P>,
    geometry: CacheGeometry<P>,
}

impl CacheCounters {
    /// Own the root's counter partition without a write.
    pub(crate) fn new(parts: CacheCounterPeripherals) -> Self {
        Self {
            counters: parts.cache_access_counters,
            geometry: parts.cache_geometry,
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> CacheCounters<oer_mmio::Host<'a>> {
    /// The counter and geometry words over two host memories, each from
    /// the instance's base.
    pub fn host(
        counters: &'a mut [u32],
        geometry: &'a mut [u32],
    ) -> Result<Self, oer_mmio::HostTooSmall> {
        Ok(Self {
            counters: CacheAccessCounters::host(counters)?,
            geometry: CacheGeometry::host(geometry)?,
        })
    }
}

impl<P: Placement> CacheCounters<P> {
    /// Count from zero on the buses `buses` selects and on no other. Five
    /// steps, in this order, each one typed write:
    ///
    /// 1. set `TRACE_ENA.L1_CACHE_TRACE_ENA`, which the header names the
    ///    L1 cache's trace "for the performance counter and fail tracer"
    ///    (the other lane as read);
    /// 2. stop every bus's counter: every `L1_*_CNT_ENA` zero, no clear;
    /// 3. clear each selected bus's counts (`L1_*_CNT_CLR`), each strobe
    ///    in a write of its own with every enable still zero;
    /// 4. clear the selected buses' overflow bits (`L1_*_OVF_INT_CLR`,
    ///    which the header says also clears the counts);
    /// 5. enable the selected buses in one write that sets no clear lane.
    ///
    /// Every selected bus thus starts counting at the same write, and no
    /// clear lane is set in the word the counters then run under: `CNT_CLR`
    /// is `WT`, and the PAC writes a trigger lane only in its own strobe.
    ///
    /// SOURCE: ESP_IDF_4D59230D_SOC_ESP32S31_REGISTER_SOC_CACHE_REG_H
    /// (`TRACE_ENA`, `L1_CACHE_ACS_CNT_CTRL`, `L1_CACHE_ACS_CNT_INT_CLR`).
    /// The earlier open-esp-radio-rs implementation
    /// (`crates/adapters/esp-hal/esp32s31/soc/src/cache/performance.rs`,
    /// da26f64c3, `enable`) took the same order: trace, clear, then enable.
    pub fn start(&mut self, buses: Buses) {
        self.counters.modify_trace_ena(|w| {
            w.set_l1_cache_trace_ena(true);
        });
        self.counters.modify_l1_cache_acs_cnt_ctrl(|w| {
            w.set_l1_ibus0_cnt_ena(false)
                .set_l1_ibus1_cnt_ena(false)
                .set_l1_dbus0_cnt_ena(false)
                .set_l1_dbus1_cnt_ena(false);
        });
        if buses.ibus0 {
            self.counters
                .trigger_l1_cache_acs_cnt_ctrl_l1_ibus0_cnt_clr();
        }
        if buses.ibus1 {
            self.counters
                .trigger_l1_cache_acs_cnt_ctrl_l1_ibus1_cnt_clr();
        }
        if buses.dbus0 {
            self.counters
                .trigger_l1_cache_acs_cnt_ctrl_l1_dbus0_cnt_clr();
        }
        if buses.dbus1 {
            self.counters
                .trigger_l1_cache_acs_cnt_ctrl_l1_dbus1_cnt_clr();
        }
        self.counters.trigger_l1_cache_acs_cnt_int_clr(|t| {
            if buses.ibus0 {
                t.l1_ibus0_ovf_int_clr();
            }
            if buses.ibus1 {
                t.l1_ibus1_ovf_int_clr();
            }
            if buses.dbus0 {
                t.l1_dbus0_ovf_int_clr();
            }
            if buses.dbus1 {
                t.l1_dbus1_ovf_int_clr();
            }
        });
        self.counters.modify_l1_cache_acs_cnt_ctrl(|w| {
            w.set_l1_ibus0_cnt_ena(buses.ibus0)
                .set_l1_ibus1_cnt_ena(buses.ibus1)
                .set_l1_dbus0_cnt_ena(buses.dbus0)
                .set_l1_dbus1_cnt_ena(buses.dbus1);
        });
    }

    /// Whether the L1 cache's trace for its performance counters is on
    /// (`TRACE_ENA.L1_CACHE_TRACE_ENA`).
    pub fn tracing(&self) -> bool {
        self.counters.read_trace_ena().l1_cache_trace_ena()
    }

    /// Whose counters run: each bus's enable bit.
    pub fn enabled(&self) -> Buses {
        let control = self.counters.read_l1_cache_acs_cnt_ctrl();
        Buses {
            ibus0: control.l1_ibus0_cnt_ena(),
            ibus1: control.l1_ibus1_cnt_ena(),
            dbus0: control.l1_dbus0_cnt_ena(),
            dbus1: control.l1_dbus1_cnt_ena(),
        }
    }

    /// Every bus's counts, then the overflow bits: an overflow while the
    /// counts are read marks its bus.
    pub fn snapshot(&self) -> Snapshot {
        let c = &self.counters;
        let mut snapshot = Snapshot {
            ibus0: InstructionBus {
                hits: c.read_l1_ibus0_acs_hit_cnt().l1_ibus0_hit_cnt(),
                misses: c.read_l1_ibus0_acs_miss_cnt().l1_ibus0_miss_cnt(),
                conflicts: c.read_l1_ibus0_acs_conflict_cnt().l1_ibus0_conflict_cnt(),
                next_level_reads: c.read_l1_ibus0_acs_nxtlvl_rd_cnt().l1_ibus0_nxtlvl_rd_cnt(),
                overflowed: false,
            },
            ibus1: InstructionBus {
                hits: c.read_l1_ibus1_acs_hit_cnt().l1_ibus1_hit_cnt(),
                misses: c.read_l1_ibus1_acs_miss_cnt().l1_ibus1_miss_cnt(),
                conflicts: c.read_l1_ibus1_acs_conflict_cnt().l1_ibus1_conflict_cnt(),
                next_level_reads: c.read_l1_ibus1_acs_nxtlvl_rd_cnt().l1_ibus1_nxtlvl_rd_cnt(),
                overflowed: false,
            },
            dbus0: DataBus {
                hits: c.read_l1_dbus0_acs_hit_cnt().l1_dbus0_hit_cnt(),
                misses: c.read_l1_dbus0_acs_miss_cnt().l1_dbus0_miss_cnt(),
                conflicts: c.read_l1_dbus0_acs_conflict_cnt().l1_dbus0_conflict_cnt(),
                next_level_reads: c.read_l1_dbus0_acs_nxtlvl_rd_cnt().l1_dbus0_nxtlvl_rd_cnt(),
                next_level_writes: c.read_l1_dbus0_acs_nxtlvl_wr_cnt().l1_dbus0_nxtlvl_wr_cnt(),
                overflowed: false,
            },
            dbus1: DataBus {
                hits: c.read_l1_dbus1_acs_hit_cnt().l1_dbus1_hit_cnt(),
                misses: c.read_l1_dbus1_acs_miss_cnt().l1_dbus1_miss_cnt(),
                conflicts: c.read_l1_dbus1_acs_conflict_cnt().l1_dbus1_conflict_cnt(),
                next_level_reads: c.read_l1_dbus1_acs_nxtlvl_rd_cnt().l1_dbus1_nxtlvl_rd_cnt(),
                next_level_writes: c.read_l1_dbus1_acs_nxtlvl_wr_cnt().l1_dbus1_nxtlvl_wr_cnt(),
                overflowed: false,
            },
        };
        let overflow = c.read_l1_cache_acs_cnt_int_raw();
        snapshot.ibus0.overflowed = overflow.l1_ibus0_ovf_int_raw();
        snapshot.ibus1.overflowed = overflow.l1_ibus1_ovf_int_raw();
        snapshot.dbus0.overflowed = overflow.l1_dbus0_ovf_int_raw();
        snapshot.dbus1.overflowed = overflow.l1_dbus1_ovf_int_raw();
        snapshot
    }

    /// The configured cache sizes, line sizes and the L2 bypass, and the
    /// fixed L1 associativity.
    pub fn geometry(&self) -> Geometry {
        let g = &self.geometry;
        let icache = g.read_l1_icache_cachesize_conf();
        let iblock = g.read_l1_icache_blocksize_conf();
        let dcache = g.read_l1_dcache_cachesize_conf();
        let dblock = g.read_l1_dcache_blocksize_conf();
        let l2 = g.read_l2_cache_cachesize_conf();
        let l2_block = g.read_l2_cache_blocksize_conf();
        Geometry {
            l1_icache: Level {
                size_bytes: size([
                    icache.l1_icache_cachesize_256(),
                    icache.l1_icache_cachesize_512(),
                    icache.l1_icache_cachesize_1k(),
                    icache.l1_icache_cachesize_2k(),
                    icache.l1_icache_cachesize_4k(),
                    icache.l1_icache_cachesize_8k(),
                    icache.l1_icache_cachesize_16k(),
                    icache.l1_icache_cachesize_32k(),
                    icache.l1_icache_cachesize_64k(),
                    icache.l1_icache_cachesize_128k(),
                    icache.l1_icache_cachesize_256k(),
                    icache.l1_icache_cachesize_512k(),
                    icache.l1_icache_cachesize_1024k(),
                ]),
                line_bytes: line([
                    iblock.l1_icache_blocksize_8(),
                    iblock.l1_icache_blocksize_16(),
                    iblock.l1_icache_blocksize_32(),
                    iblock.l1_icache_blocksize_64(),
                    iblock.l1_icache_blocksize_128(),
                    iblock.l1_icache_blocksize_256(),
                ]),
            },
            l1_icache_ways: cache::L1_ICACHE_WAYS,
            l1_dcache: Level {
                size_bytes: size([
                    dcache.l1_dcache_cachesize_256(),
                    dcache.l1_dcache_cachesize_512(),
                    dcache.l1_dcache_cachesize_1k(),
                    dcache.l1_dcache_cachesize_2k(),
                    dcache.l1_dcache_cachesize_4k(),
                    dcache.l1_dcache_cachesize_8k(),
                    dcache.l1_dcache_cachesize_16k(),
                    dcache.l1_dcache_cachesize_32k(),
                    dcache.l1_dcache_cachesize_64k(),
                    dcache.l1_dcache_cachesize_128k(),
                    dcache.l1_dcache_cachesize_256k(),
                    dcache.l1_dcache_cachesize_512k(),
                    dcache.l1_dcache_cachesize_1024k(),
                ]),
                line_bytes: line([
                    dblock.l1_dcache_blocksize_8(),
                    dblock.l1_dcache_blocksize_16(),
                    dblock.l1_dcache_blocksize_32(),
                    dblock.l1_dcache_blocksize_64(),
                    dblock.l1_dcache_blocksize_128(),
                    dblock.l1_dcache_blocksize_256(),
                ]),
            },
            l1_dcache_ways: cache::L1_DCACHE_WAYS,
            l2: Level {
                size_bytes: size([
                    l2.l2_cache_cachesize_256(),
                    l2.l2_cache_cachesize_512(),
                    l2.l2_cache_cachesize_1k(),
                    l2.l2_cache_cachesize_2k(),
                    l2.l2_cache_cachesize_4k(),
                    l2.l2_cache_cachesize_8k(),
                    l2.l2_cache_cachesize_16k(),
                    l2.l2_cache_cachesize_32k(),
                    l2.l2_cache_cachesize_64k(),
                    l2.l2_cache_cachesize_128k(),
                    l2.l2_cache_cachesize_256k(),
                    l2.l2_cache_cachesize_512k(),
                    l2.l2_cache_cachesize_1024k(),
                ]),
                line_bytes: line([
                    l2_block.l2_cache_blocksize_8(),
                    l2_block.l2_cache_blocksize_16(),
                    l2_block.l2_cache_blocksize_32(),
                    l2_block.l2_cache_blocksize_64(),
                    l2_block.l2_cache_blocksize_128(),
                    l2_block.l2_cache_blocksize_256(),
                ]),
            },
            l2_bypassed: g.read_l2_bypass_cache_conf().bypass_l2_cache_en(),
        }
    }
}

/// The bytes each size lane names, from `_256` to `_1024K`, in field order.
/// SOURCE: ESP_IDF_4D59230D_SOC_ESP32S31_REGISTER_SOC_CACHE_REG_H
/// (`*_CACHESIZE_CONF`: one-hot fields named by the size they configure).
const SIZE_BYTES: [u32; 13] = [
    256,
    512,
    1 << 10,
    2 << 10,
    4 << 10,
    8 << 10,
    16 << 10,
    32 << 10,
    64 << 10,
    128 << 10,
    256 << 10,
    512 << 10,
    1024 << 10,
];

/// The bytes each block-size lane names, from `_8` to `_256`, in field
/// order. SOURCE: ESP_IDF_4D59230D_SOC_ESP32S31_REGISTER_SOC_CACHE_REG_H
/// (`*_BLOCKSIZE_CONF`: one-hot fields named by the block size).
const LINE_BYTES: [u32; 6] = [8, 16, 32, 64, 128, 256];

fn size(lanes: [bool; 13]) -> Option<u32> {
    one_hot(lanes.into_iter().zip(SIZE_BYTES))
}

fn line(lanes: [bool; 6]) -> Option<u32> {
    one_hot(lanes.into_iter().zip(LINE_BYTES))
}

/// The value of the one set lane; `None` for none or several.
fn one_hot(lanes: impl Iterator<Item = (bool, u32)>) -> Option<u32> {
    let mut set = lanes.filter_map(|(on, value)| on.then_some(value));
    match (set.next(), set.next()) {
        (Some(value), None) => Some(value),
        _ => None,
    }
}
