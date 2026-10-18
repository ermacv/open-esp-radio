//! Bluetooth LE controller leaves of the pinned `esp32s31-bt-lib` and
//! `libbtbb.a` archives, and the Bluetooth low-power clock selection of the
//! pinned ESP-IDF modem clock driver (`libesp_hw_support.a` over
//! `libhal.a`), compared with the compiled production Bluetooth probes on the
//! `wifi-mac` leaf machinery.
//!
//! The controller archives name their functions by obfuscated symbols that
//! change between releases; each leaf names the symbol of the pinned
//! release.
use crate::harness::Result;
use crate::mac::{
    Domain, Leaf, Objects, Replacement, Suite, Vendor, in_archive, leaf, objects, ordered, prefix,
    quiet, released, replaced, ruled, stated, tail_prefix,
};
use blobray_domain::ReadRun;

/// Session inputs of the second to fourth suite archives.
const BTDM_COMMON_INPUT: u64 = 4;
const BTBB_INPUT: u64 = 5;
const HW_SUPPORT_INPUT: u64 = 6;

/// `bt_bb_v2_init_cmplx` prints its version for a nonzero argument; the
/// claim covers argument one.
const PRINT_VERSION: &[u32] = &[1];
/// The `phy_param` byte at 0x120 the baseband reads: zero, a pattern and
/// all set.
const GAIN_PARAMETERS: &[u32] = &[0, 0x5a, 0xff];
const PHY_PARAM: &str = "phy_param";
const PHY_PARAM_GAIN: u32 = 0x120;

/// Memory-list selectors the production type admits.
const MEMORY_LIST_SELECTORS: &[u32] = &[1, 2, 3];
/// Memory-list pointers: none, and the first, an inner and the last word of
/// the encodable controller-SRAM window.
const MEMORY_LIST_POINTERS: &[u32] = &[0, 0x2f00_0000, 0x2f12_3454, 0x2f3f_fffc];

/// The scheduler's environment pointer cell in `libbtdm_common.a`, the
/// environment it points to, and the eight-byte argument it copies.
const SCHEDULER_ENVIRONMENT_CELL: &str = "r_sym_bt_YnZmsxu068uuC3PC4iu8";
const SCHEDULER_ENVIRONMENT: u32 = 0x3fff_3000;
const SCHEDULER_ENVIRONMENT_BYTES: usize = 0x20;
const SCHEDULER_ARGUMENT: u32 = 0x3fff_3100;
const SCHEDULER_ARGUMENT_BYTES: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];

/// The scheduler leaf's environment and argument: the pointer cell in image
/// data names the environment, which the vendor clears and fills from the
/// argument after its hardware list-head edges.
fn scheduler_abi(_words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    Ok(Objects {
        vendor_words: vec![SCHEDULER_ARGUMENT],
        vendor: vec![
            (SCHEDULER_ENVIRONMENT, vec![0; SCHEDULER_ENVIRONMENT_BYTES]),
            (SCHEDULER_ARGUMENT, SCHEDULER_ARGUMENT_BYTES.to_vec()),
        ],
        image: vec![(
            vendor.symbol(SCHEDULER_ENVIRONMENT_CELL)?,
            SCHEDULER_ENVIRONMENT.to_le_bytes().to_vec(),
        )],
        ..Default::default()
    })
}

/// The scheduler diagnostic value register both sides sample in pairs.
const DIAGNOSTIC_VALUE: u32 = 0x2010_11ec;
/// Sample states: the radio fill as a stable value, then a first pair that
/// disagrees and settles on all bits set or on zero.
const DIAGNOSTIC_STABLE: u32 = 0;
const DIAGNOSTIC_SETTLES_SET: u32 = 1;
const DIAGNOSTIC_SETTLES_CLEAR: u32 = 2;
const DIAGNOSTIC_SAMPLE_STATES: &[u32] = &[
    DIAGNOSTIC_STABLE,
    DIAGNOSTIC_SETTLES_SET,
    DIAGNOSTIC_SETTLES_CLEAR,
];
/// Reads of a sample whose first pair disagrees: that pair, then an
/// agreeing pair.
const SETTLING_READS_AFTER_FIRST: u32 = 3;

/// The diagnostic value a sample state serves: none beyond the fill when
/// stable; otherwise one differing read, then exactly the settled reads, so
/// a side reading more or fewer than the two pairs fails the case.
fn diagnostic_sample_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let [state] = words else {
        unreachable!("diagnostic sample: its state only")
    };
    let settled = match *state {
        DIAGNOSTIC_STABLE => return Ok(Objects::default()),
        DIAGNOSTIC_SETTLES_SET => u32::MAX,
        DIAGNOSTIC_SETTLES_CLEAR => 0,
        _ => unreachable!("declared diagnostic sample state"),
    };
    Ok(Objects {
        sequences: vec![(
            DIAGNOSTIC_VALUE,
            vec![
                ReadRun::once(!settled),
                ReadRun {
                    value: settled,
                    count: SETTLING_READS_AFTER_FIRST,
                },
            ],
        )],
        ..Default::default()
    })
}

