//! Function analysis of captured libraries in process: graphs, values,
//! references, gaps and budgets of one function `entry`.
#![cfg(target_os = "linux")]
#[allow(dead_code)]
mod support;
use object::{
    Architecture, BinaryFormat, Endianness, RelocationFlags, SectionKind, SymbolFlags, SymbolKind,
    SymbolScope,
    write::{Object, Relocation, Symbol, SymbolSection},
};
use oer_tool_analysis_blobray_application::in_process::{Executable, Limits};
use oer_tool_analysis_blobray_application::library::{
    Assumptions, LibraryOutcome, analyze_library,
};
use oer_tool_analysis_riscv_model::*;

fn symbol(name: &[u8], section: SymbolSection, size: u64, kind: SymbolKind) -> Symbol {
    Symbol {
        name: name.into(),
        value: 0,
        size,
        kind,
        scope: SymbolScope::Linkage,
        weak: false,
        section,
        flags: SymbolFlags::None,
    }
}

const BRANCH: &[u8] = &[
    1, 0, 0x63, 0x04, 0x05, 0, 0x13, 0x05, 0x15, 0, 0x67, 0x80, 0, 0,
];

/// A relocatable object whose function `entry` of `size` bytes holds
/// `bytes`; with `references`, it calls `outside` and reads `data`.
fn object(bytes: &[u8], size: u64, references: bool) -> Vec<u8> {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, bytes, 2);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        size,
        SymbolKind::Text,
    ));
    if references {
        let call = obj.add_symbol(symbol(
            b"outside",
            SymbolSection::Undefined,
            0,
            SymbolKind::Text,
        ));
        let data = obj.add_symbol(symbol(
            b"data",
            SymbolSection::Undefined,
            0,
            SymbolKind::Data,
        ));
        for (offset, symbol, kind) in [
            (0, call, object::elf::R_RISCV_CALL),
            (8, data, object::elf::R_RISCV_HI20),
            (12, data, object::elf::R_RISCV_LO12_I),
        ] {
            obj.add_relocation(
                text,
                Relocation {
                    offset,
                    symbol,
                    addend: 0,
                    flags: RelocationFlags::Elf { r_type: kind },
                },
            )
            .unwrap();
        }
    }
    obj.write().unwrap()
}

fn words(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

struct Analysis {
    records: Vec<FunctionRecord>,
    coverage: FunctionCoverage,
    semantics: SemanticSummary,
}
impl Analysis {
    fn count(&self, kind: impl Fn(&FunctionRecord) -> bool) -> usize {
        self.records.iter().filter(|r| kind(r)).count()
    }
    fn instructions(&self) -> usize {
        self.count(|r| matches!(r, FunctionRecord::Instruction { .. }))
    }
}

/// The analysis of `entry`, the one function of a library holding `object`.
fn analyze_with(object: Vec<u8>, memory: u64, control: &mut Limits) -> Result<Analysis> {
    let library = Executable::new(support::archive(&[(b"entry.o", &object)], false));
    let memory = WorkingMemory::new(memory).unwrap();
    let mut found = None;
    analyze_library(
        &[library],
        Assumptions::default(),
        &oer_tool_analysis_riscv_lift::RiscvDecoder,
        &memory,
        control,
        &mut |outcome, _| {
            match outcome {
                LibraryOutcome::Analyzed(analyzed)
                    if analyzed.function.name.as_deref() == Some(b"entry") =>
                {
                    found = Some(Ok(Analysis {
                        records: analyzed.records.to_vec(),
                        coverage: analyzed.coverage,
                        semantics: analyzed.semantics,
                    }));
                }
                LibraryOutcome::Blocked { function, error }
                    if function.name.as_deref() == Some(b"entry") =>
                {
                    found = Some(Err(error.clone()));
                }
                _ => (),
            }
            Ok(())
        },
    )?;
    found.expect("entry is analyzed or blocked")
}

fn limits() -> Limits {
    Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::from_secs(60))
}

