//! Comparison of the RX-ring recycle with `wDev_AppendRxBlocks`.
//!
//! The vendor returns a completed descriptor unit to its receive list in one
//! call: it rearms every descriptor of the unit and resets its buffer guards,
//! links the unit after the list's tail, rings the append doorbell and polls
//! it, then repairs the descriptor base when the walker already stopped at
//! the old tail. Production does the same through its ring owner's recycle
//! and reload completion. Both sides hold the ring at the addresses of the
//! production probe's DMA arena, which its layout entry reports; each case
//! completes the ring's first descriptors as one unit and compares the
//! doorbell, cursor and base register effects, the descriptors and the
//! unit's buffer guards.
use crate::harness::{Arg, Result, case, direct, invalid, known, region, selection};
use crate::mac::{LEAF_FILLS, Mac};
use blobray_domain::{
    CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, ComparisonVerdict,
    DeviceBehavior, DeviceDeclaration, EffectDisposition, EffectPattern, EffectRule,
    EffectSelector, EffectValue, ExecutionCase, MemoryPair, ReadRun, RegionLifetime, SessionReset,
};

/// The vendor function, linked as an extra root.
pub const ROOTS: &[&str] = &[VENDOR];
/// Evidence claim: the vendor append with the production recycle.
pub const CLAIMS: &[(&str, &str, &str)] = &[("archive", VENDOR, RECYCLE_PROBE)];
const VENDOR: &str = "wDev_AppendRxBlocks";
const LAYOUT_PROBE: &str = "open_libpp_rx_append_trace_layout";
const RECYCLE_PROBE: &str = "open_libpp_rx_append_trace_recycle";

/// `wDevCtrl`, whose first two words are the receive list's head and tail.
const LIST: &str = "wDevCtrl";
/// The OS adapter table pointer, in the vendor firmware, and the offsets of the critical-section
/// enter and exit functions the vendor calls through it.
const OSI: &str = "g_osi_funcs_p";
const OSI_ENTER: u32 = 0x28;
const OSI_EXIT: u32 = 0x2c;
const LOCK: &str = "g_intr_lock_mux";
const STATISTICS: &str = "esp_test_rx_statistics";
const STATISTICS_BYTES: u32 = 8;
/// Vendor functions answered without effect: the assertion, the diagnostic
/// list record, and the list dump the vendor reaches only when the doorbell
/// never clears.
const QUIET: &[&str] = &[
    "wifi_assert",
    "wdev_record_rx_linked_list",
    "wdev_dump_rx_linked_list",
];

/// Scratch the scenario owns: the layout output, the OS adapter table, its
/// two entries and the source of the image patches.
const LAYOUT_OUTPUT: u32 = 0x3fff_2000;
const LAYOUT_OUTPUT_WORDS: u32 = 64;
const OSI_TABLE: u32 = 0x3fff_2200;
const OSI_TABLE_BYTES: u32 = 0x40;
const OSI_ENTER_FUNCTION: u32 = 0x3fff_2400;
const OSI_EXIT_FUNCTION: u32 = 0x3fff_2410;
const PATCH: u32 = 0x3fff_2500;
/// Words of the layout output before the buffer addresses: count,
/// capacity and descriptor base.
const LAYOUT_HEADER_WORDS: usize = 3;

/// `WIFI_MAC_RX_DMA` registers: the control word whose bit 0 is the append
/// doorbell, the descriptor base, the walker's next and last descriptors,
/// and the high-address window `hal_mac_rx_get_last_dscr` completes LAST
/// with.
const RX_CONTROL: u32 = 0x2010_4080;
const RX_BASE: u32 = 0x2010_4084;
const RX_NEXT: u32 = 0x2010_4088;
const RX_LAST: u32 = 0x2010_408c;
const RX_WINDOW: u32 = 0x2010_4c70;
const DOORBELL: u32 = 1;
/// The walker's address projection and the DMA window's high bits.
const LOW_ADDRESS: u32 = 0x000f_ffff;

/// Descriptor layout the vendor rearm reads: word 0's size and length
/// fields, its done and owner bits; the buffer and next words.
const DESCRIPTOR_BYTES: u32 = 12;
const SIZE_MASK: u32 = 0x3fff;
const LENGTH_SHIFT: u32 = 14;
const LENGTH_MASK: u32 = 0x0fff_c000;
const DONE: u32 = 1 << 30;
const OWNED: u32 = 1 << 31;
const BUFFER_WORD: usize = 4;
const NEXT_WORD: usize = 8;
/// The guard word the vendor writes at both ends of a recycled buffer.
const GUARD: u32 = 0xdead_beef;
/// Frame length the walker reports for the unit.
const UNIT_LENGTH: u32 = 48;

