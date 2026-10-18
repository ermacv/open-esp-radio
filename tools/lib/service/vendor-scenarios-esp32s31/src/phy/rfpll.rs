//! Captured RFPLL search and frequency maintenance against compiled production.
use crate::calibration_prefix::delays_of;
use crate::contracts::{
    OutputField, omitted_read_before, output_projection, plumbing, port_polling,
};
use crate::evidence::{events, stop};
use crate::harness::direct;
use crate::harness::{Result, case, known, region, selection, with_stack_fill, words};
use crate::i2c::{I2c, all_complete, models, returned_low};
use crate::layout::*;
use crate::phy::delay_calls;
use blobray_domain::{
    CommandCell, ComparisonVerdict, DeviceBehavior, DeviceDeclaration, EffectRule, ExecutionCase,
    ExecutionEvent, ExecutionEvidence, ExecutionGap, ExecutionRegion, ExecutionStop, Invocation,
    MemoryAccess, MemoryTransaction, ModelStatus, ReadRun, RegionLifetime, RegisterCell,
    SessionReset,
};

/// Independently read candidate domains and signed outputs of the pinned archive.
/// These finite expectations are never an execution model or replacement body:
/// name, initial capacitor, lock statuses, candidates and selected capacitor.
/// Name, initial capacitor, lock statuses, candidates and selected capacitor.
pub type Search = (&'static str, i32, Vec<u32>, Vec<i32>, i32);

/// Status samples one direction of the archived `phy_rfpll_cap_init_cal_track`
/// search permits, read independently from its loop bound.
pub const SEARCH_SAMPLES: i32 = 30;
/// Status samples of a search that exhausts both directions.
const SEARCH_STATUSES: usize = 2 * SEARCH_SAMPLES as usize;

/// Reviewed divergence: production bounds the typed capacitor code to nine
/// bits and averages the codes it programs.
const BOUNDED_CODE: &str = "the user decided to bound the typed RFPLL capacitor code to nine bits: \
    production programs no upward candidate above 0x1ff and ends that direction there, and \
    averages the programmed codes (a negative candidate programs zero, as the ROM writes), where \
    the vendor searches past 0x1ff, leaks bit 9 into RFPLL_INITIAL_CONFIGURATION_HIGH and averages \
    the requested values with wrap; the leak into the neighbouring field is not reproduced";
/// Search profiles and direct-programming profiles at the code's edges,
/// which [`BOUNDED_CODE`] makes differ.
const BOUNDED_SEARCHES: &[&str] = &["lowest-initial-cap", "highest-initial-cap", "signed-wrap"];
const BOUNDED_PROGRAMS: &[&str] = &["high-byte"];

/// Each case's comparison verdict in `records`, in case order.
fn verdicts(records: &[ExecutionEvidence]) -> Vec<ComparisonVerdict> {
    records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Comparison { result, .. } => Some(result.verdict),
            _ => None,
        })
        .collect()
}

pub fn search_cases() -> Vec<Search> {
    let n = SEARCH_SAMPLES as usize;
    let down = |c: i32| (0..SEARCH_SAMPLES).map(move |i| c - i);
    let up = |c: i32| (1..=SEARCH_SAMPLES).map(move |i| c + i);
    let around = |c: i32| down(c).chain(up(c)).collect::<Vec<_>>();
    let mean = |v: Vec<i32>| v.iter().sum::<i32>() / v.len() as i32;
    vec![
        ("all-accepted", 100, vec![0; 2 * n], around(100), 100),
        ("lowest-initial-cap", 0, vec![0; 2 * n], around(0), 0),
        ("highest-initial-cap", 511, vec![0; 2 * n], around(511), 511),
        ("no-accepted", 100, vec![3; 2 * n], around(100), 100),
        (
            "positive",
            100,
            [vec![1; 2], vec![0; n]].concat(),
            [vec![100, 99], up(100).collect()].concat(),
            mean(up(100).collect()),
        ),
        (
            "negative",
            300,
            [vec![0; n], vec![2; 2]].concat(),
            [down(300).collect(), vec![301, 302]].concat(),
            mean(down(300).collect()),
        ),
        (
            "nonconsecutive",
            100,
            vec![1, 0, 1, 2, 3, 2],
            vec![100, 99, 98, 101, 102, 103],
            99,
        ),
        (
            "signed-wrap",
            0,
            vec![1, 0, 1, 2, 2],
            vec![0, -1, -2, 1, 2],
            -1,
        ),
        (
            "opposite-boundaries",
            100,
            [vec![2; n], vec![1; n]].concat(),
            around(100),
            100,
        ),
    ]
}

/// Expected command-port writes of one search.
pub fn expected_commands(candidates: &[i32], selected: i32) -> Vec<(u32, u32)> {
    let mut result = vec![0x0400_0562u32, 0x0400_0762, 0x0400_0b62, 0x0555_0b62];
    let mut high = 0x95u32;
    for (i, candidate) in candidates.iter().chain([&selected]).enumerate() {
        let programmed = (*candidate).max(0) as u32;
        high = ((high & !0x40) | ((programmed >> 8) << 6)) & 255;
        result.extend([
            0x0500_0162 | ((programmed & 255) << 16),
            0x0400_0262,
            0x0500_0262 | (high << 16),
        ]);
        if i < candidates.len() {
            result.push(0x0400_0c62);
        }
    }
    result.into_iter().map(|v| (I2C_PORT_1, v)).collect()
}

/// Frequency-control words the captured maintenance reads and writes.
const CONTROL: u32 = FREQUENCY_CONTROL;
const READ_CONTROL: u32 = 0x2010_0020;
const STATUS: u32 = CHANNEL_STATUS;
const MEMORY_DATA: u32 = 0x2010_002c;
const NUMBER: u32 = 0x2010_0030;
const READ_RESULT: u32 = 0x2010_0040;
/// SDM deadline counter sampled once per maintenance.
const SDM: u32 = 0x2010_d800;
/// Modeled frequency-control words; any other frequency access is INCOMPLETE.
const FREQUENCY_WORDS: [u32; 6] = [
    CONTROL,
    READ_CONTROL,
    STATUS,
    MEMORY_DATA,
    NUMBER,
    READ_RESULT,
];

/// Retained frequency-control words and the ordered transactions over them.
struct Frequency {
    regs: std::collections::BTreeMap<u32, u32>,
    result: Vec<(bool, u32, u32)>,
}

