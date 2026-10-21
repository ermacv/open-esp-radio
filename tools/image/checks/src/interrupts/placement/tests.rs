use super::*;
use crate::interrupts::tests::{partial, stacks};
use object::write::{Object, Relocation, StandardSection, Symbol, SymbolSection};
use object::{Architecture, BinaryFormat, Endianness, RelocationFlags, SymbolFlags, SymbolScope};
use oer_analysis_riscv_stack::{Function, FunctionFacts};

pub(in crate::interrupts) fn facts(address: u32, size: u32) -> FunctionFacts {
    FunctionFacts {
        function: Function {
            address,
            size,
            names: Vec::new(),
            section: 0,
        },
        frame: Some(0),
        source: None,
        observed: Some(0),
        complete: true,
        terminal_recursive_callees: false,
        target_loads: BTreeMap::new(),
        open_exit_target_loads: BTreeMap::new(),
        load_bases: BTreeMap::new(),
        transfers: Vec::new(),
        site_depths: BTreeMap::new(),
        call_arguments: BTreeMap::new(),
        table_bases: BTreeMap::new(),
        unresolved_registers: BTreeMap::new(),
        level_drops: Vec::new(),
        float_sites: Vec::new(),
    }
}

fn inventory() -> (InterruptStacks, Analysis) {
    let mut stacks = partial(0);
    let mut level = stacks.harts[0].exception.clone();
    level.level = 1;
    level.bound.reached = BTreeSet::from([0x5000_1000, 0x5000_2000]);
    let mut p2 = level.clone();
    p2.level = 2;
    p2.bound.reached = BTreeSet::from([0x5000_1000]);
    stacks.harts[0].levels = vec![level, p2];
    stacks.harts[0].exception.bound.reached = BTreeSet::from([0x5000_2000]);
    let mut other = stacks.harts[0].clone();
    other.core = 1;
    other.levels.truncate(1);
    other.levels[0].bound.reached = BTreeSet::from([0x5000_1000]);
    other.exception.bound.reached.clear();
    stacks.harts.push(other);
    stacks.names = BTreeMap::from([
        (0x5000_1000, "alpha::small".into()),
        (0x5000_2000, "beta::large".into()),
    ]);
    stacks.handler_code.cached = vec![0x5000_1000, 0x5000_2000];
    let analysis = Analysis {
        traps: Default::default(),
        functions: BTreeMap::from([
            (0x5000_1000, facts(0x5000_1000, 12)),
            (0x5000_2000, facts(0x5000_2000, 28)),
        ]),
        code: Vec::new(),
        summaries: BTreeSet::new(),
    };
    (stacks, analysis)
}

#[test]
fn shared_cached_functions_count_once_in_the_image_and_once_at_each_level() {
    let (stacks, analysis) = inventory();
    let (functions, levels) = cached_functions(&analysis, &stacks);
    assert_eq!(functions.len(), 2);
    assert_eq!(
        functions.iter().map(|function| function.size).sum::<u32>(),
        40
    );
    assert_eq!(
        functions
            .iter()
            .map(|function| function.address)
            .collect::<Vec<_>>(),
        [0x5000_2000, 0x5000_1000]
    );
    assert_eq!(
        levels[&Level {
            hart: 0,
            priority: 1
        }],
        Total {
            functions: 2,
            bytes: 40
        }
    );
    assert_eq!(
        levels[&Level {
            hart: 0,
            priority: 2
        }],
        Total {
            functions: 1,
            bytes: 12
        }
    );
    assert_eq!(
        levels[&Level {
            hart: 0,
            priority: 0
        }],
        Total {
            functions: 1,
            bytes: 28
        }
    );
    assert_eq!(
        levels[&Level {
            hart: 1,
            priority: 1
        }],
        Total {
            functions: 1,
            bytes: 12
        }
    );
    assert_eq!(
        levels[&Level {
            hart: 1,
            priority: 0
        }],
        Total::default()
    );
    assert_eq!(functions[1].levels.len(), 3);
}

fn symbol(
    writer: &mut Object<'_>,
    name: &str,
    address: u64,
    size: u64,
    kind: object::SymbolKind,
) -> object::write::SymbolId {
    writer.add_symbol(Symbol {
        name: name.as_bytes().to_vec(),
        value: address,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Absolute,
        flags: SymbolFlags::None,
    })
}

