//! The coexistence time-slice schedule of `libcoexist.a[coexist_scheme.o]`.
//!
//! Every case seeds the vendor `coex_schm_env` with one schedule state and
//! runs one schedule entry: the scheme selection, a status-bit set or clear,
//! a phase timeout or a restart. The production probe receives the same
//! state as a vendor-shaped image and the vendor address of every scheme, so
//! both report the selected scheme as the same pointer. The cases compare the
//! resulting scheme, phase index and status words, and the scenario checks
//! that production re-arms the phase timer for the same microseconds and
//! notifies the same radios as the vendor's modeled timer and callbacks.
use crate::harness::{Arg, Input, Result, case, direct, invalid, region};
use crate::phy::{image_layout, select};
use crate::session::{Session, request};
use oer_analysis_blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, ComparisonVerdict,
    ExecutionCase, ExecutionEvent, ExecutionEvidence, LinkRequest, MemoryPair, MemorySelection,
    RegionLifetime, SessionReset,
};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Global entries of `coexist_scheme.o` linked as roots; the first is the
/// link entry. `coex_schm_init` installs the static
/// `coex_schm_timeout_process` as the phase-timer function, which links it;
/// the static `coex_schm_status_change` is linked through the status entries.
/// Cases enter the static functions at their image addresses, and the
/// claims name them there.
pub const ROOTS: &[&str] = &[
    "coex_schm_status_bit_set",
    "coex_schm_status_bit_clear",
    "coex_schm_process_restart",
    "coex_schm_init",
];
/// Evidence claims: each vendor schedule entry with the production schedule.
pub const CLAIMS: &[(&str, &str, &str)] = &[
    (
        "archive",
        "coex_schm_status_bit_set",
        "open_coex_schm_trace_step",
    ),
    (
        "archive",
        "coex_schm_status_bit_clear",
        "open_coex_schm_trace_step",
    ),
    (
        "archive",
        "coex_schm_process_restart",
        "open_coex_schm_trace_step",
    ),
    (
        "archive",
        "coex_schm_status_change",
        "open_coex_schm_trace_step",
    ),
    (
        "archive",
        "coex_schm_timeout_process",
        "open_coex_schm_trace_step",
    ),
];
/// The schemes `coexist_scheme.o` defines.
const SCHEMES: usize = 107;
/// Guest events one case may record.
const EVENTS: u32 = 1 << 8;

/// `coex_schm_env` fields: scheme pointer, phase index, the Wi-Fi, BLE and
/// BT status words, interval, semaphore, Bluetooth callback, Wi-Fi callback,
/// external-coexistence status, flexible period and IEEE 802.15.4 status.
const ENV_BYTES: usize = 64;
const ENV_SCHEME: usize = 0;
const ENV_PHASE: usize = 4;
const ENV_WIFI: usize = 6;
const ENV_BLE: usize = 8;
const ENV_BT: usize = 10;
const ENV_INTERVAL: usize = 12;
const ENV_SEMAPHORE: usize = 16;
const ENV_BT_CALLBACK: usize = 20;
const ENV_WIFI_CALLBACK: usize = 52;
const ENV_EXTERNAL: usize = 56;
const ENV_FLEXIBLE: usize = 58;
const ENV_IEEE802154: usize = 60;
/// The compared fields: the scheme pointer, the phase index, the Wi-Fi, BLE
/// and BT words, the external-coexistence word and the IEEE 802.15.4 word.
const COMPARED: [(&str, u32, u32); 5] = [
    ("scheme", ENV_SCHEME as u32, 4),
    ("phase-index", ENV_PHASE as u32, 1),
    ("wifi-ble-bt-status", ENV_WIFI as u32, 6),
    ("external-status", ENV_EXTERNAL as u32, 2),
    ("ieee802154-status", ENV_IEEE802154 as u32, 2),
];

