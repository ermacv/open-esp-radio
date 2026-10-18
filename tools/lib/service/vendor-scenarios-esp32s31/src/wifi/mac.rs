//! Wi-Fi MAC HAL leaves of the pinned `libpp.a` against compiled production.
//!
//! Each leaf is one vendor function and the production probe that runs its
//! HAL counterpart. Every case runs both over the whole radio register block
//! retained from one fill pattern, so every register bit the leaf reads takes
//! both values across the fills, and compares every register effect exactly
//! and, where the leaf returns one, the return word.
use crate::harness::{Result, case, direct, invalid};
use crate::layout::INPUT;
use crate::session::{Session, request};
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallOutput, CallOutputScope, CallRepetition,
    CallResponse, ExecutionTarget, RegionLifetime, SessionReset,
};
pub use oer_vendor_scenario_engine::leaf::{
    Dispatch, DispatchTarget, Domain, LEAF_EVENTS, LEAF_FILLS, Leaf, LeafCase,
    LeafOptions as MacOptions, LeafRun as Mac, OUTPUT_FILL, Objects, Replacement, Suite, Vendor,
    VendorAbi, call_boundary, claims, compared_bytes, dispatching, exercise, image_symbols,
    in_archive, leaf, objects, ordered, output, prefix, quiet, released, released_when_leased,
    replaced, rom, ruled, stated, tail_prefix, vendor_reads,
};
use std::any::Any;
use std::collections::BTreeMap;

/// Names no pinned input defines: newlib `putchar`, which only the
/// `libpp.a` diagnostic dumps reachable from the transmit leaves call, and,
/// as in the Bluetooth suite, the peripheral bases the linker scripts provide,
/// the firmware's writable-IRAM error, log and I2C functions only modem clock
/// driver paths the compared cases never take reference, and the
/// writable-IRAM FreeRTOS port the low-power clock leaf answers as quiet
/// calls. They resolve to an unmapped address, so reaching one stops the case.
const ABSENT: &[&str] = &[
    "putchar",
    "MODEM_SYSCON",
    "MODEM_LPCON",
    "LP_CLKRST",
    "PMU",
    "HP_SYS_CLKRST",
    "HP_ALIVE_SYS",
    "TIMERG0",
    "esp_rom_delay_us",
    "esp_rom_printf",
    "_esp_error_check_failed",
    "_regi2c_impl_write",
    "esp_log",
    "esp_log_timestamp",
    "xPortInIsrContext",
    "xPortEnterCriticalTimeout",
    "vPortExitCriticalMultiCore",
];
/// Input index of the pinned `libnet80211.a`.
const NET80211_INPUT: u64 = 4;
/// Logical transmit queues the production HAL admits.
const QUEUES: &[u32] = &[0, 1, 2, 3];
/// Event masks: none, the lowest bit, an alternating pattern and all bits.
const EVENT_MASKS: &[u32] = &[0, 1, 0x5a5a_a5a5, u32::MAX];

/// Artifact ids of the ROM and of the vendor firmware every ESP32-S31 leaf
/// suite links: the firmware supplies the network-stack data and logging
/// symbols that code after a compared prefix references. Every suite keeps
/// the `mac` contract identifier its reviewed contracts carry.
pub const ROM: &str = "rom";
pub const FIRMWARE: &str = "phy-sdk";
pub const CONTRACT_ID: &str = "mac";

/// Link roots of the Wi-Fi rate tables and retry sequences, beyond the
/// leaves.
const WIFI_ROOTS: &[&[&str]] = &[
    &[RateTables::INDEX_CALLER],
    crate::retry::ROOTS,
    crate::rx_append::ROOTS,
    crate::ampdu_resort::ROOTS,
];

/// Evidence claims of the retry sequences and the RX append.
const WIFI_CLAIMS: &[(&str, &str, &str)] = &concat_claims::<
    {
        crate::retry::CLAIMS.len()
            + crate::rx_append::CLAIMS.len()
            + crate::ampdu_resort::CLAIMS.len()
    },
>(
    &concat_claims::<{ crate::retry::CLAIMS.len() + crate::rx_append::CLAIMS.len() }>(
        crate::retry::CLAIMS,
        crate::rx_append::CLAIMS,
    ),
    crate::ampdu_resort::CLAIMS,
);

