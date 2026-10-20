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
            println!(
                "proof\t{function:#010x}\t{}\t{:?}\t{:?}",
                use_.index, use_.barrier, use_.stores
            );
        }
    }
    println!(
        "pcrel-header\tlow_index\thigh_index\tlow_section\tlow_site\tlow_symbol\tlow_symbol_section\tlow_symbol_address\tlow_addend\thigh_site\thigh_symbol\thigh_symbol_section\thigh_symbol_address\thigh_addend\tnormalized_target\tfunctions\tclassification"
    );
    for pair in audit.pc_relative_uses() {
        let low = image.address_uses().get(pair.low).ok_or("bad low index")?;
        let high = pair.high.and_then(|index| image.address_uses().get(index));
        let symbol = |use_: &oer_analysis_riscv_program::facts::addresses::AddressUse<'_>| {
            use_.symbol
                .as_ref()
                .map(|symbol| format!("{}:{:?}", symbol.index, symbol.name))
                .unwrap_or_else(|| "-".into())
        };
        let section = |use_: &oer_analysis_riscv_program::facts::addresses::AddressUse<'_>| {
            use_.symbol
                .as_ref()
                .and_then(|s| s.section)
                .map(|s| s.to_string())
                .unwrap_or_else(|| "-".into())
        };
        let address = |use_: &oer_analysis_riscv_program::facts::addresses::AddressUse<'_>| {
            use_.symbol
                .as_ref()
                .map(|s| format!("{:#010x}", s.address))
                .unwrap_or_else(|| "-".into())
        };
        let classification = if let Some(reason) = pair.unknown {
            format!("{reason:?}")
        } else if pair.functions.is_empty() {
            "NonFunctionTarget".into()
        } else {
            "AuditedFunctionTarget".into()
        };
        println!(
            "pcrel\t{}\t{}\t{}\t{:#010x}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{}\t{:?}\t{}",
            pair.low,
            pair.high
                .map(|i| i.to_string())
                .unwrap_or_else(|| "-".into()),
            low.relocation.section,
            low.relocation.at,
            symbol(low),
            section(low),
            address(low),
            low.relocation.addend,
            high.map(|h| format!("{:#010x}", h.relocation.at))
                .unwrap_or_else(|| "-".into()),
            high.map(symbol).unwrap_or_else(|| "-".into()),
            high.map(section).unwrap_or_else(|| "-".into()),
            high.map(address).unwrap_or_else(|| "-".into()),
            high.map(|h| h.relocation.addend.to_string())
                .unwrap_or_else(|| "-".into()),
            pair.target
                .map(|v| format!("{v:#010x}"))
                .unwrap_or_else(|| "-".into()),
            pair.functions,
            classification
        );
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