/// `coex_adapter_funcs_t` of the S31 build: semaphore take and give, timer
/// disarm and `timer_arm_us`.
const ADAPTER: u32 = 0x3fff_8000;
const ADAPTER_BYTES: usize = 72;
const ADAPTER_TAKE: usize = 24;
const ADAPTER_GIVE: usize = 28;
const ADAPTER_DISARM: usize = 52;
const ADAPTER_ARM: usize = 64;
/// Unmapped addresses of the modeled adapter functions and callbacks.
const TAKE: u32 = 0x5000_0000;
const GIVE: u32 = 0x5000_0010;
const DISARM: u32 = 0x5000_0020;
const ARM: u32 = 0x5000_0030;
const WIFI_CALLBACK: u32 = 0x5000_0040;
const BT_CALLBACK: u32 = 0x5000_0050;
/// Source of the adapter-table pointer the setup case copies.
const SETUP_SOURCE: u32 = 0x3fff_8100;
/// Source of the environment image a patch case copies.
const ENV_SOURCE: u32 = 0x3fff_8200;
/// Unmapped addresses reported for schemes the linker dropped.
const UNLINKED: u32 = 0x5100_0000;
/// A nonzero semaphore handle, so the schedule lock is taken.
const SEMAPHORE: u32 = 0x5000_1000;

/// Production-side scratch: the state image, the scheme address table and
/// the four-word report.
const IMAGE: u32 = 0x3fff_9000;
const TABLE: u32 = 0x3fff_9100;
const REPORT: u32 = 0x3fff_9400;

/// Schedule entries in the probe's operation numbering.
#[derive(Clone, Copy, Debug)]
enum Operation {
    Select,
    Set { kind: u32, bits: u32 },
    Clear { kind: u32, bits: u32 },
    Timeout,
    Restart,
}

impl Operation {
    const fn code(self) -> u32 {
        match self {
            Self::Select => 0,
            Self::Set { .. } => 1,
            Self::Clear { .. } => 2,
            Self::Timeout => 3,
            Self::Restart => 4,
        }
    }

    const fn entry(self) -> &'static str {
        match self {
            Self::Select => "coex_schm_status_change",
            Self::Set { .. } => "coex_schm_status_bit_set",
            Self::Clear { .. } => "coex_schm_status_bit_clear",
            Self::Timeout => "coex_schm_timeout_process",
            Self::Restart => "coex_schm_process_restart",
        }
    }

    const fn arguments(self) -> (u32, u32) {
        match self {
            Self::Set { kind, bits } | Self::Clear { kind, bits } => (kind, bits),
            _ => (0, 0),
        }
    }
}

/// One schedule state: the five status words in `[wifi, ble, bt, external,
/// i]` order, the scheme, the phase index and the interval.
#[derive(Clone, Copy, Debug)]
struct State {
    status: [u16; 5],
    scheme: usize,
    phase: u8,
    interval: u32,
}

pub struct CoexOptions {
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: crate::harness::Budget,
}

pub struct Coex {
    pub session: Session,
    pub roots: BTreeMap<String, u32>,
    pub run: PathBuf,
    vendor: oer_analysis_blobray_domain::ExecutionTarget,
    production: oer_analysis_blobray_domain::ExecutionTarget,
    env: u32,
    funcs: u32,
    /// Image addresses of every schedule entry the cases enter.
    entries: BTreeMap<&'static str, u32>,
    /// The vendor address of every scheme, in name order.
    schemes: Vec<u32>,
    /// The phase count of every scheme, read from its vendor image.
    phases: Vec<u8>,
}

impl Coex {
    pub fn new(options: &CoexOptions) -> Result<Self> {
        let inputs = [
            Input {
                role: "libcoexist",
                path: &options.library,
                sha256: Some(crate::artifacts::sha256("libcoexist")?),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(crate::artifacts::sha256("rom")?),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
        ];
        let session = Session::start(
            &options.output,
            options.budget,
            &inputs,
            "coexistence schedule comparison",
        )?;
        let link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: select(&session, 0, ROOTS[0])?,
            roots: ROOTS[1..]
                .iter()
                .map(|root| select(&session, 0, root))
                .collect::<Result<_>>()?,
            layout: image_layout(),
            // Only `coex_schm_init`'s failed-semaphore branch prints.
            absent: vec!["coexist_printf".into()],
        };
        let linked = session.link(
            &link,
            &options.linker,
            ROOTS[0],
            &[crate::layout::ROM_INPUT],
        )?;
        let (vendor, production) = session.targets(&linked.manifest.elf)?;
        let elf = std::fs::read(session.run.join("image/image.elf"))?;
        let (symbols, _) = image_data(&elf)?;
        let symbol = |name: &str| {
            symbols
                .get(name)
                .copied()
                .ok_or_else(|| invalid(format!("the image lacks {name}")))
        };
        // Every scheme the object defines, in name order, with its bytes.
        // The linker drops schemes no selection references; those keep a
        // unique unmapped address the vendor can never report.
        let defined = archive_schemes(&std::fs::read(&options.library)?)?;
        if defined.len() != SCHEMES {
            return Err(invalid(format!(
                "coexist_scheme.o defines {} schemes, not {SCHEMES}",
                defined.len()
            )));
        }
        let schemes = defined
            .iter()
            .enumerate()
            .map(|(index, (name, _))| {
                symbols
                    .get(name)
                    .copied()
                    .unwrap_or(UNLINKED + 16 * index as u32)
            })
            .collect();
        let phases = defined
            .iter()
            .map(|(name, bytes)| {
                bytes
                    .first()
                    .copied()
                    .ok_or_else(|| invalid(format!("{name} has no bytes")))
            })
            .collect::<Result<_>>()?;
        let entries = [
            "coex_schm_status_bit_set",
            "coex_schm_status_bit_clear",
            "coex_schm_status_change",
            "coex_schm_timeout_process",
            "coex_schm_process_restart",
        ]
        .into_iter()
        .map(|name| Ok((name, symbol(name)?)))
        .collect::<Result<_>>()?;
        Ok(Self {
            entries,
            env: symbol("coex_schm_env")?,
            funcs: symbol("g_coa_funcs_p")?,
            schemes,
            phases,
            run: session.run.clone(),
            roots: linked.roots,
            vendor,
            production,
            session,
        })
    }