/// `first` followed by `second`, in a constant.
const fn concat_claims<const N: usize>(
    first: &[(&'static str, &'static str, &'static str)],
    second: &[(&'static str, &'static str, &'static str)],
) -> [(&'static str, &'static str, &'static str); N] {
    let mut claims = [("", "", ""); N];
    let mut index = 0;
    while index < N {
        claims[index] = if index < first.len() {
            first[index]
        } else {
            second[index - first.len()]
        };
        index += 1;
    }
    claims
}

/// The Wi-Fi MAC suite over `libpp.a` and `libnet80211.a`, with the rate
/// tables and retry sequences.
pub const WIFI_MAC: Suite = Suite {
    title: "Wi-Fi MAC HAL leaf comparison",
    id: CONTRACT_ID,
    archives: &["libpp", "libnet80211", "libesp-hw-support", "libhal"],
    rom: ROM,
    firmware: Some(FIRMWARE),
    leaves: LEAVES,
    absent: ABSENT,
    roots: WIFI_ROOTS,
    prepare: Some(wifi_prepare),
    claims: WIFI_CLAIMS,
};

/// The vendor rate tables the Wi-Fi builders and retry sequences read,
/// checked against the rate domain production admits.
fn wifi_prepare(run: &mut Mac) -> Result<Box<dyn Any>> {
    let vendor = run.vendor.clone();
    let image = image_symbols(&run.session.run.join("image/image.elf"))?;
    let dot11n_index = schedule_indices(
        &mut run.session,
        &vendor,
        &image,
        RateTables::DOT11N_INDEX,
        HT_RATE_CODES,
    )?;
    let dot11ax_index = schedule_indices(
        &mut run.session,
        &vendor,
        &image,
        RateTables::DOT11AX_INDEX,
        HE_RATE_CODES,
    )?;
    let rates = RateTables {
        index: vendor_section(&run.session, RateTables::OBJECT, RateTables::INDEX_SECTION)?,
        arena: vendor_section(&run.session, RateTables::OBJECT, RateTables::ARENA_SECTION)?,
        dot11n: vendor_section(&run.session, RateTables::OBJECT, RateTables::DOT11N_SECTION)?,
        dot11n_index,
        dot11ax: vendor_section(
            &run.session,
            RateTables::OBJECT,
            RateTables::DOT11AX_SECTION,
        )?,
        dot11ax_index,
    };
    let admitted: Vec<u32> = rates
        .mapped()
        .into_iter()
        .filter(|code| !UNADMITTED_RATE_CODES.contains(code))
        .collect();
    let arenas = [
        (RateArena::Legacy, RATE_CODES),
        (RateArena::Ht, HT_RATE_CODES),
        (RateArena::He, HE_RATE_CODES),
    ];
    for (arena, codes) in arenas {
        for code in codes {
            if !rates.covers_publication_limit(arena, *code)? {
                return Err(invalid(format!(
                    "vendor record of rate {code:#x} ends before its publication limit"
                )));
            }
        }
    }
    if admitted != RATE_CODES {
        return Err(invalid(format!(
            "rcGetRate rate domain differs from the vendor index table: {:x?}",
            rates.mapped()
        )));
    }
    Ok(Box::new(rates))
}

/// `libpp.a[trc.o]` 802.11g retry data: the rate-code-to-record index table
/// `rc11GRate2SchedIdx` reads, and the schedule arena `rc11GSchedTbl`.
#[derive(Default)]
pub struct RateTables {
    pub index: Vec<u8>,
    pub arena: Vec<u8>,
    /// The 802.11n schedule arena and the record index the vendor's
    /// `rc11NRate2SchedIdx` returns for each HT rate code.
    pub dot11n: Vec<u8>,
    pub dot11n_index: BTreeMap<u32, u8>,
    /// The 802.11ax schedule arena and the record index the vendor's
    /// `rc11AXRate2SchedIdx` returns for each HE rate code.
    pub dot11ax: Vec<u8>,
    pub dot11ax_index: BTreeMap<u32, u8>,
}

/// Rate-control byte domains: the same byte means an HT rate in the
/// 802.11n arena and an HE rate in the 802.11ax arena.
#[derive(Clone, Copy)]
pub enum RateArena {
    Legacy,
    Ht,
    He,
}

impl RateTables {
    /// Object and data sections of the tables.
    const OBJECT: &'static str = "trc.o";
    const INDEX_SECTION: &'static str = ".rodata.CSWTCH.73";
    const ARENA_SECTION: &'static str = ".data.rc11GSchedTbl";
    const DOT11N_SECTION: &'static str = ".data.rc11NSchedTbl";
    const DOT11AX_SECTION: &'static str = ".data.rc11AXSchedTbl";
    /// Local vendor functions returning the 802.11n and 802.11ax record
    /// index of a rate code, and their caller, a global symbol that links
    /// them.
    const DOT11N_INDEX: &'static str = "rc11NRate2SchedIdx";
    const DOT11AX_INDEX: &'static str = "rc11AXRate2SchedIdx";
    const INDEX_CALLER: &'static str = "rcUpdatePhyMode";
    /// Bytes of one schedule record, and its publication-limit byte.
    const RECORD: usize = 12;
    const PUBLICATION_LIMIT: usize = 8;
    /// Index-table entry of a rate with no 802.11g record.
    const UNMAPPED: u8 = 0xff;

    /// The vendor schedule record of `code` in `arena`: the 802.11g record
    /// of a legacy rate, the 802.11n record of an HT rate and the 802.11ax
    /// record of an HE rate.
    pub(crate) fn record(&self, arena: RateArena, code: u32) -> Result<Vec<u8>> {
        let (records, index) = match arena {
            RateArena::Legacy => return self.legacy_record(code),
            RateArena::Ht => (&self.dot11n, self.dot11n_index.get(&code)),
            RateArena::He => (&self.dot11ax, self.dot11ax_index.get(&code)),
        };
        let index = index.ok_or_else(|| invalid(format!("rate {code:#x} has no vendor index")))?;
        let start = usize::from(*index) * Self::RECORD;
        records
            .get(start..start + Self::RECORD)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| invalid(format!("vendor record {index} outside its arena")))
    }

    /// The vendor 802.11g record of legacy rate `code`.
    fn legacy_record(&self, code: u32) -> Result<Vec<u8>> {
        let index = *self
            .index
            .get(code as usize)
            .ok_or_else(|| invalid(format!("rate {code:#x} outside the vendor index table")))?;
        if index == Self::UNMAPPED {
            return Err(invalid(format!(
                "rate {code:#x} has no vendor 802.11g record"
            )));
        }
        let start = usize::from(index) * Self::RECORD;
        self.arena
            .get(start..start + Self::RECORD)
            .map(<[u8]>::to_vec)
            .ok_or_else(|| invalid(format!("vendor record {index} outside the arena")))
    }

    /// The publication limit, byte 0x08, of legacy rate `code`'s record.
    pub(crate) fn publication_limit(&self, code: u32) -> Result<u8> {
        Ok(self.legacy_record(code)?[Self::PUBLICATION_LIMIT])
    }

    /// Whether the attempt counts of `code`'s record reach its publication
    /// limit at byte 0x08, so the retry-limit owner ends the MPDU before the
    /// record is exhausted.
    fn covers_publication_limit(&self, arena: RateArena, code: u32) -> Result<bool> {
        let record = self.record(arena, code)?;
        let attempts: u32 = (0..4).map(|pair| u32::from(record[2 * pair + 1])).sum();
        Ok(attempts >= u32::from(record[Self::PUBLICATION_LIMIT]))
    }

    /// The rate codes the vendor maps to an 802.11g record.
    fn mapped(&self) -> Vec<u32> {
        (0..self.index.len() as u32)
            .filter(|code| self.index[*code as usize] != Self::UNMAPPED)
            .collect()
    }
}

/// A leaf whose vendor function is the `libnet80211.a` root of that name.
const fn net80211(leaf: Leaf) -> Leaf {
    in_archive(leaf, NET80211_INPUT)
}

/// `hal_mac_tx_config_edca` object at `INPUT`: a context pointer, then the
/// queue byte, the AIFSN byte and the contention-window halfword. The
/// context holds at 0x34 a pointer to the interface record, whose word at
/// 0x10 carries the interface in bits 18 and 19. The probe's first word is
/// the object address itself.
fn edca_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    const CONTEXT: u32 = INPUT + 0x100;
    const INTERFACE_RECORD: u32 = INPUT + 0x200;
    let [_, queue, aifsn, window, interface] = words else {
        unreachable!("EDCA words: object, queue, AIFSN, window, interface")
    };
    let mut object = CONTEXT.to_le_bytes().to_vec();
    object.extend([*queue as u8, *aifsn as u8]);
    object.extend((*window as u16).to_le_bytes());
    let mut context = vec![0u8; 0x38];
    context[0x34..].copy_from_slice(&INTERFACE_RECORD.to_le_bytes());
    let mut record = vec![0u8; 0x14];
    record[0x10..].copy_from_slice(&(interface << 18).to_le_bytes());
    Ok(Objects {
        vendor_words: vec![INPUT],
        vendor: vec![
            (INPUT, object),
            (CONTEXT, context),
            (INTERFACE_RECORD, record),
        ],
        ..Default::default()
    })
}

/// Canonical HT transmit parameters of the reviewed `hal_mac_tx_set_ppdu`
/// fixture, in `CanonicalHtTxParameters` order: queue 0, descriptor head,
/// The rate fields MCS, guard interval and width come from the case state;
/// the rest is an A-MPDU of length 0xc2e over two descriptors, spacing
/// density 5, no timeout, scheduler and packet priority 1, one priority,
/// zero AIFSN and window, the station interface, no hardware key and no
/// TXOP. The probe takes the data and RTS powers from the power table.
const PPDU_CANONICAL: [u32; 18] = [
    0,
    PPDU_DESCRIPTOR,
    7,
    1,
    1,
    1,
    0xc2e,
    2,
    1,
    0,
    1,
    1,
    1,
    0,
    0,
    0,
    0,
    0,
];
/// Canonical word indices of the rate fields.
const PPDU_CANONICAL_MCS: usize = 2;
const PPDU_CANONICAL_GUARD_INTERVAL: usize = 3;
const PPDU_CANONICAL_WIDTH: usize = 4;
/// Every HT rate of the comparison, packed as MCS, short guard interval
/// (bit 8), 40 MHz (bit 16) and a single MPDU (bit 24).
const PPDU_RATES: [u32; 64] = {
    let mut rates = [0; 64];
    let mut i = 0;
    while i < rates.len() {
        let i32 = i as u32;
        rates[i] = (i32 & 7) | (i32 >> 3 & 1) << 8 | (i32 >> 4 & 1) << 16 | (i32 >> 5 & 1) << 24;
        i += 1;
    }
    rates
};
/// Canonical word index of the HT format: zero for a single MPDU, one for
/// an A-MPDU.
const PPDU_CANONICAL_FORMAT: usize = 5;
/// Canonical word index of the descriptor count; a single MPDU has one.
const PPDU_CANONICAL_DESCRIPTORS: usize = 7;
/// Canonical legacy parameters: rate and group receiver come from the case
/// state; the signal is the fixture length, with the HT fixture's
/// priorities and the station interface.
const PPDU_LEGACY_CANONICAL: [u32; 13] = [0, PPDU_DESCRIPTOR, 0, 0xc2e, 0, 1, 1, 1, 0, 0, 0, 0, 0];
const PPDU_LEGACY_CANONICAL_RATE: usize = 2;
const PPDU_LEGACY_CANONICAL_SIGNAL: usize = 3;
const PPDU_LEGACY_CANONICAL_GROUP: usize = 11;
/// Frame buffer the legacy descriptor points to; its first word carries
/// the hardware frame length `mac_tx_set_plcp1` publishes.
const PPDU_LEGACY_BUFFER: u32 = 0x3fff_1700;
/// Descriptor word-0 bits of the HT fixture a legacy frame clears: A-MPDU
/// (bit 22) and the acknowledgement-class bit 19 `mac_tx_set_plcp0` adds.
const PPDU_AGGREGATE_BITS: u32 = 1 << 22 | 1 << 19;
/// Descriptor word-0 bit that makes `mac_tx_set_plcp0` publish no
/// acknowledgement: a group receiver.
const PPDU_GROUP_RECEIVER: u32 = 1 << 1;
/// Transmit-context words through the TXOP bytes at 0x1c and 0x1d, which
/// `lmacInitAc` initializes to count zero and slot three, no TXOP queue;
/// only `lmacRequestTxopQueue` grants a slot, for QoS frames of a queue
/// with a TXOP limit.
const PPDU_PROGRAM_WORDS: usize = 8;
const PPDU_NO_TXOP_SLOT: u32 = 0x0000_0300;
/// Every legacy rate code production transmits, each for a single and a
/// group receiver (bit 8).
const PPDU_LEGACY_STATES: [u32; 30] = {
    const RATES: [u32; 15] = [0, 1, 2, 3, 5, 6, 7, 8, 9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf];
    let mut states = [0; 30];
    let mut i = 0;
    while i < states.len() {
        states[i] = RATES[i % RATES.len()] | ((i / RATES.len()) as u32) << 8;
        i += 1;
    }
    states
};
/// Where production receives its copy of the power table.
const PPDU_POWER_COPY: u32 = INPUT + 0x100;
/// The vendor HT descriptor object, its rate byte (word 3) and its width
/// word (word 2), whose bit 15 selects 40 MHz.
const PPDU_HT_OBJECT: u32 = 0x3fff_1300;
const PPDU_HT_RATE_WORD: usize = 3;
const PPDU_HT_WIDTH_WORD: usize = 2;
const PPDU_HT_40MHZ: u32 = 0x8000;
/// HT object words whose byte 2 (bytes 0x2a and 0x2e) `mac_tx_set_htsig`
/// publishes as the DMA descriptor counts, and their value for a single
/// MPDU, which has one descriptor.
const PPDU_HT_DESCRIPTOR_COUNT_WORDS: [usize; 2] = [10, 11];
const PPDU_SINGLE_DESCRIPTOR_COUNT: u32 = 0x0001_0000;
const PPDU_DESCRIPTOR_COUNT_MASK: u32 = 0x00ff_0000;
/// Offset of the `pTxRx` row word whose bytes 0 and 1 are the entry classes
/// `mac_tx_set_len` and `mac_tx_set_htsig` publish.
const PPDU_AUXILIARY_CLASS_OFFSET: u32 = 0x40;
/// The transmit context (`esf_buf`) of the fixture and its word at 0x24,
/// whose halfword bit 12 marks an A-MPDU for `mac_tx_set_plcp0` and
/// `mac_tx_set_hesig` and whose byte 0x26 is the MPDU descriptor count
/// `mac_tx_set_hesig` publishes.
const PPDU_TX_CONTEXT: u32 = 0x3fff_1100;
const PPDU_TX_CONTEXT_FLAGS_WORD: usize = 9;
const PPDU_HE_AGGREGATE: u32 = 1 << 12;
const PPDU_HE_AGGREGATE_MPDUS: u32 = 0x0002_0000;
const PPDU_HE_SINGLE_MPDU: u32 = 0x0001_0000;
/// Descriptor word-0 bits of an HE SU PPDU: bit 31 selects the HE branch of
/// `hal_mac_tx_set_ppdu` and bit 30 the SU acknowledgement of
/// `mac_tx_set_plcp0`.
const PPDU_HE_SU: u32 = 1 << 31 | 1 << 30;
/// Descriptor word 1, whose low nibble is the TID `mac_tx_set_tb` tests
/// against `wifi_he_get_hetb_tid_bitmap`: 0x01, 0x81, 0xa1 or 0xa3 by
/// configuration, 0xa1 otherwise. TID 6 is in none, so the PPDU gets no HE
/// trigger-based preparation, which production does not implement.
const PPDU_HE_TID_WORD: usize = 1;
const PPDU_HE_NON_TB_TID: u32 = 6;
/// Descriptor word 10, whose low halfword (0x28) is the minimum-MPDU spacing
/// `mac_tx_set_hesig` publishes in all three channel-width lanes.
const PPDU_HE_SPACING_WORD: usize = 10;
const PPDU_HE_CANONICAL_SPACING: usize = 11;
/// HT-object byte 0x2f, whose low two bits `mac_tx_set_hesig` publishes as
/// the HE-SIG-A GI/LTF code.
const PPDU_HE_GI_LTF_WORD: usize = 11;
const PPDU_HE_GI_LTF_SHIFT: u32 = 24;
/// First HE rate code; the vendor descriptor carries `0x1a + MCS` for every
/// GI/LTF.
const PPDU_HE_CODE: u32 = 0x1a;
/// Canonical HE parameters: MCS, GI/LTF and format come from the case state;
/// LDPC (the vendor's `esp_wifi_cert_tx_bcc` default), no DCM, the fixture
/// APEP length, BSS color and spatial reuse zero, and the HT fixture's
/// spacing, priorities and interface.
const PPDU_HE_CANONICAL: [u32; 20] = [
    0,
    PPDU_DESCRIPTOR,
    0,
    0,
    1,
    0,
    0,
    0xc2e,
    1,
    0,
    0,
    40,
    0,
    1,
    1,
    1,
    0,
    0,
    0,
    0,
];
const PPDU_HE_CANONICAL_MCS: usize = 2;
const PPDU_HE_CANONICAL_GI_LTF: usize = 3;
const PPDU_HE_CANONICAL_FORMAT: usize = 6;
const PPDU_HE_CANONICAL_DESCRIPTORS: usize = 8;
/// HE SU case-state bits beyond the MCS (bits 0..8) and GI/LTF code (bits
/// 8..10): A-MPDU, BCC instead of LDPC, dual-carrier modulation and the
/// interface's hardware BSS color.
const PPDU_HE_STATE_AGGREGATE: u32 = 1 << 16;
const PPDU_HE_STATE_BCC: u32 = 1 << 17;
const PPDU_HE_STATE_DCM: u32 = 1 << 18;
const PPDU_HE_STATE_BSS_COLOR: u32 = 1 << 19;
/// With the BSS-color flag, the interface's color register disabled: the
/// vendor then signals color zero.
const PPDU_HE_STATE_BSS_COLOR_DISABLED: u32 = 1 << 20;
/// HE rates the sequence of GI/LTF codes and formats covers.
const PPDU_HE_RATES: usize = 80;
/// The MCS indices production admits with dual-carrier modulation.
const PPDU_HE_DCM_MCS: [u32; 4] = [0, 1, 3, 4];
/// The MCS of the BSS-color states.
const PPDU_HE_BSS_COLOR_MCS: u32 = 7;
/// Every HE SU rate with LDPC and with BCC; every DCM rate; and one MCS
/// with the hardware BSS color, enabled over every GI/LTF code and format
/// and disabled for both formats.
const PPDU_HE_STATES: [u32; 2 * PPDU_HE_RATES + 8 * PPDU_HE_DCM_MCS.len() + 10] = {
    let mut states = [0; 2 * PPDU_HE_RATES + 8 * PPDU_HE_DCM_MCS.len() + 10];
    let mut i = 0;
    while i < 2 * PPDU_HE_RATES {
        let rate = (i % PPDU_HE_RATES) as u32;
        let bcc = if i < PPDU_HE_RATES {
            0
        } else {
            PPDU_HE_STATE_BCC
        };
        states[i] =
            (rate % 10) | ((rate / 10 % 4) << 8) | ((rate / 40) * PPDU_HE_STATE_AGGREGATE) | bcc;
        i += 1;
    }
    let mut j = 0;
    while j < 8 * PPDU_HE_DCM_MCS.len() {
        let variant = (j / PPDU_HE_DCM_MCS.len()) as u32;
        states[i] = PPDU_HE_DCM_MCS[j % PPDU_HE_DCM_MCS.len()]
            | ((variant % 4) << 8)
            | ((variant / 4) * PPDU_HE_STATE_AGGREGATE)
            | PPDU_HE_STATE_DCM;
        i += 1;
        j += 1;
    }
    let mut k = 0;
    while k < 8 {
        let variant = k as u32;
        states[i] = PPDU_HE_BSS_COLOR_MCS
            | ((variant % 4) << 8)
            | ((variant / 4) * PPDU_HE_STATE_AGGREGATE)
            | PPDU_HE_STATE_BSS_COLOR;
        i += 1;
        k += 1;
    }
    states[i] = PPDU_HE_BSS_COLOR_MCS | PPDU_HE_STATE_BSS_COLOR | PPDU_HE_STATE_BSS_COLOR_DISABLED;
    states[i + 1] = states[i] | PPDU_HE_STATE_AGGREGATE;
    states
};
/// HT-object word 12 (0x30), whose bit 15 selects dual-carrier modulation
/// and bit 19 the interface's hardware BSS color in `mac_tx_set_hesig`.
const PPDU_HE_FLAGS_WORD: usize = 12;
const PPDU_HE_FLAG_DCM: u32 = 1 << 15;
const PPDU_HE_FLAG_BSS_COLOR: u32 = 1 << 19;
/// `hal_he_get_bss_color` of the station interface: the register whose bit
/// 27 enables the color in bits 21..27, and the color of the states.
const PPDU_HE_BSS_COLOR_REGISTER: u32 = 0x2010_4020;
const PPDU_HE_BSS_COLOR_ENABLE: u32 = 1 << 27;
const PPDU_HE_BSS_COLOR_SHIFT: u32 = 21;
const PPDU_HE_BSS_COLOR: u32 = 0x2a;
/// `esp_wifi_cert_tx_bcc`: one selects BCC, anything else LDPC.
const PPDU_HE_CERT_BCC: u32 = 1;
/// Canonical word indices of the HE coding and BSS color.
const PPDU_HE_CANONICAL_LDPC: usize = 4;
const PPDU_HE_CANONICAL_DCM: usize = 5;
const PPDU_HE_CANONICAL_BSS_COLOR: usize = 9;
/// First HT rate-control code of each guard interval.
const PPDU_HT_LONG_GI_CODE: u32 = 0x10;
const PPDU_HT_SHORT_GI_CODE: u32 = 0x1a;
/// The PTI of coexistence event one in the pinned `libcoexist.a`
/// `coex_pti_tab`, which `mac_tx_set_pti` takes the minimum with: every
/// data priority production sends lies below it.
const PPDU_COEX_EVENT_ONE_PTI: u32 = 5;
/// Vendor PP object addresses of that fixture: the transmit context, the
/// first descriptor, the `pTxRx` rate table and the OSI function table.
const PPDU_PROGRAM: u32 = 0x3fff_1000;
const PPDU_DESCRIPTOR: u32 = 0x3fff_1200;
const PPDU_AUXILIARY: u32 = 0x3fff_1600;
const PPDU_OSI_TABLE: u32 = 0x3fff_1800;
/// Bytes of the OSI function table through the `coex_pti_clamp` slot.
const PPDU_OSI_BYTES: usize = 0x1ac;
const PPDU_COEX_PTI_CLAMP_SLOT: usize = 0x1a8;
/// Unmapped address the OSI slot names; a call model answers it.
const PPDU_COEX_PTI_CLAMP: u32 = 0x5000_0000;
/// Vendor PP objects of the fixture, as (address, words).
const PPDU_VENDOR: &[(u32, &[u32])] = &[
    (PPDU_PROGRAM, &[0x3fff_1100, 0]),
    (
        0x3fff_1100,
        &[
            0,
            PPDU_DESCRIPTOR,
            0,
            0,
            0,
            // Two halfword lengths summed by the bounded HT A-MPDU branch
            // of mac_tx_set_len; the second remains zero.
            0x0000_0c2e,
            0,
            0,
            0,
            0,
            0,
            0x3fff_1500,
            0,
            0x3fff_1300,
        ],
    ),
    (PPDU_DESCRIPTOR, &[0x3fff_1400, 0x0000_8000, 0, 0]),
    (
        0x3fff_1300,
        &[
            // Word-zero bit 14 enables the guarded channel-width branch;
            // word-two bit 15 selects 40 MHz in it and in mac_tx_set_htsig.
            0x004c_6009,
            0,
            0x0000_8000,
            0x0000_0021,
            0,
            0,
            0,
            0,
            0x0001_0001,
            0,
            0x0002_0000,
            0x0002_0000,
            0,
            0,
            0,
            0,
        ],
    ),
    (0x3fff_1400, &[0]),
    (0x3fff_1500, &[0]),
    (0x3fff_1580, &[0x0028_0000, 0x0004_0000, 0, 0, 0, 0]),
    // The selected pTxRx rate row contributes entry class one to both packed
    // length words at byte offsets 0x40 and 0x41.
    (PPDU_AUXILIARY, &[0; 16]),
    (
        PPDU_AUXILIARY + PPDU_AUXILIARY_CLASS_OFFSET,
        &[0x0000_0101, 0x0000_0c2e, 0],
    ),
];
/// ROM `s_phy_get_max_pwr` rows the fixture seeds: every rate's power pair
/// differs from its neighbours', so a lookup at a wrong rate differs.
const PPDU_MAX_POWER: [u32; 22] = {
    let mut rows = [0; 22];
    let mut pair = 0;
    while pair < 2 * rows.len() {
        let value = ((pair as u32 % 13) + 1) | ((((pair as u32 + 5) % 13) + 1) << 8);
        rows[pair / 2] |= value << (16 * (pair % 2));
        pair += 1;
    }
    rows
};

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// The reviewed ordinary-queue-zero HT40 MCS7 A-MPDU fixture of
/// `hal_mac_tx_set_ppdu`: vendor PP objects, the ROM rate-table pointer,
/// power rows and OSI table, and the canonical production parameters.
fn ppdu_abi(words_in: &[u32], vendor_side: &Vendor<'_>) -> Result<Objects> {
    let [_, _, _, rate] = words_in else {
        unreachable!("PPDU words: program, auxiliary, power table, rate")
    };
    let (mcs, short_gi, wide, single) =
        (rate & 0xff, rate >> 8 & 1, rate >> 16 & 1, rate >> 24 & 1);
    let code = if short_gi == 1 {
        PPDU_HT_SHORT_GI_CODE
    } else {
        PPDU_HT_LONG_GI_CODE
    } + mcs;
    let mut canonical = PPDU_CANONICAL;
    canonical[PPDU_CANONICAL_MCS] = mcs;
    canonical[PPDU_CANONICAL_GUARD_INTERVAL] = short_gi;
    canonical[PPDU_CANONICAL_WIDTH] = wide;
    canonical[PPDU_CANONICAL_FORMAT] = 1 - single;
    if single == 1 {
        canonical[PPDU_CANONICAL_DESCRIPTORS] = 1;
    }
    let frame = if single == 1 {
        Frame::HtSingle
    } else {
        Frame::HtAggregate
    };
    ppdu_objects(vendor_side, code, wide, frame, words(&canonical))
}

/// The legacy fixture: the HT fixture's objects with a legacy rate code.
fn legacy_ppdu_abi(words_in: &[u32], vendor_side: &Vendor<'_>) -> Result<Objects> {
    let [_, _, _, rate] = words_in else {
        unreachable!("PPDU words: program, auxiliary, power table, rate")
    };
    let (code, group) = (rate & 0xff, rate >> 8 & 1);
    let mut canonical = PPDU_LEGACY_CANONICAL;
    canonical[PPDU_LEGACY_CANONICAL_RATE] = code;
    canonical[PPDU_LEGACY_CANONICAL_GROUP] = group;
    ppdu_objects(
        vendor_side,
        code,
        0,
        Frame::Legacy { group: group == 1 },
        words(&canonical),
    )
}

/// The PPDU a fixture describes.
#[derive(Clone, Copy)]
enum Frame {
    HtAggregate,
    /// A single MPDU, whose length the vendor reads from the frame buffer.
    HtSingle,
    Legacy {
        group: bool,
    },
    /// An HE SU PPDU with its GI/LTF code, BCC instead of LDPC, dual-carrier
    /// modulation, and the interface's hardware BSS color, enabled or not.
    He {
        aggregate: bool,
        gi_ltf: u32,
        bcc: bool,
        dcm: bool,
        bss_color: Option<bool>,
    },
}

/// The HE fixture: the HT fixture's objects as an HE SU PPDU.
fn he_ppdu_abi(words_in: &[u32], vendor_side: &Vendor<'_>) -> Result<Objects> {
    let [_, _, _, state] = words_in else {
        unreachable!("PPDU words: program, auxiliary, power table, state")
    };
    let (mcs, gi_ltf) = (state & 0xff, state >> 8 & 0xff);
    let aggregate = u32::from(state & PPDU_HE_STATE_AGGREGATE != 0);
    let bcc = state & PPDU_HE_STATE_BCC != 0;
    let dcm = state & PPDU_HE_STATE_DCM != 0;
    let bss_color = state & PPDU_HE_STATE_BSS_COLOR != 0;
    let color_enabled = state & PPDU_HE_STATE_BSS_COLOR_DISABLED == 0;
    let mut canonical = PPDU_HE_CANONICAL;
    canonical[PPDU_HE_CANONICAL_MCS] = mcs;
    canonical[PPDU_HE_CANONICAL_GI_LTF] = gi_ltf;
    canonical[PPDU_HE_CANONICAL_FORMAT] = aggregate;
    canonical[PPDU_HE_CANONICAL_DESCRIPTORS] = 1 + aggregate;
    canonical[PPDU_HE_CANONICAL_LDPC] = u32::from(!bcc);
    canonical[PPDU_HE_CANONICAL_DCM] = u32::from(dcm);
    if bss_color && color_enabled {
        canonical[PPDU_HE_CANONICAL_BSS_COLOR] = PPDU_HE_BSS_COLOR;
    }
    ppdu_objects(
        vendor_side,
        PPDU_HE_CODE + mcs,
        0,
        Frame::He {
            aggregate: aggregate == 1,
            gi_ltf,
            bcc,
            dcm,
            bss_color: bss_color.then_some(color_enabled),
        },
        words(&canonical),
    )
}

/// Vendor PP objects of the fixture with rate-control code `code` and the
/// 40-MHz bit `wide`, and the production canonical parameters.
fn ppdu_objects(
    vendor_side: &Vendor<'_>,
    code: u32,
    wide: u32,
    frame: Frame,
    canonical: Vec<u8>,
) -> Result<Objects> {
    // The HT single-MPDU and legacy transformations of the A-MPDU fixture.
    let single = matches!(frame, Frame::HtSingle | Frame::Legacy { .. });
    let legacy = matches!(frame, Frame::Legacy { .. });
    // Every non-A-MPDU HT fixture and every HE fixture reads the frame
    // buffer through descriptor word 1.
    let buffer = !matches!(frame, Frame::HtAggregate);
    let rom = |name: &str| vendor_side.symbol(name);
    let mut vendor: Vec<(u32, Vec<u8>)> = PPDU_VENDOR
        .iter()
        .map(|(address, values)| {
            let mut values = values.to_vec();
            if *address == PPDU_HT_OBJECT {
                values[PPDU_HT_RATE_WORD] = code;
                values[PPDU_HT_WIDTH_WORD] &= !PPDU_HT_40MHZ;
                values[PPDU_HT_WIDTH_WORD] |= wide * PPDU_HT_40MHZ;
                if single {
                    values[0] &= !PPDU_AGGREGATE_BITS;
                    for word in PPDU_HT_DESCRIPTOR_COUNT_WORDS {
                        values[word] &= !PPDU_DESCRIPTOR_COUNT_MASK;
                        values[word] |= PPDU_SINGLE_DESCRIPTOR_COUNT;
                    }
                }
                if let Frame::Legacy { group: true } = frame {
                    values[0] |= PPDU_GROUP_RECEIVER;
                }
                if let Frame::He {
                    aggregate,
                    gi_ltf,
                    dcm,
                    bss_color,
                    ..
                } = frame
                {
                    values[PPDU_HE_FLAGS_WORD] &= !(PPDU_HE_FLAG_DCM | PPDU_HE_FLAG_BSS_COLOR);
                    if dcm {
                        values[PPDU_HE_FLAGS_WORD] |= PPDU_HE_FLAG_DCM;
                    }
                    if bss_color.is_some() {
                        values[PPDU_HE_FLAGS_WORD] |= PPDU_HE_FLAG_BSS_COLOR;
                    }
                    values[0] &= !PPDU_AGGREGATE_BITS;
                    values[0] |= PPDU_HE_SU;
                    values[PPDU_HE_TID_WORD] = PPDU_HE_NON_TB_TID;
                    values[PPDU_HE_SPACING_WORD] &= !0xffff;
                    values[PPDU_HE_SPACING_WORD] |= PPDU_HE_CANONICAL[PPDU_HE_CANONICAL_SPACING];
                    values[PPDU_HE_GI_LTF_WORD] &= !(0xff << PPDU_HE_GI_LTF_SHIFT);
                    values[PPDU_HE_GI_LTF_WORD] |= gi_ltf << PPDU_HE_GI_LTF_SHIFT;
                    if !aggregate {
                        for word in PPDU_HT_DESCRIPTOR_COUNT_WORDS {
                            values[word] &= !PPDU_DESCRIPTOR_COUNT_MASK;
                            values[word] |= PPDU_SINGLE_DESCRIPTOR_COUNT;
                        }
                    }
                }
            }
            if let Frame::He { aggregate, .. } = frame
                && *address == PPDU_TX_CONTEXT
            {
                values[PPDU_TX_CONTEXT_FLAGS_WORD] = if aggregate {
                    PPDU_HE_AGGREGATE | PPDU_HE_AGGREGATE_MPDUS
                } else {
                    PPDU_HE_SINGLE_MPDU
                };
            }
            // A single MPDU has no aggregation state: its `pTxRx` row
            // contributes entry class zero to both packed length words.
            if single && *address == PPDU_AUXILIARY + PPDU_AUXILIARY_CLASS_OFFSET {
                values[0] = 0;
            }
            if buffer && *address == PPDU_DESCRIPTOR {
                values[1] = PPDU_LEGACY_BUFFER;
            }
            if legacy && *address == PPDU_PROGRAM {
                values.resize(PPDU_PROGRAM_WORDS, 0);
                values[PPDU_PROGRAM_WORDS - 1] = PPDU_NO_TXOP_SLOT;
            }
            (*address, words(&values))
        })
        .collect();
    let mut table = vec![0u8; PPDU_OSI_BYTES];
    table[PPDU_COEX_PTI_CLAMP_SLOT..].copy_from_slice(&PPDU_COEX_PTI_CLAMP.to_le_bytes());
    // `mac_tx_set_hesig` reads the certification BCC override from ROM
    // `.bss`, zero unless certification code sets it: LDPC.
    let mut registers = vec![];
    if let Frame::He { bcc, bss_color, .. } = frame {
        let cert = if bcc { PPDU_HE_CERT_BCC } else { 0 };
        vendor.push((rom("esp_wifi_cert_tx_bcc")?, cert.to_le_bytes().to_vec()));
        if let Some(enabled) = bss_color {
            let enable = if enabled { PPDU_HE_BSS_COLOR_ENABLE } else { 0 };
            registers.push((
                PPDU_HE_BSS_COLOR_REGISTER,
                enable | PPDU_HE_BSS_COLOR << PPDU_HE_BSS_COLOR_SHIFT,
            ));
        }
    }
    if buffer {
        vendor.push((
            PPDU_LEGACY_BUFFER,
            words(&[PPDU_LEGACY_CANONICAL[PPDU_LEGACY_CANONICAL_SIGNAL]]),
        ));
    }
    vendor.extend([
        (PPDU_OSI_TABLE, table),
        (rom("pTxRx")?, PPDU_AUXILIARY.to_le_bytes().to_vec()),
        (rom("g_osi_funcs_p")?, PPDU_OSI_TABLE.to_le_bytes().to_vec()),
        (rom("s_phy_get_max_pwr")?, words(&PPDU_MAX_POWER)),
    ]);
    let clamp = CallDeclaration {
        id: "coex-pti-clamp".into(),
        applicability: "the OSI coexistence PTI of event one, through its output byte".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: PPDU_COEX_PTI_CLAMP,
            boundary: CallBoundary::Unmapped,
            allow_tail: false,
        },
        argument_words: 2,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            // The clamp stores its one-byte result through its second
            // argument, a byte of the caller's frame.
            outputs: vec![CallOutput {
                pointer_argument: 1,
                byte_offset: 0,
                width: 1,
                value: PPDU_COEX_EVENT_ONE_PTI,
                scope: CallOutputScope::PrivateStack,
            }],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Finite,
    };
    Ok(Objects {
        vendor_words: vec![PPDU_PROGRAM, PPDU_AUXILIARY],
        vendor,
        production: vec![
            (INPUT, canonical),
            (PPDU_POWER_COPY, words(&PPDU_MAX_POWER)),
        ],
        calls: vec![clamp],
        compared: vec![],
        registers,
        ..Default::default()
    })
}

/// `rcGetRate` fixture of the normal 802.11g schedules: the rate context
/// at `RATE_CONTEXT`, whose schedule flags at 0x0c select no fixed rate, and
/// the transmit descriptor at `RATE_DESCRIPTOR`, whose word 1 carries the
/// MPDU, short and long retry counters in bytes 1..3, whose byte 0x0c
/// receives the selected rate and whose word at 0x1c points to the vendor
/// schedule record of the initial rate, taken from the captured arena.
const RATE_CONTEXT: u32 = 0x3fff_1100;
const RATE_DESCRIPTOR: u32 = 0x3fff_1000;
const RATE_SCHEDULE: u32 = 0x3fff_1200;
/// Legacy initial rates: every code the vendor maps to an 802.11g record,
/// checked against the captured index table when the scenario starts.
const RATE_CODES: &[u32] = &[0, 1, 2, 5, 6, 8, 9, 0xa, 0xb, 0xc, 0xd, 0xe, 0xf];
/// HT rate codes production retries through an 802.11n record: long-GI
/// MCS0 to MCS7 and short-GI MCS7.
const HT_RATE_CODES: &[u32] = &[0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x21];
/// HE rate codes production retries through an 802.11ax record: 1600-ns
/// MCS0 to MCS9 and 800-ns MCS9.
const HE_RATE_CODES: &[u32] = &[
    0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x23,
];
/// Descriptor word-0 bits of an HE single-MPDU, which `rcGetSMPDURate`
/// requires: bits 31 and 30 set, bit 14 clear.
const HE_SMPDU_FORMAT: u32 = 0xc000_0000;
/// Every initial rate of the `rcGetRate` comparison.
const RETRY_RATE_CODES: [u32; RATE_CODES.len() + HT_RATE_CODES.len()] = {
    let mut codes = [0; RATE_CODES.len() + HT_RATE_CODES.len()];
    let mut i = 0;
    while i < RATE_CODES.len() {
        codes[i] = RATE_CODES[i];
        i += 1;
    }
    while i < codes.len() {
        codes[i] = HT_RATE_CODES[i - RATE_CODES.len()];
        i += 1;
    }
    codes
};
/// Long-range codes the vendor also maps to 802.11g records. Production
/// publishes no long-range frame, so its ordinary retry selector admits no
/// long-range initial rate.
const UNADMITTED_RATE_CODES: &[u32] = &[0x29, 0x2a];
/// Retry-counter words: initial publication, second publication, first-rate
/// fallback, a short counter dominating the MPDU counter and a count past
/// the third pair of every record.
const RATE_COUNTERS: &[u32] = &[0, 0x0001_0100, 0x0002_0200, 0x0004_0200, 0x0000_0700];
/// Descriptor byte that receives the selected rate.
const RATE_SELECTED: u32 = RATE_DESCRIPTOR + 0x0c;

fn rate_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let arena = if vendor
        .context::<RateTables>()?
        .dot11n_index
        .contains_key(&words[2])
    {
        RateArena::Ht
    } else {
        RateArena::Legacy
    };
    scheduled_rate_abi(words, vendor, arena, 0)
}

/// `rcGetRate` fixture of an HE single-MPDU over the 802.11ax schedules.
fn he_rate_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    scheduled_rate_abi(words, vendor, RateArena::He, HE_SMPDU_FORMAT)
}