fn analyze(object: Vec<u8>) -> Analysis {
    analyze_with(object, 32 * 1024 * 1024, &mut limits()).unwrap()
}

#[test]
fn the_function_graph_follows_branches_to_every_block() {
    let f = analyze(object(BRANCH, BRANCH.len() as u64, false));
    assert!(f.coverage.complete());
    assert_eq!(f.instructions(), 4);
    assert_eq!(f.count(|r| matches!(r, FunctionRecord::Block { .. })), 3);
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::Edge {
            from: 2,
            target: Some(10),
            relation: EdgeKind::Taken,
            ..
        }
    )));
}

#[test]
fn external_calls_and_data_references_do_not_require_linking() {
    let bytes = [
        0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x37, 0x05, 0, 0, 0x03, 0x25, 0x05, 0, 0x67, 0x80, 0, 0,
    ];
    let f = analyze(object(&bytes, bytes.len() as u64, true));
    assert!(f.coverage.complete());
    assert_eq!(
        f.count(|r| matches!(r, FunctionRecord::Reference { .. })),
        3
    );
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::Edge {
            from: 4,
            target: None,
            relation: EdgeKind::Call,
            external: true
        }
    )));
    assert!(f.records.iter().any(|r| matches!(r, FunctionRecord::Reference { target, known: true, .. } if target.name == b"data" && target.section.is_none())));
}

#[test]
fn unknown_instructions_and_indirect_jumps_keep_partial_results() {
    for bytes in [&[0xff, 0xff, 0xff, 0xff][..], &[0x67, 0, 0x03, 0][..]] {
        let f = analyze(object(bytes, bytes.len() as u64, false));
        assert!(!f.coverage.control_flow);
        assert!(!f.records.is_empty());
    }
}

#[test]
fn targets_inside_instructions_and_truncation_are_visible_gaps() {
    // beq zero, zero, +2 targets the second halfword of itself.
    for bytes in [&[0x63, 0x01, 0, 0, 0x67, 0x80, 0, 0][..], &[0x13, 0, 0][..]] {
        let f = analyze(object(bytes, bytes.len() as u64, false));
        assert!(!f.coverage.complete());
        assert!(f.records.iter().any(|r| matches!(
            r,
            FunctionRecord::Gap { .. }
                | FunctionRecord::Edge {
                    relation: EdgeKind::Conflict,
                    ..
                }
        )));
    }
}

#[test]
fn relax_with_null_symbol_is_metadata_not_a_missing_definition() {
    use object::{Object as _, ObjectSection as _};
    let code = [
        0x97, 0, 0, 0, 0xe7, 0x80, 0, 0, 0x37, 0x05, 0, 0, 0x03, 0x25, 0x05, 0, 0x67, 0x80, 0, 0,
    ];
    let mut bytes = object(&code, code.len() as u64, true);
    let file = object::File::parse(&*bytes).unwrap();
    let (offset, _) = file
        .section_by_name(".rela.text.entry")
        .unwrap()
        .file_range()
        .unwrap();
    // Replace the third relocation with a valid r_sym=0 RELAX marker.
    let info = offset as usize + 2 * 12 + 4;
    bytes[info..info + 4].copy_from_slice(&object::elf::R_RISCV_RELAX.to_le_bytes());
    let f = analyze(bytes);
    assert!(f.records.iter().any(|r| matches!(r, FunctionRecord::Reference { raw, reference_kind: ReferenceKind::Metadata, known: true, .. } if raw.target.definition == SymbolDefinition::Null && raw.target.symbol.index == 0)));
}