/// The scheduler diagnostic selector the samples write, and the command
/// and status words of the two scheduler command engines.
const DIAGNOSTIC_SELECTOR: u32 = 0x2010_11e8;
/// Diagnostic selectors: scheduler BUSY, execution-modify progress and
/// execution-modify settle.
const SELECT_BUSY: u32 = 0x38;
const SELECT_PROGRESS: u32 = 0xc01;
const SELECT_SETTLE: u32 = 0x7;
/// Sample values: idle (BUSY bit seven clear), busy, a progress lane zero of two
/// (a conflicting state the vendor repeats on), and a settle of nine (not
/// yet settled).
const IDLE: u32 = 0;
const BUSY: u32 = 0x80;
const PROGRESS_REPEAT: u32 = 2;
/// Progress lane zero of three repeats only when lane one's bits five and
/// four read one.
const PROGRESS_LANE_ONE_REPEAT: u32 = 0x1003;
const PROGRESS_LANE_ONE_PROCEEDS: u32 = 0x0003;
const SETTLE_PENDING: u32 = 9;
/// Reads of one agreeing sample pair.
const PAIR: u32 = 2;

/// Hardware lists and list-deletion flags the execution modify admits.
const HARDWARE_LISTS: &[u32] = &[0, 7, 15];
const LIST_DELETION: &[u32] = &[0, 1];
/// Execution-modify states: a direct pass, a first progress sample the
/// vendor repeats on (then a second attempt), a first settle sample of
/// nine, and a progress lane zero of three that lane one makes repeat or
/// proceed.
const MODIFY_DIRECT: u32 = 0;
const MODIFY_PROGRESS_REPEATS: u32 = 1;
const MODIFY_SETTLE_WAITS: u32 = 2;
const MODIFY_LANE_ONE_REPEATS: u32 = 3;
const MODIFY_LANE_ONE_PROCEEDS: u32 = 4;
const MODIFY_STATES: &[u32] = &[
    MODIFY_DIRECT,
    MODIFY_PROGRESS_REPEATS,
    MODIFY_SETTLE_WAITS,
    MODIFY_LANE_ONE_REPEATS,
    MODIFY_LANE_ONE_PROCEEDS,
];

/// The multiplexed diagnostic port answering `sequences` of agreeing
/// sample pairs per selector.
fn diagnostic_port(sequences: &[(u32, &[u32])]) -> blobray_domain::DeviceDeclaration {
    blobray_domain::DeviceDeclaration {
        id: "scheduler-diagnostic".into(),
        applicability: "the scheduler diagnostic multiplexer: agreeing pairs per selector".into(),
        lifetime: blobray_domain::RegionLifetime::Phase,
        behavior: blobray_domain::DeviceBehavior::SelectedSequence {
            selector_address: DIAGNOSTIC_SELECTOR,
            value_address: DIAGNOSTIC_VALUE,
            width: 4,
            sequences: sequences
                .iter()
                .map(|(selector, samples)| blobray_domain::SelectedRuns {
                    selector: *selector,
                    runs: samples
                        .iter()
                        .map(|value| ReadRun {
                            value: *value,
                            count: PAIR,
                        })
                        .collect(),
                })
                .collect(),
        },
    }
}

/// `(index, list_deletion, state)`, the vendor taking the list's one-hot
/// mask: the samples of every attempt. Each
/// attempt samples BUSY in its preamble and its completion, progress until
/// it does not repeat, and settle until it leaves nine.
fn execution_modify_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let [index, deletion, state] = *words else {
        unreachable!("execution modify: index, deletion and state")
    };
    let port = match state {
        MODIFY_DIRECT => diagnostic_port(&[
            (SELECT_BUSY, &[IDLE, IDLE]),
            (SELECT_PROGRESS, &[IDLE]),
            (SELECT_SETTLE, &[IDLE]),
        ]),
        MODIFY_PROGRESS_REPEATS => diagnostic_port(&[
            (SELECT_BUSY, &[IDLE, IDLE, IDLE, IDLE]),
            (SELECT_PROGRESS, &[PROGRESS_REPEAT, IDLE, IDLE]),
            (SELECT_SETTLE, &[IDLE, IDLE]),
        ]),
        MODIFY_LANE_ONE_REPEATS => diagnostic_port(&[
            (SELECT_BUSY, &[IDLE, IDLE, IDLE, IDLE]),
            (SELECT_PROGRESS, &[PROGRESS_LANE_ONE_REPEAT, IDLE, IDLE]),
            (SELECT_SETTLE, &[IDLE, IDLE]),
        ]),
        MODIFY_LANE_ONE_PROCEEDS => diagnostic_port(&[
            (SELECT_BUSY, &[IDLE, IDLE]),
            (SELECT_PROGRESS, &[PROGRESS_LANE_ONE_PROCEEDS]),
            (SELECT_SETTLE, &[IDLE]),
        ]),
        MODIFY_SETTLE_WAITS => diagnostic_port(&[
            (SELECT_BUSY, &[IDLE, IDLE]),
            (SELECT_PROGRESS, &[IDLE]),
            (SELECT_SETTLE, &[SETTLE_PENDING, IDLE]),
        ]),
        _ => unreachable!("declared execution-modify state"),
    };
    // Every vendor caller passes the list as its one-hot mask.
    Ok(Objects {
        vendor_words: vec![1 << index, deletion],
        devices: vec![port],
        ..Default::default()
    })
}

