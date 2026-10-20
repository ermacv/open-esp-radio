use super::*;
use oer_analysis_riscv_model::UnknownCallBehavior;

#[path = "waker_scope_tests.rs"]
mod waker_scopes;

#[path = "recursion_tests.rs"]
mod recursion;

/// Working memory of a test's analysis: no test comes near it.
const TEST_MEMORY: u64 = 1 << 32;

/// Run `body` over the facts of `elf`, read without a work budget.
pub(crate) fn with_facts<R>(
    elf: &[u8],
    body: impl FnOnce(&ImageFacts<'_>) -> Result<R>,
) -> Result<R> {
    let memory = WorkingMemory::new(TEST_MEMORY)?;
    let facts = ImageFacts::new(elf, &[], &memory, &mut || Ok(()))?;
    body(&facts)
}

/// A ledger in `memory`.
fn kept(memory: &WorkingMemory) -> Ledger<'_> {
    Ledger::new(memory, &mut || Ok(())).unwrap()
}

/// A ledger in an unbounded test memory, for what a test keeps.
pub(crate) fn ledger() -> Ledger<'static> {
    let memory = Box::leak(Box::new(WorkingMemory::new(TEST_MEMORY).unwrap()));
    Ledger::new(memory, &mut || Ok(())).unwrap()
}

/// [`crate::read_debug`] of `elf` without a work budget.
pub(crate) fn debug(elf: &[u8]) -> (Dwarf, TypeFacts) {
    crate::read_debug(
        &std::rc::Rc::from(elf),
        DebugMetadata::Standard,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap()
}

/// [`crate::analyze`] of `elf` and its `companions` under `memory` and `control`.
fn analyze_within(
    elf: &[u8],
    companions: &[&[u8]],
    summaries: &[Summary],
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<Analysis> {
    let image = ImageFacts::new(elf, &[], memory, control)?;
    let companions = companions
        .iter()
        .map(|companion| ImageFacts::new(companion, &[], memory, control))
        .collect::<Result<Vec<_>>>()?;
    let companions: Vec<&ImageFacts<'_>> = companions.iter().collect();
    let mut keep = Ledger::new(memory, control)?;
    let (dwarf, _) = crate::read_debug(
        &std::rc::Rc::from(elf),
        DebugMetadata::Standard,
        &mut keep,
        control,
    )?;
    crate::analyze(&image, &companions, summaries, &dwarf, &mut keep, control)
}

/// [`crate::analyze`] without a work budget.
fn analyze(elf: &[u8], companions: &[&[u8]], summaries: &[Summary]) -> Result<Analysis> {
    let memory = WorkingMemory::new(TEST_MEMORY)?;
    analyze_within(elf, companions, summaries, &memory, &mut || Ok(()))
}

fn functions(elf: &[u8]) -> Result<Vec<Function>> {
    with_facts(elf, |facts| Ok(facts.functions().to_vec()))
}

fn stack_sizes(elf: &[u8]) -> Result<BTreeMap<u32, u64>> {
    with_facts(elf, |facts| {
        crate::stack_sizes(facts, &mut ledger(), &mut || Ok(()))
    })
}

fn taken_addresses(elf: &[u8]) -> Result<BTreeSet<u32>> {
    with_facts(elf, |facts| {
        crate::taken_addresses(facts, &mut ledger(), &mut || Ok(()))
    })
}

fn address_taken(elf: &[u8], function: u32) -> Result<bool> {
    Ok(taken_addresses(elf)?.contains(&function))
}

fn vector_table(elf: &[u8], symbol: &str) -> Result<Vec<Option<u32>>> {
    with_facts(elf, |facts| {
        crate::vector_table(facts, symbol, &mut || Ok(()))
    })
}

fn trap_entry(elf: &[u8], entry: u32) -> Result<TrapEntry> {
    with_facts(elf, |facts| crate::trap_entry(facts, entry, &mut || Ok(())))
}

fn interrupt_table(elf: &[u8], symbol: &str, layout: &TableLayout) -> Result<Vec<TableEntry>> {
    with_facts(elf, |facts| {
        crate::interrupt_table(facts, symbol, layout, &mut || Ok(()))
    })
}

fn swept(elf: &[u8]) -> Result<sweep::Swept> {
    with_facts(elf, |facts| {
        sweep::transfers(facts, &facts.functions()[0], &mut ledger(), &mut || Ok(()))
    })
}

const SP_DOWN_16: u32 = 0xff01_0113; // addi sp, sp, -16
const SAVE_RA: u32 = 0x0011_2623; // sw ra, 12(sp)
const LOAD_RA: u32 = 0x00c1_2083; // lw ra, 12(sp)
const SP_UP_16: u32 = 0x0101_0113; // addi sp, sp, 16
const SP_DOWN_32: u32 = 0xfe01_0113; // addi sp, sp, -32
const SP_UP_32: u32 = 0x0201_0113; // addi sp, sp, 32
const RET: u32 = 0x0000_8067; // jalr zero, 0(ra)
const LOAD_SLOT: u32 = 0x0001_2783; // lw a5, 0(sp)
const CALL_A5: u32 = 0x0007_80e7; // jalr ra, 0(a5)
const TEXT: u32 = 0x1000;
const BRANCH_NEXT: u32 = 0x0000_0263; // beq zero, zero, 4

/// `jal ra, target` at `site`.
fn call(site: u32, target: u32) -> u32 {
    let offset = target.wrapping_sub(site);
    ((offset >> 20 & 1) << 31)
        | ((offset >> 1 & 0x3ff) << 21)
        | ((offset >> 11 & 1) << 20)
        | ((offset >> 12 & 0xff) << 12)
        | (1 << 7)
        | 0x6f
}

struct Symbol {
    name: &'static str,
    words: Vec<u32>,
    /// `.stack_sizes` record, if any.
    frame: Option<u8>,
}

const RODATA: u32 = 0x2000;

/// A static RV32 executable with one loaded `.text` holding the symbols in
/// order from `TEXT`, a symbol table and `.stack_sizes`.
fn executable(symbols: &[Symbol]) -> Vec<u8> {
    image(symbols, &[], false)
}

/// [`executable`] with `rodata` words in a `.rodata` at `RODATA`, covered by
/// a sized `table` data symbol when `sized`.
fn image(symbols: &[Symbol], rodata: &[u32], sized: bool) -> Vec<u8> {
    placed(TEXT, symbols, rodata, sized, false)
}

/// [`image`] whose `.rodata` is a compiler jump table: a `.LJTI0_0` label and
/// an `R_RISCV_32` relocation for each word, as `--emit-relocs` keeps them.
fn labelled(symbols: &[Symbol], rodata: &[u32]) -> Vec<u8> {
    placed(TEXT, symbols, rodata, false, true)
}

/// [`image`] with its `.text` at `text` instead of `TEXT`, and a labelled
/// jump table when `labelled`.
fn placed(
    text_base: u32,
    symbols: &[Symbol],
    rodata_words: &[u32],
    sized: bool,
    labelled: bool,
) -> Vec<u8> {
    let mut objects = Vec::new();
    if labelled {
        objects.push((".LJTI0_0", 0, 0));
    }
    if sized {
        objects.push(("table", 0, 4 * rodata_words.len() as u32));
    }
    let relocated: Vec<usize> = if labelled {
        (0..rodata_words.len()).collect()
    } else {
        Vec::new()
    };
    built(
        text_base,
        symbols,
        &Rodata {
            words: rodata_words,
            relocated: &relocated,
            objects: &objects,
        },
    )
}

/// `.rodata` words at `RODATA`: which of them carry an `R_RISCV_32`
/// relocation (by index), and its symbols as (name, byte offset, size); a
/// size of zero is a label.
struct Rodata<'a> {
    words: &'a [u32],
    relocated: &'a [usize],
    objects: &'a [(&'static str, u32, u32)],
}

