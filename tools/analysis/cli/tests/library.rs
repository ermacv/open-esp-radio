#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};
use oer_analysis_blobray_application::in_process::Executable;
use oer_analysis_blobray_application::library::{Assumptions, register_accesses};
use oer_analysis_blobray_domain::*;
use oer_analysis_riscv_model::*;

struct WorkBudget {
    units: u64,
}
impl WorkBudget {
    fn new() -> Self {
        Self { units: 0 }
    }
}
impl RunControl for WorkBudget {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.units = self.units.saturating_add(units);
        if self.units > DEFAULT_WORK_UNITS {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "test work budget exhausted",
            ));
        }
        Ok(())
    }
}

/// A relocatable object with one function `entry` of `size` bytes over `code`.
fn object(code: &[u32], size: u64) -> Vec<u8> {
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &bytes, 2);
    obj.add_symbol(Symbol {
        name: b"entry".to_vec(),
        value: 0,
        size,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    obj.write().unwrap()
}

// lui t0,0x20; lw t1,0(t0); andi t2,t1,0xf; andi t1,t1,-16;
// ori t1,t1,3; sw t1,0(t0); lb t2,1(t0); lw t2,0(a0); ret.
const REGISTERS: [u32; 9] = [
    0x000202b7, 0x0002a303, 0x00f37393, 0xff037313, 0x00336313, 0x0062a023, 0x00128383, 0x00052383,
    0x00008067,
];

fn accesses(
    inputs: &[Executable],
    ranges: &[ImageRegion],
) -> Result<(RegisterAccessSummary, Vec<RegisterAccess>)> {
    accesses_with(inputs, ranges, Assumptions::default())
}

fn accesses_with(
    inputs: &[Executable],
    ranges: &[ImageRegion],
    assumptions: Assumptions<'_>,
) -> Result<(RegisterAccessSummary, Vec<RegisterAccess>)> {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let mut control = WorkBudget::new();
    let mut records = Vec::new();
    let summary = register_accesses(
        inputs,
        assumptions,
        ranges,
        &oer_analysis_riscv_lift::RiscvDecoder,
        &memory,
        &mut control,
        &mut |record, _| {
            records.push(record.clone());
            Ok(())
        },
    )?;
    Ok((summary, records))
}

fn library() -> Executable {
    let code = object(&REGISTERS, 36);
    Executable::new(support::archive(
        &[(b"first.o", &code), (b"second.o", &code)],
        false,
    ))
}

#[test]
fn every_library_function_reports_its_masked_and_unresolved_accesses() {
    let (summary, records) = accesses(&[library()], &[]).unwrap();
    assert_eq!(summary.functions, 2, "{records:?}");
    assert_eq!((summary.blocked_functions, summary.gaps), (0, 0));
    assert!(summary.unresolved_addresses > 0);
    let observed: Vec<_> = records
        .iter()
        .map(|r| match r {
            RegisterAccess::Observation {
                function,
                address,
                mask,
                ..
            } => (function, *address, *mask),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert!(
        observed.iter().all(|(function, _, _)| function.input == 0
            && function.name.as_deref() == Some(&b"entry"[..]))
    );
    // Both archive members are analyzed, each under its own occurrence.
    let members: std::collections::BTreeSet<_> =
        observed.iter().map(|(f, _, _)| &f.symbol.object).collect();
    assert_eq!(members.len(), 2);
    for kind in [
        RegisterMaskKind::ReadSelection,
        RegisterMaskKind::WriteReplacement,
    ] {
        assert!(
            observed
                .iter()
                .any(|(_, address, mask)| *address == Some(0x20000)
                    && *mask == Some(RegisterMask { kind, bits: 15 }))
        );
    }
}

#[test]
fn a_store_describes_its_stored_bits_and_a_load_never_does() {
    let (_, records) = accesses(&[library()], &[]).unwrap();
    let mut stores = 0;
    for record in &records {
        let RegisterAccess::Observation {
            fact,
            address,
            stored,
            ..
        } = record
        else {
            continue;
        };
        // A load, or a masked read whose fact is its selecting expression.
        if !matches!(
            fact.as_ref(),
            FunctionRecord::MemoryAccess {
                access: MemoryKind::Store,
                ..
            }
        ) {
            assert!(stored.is_none(), "a read stores nothing: {record:?}");
            continue;
        }
        stores += 1;
        assert_eq!(*address, Some(0x20000));
        // `sw ((lw 0x20000) & ~15 | 3), 0x20000`.
        assert_eq!(
            stored.as_deref(),
            Some(
                &[
                    StoredBits {
                        low: 0,
                        width: 4,
                        source: StoredBitsSource::Constant { value: 3 },
                    },
                    StoredBits {
                        low: 4,
                        width: 28,
                        source: StoredBitsSource::Load {
                            address: Some(0x20000),
                            width: 4,
                            low: 4,
                            same_word: true,
                        },
                    },
                ][..]
            ),
            "{record:?}"
        );
    }
    assert_eq!(stores, 2, "one store per archive member");
}

#[test]
fn ranges_select_resolved_addresses_and_bad_ranges_are_rejected() {
    let inside = [ImageRegion {
        start: 0x20000,
        length: 4,
    }];
    let (summary, records) = accesses(&[library()], &inside).unwrap();
    assert!(records.iter().all(|r| matches!(
        r,
        RegisterAccess::Observation {
            address: None | Some(0x20000 | 0x20001),
            ..
        }
    )));
    let outside = [ImageRegion {
        start: 0x40000,
        length: 4,
    }];
    let (without, _) = accesses(&[library()], &outside).unwrap();
    assert!(without.observations < summary.observations);
    assert_eq!(without.unresolved_addresses, summary.unresolved_addresses);
    for bad in [
        vec![ImageRegion {
            start: 0x20000,
            length: 0,
        }],
        vec![ImageRegion {
            start: u32::MAX,
            length: 2,
        }],
        vec![inside[0]; 257],
    ] {
        let error = accesses(&[library()], &bad).unwrap_err();
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    }
}

#[test]
fn unanalyzable_functions_and_foreign_objects_are_reported_not_skipped() {
    let without_size = object(&REGISTERS, 0);
    let inputs = [
        Executable::new(support::archive(
            &[
                (b"unsized.o", &without_size),
                (b"notes.txt", b"not an object"),
            ],
            false,
        )),
        Executable::new(b"neither an archive nor an ELF".to_vec()),
    ];
    let (summary, records) = accesses(&inputs, &[]).unwrap();
    assert_eq!(summary.functions, 0);
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Blocked { function, error }
            if function.input == 0 && error.code == ErrorCode::NeedsExtent
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Gap { input: 0, object: Some(_), reason }
            if reason.contains("not a supported ELF32")
    )));
    assert!(records.iter().any(|r| matches!(
        r,
        RegisterAccess::Gap { input: 1, reason, .. } if reason == "unsupported input format"
    )));
    assert_eq!(summary.blocked_functions, 1);
}