/// Selector writes are not paired one to one: production reselects its
/// diagnostic signal at the start of every step it resumes after
/// `Pending`, since the runtime's BUSY observations between steps select
/// BUSY on the same selector, while the vendor holds one critical section
/// across its polling loop and selects once. The multiplexed diagnostic
/// model checks the selections instead: an undeclared selector is an issue,
/// and every sample must read exactly the runs declared for the selector in
/// force, so each side selects the right signal before each sample.
fn scheduler_rules() -> Vec<blobray_domain::EffectRule> {
    vec![
        crate::contracts::ignored(
            "diagnostic-selections".into(),
            blobray_domain::EffectPattern {
                selector: blobray_domain::EffectSelector::MmioWrite {
                    address: DIAGNOSTIC_SELECTOR,
                    width: 4,
                },
                value: blobray_domain::EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            },
            MAX_SELECTIONS,
            "the finite-step execution modify and lock reselect their diagnostic signal after every \
            Pending because interleaved BUSY observations select 0x38; the vendor holds a \
            critical section across the polling loop; the diagnostic model checks every \
            selection against the sample that follows",
        ),
        blobray_domain::EffectRule {
            name: "publication-ordering-fences".into(),
            vendor: None,
            replacement: Some(blobray_domain::EffectPattern {
                selector: blobray_domain::EffectSelector::Fence {
                    predecessor: FULL_FENCE,
                    successor: FULL_FENCE,
                },
                value: blobray_domain::EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            }),
            disposition: blobray_domain::EffectDisposition::Added,
            min_occurrences: 1,
            max_occurrences: MAX_ORDERING_FENCES,
            reason: "production orders each command publication and START clear against the \
            following device accesses; the vendor leaves ordering to its caller"
                .into(),
        },
    ]
}
/// The BUSY selections production repeats between diagnostic reads. The first
/// selection, and every read, still compare exactly; another selector value
/// or a read without a selection stays a difference.
fn busy_reselection_rules() -> Vec<blobray_domain::EffectRule> {
    let read = blobray_domain::EffectSelector::MmioRead {
        address: DIAGNOSTIC_VALUE,
        width: 4,
    };
    vec![blobray_domain::EffectRule {
        name: "busy-reselections".into(),
        vendor: None,
        replacement: Some(blobray_domain::EffectPattern {
            selector: blobray_domain::EffectSelector::MmioWrite {
                address: DIAGNOSTIC_SELECTOR,
                width: 4,
            },
            value: blobray_domain::EffectValue::Exact { value: SELECT_BUSY },
            preceded_by: Some(read),
            occurrence: None,
            followed_by: Some(read),
        }),
        disposition: blobray_domain::EffectDisposition::Added,
        min_occurrences: 0,
        max_occurrences: MAX_SELECTIONS,
        reason: "the HAL repeats the PAC's single select-plus-two-read diagnostic transaction \
            within its attempt budget (b0025708f), so each attempt re-publishes the identical \
            scheduler-status selector; the vendor selects once before its unbounded read loop"
            .into(),
    }]
}
/// Full-fence predecessor and successor sets, and the fences of one case's
/// at most two attempts, each a publication and a START clear.
const FULL_FENCE: u8 = 0xf;
const MAX_ORDERING_FENCES: u32 = 4;
/// Selector writes one execution-modify or execution-lock case may perform
/// per side.
const MAX_SELECTIONS: u32 = 16;

/// Execution-lock addresses (the first and last word of the encodable
/// controller-SRAM window).
const LOCK_ADDRESSES: &[u32] = &[0x2f00_0000, 0x2f3f_fffc];
/// Execution-lock states: the scheduler idle, and busy for one sample after
/// the publication.
const LOCK_IDLE: u32 = 0;
const LOCK_BUSY_ONCE: u32 = 1;
const LOCK_STATES: &[u32] = &[LOCK_IDLE, LOCK_BUSY_ONCE];

