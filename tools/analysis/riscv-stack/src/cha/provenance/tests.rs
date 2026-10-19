use super::*;
use crate::dwarf::Reader;
use crate::dwarf::locations::LocalLocations;
use crate::tests::{compiled, ledger, with_facts};

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
        let mut locations = Locations {
            debug: types.locations.as_ref().unwrap(),
            aliases: BTreeMap::new(),
        };
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
    let locations = Locations {
        debug: &prepared,
        aliases: BTreeMap::new(),
    };
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
    let source = PROGRAM.replace(
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
    let (_, types) = typed_debug(&elf);
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
