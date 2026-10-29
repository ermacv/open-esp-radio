use super::*;
use crate::tests::{compiled, ledger, with_facts};
use crate::{Analysis, DebugMetadata, StorageEvidence, UnknownUse, read_debug};
use oer_analysis_elf::rv32::{self, Role};
use oer_analysis_riscv_model::{ErrorCode, UnknownCallBehavior};
use oer_analysis_riscv_program::facts::ImageFacts;
use std::rc::Rc;

const PROGRAM: &str = r#"
#![no_std]
#![no_main]
use core::hint::black_box;
#[repr(C)]
pub struct Wanted { marker: u32, run: fn(u32) -> u32 }
#[repr(C)]
pub struct Stored { run: fn(u16) -> u16 }
#[unsafe(no_mangle)]
#[inline(never)]
pub fn different(value: u16) -> u16 { black_box(value.wrapping_add(2)) }
#[inline(never)]
fn target(value: u32) -> u32 { black_box(value.wrapping_add(1)) }
#[unsafe(no_mangle)]
pub static mut CALLBACKS: Wanted = Wanted { marker: 7, run: target };
#[unsafe(no_mangle)]
pub static OTHER: fn(u16) -> u16 = different;
#[unsafe(no_mangle)]
#[inline(never)]
pub extern "C" fn install(stored: &mut Stored) {
    unsafe { core::arch::asm!(
        ".option push", ".option norelax",
        "2: auipc t0, %pcrel_hi({function})",
        "addi t0, t0, %pcrel_lo(2b)",
        "sw t0, 0({storage})",
        "li t0, 0",
        ".option pop",
        function = sym different, storage = in(reg) stored,
        out("t0") _, options(nostack)
    ); }
}
#[unsafe(no_mangle)]
pub extern "C" fn _start(value: u32, callbacks: &Wanted) -> u32 {
    (callbacks.run)(value).wrapping_add(callbacks.marker)
}
#[unsafe(no_mangle)]
pub extern "C" fn global(value: u32) -> u32 {
    (unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CALLBACKS.run)) })(value)
}
#[unsafe(no_mangle)]
pub extern "C" fn keep(stored: &mut Stored) {
    install(stored); black_box(&OTHER); black_box(global as extern "C" fn(u32) -> u32);
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! { loop {} }
"#;

fn named(analysis: &Analysis, name: &str) -> u32 {
    analysis
        .functions
        .values()
        .find(|facts| {
            facts.function.names.iter().any(|symbol| {
                let symbol = oer_analysis_elf::demangle(symbol);
                symbol == name || symbol.ends_with(&format!("::{name}"))
            })
        })
        .unwrap()
        .function
        .address
}

fn with_audit(
    elf: &[u8],
    check: impl FnOnce(&ImageFacts<'_>, &Analysis, &FunctionPointerAudit<'_, '_>) -> Result<()>,
) {
    let (_, types) = read_debug(
        &Rc::from(elf),
        DebugMetadata::TypedLocations,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    with_facts(elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        check(image, &analysis, &audit)
    })
    .unwrap();
}

fn assert_both(analysis: &Analysis, audit: &FunctionPointerAudit<'_, '_>, candidate: bool) {
    let global = function_pointer_resolutions(audit, &mut ledger(), &mut || Ok(())).unwrap();
    let typed = typed_function_pointer_resolutions(audit, &mut ledger(), &mut || Ok(())).unwrap();
    let site = |name| {
        analysis.functions[&named(analysis, name)]
            .transfers
            .iter()
            .find(|t| t.target.is_none())
            .unwrap()
            .site
    };
    let global = global.get(site("global")).expect("global field");
    let typed = typed.get(site("_start")).expect("local field");
    assert_eq!(global.targets, typed.targets);
    assert_eq!(
        global.targets.contains(&named(analysis, "different")),
        candidate
    );
}

#[test]
fn a_pc_relative_address_reaches_a_typed_word_store() {
    let elf = compiled(PROGRAM);
    with_audit(&elf, |image, analysis, audit| {
        let function = named(analysis, "different");
        let pair = audit
            .pc_relative_uses()
            .iter()
            .find(|p| p.functions.contains(&function))
            .expect("function pair");
        assert_eq!(pair.target, Some(function));
        let high = &image.address_uses()[pair.high.unwrap()];
        let low = &image.address_uses()[pair.low];
        assert_eq!(high.role, Role::PcRelativeHigh);
        assert_eq!(low.role, Role::PcRelativeLow);
        assert_ne!(
            low.symbol.as_ref().unwrap().address,
            function.into(),
            "low still names the AUIPC label"
        );
        let uses = audit
            .uses()
            .find(|(entry, _)| *entry == function)
            .unwrap()
            .1;
        for index in [pair.low, pair.high.unwrap()] {
            let proof = uses.iter().find(|u| u.index == index).unwrap();
            assert!(
                matches!(proof.evidence, StorageEvidence::Typed(_)),
                "{proof:?}"
            );
            assert_eq!(proof.stores.len(), 1);
            assert!(proof.barrier.is_none());
        }
        assert_both(analysis, audit, false);
        Ok(())
    });
}

struct PairBytes {
    high: usize,
    low: usize,
    high_word: usize,
    low_word: usize,
    high_site: u32,
    label: usize,
    symbols: usize,
    target: u32,
    section: usize,
    spare: usize,
    foreign_section: usize,
}

fn pair_bytes(elf: &[u8]) -> PairBytes {
    let file = oer_analysis_elf::Elf::parse(elf).unwrap();
    let function = file.symbols().find(|s| s.name == "different").unwrap();
    let (section, high) = file
        .sections()
        .find_map(|s| {
            file.relocations(s.index)
                .ok()?
                .find(|r| {
                    r.r_type == rv32::R_RISCV_PCREL_HI20
                        && r.target == oer_analysis_elf::Target::Symbol(function.index)
                })
                .map(|r| (s, r))
        })
        .unwrap();
    let low = file
        .relocations(section.index)
        .unwrap()
        .find(|r| {
            r.r_type == rv32::R_RISCV_PCREL_LO12_I
                && match r.target {
                    oer_analysis_elf::Target::Symbol(index) => {
                        file.symbol_at(index).unwrap().address == high.at
                    }
                    _ => false,
                }
        })
        .unwrap();
    let rela = file
        .sections()
        .find(|s| {
            s.kind == oer_analysis_elf::SectionKind::Rela && s.relocates == Some(section.index)
        })
        .unwrap();
    let offset = |r: &oer_analysis_elf::Relocation| {
        rela.file_offset as usize
            + 12 * rela
                .data
                .chunks_exact(12)
                .position(|entry| {
                    u32::from_le_bytes(entry[..4].try_into().unwrap()) == r.at as u32
                        && (u32::from_le_bytes(entry[4..8].try_into().unwrap()) & 255) == r.r_type
                })
                .unwrap()
    };
    let spare = rela
        .data
        .chunks_exact(12)
        .position(|entry| {
            rv32::transfers(u32::from_le_bytes(entry[4..8].try_into().unwrap()) & 255)
        })
        .unwrap();
    let label = if let oer_analysis_elf::Target::Symbol(index) = low.target {
        Some(index)
    } else {
        None
    }
    .expect("low label symbol");
    PairBytes {
        high: offset(&high),
        low: offset(&low),
        high_word: section.file_offset as usize + (high.at - section.address) as usize,
        low_word: section.file_offset as usize + (low.at - section.address) as usize,
        high_site: high.at as u32,
        label,
        symbols: file
            .section(file.symbol_table_section().unwrap())
            .unwrap()
            .file_offset as usize,
        target: function.address as u32,
        section: section.index,
        spare: rela.file_offset as usize + spare * 12,
        foreign_section: file
            .sections()
            .find(|s| s.allocated && !s.executable && s.index != section.index)
            .unwrap()
            .index,
    }
}

fn set_u32(elf: &mut [u8], at: usize, value: u32) {
    elf[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn get_u32(elf: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(elf[at..at + 4].try_into().unwrap())
}

fn set_materialized_target(elf: &mut [u8], p: &PairBytes, target: u32) {
    let delta = target.wrapping_sub(p.high_site);
    let upper = delta.wrapping_add(0x800) & 0xffff_f000;
    let lower = delta.wrapping_sub(upper);
    let high = get_u32(elf, p.high_word);
    let low = get_u32(elf, p.low_word);
    set_u32(elf, p.high_word, (high & 0xfff) | upper);
    set_u32(elf, p.low_word, (low & 0x000f_ffff) | (lower << 20));
}

fn assert_barrier(elf: &[u8], reason: UnknownUse) {
    with_audit(elf, |_, analysis, audit| {
        let function = named(analysis, "different");
        let uses = audit
            .uses()
            .find(|(entry, _)| *entry == function)
            .unwrap()
            .1;
        assert!(
            uses.iter()
                .any(|u| matches!(u.evidence, StorageEvidence::Unknown(found) if found == reason)),
            "{uses:?}"
        );
        assert!(
            uses.iter()
                .any(|u| matches!(u.evidence, StorageEvidence::Typed(_))),
            "known static storage must remain alongside the unknown use"
        );
        assert_both(analysis, audit, true);
        Ok(())
    });
}

#[test]
fn label_identity_pairs_nonadjacent_uses_even_with_equal_symbol_names() {
    let source = PROGRAM.replace("\"addi t0, t0, %pcrel_lo(2b)\",", "\"3: auipc t1, %pcrel_hi({other})\",\n\"addi t1, t1, %pcrel_lo(3b)\",\n\"addi t0, t0, %pcrel_lo(2b)\",")
        .replace("function = sym different,", "other = sym OTHER, function = sym different,")
        .replace("out(\"t0\") _,", "out(\"t0\") _, out(\"t1\") _,");
    let mut elf = compiled(&source);
    let p = pair_bytes(&elf);
    let other_label = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        file.relocations(p.section)
            .unwrap()
            .filter(|r| r.r_type == rv32::R_RISCV_PCREL_LO12_I)
            .find_map(|r| match r.target {
                oer_analysis_elf::Target::Symbol(index) if index != p.label => Some(index),
                _ => None,
            })
            .unwrap()
    };
    let name = get_u32(&elf, p.symbols + p.label * 16);
    set_u32(&mut elf, p.symbols + other_label * 16, name);
    with_audit(&elf, |image, analysis, audit| {
        let function = named(analysis, "different");
        let p = audit
            .pc_relative_uses()
            .iter()
            .find(|p| p.functions.contains(&function))
            .unwrap();
        assert!(
            p.low > p.high.unwrap() + 1,
            "there must be intervening relocation observations"
        );
        assert!(
            image.address_uses()[p.low].relocation.at
                > image.address_uses()[p.high.unwrap()].relocation.at + 4
        );
        assert_eq!(p.target, Some(function));
        assert_both(analysis, audit, false);
        Ok(())
    });
}

#[test]
fn absent_ambiguous_wrong_and_cross_section_pairs_never_use_the_label_as_a_callback() {
    let original = compiled(PROGRAM);
    let p = pair_bytes(&original);
    for variant in 0..5 {
        let mut elf = original.clone();
        match variant {
            0 => elf[p.high + 4] = rv32::R_RISCV_NONE as u8,
            1 => {
                let copy = elf[p.high..p.high + 12].to_vec();
                elf[p.spare..p.spare + 12].copy_from_slice(&copy);
            }
            2 => {
                let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
                let wrong = file
                    .symbols()
                    .find(|s| s.name == "different")
                    .unwrap()
                    .index;
                set_u32(
                    &mut elf,
                    p.low + 4,
                    (wrong as u32) << 8 | rv32::R_RISCV_PCREL_LO12_I,
                );
            }
            3 => {
                // Same numeric AUIPC address, but a label in another section.
                elf[p.symbols + p.label * 16 + 14..p.symbols + p.label * 16 + 16]
                    .copy_from_slice(&(p.foreign_section as u16).to_le_bytes());
            }
            _ => set_u32(&mut elf, p.low + 8, 4),
        }
        with_audit(&elf, |_, analysis, audit| {
            assert!(
                audit
                    .pc_relative_uses()
                    .iter()
                    .any(|p| p.unknown == Some(UnknownUse::InvalidPcRelativePair)),
                "variant {variant}"
            );
            let owner = named(analysis, "install");
            assert!(
                audit.uses().all(|(function, _)| function != owner),
                "an AUIPC label is not an address use of its containing function"
            );
            assert_both(analysis, audit, true);
            Ok(())
        });
    }
}

#[test]
fn overwritten_upper_values_and_incoming_bypass_paths_remain_unknown() {
    let overwritten = PROGRAM.replace(
        "\"addi t0, t0, %pcrel_lo(2b)\",",
        "\"li t0, 0\", \"addi t0, t0, %pcrel_lo(2b)\",",
    );
    assert_barrier(&compiled(&overwritten), UnknownUse::RegisterClobbered);
    for destination in ["addi t0, t0, %pcrel_lo(2b)", "sw t0, 0({storage})"] {
        let source = PROGRAM
            .replace("\"2: auipc", "\"beqz {storage}, 4f\", \"2: auipc")
            .replace(
                &format!("\"{destination}\""),
                &format!("\"4: {destination}\""),
            );
        assert_barrier(&compiled(&source), UnknownUse::IncomingPath);
    }
}

#[test]
fn wrong_base_registers_do_not_inherit_a_correctly_normalized_pair() {
    let mut elf = compiled(PROGRAM);
    let p = pair_bytes(&elf);
    let low = get_u32(&elf, p.low_word);
    set_u32(&mut elf, p.low_word, (low & !(31 << 15)) | (6 << 15));
    assert_barrier(&elf, UnknownUse::UnsupportedMaterialization);
}

#[test]
fn addends_and_interior_symbols_keep_the_original_function_possible() {
    let original = compiled(PROGRAM);
    let p = pair_bytes(&original);
    let mut addend = original.clone();
    set_u32(&mut addend, p.high + 8, 2);
    set_materialized_target(&mut addend, &p, p.target + 2);
    assert_barrier(&addend, UnknownUse::NonExactAddress);
    let mut interior = original;
    let (index, section) = {
        let file = oer_analysis_elf::Elf::parse(&interior).unwrap();
        let index = file
            .symbols()
            .find(|s| {
                s.size == 0
                    && s.kind == oer_analysis_elf::SymbolKind::Unknown
                    && s.section
                        .is_some_and(|i| !file.section(i).unwrap().allocated)
            })
            .unwrap()
            .index;
        (
            index,
            file.symbols()
                .find(|s| s.name == "different")
                .unwrap()
                .section
                .unwrap(),
        )
    };
    set_u32(&mut interior, p.symbols + index * 16 + 4, p.target + 2);
    interior[p.symbols + index * 16 + 14..p.symbols + index * 16 + 16]
        .copy_from_slice(&(section as u16).to_le_bytes());
    set_u32(
        &mut interior,
        p.high + 4,
        (index as u32) << 8 | rv32::R_RISCV_PCREL_HI20,
    );
    set_materialized_target(&mut interior, &p, p.target + 2);
    assert_barrier(&interior, UnknownUse::NonExactAddress);
}

#[test]
fn an_unknown_store_or_another_unknown_use_blocks_both_resolvers_exclusion() {
    let unknown_store = PROGRAM.replace("sw t0, 0({storage})", "sw t0, 4({storage})");
    assert_barrier(&compiled(&unknown_store), UnknownUse::UnknownWrite);
    let another_use = PROGRAM.replace(
        "install(stored);",
        "install(stored); black_box(different as *const () as usize);",
    );
    with_audit(&compiled(&another_use), |_, analysis, audit| {
        let function = named(analysis, "different");
        let p = audit
            .pc_relative_uses()
            .iter()
            .find(|p| p.functions.contains(&function))
            .unwrap();
        let uses = audit
            .uses()
            .find(|(entry, _)| *entry == function)
            .unwrap()
            .1;
        assert!(
            uses.iter()
                .any(|u| u.index == p.low && matches!(u.evidence, StorageEvidence::Typed(_)))
        );
        assert!(
            uses.iter()
                .any(|u| matches!(u.evidence, StorageEvidence::Unknown(_)))
        );
        assert_both(analysis, audit, true);
        Ok(())
    });
}

#[test]
fn equal_numeric_addresses_in_distinct_elfs_cannot_share_pair_or_type_facts() {
    let first = compiled(PROGRAM);
    let second = compiled(&PROGRAM.replace("marker: 7", "marker: 8"));
    assert_eq!(pair_bytes(&first).high_site, pair_bytes(&second).high_site);
    let (_, types) = read_debug(
        &Rc::from(&first[..]),
        DebugMetadata::TypedLocations,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    with_facts(&second, |image| {
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut ledger(),
            &mut || Ok(()),
        )?;
        let error =
            audit_function_addresses(image, &analysis, &types, &mut ledger(), &mut || Ok(()))
                .err()
                .expect("foreign ELF");
        assert_eq!(error.code, ErrorCode::Incompatible);
        Ok(())
    })
    .unwrap();
    with_audit(&second, |_, analysis, audit| {
        assert_both(analysis, audit, false);
        Ok(())
    });
}

#[test]
fn normalization_alone_does_not_prove_other_lowering_forms_or_dead_values() {
    let store_low = PROGRAM.replace("addi t0, t0, %pcrel_lo(2b)", "sw zero, %pcrel_lo(2b)(t0)");
    assert_barrier(
        &compiled(&store_low),
        UnknownUse::UnsupportedMaterialization,
    );
    let dead = PROGRAM.replace("sw t0, 0({storage})", "nop # {storage}");
    assert_barrier(&compiled(&dead), UnknownUse::NoStorage);
}

#[test]
fn normalization_and_proof_temporary_storage_obey_resource_limits() {
    let elf = compiled(PROGRAM);
    let (_, types) = read_debug(
        &Rc::from(&elf[..]),
        DebugMetadata::TypedLocations,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    with_facts(&elf, |image| {
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut ledger(),
            &mut || Ok(()),
        )?;
        let memory = oer_analysis_riscv_model::WorkingMemory::new(1 << 24)?;
        let mut total = 0;
        {
            let mut keep = oer_analysis_riscv_model::Ledger::new(&memory, &mut || Ok(()))?;
            let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || {
                total += 1;
                Ok(())
            })?;
            assert!(!audit.pc_relative_uses().is_empty());
        }
        assert_eq!(memory.used(), 0);
        for stop in [1, total / 3, total * 2 / 3, total] {
            let mut checkpoints = 0;
            {
                let mut keep = oer_analysis_riscv_model::Ledger::new(&memory, &mut || Ok(()))?;
                let error =
                    audit_function_addresses(image, &analysis, &types, &mut keep, &mut || {
                        checkpoints += 1;
                        if checkpoints == stop {
                            Err(oer_analysis_riscv_model::Error::new(
                                ErrorCode::Cancelled,
                                "cancelled",
                            ))
                        } else {
                            Ok(())
                        }
                    })
                    .err()
                    .expect("cancelled audit");
                assert_eq!(error.code, ErrorCode::Cancelled);
            }
            assert_eq!(memory.used(), 0);
        }
        let tiny = oer_analysis_riscv_model::WorkingMemory::new(1)?;
        {
            let mut keep = oer_analysis_riscv_model::Ledger::new(&tiny, &mut || Ok(()))?;
            let error =
                audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))
                    .err()
                    .expect("memory limit");
            assert_eq!(error.code, ErrorCode::ResourceLimited);
        }
        assert_eq!(tiny.used(), 0);
        Ok(())
    })
    .unwrap();
}
