use super::*;
use crate::tests::{compiled, debug, ledger, with_facts};

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
    let (_, types) = debug(&elf);
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
        let locations = Locations::read(image, &mut keep, &mut || Ok(()))?;
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
    let (_, types) = debug(&elf);
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
    let (_, types) = debug(&elf);
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
    let (_, types) = debug(&elf);
    let ty = types
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("Callbacks"))
        .unwrap()
        .0;
    let mut locations = Locations::default();
    locations.variables.insert(0, (Some(*ty), None));
    // marker (4 bytes) has no location. run lives in sp+36, with its
    // original aggregate offset still four bytes.
    let expression = gimli::Expression(Reader::new(
        &[0x93, 4, 0x72, 36, 0x93, 4],
        gimli::RunTimeEndian::Little,
    ));
    locations
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
    with_facts(&elf, |image| {
        let mut keep = ledger();
        let error = Locations::read(image, &mut keep, &mut || {
            Err(Error::new(ErrorCode::Cancelled, "cancelled"))
        })
        .err()
        .expect("cancelled while walking DWARF");
        assert_eq!(error.code, ErrorCode::Cancelled);
        Ok(())
    })
    .unwrap();
}