/// `(address, index, state)` in probe order; the vendor takes the list
/// first. The idle preamble samples BUSY once, then the lock samples it
/// until it reads idle.
fn execution_lock_abi(words: &[u32], _vendor: &Vendor<'_>) -> Result<Objects> {
    let [address, index, state] = *words else {
        unreachable!("execution lock: address, index and state")
    };
    let busy: &[u32] = match state {
        LOCK_IDLE => &[IDLE, IDLE],
        LOCK_BUSY_ONCE => &[IDLE, BUSY, IDLE],
        _ => unreachable!("declared execution-lock state"),
    };
    Ok(Objects {
        vendor_words: vec![index, address],
        devices: vec![diagnostic_port(&[(SELECT_BUSY, busy)])],
        ..Default::default()
    })
}

/// The baseband's `phy_param` gain byte, from the semantic words: the
/// version flag and the byte.
fn btbb_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [print_version, gain] = words else {
        unreachable!("baseband words: version flag, gain byte")
    };
    Ok(Objects {
        vendor_words: vec![*print_version],
        vendor: vec![(
            vendor.symbol(PHY_PARAM)? + PHY_PARAM_GAIN,
            vec![*gain as u8],
        )],
        ..Default::default()
    })
}

/// BLE PHY initialization timing bytes: zero, the normal profile's 91 and
/// all set.
const PHY_TIMING_BYTES: &[u32] = &[0, 91, 0xff];
/// Environment bases: a low and a high word-aligned base inside controller
/// SRAM.
const PHY_ENVIRONMENTS: &[u32] = &[0x2f00_0100, 0x2f07_ff00];
/// Resolving-list bases: a low and a high word inside controller SRAM.
const PHY_RESOLVING_LISTS: &[u32] = &[0x2f00_0200, 0x2f07_fe00];
/// The 0x20101470 bit-18 branch: not taken and taken.
const PHY_BRANCH: &[u32] = &[0, 1];
/// Controller configuration words at +0x40: zero, the pinned default 0x7d0
/// and all set.
const PHY_CONFIGURATION_WORDS: &[u32] = &[0, 0x7d0, u32::MAX];

/// The configuration pointer cell and the configuration it names, with its
/// timing byte at +0x10 and word at +0x40.
const PHY_CONFIGURATION_CELL: &str = "sym_controller_5RjCWP84jPuSUuPWsdfH";
const PHY_CONFIGURATION: u32 = 0x3fff_3200;
const PHY_CONFIGURATION_BYTES: usize = 0x48;
const PHY_CONFIGURATION_TIMING: usize = 0x10;
const PHY_CONFIGURATION_WORD: usize = 0x40;
/// The environment pointer cell of `ble_70.o`.
const PHY_ENVIRONMENT_CELL: &str = "sym_ble_wp7iIlMybMkUqAGDXiBb";
/// The resolving-list pointer cell of `ble_38.o`, which
/// `r_sym_ble_K9RI7JxM1NMxwdTUJ82J` returns.
const PHY_RESOLVING_LIST_CELL: &str = "sym_ble_sk1y8NvQWFYV5NnjiU1r";
/// The `ble_25.o` pointer cell `r_sym_ble_Fh7KRrkOJ0DaZapRKpqP` follows: its
/// object's word +0x20 names the SDK options, whose byte +0x55 selects the
/// branch.
const PHY_OPTIONS_CELL: &str = "sym_ble_yqQIeEuNtnC1SQAK2SBM";
const PHY_OPTIONS_HOLDER: u32 = 0x3fff_3300;
const PHY_OPTIONS_HOLDER_BYTES: usize = 0x24;
const PHY_OPTIONS_POINTER: usize = 0x20;
const PHY_OPTIONS: u32 = 0x3fff_3400;
const PHY_OPTIONS_BRANCH: usize = 0x55;

/// Modem ETM channel-zero event and task words and the channel-enable set
/// word the vendor writes, and the channel-two words production writes in
/// their place: IEEE 802.15.4 owns channel zero, so by the user's decision
/// the Bluetooth route runs on channel two.
const ETM_CHANNEL0_EVENT: u32 = 0x2010_880c;
const ETM_CHANNEL0_TASK: u32 = 0x2010_8810;
const ETM_CHANNEL2_EVENT: u32 = 0x2010_881c;
const ETM_CHANNEL2_TASK: u32 = 0x2010_8820;
const ETM_CHANNEL_ENABLE_SET: u32 = 0x2010_8804;
const ETM_REMAP: &str = "modem ETM channel zero is IEEE 802.15.4's; by the user's decision \
    production runs the Bluetooth PHY route on channel two";