fn scheduled_rate_abi(
    words: &[u32],
    vendor: &Vendor<'_>,
    arena: RateArena,
    format: u32,
) -> Result<Objects> {
    let resolve = |name: &str| vendor.symbol(name);
    let [_, _, rate, counters] = words else {
        unreachable!("rate words: context, descriptor, initial rate, counter state")
    };
    // Through word 0x30, whose format flags the tail after `rcGetSMPDURate`
    // reads; clear flags select no DCM or rate clamp.
    let mut descriptor = [0u32; 13];
    descriptor[0] = format;
    descriptor[1] = *counters;
    // The selected-rate byte starts all set, so both sides must write it.
    descriptor[3] = u32::MAX;
    descriptor[7] = RATE_SCHEDULE;
    let objects = vec![
        (RATE_DESCRIPTOR, self::words(&descriptor)),
        (RATE_CONTEXT, vec![0; 16]),
        (
            RATE_SCHEDULE,
            vendor.context::<RateTables>()?.record(arena, *rate)?,
        ),
    ];
    let assert = CallDeclaration {
        id: "wifi-assert".into(),
        applicability: "a vendor assertion the reviewed schedule never fails".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address: resolve("wifi_assert")?,
            boundary: vendor.boundary("wifi_assert"),
            allow_tail: false,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    };
    Ok(Objects {
        vendor_words: vec![RATE_CONTEXT, RATE_DESCRIPTOR],
        vendor: objects.clone(),
        production: objects,
        calls: vec![assert],
        compared: vec![(RATE_SELECTED, 1)],
        ..Default::default()
    })
}