/// An executable of `symbols` at `text_base` and `rodata`.
fn built(text_base: u32, symbols: &[Symbol], rodata: &Rodata<'_>) -> Vec<u8> {
    let rodata_words = rodata.words;
    let objects = rodata.objects;
    let relocated = rodata.relocated;
    let rodata: Vec<u8> = rodata_words
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let mut text = Vec::new();
    let mut entries = Vec::new();
    let mut sizes = Vec::new();
    for symbol in symbols {
        let address = text_base + text.len() as u32;
        for word in &symbol.words {
            text.extend_from_slice(&word.to_le_bytes());
        }
        entries.push((symbol.name, address, 4 * symbol.words.len() as u32));
        if let Some(frame) = symbol.frame {
            sizes.extend_from_slice(&address.to_le_bytes());
            sizes.push(frame);
        }
    }
    let mut strtab = vec![0u8];
    let mut symtab = vec![0u8; 16];
    let code_symbols = entries.len();
    for &(name, offset, size) in objects {
        entries.push((name, RODATA + offset, size));
    }
    // Relocations with no symbol: the addend is the address.
    let mut rela = Vec::new();
    for &i in relocated {
        rela.extend_from_slice(&(RODATA + 4 * i as u32).to_le_bytes());
        rela.extend_from_slice(&1u32.to_le_bytes()); // R_RISCV_32
        rela.extend_from_slice(&rodata_words[i].to_le_bytes());
    }
    for (index, (name, address, size)) in entries.iter().enumerate() {
        let offset = strtab.len() as u32;
        strtab.extend_from_slice(name.as_bytes());
        strtab.push(0);
        symtab.extend_from_slice(&offset.to_le_bytes());
        symtab.extend_from_slice(&address.to_le_bytes());
        symtab.extend_from_slice(&size.to_le_bytes());
        let data = index >= code_symbols;
        // STB_GLOBAL with STT_NOTYPE for a label, STT_OBJECT or STT_FUNC.
        symtab.push(match (*size, data) {
            (0, true) => 0x10,
            (_, true) => 0x11,
            _ => 0x12,
        });
        symtab.push(0);
        symtab.extend_from_slice(&(if data { 6u16 } else { 1 }).to_le_bytes()); // .rodata or .text
    }
    let shstrtab = b"\0.text\0.stack_sizes\0.symtab\0.strtab\0.shstrtab\0.rodata\0.rela.rodata\0";
    // Layout: ELF header, one program header, then the section contents.
    let mut out = vec![0u8; 52 + 2 * 32];
    let place = |out: &mut Vec<u8>, bytes: &[u8]| {
        while !out.len().is_multiple_of(4) {
            out.push(0);
        }
        let offset = out.len() as u32;
        out.extend_from_slice(bytes);
        offset
    };
    let text_offset = place(&mut out, &text);
    let sizes_offset = place(&mut out, &sizes);
    let symtab_offset = place(&mut out, &symtab);
    let strtab_offset = place(&mut out, &strtab);
    let shstrtab_offset = place(&mut out, shstrtab);
    let rodata_offset = place(&mut out, &rodata);
    let rela_offset = place(&mut out, &rela);
    while !out.len().is_multiple_of(4) {
        out.push(0);
    }
    let sections = out.len() as u32;
    let header = |name: u32,
                  kind: u32,
                  flags: u32,
                  address: u32,
                  offset: u32,
                  size: u32,
                  link: u32,
                  info: u32,
                  align: u32,
                  entry: u32| {
        [
            name, kind, flags, address, offset, size, link, info, align, entry,
        ]
    };
    let headers = [
        [0; 10],
        header(
            1,
            1,
            6,
            text_base,
            text_offset,
            text.len() as u32,
            0,
            0,
            4,
            0,
        ),
        header(7, 1, 0, 0, sizes_offset, sizes.len() as u32, 0, 0, 1, 0),
        header(20, 2, 0, 0, symtab_offset, symtab.len() as u32, 4, 1, 4, 16),
        header(28, 3, 0, 0, strtab_offset, strtab.len() as u32, 0, 0, 1, 0),
        header(
            36,
            3,
            0,
            0,
            shstrtab_offset,
            shstrtab.len() as u32,
            0,
            0,
            1,
            0,
        ),
        header(
            46,
            1,
            2,
            RODATA,
            rodata_offset,
            rodata.len() as u32,
            0,
            0,
            4,
            0,
        ),
        // SHT_RELA with SHF_INFO_LINK for .rodata (6), symbols in .symtab (3).
        header(54, 4, 0x40, 0, rela_offset, rela.len() as u32, 3, 6, 4, 12),
    ];
    for fields in headers {
        for field in fields {
            out.extend_from_slice(&field.to_le_bytes());
        }
    }
    // ELF header: ELFCLASS32, little endian, ET_EXEC, EM_RISCV, soft float.
    let mut elf = Vec::new();
    elf.extend_from_slice(b"\x7fELF\x01\x01\x01\0\0\0\0\0\0\0\0\0");
    elf.extend_from_slice(&2u16.to_le_bytes());
    elf.extend_from_slice(&243u16.to_le_bytes());
    for word in [1u32, text_base, 52, sections, 0] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    for half in [52u16, 32, 2, 40, 8, 5] {
        elf.extend_from_slice(&half.to_le_bytes());
    }
    // PT_LOAD of .text, readable and executable.
    for word in [
        1u32,
        text_offset,
        text_base,
        text_base,
        text.len() as u32,
        text.len() as u32,
        5,
        4,
    ] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    // PT_LOAD of .rodata, readable.
    for word in [
        1u32,
        rodata_offset,
        RODATA,
        RODATA,
        rodata.len() as u32,
        rodata.len() as u32,
        4,
        4,
    ] {
        elf.extend_from_slice(&word.to_le_bytes());
    }
    out[..116].copy_from_slice(&elf);
    out
}

fn leaf(frame: Option<u8>) -> Symbol {
    Symbol {
        name: "leaf",
        words: vec![SP_DOWN_32, SP_UP_32, RET],
        frame,
    }
}

#[test]
fn a_callee_runs_below_its_call_site_depth() {
    // root: 16 bytes, calls leaf (32 bytes) at depth 16.
    let leaf_address = TEXT + 24;
    let image = executable(&[
        Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, leaf_address),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        },
        leaf(Some(32)),
    ]);
    let analysis = analyze(&image, &[], &[]).unwrap();
    let root = &analysis.functions[&TEXT];
    assert_eq!(root.frame, Some(16));
    assert_eq!(root.source, Some(FrameSource::StackSizes));
    assert_eq!(root.observed, Some(16));
    let bound = analysis.bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert!(bound.unresolved.is_empty());
    assert_eq!(bound.path, [(TEXT, 16), (leaf_address, 32)]);
}

#[test]
fn an_unresolved_call_leaves_the_bound_unknown_with_its_reason() {
    // root loads a pointer from its stack slot and calls through it.
    let image = executable(&[Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
        ],
        frame: Some(16),
    }]);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.unresolved, [(TEXT + 12, Reason::StackSlotCall)]);
    assert_eq!(
        bound.reasons(),
        BTreeMap::from([(Reason::StackSlotCall, 1)])
    );
}

#[test]
fn a_resolution_gives_an_unresolved_site_each_of_its_targets() {
    // root: frame 16, an indirect call through a5 at offset 8.
    let root = Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
        ],
        frame: Some(16),
    };
    let small = Symbol {
        name: "small",
        words: vec![SP_DOWN_16, SP_UP_16, RET],
        frame: Some(16),
    };
    let large = Symbol {
        name: "large",
        words: vec![SP_DOWN_32, SP_UP_32, RET],
        frame: Some(32),
    };
    let elf = executable(&[root, small, large]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let site = TEXT + 12;
    let small = TEXT + 7 * 4;
    let large = small + 3 * 4;
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, None);
    let mut resolutions = Resolutions::new();
    resolutions.add(site, Fact::FieldType, [small, large]);
    let bound = analysis.bound_with(TEXT, &resolutions).unwrap();
    assert_eq!(bound.bytes, Some(16 + 32));
    assert_eq!(bound.path.last(), Some(&(large, 32)));
    // A site the analysis resolved keeps its own target.
    let mut only_small = Resolutions::new();
    only_small.add(site, Fact::FieldType, [small]);
    assert_eq!(
        analysis.bound_with(TEXT, &only_small).unwrap().bytes,
        Some(32)
    );
}

