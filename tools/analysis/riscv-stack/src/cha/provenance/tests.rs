use super::*;
use crate::Analysis;
use crate::cha::TypeFacts;
use crate::cha::locations::Locations;
use crate::dwarf::Reader;
use crate::dwarf::locations::LocalLocations;
use crate::tests::{compiled, ledger, with_facts};
use crate::{StorageEvidence, UnknownUse, audit_function_addresses};
use oer_analysis_elf::dwarf::gimli;
use oer_analysis_elf::rv32::Role;
use oer_analysis_riscv_program::facts::ImageFacts;

fn typed_function_pointer_resolutions(
    image: &ImageFacts<'_>,
    analysis: &Analysis,
    types: &TypeFacts,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<Resolutions> {
    let mut scratch = Ledger::new(keep.memory(), control)?;
    let audit = audit_function_addresses(image, analysis, types, &mut scratch, control)?;
    crate::typed_function_pointer_resolutions(&audit, keep, control)
}

fn typed_debug(elf: &[u8]) -> (crate::Dwarf, TypeFacts) {
    crate::read_debug(
        &std::rc::Rc::from(elf),
        crate::DebugMetadata::TypedLocations,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap()
}

const PROGRAM: &str = r#"
#![no_std]
#![no_main]
use core::hint::black_box;

#[repr(C)]
pub struct Callbacks { marker: u32, run: fn(u32) -> u32 }
#[inline(never)]
fn target(value: u32) -> u32 { black_box(value.wrapping_add(1)) }
#[inline(never)]
fn different(value: u16) -> u16 { black_box(value.wrapping_add(2)) }
#[unsafe(no_mangle)]
pub static CALLBACKS: Callbacks = Callbacks { marker: 7, run: target };
#[unsafe(no_mangle)]
pub static OTHER: fn(u16) -> u16 = different;
#[unsafe(no_mangle)]
pub extern "C" fn _start(value: u32, callbacks: &Callbacks) -> u32 {
    (callbacks.run)(value).wrapping_add(callbacks.marker)
}
#[unsafe(no_mangle)]
pub extern "C" fn keep() -> *const Callbacks {
    black_box(&OTHER);
    &CALLBACKS
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! { loop {} }
"#;

fn named(analysis: &Analysis, needle: &str) -> u32 {
    analysis
        .functions
        .values()
        .find(|facts| {
            facts.function.names.iter().any(|name| {
                let name = oer_analysis_elf::demangle(name);
                name == needle || name.ends_with(&format!("::{needle}"))
            })
        })
        .unwrap_or_else(|| panic!("missing {needle}"))
        .function
        .address
}

#[test]
fn a_pointer_parameter_names_its_function_field_at_the_load() {
    let elf = compiled(PROGRAM);
    let (_, types) = typed_debug(&elf);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let mut analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let mut locations = Locations::default();
        locations.read_aliases(image, &mut keep, &mut || Ok(()))?;
        let start = named(&analysis, "_start");
        let transfer = analysis.functions[&start]
            .transfers
            .iter()
            .find(|transfer| transfer.target.is_none())
            .unwrap();
        let load = transfer.load.expect("load provenance");
        assert!(
            locations
                .field(&types, load.site, load.base, load.offset, &mut || Ok(()))?
                .is_some(),
            "{load:?}"
        );
        // A joint analysis may also contain a ROM companion whose section
        // indexes and code must never be interpreted through this ELF.
        let site = transfer.site;
        let mut companion = analysis.functions[&start].clone();
        companion.function.address = 0xffff_0000;
        companion.function.section = usize::MAX;
        analysis
            .functions
            .insert(companion.function.address, companion);
        let resolutions = typed_function_pointer_resolutions(
            image,
            &analysis,
            &types,
            &mut keep,
            &mut || Ok(()),
        )?;
        let resolution = resolutions.get(site).expect("typed receiver resolution");
        assert!(resolution.targets.contains(&named(&analysis, "target")));
        assert!(
            !resolution.targets.contains(&named(&analysis, "different")),
            "an incompatible function is excluded only through its typed storage use"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn one_erased_address_use_keeps_an_incompatible_declaration_a_candidate() {
    let source = PROGRAM.replace(
        "black_box(&OTHER);",
        "black_box(&OTHER); black_box(different as *const () as usize);",
    );
    let elf = compiled(&source);
    let (_, types) = typed_debug(&elf);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let resolutions = typed_function_pointer_resolutions(
            image,
            &analysis,
            &types,
            &mut keep,
            &mut || Ok(()),
        )?;
        let start = named(&analysis, "_start");
        let transfer = analysis.functions[&start]
            .transfers
            .iter()
            .find(|transfer| transfer.target.is_none())
            .unwrap();
        assert!(
            resolutions
                .get(transfer.site)
                .unwrap()
                .targets
                .contains(&named(&analysis, "different")),
            "the integer escape defeats exclusion despite different declared parameters and result"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_function_stored_after_a_signature_cast_has_its_storage_type() {
    let source = PROGRAM.replace(
        "pub static OTHER: fn(u16) -> u16 = different;",
        "pub static OTHER: fn(u32) -> u32 = unsafe { core::mem::transmute(different as fn(u16) -> u16) };",
    );
    let elf = compiled(&source);
    let (_, types) = typed_debug(&elf);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(image, &[], &[], &UnknownCallBehavior, &mut keep, &mut || Ok(()))?;
        let resolutions = typed_function_pointer_resolutions(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        let start = named(&analysis, "_start");
        let transfer = analysis.functions[&start].transfers.iter().find(|transfer| transfer.target.is_none()).unwrap();
        assert!(resolutions.get(transfer.site).unwrap().targets.contains(&named(&analysis, "different")), "the stored function type, rather than the declaration, decides whether a candidate can match");
        Ok(())
    }).unwrap();
}

#[test]
fn an_optimized_out_piece_does_not_shift_the_next_stack_field() {
    let elf = compiled(PROGRAM);
    let (_, types) = typed_debug(&elf);
    let ty = types
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Callbacks"))
        .unwrap()
        .0;
    let mut prepared = LocalLocations::default();
    prepared.variables.insert(0, (Some(*ty), None));
    // marker (4 bytes) has no location. run lives in sp+36, with its
    // original aggregate offset still four bytes.
    let expression = gimli::Expression(Reader::new(
        &[0x93, 4, 0x72, 36, 0x93, 4],
        gimli::RunTimeEndian::Little,
    ));
    prepared
        .expression(
            expression,
            gimli::Encoding {
                address_size: 4,
                format: gimli::Format::Dwarf32,
                version: 4,
            },
            0,
            &[(100, 200)],
            &mut ledger(),
            &mut || Ok(()),
        )
        .unwrap();
    let mut types = types;
    types.locations = Some(prepared);
    let locations = Locations::default();
    assert!(
        locations
            .field(&types, 100, 2, 36, &mut || Ok(()))
            .unwrap()
            .is_some()
    );
    assert!(
        locations
            .field(&types, 200, 2, 36, &mut || Ok(()))
            .unwrap()
            .is_none(),
        "ranges stay half-open"
    );
    assert!(
        locations
            .field(&types, 100, 2, 40, &mut || Ok(()))
            .unwrap()
            .is_none(),
        "only initialized pieces are evidence"
    );
}

#[test]
fn reading_locations_respects_the_work_budget() {
    let elf = compiled(PROGRAM);
    let mut keep = ledger();
    let error = crate::read_debug(
        &std::rc::Rc::from(&elf[..]),
        crate::DebugMetadata::TypedLocations,
        &mut keep,
        &mut || Err(Error::new(ErrorCode::Cancelled, "cancelled")),
    )
    .err()
    .expect("cancelled while walking DWARF");
    assert_eq!(error.code, ErrorCode::Cancelled);
}

#[test]
fn one_unknown_relocation_among_typed_uses_keeps_the_function_a_candidate() {
    let source = global_program().replace(
        "black_box(&OTHER);",
        "black_box(&OTHER); black_box(different as *const () as usize);",
    );
    let mut elf = compiled(&source);
    let at = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        let different = file
            .symbols()
            .find(|symbol| symbol.demangled().ends_with("::different"))
            .unwrap();
        let section = file
            .sections()
            .find(|section| {
                section.executable
                    && file.relocations(section.index).unwrap().any(|relocation| {
                        relocation.target == oer_analysis_elf::Target::Symbol(different.index)
                            && relocation.r_type == oer_analysis_elf::rv32::R_RISCV_HI20
                    })
            })
            .unwrap();
        let rela = file
            .sections()
            .find(|rela| {
                rela.kind == oer_analysis_elf::SectionKind::Rela
                    && rela.relocates == Some(section.index)
            })
            .unwrap();
        let index = rela
            .data
            .chunks_exact(12)
            .position(|entry| {
                u32::from_le_bytes(entry[4..8].try_into().unwrap())
                    == ((different.index as u32) << 8 | oer_analysis_elf::rv32::R_RISCV_HI20)
            })
            .unwrap();
        rela.file_offset as usize + index * 12 + 4
    };
    // Keep the symbol identity and addend, but make the role unfamiliar.
    elf[at] = 0xff;
    check_both(&elf, true, Some(UnknownUse::UnsupportedRelocation));
    let (_, types) = typed_debug(&elf);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let different = named(&analysis, "different");
        assert!(image.address_uses().iter().any(|use_| {
            use_.role == Role::Other
                && use_
                    .function
                    .nearest
                    .is_some_and(|relation| relation.entry == different)
        }));
        let resolutions = typed_function_pointer_resolutions(
            image,
            &analysis,
            &types,
            &mut keep,
            &mut || Ok(()),
        )?;
        let transfer = analysis.functions[&named(&analysis, "_start")]
            .transfers
            .iter()
            .find(|transfer| transfer.target.is_none())
            .unwrap();
        assert!(
            resolutions
                .get(transfer.site)
                .unwrap()
                .targets
                .contains(&different)
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_resolver_requires_prepared_locations_without_implicitly_loading_them() {
    let elf = compiled(PROGRAM);
    let (_, types) = crate::tests::debug(&elf);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let error =
            typed_function_pointer_resolutions(image, &analysis, &types, &mut keep, &mut || Ok(()))
                .err()
                .unwrap();
        assert_eq!(error.code, ErrorCode::Incompatible);
        assert!(error.message.contains("not requested"));
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_resolver_uses_prepared_metadata_when_debug_sections_cannot_be_loaded_again() {
    let mut elf = compiled(PROGRAM);
    let (_, mut types) = typed_debug(&elf);
    let info = oer_analysis_elf::Elf::parse(&elf)
        .unwrap()
        .section_by_name(".debug_info")
        .unwrap()
        .index;
    let shoff = u32::from_le_bytes(elf[32..36].try_into().unwrap()) as usize;
    let flags = shoff + info * 40 + 8;
    // Marking the already prepared debug section compressed makes any
    // subsequent DebugSections::load fail while preserving machine code.
    elf[flags..flags + 4].copy_from_slice(&object::elf::SHF_COMPRESSED.to_le_bytes());
    // Corrupt only the retained source's section flags, in the same buffer.
    // All prepared DIE and storage identities still refer to this image.
    std::rc::Rc::get_mut(types.source.as_mut().unwrap()).unwrap()[flags..flags + 4]
        .copy_from_slice(&object::elf::SHF_COMPRESSED.to_le_bytes());
    with_facts(&elf, |image| {
        assert!(oer_analysis_elf::dwarf::DebugSections::load(image.file()).is_err());
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let resolutions = typed_function_pointer_resolutions(
            image,
            &analysis,
            &types,
            &mut keep,
            &mut || Ok(()),
        )?;
        let site = analysis.functions[&named(&analysis, "_start")]
            .transfers
            .iter()
            .find(|transfer| transfer.target.is_none())
            .unwrap()
            .site;
        let resolution = resolutions.get(site).unwrap();
        assert!(resolution.targets.contains(&named(&analysis, "target")));
        assert!(!resolution.targets.contains(&named(&analysis, "different")));
        Ok(())
    })
    .unwrap();
}

fn global_program() -> String {
    PROGRAM
        .replace("pub static CALLBACKS:", "pub static mut CALLBACKS:")
        .replace("    &CALLBACKS", "    core::ptr::addr_of!(CALLBACKS)")
        .replace(
            "    black_box(&OTHER);",
            "    black_box(global as extern \"C\" fn(u32) -> u32); black_box(&OTHER);",
        )
        + r#"
#[unsafe(no_mangle)]
pub extern "C" fn global(value: u32) -> u32 {
    (unsafe { core::ptr::read_volatile(core::ptr::addr_of!(CALLBACKS.run)) })(value)
}
"#
}

fn check_both(elf: &[u8], expected: bool, reason: Option<UnknownUse>) {
    let (_, types) = typed_debug(elf);
    with_facts(elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(image, &[], &[], &UnknownCallBehavior, &mut keep, &mut || Ok(()))?;
        let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        let different = named(&analysis, "different");
        let uses = audit.uses().find(|(function, _)| *function == different).unwrap().1;
        assert!(!uses.is_empty());
        if let Some(reason) = reason {
            assert!(uses.iter().any(|use_| matches!(use_.evidence, StorageEvidence::Typed(_))), "the regression must retain a known use alongside the unknown: {uses:?}");
            assert!(uses.iter().any(|use_| matches!(use_.evidence, StorageEvidence::Unknown(found) if found == reason)), "{uses:?}");
        }
        let global = crate::function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        let typed = crate::typed_function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        let site = |name| analysis.functions[&named(&analysis, name)].transfers.iter().find(|t| t.target.is_none()).unwrap().site;
        let global = global.get(site("global")).expect("global field");
        let typed = typed.get(site("_start")).expect("typed field");
        assert_eq!(global.targets, typed.targets, "the same field and audit must give the same candidates");
        assert_eq!(global.targets.contains(&different), expected);
        Ok(())
    }).unwrap();
}

#[test]
fn both_resolvers_use_all_storage_types_including_a_signature_cast() {
    let source = global_program();
    check_both(&compiled(&source), false, None);
    // Keep both incompatible and compatible storage uses of the same function.
    let source = source.replace("pub static OTHER: fn(u16) -> u16 = different;", "pub static OTHER: fn(u16) -> u16 = different;\n#[unsafe(no_mangle)]\npub static CAST: fn(u32) -> u32 = unsafe { core::mem::transmute(different as fn(u16) -> u16) };").replace("black_box(&OTHER);", "black_box(&OTHER); black_box(&CAST);");
    check_both(&compiled(&source), true, None);
}

#[test]
fn both_resolvers_keep_one_unknown_write_among_known_uses() {
    let source = global_program().replace("pub static OTHER:", "static mut ERASED: usize = 0;\npub static OTHER:").replace("black_box(&OTHER);", "black_box(&OTHER); unsafe { core::ptr::write_volatile(core::ptr::addr_of_mut!(ERASED), different as *const () as usize); }");
    check_both(&compiled(&source), true, Some(UnknownUse::UnknownWrite));
}

#[test]
fn both_resolvers_keep_an_addend_among_exact_typed_uses() {
    let source = global_program().replace("pub static OTHER: fn(u16) -> u16 = different;", "pub static OTHER: fn(u16) -> u16 = different;\n#[unsafe(no_mangle)]\npub static EXTRA: fn(u16) -> u16 = different;").replace("black_box(&OTHER);", "black_box(&OTHER); black_box(&EXTRA);");
    let mut elf = compiled(&source);
    let (rela_at, word_at, target) = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        let extra = file.symbols().find(|s| s.name == "EXTRA").unwrap();
        let section = file.section(extra.section.unwrap()).unwrap();
        let rela = file
            .sections()
            .find(|s| {
                s.kind == oer_analysis_elf::SectionKind::Rela && s.relocates == Some(section.index)
            })
            .unwrap();
        let index = rela
            .data
            .chunks_exact(12)
            .position(|entry| {
                u32::from_le_bytes(entry[..4].try_into().unwrap()) == extra.address as u32
            })
            .unwrap();
        let word_at = section.file_offset as usize + (extra.address - section.address) as usize;
        (
            rela.file_offset as usize + index * 12 + 8,
            word_at,
            u32::from_le_bytes(elf[word_at..word_at + 4].try_into().unwrap()),
        )
    };
    elf[rela_at..rela_at + 4].copy_from_slice(&2_i32.to_le_bytes());
    elf[word_at..word_at + 4].copy_from_slice(&(target + 2).to_le_bytes());
    check_both(&elf, true, Some(UnknownUse::NonExactAddress));
}

#[test]
fn type_facts_from_another_elf_are_rejected_even_with_matching_die_offsets() {
    let elf = compiled(&global_program());
    let other = compiled(&global_program().replace("marker: 7", "marker: 8"));
    let (_, types) = typed_debug(&other);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let error = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))
            .err()
            .expect("different ELF");
        assert_eq!(error.code, ErrorCode::Incompatible);
        Ok(())
    })
    .unwrap();
}

#[test]
fn no_address_evidence_and_empty_sets_never_prove_unreachability() {
    let elf = compiled(&global_program());
    let (_, types) = typed_debug(&elf);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let mut audit =
            audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        // Remove the evidence, retaining the address-taken universe.
        for uses in audit.uses.values_mut() {
            uses.clear();
        }
        let global = crate::function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        let typed = crate::typed_function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        assert!(!global.is_empty() && !typed.is_empty());
        assert_eq!(
            global.iter().next().unwrap().1.targets,
            typed.iter().next().unwrap().1.targets
        );
        assert!(
            global
                .iter()
                .all(|(_, r)| r.targets.contains(&named(&analysis, "different")))
        );
        audit.uses.clear();
        let global = crate::function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        assert!(!global.is_empty());
        assert!(
            global
                .iter()
                .all(|(_, r)| r.targets.is_empty() && !r.resolves())
        );
        let bound = analysis
            .bound_with(named(&analysis, "_start"), &global)
            .unwrap();
        assert!(!bound.unresolved.is_empty());
        Ok(())
    })
    .unwrap();
}