/// One walker state: how far it completed beyond the unit, and its NEXT
/// and LAST as descriptor indices (`None` for a zero NEXT) when production
/// proves the unit released before the append and when both sides settle
/// the doorbell after it.
#[derive(Clone, Copy)]
struct Cursor {
    label: &'static str,
    /// Single-descriptor units the walker completed after the unit.
    later: u32,
    /// Reads the doorbell stays pending.
    pending: u32,
    /// Production's release proof: LAST, then NEXT.
    proof: (Walker, Option<Walker>),
    /// The NEXT both sides sample after the doorbell, and LAST when it is
    /// zero.
    settled: (Option<Walker>, Walker),
    /// Whether the settled cursor repairs the base to the unit's head.
    repair: bool,
    /// Whether the unit is the whole list: the vendor's list is empty once
    /// `wDev_DiscardFrame` detached it, and both sides publish the unit
    /// through the base without a doorbell.
    empty: bool,
}

/// A descriptor the walker names, relative to the unit.
#[derive(Clone, Copy)]
enum Walker {
    /// The descriptor `n` after the unit's tail.
    AfterUnit(u32),
    /// The list's tail before the append.
    OldTail,
}

/// The walker completed one further unit and moved on: production's
/// completed frontier proves the release, and no repair follows. Or it
/// exhausted the list at its old tail: the settled cursor repairs the base
/// to the appended head, unless the walker already consumed the unit and
/// stopped at its tail. Or it exhausted a list the unit made up whole:
/// the unit is published through the base.
const CURSORS: &[Cursor] = &[
    Cursor {
        label: "beyond",
        later: 1,
        pending: 0,
        proof: (Walker::AfterUnit(1), Some(Walker::AfterUnit(2))),
        settled: (Some(Walker::AfterUnit(2)), Walker::OldTail),
        repair: false,
        empty: false,
    },
    Cursor {
        label: "beyond-pending",
        later: 1,
        pending: 2,
        proof: (Walker::AfterUnit(1), Some(Walker::AfterUnit(2))),
        settled: (Some(Walker::AfterUnit(2)), Walker::OldTail),
        repair: false,
        empty: false,
    },
    Cursor {
        label: "exhausted",
        later: 0,
        pending: 1,
        proof: (Walker::OldTail, None),
        settled: (None, Walker::OldTail),
        repair: true,
        empty: false,
    },
    Cursor {
        label: "exhausted-new-tail",
        later: 0,
        pending: 0,
        proof: (Walker::OldTail, None),
        settled: (None, Walker::AfterUnit(0)),
        repair: false,
        empty: false,
    },
    Cursor {
        label: "empty",
        later: 0,
        pending: 0,
        proof: (Walker::OldTail, None),
        settled: (None, Walker::OldTail),
        repair: true,
        empty: true,
    },
];
/// Descriptors of the completed unit, besides the whole ring the empty
/// list returns.
const UNITS: &[u32] = &[1, 2];

/// The fence sets of a full `fence rw, rw`, and of the acquire
/// (`fence r, rw`) and release (`fence rw, w`) fences of atomics.
const FULL_FENCE: u8 = 0xf;
const ACQUIRE_FENCE: (u8, u8) = (2, 3);
const RELEASE_FENCE: (u8, u8) = (3, 1);
/// Guest events one append may record.
const APPEND_EVENTS: u32 = 1 << 12;

/// The production arena the layout entry reports.
struct Layout {
    capacity: u32,
    base: u32,
    buffers: Vec<u32>,
}

impl Layout {
    fn descriptor(&self, index: usize) -> u32 {
        self.base + index as u32 * DESCRIPTOR_BYTES
    }
}

fn quiet(name: &str, address: u32, boundary: CallBoundary) -> CallDeclaration {
    CallDeclaration {
        id: name.into(),
        applicability: "a diagnostic or serialization callee of the append answered \
            without effect"
            .into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary,
            allow_tail: true,
        },
        argument_words: 2,
        responses: vec![CallResponse {
            return_words: [Some(0), None],
            outputs: vec![],
            allocation: None,
            delay_micros: None,
        }],
        repetition: CallRepetition::Unbounded,
    }
}