    /// The vendor-shaped environment image of `state`.
    fn image(&self, state: State) -> Vec<u8> {
        let mut env = vec![0u8; ENV_BYTES];
        let mut put = |at: usize, bytes: &[u8]| env[at..at + bytes.len()].copy_from_slice(bytes);
        put(ENV_SCHEME, &self.schemes[state.scheme].to_le_bytes());
        put(ENV_PHASE, &[state.phase]);
        for (at, word) in [ENV_WIFI, ENV_BLE, ENV_BT, ENV_EXTERNAL, ENV_IEEE802154]
            .into_iter()
            .zip(state.status)
        {
            put(at, &word.to_le_bytes());
        }
        put(ENV_INTERVAL, &state.interval.to_le_bytes());
        put(ENV_SEMAPHORE, &SEMAPHORE.to_le_bytes());
        put(ENV_BT_CALLBACK, &BT_CALLBACK.to_le_bytes());
        put(ENV_WIFI_CALLBACK, &WIFI_CALLBACK.to_le_bytes());
        put(ENV_FLEXIBLE, &[0]);
        env
    }

    /// One compared case of `operation` from `state`.
    fn case(&self, name: String, state: State, operation: Operation) -> Result<ExecutionCase> {
        let env = self.image(state);
        let phase = |address: u32, bytes: &[u8]| {
            region(
                address,
                bytes.len() as u32,
                bytes,
                None,
                RegionLifetime::Phase,
            )
        };
        let observed = |base: u32| -> Vec<MemorySelection> {
            COMPARED
                .iter()
                .map(|(name, offset, length)| MemorySelection {
                    name: (*name).into(),
                    address: base + offset,
                    length: *length,
                })
                .collect()
        };
        let (kind, bits) = operation.arguments();
        let entry = self.entries[operation.entry()];
        let mut vendor = direct(entry, &[kind, bits], vec![], vec![], observed(self.env));
        vendor.calls = [
            ("semaphore-take", TAKE, 2, 1),
            ("semaphore-give", GIVE, 1, 1),
            ("timer-disarm", DISARM, 1, 0),
            ("timer-arm-us", ARM, 3, 0),
            ("wifi-phase-callback", WIFI_CALLBACK, 1, 0),
            ("bt-phase-callback", BT_CALLBACK, 1, 0),
        ]
        .into_iter()
        .map(|(id, address, words, value)| model(id, address, words, value))
        .collect();
        let table: Vec<u8> = self.schemes.iter().flat_map(|a| a.to_le_bytes()).collect();
        let mut production = self.session.probes.invoke(
            "open_coex_schm_trace_step",
            vec![
                ("env", Arg::Word(Some(i64::from(IMAGE)))),
                ("schemes", Arg::Word(Some(i64::from(TABLE)))),
                ("operation", Arg::Word(Some(i64::from(operation.code())))),
                ("kind", Arg::Word(Some(i64::from(kind)))),
                ("bits", Arg::Word(Some(i64::from(bits)))),
                ("report", Arg::Word(Some(i64::from(REPORT)))),
            ],
            vec![],
            observed(IMAGE)
                .into_iter()
                .chain([MemorySelection {
                    name: "report".into(),
                    address: REPORT,
                    length: 16,
                }])
                .collect(),
        )?;
        production.memory.extend([
            phase(IMAGE, &env)?,
            phase(TABLE, &table)?,
            phase(REPORT, &[0; 16])?,
        ]);
        production.arguments.resize(8, Some(0));
        let mut row = case(name, vendor, Some(production), SessionReset::Warm, false);
        let relation = row.relation.as_mut().expect("compared case");
        relation.memory = (0..COMPARED.len() as u16)
            .map(|index| MemoryPair {
                vendor: index,
                replacement: index,
            })
            .collect();
        Ok(row)
    }