impl Frequency {
    fn get(&self, register: u32) -> u32 {
        self.regs[&register]
    }
    fn read(&mut self, register: u32) {
        let value = self.get(register);
        self.result.push((false, register, value));
    }
    fn write(&mut self, register: u32, value: u32) {
        self.regs.insert(register, value);
        self.result.push((true, register, value));
    }
    fn update(&mut self, register: u32, f: impl FnOnce(u32) -> u32) {
        let value = f(self.get(register));
        self.write(register, value);
    }
}

/// Instruction-derived retained-word transaction oracle. It does not feed
/// execution: the device receives only finite input words. Entries are
/// (write, address, value).
pub fn frequency_expectation(
    initial: u32,
    contents: Option<&[u32]>,
    delta: i32,
    channel: u32,
) -> Vec<(bool, u32, u32)> {
    let mut f = Frequency {
        regs: std::collections::BTreeMap::from([
            (CONTROL, 0x4128_0055),
            (READ_CONTROL, 0xa5a4_5678),
            (STATUS, initial),
            (NUMBER, 0x1234_5678),
        ]),
        result: vec![],
    };
    f.read(STATUS);
    f.write(STATUS, (initial & !3) | 2);
    f.result.push((false, SDM, 0x9876_5432));
    f.read(NUMBER);
    if let Some(contents) = contents {
        for (i, word) in contents.iter().enumerate() {
            let address = 0x20 + 7 * i as u32;
            f.read(CONTROL);
            f.update(CONTROL, |v| (v & !0x7ff00) | (address << 8));
            f.read(NUMBER);
            f.update(NUMBER, |v| (v & !3) | 2);
            f.read(READ_CONTROL);
            f.update(READ_CONTROL, |v| v | 0x10000);
            f.read(READ_CONTROL);
            f.update(READ_CONTROL, |v| v & !0x10000);
            f.result.push((false, READ_RESULT, *word));
            let corrected = (((word & 255) | ((word >> 6) & 0x100)) as i32 + delta) & 0xffff;
            let signed = i32::from(corrected as u16 as i16);
            let encoded = (corrected as u32 & 255)
                | (word & 0xbf00)
                | (((signed >> 8) as u32) << 14)
                | 0x0300_0000;
            f.read(CONTROL);
            f.update(CONTROL, |v| (v & !0x7ff00) | (address << 8));
            f.write(MEMORY_DATA, encoded);
            f.read(CONTROL);
            f.update(CONTROL, |v| v | 0x10_0000);
            f.read(CONTROL);
            f.update(CONTROL, |v| v & !0x10_0000);
        }
        let frequency = match channel {
            1 | 2412 => 2412,
            13 => 2472,
            14 | 2484 => 2484,
            other => panic!("unsupported channel {other}"),
        };
        f.read(CONTROL);
        f.update(CONTROL, |v| (v & !255) | ((frequency - 0x60) & 255));
    }
    f.read(STATUS);
    f.update(STATUS, |v| v & !3);
    f.result
}

/// Name, lock statuses, signed correction, initial control word, frequency
/// memory contents and channel of one maintenance case.
type Maintenance = (String, Vec<u32>, i32, u32, Option<Vec<u32>>, u32);

struct Rfpll {
    parameter: u32,
    memcpy: u32,
    rom_delay: u32,
    production_delay: u32,
    search_entry: u32,
    maintain_entry: u32,
    program_entry: u32,
    vendor_program: u32,
    vendor_search: u32,
    vendor_track: u32,
    callbacks: Vec<u32>,
}

/// One direct-programming profile: request, initial capacitor, lock samples
/// before lock (`None`: never), capacitor-status samples, and the selected
/// capacitor read independently from the pinned archive `phy_rfpll_cap_init_cal_new`.
struct Program {
    name: String,
    frequency: u32,
    crystal: u32,
    offset: u32,
    cap: i32,
    lock: Option<u32>,
    statuses: Vec<u32>,
    busy: u32,
    selected: u32,
}

fn program_cases() -> Vec<Program> {
    let profile = |name: &str, cap: i32, lock, statuses: Vec<u32>, busy, selected| Program {
        name: name.into(),
        frequency: 2412,
        crystal: 1,
        offset: 0,
        cap,
        lock,
        statuses,
        busy,
        selected,
    };
    let all = || vec![0; SEARCH_STATUSES];
    let mut cases = vec![
        profile("all-accepted", 100, Some(0), all(), 0, 100),
        profile("all-accepted-busy", 100, Some(0), all(), 1, 100),
        profile("late-lock", 100, Some(3), all(), 0, 100),
        profile("no-lock", 100, None, all(), 0, 100),
        // Decrease is a boundary only upward: the downward direction runs
        // to its bound, the upward one ends after two samples.
        profile(
            "no-accepted",
            100,
            Some(0),
            vec![2; SEARCH_SAMPLES as usize + 2],
            0,
            100,
        ),
        profile(
            "up-only",
            100,
            Some(0),
            [vec![1; 2], vec![0; 10], vec![2; 2]].concat(),
            0,
            105,
        ),
        profile(
            "down-only",
            100,
            Some(0),
            [vec![0; 10], vec![1; 2], vec![2; 2]].concat(),
            0,
            95,
        ),
        profile(
            "nonconsecutive-boundaries",
            100,
            Some(0),
            vec![1, 0, 1, 0, 2, 0, 2],
            0,
            101,
        ),
        profile("signed-wrap", 1, Some(0), vec![0, 0, 0, 1, 1, 2, 2], 0, 0),
        // Other statuses and the opposite direction's boundary status are
        // neither accepted nor counted: downward 3, then upward 1.
        profile(
            "non-boundary-statuses",
            100,
            Some(0),
            vec![3, 0, 1, 1, 1, 0, 2, 2],
            0,
            100,
        ),
        profile("high-byte", 511, Some(0), all(), 0, 511),
    ];
    for frequency in PROGRAM_FREQUENCIES {
        for crystal in PROGRAM_CRYSTALS {
            for offset in PROGRAM_OFFSETS {
                cases.push(Program {
                    name: format!("sdm-{frequency}-{crystal}-{offset}"),
                    frequency,
                    crystal,
                    offset,
                    ..profile("", 100, Some(0), all(), 0, 100)
                });
            }
        }
    }
    cases
}