/// An empty set resolves a site only where its fact proves the site reaches
/// nothing; otherwise the site is a hole, and the bound keeps the resolved
/// part as `partial + ?`.
#[test]
fn an_empty_candidate_set_is_a_hole_unless_its_fact_proves_it() {
    let root = Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16, SAVE_RA, LOAD_SLOT, CALL_A5, LOAD_RA, SP_UP_16, RET,
        ],
        frame: Some(16),
    };
    let elf = executable(&[root]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let site = TEXT + 12;
    let mut found = Resolutions::new();
    found.add(site, Fact::WakerVtables, []);
    let hole = analysis.bound_with(TEXT, &found).unwrap();
    assert_eq!(hole.bytes, None);
    assert_eq!(hole.partial, 16);
    assert_eq!(hole.unresolved, [(site, Reason::NoCandidate)]);
    let mut proven = Resolutions::new();
    proven.add(site, Fact::IpcPosts, []);
    assert_eq!(analysis.bound_with(TEXT, &proven).unwrap().bytes, Some(16));
    // Facts unite: a field type's candidate joins a found-empty waker set.
    found.add(site, Fact::FieldType, [TEXT]);
    assert_eq!(found.get(site).unwrap().targets, BTreeSet::from([TEXT]));
}

#[test]
fn recursion_leaves_the_bound_unknown() {
    let image = executable(&[Symbol {
        name: "root",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            call(TEXT + 8, TEXT),
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    }]);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::Recursion, 1)]));
}

#[test]
fn an_observed_depth_beyond_the_frame_record_fails() {
    let image = executable(&[leaf(Some(16))]);
    let error = analyze(&image, &[], &[]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_complete_graph_without_a_record_gives_the_observed_frame() {
    let analysis = analyze(&executable(&[leaf(None)]), &[], &[]).unwrap();
    let facts = &analysis.functions[&TEXT];
    assert_eq!(
        (facts.frame, facts.source),
        (Some(32), Some(FrameSource::Observed))
    );
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(32));
}

#[test]
fn frame_records_decode_with_uleb128_sizes() {
    let image = executable(&[leaf(Some(32))]);
    assert_eq!(stack_sizes(&image).unwrap(), BTreeMap::from([(TEXT, 32)]));
}

/// `slli a0, a0, 2; lui a2, %hi(RODATA); addi a2, a2, 0; add a0, a0, a2;
/// lw a0, 0(a0); jr a0`, then two returns as the table's arms; `bounded`
/// first checks the index: `li a1, 1; bltu a1, a0, <first arm>`.
fn dispatch(bounded: bool) -> Symbol {
    let check: &[u32] = if bounded {
        &[0x0010_0593, 0x00a5_ee63]
    } else {
        &[]
    };
    let mut words = check.to_vec();
    words.extend([
        0x0025_1513,
        0x0000_2637,
        0x0006_0613,
        0x00c5_0533,
        0x0005_2503,
        0x0005_0067,
        RET,
        RET,
    ]);
    Symbol {
        name: "dispatch",
        words,
        frame: Some(0),
    }
}

#[test]
fn a_bounded_jump_table_reads_exactly_its_entries() {
    // Two arms; the word after the table is not an entry.
    let image = image(&[dispatch(true)], &[TEXT + 32, TEXT + 36, 0x5000], false);
    assert_eq!(swept(&image).unwrap().transfers, []);
    assert_eq!(
        analyze(&image, &[], &[])
            .unwrap()
            .bound(TEXT)
            .unwrap()
            .bytes,
        Some(0)
    );
}

#[test]
fn a_bounded_table_entry_out_of_the_function_is_a_jump() {
    let image = image(&[dispatch(true)], &[TEXT + 32, 0x5000], false);
    let transfers: Vec<_> = swept(&image)
        .unwrap()
        .transfers
        .iter()
        .map(|t| (t.site, t.target, t.kind))
        .collect();
    assert_eq!(transfers, [(TEXT + 28, Some(0x5000), TransferKind::Tail)]);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::OutsideImage, 1)]));
}

#[test]
fn a_table_base_set_before_an_exit_is_found_by_its_dispatch_record() {
    // `lui s4, %hi(RODATA)`, a branch around an early return, then a
    // bounded dispatch through `s4`, as the ROM's `spi_slave_download`
    // does: the linear sweep forgets `s4` at the return, the value analysis
    // follows the branch, so only its `Dispatch` record locates the table.
    let arm = [LOAD_RA, SP_UP_16, RET];
    let mut words = vec![
        SP_DOWN_16,
        SAVE_RA,
        0x0000_2a37, // lui s4, 0x2
        0x0005_1863, // bnez a0, <the dispatch>
    ];
    words.extend(arm);
    words.extend([
        0x0010_0593, // li a1, 1
        0x00a5_ea63, // bltu a1, a0, <first arm>
        0x0025_1513, // slli a0, a0, 2
        0x0145_0533, // add a0, a0, s4
        0x0005_2503, // lw a0, 0(a0)
        0x0005_0067, // jr a0
    ]);
    words.extend(arm);
    words.extend(arm);
    let image = image(
        &[Symbol {
            name: "root",
            words,
            frame: Some(16),
        }],
        &[TEXT + 52, TEXT + 64],
        false,
    );
    assert!(swept(&image).unwrap().jumps.is_empty());
    let analysis = analyze(&image, &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].complete);
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(16));
}

#[test]
fn an_unbounded_jump_table_stays_unknown() {
    // Without a bound or a sized table object, entries past the readable
    // ones could leave the function.
    let image = image(&[dispatch(false)], &[TEXT + 24, TEXT + 28], false);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.unresolved, [(TEXT + 20, Reason::IndirectJump)]);
}

#[test]
fn a_jump_through_a_sized_table_object_keeps_only_entries_out_of_the_function() {
    let image = image(&[dispatch(false)], &[TEXT + 24, 0x5000], true);
    let analysis = analyze(&image, &[], &[]).unwrap();
    let jumps: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target))
        .collect();
    assert_eq!(jumps, [(TEXT + 20, Some(0x5000))]);
}

#[test]
fn a_call_through_a_constant_table_object_reaches_every_entry() {
    // root: lui s11, %hi(RODATA); addi s11, s11, 0; then, past a merge
    // where the sweep forgets the constant but the value analysis keeps it,
    // slli a0, a0, 2; add a0, a0, s11; lw a0, 0(a0); jalr a0, at depth 16.
    let (small, large) = (TEXT + 52, TEXT + 64);
    let image = image(
        &[
            Symbol {
                name: "root",
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    0x0000_2db7,
                    0x000d_8d93,
                    BRANCH_NEXT,
                    0x0025_1513,
                    0x01b5_0533,
                    0x0005_2503,
                    0x0005_00e7,
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                    RET,
                ],
                frame: Some(16),
            },
            Symbol {
                name: "small",
                words: vec![SP_DOWN_16, SP_UP_16, RET],
                frame: Some(16),
            },
            Symbol {
                name: "large",
                words: vec![SP_DOWN_32, SP_UP_32, RET],
                frame: Some(32),
            },
        ],
        &[small, large, small],
        true,
    );
    let analysis = analyze(&image, &[], &[]).unwrap();
    let calls: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target))
        .collect();
    assert_eq!(calls, [(TEXT + 32, Some(small)), (TEXT + 32, Some(large))]);
    let bound = analysis.bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert_eq!(bound.path, [(TEXT, 16), (large, 32)]);
}