    /// Submit `rows`, require MATCH, and check each case's timer and
    /// callbacks against the production report.
    /// The ROM `memcpy`.
    fn memcpy(&self) -> Result<u32> {
        Ok(u32::try_from(
            crate::harness::symbol(
                &self.session.inventory,
                crate::layout::ROM_INPUT as usize,
                "memcpy",
            )?
            .value,
        )?)
    }

    /// The cold setup case: the session-long adapter table and the ROM
    /// data cell `g_coa_funcs_p`, which has no loaded mapping, pointing at
    /// it.
    fn setup(&self) -> Result<ExecutionCase> {
        let mut adapter = vec![0u8; ADAPTER_BYTES];
        for (slot, target) in [
            (ADAPTER_TAKE, TAKE),
            (ADAPTER_GIVE, GIVE),
            (ADAPTER_DISARM, DISARM),
            (ADAPTER_ARM, ARM),
        ] {
            adapter[slot..slot + 4].copy_from_slice(&target.to_le_bytes());
        }
        let session = |address: u32, bytes: &[u8]| {
            region(
                address,
                bytes.len() as u32,
                bytes,
                None,
                RegionLifetime::Session,
            )
        };
        let memcpy = self.memcpy()?;
        let vendor = direct(
            memcpy,
            &[SETUP_SOURCE, SETUP_SOURCE, 0],
            vec![
                session(ADAPTER, &adapter)?,
                session(self.funcs, &ADAPTER.to_le_bytes())?,
            ],
            vec![],
            vec![],
        );
        let production = direct(
            memcpy,
            &[SETUP_SOURCE, SETUP_SOURCE, 0],
            vec![],
            vec![],
            vec![],
        );
        Ok(crate::harness::setup(
            "coex-setup",
            vendor,
            production,
            SessionReset::Cold,
        ))
    }

    /// The warm patch case before one compared case: the ROM `memcpy`
    /// writes the case's state over the vendor `coex_schm_env`, which the
    /// linked image maps.
    fn patch(&self, name: &str, state: State) -> Result<ExecutionCase> {
        let env = self.image(state);
        let memcpy = self.memcpy()?;
        let vendor = direct(
            memcpy,
            &[self.env, ENV_SOURCE, ENV_BYTES as u32],
            vec![region(
                ENV_SOURCE,
                ENV_BYTES as u32,
                &env,
                None,
                RegionLifetime::Phase,
            )?],
            vec![],
            vec![],
        );
        let production = direct(memcpy, &[ENV_SOURCE, ENV_SOURCE, 0], vec![], vec![], vec![]);
        Ok(crate::harness::setup(
            format!("{name}-patch"),
            vendor,
            production,
            SessionReset::Warm,
        ))
    }

    fn submit(&mut self, label: &str, cases: Vec<(String, State, Operation)>) -> Result<()> {
        let count = cases.len() as u32;
        let mut rows = vec![self.setup()?];
        for (name, state, operation) in cases {
            rows.push(self.patch(&name, state)?);
            rows.push(self.case(name, state, operation)?);
        }
        let (vendor, production) = (self.vendor.clone(), self.production.clone());
        let records = self
            .session
            .submit(
                label,
                &request(&vendor, Some(&production), None, rows, EVENTS),
                Some(ComparisonVerdict::Match),
            )?
            .records
            .clone();
        for case in (0..count).map(|index| 2 + 2 * index) {
            let vendor = vendor_step(&records, case);
            let report = report(&records, case)?;
            let expected = match vendor {
                None => [0; 4],
                Some(step) => [
                    u32::from(step.armed.is_some()),
                    step.armed.unwrap_or(0),
                    u32::from(step.wifi),
                    u32::from(step.bluetooth),
                ],
            };
            if report != expected {
                return Err(invalid(format!(
                    "{label} case {case}: production reported {report:x?}, the vendor {expected:x?}"
                )));
            }
        }
        Ok(())
    }
}