/// Transmit-error details of status four and the retry leaf each selects;
/// `0xc0` is a security-key error.
const TX_ERROR_DETAILS: &[u32] = &[0, 1, 2, 3, 4, 5, 6, TX_ERROR_KEY];
const TX_ERROR_KEY: u32 = 0xc0;
/// The vendor queue contexts `our_instances_ptr` names for the key-error
/// path, which marks the queue's exchange ending before dispatching: one
/// `TX_ERROR_INSTANCE_BYTES` context per queue.
const TX_ERROR_INSTANCES: u32 = 0x3fff_1000;
const TX_ERROR_INSTANCE_BYTES: usize = 0x38;
const TX_ERROR_QUEUE_CONTEXTS: usize = 5;
/// Ordinary queues the dispatcher cases use: the lowest and the highest.
const TX_ERROR_QUEUES: &[u32] = &[0, 4];

/// `lmacProcessTxError` of status four: detail zero is a CTS timeout,
/// details two and six ACK timeouts, `0xc0` a security-key error that ends
/// the exchange through `lmacProcessTxseckiderr(context, queue)`, and the
/// others collisions.
fn tx_error_leaf(words: &[u32]) -> DispatchTarget {
    match words[1] {
        0 => DispatchTarget::first("lmacProcessCtsTimeout", "open_libpp_tx_retry_cts_timeout"),
        2 | 6 => DispatchTarget::first("lmacProcessAckTimeout", "open_libpp_tx_retry_ack_timeout"),
        TX_ERROR_KEY => DispatchTarget {
            vendor: "lmacProcessTxseckiderr",
            production: "open_libpp_tx_retry_security_key_error",
            vendor_word: 1,
        },
        _ => DispatchTarget::first("lmacProcessCollision", "open_libpp_tx_retry_collision"),
    }
}