/// The production arena, from its layout entry.
fn layout(ctx: &mut Mac) -> Result<Layout> {
    let memcpy = memcpy(ctx)?;
    let mut production = ctx.session.probes.invoke(
        LAYOUT_PROBE,
        vec![("output", Arg::Word(Some(i64::from(LAYOUT_OUTPUT))))],
        vec![],
        vec![selection(LAYOUT_OUTPUT, LAYOUT_OUTPUT_WORDS * 4)],
    )?;
    production.memory.push(known(
        LAYOUT_OUTPUT,
        LAYOUT_OUTPUT_WORDS * 4,
        &vec![0; (LAYOUT_OUTPUT_WORDS * 4) as usize],
    )?);
    production.arguments.resize(8, Some(0));
    let vendor = direct(memcpy, &[PATCH, PATCH, 0], vec![], vec![], vec![]);
    let mut row = crate::harness::setup("rx-append-layout", vendor, production, SessionReset::Cold);
    row.stack_fill = Some(LEAF_FILLS[0]);
    let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
    let records = ctx
        .submit(
            "rx-append-layout",
            &crate::session::request(&vendor, Some(&production), None, vec![row], APPEND_EVENTS),
            Some(ComparisonVerdict::Match),
        )?
        .records
        .clone();
    if crate::i2c::returned_low(&records, 0, true) != Some(0) {
        return Err(invalid("the RX append layout entry failed"));
    }
    let words: Vec<u32> = crate::evidence::output(&records, 0, true)
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]))
        .collect();
    let count = *words
        .first()
        .ok_or_else(|| invalid("no RX append layout"))? as usize;
    let buffers = words
        .get(LAYOUT_HEADER_WORDS..LAYOUT_HEADER_WORDS + count)
        .ok_or_else(|| invalid("RX append layout exceeds its output"))?
        .to_vec();
    Ok(Layout {
        capacity: words[1],
        base: words[2],
        buffers,
    })
}

fn memcpy(ctx: &Mac) -> Result<u32> {
    Ok(u32::try_from(
        crate::harness::symbol(
            &ctx.session.inventory,
            crate::layout::ROM_INPUT as usize,
            "memcpy",
        )?
        .value,
    )?)
}

/// The ring before the append, as the walker left it after completing the
/// first `unit` descriptors and `later` single-descriptor units after them:
/// every descriptor armed and linked, the completed buffers written, each
/// unit's last descriptor done with its length and the first unit, as
/// `wDev_DiscardFrame` leaves it for the vendor, terminated.
fn ring(layout: &Layout, unit: usize, later: usize) -> (Vec<u8>, Vec<(u32, Vec<u8>)>) {
    let count = layout.buffers.len();
    let armed = layout.capacity | (layout.capacity << LENGTH_SHIFT) | OWNED;
    let mut descriptors = vec![0u8; count * DESCRIPTOR_BYTES as usize];
    for (index, chunk) in descriptors
        .chunks_exact_mut(DESCRIPTOR_BYTES as usize)
        .enumerate()
    {
        let mut word0 = armed;
        let mut next = if index + 1 < count {
            layout.descriptor(index + 1)
        } else {
            0
        };
        if index + 1 >= unit && index < unit + later {
            word0 = (armed & !LENGTH_MASK) | (UNIT_LENGTH << LENGTH_SHIFT) | DONE;
        }
        if index + 1 == unit {
            next = 0;
        }
        chunk[..4].copy_from_slice(&word0.to_le_bytes());
        chunk[BUFFER_WORD..BUFFER_WORD + 4].copy_from_slice(&layout.buffers[index].to_le_bytes());
        chunk[NEXT_WORD..NEXT_WORD + 4].copy_from_slice(&next.to_le_bytes());
    }
    let buffers = layout
        .buffers
        .iter()
        .enumerate()
        .map(|(index, address)| {
            let mut bytes = vec![0u8; layout.capacity as usize + 4];
            let first = if index < unit + later {
                UNIT_LENGTH
            } else {
                GUARD
            };
            bytes[..4].copy_from_slice(&first.to_le_bytes());
            let end = layout.capacity as usize;
            bytes[end..end + 4].copy_from_slice(&GUARD.to_le_bytes());
            (*address, bytes)
        })
        .collect();
    (descriptors, buffers)
}