const PHY_ETM_REPLACEMENTS: &[Replacement] = &[
    Replacement {
        vendor: (ETM_CHANNEL0_EVENT, 8),
        replacement: (ETM_CHANNEL2_EVENT, 8),
        reason: ETM_REMAP,
    },
    Replacement {
        vendor: (ETM_CHANNEL0_TASK, 0x14),
        replacement: (ETM_CHANNEL2_TASK, 0x14),
        reason: ETM_REMAP,
    },
    Replacement {
        vendor: (ETM_CHANNEL_ENABLE_SET, 1 << 0),
        replacement: (ETM_CHANNEL_ENABLE_SET, 1 << 2),
        reason: ETM_REMAP,
    },
];

/// The BLE PHY initializer reads every input from linked globals: the
/// configuration, the environment and resolving-list cells, and the SDK
/// options behind the `ble_25.o` cell.
fn phy_init_abi(words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    let [timing, environment, resolving_list, branch, word] = words else {
        unreachable!("PHY init words: timing, environment, resolving list, branch, word")
    };
    let mut configuration = vec![0; PHY_CONFIGURATION_BYTES];
    configuration[PHY_CONFIGURATION_TIMING] = *timing as u8;
    configuration[PHY_CONFIGURATION_WORD..PHY_CONFIGURATION_WORD + 4]
        .copy_from_slice(&word.to_le_bytes());
    let mut holder = vec![0; PHY_OPTIONS_HOLDER_BYTES];
    holder[PHY_OPTIONS_POINTER..].copy_from_slice(&PHY_OPTIONS.to_le_bytes());
    let mut options = vec![0; PHY_OPTIONS_BRANCH + 1];
    options[PHY_OPTIONS_BRANCH] = *branch as u8;
    Ok(Objects {
        vendor: vec![
            (PHY_CONFIGURATION, configuration),
            (PHY_OPTIONS_HOLDER, holder),
            (PHY_OPTIONS, options),
        ],
        image: vec![
            (
                vendor.symbol(PHY_CONFIGURATION_CELL)?,
                PHY_CONFIGURATION.to_le_bytes().to_vec(),
            ),
            (
                vendor.symbol(PHY_ENVIRONMENT_CELL)?,
                environment.to_le_bytes().to_vec(),
            ),
            (
                vendor.symbol(PHY_RESOLVING_LIST_CELL)?,
                resolving_list.to_le_bytes().to_vec(),
            ),
            (
                vendor.symbol(PHY_OPTIONS_CELL)?,
                PHY_OPTIONS_HOLDER.to_le_bytes().to_vec(),
            ),
        ],
        ..Default::default()
    })
}