/// `lmacProcessTxError` loads ROM `our_instances_ptr` before dispatching;
/// the key-error detail writes the queue context it names.
fn tx_error_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let resolve = |name: &str| vendor.symbol(name);
    Ok(Objects {
        vendor_words: words.to_vec(),
        vendor: vec![
            (
                resolve("our_instances_ptr")?,
                TX_ERROR_INSTANCES.to_le_bytes().to_vec(),
            ),
            (
                TX_ERROR_INSTANCES,
                vec![0; TX_ERROR_QUEUE_CONTEXTS * TX_ERROR_INSTANCE_BYTES],
            ),
        ],
        ..Default::default()
    })
}

/// Logical MAC interface contexts: station, access point and two others.
const INTERFACES: &[u32] = &[0, 1, 2, 3];
/// Interface addresses: ascending bytes, all clear, all set with the group
/// bit, and an alternating pattern.
const ADDRESSES: &[[u8; 6]] = &[
    [0x00, 0x11, 0x22, 0x33, 0x44, 0x55],
    [0; 6],
    [0xff; 6],
    [0x5a, 0xa5, 0x5a, 0xa5, 0x5a, 0xa5],
];

/// Receive-descriptor base addresses: null, an internal-RAM word, an
/// alternating pattern and all bits.
const RX_BASES: &[u32] = &[0, 0x4080_0000, 0x5a5a_a5a4, u32::MAX];
/// Clear-channel-assessment selector values of the two-bit field.
const CCA: &[u32] = &[0, 1, 2, 3];
/// The only AP TSF reset selector production implements: a fresh epoch.
const TSF_FRESH_EPOCH: &[u32] = &[0];
/// The only queue-state clear selector production implements: ordinary
/// transmit completion.
const COMPLETION_CLEAR: &[u32] = &[2];