/// The doorbell, cursor, window and base registers of one side of a case.
/// Production first reads the doorbell to see the previous append settled
/// and samples LAST and NEXT to prove the unit released, then appends.
fn registers(
    layout: &Layout,
    unit: usize,
    cursor: Cursor,
    production: bool,
) -> Vec<DeviceDeclaration> {
    let device = |id: &str, behavior| DeviceDeclaration {
        id: id.into(),
        applicability: "the RX descriptor walker around one append doorbell".into(),
        lifetime: RegionLifetime::Phase,
        behavior,
    };
    let address = |walker: Walker| match walker {
        Walker::AfterUnit(n) => layout.descriptor(unit - 1 + n as usize),
        Walker::OldTail => layout.descriptor(layout.buffers.len() - 1),
    };
    let low = |walker: Option<Walker>| walker.map_or(0, |w| address(w) & LOW_ADDRESS);
    let mut control = vec![];
    let (mut next, mut last) = (vec![], vec![]);
    if production {
        control.push(0);
        last.push(ReadRun::once(low(Some(cursor.proof.0))));
        next.push(ReadRun::once(low(cursor.proof.1)));
    }
    if !cursor.empty {
        // The doorbell's read-modify-write, its pending polls and the
        // settled read.
        control.push(0);
        control.extend(std::iter::repeat_n(DOORBELL, cursor.pending as usize));
        control.push(0);
        next.push(ReadRun::once(low(cursor.settled.0)));
        if cursor.settled.0.is_none() {
            last.push(ReadRun::once(low(Some(cursor.settled.1))));
        }
    }
    let mut models = vec![];
    if !control.is_empty() {
        models.push(device(
            "rx-control",
            DeviceBehavior::Fifo {
                address: RX_CONTROL,
                width: 4,
                reads: control,
                writes: if cursor.empty { vec![] } else { vec![DOORBELL] },
            },
        ));
    }
    if !next.is_empty() {
        models.push(device(
            "rx-next",
            DeviceBehavior::SequenceRead {
                address: RX_NEXT,
                width: 4,
                runs: next,
            },
        ));
    }
    if !last.is_empty() {
        models.push(device(
            "rx-last",
            DeviceBehavior::SequenceRead {
                address: RX_LAST,
                width: 4,
                runs: last,
            },
        ));
    }
    if !production && !cursor.empty && cursor.settled.0.is_none() {
        models.push(device(
            "rx-window",
            DeviceBehavior::ConstantRead {
                address: RX_WINDOW,
                width: 4,
                value: layout.base & !LOW_ADDRESS,
            },
        ));
    }
    if cursor.repair {
        models.push(device(
            "rx-base",
            DeviceBehavior::Fifo {
                address: RX_BASE,
                width: 4,
                reads: vec![],
                writes: vec![layout.descriptor(0)],
            },
        ));
    }
    models
}

/// Production's first sample of the walker's cursor register `address`, its
/// proof that the unit's link is released before rearming it. Every later
/// sample settles the doorbell as the vendor's does and compares exactly.
fn release_proof(name: &str, address: u32) -> EffectRule {
    EffectRule {
        name: name.into(),
        vendor: None,
        replacement: Some(EffectPattern {
            selector: EffectSelector::MmioRead { address, width: 4 },
            value: EffectValue::Any,
            preceded_by: None,
            occurrence: Some(1),
            followed_by: None,
        }),
        disposition: EffectDisposition::Added,
        min_occurrences: 1,
        max_occurrences: 1,
        reason: "production samples the walker's LAST and NEXT to prove the unit released \
            before rearming it; the vendor's caller leaves the unit to the walker without that \
            proof"
            .into(),
    }
}