/// Real retained relocations: two interior references and aliases of one
/// PSRAM object, an SRAM object, and cached data without a sized symbol.
fn data_elf() -> (Vec<u8>, InterruptStacks) {
    let stacks = stacks(Vec::new());
    let mut writer = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = writer.section_id(StandardSection::Text);
    writer.append_section_data(text, &[0; 32], 4);
    let psram = u64::from(stacks.contract.memory.psram.unwrap().origin) + 0x100;
    let sram = u64::from(stacks.contract.memory.sram.origin) + 0x100;
    let object = symbol(&mut writer, "table", psram, 24, object::SymbolKind::Data);
    let alias = symbol(
        &mut writer,
        "table_alias",
        psram,
        24,
        object::SymbolKind::Data,
    );
    let local = symbol(&mut writer, "local", sram, 16, object::SymbolKind::Data);
    let unknown = symbol(
        &mut writer,
        "label",
        psram + 0x100,
        0,
        object::SymbolKind::Data,
    );
    for (offset, symbol, addend) in [
        (0, object, 4),
        (4, alias, 12),
        (8, local, 0),
        (12, unknown, 0),
    ] {
        writer
            .add_relocation(
                text,
                Relocation {
                    offset,
                    symbol,
                    addend,
                    flags: RelocationFlags::Elf {
                        r_type: object::elf::R_RISCV_HI20,
                    },
                },
            )
            .unwrap();
    }
    (writer.write().unwrap(), stacks)
}

