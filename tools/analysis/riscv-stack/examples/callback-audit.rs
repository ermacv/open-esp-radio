use oer_analysis_riscv_model::{Ledger, WorkingMemory};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::*;
use std::collections::BTreeMap;
use std::rc::Rc;
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .ok_or("usage: callback-audit ELF [ROM_ELF [SUMMARIES_TOML]]")?;
    let bytes: Rc<[u8]> = std::fs::read(path)?.into();
    let companion = args.next().map(std::fs::read).transpose()?;
    let summaries = args
        .next()
        .map(std::fs::read_to_string)
        .transpose()?
        .map(|text| parse_summaries(&text))
        .transpose()?
        .unwrap_or_default();
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let image = ImageFacts::new(&bytes, &[], &memory, &mut control)?;
    let mut keep = Ledger::new(&memory, &mut control)?;
    let (dwarf, types) = read_debug(
        &bytes,
        DebugMetadata::TypedLocations,
        &mut keep,
        &mut control,
    )?;
    let companion = companion
        .as_deref()
        .map(|bytes| ImageFacts::new(bytes, &[], &memory, &mut control))
        .transpose()?;
    let companions: Vec<_> = companion.iter().collect();
    let analysis = analyze(
        &image,
        &companions,
        &summaries,
        &dwarf,
        &mut keep,
        &mut control,
    )?;
    let audit = audit_function_addresses(&image, &analysis, &types, &mut keep, &mut control)?;
    let global = function_pointer_resolutions(&audit, &mut keep, &mut control)?;
    let typed = typed_function_pointer_resolutions(&audit, &mut keep, &mut control)?;
    let mut unknown = BTreeMap::<UnknownUse, usize>::new();
    let mut typed_uses = 0;
    let mut no_uses = 0;
    for (function, uses) in audit.uses() {
        if uses.is_empty() {
            no_uses += 1;
        }
        for use_ in uses {
            let original = image
                .address_uses()
                .get(use_.index)
                .ok_or("audit index outside its source image")?;
            let result = match &use_.evidence {
                StorageEvidence::Typed(fields) => {
                    typed_uses += 1;
                    format!("typed:{fields:?}")
                }
                StorageEvidence::Unknown(reason) => {
                    *unknown.entry(*reason).or_default() += 1;
                    format!("{reason:?}")
                }
            };
            println!(
                "use\t{function:#010x}\t{}\t{:#010x}\t{:?}\t{}\t{result}",
                use_.index, original.relocation.at, original.role, original.relocation.addend
            );
        }
    }
    println!("typed-uses\t{typed_uses}");
    println!("no-uses\t{no_uses}");
    for (reason, count) in unknown {
        println!(
            "unknown-class\t{reason:?}\t{count}\t{}",
            reason.missing_fact()
        );
    }
    for (resolver, resolutions) in [("global", global), ("typed", typed)] {
        for (site, resolution) in resolutions.iter() {
            println!(
                "site\t{resolver}\t{site:#010x}\t{}",
                resolution.targets.len()
            );
            for target in &resolution.targets {
                println!("candidate\t{resolver}\t{site:#010x}\t{target:#010x}");
            }
        }
    }
    Ok(())
}