/// Every compared Bluetooth leaf.
pub const LEAVES: &[Leaf] = &[
    // Current-RX memory-list pointer (formerly `r_sym_ble_LboRu27EaU8MV8Q7UUfZ`).
    ordered(
        leaf(
            "r_sym_ble_jhaN9KOsH0u0RgIu2DI9",
            "open_ble_memory_list_a_trace_r_sym_ble_lbo_ru27_ea_u8_mv8_q7_uuf_z",
            &[
                ("selector", Domain::Words(MEMORY_LIST_SELECTORS)),
                ("pointer", Domain::Words(MEMORY_LIST_POINTERS)),
            ],
            false,
        ),
        1,
    ),
    // Next-RX memory-list pointer (formerly `r_sym_ble_ZzrExMrn8EDiTFI7PENK`).
    ordered(
        leaf(
            "r_sym_ble_95olV4pLl0ue84SnuzXf",
            "open_ble_memory_list_b_trace_r_sym_ble_zzr_ex_mrn8_edi_tfi7_penk",
            &[
                ("selector", Domain::Words(MEMORY_LIST_SELECTORS)),
                ("pointer", Domain::Words(MEMORY_LIST_POINTERS)),
            ],
            false,
        ),
        1,
    ),
    // NRT interrupt capture and acknowledgement, up to its log/broker suffix
    // (formerly `r_sym_ble_ywjh0f9yjTBeI7XgS5da`).
    ordered(
        prefix(
            leaf(
                "r_sym_nrtIsr_kgxM2CJfVperrlVjmB4B",
                "open_ble_interrupt_trace_r_sym_ble_ywjh0f9yj_t_be_i7_xg_s5da",
                &[],
                false,
            ),
            "r_ble_log_set_buf_index_flag",
        ),
        1,
    ),
    // Scheduler hardware list heads, up to the event initialization.
    in_archive(
        ordered(
            prefix(
                objects(
                    leaf(
                        "r_sym_bt_XPuqTHliEO5V9xpR7aJR",
                        "open_ble_scheduler_trace_r_sym_bt_x_puq_thli_eo5_v9xp_r7a_jr",
                        &[],
                        false,
                    ),
                    scheduler_abi,
                ),
                "wr_btdm_osal_event_init",
            ),
            1,
        ),
        BTDM_COMMON_INPUT,
    ),
    // The diagnostic scheduler-BUSY sample opening the scheduler stop
    // (`r_btdm_sched_stop`): its busy path next calls the logger and its idle
    // path tail-calls it. The sample crosses a clock domain and is accepted
    // only when a fresh pair of reads agrees; the states model a stable
    // value and a first pair that disagrees, settling busy or idle.
    in_archive(
        ruled(
            tail_prefix(
                stated(
                    objects(
                        leaf(
                            "r_sym_bt_74l62ZLsZuXg67pPHSd7",
                            "open_ble_scheduler_stop_busy_trace_r_btdm_sched_stop",
                            &[],
                            false,
                        ),
                        diagnostic_sample_abi,
                    ),
                    DIAGNOSTIC_SAMPLE_STATES,
                ),
                "wr_btdm_log_internal_x0",
            ),
            busy_reselection_rules,
        ),
        BTDM_COMMON_INPUT,
    ),
    // The scheduler execution modify (`r_btdm_sched_execution_modify`) and
    // execution lock (`r_btdm_sched_execution_lock`), compared through their
    // diagnostic samples.
    in_archive(
        ruled(
            quiet(
                stated(
                    objects(
                        leaf(
                            "r_sym_bt_rPoPGH6BBYjZaunDU5FV",
                            "open_ble_scheduler_trace_execution_modify",
                            &[
                                ("index", Domain::Words(HARDWARE_LISTS)),
                                ("list_deletion", Domain::Words(LIST_DELETION)),
                            ],
                            false,
                        ),
                        execution_modify_abi,
                    ),
                    MODIFY_STATES,
                ),
                SCHEDULER_QUIET,
            ),
            scheduler_rules,
        ),
        BTDM_COMMON_INPUT,
    ),
    in_archive(
        ruled(
            quiet(
                stated(
                    objects(
                        leaf(
                            "r_sym_bt_9H3AnHbaHJ3auzSvPDme",
                            "open_ble_scheduler_trace_execution_lock",
                            &[
                                ("address", Domain::Words(LOCK_ADDRESSES)),
                                ("index", Domain::Words(HARDWARE_LISTS)),
                            ],
                            false,
                        ),
                        execution_lock_abi,
                    ),
                    LOCK_STATES,
                ),
                SCHEDULER_QUIET,
            ),
            scheduler_rules,
        ),
        BTDM_COMMON_INPUT,
    ),
    // Baseband v2 initialization with the version log answered without
    // effect, closed by the owner's device fence.
    in_archive(
        ordered(
            quiet(
                objects(
                    leaf(
                        "bt_bb_v2_init_cmplx",
                        "open_btbb_v2_init_trace_r_sym_bt_bb_v2_init_cmplx_x1",
                        &[
                            ("_print_version", Domain::Words(PRINT_VERSION)),
                            ("gain_parameter", Domain::Words(GAIN_PARAMETERS)),
                        ],
                        false,
                    ),
                    btbb_abi,
                ),
                &["bt_bb_v2_version"],
            ),
            1,
        ),
        BTBB_INPUT,
    ),
    // BLE PHY register initialization (`r_ble_phy_init_registers`). The
    // scheduler timing and ETM resource bookkeeping it calls touch only
    // software state and are answered without effect; the ETM route moves to
    // channel two; the owner closes with its device fence.
    ordered(
        replaced(
            quiet(
                objects(
                    leaf(
                        "r_sym_ble_nENHlP4KBuQYlFVffaR5",
                        "open_ble_phy_register_init_trace_r_ble_phy_init_registers",
                        &[
                            (
                                "private_timing_source_byte",
                                Domain::Words(PHY_TIMING_BYTES),
                            ),
                            ("environment_address", Domain::Words(PHY_ENVIRONMENTS)),
                            ("resolving_list_address", Domain::Words(PHY_RESOLVING_LISTS)),
                            ("set_branch_control_0470_bit_18", Domain::Words(PHY_BRANCH)),
                            (
                                "configuration_word_40",
                                Domain::Words(PHY_CONFIGURATION_WORDS),
                            ),
                        ],
                        false,
                    ),
                    phy_init_abi,
                ),
                &[
                    "r_sym_sched_modWXEVwpjpaAKhsFBrP",
                    "r_sym_resMgmt_Zl2TCBLFeHxQsRxH3wmd",
                ],
            ),
            PHY_ETM_REPLACEMENTS,
        ),
        1,
    ),
    // The FreeRTOS critical section and the sleep power-domain bookkeeping
    // around the modem clock registers are answered without effect.
    ruled(
        quiet(
            released(
                in_archive(
                    objects(
                        leaf(
                            "modem_clock_select_lp_clock_source",
                            "open_bluetooth_trace_select_low_power_clock",
                            &[],
                            false,
                        ),
                        low_power_clock_select_abi,
                    ),
                    HW_SUPPORT_INPUT,
                ),
                1,
            ),
            MODEM_CLOCK_QUIET,
        ),
        low_power_timer_gate_rules,
    ),
    ruled(
        quiet(
            released(
                in_archive(
                    objects(
                        leaf(
                            "modem_clock_deselect_lp_clock_source",
                            "open_bluetooth_trace_deselect_low_power_clock",
                            &[],
                            false,
                        ),
                        low_power_clock_deselect_abi,
                    ),
                    HW_SUPPORT_INPUT,
                ),
                1,
            ),
            MODEM_CLOCK_QUIET,
        ),
        low_power_timer_gate_rules,
    ),
];