#[test]
fn missing_dwarf_supplies_no_exclusion_or_unreachable_call_proof() {
    let mut elf = compiled(&global_program());
    // Hide all debug sections from the reader, keeping the code/relocations.
    let names = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        file.sections()
            .filter(|s| s.name.starts_with(".debug_"))
            .map(|s| s.name.as_bytes().to_vec())
            .collect::<Vec<_>>()
    };
    for name in names {
        if let Some(at) = elf.windows(name.len()).position(|bytes| bytes == name) {
            elf[at..at + 7].copy_from_slice(b".erased");
        }
    }
    let (_, types) = typed_debug(&elf);
    assert!(types.globals.is_empty());
    with_facts(&elf, |image| {
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
        assert!(
            audit
                .uses()
                .flat_map(|(_, uses)| uses)
                .all(|u| matches!(u.evidence, StorageEvidence::Unknown(_)))
        );
        assert!(crate::function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?.is_empty());
        assert!(
            crate::typed_function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?
                .is_empty()
        );
        assert!(
            !analysis
                .bound(named(&analysis, "_start"))
                .unwrap()
                .unresolved
                .is_empty()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn merged_symbols_share_every_known_and_unknown_address_use() {
    let source = global_program()
        .replace("#[unsafe(no_mangle)]\npub static OTHER:", "#[unsafe(no_mangle)]\npub static ALIAS: fn(i32) -> i32 = alias;\n#[unsafe(no_mangle)]\npub static OTHER:")
        .replace("black_box(&OTHER);", "black_box(&OTHER); black_box(&ALIAS); black_box(alias as *const () as usize);")
        + r#"
#[unsafe(no_mangle)]
#[inline(never)]
pub fn alias(value: i32) -> i32 { black_box(value.wrapping_add(1)) }
"#;
    let source = source.replace(
        "#[inline(never)]\nfn target",
        "#[unsafe(no_mangle)]\n#[inline(never)]\npub fn target",
    );
    let elf = compiled(&source);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let target = named(&analysis, "target");
        assert_eq!(
            target,
            named(&analysis, "alias"),
            "fixture must merge bodies of different declaration types"
        );
        let (_, types) = typed_debug(&elf);
        let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        let uses = audit
            .uses()
            .find(|(function, _)| *function == target)
            .unwrap()
            .1;
        assert!(
            uses.iter()
                .any(|u| matches!(u.evidence, StorageEvidence::Typed(_)))
        );
        assert!(
            uses.iter()
                .any(|u| matches!(u.evidence, StorageEvidence::Unknown(_)))
        );
        for resolutions in [
            crate::function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?,
            crate::typed_function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?,
        ] {
            assert!(!resolutions.is_empty());
            assert!(resolutions.iter().all(|(_, r)| r.targets.contains(&target)));
        }
        Ok(())
    })
    .unwrap();
}

#[test]
fn the_common_audit_obeys_work_and_memory_budgets() {
    let elf = compiled(PROGRAM);
    let (_, types) = typed_debug(&elf);
    with_facts(&elf, |image| {
        let analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut ledger(),
            &mut || Ok(()),
        )?;
        let error = audit_function_addresses(image, &analysis, &types, &mut ledger(), &mut || {
            Err(Error::new(ErrorCode::Cancelled, "stop"))
        })
        .err()
        .expect("work budget");
        assert_eq!(error.code, ErrorCode::Cancelled);
        let memory = WorkingMemory::new(1)?;
        let error = audit_function_addresses(
            image,
            &analysis,
            &types,
            &mut Ledger::new(&memory, &mut || Ok(()))?,
            &mut || Ok(()),
        )
        .err()
        .expect("memory budget");
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_missing_parameter_type_does_not_exclude_a_candidate() {
    let mut elf = compiled(&global_program().replace("u16", "u32"));
    let (other_address, call_address) = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        let address = |name| file.symbols().find(|s| s.name == name).unwrap().address as u32;
        (address("OTHER"), address("CALLBACKS") + 4)
    };
    // Real DWARF4 input: both storage types have one formal parameter, but
    // the first parameter has no DW_AT_type. Return types are the same.
    let abbrev = vec![
        1, 0x11, 1, 0, 0, // compile_unit
        2, 0x24, 0, 0x0b, 0x0b, 0, 0, // base_type, byte_size/data1
        3, 0x15, 1, 0x49, 0x13, 0, 0, // subroutine_type, type/ref4
        4, 0x05, 0, 0, 0, // formal_parameter with no type
        5, 0x05, 0, 0x49, 0x13, 0, 0, // formal_parameter, type/ref4
        6, 0x0f, 0, 0x49, 0x13, 0, 0, // pointer_type, type/ref4
        7, 0x34, 0, 0x49, 0x13, 0x02, 0x18, 0, 0, // variable, type/ref4, location/exprloc
        0,
    ];
    let mut info = vec![0; 4];
    info.extend_from_slice(&4_u16.to_le_bytes());
    info.extend_from_slice(&0_u32.to_le_bytes());
    info.extend_from_slice(&[4, 1]); // address size, CU
    let word = info.len() as u32;
    info.extend_from_slice(&[2, 4]);
    let stored = info.len();
    info.push(3);
    info.extend_from_slice(&word.to_le_bytes());
    info.extend_from_slice(&[4, 0]); // one untyped parameter, end subroutine
    let called = info.len();
    info.push(3);
    info.extend_from_slice(&word.to_le_bytes());
    info.push(5);
    info.extend_from_slice(&word.to_le_bytes());
    info.push(0);
    let stored_pointer = info.len() as u32;
    info.push(6);
    info.extend_from_slice(&(stored as u32).to_le_bytes());
    let called_pointer = info.len() as u32;
    info.push(6);
    info.extend_from_slice(&(called as u32).to_le_bytes());
    for (ty, address) in [
        (stored_pointer, other_address),
        (called_pointer, call_address),
    ] {
        info.push(7);
        info.extend_from_slice(&ty.to_le_bytes());
        info.extend_from_slice(&[5, 0x03]); // expression size, DW_OP_addr
        info.extend_from_slice(&address.to_le_bytes());
    }
    info.push(0);
    let length = (info.len() - 4) as u32;
    info[..4].copy_from_slice(&length.to_le_bytes());
    let indices = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        [
            file.section_by_name(".debug_info").unwrap().index,
            file.section_by_name(".debug_abbrev").unwrap().index,
        ]
    };
    let shoff = u32::from_le_bytes(elf[32..36].try_into().unwrap()) as usize;
    for (index, bytes) in indices.into_iter().zip([info, abbrev]) {
        let at = elf.len() as u32;
        elf[shoff + index * 40 + 16..shoff + index * 40 + 20].copy_from_slice(&at.to_le_bytes());
        elf[shoff + index * 40 + 20..shoff + index * 40 + 24]
            .copy_from_slice(&(bytes.len() as u32).to_le_bytes());
        elf.extend_from_slice(&bytes);
    }
    let (_, types) = typed_debug(&elf);
    assert_eq!(types.signatures[&stored].parameters, [None]);
    assert_eq!(types.signatures[&called].parameters, [Some(word as usize)]);
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let analysis = crate::analyze(image, &[], &[], &UnknownCallBehavior, &mut keep, &mut || Ok(()))?;
        let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        let function = named(&analysis, "different");
        let uses = audit.uses().find(|(entry, _)| *entry == function).unwrap().1;
        assert!(!uses.is_empty());
        assert!(uses.iter().all(|use_| matches!(use_.evidence, StorageEvidence::Typed(_))),
            "the regression must reach storage-type matching without an unknown-use fallback");
        let resolutions = crate::function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        let site = analysis.functions[&named(&analysis, "global")].transfers.iter()
            .find(|t| t.target.is_none()).unwrap().site;
        let targets = &resolutions.get(site).expect("global callback field").targets;
        assert!(targets.contains(&function),
            "one parameter without a type was treated as zero parameters: stored={:?}, called={:?}, targets={targets:?}",
            types.signatures.get(&stored), types.signatures.get(&called));
        Ok(())
    }).unwrap();
}