#[test]
fn cycles_terminate_and_budgets_stop_the_analysis() {
    let f = analyze(object(&[0x6f, 0, 0, 0], 4, false));
    assert_eq!(f.instructions(), 1);
    let error = analyze_with(object(&[0x6f, 0, 0, 0], 4, false), 1024, &mut limits())
        .err()
        .unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
    let mut code = [1, 0].repeat(20000);
    code.extend_from_slice(&[0x67, 0x80, 0, 0]);
    for mut control in [
        Limits::new(1, std::time::Duration::from_secs(60)),
        Limits::new(DEFAULT_WORK_UNITS, std::time::Duration::ZERO),
    ] {
        let error = analyze_with(
            object(&code, code.len() as u64, false),
            32 * 1024 * 1024,
            &mut control,
        )
        .err()
        .unwrap();
        assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    }
}

#[test]
fn values_and_memory_effects_are_resolved_to_constant_and_stack_addresses() {
    let code = words(&[
        0x60000537, 0x12050513, 0x02a00593, 0x00b52223, 0x00852603, 0xff010113, 0x00112623,
        0x00900013, 0x00008067,
    ]);
    let f = analyze(object(&code, code.len() as u64, false));
    assert!(f.coverage.complete());
    assert!(f.semantics.complete);
    assert_eq!(f.semantics.accesses, 3);
    assert_eq!(f.semantics.known_addresses, 3);
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::MemoryAccess {
            offset: 12,
            access: MemoryKind::Store,
            width: 4,
            address: AbstractValue::Constant { value: 0x60000124 },
            value: Some(AbstractValue::Constant { value: 42 }),
            ..
        }
    )));
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::MemoryAccess {
            offset: 16,
            access: MemoryKind::Load,
            address: AbstractValue::Constant { value: 0x60000128 },
            value: None,
            ..
        }
    )));
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::Value {
            register: 12,
            value: AbstractValue::Expression { .. },
            ..
        }
    )));
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::MemoryAccess {
            offset: 24,
            address: AbstractValue::EntryStack { offset: -4 },
            ..
        }
    )));
}

#[test]
fn opaque_calls_and_atomic_accesses_preserve_unknowns() {
    // li a0, 64; jal ra, +128; lw a1, 0(a0); lr.w a2,(a0);
    // sc.w a3,a1,(a0); amoadd.w a4,a1,(a0); ret
    let code = words(&[
        0x04000513, 0x080000ef, 0x00052583, 0x1005262f, 0x18b526af, 0x00b5272f, 0x00008067,
    ]);
    let f = analyze(object(&code, code.len() as u64, false));
    assert!(!f.semantics.complete);
    for kind in [
        MemoryKind::Load,
        MemoryKind::LoadReserved,
        MemoryKind::StoreConditional,
        MemoryKind::Atomic,
    ] {
        assert!(f.records.iter().any(|r| matches!(r, FunctionRecord::MemoryAccess { access, address: AbstractValue::Unknown | AbstractValue::Expression { .. }, .. } if *access == kind)), "{kind:?}");
    }
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            reason: SemanticGapReason::OpaqueCall,
            ..
        }
    )));
}

fn pcrel_object(clobber: bool, ambiguous: bool) -> Vec<u8> {
    let code = words(&[
        0x00000517,
        0x02a00593,
        if clobber { 0x00000513 } else { 0x00000013 },
        0x00052603,
        0x00008067,
    ]);
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &code, 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        code.len() as u64,
        SymbolKind::Text,
    ));
    let label = obj.add_symbol(symbol(
        b"hi_label",
        SymbolSection::Section(text),
        0,
        SymbolKind::Text,
    ));
    let data = obj.add_symbol(symbol(
        b"data",
        SymbolSection::Undefined,
        0,
        SymbolKind::Data,
    ));
    let mut relocations = vec![
        (0, data, object::elf::R_RISCV_PCREL_HI20, 4),
        (12, label, object::elf::R_RISCV_PCREL_LO12_I, 0),
    ];
    if ambiguous {
        relocations.push((0, data, object::elf::R_RISCV_PCREL_HI20, 8));
    }
    for (offset, symbol, r_type, addend) in relocations {
        obj.add_relocation(
            text,
            Relocation {
                offset,
                symbol,
                addend,
                flags: RelocationFlags::Elf { r_type },
            },
        )
        .unwrap();
    }
    obj.write().unwrap()
}