fn bank(cap: i32, statuses: &[u32], busy: u32) -> Vec<DeviceDeclaration> {
    let cap = cap as u32;
    let mut cells: Vec<CommandCell> = [
        (1u32, cap & 255),
        (2, 0x95),
        (5, cap & 255),
        (7, 0xc2 | ((cap >> 8) << 2)),
        (11, 0x15),
    ]
    .map(|(r, v)| CommandCell {
        selector: (r << 8) | 0x62,
        initial: v,
        reads: None,
    })
    .to_vec();
    cells.push(CommandCell {
        selector: 0x0c62,
        initial: 0,
        reads: Some(statuses.iter().map(|s| 0xa3 | (s << 2)).collect()),
    });
    vec![
        analog_bank(
            "rfpll",
            "explicit capacitor bytes and finite lock-status samples; no search algorithm",
            busy,
            [0, 0],
            cells,
        ),
        DeviceDeclaration {
            id: "transport-controls".into(),
            applicability: "explicit retained host-map/read-mask controls".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: vec![
                    RegisterCell {
                        address: I2C_READ_MASK,
                        width: 4,
                        value: 0,
                    },
                    RegisterCell {
                        address: I2C_HOST_MAP,
                        width: 4,
                        value: 0,
                    },
                ],
            },
        },
    ]
}

fn sequence(address: u32, values: &[u32], name: &str) -> DeviceDeclaration {
    sequence_read(
        name,
        address,
        values.iter().map(|v| ReadRun::once(*v)).collect(),
    )
}

fn maintenance_models(
    statuses: &[u32],
    initial: u32,
    contents: Option<&[u32]>,
    busy: u32,
) -> Vec<DeviceDeclaration> {
    let mut result = bank(100, statuses, busy);
    let mut cells = vec![(STATUS, initial)];
    if let Some(contents) = contents {
        cells.extend([
            (FREQUENCY_CONTROL, 0x4128_0055),
            (0x2010_0020, 0xa5a4_5678),
            (0x2010_002c, 0),
            (0x2010_0030, 0x1234_5678),
        ]);
        result.push(sequence(0x2010_0040, contents, "frequency-memory"));
    } else {
        result.push(sequence(0x2010_0030, &[0x1234_5678], "i2c-number"));
    }
    cells.sort();
    result.push(DeviceDeclaration {
        id: "frequency-control".into(),
        applicability: "explicit retained frequency control words".into(),
        lifetime: RegionLifetime::Phase,
        behavior: DeviceBehavior::RegisterBank {
            cells: cells
                .into_iter()
                .map(|(address, value)| RegisterCell {
                    address,
                    width: 4,
                    value,
                })
                .collect(),
        },
    });
    result.push(sequence(0x2010_d800, &[0x9876_5432], "sdm"));
    result
}

impl Rfpll {
    fn enter(
        &self,
        target: u32,
        arguments: &[u32],
        models: Vec<DeviceDeclaration>,
        memory: Vec<ExecutionRegion>,
    ) -> Result<Invocation> {
        let mut regions = vec![
            known(ROM_INTERFACE_POINTER, 4, &words(&[CALLBACK_TABLE]))?,
            known(CALLBACK_TABLE, 16, &words(&self.callbacks))?,
        ];
        regions.extend(memory);
        Ok(direct(target, arguments, regions, models, vec![]))
    }

    fn delay(&self, side: bool) -> u32 {
        if side {
            self.production_delay
        } else {
            self.rom_delay
        }
    }

    fn search(&self, side: bool, cap: i32, statuses: &[u32], busy: u32) -> Result<Invocation> {
        let mut phase = self.enter(
            if side {
                self.search_entry
            } else {
                self.vendor_search
            },
            &[],
            bank(cap, statuses, busy),
            vec![],
        )?;
        phase.calls = delay_calls("requested-delay", self.delay(side));
        Ok(phase)
    }

    /// Direct synthesizer programming: archive `phy_set_rfpll_freq_new`
    /// writing its SDM image through a scratch buffer, or the production
    /// probe.
    fn program(&self, side: bool, program: &Program) -> Result<Invocation> {
        let mut models = bank(program.cap, &program.statuses, program.busy);
        if let Some(DeviceDeclaration {
            behavior: DeviceBehavior::CommandBank(bank),
            ..
        }) = models.first_mut()
        {
            // Lock samples, then the calibrated-capacitor high read, share
            // register 7 of block 0x62.
            let high = ((program.cap as u32) >> 8) << 2;
            let lock = |locked: bool| 0xc0 | (u32::from(locked) << 1) | high;
            let mut samples = match program.lock {
                Some(unlocked) => [vec![lock(false); unlocked as usize], vec![lock(true)]].concat(),
                None => vec![lock(false); LOCK_SAMPLES as usize],
            };
            samples.push(*samples.last().unwrap());
            for cell in &mut bank.cells {
                if cell.selector == 0x0762 {
                    cell.reads = Some(samples.clone());
                }
            }
            // SDM and calibration-restart registers of blocks 0x62 and 0x63.
            for selector in [0x0062u32, 0x0063, 0x0363, 0x0463, 0x0563, 0x0663] {
                bank.cells.push(CommandCell {
                    selector,
                    initial: 0,
                    reads: None,
                });
            }
            bank.cells.sort_by_key(|c| c.selector);
        }
        let (frequency, crystal, offset) = (program.frequency, program.crystal, program.offset);
        let mut phase = if side {
            self.enter(
                self.program_entry,
                &[frequency, crystal, offset],
                models,
                vec![],
            )?
        } else {
            self.enter(
                self.vendor_program,
                &[crystal, frequency, offset, SDM_BUFFER],
                models,
                vec![region(SDM_BUFFER, 8, &[0; 8], None, RegionLifetime::Phase)?],
            )?
        };
        phase.calls = delay_calls("requested-delay", self.delay(side));
        Ok(phase)
    }

    fn setup(&self, side: bool) -> Result<Invocation> {
        let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
        data[..2].copy_from_slice(&[100, 0]);
        self.setup_data(side, &data)
    }