/// Boolean flag words.
const FLAGS: &[u32] = &[0, 1];
/// Words the vendor ignores: zero and a pattern.
const IGNORED: &[u32] = &[0, 0xa5a5_5a5a];
/// RTS thresholds in bytes, including one the 16-bit field truncates.
const RTS_THRESHOLDS: &[u32] = &[0, 1, 0xffff, 0x1_2345];

/// AIFSN values: zero, a default, the four-bit maximum and one bit beyond.
const AIFSN: &[u32] = &[0, 2, 15, 0x1f];
/// Contention windows: zero, a default, the ten-bit maximum and one beyond.
const WINDOWS: &[u32] = &[0, 0x15, 0x3ff, 0x7ff];
/// The EDCA object address the probe receives and ignores.
const EDCA_OBJECT: &[u32] = &[INPUT];

/// STA TSF low and high words, each through a nullable output pointer.
const TSF_LOW: Domain = Domain::Output {
    offset: 0,
    length: 4,
    nullable: true,
};
const TSF_HIGH: Domain = Domain::Output {
    offset: 4,
    length: 4,
    nullable: true,
};

/// Bytes of the transmit Block Ack record: control, sequence and bitmap.
const BLOCK_ACK_BYTES: u32 = 12;

/// Every compared leaf.
pub const LEAVES: &[Leaf] = &[
    // The association boundary orders its interface mode before subsequent
    // key publication. Compare every vendor register effect and require the
    // one additional production device fence.
    ordered(
        leaf(
            "hal_crypto_enable",
            "open_libpp_crypto_trace_hal_crypto_enable",
            &[
                ("interface", Domain::Words(&[0, 1])),
                ("_algorithm", Domain::Words(&[3])),
                ("_management", Domain::Words(&[1])),
                ("spp", Domain::Words(&[0, 1])),
            ],
            false,
        ),
        1,
    ),
    leaf(
        "hal_disable_softap_tsf",
        "open_libpp_ap_tsf_trace_hal_disable_softap_tsf",
        &[],
        false,
    ),
    leaf(
        "hal_mac_tsf_reset",
        "open_libpp_ap_tsf_start_trace_hal_mac_tsf_reset",
        &[("selector", Domain::Words(TSF_FRESH_EPOCH))],
        false,
    ),
    leaf(
        "hal_mac_interrupt_get_event",
        "open_libpp_trace_hal_mac_interrupt_ret_get_event",
        &[],
        true,
    ),
    ordered(
        leaf(
            "hal_mac_interrupt_clr_event",
            "open_libpp_trace_hal_mac_interrupt_ret_clr_event",
            &[("events", Domain::Words(EVENT_MASKS))],
            false,
        ),
        1,
    ),
    leaf(
        "hal_pwr_interrupt_get_event",
        "open_libpp_power_irq_trace_hal_pwr_interrupt_get_event",
        &[],
        true,
    ),
    ordered(
        leaf(
            "hal_pwr_interrupt_clr_event",
            "open_libpp_power_irq_trace_hal_pwr_interrupt_clr_event",
            &[("events", Domain::Words(EVENT_MASKS))],
            false,
        ),
        1,
    ),
    leaf(
        "hal_mac_rx_disable",
        "open_libpp_rx_trace_hal_mac_rx_disable",
        &[],
        false,
    ),
    leaf(
        "hal_mac_rx_enable",
        "open_libpp_rx_trace_hal_mac_rx_enable",
        &[],
        false,
    ),
    leaf(
        "hal_mac_rx_set_base",
        "open_libpp_rx_trace_hal_mac_rx_set_base",
        &[("address", Domain::Words(RX_BASES))],
        false,
    ),
    leaf(
        "hal_mac_rx_is_dscr_reload",
        "open_libpp_rx_trace_hal_mac_rx_is_dscr_reload",
        &[],
        true,
    ),
    leaf(
        "hal_mac_rx_set_dscr_reload",
        "open_libpp_rx_trace_hal_mac_rx_set_dscr_reload",
        &[],
        false,
    ),
    leaf(
        "hal_mac_tx_set_cca",
        "open_libpp_tx_trace_hal_mac_tx_set_cca",
        &[("value", Domain::Words(CCA))],
        true,
    ),
    leaf(
        "hal_mac_get_txq_in_trig_flow_state",
        "open_libpp_tx_trace_hal_mac_get_txq_in_trig_flow_state",
        &[],
        true,
    ),
    leaf(
        "hal_mac_is_txq_enabled",
        "open_libpp_tx_trace_hal_mac_is_txq_enabled",
        &[("queue", Domain::Words(QUEUES))],
        true,
    ),
    leaf(
        "hal_mac_is_txq_valid",
        "open_libpp_tx_trace_hal_mac_is_txq_valid",
        &[("queue", Domain::Words(QUEUES))],
        true,
    ),
    leaf(
        "hal_mac_set_txq_invalid",
        "open_libpp_tx_trace_hal_mac_set_txq_invalid",
        &[("queue", Domain::Words(QUEUES))],
        false,
    ),
    leaf(
        "hal_mac_txq_disable",
        "open_libpp_tx_trace_hal_mac_txq_disable",
        &[("queue", Domain::Words(QUEUES))],
        false,
    ),
    leaf(
        "hal_mac_txq_disable",
        "open_ordinary_tx_ownership_disable",
        &[("queue", Domain::Words(QUEUES))],
        false,
    ),
    leaf(
        "hal_mac_clr_txq_state",
        "open_ordinary_tx_ownership_acknowledge",
        &[
            ("selector", Domain::Words(COMPLETION_CLEAR)),
            ("queue", Domain::Words(QUEUES)),
        ],
        false,
    ),
    leaf(
        "hal_disable_sta_beacon_filter",
        "open_wifi_sta_trace_hal_disable_sta_beacon_filter",
        &[],
        false,
    ),
    leaf(
        "hal_mac_set_addr",
        "open_libpp_interface_trace_hal_mac_set_addr",
        &[
            ("interface", Domain::Words(INTERFACES)),
            ("address", Domain::Addresses(ADDRESSES)),
        ],
        false,
    ),
    leaf(
        "hal_mac_set_bssid",
        "open_libpp_interface_trace_hal_mac_set_bssid",
        &[
            ("interface", Domain::Words(INTERFACES)),
            ("address", Domain::Addresses(ADDRESSES)),
        ],
        false,
    ),
    leaf(
        "hal_he_set_tx_protection",
        "open_tx_protection_control_configure_rts",
        &[
            ("queue", Domain::Words(QUEUES)),
            ("enabled", Domain::Words(FLAGS)),
            ("_unused", Domain::Words(IGNORED)),
            ("duration_threshold", Domain::Words(FLAGS)),
            ("threshold_bytes", Domain::Words(RTS_THRESHOLDS)),
        ],
        false,
    ),
    leaf(
        "hal_he_disable_rts_threshold",
        "open_tx_protection_control_disable_he_threshold",
        &[],
        false,
    ),
    prefix(
        ordered(
            leaf(
                "hal_mac_txq_enable",
                "open_ordinary_tx_ownership_publish",
                &[("queue", Domain::Words(QUEUES))],
                false,
            ),
            2,
        ),
        "GetAccess",
    ),
    leaf(
        "hal_mac_tx_get_blockack",
        "open_libpp_tx_trace_hal_mac_tx_get_blockack",
        &[
            ("queue", Domain::Words(QUEUES)),
            ("output_address", output(BLOCK_ACK_BYTES)),
        ],
        true,
    ),
    objects(
        leaf(
            "hal_mac_tx_config_edca",
            "open_libpp_tx_trace_hal_mac_tx_config_edca",
            &[
                ("_vendor_config_address", Domain::Words(EDCA_OBJECT)),
                ("queue", Domain::Words(QUEUES)),
                ("aifsn", Domain::Words(AIFSN)),
                ("contention_window", Domain::Words(WINDOWS)),
                ("interface", Domain::Words(INTERFACES)),
            ],
            true,
        ),
        edca_abi,
    ),
    rom(leaf(
        "hal_get_sta_tsf",
        "open_rom_power_tsf_trace_hal_get_sta_tsf",
        &[("low", TSF_LOW), ("high", TSF_HIGH)],
        false,
    )),
    stated(
        objects(
            leaf(
                "hal_mac_tx_set_ppdu",
                "open_libpp_tx_trace_hal_mac_tx_set_ppdu",
                &[
                    ("program_address", Domain::Words(&[INPUT])),
                    ("_vendor_auxiliary", Domain::Words(&[PPDU_AUXILIARY])),
                    ("power_table", Domain::Words(&[PPDU_POWER_COPY])),
                ],
                false,
            ),
            ppdu_abi,
        ),
        &PPDU_RATES,
    ),
    stated(
        objects(
            leaf(
                "hal_mac_tx_set_ppdu",
                "open_libpp_tx_trace_hal_mac_tx_set_legacy_ppdu",
                &[
                    ("program_address", Domain::Words(&[INPUT])),
                    ("_vendor_auxiliary", Domain::Words(&[PPDU_AUXILIARY])),
                    ("power_table", Domain::Words(&[PPDU_POWER_COPY])),
                ],
                false,
            ),
            legacy_ppdu_abi,
        ),
        &PPDU_LEGACY_STATES,
    ),
    stated(
        objects(
            vendor_reads(
                leaf(
                    "hal_mac_tx_set_ppdu",
                    "open_libpp_tx_trace_hal_mac_tx_set_he_ppdu",
                    &[
                        ("program_address", Domain::Words(&[INPUT])),
                        ("_vendor_auxiliary", Domain::Words(&[PPDU_AUXILIARY])),
                        ("power_table", Domain::Words(&[PPDU_POWER_COPY])),
                    ],
                    false,
                ),
                &[(
                    PPDU_HE_BSS_COLOR_REGISTER,
                    "the vendor reads the station's BSS color from the MAC register \
                        `hal_he_get_bss_color` samples; production publishes the color its \
                        association owner holds",
                )],
            ),
            he_ppdu_abi,
        ),
        &PPDU_HE_STATES,
    ),
    stated(
        objects(
            leaf(
                "rcGetRate",
                "open_libpp_tx_retry_trace_rc_get_rate",
                &[
                    ("_rate_context", Domain::Words(&[RATE_CONTEXT])),
                    ("descriptor_address", Domain::Words(&[RATE_DESCRIPTOR])),
                    ("initial_rate", Domain::Words(&RETRY_RATE_CODES)),
                ],
                false,
            ),
            rate_abi,
        ),
        RATE_COUNTERS,
    ),
    stated(
        objects(
            leaf(
                "rcGetRate",
                "open_libpp_tx_retry_trace_rc_get_he_rate",
                &[
                    ("_rate_context", Domain::Words(&[RATE_CONTEXT])),
                    ("descriptor_address", Domain::Words(&[RATE_DESCRIPTOR])),
                    ("initial_rate", Domain::Words(HE_RATE_CODES)),
                ],
                false,
            ),
            he_rate_abi,
        ),
        RATE_COUNTERS,
    ),
    quiet(
        dispatching(
            objects(
                leaf(
                    "lmacProcessTxError",
                    "open_libpp_tx_retry_trace_lmac_process_tx_error",
                    &[
                        ("queue", Domain::Words(TX_ERROR_QUEUES)),
                        ("detail", Domain::Words(TX_ERROR_DETAILS)),
                        ("selector", Domain::Words(&[0])),
                    ],
                    false,
                ),
                tx_error_abi,
            ),
            tx_error_leaf,
        ),
        &["wifi_assert"],
    ),
    net80211(objects(
        leaf(
            "wifi_set_rx_policy",
            "open_wifi_sta_ap_trace_wifi_set_rx_policy",
            &[
                ("policy", Domain::Words(RX_POLICIES)),
                ("address_low", Domain::Words(RX_ADDRESS_LOW)),
                ("address_high", Domain::Words(RX_ADDRESS_HIGH)),
                ("mode", Domain::Words(RX_MODES)),
            ],
            false,
        ),
        rx_policy_abi,
    )),
    // Production publishes the interface addresses in its cold transaction
    // and claims only the suffix of policy zero; the vendor address
    // republication is answered without effect.
    quiet(
        net80211(objects(
            leaf(
                "wifi_set_rx_policy",
                "open_wifi_sta_ap_trace_disable_all_role_receive",
                &[("_policy", Domain::Words(&[RX_POLICY_NONE]))],
                false,
            ),
            rx_disable_all_abi,
        )),
        &["ic_set_mac"],
    ),
    crate::low_power_clock::SELECT,
];