/// `caller`: `li a0, 0x67; li a1, 1; mv a2, a5; call phy_i2c_writeReg; ret`.
fn calling_object() -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let words: [u32; 6] = [
        0x0670_0513, // addi a0, zero, 0x67
        0x0010_0593, // addi a1, zero, 1
        0x0007_8613, // addi a2, a5, 0
        0x0000_0097, // auipc ra, 0
        0x0000_80e7, // jalr ra, 0(ra)
        0x0000_8067, // jalr zero, 0(ra)
    ];
    let code: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 4);
    object.add_symbol(Symbol {
        name: b"caller".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let callee = object.add_symbol(Symbol {
        name: b"phy_i2c_writeReg".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Unknown,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            text,
            Relocation {
                offset: 12,
                symbol: callee,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_CALL_PLT,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

/// `dispatch(a0)`: `li a5, 2; bltu a5, a0, default; lui/addi a5, .Ltable;
/// slli a4, a0, 2; add; lw a5; jalr zero, 0(a5)` with three cases and a
/// default, the table in `.rodata` relocated by `.rela.rodata`. `bounded`
/// keeps the check (a `nop` replaces it otherwise); `relocated` entries of
/// the three carry their `R_RISCV_32`.
fn switch_object(bounded: bool, relocated: u64) -> Vec<u8> {
    let words: [u32; 16] = [
        0x0020_0793,                                     // 0x00 addi a5, zero, 2
        if bounded { 0x02a7_ea63 } else { 0x0000_0013 }, // 0x04 bltu a5, a0, 0x38
        0x0000_07b7,                                     // 0x08 lui a5, %hi(.Ltable)
        0x0007_8793,                                     // 0x0c addi a5, a5, %lo(.Ltable)
        0x0025_1713,                                     // 0x10 slli a4, a0, 2
        0x00f7_0733,                                     // 0x14 add a4, a4, a5
        0x0007_2783,                                     // 0x18 lw a5, 0(a4)
        0x0007_8067,                                     // 0x1c jalr zero, 0(a5)
        0x00a0_0513,
        0x0000_8067, // 0x20 case 0
        0x00b0_0513,
        0x0000_8067, // 0x28 case 1
        0x00c0_0513,
        0x0000_8067, // 0x30 case 2
        0x0000_0513,
        0x0000_8067, // 0x38 default
    ];
    switches_object(&words, &[(0x08, &[0x20, 0x28, 0x30], relocated as usize)])
}

/// A relocatable object defining `dispatch` as `words` in `.text`, with one
/// `.Ltable{n}` in `.rodata` per `(lui offset, case labels, relocated)`: the
/// `lui/addi` pair at that offset addresses it and its first `relocated`
/// entries carry their `R_RISCV_32` to the case labels.
fn switches_object(words: &[u32], tables: &[(u64, &[i64], usize)]) -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let code: Vec<u8> = words.iter().flat_map(|word| word.to_le_bytes()).collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 4);
    let rodata = object.add_section(Vec::new(), b".rodata".to_vec(), SectionKind::ReadOnlyData);
    let entries: usize = tables.iter().map(|(_, cases, _)| cases.len()).sum();
    object.append_section_data(rodata, &vec![0; 4 * entries], 4);
    object.add_symbol(Symbol {
        name: b"dispatch".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let text_symbol = object.section_symbol(text);
    let relocate = |object: &mut Object, section, offset, symbol, addend, r_type| {
        object
            .add_relocation(
                section,
                Relocation {
                    offset,
                    symbol,
                    addend,
                    flags: RelocationFlags::Elf { r_type },
                },
            )
            .unwrap();
    };
    let mut at = 0;
    for (n, (lui, cases, relocated)) in tables.iter().enumerate() {
        let table = object.add_symbol(Symbol {
            name: format!(".Ltable{n}").into_bytes(),
            value: at,
            size: 0,
            kind: SymbolKind::Data,
            scope: SymbolScope::Compilation,
            weak: false,
            section: SymbolSection::Section(rodata),
            flags: SymbolFlags::None,
        });
        relocate(&mut object, text, *lui, table, 0, object::elf::R_RISCV_HI20);
        relocate(
            &mut object,
            text,
            lui + 4,
            table,
            0,
            object::elf::R_RISCV_LO12_I,
        );
        for (entry, case) in cases.iter().enumerate().take(*relocated) {
            relocate(
                &mut object,
                rodata,
                at + 4 * entry as u64,
                text_symbol,
                *case,
                object::elf::R_RISCV_32,
            );
        }
        at += 4 * cases.len() as u64;
    }
    object.write().unwrap()
}

/// `orphan`: `addi a0, a0, %pcrel_lo(.Lnone); ret`, whose low half names no
/// `%pcrel_hi` and so no target.
fn orphan_relocation_object() -> Vec<u8> {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags,
        SymbolKind, SymbolScope,
    };
    let code: Vec<u8> = [0x0005_0513_u32, 0x0000_8067]
        .iter()
        .flat_map(|word| word.to_le_bytes())
        .collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    object.append_section_data(text, &code, 4);
    object.add_symbol(Symbol {
        name: b"orphan".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let label = object.add_symbol(Symbol {
        name: b".Lnone".to_vec(),
        value: 4,
        size: 0,
        kind: SymbolKind::Label,
        scope: SymbolScope::Compilation,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: label,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_PCREL_LO12_I,
                },
            },
        )
        .unwrap();
    object.write().unwrap()
}

#[test]
fn partial_functions_are_counted_once_per_cause() {
    let causes = |name: &[u8], object: Vec<u8>| {
        let library = Executable::new(support::archive(&[(name, &object)], false));
        let (summary, _) = accesses(&[library], &[]).unwrap();
        assert_eq!(summary.partial_functions, 1);
        summary.partial_causes
    };
    let only = |cause: fn(&mut PartialCauses) -> &mut u64| {
        let mut causes = PartialCauses::default();
        *cause(&mut causes) = 1;
        causes
    };
    assert_eq!(
        causes(b"call.o", calling_object()),
        only(|c| &mut c.opaque_calls),
        "an opaque call leaves the values after it unknown, nothing else"
    );
    assert_eq!(
        causes(b"switch.o", switch_object(false, 3)),
        only(|c| &mut c.control_flow),
        "an unexpanded indirect jump is control flow only"
    );
    assert_eq!(
        causes(b"orphan.o", orphan_relocation_object()),
        PartialCauses {
            references: 1,
            // The graph treats an instruction with an unknown relocation as
            // possibly transferring control, so its flow is incomplete too.
            control_flow: 1,
            ..PartialCauses::default()
        },
    );
    // Together, each function counts once in the total and once per cause.
    let library = Executable::new(support::archive(
        &[
            (b"call.o", &calling_object()),
            (b"switch.o", &switch_object(false, 3)),
            (b"orphan.o", &orphan_relocation_object()),
        ],
        false,
    ));
    let (summary, _) = accesses(&[library], &[]).unwrap();
    assert_eq!((summary.functions, summary.partial_functions), (3, 3));
    assert_eq!(
        summary.partial_causes,
        PartialCauses {
            control_flow: 2,
            references: 1,
            opaque_calls: 1,
            ..PartialCauses::default()
        }
    );
}

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

#[test]
fn a_call_takes_the_constants_its_callee_returns() {
    const RET: u32 = 0x0000_8067;
    let code = [
        // helper @0x1000: a0 = 0x20101000, or 0x20100894 unless a1 == 0.
        0x2010_1537, // lui a0, 0x20101
        0x0005_8463, // beq a1, zero, +8
        0x8945_0513, // addi a0, a0, -1900
        RET,
        // caller @0x1010: loads through helper's result.
        call(0x1010, 0x1000),
        0x0005_2503, // lw a0, 0(a0)
        RET,
        // loaded @0x101c: returns a word it loads, no constant.
        0x0005_2503, // lw a0, 0(a0)
        RET,
        // other @0x1024: loads through loaded's result.
        call(0x1024, 0x101c),
        0x0005_2503, // lw a0, 0(a0)
        RET,
    ];
    let image = support::executable_with_symbols(
        &code,
        &[
            ("helper", 0x1000, 16),
            ("caller", 0x1010, 12),
            ("loaded", 0x101c, 8),
            ("other", 0x1024, 12),
        ],
    );
    let observed = |call_returns: bool, name: &str| -> Vec<u32> {
        let (_, records) = accesses_with(
            &[Executable::new(image.clone())],
            &[ImageRegion {
                start: 0x2010_0000,
                length: 0x2000,
            }],
            Assumptions {
                abi: None,
                call_returns,
                read_only: &[],
            },
        )
        .unwrap();
        let mut addresses: Vec<u32> = records
            .iter()
            .filter_map(|record| match record {
                RegisterAccess::Observation {
                    function, address, ..
                } if function.name.as_deref() == Some(name.as_bytes()) => *address,
                _ => None,
            })
            .collect();
        addresses.sort_unstable();
        addresses
    };
    assert_eq!(observed(true, "caller"), [0x2010_0894, 0x2010_1000]);
    // A callee with a return that is no constant gives the call nothing.
    assert_eq!(observed(true, "other"), Vec::<u32>::new());
    // Calls take returned constants only when the request assumes them.
    assert_eq!(observed(false, "caller"), Vec::<u32>::new());
}

/// A relocatable object of global functions `(name, offset, size)` over
/// `code`, with relocations `(offset, symbol, type)` against its own or
/// undefined symbols.
fn functions_object(
    code: &[u32],
    functions: &[(&str, u64, u64)],
    relocations: &[(u64, &str, u32)],
) -> Vec<u8> {
    use object::write::Relocation;
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &bytes, 4);
    let symbol = |obj: &mut Object, name: &str, value, size, section| {
        obj.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value,
            size,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section,
            flags: SymbolFlags::None,
        })
    };
    let mut ids = Vec::new();
    for &(name, offset, size) in functions {
        ids.push((
            name,
            symbol(&mut obj, name, offset, size, SymbolSection::Section(text)),
        ));
    }
    for &(offset, name, r_type) in relocations {
        let id = match ids.iter().find(|(known, _)| *known == name) {
            Some(&(_, id)) => id,
            None => {
                let id = symbol(&mut obj, name, 0, 0, SymbolSection::Undefined);
                ids.push((name, id));
                id
            }
        };
        obj.add_relocation(
            text,
            Relocation {
                offset,
                symbol: id,
                addend: 0,
                flags: object::RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}

/// The addresses `name`'s observations of `[0x20100000, 0x20102000)` hold
/// in `inputs`, analyzed with returned constants.
fn returned_addresses(inputs: &[Executable], name: &str) -> Vec<u32> {
    let (_, records) = accesses_with(
        inputs,
        &[ImageRegion {
            start: 0x2010_0000,
            length: 0x3000,
        }],
        Assumptions {
            abi: None,
            call_returns: true,
            read_only: &[],
        },
    )
    .unwrap();
    let mut addresses: Vec<u32> = records
        .iter()
        .filter_map(|record| match record {
            RegisterAccess::Observation {
                function, address, ..
            } if function.name.as_deref() == Some(name.as_bytes()) => *address,
            _ => None,
        })
        .collect();
    addresses.sort_unstable();
    addresses
}

#[test]
fn an_object_call_takes_its_global_callees_returned_constants() {
    // helper: lui a0, 0x20101; ret.
    let helper = functions_object(&[0x2010_1537, 0x0000_8067], &[("helper", 0, 8)], &[]);
    // by_jal: jal ra, helper; lw a0, 0(a0); ret.
    // by_call: auipc ra, %hi(helper); jalr ra, %lo(helper)(ra); lw a0, 0(a0); ret.
    let caller = functions_object(
        &[
            0x0000_00ef,
            0x0005_2503,
            0x0000_8067,
            0x0000_0097,
            0x0000_80e7,
            0x0005_2503,
            0x0000_8067,
        ],
        &[("by_jal", 0, 12), ("by_call", 12, 16)],
        &[
            (0, "helper", object::elf::R_RISCV_JAL),
            (12, "helper", object::elf::R_RISCV_CALL_PLT),
        ],
    );
    let library = Executable::new(support::archive(
        &[(b"helper.o", &helper), (b"caller.o", &caller)],
        false,
    ));
    let inputs = [library];
    assert_eq!(returned_addresses(&inputs, "by_jal"), [0x2010_1000]);
    assert_eq!(returned_addresses(&inputs, "by_call"), [0x2010_1000]);
}

#[test]
fn an_object_call_bypassing_its_upper_does_not_take_the_relocated_callees_results() {
    let helper = functions_object(&[0x2010_1537, 0x0000_8067], &[("helper", 0, 8)], &[]);
    for relocation in [object::elf::R_RISCV_CALL, object::elf::R_RISCV_CALL_PLT] {
        for (branch, expected) in [
            (0x0005_0463, vec![0x2010_1000]), // Both paths execute AUIPC.
            (0x0005_0663, vec![]),            // One path enters JALR with unknown entry RA.
        ] {
            let caller = functions_object(
                &[
                    branch,
                    0x0000_0013, // nop
                    0x0000_0097, // auipc ra, 0, relocated to helper
                    0x0000_80e7, // jalr ra, 0(ra)
                    0x0005_2503, // lw a0, 0(a0): uses the call's result
                    0x0000_8067, // ret
                ],
                &[("caller", 0, 24)],
                &[(8, "helper", relocation)],
            );
            let library = Executable::new(support::archive(
                &[(b"helper.o", &helper), (b"caller.o", &caller)],
                false,
            ));
            assert_eq!(
                returned_addresses(&[library], "caller"),
                expected,
                "branch={branch:#x}, relocation={relocation}"
            );
        }
    }
}

#[test]
fn a_symbol_defined_with_other_results_answers_nothing() {
    // helper: lui a0, 0x20101; ret.
    let helper = functions_object(&[0x2010_1537, 0x0000_8067], &[("helper", 0, 8)], &[]);
    // helper: beq a1, zero, elsewhere; lui a0, 0x20101; ret. Its branch
    // out leaves through other code, so it gives no results.
    let branching = functions_object(
        &[0x0005_8063, 0x2010_1537, 0x0000_8067],
        &[("helper", 0, 12)],
        &[(0, "elsewhere", object::elf::R_RISCV_BRANCH)],
    );
    // by_jal: jal ra, helper; lw a0, 0(a0); ret.
    let caller = functions_object(
        &[0x0000_00ef, 0x0005_2503, 0x0000_8067],
        &[("by_jal", 0, 12)],
        &[(0, "helper", object::elf::R_RISCV_JAL)],
    );
    let library = Executable::new(support::archive(
        &[
            (b"helper.o", &helper),
            (b"branching.o", &branching),
            (b"caller.o", &caller),
        ],
        false,
    ));
    assert_eq!(returned_addresses(&[library], "by_jal"), Vec::<u32>::new());
}

#[test]
fn a_callee_that_branches_out_returns_nothing() {
    const RET: u32 = 0x0000_8067;
    let code = [
        // cond @0x1000: bne a1, zero, other; lui a0, 0x20101; ret.
        0x0005_9663,
        0x2010_1537,
        RET,
        // other @0x100c: lui a0, 0x20102; ret.
        0x2010_2537,
        RET,
        // caller @0x1014: jal ra, cond; lw a0, 0(a0); ret.
        call(0x1014, 0x1000),
        0x0005_2503,
        RET,
    ];
    let image = support::executable_with_symbols(
        &code,
        &[
            ("cond", 0x1000, 12),
            ("other", 0x100c, 8),
            ("caller", 0x1014, 12),
        ],
    );
    assert_eq!(
        returned_addresses(&[Executable::new(image)], "caller"),
        Vec::<u32>::new()
    );
}