    /// Copy `data` into the vendor `phy_param` or a production buffer.
    fn setup_data(&self, side: bool, data: &[u8]) -> Result<Invocation> {
        let target = if side {
            PARAMETER_DESTINATION
        } else {
            self.parameter
        };
        let mut memory = vec![known(PARAMETER_SOURCE, PHY_PARAM_BYTES, data)?];
        if side {
            memory.push(region(
                target,
                PHY_PARAM_BYTES,
                &[],
                None,
                RegionLifetime::Session,
            )?);
        }
        self.enter(
            self.memcpy,
            &[target, PARAMETER_SOURCE, PHY_PARAM_BYTES],
            vec![],
            memory,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn maintain(
        &self,
        side: bool,
        statuses: &[u32],
        initial: u32,
        contents: Option<&[u32]>,
        channel: u32,
        busy: u32,
        diagnostics: u32,
    ) -> Result<Invocation> {
        let mut table = vec![0u8; 284];
        table.extend((channel as u16).to_le_bytes());
        let memory = vec![
            known(ROM_PARAMETER_POINTER, 4, &words(&[0x3fff_0000]))?,
            known(0x3fff_0000, 286, &table)?,
        ];
        let (target, arguments) = if side {
            (self.maintain_entry, [channel])
        } else {
            (self.vendor_track, [diagnostics])
        };
        let mut phase = self.enter(
            target,
            &arguments,
            maintenance_models(statuses, initial, contents, busy),
            memory,
        )?;
        phase.calls = delay_calls("requested-delay", self.delay(side));
        Ok(phase)
    }
}

/// Port polling and the vendor's additional channel-status sample
/// immediately before a frequency-control read.
fn maintain_rules() -> Vec<EffectRule> {
    let mut rules = port_polling(MAX_EVENTS);
    rules.push(omitted_read_before(
        "channel-status-resample".into(),
        CHANNEL_STATUS,
        FREQUENCY_CONTROL,
        1,
        "the vendor samples channel status again immediately before reading frequency control",
    ));
    rules
}

fn in_frequency_domain(address: u32) -> bool {
    FREQUENCY_WORDS.contains(&address) || address == SDM
}

/// Direct-programming frequencies in MHz: both sides of the ROM 4000-MHz
/// divider split and the 2.4- and 5-GHz band edges.
const PROGRAM_FREQUENCIES: [u32; 6] = [2412, 2484, 4000, 4001, 5180, 5825];
/// Crystal selectors: the three table entries and the out-of-table default.
const PROGRAM_CRYSTALS: [u32; 5] = [0, 1, 2, 3, 4];
/// Frequency offsets, including the largest byte.
const PROGRAM_OFFSETS: [u32; 3] = [0, 7, 255];
/// Lock-status samples the ROM takes before giving up on calibration end.
const LOCK_SAMPLES: u32 = 100;
/// Scratch buffer the ROM programming path fills with its SDM image.
const SDM_BUFFER: u32 = 0x3fff_2000;

pub fn exercise(ctx: &mut I2c) -> Result<()> {
    let rfpll = Rfpll {
        parameter: ctx.parameter,
        memcpy: ctx.captured(1, "memcpy"),
        rom_delay: ctx.captured(1, "ets_delay_us"),
        production_delay: ctx.probe("open_phy_trace_delay_event"),
        search_entry: ctx.probe("open_phy_rfpll_trace_search"),
        maintain_entry: ctx.probe("open_phy_rfpll_trace_maintain"),
        program_entry: ctx.probe("open_phy_rfpll_trace_program"),
        vendor_program: ctx.root("phy_set_rfpll_freq_new"),
        vendor_search: ctx.root("phy_rfpll_cap_init_cal_track"),
        vendor_track: ctx.root("phy_rfpll_cap_track_new"),
        callbacks: [
            "phy_i2c_enter_critical",
            "phy_i2c_exit_critical",
            "phy_get_i2c_read_mask_new",
            "phy_get_i2c_hostid_new",
        ]
        .map(|n| ctx.root(n))
        .to_vec(),
    };
    let (vendor, replacement) = (ctx.vendor.clone(), ctx.replacement.clone());
    let search_effects = ctx.review_pair(
        "rfpll-search",
        ctx.root_endpoint("phy_rfpll_cap_init_cal_track")?,
        ctx.input_endpoint(2, "open_phy_rfpll_trace_search")?,
        port_polling(MAX_EVENTS),
        "RFPLL capacitor search under explicit capacitor, status and busy inputs",
    )?;
    let search_row =
        |label: String, cap: i32, statuses: &[u32], busy: u32| -> Result<ExecutionCase> {
            let mut row = case(
                label,
                rfpll.search(false, cap, statuses, busy)?,
                Some(rfpll.search(true, cap, statuses, busy)?),
                SessionReset::Cold,
                false,
            );
            let relation = row.relation.as_mut().unwrap();
            relation.returns.low = true;
            relation.effects = Some(search_effects.clone());
            Ok(row)
        };
    // The search profiles are two requests, the matching ones and those at
    // the code's edges; each case sets its own stack fill.
    let (mut rows, mut expectations) = ([vec![], vec![]], [vec![], vec![]]);
    for (name, cap, statuses, candidates, selected) in search_cases() {
        let bounded = usize::from(BOUNDED_SEARCHES.contains(&name));
        for fill in FILLS {
            for busy in [0u32, 1] {
                let label = format!("rfpll-search-{name}-{fill}-{busy}");
                let mut row = search_row(label.clone(), cap, &statuses, busy)?;
                row.stack_fill = Some(fill);
                rows[bounded].push(row);
                expectations[bounded].push((
                    label,
                    cap,
                    statuses.clone(),
                    candidates.clone(),
                    selected,
                ));
            }
        }
    }
    let [rows, bounded_rows] = rows;
    let [expectations, bounded_expectations] = expectations;
    let bounded = ctx.submit_with(
        "rfpll-search-bounded-code",
        &vendor,
        Some(&replacement),
        None,
        bounded_rows,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    assert!(
        verdicts(&bounded)
            .iter()
            .all(|v| *v == ComparisonVerdict::Diff),
        "every edge search differs: {BOUNDED_CODE}"
    );
    let records = ctx.submit_with(
        "rfpll-search",
        &vendor,
        Some(&replacement),
        None,
        rows,
        MAX_EVENTS,
        Some(ComparisonVerdict::Match),
    )?;
    // The vendor side of the edge searches still follows the oracle.
    let checked = expectations
        .iter()
        .enumerate()
        .flat_map(|(case, e)| [(&records, case, e, false), (&records, case, e, true)])
        .chain(
            bounded_expectations
                .iter()
                .enumerate()
                .map(|(case, e)| (&bounded, case, e, false)),
        );
    for (records, case, (label, cap, statuses, candidates, selected), side) in checked {
        let (case, cap, selected) = (case as u32, *cap, *selected);
        let low = returned_low(records, case, side);
        assert_eq!(low, Some((selected - cap) as u32), "{label} {side}");
        let observed = events(records, case, side);
        let commands: Vec<_> = observed
            .iter()
            .filter_map(|e| match e {
                ExecutionEvent::Write {
                    address: address @ (I2C_PORT_0 | I2C_PORT_1),
                    value,
                    ..
                } => Some((*address, *value)),
                _ => None,
            })
            .collect();
        assert_eq!(
            commands,
            expected_commands(candidates, selected),
            "{label} {side}"
        );
        let waits = delays_of(&observed);
        assert_eq!(waits, vec![5; statuses.len()], "{label} {side}");
        assert!(all_complete(records, case, side), "{label} {side}");
    }
    let program_effects = ctx.review_pair(
        "rfpll-program",
        ctx.root_endpoint("phy_set_rfpll_freq_new")?,
        ctx.input_endpoint(2, "open_phy_rfpll_trace_program")?,
        plumbing(&[], MAX_EVENTS),
        "direct RFPLL programming under explicit capacitor, lock and status inputs",
    )?;
    let (programs, bounded_programs): (Vec<_>, Vec<_>) = program_cases()
        .into_iter()
        .partition(|p| !BOUNDED_PROGRAMS.contains(&p.name.as_str()));
    let program_rows = |programs: &[Program]| -> Result<Vec<ExecutionCase>> {
        let mut rows = vec![];
        for program in programs {
            let mut row = case(
                format!("rfpll-program-{}", program.name),
                rfpll.program(false, program)?,
                Some(rfpll.program(true, program)?),
                SessionReset::Cold,
                false,
            );
            let relation = row.relation.as_mut().unwrap();
            relation.returns.low = true;
            relation.effects = Some(program_effects.clone());
            row.stack_fill = Some(FILLS[0]);
            rows.push(row);
        }
        Ok(rows)
    };
    let bounded = ctx.submit_with(
        "rfpll-program-bounded-code",
        &vendor,
        Some(&replacement),
        None,
        program_rows(&bounded_programs)?,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    assert!(
        verdicts(&bounded)
            .iter()
            .all(|v| *v == ComparisonVerdict::Diff),
        "every edge program differs: {:?} {:?}: {BOUNDED_CODE}",
        bounded_programs.iter().map(|p| &p.name).collect::<Vec<_>>(),
        verdicts(&bounded)
    );
    for (case, program) in bounded_programs.iter().enumerate() {
        let expected = ((program.cap as u32) << 16) | program.selected;
        assert_eq!(
            returned_low(&bounded, case as u32, false),
            Some(expected),
            "rfpll-program-{} vendor",
            program.name
        );
    }
    let rows = program_rows(&programs)?;
    let records = ctx.submit_with(
        "rfpll-program",
        &vendor,
        Some(&replacement),
        None,
        rows,
        MAX_EVENTS,
        Some(ComparisonVerdict::Match),
    )?;
    for (case, program) in programs.iter().enumerate() {
        let expected = ((program.cap as u32) << 16) | program.selected;
        for side in [false, true] {
            let label = format!("rfpll-program-{} {side}", program.name);
            assert_eq!(
                returned_low(&records, case as u32, side),
                Some(expected),
                "{label}"
            );
            assert!(all_complete(&records, case as u32, side), "{label}");
        }
    }
    let mut maintenance: Vec<Maintenance> = vec![];
    for status in [0u32, 3] {
        for initial in [0x2582_4e58u32, 0xa5a5_5a5b] {
            maintenance.push((
                format!("zero-{status}-{initial:x}"),
                vec![status; 2 * SEARCH_SAMPLES as usize],
                0,
                initial,
                None,
                13,
            ));
        }
    }
    // Ten accepted samples bounded by two boundary statuses in each direction.
    let positive = [vec![1; 2], vec![0; 10], vec![2; 2]].concat();
    let negative = [vec![0; 10], vec![1; 2], vec![2; 2]].concat();
    for (name, statuses, delta, boundary) in [
        ("positive", positive.clone(), 5, None),
        ("negative", negative.clone(), -5, None),
        ("underflow", negative, -5, Some(0x00aa_bf00u32)),
        ("overflow", positive, 5, Some(0x00aa_ffff)),
    ] {
        let contents: Vec<u32> = (0..85u32)
            .map(|i| boundary.unwrap_or(0x0055_0000 | (i << 8) | (100 + i)))
            .collect();
        for channel in [1, 13, 14, 2412, 2484] {
            maintenance.push((
                name.into(),
                statuses.clone(),
                delta,
                0x2582_4e58,
                Some(contents.clone()),
                channel,
            ));
        }
    }
    let maintain_effects = ctx.review_pair(
        "rfpll-maintain",
        ctx.root_endpoint("phy_rfpll_cap_track_new")?,
        ctx.input_endpoint(2, "open_phy_rfpll_trace_maintain")?,
        maintain_rules(),
        "RFPLL frequency maintenance under explicit status, frequency-memory and channel inputs",
    )?;
    // Every maintenance profile is one request; each sets its own stack fill.
    let (mut all, mut labels) = (vec![], vec![]);
    for (name, statuses, _, initial, contents, channel) in &maintenance {
        for fill in FILLS {
            let label = format!("rfpll-maintain-{name}-{channel}-{fill}");
            let mut rows = vec![
                case(
                    "initialize-parameters",
                    rfpll.setup(false)?,
                    Some(rfpll.setup(true)?),
                    SessionReset::Cold,
                    false,
                ),
                case(
                    label.clone(),
                    rfpll.maintain(
                        false,
                        statuses,
                        *initial,
                        contents.as_deref(),
                        *channel,
                        0,
                        0,
                    )?,
                    Some(rfpll.maintain(
                        true,
                        statuses,
                        *initial,
                        contents.as_deref(),
                        *channel,
                        0,
                        0,
                    )?),
                    SessionReset::Warm,
                    false,
                ),
            ];
            rows[1].relation.as_mut().unwrap().effects = Some(maintain_effects.clone());
            all.extend(with_stack_fill(rows, fill));
            labels.push(label);
        }
    }
    let records = ctx.submit_with(
        "rfpll-maintain",
        &vendor,
        Some(&replacement),
        None,
        all,
        MAX_EVENTS,
        Some(ComparisonVerdict::Match),
    )?;
    let profiles = maintenance
        .iter()
        .flat_map(|m| FILLS.map(|_| m))
        .zip(&labels)
        .enumerate();
    for (i, ((_, statuses, delta, initial, contents, channel), label)) in profiles {
        let measured = 2 * i as u32 + 1;
        for side in [false, true] {
            let low = returned_low(&records, measured, side);
            if side {
                assert_eq!(low, Some(*delta as u32), "{label}");
            }
            let observed = events(&records, measured, side);
            let mut expected_waits = vec![2];
            expected_waits.extend(vec![5; statuses.len()]);
            assert_eq!(delays_of(&observed), expected_waits, "{label}");
            let mut projected = observed.clone();
            if !side && contents.is_some() {
                let indices: Vec<_> = projected
                    .iter()
                    .enumerate()
                    .filter(|(_, e)| {
                        matches!(
                            e,
                            ExecutionEvent::Read {
                                address: CHANNEL_STATUS,
                                ..
                            }
                        )
                    })
                    .map(|(i, _)| i)
                    .collect();
                assert_eq!(indices.len(), 3, "{label}");
                let removed = projected.remove(indices[1]);
                assert!(
                    matches!(
                        removed,
                        ExecutionEvent::Read {
                            value: 0x2582_4e5a,
                            ..
                        }
                    ),
                    "{label}"
                );
            }
            let frequency: Vec<_> = projected
                .iter()
                .filter_map(|e| match e {
                    ExecutionEvent::Read { address, value, .. }
                        if in_frequency_domain(*address) =>
                    {
                        Some((false, *address, *value))
                    }
                    ExecutionEvent::Write { address, value, .. }
                        if in_frequency_domain(*address) =>
                    {
                        Some((true, *address, *value))
                    }
                    _ => None,
                })
                .collect();
            assert_eq!(
                frequency,
                frequency_expectation(*initial, contents.as_deref(), *delta, *channel),
                "{label} {side}"
            );
            assert!(all_complete(&records, measured, side), "{label} {side}");
        }
    }
    let label = "rfpll-maintenance-timeout";
    let mut failed = rfpll.maintain(true, &[], 0x2582_4e58, None, 13, 65535, 0)?;
    // No status sample is expected while the first command remains pending.
    if let DeviceBehavior::CommandBank(bank) = &mut failed.models[0].behavior {
        bank.cells.retain(|c| c.reads.is_none());
    }
    let mut row = case(label, failed, None, SessionReset::Cold, false);
    row.relation = None;
    let records = ctx.submit_with(
        label,
        &replacement,
        None,
        Some(0x5a),
        vec![row],
        MAX_EVENTS,
        None,
    )?;
    assert_eq!(returned_low(&records, 0, false), Some(0x8000_0000));
    let frequency_writes: Vec<_> = events(&records, 0, false)
        .iter()
        .filter_map(|e| match e {
            ExecutionEvent::Write { address, value, .. } if FREQUENCY_WORDS.contains(address) => {
                Some((*address, *value))
            }
            _ => None,
        })
        .collect();
    assert_eq!(frequency_writes, [(CHANNEL_STATUS, 0x2582_4e5a)]);
    let pending = models(&records, 0, false)
        .into_iter()
        .find(|m| m.id == "rfpll")
        .expect("rfpll model");
    assert!(
        pending.commands.is_some_and(|c| c.pending == 1)
            && pending.status == ModelStatus::Incomplete
    );
    let original: Vec<ExecutionCase> = {
        let mut row = case(
            "rfpll-search",
            rfpll.search(false, 100, &[0; SEARCH_STATUSES], 0)?,
            Some(rfpll.search(true, 100, &[0; SEARCH_STATUSES], 0)?),
            SessionReset::Cold,
            false,
        );
        let relation = row.relation.as_mut().unwrap();
        relation.returns.low = true;
        relation.effects = Some(search_effects.clone());
        vec![row]
    };
    let mut changed = original.clone();
    if let DeviceBehavior::CommandBank(bank) =
        &mut changed[0].replacement.as_mut().unwrap().models[0].behavior
    {
        bank.cells
            .iter_mut()
            .find(|c| c.selector == 0x0562)
            .unwrap()
            .initial = 101;
    }
    ctx.submit_with(
        "rfpll-changed-capacitor",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        changed,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    let mut unknown = original.clone();
    unknown[0]
        .vendor
        .memory
        .iter_mut()
        .find(|r| r.seed.address == ROM_INTERFACE_POINTER)
        .expect("interface pointer region")
        .seed
        .bytes
        .clear();
    let records = ctx.submit_with(
        "rfpll-unknown-callbacks",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        unknown,
        MAX_EVENTS,
        Some(ComparisonVerdict::Incomplete),
    )?;
    // The unknown interface pointer loads an unknown callback, which is never
    // called.
    assert!(matches!(
        stop(&records, 0, false),
        ExecutionStop::Incomplete {
            reason: ExecutionGap::UnknownRegister { .. },
            ..
        }
    ));
    let mut diagnostics = vec![
        case(
            "initialize-parameters",
            rfpll.setup(false)?,
            Some(rfpll.setup(true)?),
            SessionReset::Cold,
            false,
        ),
        case(
            "diagnostics-unmapped",
            rfpll.maintain(false, &[0; SEARCH_STATUSES], 0x2582_4e58, None, 13, 0, 1)?,
            Some(rfpll.maintain(true, &[0; SEARCH_STATUSES], 0x2582_4e58, None, 13, 0, 0)?),
            SessionReset::Warm,
            false,
        ),
    ];
    diagnostics[1].relation.as_mut().unwrap().effects = Some(maintain_effects.clone());
    let records = ctx.submit_with(
        "rfpll-diagnostics-unmapped",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        diagnostics,
        MAX_EVENTS,
        Some(ComparisonVerdict::Incomplete),
    )?;
    let printf = ctx.captured(4, "phy_printf");
    assert!(matches!(
        stop(&records, 1, false),
        ExecutionStop::Incomplete { reason: ExecutionGap::Memory { address, access: MemoryAccess::Fetch }, .. } if address == printf
    ));
    let mut raw = original.clone();
    raw[0].relation.as_mut().unwrap().effects = None;
    ctx.submit_with(
        "rfpll-raw-polling",
        &vendor,
        Some(&replacement),
        Some(0x5a),
        raw,
        MAX_EVENTS,
        Some(ComparisonVerdict::Diff),
    )?;
    let mut original = original;
    // The contract's occurrence bounds exceed a one-event capacity; the
    // exhaustion under test precedes any effect comparison.
    original[0].relation.as_mut().unwrap().effects = None;
    let limited = crate::session::request(&vendor, Some(&replacement), Some(0x5a), original, 1);
    ctx.capacity_failure("rfpll-capacity", &limited)?;
    thermal(ctx, &rfpll)
}

/// `phy_param` offsets of the thermal child: current temperature, the RFPLL
/// enable byte, reference temperature, busy flag, override flags and
/// threshold, and the tracking result flags.
const CURRENT_TEMPERATURE: usize = 0;
const RFPLL_ENABLED: usize = 9;
const REFERENCE_TEMPERATURE: u32 = 304;
const TRACKING_BUSY: u32 = 404;
const OVERRIDE: usize = 432;
const TRACKING_FLAGS: u32 = 0x1e6;
/// Result flags before tracking and after an executed correction.
const FLAGS_IDLE: u16 = 0xa004;
const FLAGS_PERFORMED: u16 = 0xa005;
/// Stack and retained-register fill of the thermal cases.
const THERMAL_FILL: u8 = FILLS[1];
/// Lock statuses of an executed thermal correction.
const THERMAL_STATUSES: [u32; SEARCH_STATUSES] = [3; SEARCH_STATUSES];
/// Channel the production child receives; the vendor reads no channel table
/// without frequency-memory contents.
const THERMAL_CHANNEL: u32 = 13;
/// The reference temperature and result flags the thermal child commits, in
/// production output order.
const THERMAL_COMMITTED: [OutputField; 2] = [
    OutputField {
        name: "reference-temperature",
        parameter: REFERENCE_TEMPERATURE,
        output: 0,
        width: 2,
        count: 1,
    },
    OutputField {
        name: "tracking-flags",
        parameter: TRACKING_FLAGS,
        output: 2,
        width: 2,
        count: 1,
    },
];
/// Production thermal output bytes.
const THERMAL_OUTPUT_BYTES: u32 = 4;
/// Untouched production output bytes.
const THERMAL_OUTPUT_FILL: u8 = 0xa5;

/// One thermal child case: current and reference temperature, override flags
/// and threshold, busy flag, and whether the hardware correction executes.
/// Expected outcomes are explicit cases, not a shadow threshold policy.
struct Thermal {
    name: &'static str,
    current: i16,
    reference: i16,
    flags: u8,
    threshold: u8,
    busy: u8,
    executes: bool,
}

const fn thermal_case(
    name: &'static str,
    current: i16,
    reference: i16,
    flags: u8,
    threshold: u8,
    busy: u8,
    executes: bool,
) -> Thermal {
    Thermal {
        name,
        current,
        reference,
        flags,
        threshold,
        busy,
        executes,
    }
}

const THERMAL: [Thermal; 12] = [
    thermal_case("below-default", 114, 100, 0, 0, 0, false),
    thermal_case("exact-default", 115, 100, 0, 0, 0, true),
    thermal_case("cooling-below", 86, 100, 0, 0, 0, false),
    thermal_case("cooling-exact", 85, 100, 0, 0, 0, true),
    thermal_case("busy", 200, 100, 0, 0, 1, false),
    thermal_case("override-below", 119, 100, 1, 20, 0, false),
    thermal_case("override-exact", 120, 100, 1, 20, 0, true),
    thermal_case("calibration-flag-only", 115, 100, 2, 255, 0, true),
    thermal_case("zero-override", 100, 100, 1, 0, 0, true),
    thermal_case("zero-override-busy", 100, 100, 1, 0, 1, false),
    thermal_case("negative-temperatures", -85, -100, 0, 0, 0, true),
    thermal_case("full-signed-span", i16::MAX, i16::MIN, 0, 0, 0, true),
];

impl Thermal {
    fn parameters(&self) -> Vec<u8> {
        let mut data = vec![0u8; PHY_PARAM_BYTES as usize];
        data[CURRENT_TEMPERATURE..][..2].copy_from_slice(&self.current.to_le_bytes());
        data[RFPLL_ENABLED] = 0;
        data[REFERENCE_TEMPERATURE as usize..][..2].copy_from_slice(&self.reference.to_le_bytes());
        data[TRACKING_BUSY as usize] = self.busy;
        data[OVERRIDE..][..2].copy_from_slice(&[self.flags, self.threshold]);
        data[TRACKING_FLAGS as usize..][..2].copy_from_slice(&FLAGS_IDLE.to_le_bytes());
        data
    }
    fn models(&self) -> Vec<DeviceDeclaration> {
        if self.executes {
            let mut models = maintenance_models(&THERMAL_STATUSES, 0x2582_4e58, None, 0);
            // Every other radio register is retained storage starting with the fill.
            models.push(radio_aperture(THERMAL_FILL));
            models
        } else {
            vec![]
        }
    }
    /// Committed reference temperature: the current one after a correction.
    fn reference_after(&self) -> i16 {
        if self.executes {
            self.current
        } else {
            self.reference
        }
    }
}

/// The vendor thermal child with its guards against the compiled production
/// child, which runs only after its own physical admission: busy cases are
/// vendor characterization. Each side's commit is checked against the
/// explicit case; effects compare under the reviewed maintenance contract.
fn thermal(ctx: &mut I2c, rfpll: &Rfpll) -> Result<()> {
    let applicability =
        "RFPLL thermal tracking under explicit temperatures, overrides and lock statuses";
    let (vendor_entry, production_entry) = (
        ctx.root_endpoint("phy_rfpll_cap_track_new")?,
        ctx.input_endpoint(2, "open_phy_rfpll_trace_track")?,
    );
    let committed = ctx.review_projection(
        "rfpll-thermal-committed",
        "esp32s31.phy.rfpll-thermal.committed",
        output_projection(
            vendor_entry.clone(),
            rfpll.parameter,
            production_entry.clone(),
            THERMAL_OUTPUT_BYTES,
            &THERMAL_COMMITTED,
            applicability,
        ),
        "production publishes the reference temperature and progress the child commits",
    )?;
    let effects = ctx.review_pair(
        "rfpll-thermal",
        vendor_entry,
        production_entry,
        maintain_rules(),
        applicability,
    )?;
    let track = ctx.probe("open_phy_rfpll_trace_track");
    let table = |memory: &mut Vec<ExecutionRegion>| -> Result<()> {
        let mut table = vec![0u8; 284];
        table.extend((THERMAL_CHANNEL as u16).to_le_bytes());
        memory.push(known(ROM_PARAMETER_POINTER, 4, &words(&[0x3fff_0000]))?);
        memory.push(known(0x3fff_0000, 286, &table)?);
        Ok(())
    };
    let vendor_phase = |case: &Thermal| -> Result<Invocation> {
        let mut memory = vec![];
        table(&mut memory)?;
        let mut phase = rfpll.enter(rfpll.vendor_track, &[0], case.models(), memory)?;
        phase.observe_memory = vec![selection(rfpll.parameter, PHY_PARAM_BYTES)];
        phase.observe_timeline.writes = true;
        phase.calls = delay_calls("requested-delay", rfpll.delay(false));
        Ok(phase)
    };
    let (mut compared, mut characterized) = (vec![], vec![]);
    let (mut compared_cases, mut characterized_cases) = (vec![], vec![]);
    for case in &THERMAL {
        let setup = rfpll.setup_data(false, &case.parameters())?;
        if case.busy == 0 {
            let override_word = if case.flags & 1 != 0 {
                u32::from(case.threshold)
            } else {
                u32::MAX
            };
            let mut production = direct(
                track,
                &[
                    0,
                    case.current as i32 as u32,
                    case.reference as i32 as u32,
                    override_word,
                    THERMAL_CHANNEL,
                    u32::from(FLAGS_IDLE),
                    OUTPUT,
                ],
                vec![region(
                    OUTPUT,
                    THERMAL_OUTPUT_BYTES,
                    &[THERMAL_OUTPUT_FILL; THERMAL_OUTPUT_BYTES as usize],
                    None,
                    RegionLifetime::Phase,
                )?],
                case.models(),
                vec![selection(OUTPUT, THERMAL_OUTPUT_BYTES)],
            );
            production.calls = delay_calls("requested-delay", rfpll.delay(true));
            let mut row = case_row(case.name, vendor_phase(case)?, Some(production));
            let relation = row.relation.as_mut().unwrap();
            relation.effects = Some(effects.clone());
            relation.projection = Some(committed.clone());
            compared.extend([
                crate::harness::case(
                    "initialize-parameters",
                    setup,
                    Some(rfpll.setup_data(true, &case.parameters())?),
                    SessionReset::Cold,
                    false,
                ),
                row,
            ]);
            compared_cases.push(case);
        } else {
            characterized.extend([
                crate::harness::case(
                    "initialize-parameters",
                    setup,
                    None,
                    SessionReset::Cold,
                    false,
                ),
                case_row(case.name, vendor_phase(case)?, None),
            ]);
            characterized_cases.push(case);
        }
    }
    let (vendor, replacement) = (ctx.vendor.clone(), ctx.replacement.clone());
    let records = ctx.submit_with(
        "rfpll-thermal",
        &vendor,
        Some(&replacement),
        Some(THERMAL_FILL),
        compared,
        MAX_EVENTS,
        Some(ComparisonVerdict::Match),
    )?;
    for (i, case) in compared_cases.iter().enumerate() {
        let child = 2 * i as u32 + 1;
        check_thermal_vendor(case, rfpll.parameter, &records, child);
        assert!(all_complete(&records, child, true), "{}", case.name);
        assert_eq!(
            returned_low(&records, child, true),
            Some(u32::from(case.reference_after() as u16) | (u32::from(case.executes) << 16)),
            "{}: production outcome",
            case.name
        );
    }
    let mut rows = characterized;
    for row in &mut rows {
        row.relation = None;
    }
    let records = ctx.submit_with(
        "rfpll-thermal-busy",
        &vendor,
        None,
        Some(THERMAL_FILL),
        rows,
        MAX_EVENTS,
        None,
    )?;
    for (i, case) in characterized_cases.iter().enumerate() {
        check_thermal_vendor(case, rfpll.parameter, &records, 2 * i as u32 + 1);
    }
    Ok(())
}

fn case_row(name: &str, vendor: Invocation, production: Option<Invocation>) -> ExecutionCase {
    crate::harness::case(name, vendor, production, SessionReset::Warm, false)
}

/// The vendor's guard decision, commit and ordering: reference, then result
/// flags, then hardware frequency-control restoration, then busy release.
fn check_thermal_vendor(case: &Thermal, parameter: u32, records: &[ExecutionEvidence], child: u32) {
    let name = case.name;
    assert!(all_complete(records, child, false), "{name}");
    let observed = events(records, child, false);
    let mmio = observed.iter().any(|e| {
        matches!(
            e,
            ExecutionEvent::Read { .. } | ExecutionEvent::Write { .. }
        )
    });
    assert_eq!(mmio, case.executes, "{name}: hardware admission");
    let state = crate::evidence::output(records, child, false);
    let word =
        |offset: u32| u16::from_le_bytes([state[offset as usize], state[offset as usize + 1]]);
    assert_eq!(
        word(REFERENCE_TEMPERATURE) as i16,
        case.reference_after(),
        "{name}: reference"
    );
    assert_eq!(
        word(TRACKING_FLAGS),
        if case.executes {
            FLAGS_PERFORMED
        } else {
            FLAGS_IDLE
        },
        "{name}: flags"
    );
    assert_eq!(
        state[TRACKING_BUSY as usize], case.busy,
        "{name}: busy state"
    );
    if !case.executes {
        return;
    }
    let ram_write = |offset: u32, value: Option<u32>| {
        observed.iter().position(|e| {
            matches!(e, ExecutionEvent::Memory {
                transaction: MemoryTransaction::Write { address, value: v, .. }, ..
            } if *address == parameter + offset && value.is_none_or(|value| value == *v))
        })
    };
    let reference = ram_write(REFERENCE_TEMPERATURE, None).expect("reference publication");
    let flags = ram_write(TRACKING_FLAGS, None).expect("result publication");
    let restore = observed
        .iter()
        .rposition(|e| {
            matches!(
                e,
                ExecutionEvent::Write {
                    address: STATUS,
                    ..
                }
            )
        })
        .expect("hardware control restoration");
    let release = ram_write(TRACKING_BUSY, Some(0)).expect("busy release");
    assert!(
        reference < flags && flags < restore && restore < release,
        "{name}: vendor publication and restoration order"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_oracle_programs_every_candidate_then_the_selection() {
        let commands = expected_commands(&[100, 99], 99);
        // Four setup commands, then three per candidate plus a status read, then the selection.
        assert_eq!(commands.len(), 4 + 2 * 4 + 3);
        assert_eq!(commands[4], (I2C_PORT_1, 0x0500_0162 | (100 << 16)));
        // Negative candidates program zero.
        assert_eq!(expected_commands(&[-2], 0)[4], (I2C_PORT_1, 0x0500_0162));
        let cases = search_cases();
        assert_eq!(cases.len(), 9);
        assert_eq!(cases[5].3.len(), SEARCH_SAMPLES as usize + 2);
        assert_eq!(&cases[5].3[..2], &[300, 299]);
        assert_eq!(cases[5].4, 285);
    }

    #[test]
    fn frequency_oracle_encodes_signed_corrections() {
        let zero = frequency_expectation(0x2582_4e58, None, 0, 13);
        assert_eq!(zero.len(), 6);
        assert_eq!(zero[1], (true, CHANNEL_STATUS, 0x2582_4e5a));
        let underflow = frequency_expectation(0x2582_4e58, Some(&[0x00aa_bf00]), -5, 1);
        let encoded = underflow
            .iter()
            .find(|(write, address, _)| *write && *address == 0x2010_002c)
            .unwrap()
            .2;
        // Word bit 14 would be the ninth capacitor bit; here it is clear, so
        // 0 - 5 wraps to 0xfffb: low byte 0xfb, retained 0xbf00 and the sign field.
        assert_eq!(encoded, 0xffff_fffb);
        let ninth = frequency_expectation(0x2582_4e58, Some(&[0x0000_4000]), -5, 1);
        let encoded = ninth
            .iter()
            .find(|(write, address, _)| *write && *address == 0x2010_002c)
            .unwrap()
            .2;
        // 0x100 - 5 = 0xfb stays positive: no sign field.
        assert_eq!(encoded, 0x0300_00fb);
    }
}