/// The record index of every rate code in `codes`, returned by the vendor's
/// own index function `function` executed in the linked image.
fn schedule_indices(
    session: &mut Session,
    vendor: &ExecutionTarget,
    image: &BTreeMap<String, u32>,
    function: &str,
    codes: &[u32],
) -> Result<BTreeMap<u32, u8>> {
    let entry = *image
        .get(function)
        .ok_or_else(|| invalid(format!("the linked image lacks {function}")))?;
    let rows = codes
        .iter()
        .map(|code| {
            let mut row = case(
                format!("{function}-{code:x}"),
                direct(entry, &[*code], vec![], vec![], vec![]),
                None,
                SessionReset::Cold,
                false,
            );
            // Vendor-only: nothing to compare.
            row.relation = None;
            row.stack_fill = Some(LEAF_FILLS[0]);
            row
        })
        .collect();
    let records = session
        .submit(
            function,
            &request(vendor, None, None, rows, LEAF_EVENTS),
            None,
        )?
        .records
        .clone();
    codes
        .iter()
        .enumerate()
        .map(|(case, code)| {
            let index = crate::i2c::returned_low(&records, case as u32, false)
                .ok_or_else(|| invalid(format!("{function} returned nothing for {code:#x}")))?;
            Ok((*code, u8::try_from(index)?))
        })
        .collect()
}