#[test]
fn a_table_base_rewritten_before_the_call_stays_unresolved() {
    // As above, but `addi s11, s11, 4` after the indexing `add`.
    let image = image(
        &[Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                0x0000_2db7,
                0x000d_8d93,
                BRANCH_NEXT,
                0x0025_1513,
                0x01b5_0533,
                0x004d_8d93,
                0x0005_2503,
                0x0005_00e7,
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        }],
        &[TEXT, TEXT],
        true,
    );
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, None);
    assert_eq!(bound.reasons(), BTreeMap::from([(Reason::LoadedCall, 1)]));
}

#[test]
fn analysed_jump_targets_inside_the_function_are_not_transfers() {
    // li a0, 0; beqz a1, 1f; li a0, 1; 1: slli a0, a0, 2; lui a2,
    // %hi(RODATA); addi a2, a2, 0; add a0, a0, a2; lw a0, 0(a0); jr a0;
    // ret. The value analysis knows the index is 0 or 1: entry 0 is the
    // local `ret`, entry 1 the function `leaf`.
    let leaf_address = TEXT + 40;
    let image = image(
        &[
            Symbol {
                name: "dispatch",
                words: vec![
                    0x0000_0513,
                    0x0005_8463,
                    0x0010_0513,
                    0x0025_1513,
                    0x0000_2637,
                    0x0006_0613,
                    0x00c5_0533,
                    0x0005_2503,
                    0x0005_0067,
                    RET,
                ],
                frame: Some(0),
            },
            leaf(Some(32)),
        ],
        &[TEXT + 36, leaf_address],
        false,
    );
    let analysis = analyze(&image, &[], &[]).unwrap();
    let jumps: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target, t.kind))
        .collect();
    assert_eq!(jumps, [(TEXT + 32, Some(leaf_address), TransferKind::Tail)]);
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(32));
}

#[test]
fn a_call_into_a_companion_runs_below_its_site() {
    // root calls a function of a ROM-like companion ELF at 0x9000, which
    // has no .stack_sizes: its frame is the observed one.
    let rom = 0x9000;
    let image = image(
        &[Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, rom),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        }],
        &[],
        false,
    );
    let companion = placed(rom, &[leaf(None)], &[], false, false);
    let alone = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(alone.reasons(), BTreeMap::from([(Reason::OutsideImage, 1)]));
    let with_rom = analyze(&image, &[&companion], &[]).unwrap();
    assert_eq!(with_rom.functions[&rom].source, Some(FrameSource::Observed));
    let bound = with_rom.bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(48));
    assert_eq!(bound.path, [(TEXT, 16), (rom, 32)]);
}