/// Only the seed instruction has the pointer's DWARF location. All later
/// evidence must come from the machine path, including its register move.
fn transported_base(before: &str, between: &str, expected: bool) {
    let program = PROGRAM.replace("fn _start(", "fn typed_start(").replace(
        "black_box(&OTHER);",
        "black_box(typed_start as extern \"C\" fn(u32, &Callbacks) -> u32); black_box(&OTHER);",
    );
    let program = format!(
        r##"{program}
core::arch::global_asm!(r#"
.option push
.option norvc
.section .text._start,"ax"
.global _start
.type _start,@function
_start:
    addi sp, sp, -16
    sw ra, 12(sp)
    {before}
.global seed
seed:
    addi t0, a1, 0
    {between}
.global callback_load
callback_load:
    lw t1, 4(t0)
    jalr ra, 0(t1)
exit:
    lw ra, 12(sp)
    addi sp, sp, 16
    ret
.size _start, .-_start
.option pop
"#);
"##
    );
    let elf = compiled(&program);
    let (_, mut types) = typed_debug(&elf);
    let pointer = *types
        .types
        .iter()
        .find(|(_, ty)| {
            ty.tag == gimli::DW_TAG_pointer_type
                && ty
                    .target
                    .and_then(|id| types.resolve(id))
                    .is_some_and(|(_, ty)| ty.name.as_deref() == Some("Callbacks"))
        })
        .expect("pointer parameter type")
        .0;
    with_facts(&elf, |image| {
        let seed = image
            .file()
            .symbols()
            .find(|s| s.name == "seed")
            .unwrap()
            .address as u32;
        let mut debug = LocalLocations::default();
        debug.variables.insert(0, (Some(pointer), None));
        debug.expression(
            gimli::Expression(Reader::new(&[0x5b], gimli::RunTimeEndian::Little)), // DW_OP_reg11
            gimli::Encoding {
                address_size: 4,
                format: gimli::Format::Dwarf32,
                version: 4,
            },
            0,
            &[(seed, seed + 4)],
            &mut ledger(),
            &mut || Ok(()),
        )?;
        types.locations = Some(debug);
        let mut keep = ledger();
        let mut analysis = crate::analyze(
            image,
            &[],
            &[],
            &UnknownCallBehavior,
            &mut keep,
            &mut || Ok(()),
        )?;
        let start = named(&analysis, "_start");
        let site = analysis.functions[&start]
            .transfers
            .iter()
            .find(|t| t.target.is_none())
            .unwrap()
            .site;
        let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        let resolved =
            crate::typed_function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?;
        assert_eq!(
            resolved.get(site).is_some(),
            expected,
            "{:#?}",
            analysis.functions[&start].load_bases
        );
        if let Some(resolved) = resolved.get(site) {
            assert!(resolved.targets.contains(&named(&analysis, "target")));
            assert!(!resolved.targets.contains(&named(&analysis, "different")));
            let load = analysis.functions[&start].target_loads[&site][0];
            let proof = audit.load_field(start, load.site, &mut || Ok(()))?.unwrap();
            assert_eq!(
                proof.witness,
                Some(RegisterWitness {
                    site: seed,
                    register: 11
                })
            );
        }
        // Removing transport restores the original hole. The seed's
        // half-open range is not extended by the new rule.
        drop(audit);
        analysis
            .functions
            .get_mut(&start)
            .unwrap()
            .load_bases
            .clear();
        let audit = audit_function_addresses(image, &analysis, &types, &mut keep, &mut || Ok(()))?;
        assert!(
            crate::typed_function_pointer_resolutions(&audit, &mut keep, &mut || Ok(()))?
                .get(site)
                .is_none()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn a_pointer_witness_crosses_a_move_and_a_branch_continuation() {
    transported_base("", "beq a0, zero, exit", true);
}

#[test]
fn a_bypass_of_the_pointer_definition_stops_transport() {
    transported_base("beq a2, zero, callback_load", "", false);
}

#[test]
fn an_unknown_incoming_pointer_stops_transport_at_the_join() {
    transported_base(
        "beq a2, zero, unknown",
        "j callback_load\nunknown: addi t0, a3, 0",
        false,
    );
}

#[test]
fn overwriting_the_pointer_stops_transport() {
    transported_base("", "addi t0, a2, 0", false);
}

#[test]
fn an_unknown_alias_write_stops_transport() {
    transported_base("", "sw zero, 0(a2)", false);
}

#[test]
fn an_unclosed_graph_stops_transport() {
    transported_base(
        "beq a2, zero, unknown",
        "j callback_load\nunknown: jr a3",
        false,
    );
}