/// Effects the two sides reach differently: the vendor completes LAST with
/// its high-address window, which production's DMA window makes constant,
/// and production orders its cursor samples with device fences.
fn contract(ctx: &mut Mac) -> Result<oer_riscv_model::ArtifactId> {
    let image = ctx.image_symbols()?;
    let vendor = ctx.session.image_endpoint(
        &ctx.image_object,
        VENDOR,
        ctx.symbol_address(&image, VENDOR)?,
    )?;
    let production = ctx.session.input_endpoint(2, RECYCLE_PROBE)?;
    let window = EffectPattern {
        selector: EffectSelector::MmioRead {
            address: RX_WINDOW,
            width: 4,
        },
        value: EffectValue::Any,
        preceded_by: None,
        occurrence: None,
        followed_by: None,
    };
    let rules = vec![
        EffectRule {
            name: "vendor-only-window-read".into(),
            vendor: Some(window),
            replacement: Some(window),
            disposition: EffectDisposition::Omitted,
            min_occurrences: 0,
            max_occurrences: 1,
            reason: "the vendor completes LAST's low address with the DMA window's high bits; \
                production binds its ring inside that window and compares the low address"
                .into(),
        },
        EffectRule {
            name: "settled-check".into(),
            vendor: None,
            replacement: Some(EffectPattern {
                selector: EffectSelector::MmioRead {
                    address: RX_CONTROL,
                    width: 4,
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: Some(1),
                followed_by: None,
            }),
            disposition: EffectDisposition::Added,
            min_occurrences: 1,
            max_occurrences: 1,
            reason: "production reads the doorbell before an append to see the previous append \
                settled, then proves the unit released; the vendor completes each append's \
                poll before returning and leaves the unit to the walker without that proof"
                .into(),
        },
        release_proof("release-proof-last", RX_LAST),
        release_proof("release-proof-next", RX_NEXT),
        EffectRule {
            name: "atomic-ordering-fence".into(),
            vendor: None,
            replacement: Some(EffectPattern {
                selector: EffectSelector::Fence {
                    predecessor: ACQUIRE_FENCE.0,
                    successor: ACQUIRE_FENCE.1,
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            }),
            disposition: EffectDisposition::Added,
            min_occurrences: 0,
            max_occurrences: u32::from(u8::MAX),
            reason: "the ring and arena ownership states are atomics".into(),
        },
        EffectRule {
            name: "atomic-release-fence".into(),
            vendor: None,
            replacement: Some(EffectPattern {
                selector: EffectSelector::Fence {
                    predecessor: RELEASE_FENCE.0,
                    successor: RELEASE_FENCE.1,
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            }),
            disposition: EffectDisposition::Added,
            min_occurrences: 0,
            max_occurrences: u32::from(u8::MAX),
            reason: "the ring and arena ownership states are atomics".into(),
        },
        EffectRule {
            name: "cursor-ordering-fence".into(),
            vendor: None,
            replacement: Some(EffectPattern {
                selector: EffectSelector::Fence {
                    predecessor: FULL_FENCE,
                    successor: FULL_FENCE,
                },
                value: EffectValue::Any,
                preceded_by: None,
                occurrence: None,
                followed_by: None,
            }),
            disposition: EffectDisposition::Added,
            min_occurrences: 0,
            max_occurrences: u32::from(u8::MAX),
            reason: "production orders the doorbell and its NEXT and LAST samples with device \
                fences; the vendor relies on its uncached register accesses"
                .into(),
        },
    ];
    ctx.session.review_effects(
        "rx-append-effects",
        &format!(
            "{}.{}.{VENDOR}.effects",
            crate::CHIP.name,
            crate::mac::CONTRACT_ID
        ),
        crate::phy::contracts::phy_contract(
            vendor,
            production,
            rules,
            "one append of a completed unit to a live receive list",
        ),
        "every doorbell, cursor and base effect compares exactly; production adds fences and \
            omits the window read",
    )
}

/// The rows of one case: the compared append, whose vendor side preloads
/// the receive list and ring.
fn case_rows(
    ctx: &mut Mac,
    layout: &Layout,
    unit: usize,
    cursor: Cursor,
    fill: u8,
    effects: &oer_riscv_model::ArtifactId,
) -> Result<Vec<ExecutionCase>> {
    let image = ctx.image_symbols()?;
    let symbol = |name: &str| ctx.symbol_address(&image, name);
    let label = format!("rx-append-{unit}-{}-{fill:02x}", cursor.label);
    let head = layout.descriptor(0);
    let old_tail = layout.descriptor(layout.buffers.len() - 1);
    let list_head = if cursor.empty { 0 } else { head };
    let list: Vec<u8> = [list_head, old_tail]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    // The ring lives in the production probe's DMA arena, which the session
    // maps for both sides: the vendor's ring is preloaded into it.
    let (descriptors, buffers) = ring(layout, unit, cursor.later as usize);
    let mut preload = vec![(symbol(LIST)?, list), (layout.base, descriptors.clone())];
    preload.extend(buffers);
    let named = |name: String, address: u32, length: u32| blobray_domain::MemorySelection {
        name,
        address,
        length,
    };
    let mut observed = vec![named(
        "descriptors".into(),
        layout.base,
        descriptors.len() as u32,
    )];
    for (index, address) in layout.buffers[..unit].iter().enumerate() {
        observed.push(named(format!("buffer-{index}-head"), *address, 4));
        observed.push(named(
            format!("buffer-{index}-guard"),
            address + layout.capacity,
            4,
        ));
    }
    let mut table = vec![0u8; OSI_TABLE_BYTES as usize];
    table[OSI_ENTER as usize..OSI_ENTER as usize + 4]
        .copy_from_slice(&OSI_ENTER_FUNCTION.to_le_bytes());
    table[OSI_EXIT as usize..OSI_EXIT as usize + 4]
        .copy_from_slice(&OSI_EXIT_FUNCTION.to_le_bytes());
    let memory = vec![
        region(
            OSI_TABLE,
            OSI_TABLE_BYTES,
            &table,
            None,
            RegionLifetime::Phase,
        )?,
        // The adapter pointer belongs to the vendor firmware's data, outside
        // the linked image the patches write.
        known(symbol(OSI)?, 4, &OSI_TABLE.to_le_bytes())?,
        // The critical-section lock argument, unused by the answered calls.
        known(symbol(LOCK)?, 4, &[0; 4])?,
        // The test statistics a base repair counts into, disabled: its
        // pointer at +4 is null.
        known(
            symbol(STATISTICS)?,
            STATISTICS_BYTES,
            &[0; STATISTICS_BYTES as usize],
        )?,
    ];
    let mut vendor = direct(
        symbol(VENDOR)?,
        &[head, layout.descriptor(unit - 1), unit as u32],
        memory,
        registers(layout, unit, cursor, false),
        observed.clone(),
    );
    vendor.preload = preload
        .into_iter()
        .map(|(address, bytes)| blobray_domain::MemoryPreload { address, bytes })
        .collect();
    for name in QUIET {
        vendor.calls.push(quiet(
            name,
            symbol(name)?,
            crate::mac::call_boundary(&image, name),
        ));
    }
    for (name, address) in [
        ("osi-enter", OSI_ENTER_FUNCTION),
        ("osi-exit", OSI_EXIT_FUNCTION),
    ] {
        vendor
            .calls
            .push(quiet(name, address, CallBoundary::Unmapped));
    }
    let mut production = ctx.session.probes.invoke(
        RECYCLE_PROBE,
        vec![
            ("descriptors", Arg::Word(Some(unit as i64))),
            ("later", Arg::Word(Some(i64::from(cursor.later)))),
            ("length", Arg::Word(Some(i64::from(UNIT_LENGTH)))),
        ],
        registers(layout, unit, cursor, true),
        observed.clone(),
    )?;
    production.arguments.resize(8, Some(0));
    let mut row = case(label, vendor, Some(production), SessionReset::Cold, false);
    let relation = row.relation.as_mut().expect("a compared case");
    relation.effects = Some(effects.clone());
    relation.memory = (0..observed.len() as u16)
        .map(|index| MemoryPair {
            vendor: index,
            replacement: index,
        })
        .collect();
    row.stack_fill = Some(fill);
    Ok(vec![row])
}

/// Compare the append of every unit size under every cursor state; each
/// must MATCH and production must report success.
pub fn exercise(ctx: &mut Mac) -> Result<()> {
    let layout = layout(ctx)?;
    if layout.buffers.len() < 2 || layout.capacity & !SIZE_MASK != 0 {
        return Err(invalid("the RX append layout is not a ring"));
    }
    let effects = contract(ctx)?;
    let whole = layout.buffers.len() as u32;
    for unit in UNITS.iter().copied().chain([whole]) {
        let mut rows = vec![];
        let mut compared = vec![];
        for &cursor in CURSORS.iter().filter(|c| c.empty == (unit == whole)) {
            for fill in LEAF_FILLS {
                let added = case_rows(ctx, &layout, unit as usize, cursor, fill, &effects)?;
                compared.push(rows.len() as u32 + added.len() as u32 - 1);
                rows.extend(added);
            }
        }
        let label = format!("rx-append-{unit}");
        let (vendor, production) = (ctx.vendor.clone(), ctx.production.clone());
        let records = ctx
            .submit(
                &label,
                &crate::session::request(&vendor, Some(&production), None, rows, APPEND_EVENTS),
                Some(ComparisonVerdict::Match),
            )?
            .records
            .clone();
        for case in compared {
            let status = crate::i2c::returned_low(&records, case, true);
            if status != Some(0) {
                return Err(invalid(format!(
                    "{label} case {case}: production recycle reported {status:?}"
                )));
            }
        }
    }
    Ok(())
}