fn model(id: &str, address: u32, words: u16, value: u32) -> CallDeclaration {
    CallDeclaration {
        id: id.into(),
        applicability: "an adapter function or phase callback of the schedule".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary: CallBoundary::Unmapped,
            allow_tail: true,
        },
        argument_words: words,
        responses: vec![CallResponse {
            return_words: [Some(value), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// Symbol addresses of the linked image, and the bytes of every data symbol.
#[allow(clippy::type_complexity)]
fn image_data(elf: &[u8]) -> Result<(BTreeMap<String, u32>, BTreeMap<String, Vec<u8>>)> {
    let file = oer_analysis_elf::Elf::parse(elf)?;
    let mut symbols = BTreeMap::new();
    let mut data = BTreeMap::new();
    for symbol in file.symbols() {
        let (name, Ok(address)) = (symbol.name, u32::try_from(symbol.address)) else {
            continue;
        };
        if name.is_empty() || !symbol.defined {
            continue;
        }
        symbols.entry(name.to_owned()).or_insert(address);
        if symbol.kind == oer_analysis_elf::SymbolKind::Data
            && let Some(index) = symbol.section
        {
            let section = file.section(index)?;
            let offset = (symbol.address - section.address) as usize;
            let bytes = section.data;
            let end = offset + symbol.size as usize;
            if end <= bytes.len() {
                data.insert(name.to_owned(), bytes[offset..end].to_vec());
            }
        }
    }
    Ok((symbols, data))
}

/// The schemes of `coexist_scheme.o` in `archive`: every `coex_schm_<name>`
/// data object except the environment, in name order, with its bytes.
fn archive_schemes(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    for (member, data) in oer_analysis_elf::members(bytes)? {
        if member != "coexist_scheme.o" {
            continue;
        }
        let file = oer_analysis_elf::Elf::parse(data)?;
        let mut schemes = vec![];
        for symbol in file.symbols() {
            let name = symbol.name;
            if symbol.kind != oer_analysis_elf::SymbolKind::Data
                || !name.starts_with("coex_schm_")
                || name == "coex_schm_env"
            {
                continue;
            }
            let index = symbol
                .section
                .ok_or_else(|| invalid(format!("{name} has no section")))?;
            let data = file.section(index)?.data;
            let start = symbol.address as usize;
            let end = start + symbol.size as usize;
            let bytes = data
                .get(start..end)
                .ok_or_else(|| invalid(format!("{name} lies outside its section")))?;
            schemes.push((name.to_owned(), bytes.to_vec()));
        }
        schemes.sort();
        return Ok(schemes);
    }
    Err(invalid("the archive has no coexist_scheme.o"))
}

/// The phase step of one vendor case: the timer microseconds when re-armed
/// and the callbacks it called. `None` when the vendor never disarmed.
#[derive(Debug)]
struct VendorStep {
    armed: Option<u32>,
    wifi: bool,
    bluetooth: bool,
}

fn vendor_step(records: &[ExecutionEvidence], case: u32) -> Option<VendorStep> {
    let events = crate::evidence::events(records, case, false);
    let mut step: Option<VendorStep> = None;
    let mut current = None;
    for event in events {
        match event {
            ExecutionEvent::ModeledCall { target, .. } => {
                current = Some(target);
                match target {
                    DISARM => {
                        step = Some(VendorStep {
                            armed: None,
                            wifi: false,
                            bluetooth: false,
                        })
                    }
                    WIFI_CALLBACK => {
                        step.get_or_insert(VendorStep {
                            armed: None,
                            wifi: false,
                            bluetooth: false,
                        })
                        .wifi = true
                    }
                    BT_CALLBACK => {
                        step.get_or_insert(VendorStep {
                            armed: None,
                            wifi: false,
                            bluetooth: false,
                        })
                        .bluetooth = true
                    }
                    _ => {}
                }
            }
            ExecutionEvent::CallArgument { word: 1, value } if current == Some(ARM) => {
                if let Some(step) = step.as_mut() {
                    step.armed = value;
                }
            }
            _ => {}
        }
    }
    step
}

/// The production report words of one case: its sixth selection.
fn report(records: &[ExecutionEvidence], case: u32) -> Result<[u32; 4]> {
    let mut bytes = [0u8; 16];
    let mut seen = 0;
    for record in records {
        if let ExecutionEvidence::FinalMemory {
            case: c,
            replacement: true,
            chunk,
        } = record
            && *c == case
            && usize::from(chunk.selection) == COMPARED.len()
        {
            let start = chunk.offset as usize;
            let length = usize::from(chunk.length);
            bytes[start..start + length].copy_from_slice(&chunk.bytes[..length]);
            seen += length;
        }
    }
    if seen != bytes.len() {
        return Err(invalid(format!("case {case} left an incomplete report")));
    }
    Ok(std::array::from_fn(|i| {
        u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().expect("four bytes"))
    }))
}

/// Wi-Fi status values: none, scan, connecting, connected, the
/// connectionless window alone, and scan with connecting.
const WIFI: [u16; 6] = [0, 0x01, 0x02, 0x04, 0x40, 0x03];
/// BLE status values: none, a default activity bit, the mesh-traffic
/// alternative bit, mesh config, mesh traffic, mesh standby and all of them.
const BLE: [u16; 7] = [0, 0x01, 0x02, 0x08, 0x10, 0x20, 0x3a];
/// Classic Bluetooth status values: none, each tested bit and a bit outside
/// every tested mask.
const BT: [u16; 10] = [0, 0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x100];
/// External-coexistence and IEEE 802.15.4 status values.
const EXTERNAL: [u16; 3] = [0, 0x01, 0x03];
const IEEE802154: [u16; 2] = [0, 0x01];
const INTERVALS: [u32; 2] = [1024, 0x5a5];

/// Compare every schedule entry.
pub fn exercise(ctx: &mut Coex) -> Result<()> {
    // Selection over every combination of the tested status values.
    for external in EXTERNAL {
        let mut rows = vec![];
        for wifi in WIFI {
            for ble in BLE {
                for bt in BT {
                    for ieee802154 in IEEE802154 {
                        let state = State {
                            status: [wifi, ble, bt, external, ieee802154],
                            scheme: 0,
                            phase: 0,
                            interval: INTERVALS[0],
                        };
                        rows.push((
                            format!("select-{wifi:x}-{ble:x}-{bt:x}-{external:x}-{ieee802154:x}"),
                            state,
                            Operation::Select,
                        ));
                    }
                }
            }
        }
        ctx.submit(&format!("coex-select-{external:x}"), rows)?;
    }
    // Status changes from every Wi-Fi state with and without BLE, including
    // the first looping status that restarts the phases.
    let mut rows = vec![];
    for wifi in WIFI {
        for ble in [0, 0x01] {
            for (kind, bits) in [
                (0, 0x01),
                (0, 0x04),
                (0, 0x40),
                (1, 0x01),
                (1, 0x08),
                (2, 0x10),
                (3, 0x01),
                (4, 0x01),
                (5, 0x01),
            ] {
                for interval in INTERVALS {
                    let state = State {
                        status: [wifi, ble, 0, 0, 0],
                        scheme: 0,
                        phase: 1,
                        interval,
                    };
                    for operation in [
                        Operation::Set { kind, bits },
                        Operation::Clear { kind, bits },
                    ] {
                        rows.push((
                            format!("status-{operation:?}-{wifi:x}-{ble:x}-{interval:x}"),
                            state,
                            operation,
                        ));
                    }
                }
            }
        }
    }
    ctx.submit("coex-status", rows)?;
    // Phase changes of every scheme at each phase and one index beyond it,
    // under looping and non-looping status.
    for status in [
        [0x01, 0x01, 0, 0, 0],
        [0x04, 0x01, 0, 0, 0],
        [0x40, 0, 0, 0, 0x01],
        [0x04, 0, 0, 0, 0],
    ] {
        let mut rows = vec![];
        // A scheme the linker dropped is never selected, so never current.
        for scheme in (0..SCHEMES).filter(|&scheme| ctx.schemes[scheme] < UNLINKED) {
            for phase in 0..=ctx.phases[scheme] {
                for operation in [Operation::Timeout, Operation::Restart] {
                    let state = State {
                        status,
                        scheme,
                        phase,
                        interval: INTERVALS[1],
                    };
                    rows.push((
                        format!("phase-{operation:?}-{scheme}-{phase}-{:x}", status[0]),
                        state,
                        operation,
                    ));
                }
            }
        }
        ctx.submit(
            &format!("coex-phase-{:x}-{:x}-{:x}", status[0], status[1], status[4]),
            rows,
        )?;
    }
    Ok(())
}