#[test]
fn companions_may_not_overlap() {
    let image = executable(&[leaf(Some(32))]);
    let error = analyze(&image, &[&image], &[]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_zero_table_entry_is_an_empty_slot_not_a_call() {
    // The sized table holds `leaf` and an empty `Option<fn>` slot.
    let leaf_address = TEXT + 48 + 4;
    let image = image(
        &[
            Symbol {
                name: "root",
                words: vec![
                    SP_DOWN_16,
                    SAVE_RA,
                    0x0000_2db7,
                    0x000d_8d93,
                    BRANCH_NEXT,
                    0x0025_1513,
                    0x01b5_0533,
                    0x0005_2503,
                    0x0005_00e7,
                    LOAD_RA,
                    SP_UP_16,
                    RET,
                    RET,
                ],
                frame: Some(16),
            },
            leaf(Some(32)),
        ],
        &[leaf_address, 0],
        true,
    );
    let analysis = analyze(&image, &[], &[]).unwrap();
    let calls: Vec<_> = analysis.functions[&TEXT]
        .transfers
        .iter()
        .map(|t| (t.site, t.target))
        .collect();
    assert_eq!(calls, [(TEXT + 32, Some(leaf_address))]);
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(48));
}

#[test]
fn a_global_untyped_code_label_is_a_function() {
    // A companion whose only symbol is an untyped global label, as the ROM's
    // `__call_*` trampolines are, reached by `j` to the real function.
    let rom = 0x9000;
    let mut companion = placed(rom, &[leaf(None)], &[], false, false);
    // Turn the symbol `leaf` into STB_GLOBAL, STT_NOTYPE with no size.
    let mut record = rom.to_le_bytes().to_vec();
    record.extend_from_slice(&12u32.to_le_bytes());
    record.push(0x12);
    let symtab = companion
        .windows(record.len())
        .position(|w| w == record)
        .expect("leaf's symbol entry");
    companion[symtab + 4..symtab + 8].copy_from_slice(&0u32.to_le_bytes());
    companion[symtab + 8] = 0x10;
    let functions = functions(&companion).unwrap();
    assert_eq!(functions.len(), 1);
    assert_eq!((functions[0].address, functions[0].size), (rom, 12));
}

#[test]
fn a_labelled_jump_table_is_read_from_its_relocations() {
    // No bounds check: the relocations alone name the table's entries.
    let image = labelled(&[dispatch(false)], &[TEXT + 24, TEXT + 28]);
    let swept = swept(&image).unwrap();
    assert_eq!(swept.transfers, []);
    assert_eq!(swept.jumps, [(TEXT + 20, vec![TEXT + 24, TEXT + 28])]);
    let bound = analyze(&image, &[], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(bound.bytes, Some(0));
}

#[test]
fn a_relocation_that_disagrees_with_its_word_fails() {
    let mut image = labelled(&[dispatch(false)], &[TEXT + 24, TEXT + 28]);
    let mut entry = RODATA.to_le_bytes().to_vec();
    entry.extend_from_slice(&1u32.to_le_bytes());
    entry.extend_from_slice(&(TEXT + 24).to_le_bytes());
    let at = image
        .windows(entry.len())
        .position(|w| w == entry)
        .expect("the first entry's relocation");
    image[at + 8..at + 12].copy_from_slice(&(TEXT + 20).to_le_bytes());
    let error = analyze(&image, &[], &[]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_labelled_table_completes_the_control_flow_graph() {
    // The table's entries become edges of the value analysis's graph, so its
    // arms are analysed; without them the graph stops at the jump.
    let table = [TEXT + 24, TEXT + 28];
    let known = analyze(&labelled(&[dispatch(false)], &table), &[], &[]).unwrap();
    assert!(known.functions[&TEXT].complete);
    let unknown = analyze(&image(&[dispatch(false)], &table, false), &[], &[]).unwrap();
    assert!(!unknown.functions[&TEXT].complete);
}

/// An image whose `root` calls the companion's function at `ROM`, which
/// jumps through an unlabelled table it cannot bound.
const ROM: u32 = 0x9000;

fn calls_rom() -> Vec<u8> {
    image(
        &[Symbol {
            name: "root",
            words: vec![
                SP_DOWN_16,
                SAVE_RA,
                call(TEXT + 8, ROM),
                LOAD_RA,
                SP_UP_16,
                RET,
            ],
            frame: Some(16),
        }],
        &[],
        false,
    )
}

fn summary(name: &str, address: u32, frame: u64) -> Summary {
    Summary {
        name: name.into(),
        address,
        frame,
        calls: Vec::new(),
    }
}

#[test]
fn a_summary_stands_for_a_companion_function_the_code_does_not_bound() {
    let rom = placed(ROM, &[dispatch(false)], &[], false, false);
    let image = calls_rom();
    let alone = analyze(&image, &[&rom], &[]).unwrap().bound(TEXT).unwrap();
    assert_eq!(alone.bytes, None);
    let analysis = analyze(&image, &[&rom], &[summary("dispatch", ROM, 0)]).unwrap();
    assert_eq!(analysis.functions[&ROM].source, Some(FrameSource::Summary));
    assert_eq!(analysis.summaries, BTreeSet::from(["dispatch".to_owned()]));
    assert_eq!(analysis.bound(TEXT).unwrap().bytes, Some(16));
}

#[test]
fn a_summary_must_agree_with_code_the_analysis_bounds() {
    let rom = placed(ROM, &[leaf(None)], &[], false, false);
    let image = calls_rom();
    assert!(analyze(&image, &[&rom], &[summary("leaf", ROM, 32)]).is_ok());
    let error = analyze(&image, &[&rom], &[summary("leaf", ROM, 16)]).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity);
}

#[test]
fn a_summary_must_name_its_function_s_address() {
    let rom = placed(ROM, &[leaf(None)], &[], false, false);
    let image = calls_rom();
    for wrong in [summary("leaf", ROM + 4, 32), summary("other", ROM, 32)] {
        let error = analyze(&image, &[&rom], &[wrong]).unwrap_err();
        assert_eq!(error.code, ErrorCode::Integrity);
    }
}

#[test]
fn a_summary_the_image_does_not_reach_is_unused() {
    // Another image of the platform may reach it; the platform's audit
    // fails on a summary no image uses.
    let rom = placed(ROM, &[leaf(None)], &[], false, false);
    let image = executable(&[leaf(Some(32))]);
    let analysis = analyze(&image, &[&rom], &[summary("leaf", ROM, 32)]).unwrap();
    assert!(analysis.summaries.is_empty());
    assert_eq!(analysis.functions[&ROM].source, Some(FrameSource::Observed));
}

#[test]
fn only_data_neither_writable_nor_executable_reads_as_constant() {
    let elf = image(
        &[Symbol {
            name: "f",
            words: vec![RET],
            frame: Some(0),
        }],
        &[0x1234_5678],
        true,
    );
    let words = with_facts(&elf, |facts| {
        Ok([facts.read_only_word(RODATA), facts.read_only_word(TEXT)])
    })
    .unwrap();
    // Code loaded into RAM may hold a table the program rewrites.
    assert_eq!(words, [Some(0x1234_5678), None]);
}

/// The platform's interrupt entry up to its handler call
/// (the staged runtime's `PSRAM_TRAP_ENTER`).
const PLATFORM_ENTRY: [u32; 35] = [
    0x3401_1173,
    0x3402_92f3,
    0x0251_6063,
    0x0051_4133,
    0x0051_42b3,
    0x0051_4133,
    0xfb01_0113,
    0x0061_2423,
    0x0401_2023,
    0x0140_006f,
    0xfb01_0113,
    0x0061_2423,
    0x0010_0313,
    0x0461_2023,
    0x0011_2023,
    0x3400_2373,
    0x0061_2223,
    0x0071_2623,
    0x01c1_2823,
    0x01d1_2a23,
    0x01e1_2c23,
    0x01f1_2e23,
    0x02a1_2023,
    0x02b1_2223,
    0x02c1_2423,
    0x02d1_2623,
    0x02e1_2823,
    0x02f1_2a23,
    0x0301_2c23,
    0x0311_2e23,
    0x3000_2373,
    0x0461_2223,
    0x0000_6337,
    0x3003_3073,
    0x3402_9073,
];

/// `entry` words, then `la a0, handler; j dispatch`, a `dispatch` that
/// calls `a0` and a `handler` that returns.
fn trap_image(entry: &[u32]) -> (Vec<u8>, u32) {
    let auipc = TEXT + 4 * entry.len() as u32;
    let dispatch = auipc + 12;
    let handler = dispatch + 4;
    let mut words = entry.to_vec();
    words.push(0x0000_0517); // auipc a0, 0
    words.push(((handler - auipc) << 20) | (10 << 15) | (10 << 7) | 0x13); // addi a0, a0
    words.push(call(auipc + 8, dispatch) - (1 << 7)); // jal zero, dispatch
    let elf = executable(&[
        Symbol {
            name: "entry",
            words,
            frame: None,
        },
        Symbol {
            name: "dispatch",
            words: vec![0x0005_00e7], // jalr ra, 0(a0)
            frame: None,
        },
        Symbol {
            name: "handler",
            words: vec![RET],
            frame: Some(0),
        },
    ]);
    (elf, handler)
}

#[test]
fn the_platform_trap_entry_keeps_its_frame_on_the_interrupt_stack() {
    let (elf, handler) = trap_image(&PLATFORM_ENTRY);
    assert_eq!(
        trap_entry(&elf, TEXT).unwrap(),
        TrapEntry {
            entry: TEXT,
            frame: 80,
            handler,
        }
    );
}

#[test]
fn a_trap_entry_must_move_sp_before_its_first_memory_access() {
    let mut entry = vec![0x0011_2023]; // sw ra, 0(sp)
    entry.extend(PLATFORM_ENTRY);
    let (elf, _) = trap_image(&entry);
    let error = trap_entry(&elf, TEXT).unwrap_err();
    assert!(error.message.contains("outside its frame"), "{error:?}");
}

#[test]
fn a_trap_entry_learns_the_interrupt_stack_only_from_comparing_both_stacks() {
    // csrrw sp, mscratch, sp; addi sp, sp, -16; sw ra, 0(sp): mscratch may
    // hold the interrupted task's sp in a nested trap.
    let (elf, _) = trap_image(&[0x3401_1173, SP_DOWN_16, 0x0011_2023]);
    assert!(trap_entry(&elf, TEXT).is_err());
}

#[test]
fn a_vector_table_names_its_entries_by_relocations() {
    let elf = placed(
        TEXT,
        &[Symbol {
            name: "f",
            words: vec![RET],
            frame: Some(0),
        }],
        &[TEXT, TEXT],
        true,
        true,
    );
    assert_eq!(
        vector_table(&elf, "table").unwrap(),
        [Some(TEXT), Some(TEXT)]
    );
    assert!(vector_table(&elf, "missing").is_err());
}

fn binding(source: u16, level: u8, core: u32) -> [u32; 2] {
    [u32::from(source) | u32::from(level) << 16, core]
}

/// An image whose `__OER_INTERRUPT_TABLE` lists `entries` (source, level,
/// core, handler symbol index or none) in the staged runtime's binding layout.
fn table_image(entries: &[(u16, u8, u32, Option<u32>)], relocate_handlers: bool) -> Vec<u8> {
    let handler = Symbol {
        name: "handler",
        words: vec![SP_DOWN_16, SP_UP_16, RET],
        frame: Some(16),
    };
    let mut words = vec![RODATA + 8, entries.len() as u32];
    let mut relocated = vec![0];
    for &(source, level, core, target) in entries {
        words.extend(binding(source, level, core));
        if target.is_some() && relocate_handlers {
            relocated.push(words.len());
        }
        words.push(target.unwrap_or(0));
    }
    built(
        TEXT,
        &[handler],
        &Rodata {
            words: &words,
            relocated: &relocated,
            objects: &[
                ("__OER_INTERRUPT_TABLE", 0, 8),
                ("entries", 8, 12 * entries.len() as u32),
            ],
        },
    )
}

const S31_TABLE: TableLayout = TableLayout {
    entry: 12,
    source: Field { offset: 0, size: 2 },
    level: Field { offset: 2, size: 1 },
    core: Field { offset: 4, size: 4 },
    handler: 8,
};

#[test]
fn the_interrupt_table_reads_each_binding_and_its_relocated_handler() {
    let elf = table_image(&[(25, 1, 0, Some(TEXT)), (67, 8, 1, None)], true);
    let entries = interrupt_table(&elf, "__OER_INTERRUPT_TABLE", &S31_TABLE).unwrap();
    assert_eq!(
        entries,
        [
            TableEntry {
                source: 25,
                level: 1,
                core: 0,
                handler: Some(TEXT)
            },
            TableEntry {
                source: 67,
                level: 8,
                core: 1,
                handler: None
            },
        ]
    );
}

#[test]
fn an_empty_table_needs_no_pointer_relocation() {
    let elf = built(
        TEXT,
        &[Symbol {
            name: "handler",
            words: vec![SP_DOWN_16, SP_UP_16, RET],
            frame: Some(16),
        }],
        &Rodata {
            words: &[4, 0],
            relocated: &[],
            objects: &[("__OER_INTERRUPT_TABLE", 0, 8)],
        },
    );
    assert_eq!(
        interrupt_table(&elf, "__OER_INTERRUPT_TABLE", &S31_TABLE).unwrap(),
        []
    );
    // A table with entries still needs its pointer relocated.
    let mut words = vec![RODATA + 8, 1];
    words.extend(binding(25, 1, 0));
    words.push(0);
    let elf = built(
        TEXT,
        &[],
        &Rodata {
            words: &words,
            relocated: &[],
            objects: &[("__OER_INTERRUPT_TABLE", 0, 8), ("entries", 8, 12)],
        },
    );
    assert!(interrupt_table(&elf, "__OER_INTERRUPT_TABLE", &S31_TABLE).is_err());
}

#[test]
fn a_corrupt_table_length_fails_on_the_table_not_the_budget() {
    let mut elf = table_image(&[(25, 1, 0, Some(TEXT))], true);
    let at = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        let table = file.symbol("__OER_INTERRUPT_TABLE").unwrap();
        let section = file.section(table.section.unwrap()).unwrap();
        (section.file_offset + table.address + 4 - section.address) as usize
    };
    elf[at..at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    let error = with_facts(&elf, |facts| {
        crate::interrupt_table(facts, "__OER_INTERRUPT_TABLE", &S31_TABLE, &mut Units(1000))
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity, "{error:?}");
}

#[test]
fn a_handler_word_without_its_relocation_fails() {
    let elf = table_image(&[(25, 1, 0, Some(TEXT))], false);
    assert!(interrupt_table(&elf, "__OER_INTERRUPT_TABLE", &S31_TABLE).is_err());
    assert!(interrupt_table(&elf, "__MISSING", &S31_TABLE).is_err());
}

/// `lui a1, upper` then `addi a1, a1, lower`: `a1 = value`.
fn load_a1(value: u32) -> [u32; 2] {
    let lower = value & 0xfff;
    let upper = value.wrapping_add(0x800) & 0xffff_f000;
    [
        upper | (11 << 7) | 0x37,
        (lower << 20) | (11 << 15) | (11 << 7) | 0x13,
    ]
}

#[test]
fn constant_arguments_collect_a_register_at_every_direct_call() {
    let callee = TEXT + 4 * 12;
    let [lui, addi] = load_a1(0x1234);
    let [lui2, addi2] = load_a1(0x5678);
    let caller = Symbol {
        name: "caller",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            lui,
            addi,
            call(TEXT + 16, callee),
            lui2,
            addi2,
            call(TEXT + 28, callee),
            LOAD_RA,
            SP_UP_16,
            RET,
            BRANCH_NEXT,
        ],
        frame: Some(16),
    };
    let elf = executable(&[
        caller,
        Symbol {
            name: "callee",
            words: vec![RET],
            frame: Some(0),
        },
    ]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert_eq!(
        analysis.constant_arguments(callee, 1).unwrap(),
        BTreeSet::from([0x1234, 0x5678])
    );
    // a2 is never set: not exact.
    assert!(analysis.constant_arguments(callee, 2).is_err());
    assert!(!address_taken(&elf, callee).unwrap());
}

#[test]
fn a_relocated_data_word_takes_a_function_s_address() {
    let elf = labelled(&[leaf(Some(32))], &[TEXT]);
    assert!(address_taken(&elf, TEXT).unwrap());
    assert!(!address_taken(&elf, TEXT + 4).unwrap());
}

#[test]
fn a_hart_stacks_one_interrupt_per_level_and_an_exception() {
    // lui a5, 3 (the source table at 0x3000); add a5, a5, a0; lw a5, 0(a5).
    const LUI_SOURCES: u32 = (3 << 12) | (15 << 7) | 0x37;
    const ADD_INDEX: u32 = (10 << 20) | (15 << 15) | (15 << 7) | 0x33;
    const LOAD_ENTRY: u32 = (15 << 15) | (2 << 12) | (15 << 7) | 0x03;
    // slli a0, a0, 2: the source number scaled to a word index.
    const SCALE_INDEX: u32 = (2 << 20) | (10 << 15) | (1 << 12) | (10 << 7) | 0x13;
    let dispatcher = Symbol {
        name: "dispatcher",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            LUI_SOURCES,
            SCALE_INDEX,
            ADD_INDEX,
            LOAD_ENTRY,
            CALL_A5,
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    };
    let small = Symbol {
        name: "small",
        words: vec![SP_DOWN_16, SP_UP_16, RET],
        frame: Some(16),
    };
    let large = Symbol {
        name: "large",
        words: vec![SP_DOWN_32, SP_UP_32, RET],
        frame: Some(32),
    };
    let elf = executable(&[dispatcher, small, large]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dispatcher, small, large) = (TEXT, TEXT + 40, TEXT + 52);
    let entry = |level, core, handler| TableEntry {
        source: 0,
        level,
        core,
        handler: Some(handler),
    };
    let table = [entry(1, 0, small), entry(8, 0, large), entry(1, 1, small)];
    let vector = TrapEntry {
        entry: 0x100,
        frame: 80,
        handler: dispatcher,
    };
    let exception = TrapEntry {
        entry: 0x200,
        frame: 80,
        handler: small,
    };
    let harts = interrupt_stacks(
        &analysis,
        &Stacks {
            table: &table,
            cores: &[0, 1],
            always: &[1],
            vectors: &[vector],
            exception,
            sources: 0x3000,
            resolutions: &Resolutions::new(),
        },
    )
    .unwrap();
    // Level 1: 80 + 16 + 16; level 8: 80 + 16 + 32; the exception 80 + 16.
    let levels = |hart: &HartStack| {
        hart.levels
            .iter()
            .map(|level| (level.level, level.bytes))
            .collect::<Vec<_>>()
    };
    assert_eq!(levels(&harts[0]), [(1, Some(112)), (8, Some(128))]);
    assert_eq!(harts[0].exception.bytes, Some(96));
    assert_eq!(harts[0].bytes, Some(112 + 128 + 96));
    // Hart 1 has no level-8 entry; its level 1 reaches the small handler only.
    assert_eq!(levels(&harts[1]), [(1, Some(112))]);
    assert_eq!(harts[1].bytes, Some(112 + 96));
}

const MRET: u32 = 0x3020_0073;
/// `csrrsi a0, mnxti, 8`: takes the next pending interrupt.
const TAKE_NEXT: u32 = 0x3454_6573;

#[test]
fn only_an_instruction_that_lowers_the_level_breaks_the_nesting_rule() {
    // The interrupt controller lets an interrupt preempt only above both the
    // threshold and the running level: lowering the threshold, setting MIE,
    // restoring mstatus after a critical section and reading mintstatus or
    // mnxti leave the running level.
    let benign = Symbol {
        name: "benign",
        words: vec![
            0x3470_1073, // csrw mintthresh, zero
            0x3004_6073, // csrsi mstatus, 8
            0x3005_1073, // csrw mstatus, a0
            0xfb10_2573, // csrr a0, mintstatus
            0x3450_2573, // csrr a0, mnxti
            RET,
        ],
        frame: Some(0),
    };
    let returns = Symbol {
        name: "returns",
        words: vec![MRET],
        frame: Some(0),
    };
    let next = Symbol {
        name: "next",
        words: vec![TAKE_NEXT, RET],
        frame: Some(0),
    };
    let (benign_at, returns_at, next_at) = (TEXT, TEXT + 24, TEXT + 28);
    let caller_at = TEXT + 36;
    let caller = Symbol {
        name: "caller",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            call(caller_at + 8, returns_at),
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    };
    let elf = executable(&[benign, returns, next, caller]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let drops = |root| analysis.bound(root).unwrap().level_drops;
    assert_eq!(drops(benign_at), []);
    assert_eq!(drops(returns_at), [(returns_at, LevelDrop::Return)]);
    assert_eq!(drops(next_at), [(next_at, LevelDrop::NextInterrupt)]);
    // A handler reaches a drop through its calls.
    assert_eq!(drops(caller_at), [(returns_at, LevelDrop::Return)]);
}

#[test]
fn a_trap_entry_must_not_lower_the_level_before_its_handler() {
    let mut entry = PLATFORM_ENTRY.to_vec();
    entry.push(TAKE_NEXT);
    let (elf, _) = trap_image(&entry);
    let error = trap_entry(&elf, TEXT).unwrap_err();
    assert!(
        error.message.contains("lowers the interrupt level"),
        "{error:?}"
    );
}

#[test]
fn a_bound_assumes_what_the_resolutions_it_takes_assume() {
    // `caller` calls through a5; the site resolves to `callee`.
    let caller = Symbol {
        name: "caller",
        words: vec![SP_DOWN_16, SAVE_RA, CALL_A5, LOAD_RA, SP_UP_16, RET],
        frame: Some(16),
    };
    let callee = Symbol {
        name: "callee",
        words: vec![RET],
        frame: Some(0),
    };
    let elf = executable(&[caller, callee]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (caller, callee, site) = (TEXT, TEXT + 24, TEXT + 8);
    let assumed = |fact| {
        let mut resolutions = Resolutions::new();
        resolutions.add(site, fact, [callee]);
        analysis
            .bound_with(caller, &resolutions)
            .unwrap()
            .assumptions
    };
    // The interrupt table lists its handlers outright; a waker vtable found
    // from the image's types rests on the executor invariant.
    assert_eq!(assumed(Fact::InterruptTable), BTreeSet::new());
    assert_eq!(
        assumed(Fact::WakerVtables),
        BTreeSet::from([Assumption::ExecutorInvariant])
    );
}

#[test]
fn a_bound_names_what_it_reaches_and_each_function_its_floating_point() {
    // fadd.s fa0, fa0, fa1; frcsr a0 (csrr a0, fcsr).
    let float = Symbol {
        name: "float",
        words: vec![0x00b5_7553, 0x0030_2573, RET],
        frame: Some(0),
    };
    let caller_at = TEXT + 12;
    let caller = Symbol {
        name: "caller",
        words: vec![
            SP_DOWN_16,
            SAVE_RA,
            call(caller_at + 8, TEXT),
            LOAD_RA,
            SP_UP_16,
            RET,
        ],
        frame: Some(16),
    };
    let elf = executable(&[float, caller]);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert_eq!(analysis.functions[&TEXT].float_sites, [TEXT, TEXT + 4]);
    assert!(analysis.functions[&caller_at].float_sites.is_empty());
    assert_eq!(
        analysis.bound(caller_at).unwrap().reached,
        BTreeSet::from([TEXT, caller_at])
    );
}

/// A work budget of so many units.
struct Units(u64);

impl RunControl for Units {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(units)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "work budget exhausted"))?;
        Ok(())
    }
}

#[test]
fn the_working_memory_and_the_work_budget_bound_the_analysis() {
    let image = executable(&[leaf(None)]);
    // The facts alone fit; the analysis needs more than either limit.
    let needed = {
        let memory = WorkingMemory::new(TEST_MEMORY).unwrap();
        ImageFacts::new(&image, &[], &memory, &mut || Ok(())).unwrap();
        memory.peak()
    };
    let memory = WorkingMemory::new(needed + 1).unwrap();
    let facts = ImageFacts::new(&image, &[], &memory, &mut || Ok(())).unwrap();
    let error = crate::analyze(
        &facts,
        &[],
        &[],
        &UnknownCallBehavior,
        &mut kept(&memory),
        &mut || Ok(()),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    assert!(error.memory.is_some(), "{error:?}");

    let memory = WorkingMemory::new(TEST_MEMORY).unwrap();
    let facts = ImageFacts::new(&image, &[], &memory, &mut || Ok(())).unwrap();
    let error = crate::analyze(
        &facts,
        &[],
        &[],
        &UnknownCallBehavior,
        &mut kept(&memory),
        &mut Units(1),
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    assert!(
        crate::analyze(
            &facts,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut kept(&memory),
            &mut Units(u64::MAX)
        )
        .is_ok()
    );
}

#[test]
fn every_image_query_charges_its_reads_to_the_work_budget() {
    let limited = |error: Error| assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    let table = table_image(&[(25, 1, 0, Some(TEXT))], true);
    with_facts(&table, |facts| {
        limited(
            crate::interrupt_table(facts, "__OER_INTERRUPT_TABLE", &S31_TABLE, &mut Units(0))
                .unwrap_err(),
        );
        limited(crate::taken_addresses(facts, &mut ledger(), &mut Units(0)).unwrap_err());
        Ok(())
    })
    .unwrap();
    let (trap, _) = trap_image(&PLATFORM_ENTRY);
    with_facts(&trap, |facts| {
        limited(crate::trap_entry(facts, TEXT, &mut Units(0)).unwrap_err());
        Ok(())
    })
    .unwrap();
    let vectors = placed(
        TEXT,
        &[Symbol {
            name: "f",
            words: vec![RET],
            frame: Some(0),
        }],
        &[TEXT, TEXT],
        true,
        true,
    );
    with_facts(&vectors, |facts| {
        limited(crate::vector_table(facts, "table", &mut Units(0)).unwrap_err());
        Ok(())
    })
    .unwrap();
    let records = executable(&[leaf(Some(32))]);
    with_facts(&records, |facts| {
        limited(crate::stack_sizes(facts, &mut ledger(), &mut Units(0)).unwrap_err());
        Ok(())
    })
    .unwrap();
    // A table's entries are charged before the analysis follows them.
    let jump_table = labelled(&[leaf(Some(32))], &[TEXT, TEXT]);
    with_facts(&jump_table, |facts| {
        assert_eq!(
            image::jump_table(facts, RODATA, &mut ledger(), &mut Units(2))?,
            Some(vec![TEXT, TEXT])
        );
        limited(image::jump_table(facts, RODATA, &mut ledger(), &mut Units(1)).unwrap_err());
        Ok(())
    })
    .unwrap();
    // A data object reaching past read-only data is no table, whatever its
    // size claims: the words past the data are never charged.
    let mut oversized = image(&[leaf(Some(32))], &[TEXT, TEXT], true);
    let at = {
        let file = oer_analysis_elf::Elf::parse(&oversized).unwrap();
        let table = file.symbol("table").unwrap();
        let symtab = file.section(file.symbol_table_section().unwrap()).unwrap();
        // `st_size` of the table's ELF32 symbol entry.
        (symtab.file_offset + 16 * table.index as u64 + 8) as usize
    };
    oversized[at..at + 4].copy_from_slice(&(1u32 << 31).to_le_bytes());
    with_facts(&oversized, |facts| {
        assert_eq!(
            image::table(facts, RODATA, &mut ledger(), &mut Units(3))?,
            None
        );
        Ok(())
    })
    .unwrap();
    let object = image(&[leaf(Some(32))], &[TEXT, TEXT], true);
    with_facts(&object, |facts| {
        assert_eq!(
            image::table(facts, RODATA, &mut ledger(), &mut Units(2))?,
            Some(vec![TEXT, TEXT])
        );
        limited(image::table(facts, RODATA, &mut ledger(), &mut Units(1)).unwrap_err());
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_sweep_charges_each_table_entry_it_reads() {
    let sweep = |elf: &[u8], units: u64| {
        with_facts(elf, |facts| {
            sweep::transfers(
                facts,
                &facts.functions()[0],
                &mut ledger(),
                &mut Units(units),
            )
        })
    };
    // Two entries of a labelled table, read from its relocations.
    let labelled = labelled(&[dispatch(false)], &[TEXT + 24, TEXT + 28]);
    assert_eq!(
        sweep(&labelled, 2).unwrap().jumps,
        [(TEXT + 20, vec![TEXT + 24, TEXT + 28])]
    );
    let Err(error) = sweep(&labelled, 1) else {
        panic!("the sweep read past its budget");
    };
    assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    // Two entries a bounds check selects from an unlabelled table.
    let bounded = image(&[dispatch(true)], &[TEXT + 32, TEXT + 36, 0x5000], false);
    assert!(sweep(&bounded, 2).is_ok());
    let Err(error) = sweep(&bounded, 1) else {
        panic!("the sweep read past its budget");
    };
    assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
}

#[test]
fn the_analysis_charges_each_table_entry_it_follows() {
    // An unresolved tail through the labelled table at `RODATA`.
    let transfer = Transfer {
        site: TEXT + 20,
        target: None,
        kind: TransferKind::Tail,
        source: None,
        table: Some(TableBase::Address(RODATA)),
        load: None,
    };
    let jump_table = labelled(&[dispatch(false)], &[TEXT + 24, TEXT + 28]);
    with_facts(&jump_table, |facts| {
        assert_eq!(
            table_entries(facts, &transfer, None, &mut ledger(), &mut Units(2))?,
            Some(vec![TEXT + 24, TEXT + 28])
        );
        let error =
            table_entries(facts, &transfer, None, &mut ledger(), &mut Units(1)).unwrap_err();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
        Ok(())
    })
    .unwrap();
}

#[test]
fn image_tables_read_a_labelled_table_a_sized_object_or_a_bounded_run() {
    use oer_analysis_riscv_model::{AbstractValue, TableEntries};
    use oer_analysis_riscv_program::tables::ImageTables;
    let entries = |elf: &[u8], base: AbstractValue, count: Option<u32>| {
        with_facts(elf, |facts| {
            let memory = WorkingMemory::new(TEST_MEMORY)?;
            Ok(ImageTables { image: facts }
                .entries(&base, count, 0, &memory, &mut || Ok(()))?
                .map(|words| words.entries))
        })
        .unwrap()
    };
    let at = |address| AbstractValue::Constant { value: address };
    let words = vec![u64::from(TEXT), u64::from(TEXT) + 4];
    // A compiler table: the relocated words from its label, bound or not.
    let labelled = labelled(&[leaf(Some(32))], &[TEXT, TEXT + 4]);
    assert_eq!(entries(&labelled, at(RODATA), None), Some(words.clone()));
    assert_eq!(
        entries(&labelled, at(RODATA), Some(1)),
        Some(words[..1].to_vec())
    );
    // A sized object: its words.
    let sized = image(&[leaf(Some(32))], &[TEXT, TEXT + 4], true);
    assert_eq!(entries(&sized, at(RODATA), None), Some(words.clone()));
    // Neither: only a bound states the run of read-only words.
    let plain = image(&[leaf(Some(32))], &[TEXT, TEXT + 4], false);
    assert_eq!(entries(&plain, at(RODATA), None), None);
    assert_eq!(entries(&plain, at(RODATA), Some(2)), Some(words));
    // An object's symbol is no image address.
    assert_eq!(entries(&plain, AbstractValue::Unknown, Some(2)), None);
}

#[test]
fn source_symbol_kinds_preserve_the_two_address_audits_distinct_policies() {
    use crate::relocations::{FunctionUsePolicy, function_use};
    use oer_analysis_elf::{Elf, SectionKind};
    let f = Symbol {
        name: "f",
        words: vec![RET],
        frame: Some(0),
    };
    let g = Symbol {
        name: "g",
        words: vec![RET],
        frame: Some(0),
    };
    let original = labelled(&[f, g], &[TEXT + 4]);
    let (info_at, symbol_at, symbol_index) = {
        let file = Elf::parse(&original).unwrap();
        let relocation = file
            .sections()
            .find(|section| section.kind == SectionKind::Rela)
            .unwrap();
        let symbols = file.section(file.symbol_table_section().unwrap()).unwrap();
        let f = file.symbol("f").unwrap();
        (
            relocation.file_offset as usize + 4,
            symbols.file_offset as usize + f.index * 16,
            f.index,
        )
    };
    for text_symbol in [true, false] {
        let mut bytes = original.clone();
        let info = (symbol_index as u32) << 8 | object::elf::R_RISCV_32;
        bytes[info_at..info_at + 4].copy_from_slice(&info.to_le_bytes());
        bytes[info_at + 4..info_at + 8].copy_from_slice(&4_i32.to_le_bytes());
        if !text_symbol {
            // A global untyped code entry remains a function, but only the
            // typed-storage audit prefers this source identity over f+4.
            bytes[symbol_at + 12] = 0x10;
        }
        let analysis = analyze(&bytes, &[], &[]).unwrap();
        with_facts(&bytes, |image| {
            let use_ = &image.address_uses()[0];
            let task = function_use(use_, &analysis, FunctionUsePolicy::TaskPoll)?.unwrap();
            let typed = function_use(use_, &analysis, FunctionUsePolicy::TypedStorage)?.unwrap();
            assert_eq!(typed.function, Some(TEXT));
            assert!(!typed.exact);
            assert_eq!(
                task.function,
                Some(if text_symbol { TEXT } else { TEXT + 4 })
            );
            assert_eq!(task.exact, !text_symbol);
            assert_eq!(
                crate::taken_addresses(image, &mut ledger(), &mut || Ok(()))?,
                BTreeSet::from([TEXT + 4])
            );
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn main_and_companion_address_uses_keep_independent_elf_identities() {
    let main = labelled(&[leaf(Some(32))], &[TEXT + 2]);
    let companion_base = 0x8000;
    let mut companion = placed(
        companion_base,
        &[leaf(Some(32))],
        &[companion_base + 1],
        false,
        true,
    );
    // Give the companion's code a different section identity by swapping
    // .text and .stack_sizes headers and updating the symbols/RELA metadata.
    let shoff = u32::from_le_bytes(companion[32..36].try_into().unwrap()) as usize;
    let old_text = companion[shoff + 40..shoff + 80].to_vec();
    let old_sizes = companion[shoff + 80..shoff + 120].to_vec();
    companion[shoff + 40..shoff + 80].copy_from_slice(&old_sizes);
    companion[shoff + 80..shoff + 120].copy_from_slice(&old_text);
    let symtab = oer_analysis_elf::Elf::parse(&companion)
        .unwrap()
        .section(3)
        .unwrap()
        .file_offset as usize;
    companion[symtab + 16 + 14..symtab + 16 + 16].copy_from_slice(&2_u16.to_le_bytes());
    let analysis = analyze(&main, &[&companion], &[]).unwrap();
    with_facts(&main, |main| {
        with_facts(&companion, |companion| {
            use crate::relocations::{FunctionUsePolicy, function_use};
            assert_ne!(
                main.functions()[0].section,
                companion.functions()[0].section
            );
            let a = &main.address_uses()[0];
            let b = &companion.address_uses()[0];
            assert_eq!(a.function.nearest.unwrap().entry, TEXT);
            assert_eq!(b.function.nearest.unwrap().entry, companion_base);
            assert_eq!(
                function_use(a, &analysis, FunctionUsePolicy::TaskPoll)?
                    .unwrap()
                    .function,
                Some(TEXT)
            );
            assert_eq!(
                function_use(b, &analysis, FunctionUsePolicy::TaskPoll)?
                    .unwrap()
                    .function,
                Some(companion_base)
            );
            assert_eq!(
                crate::taken_addresses(main, &mut ledger(), &mut || Ok(()))?,
                BTreeSet::from([TEXT + 2])
            );
            assert_eq!(
                crate::taken_addresses(companion, &mut ledger(), &mut || Ok(()))?,
                BTreeSet::from([companion_base])
            );
            Ok(())
        })
    })
    .unwrap();
}
