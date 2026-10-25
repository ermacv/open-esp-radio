//! Wi-Fi MAC HAL leaves of the pinned `libpp.a` against compiled production.
//!
//! Each leaf is one vendor function and the production probe that runs its
//! HAL counterpart. Every case runs both over the whole radio register block
//! retained from one fill pattern, so every register bit the leaf reads takes
//! both values across the fills, and compares every register effect exactly
//! and, where the leaf returns one, the return word. Production borrows the
//! probe image's session ([`crate::session_entry`]), which every case starts
//! first.
use crate::harness::{Result, case, direct, invalid};
use crate::session::{Session, request};
use oer_analysis_blobray_domain::{
    CallBinding, CallDeclaration, CallRepetition, CallResponse, ExecutionTarget, RegionLifetime,
    SessionReset,
};
pub use oer_vendor_scenario::leaf::{
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

/// Artifact ids of the ROM and of the vendor firmware every ESP32-S31 leaf
/// suite links: the firmware supplies the network-stack data and logging
/// symbols that code after a compared prefix references. Every suite keeps
/// the `mac` contract identifier its reviewed contracts carry.
pub const ROM: &str = "rom";
pub const FIRMWARE: &str = "phy-sdk";
pub const CONTRACT_ID: &str = "mac";

/// Link roots of the Wi-Fi rate tables, beyond the leaves.
const WIFI_ROOTS: &[&[&str]] = &[&[RateTables::INDEX_CALLER]];

/// The Wi-Fi MAC suite over `libpp.a` and `libnet80211.a`, with the rate
/// tables.
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
    claims: &[],
    session: Some(crate::session_entry::ENTRY),
};

/// The vendor rate tables the Wi-Fi builders read, checked against the rate
/// domain production admits.
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

fn words(values: &[u32]) -> Vec<u8> {
    values.iter().flat_map(|w| w.to_le_bytes()).collect()
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

/// The only AP TSF reset selector production implements: a fresh epoch.
const TSF_FRESH_EPOCH: &[u32] = &[0];

/// STA TSF low and high words, each through a nullable output pointer.
/// The STA TSF outputs, both present: the production snapshot reads both
/// words, so a null output, which the vendor's conditional reads skip, has
/// no production counterpart.
const TSF_LOW: Domain = Domain::Output {
    offset: 0,
    length: 4,
    nullable: false,
};
const TSF_HIGH: Domain = Domain::Output {
    offset: 4,
    length: 4,
    nullable: false,
};

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
        "hal_mac_rx_is_dscr_reload",
        "open_libpp_rx_trace_hal_mac_rx_is_dscr_reload",
        &[],
        true,
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
    rom(leaf(
        "hal_get_sta_tsf",
        "open_rom_power_tsf_trace_hal_get_sta_tsf",
        &[("low", TSF_LOW), ("high", TSF_HIGH)],
        false,
    )),
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
        oer_analysis_blobray_domain::DataSelector::Section {
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