/// Every byte of data section `section` of `object` in the captured archive.
fn vendor_section(session: &Session, object: &str, section: &str) -> Result<Vec<u8>> {
    let object = crate::harness::named_object(&session.inventory, 0, object)?;
    let record = crate::harness::named_section(object, section)?;
    let request = crate::harness::data_request(
        object,
        blobray_domain::DataSelector::Section {
            section: record.index,
            offset: 0,
            length: record.size,
        },
    );
    let name = format!("section{}", section.replace('.', "-"));
    Ok(session.data(&name, &request)?.bytes)
}

/// `wifi_set_rx_policy` policies the production role-receive HAL admits:
/// station disabled, station, access point and access point disabled.
const RX_POLICIES: &[u32] = &[2, 6, 8, 9];
/// Low and high words of the policy address: all clear and a pattern.
const RX_ADDRESS_LOW: &[u32] = &[0, 0x3322_1100];
const RX_ADDRESS_HIGH: &[u32] = &[0, 0x5544];
/// `wifi_set_rx_policy` code that republishes both interface addresses and
/// disables both receive contexts.
const RX_POLICY_NONE: u32 = 0;
/// Station receive modes of policy six.
const RX_MODES: &[u32] = &[1, 2];
/// `g_ic` bytes the policy reads and writes, and its fields: the word
/// pointing to the station record, the station-mode word and the
/// access-point address.
const IC_BYTES: usize = 0x2d0;
const IC_STATION: usize = 0;
const IC_STATION_MODE: usize = 0x74;
const IC_AP_ADDRESS: usize = 0x214;
/// Station record word at 0x1460 points to the BSS record, whose bytes
/// 4..10 carry the BSSID.
const RX_STATION: u32 = 0x3fff_7000;
const RX_STATION_BSS: u32 = 0x1460;
const RX_BSS: u32 = 0x3fff_9000;

/// The policy's `g_ic` context and station records built from the probe
/// words: policy, address low and high words and the station mode.
fn rx_policy_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [policy, low, high, mode] = words else {
        unreachable!("receive-policy words: policy, address low, address high, mode")
    };
    let mut address = low.to_le_bytes().to_vec();
    address.extend(&high.to_le_bytes()[..2]);
    let mut context = vec![0u8; IC_BYTES];
    context[IC_STATION..IC_STATION + 4].copy_from_slice(&RX_STATION.to_le_bytes());
    context[IC_STATION_MODE..IC_STATION_MODE + 4]
        .copy_from_slice(&u32::from(*mode == 2).to_le_bytes());
    context[IC_AP_ADDRESS..IC_AP_ADDRESS + 6].copy_from_slice(&address);
    let mut bss = vec![0u8; 4];
    bss.extend(&address);
    Ok(Objects {
        vendor_words: vec![*policy],
        vendor: vec![
            (RX_STATION + RX_STATION_BSS, RX_BSS.to_le_bytes().to_vec()),
            (RX_BSS, bss),
        ],
        image: vec![(vendor.symbol("g_ic")?, context)],
        ..Default::default()
    })
}

/// The policy-zero `g_ic` context: no address and the first station mode.
fn rx_disable_all_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    rx_policy_abi(&[words[0], 0, 0, RX_MODES[0]], vendor)
}