/// Scheduler calls outside its register transaction: the logger, the
/// critical section and the assertion the bounded loops never reach.
const SCHEDULER_QUIET: &[&str] = &[
    "wr_btdm_log_internal_x1",
    "wr_btdm_log_internal_x2",
    "wr_btdm_osal_hw_enter_critical",
    "wr_btdm_osal_hw_exit_critical",
    "wr_btdm_compressed_assert_x2",
];

/// Modem clock driver calls outside its register transaction. The FreeRTOS
/// port lives in the firmware's writable IRAM, which cannot carry a link
/// definition, so the suite declares it absent.
pub(crate) const MODEM_CLOCK_QUIET: &[&str] = &[
    "xPortInIsrContext",
    "xPortEnterCriticalTimeout",
    "vPortExitCriticalMultiCore",
    "esp_sleep_pd_config",
];
/// `MODEM_LPCON.CLK_CONF` at the firmware's `MODEM_LPCON` base. ESP-HAL owns
/// its gates and read-modify-writes the word under the lock that also serves
/// its regi2c accesses; the radio requests them from its platform clock
/// provider, which the probes answer without register access.
pub(crate) const MODEM_LPCON_CLK_CONF: u32 = 0x2010_f018;
/// The one read-modify-write of `MODEM_LPCON.CLK_CONF` a low-power clock
/// selection performs.
const PLATFORM_GATE_ACCESSES: u32 = 1;

/// The vendor's read-modify-write of a platform-owned `MODEM_LPCON.CLK_CONF`
/// gate, which production leaves to the platform for `reason`.
pub(crate) fn platform_gate_rules(reason: &str) -> Vec<blobray_domain::EffectRule> {
    vec![
        crate::contracts::omitted_read(
            "platform-gate-read".into(),
            MODEM_LPCON_CLK_CONF,
            PLATFORM_GATE_ACCESSES,
            reason,
        ),
        crate::contracts::omitted_write(
            "platform-gate-write".into(),
            MODEM_LPCON_CLK_CONF,
            PLATFORM_GATE_ACCESSES,
            reason,
        ),
    ]
}

/// The Bluetooth selection sets the low-power timer gate and the
/// deselection clears it.
fn low_power_timer_gate_rules() -> Vec<blobray_domain::EffectRule> {
    platform_gate_rules(
        "production acquires and releases the modem low-power timer gate through the \
        platform clock provider; ESP-HAL writes MODEM_LPCON.CLK_CONF under its own lock",
    )
}
/// ESP-IDF `PERIPH_BT_MODULE` of `soc/esp32s31/include/soc/periph_defs.h`.
const PERIPH_BT_MODULE: u32 = 6;
/// ESP-IDF `MODEM_CLOCK_LPCLK_SRC_MAIN_XTAL` of `hal/modem_clock_types.h`.
const MODEM_CLOCK_LPCLK_SRC_MAIN_XTAL: u32 = 2;
/// Divider the ESP-IDF Bluetooth controller passes for the main crystal:
/// `CONFIG_XTAL_FREQ * 1000000 / s_bt_xtal_lpclk_freq - 1` with the reference
/// build's 40-MHz crystal and the 100-kHz default low-power clock.
const BLUETOOTH_MAIN_XTAL_DIVIDER: u32 = 40_000_000 / 100_000 - 1;

/// The modem clock HAL context `MODEM_CLOCK_instance` initializes on first
/// use: the `MODEM_SYSCON` and `MODEM_LPCON` bases the firmware's linker
/// scripts provide. It is copied into the linked image, so the driver finds
/// its HAL initialized and never references the absent linker names.
pub(crate) fn modem_clock_hal_context(vendor: &Vendor<'_>) -> Result<(u32, Vec<u8>)> {
    let mut context = (vendor.firmware)("MODEM_SYSCON")?.to_le_bytes().to_vec();
    context.extend((vendor.firmware)("MODEM_LPCON")?.to_le_bytes());
    Ok((vendor.symbol("modem_clock_hal.8")?, context))
}