#[test]
fn relocated_addresses_require_identity_pairing_and_an_unclobbered_register() {
    for (clobber, ambiguous) in [(false, false), (true, false), (false, true)] {
        let f = analyze(pcrel_object(clobber, ambiguous));
        let access = f
            .records
            .iter()
            .find_map(|r| match r {
                FunctionRecord::MemoryAccess {
                    offset: 12,
                    address,
                    relocation,
                    ..
                } => Some((address, relocation)),
                _ => None,
            })
            .unwrap();
        if clobber || ambiguous {
            assert_eq!(access.0, &AbstractValue::Unknown);
            assert!(!f.semantics.complete);
        } else {
            assert!(f.semantics.complete);
            let target = f
                .records
                .iter()
                .find_map(|r| match r {
                    FunctionRecord::Reference { raw, target, .. } if raw.offset == 0 => {
                        Some(&target.symbol)
                    }
                    _ => None,
                })
                .unwrap();
            assert_eq!(
                access.0,
                &AbstractValue::Symbol {
                    symbol: target.clone(),
                    addend: 4
                }
            );
            assert!(access.1.is_some());
        }
        // AUIPC upper alone is not the final relocated address.
        assert!(f.records.iter().any(|r| matches!(
            r,
            FunctionRecord::Value {
                offset: 0,
                value: AbstractValue::Unknown,
                ..
            }
        )));
    }
}

#[test]
fn value_state_capacity_failure_names_its_phase() {
    let mut code = [1, 0].repeat(1000);
    code.extend_from_slice(&0x00008067u32.to_le_bytes());
    let error = analyze_with(
        object(&code, code.len() as u64, false),
        2 * 1024 * 1024,
        &mut limits(),
    )
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited, "{error:?}");
    assert_eq!(error.memory.unwrap().phase, RunPhase::AnalyzeValues);
}

#[test]
fn a_relocated_tail_call_is_an_opaque_effect() {
    let code = words(&[0x00000317, 0x00030067]);
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &code, 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        8,
        SymbolKind::Text,
    ));
    let callee = obj.add_symbol(symbol(
        b"tail",
        SymbolSection::Undefined,
        0,
        SymbolKind::Text,
    ));
    obj.add_relocation(
        text,
        Relocation {
            offset: 0,
            symbol: callee,
            addend: 0,
            flags: RelocationFlags::Elf {
                r_type: object::elf::R_RISCV_CALL,
            },
        },
    )
    .unwrap();
    let f = analyze(obj.write().unwrap());
    assert!(!f.semantics.complete);
    assert_eq!(f.semantics.gaps, 1);
    assert!(f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            offset: 4,
            reason: SemanticGapReason::OpaqueCall
        }
    )));
    assert!(!f.records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            reason: SemanticGapReason::UnresolvedRelocation,
            ..
        }
    )));
}

#[test]
fn ten_thousand_section_relocations_fit_small_function_capacity() {
    let mut obj = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = obj.add_section(Vec::new(), b".text.entry".to_vec(), SectionKind::Text);
    obj.append_section_data(text, &words(&vec![0x00008067; 10001]), 4);
    obj.add_symbol(symbol(
        b"entry",
        SymbolSection::Section(text),
        4,
        SymbolKind::Text,
    ));
    let external = obj.add_symbol(symbol(
        b"external",
        SymbolSection::Undefined,
        0,
        SymbolKind::Data,
    ));
    for i in 1..=10000 {
        obj.add_relocation(
            text,
            Relocation {
                offset: i * 4,
                symbol: external,
                addend: 0,
                flags: RelocationFlags::Elf {
                    r_type: object::elf::R_RISCV_32,
                },
            },
        )
        .unwrap();
    }
    let f = analyze_with(obj.write().unwrap(), 16 * 1024 * 1024, &mut limits()).unwrap();
    assert_eq!(f.instructions(), 1);
}