#[test]
fn psram_data_references_use_the_whole_sized_object_once_and_ignore_sram() {
    let (bytes, stacks) = data_elf();
    let elf = Elf::parse(&bytes).unwrap();
    let references = cached_hook_references(
        &elf,
        &[Function {
            address: 0,
            size: 32,
            names: vec!["handler".into()],
            section: 1,
        }],
        &BTreeSet::from([0]),
        &stacks.contract.memory,
    )
    .unwrap();
    assert_eq!(references.len(), 3);
    assert!(!references.iter().any(|(_, site, _)| *site == 8));
    let (data, objects) = cached_data(&elf, &references, &stacks);
    assert_eq!(data.len(), 3);
    assert_eq!(objects.len(), 1);
    let object = objects.values().next().unwrap();
    assert_eq!(object.size, 24);
    assert_eq!(
        object.names,
        BTreeSet::from(["table".into(), "table_alias".into()])
    );
    assert_eq!(data[0].object, data[1].object);
    assert!(data[2].object.is_none());
    assert_eq!(data[0].target, object.address + 4);
    assert!(
        cached_hook_references(&elf, &[], &BTreeSet::new(), &stacks.contract.memory)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cached_code_targets_are_not_counted_as_data() {
    let (bytes, stacks) = data_elf();
    let elf = Elf::parse(&bytes).unwrap();
    // The synthetic text section occupies 0..32; the data classifier should
    // reject a code target even when the caller supplied it as a reference.
    let (data, objects) = cached_data(&elf, &[(0, 4, 16)], &stacks);
    assert!(data.is_empty());
    assert!(objects.is_empty());
}

#[test]
fn sram_is_the_union_of_placed_ranges_below_the_rom_boundary() {
    let region = Region {
        origin: 0x1000,
        length: 0x800,
    };
    let ranges = [
        SramRange {
            name: "critical BSS".into(),
            start: 0x1000,
            end: 0x1300,
        },
        SramRange {
            name: "IRQ stack inside BSS".into(),
            start: 0x1100,
            end: 0x1200,
        },
        SramRange {
            name: "DMA".into(),
            start: 0x1400,
            end: 0x1500,
        },
        SramRange {
            name: "ROM above usable SRAM".into(),
            start: 0x1800,
            end: 0x2000,
        },
        SramRange {
            name: "empty".into(),
            start: 0x1300,
            end: 0x1300,
        },
    ];
    // Physical SRAM is 0x1000 bytes, but its ROM-reserved top half is
    // already outside the contract's usable 0x800: do not subtract it again.
    assert_eq!(union_bytes(region, &ranges), 0x400);
    assert_eq!(
        u64::from(region.length) - union_bytes(region, &ranges),
        0x400
    );
    let clipped = [SramRange {
        name: "crosses usable end".into(),
        start: 0x1700,
        end: 0x1900,
    }];
    assert_eq!(union_bytes(region, &clipped), 0x100);
}

#[test]
fn sram_inventory_reads_placement_symbols_sections_and_the_boundary_source() {
    let profile: Profile = toml::from_str(include_str!(
        "../../../../../../chips/esp32s31/platform/chip.toml"
    ))
    .unwrap();
    let region = profile.memory.as_ref().unwrap().sram;
    let base = u64::from(region.origin);
    let mut writer = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = writer.section_id(StandardSection::Text);
    writer.append_section_data(text, &[0; 32], 4);
    for (name, offset) in [
        ("__runtime_isr_start", 0),
        ("__runtime_isr_end", 0x100),
        ("__runtime_critical_data_start", 0x100),
        ("__runtime_critical_bss_end", 0x400),
        ("__runtime_cpu0_irq_stack_bottom", 0x200),
        ("__runtime_cpu0_irq_stack_top", 0x300),
        ("__runtime_dma_data_start", 0x500),
        ("__runtime_dma_bss_end", 0x600),
    ] {
        symbol(
            &mut writer,
            name,
            base + offset,
            0,
            object::SymbolKind::Data,
        );
    }
    let mut bytes = writer.write().unwrap();
    // object::write emits relocatables; give .text an SRAM sh_addr to
    // represent allocated code outside the contract's explicit ranges.
    let elf = Elf::parse(&bytes).unwrap();
    let index = elf.section_by_name(".text").unwrap().index;
    let headers = u32::from_le_bytes(bytes[32..36].try_into().unwrap()) as usize;
    let width = u16::from_le_bytes(bytes[46..48].try_into().unwrap()) as usize;
    let at = headers + index * width + 12;
    bytes[at..at + 4].copy_from_slice(&(region.origin + 0x700).to_le_bytes());
    let elf = Elf::parse(&bytes).unwrap();
    let sram = Sram::of(&elf, &profile, region);
    assert_eq!(sram.placed, 0x520);
    assert_eq!(sram.free(), u64::from(region.length) - 0x520);
    assert!(sram.ranges.iter().any(|range| range.name == ".text"));
    assert!(sram.source.contains("chips/esp32s31/platform/chip.toml"));
    assert!(sram.source.contains("chips/esp32s31/layout/src/lib.rs"));
    assert!(
        sram.source
            .contains("ROM boot-stack side above the end is already excluded")
    );
}

#[test]
fn summaries_and_full_report_headings_are_stable() {
    let (stacks, analysis) = inventory();
    let (functions, _) = cached_functions(&analysis, &stacks);
    let code_summary = stacks
        .code_placement
        .summary(|address| stacks.name(address));
    let placement = InterruptPlacement {
        functions,
        references: Vec::new(),
        objects: BTreeMap::new(),
        sram: Sram {
            region: Region {
                origin: 0x1000,
                length: 2048,
            },
            source: "ROM top excluded by the contract".into(),
            ranges: Vec::new(),
            placed: 1024,
        },
        code_summary: code_summary.clone(),
        strict_totals: stacks.code_placement.totals(),
        strict_paths: stacks
            .code_placement
            .render_paths(|address| stacks.name(address)),
    };
    let expected = "strict handler code in cached memory: 0 functions, 0 bytes; by level: none\nP2 task code in PSRAM: 0 functions, 0 bytes; tasks: none\ncached handler data: 0 references to 0 objects, 0 bytes\nSRAM: usable 2048, placed 1024, free 1024 bytes\n";
    assert_eq!(placement.summary(), expected);
    let full = stacks.render_with_placement(&placement);
    let original = stacks.render();
    assert_eq!(full.replacen(expected, &code_summary, 1), original);
    let after = full
        .split_once("handler code: ")
        .unwrap()
        .1
        .split_once('\n')
        .unwrap()
        .1;
    assert!(after.starts_with(expected));
    let report = placement.render();
    let headings: Vec<_> = report
        .split("\n\n")
        .skip(1)
        .map(|section| section.lines().next().unwrap())
        .collect();
    assert_eq!(
        headings,
        [
            "Cached handler functions (size descending)",
            "Cached handler code by crate",
            "Strict handler code by region",
            "Strict cached handler paths",
            "Cached handler data references",
            "Cached handler data objects",
            "Data accounting: 0 references without a sized data symbol; object bytes cover sized objects only. Code targets are excluded. Aliases of the same address and size count once; interior addresses count the whole object. Only static references from resolved handler code are measured.",
            "SRAM accounting",
        ]
    );
    assert!(report.contains("beta\t1\t28\nalpha\t1\t12\n"));
    assert!(report.contains("name\taddress\tsize\tlevels\tcrate\tclass\n"));
    assert!(report.contains("\tbeta\tstrict\n"));
    // The measurements add no policy: the same check still gives exactly
    // the same result and warnings after rendering either report.
    let before = stacks.check(super::super::Required::Partial).unwrap();
    assert_eq!(
        stacks.check(super::super::Required::Partial).unwrap(),
        before
    );
}