/// `modem_clock_select_lp_clock_source(PERIPH_BT_MODULE, MAIN_XTAL, divider)`,
/// the call of the ESP-IDF Bluetooth controller; production takes no words.
fn low_power_clock_select_abi(_words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    Ok(Objects {
        vendor_words: vec![
            PERIPH_BT_MODULE,
            MODEM_CLOCK_LPCLK_SRC_MAIN_XTAL,
            BLUETOOTH_MAIN_XTAL_DIVIDER,
        ],
        image: vec![modem_clock_hal_context(vendor)?],
        ..Default::default()
    })
}

/// `modem_clock_deselect_lp_clock_source(PERIPH_BT_MODULE)`; production
/// takes no words.
fn low_power_clock_deselect_abi(_words: &[u32], vendor: &Vendor<'_>) -> Result<Objects> {
    Ok(Objects {
        vendor_words: vec![PERIPH_BT_MODULE],
        image: vec![modem_clock_hal_context(vendor)?],
        ..Default::default()
    })
}

/// The Bluetooth controller suite.
pub const BLUETOOTH: Suite = Suite {
    title: "Bluetooth LE controller leaf comparison",
    archives: &[
        "libble_app",
        "libbtdm_common",
        "libbtbb",
        "libesp-hw-support",
        "libhal",
    ],
    leaves: LEAVES,
    id: crate::mac::CONTRACT_ID,
    rom: crate::mac::ROM,
    firmware: Some(crate::mac::FIRMWARE),
    roots: &[],
    prepare: None,
    claims: &[],
    // ESP-IDF linker-script broker tables, which the interrupt, the scheduler
    // and its stop reach only after their compared prefixes; the peripheral bases and ROM
    // aliases the linker scripts provide and the firmware's writable-IRAM
    // error, log and I2C functions, which only modem clock driver paths the
    // compared cases never take reference; and the writable-IRAM FreeRTOS
    // port, which the modem clock leaves answer as quiet calls.
    absent: &[
        "_nrtIsr_linear_broker_flash",
        "_btdm_sched_linear_broker_flash",
        "_btdm_sched_linear_broker_ram",
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
    ],
};

#[cfg(test)]
mod tests {
    use super::*;
    use blobray_domain::{EffectDisposition, ExecutionEvent};

    /// The `MODEM_LPCON` word holding the Bluetooth low-power clock source and
    /// divider, which the selection writes and production compares.
    const BLUETOOTH_LOW_POWER_CLOCK_CONF: u32 = 0x2010_f004;

    fn dispositions(event: &ExecutionEvent) -> Vec<EffectDisposition> {
        low_power_timer_gate_rules()
            .iter()
            .filter(|rule| rule.vendor.unwrap().selects(event, None))
            .map(|rule| rule.disposition)
            .collect()
    }

    #[test]
    fn only_a_busy_reselection_between_reads_is_added() {
        let rules = busy_reselection_rules();
        let pattern = rules[0].replacement.unwrap();
        let read = ExecutionEvent::Read {
            address: DIAGNOSTIC_VALUE,
            width: 4,
            value: u32::MAX,
        };
        let select = |value| ExecutionEvent::Write {
            address: DIAGNOSTIC_SELECTOR,
            width: 4,
            value,
        };
        let history = [select(SELECT_BUSY), read.clone()];
        assert!(pattern.selects_after(&select(SELECT_BUSY), Some(&read), &history));
        // The opening selection and a selection no read follows stay
        // compared; any other selector value violates the rule.
        assert!(!pattern.selects_after(&select(SELECT_BUSY), Some(&read), &[]));
        assert!(!pattern.selects_after(&select(SELECT_BUSY), None, &history));
        assert_eq!(
            pattern.value,
            blobray_domain::EffectValue::Exact { value: SELECT_BUSY }
        );
    }

    #[test]
    fn only_the_platform_gate_word_may_be_omitted() {
        for event in [
            ExecutionEvent::Read {
                address: MODEM_LPCON_CLK_CONF,
                width: 4,
                value: 0,
            },
            ExecutionEvent::Write {
                address: MODEM_LPCON_CLK_CONF,
                width: 4,
                value: 8,
            },
        ] {
            assert_eq!(dispositions(&event), [EffectDisposition::Omitted]);
        }
        assert!(
            dispositions(&ExecutionEvent::Write {
                address: BLUETOOTH_LOW_POWER_CLOCK_CONF,
                width: 4,
                value: 4,
            })
            .is_empty()
        );
    }
}
