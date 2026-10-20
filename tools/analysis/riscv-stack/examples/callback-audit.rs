use oer_analysis_riscv_model::{Ledger, WorkingMemory};
use oer_analysis_riscv_program::facts::ImageFacts;
use oer_analysis_riscv_stack::*;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

#[derive(Debug, Parser)]
#[command(name = "callback-audit", about = "Audit function pointer evidence")]
struct Cli {
    elf: std::path::PathBuf,
    rom_elf: Option<std::path::PathBuf>,
    summaries_toml: Option<std::path::PathBuf>,
}

fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let cli: Cli = cli::parse();
    let bytes: Rc<[u8]> = std::fs::read(cli.elf)?.into();
    let companion = cli.rom_elf.map(std::fs::read).transpose()?;
    let summaries = cli
        .summaries_toml
        .map(std::fs::read_to_string)
        .transpose()?
        .map(|text| parse_summaries(&text))
        .transpose()?
        .unwrap_or_default();
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
    let mut discovered = BTreeSet::new();
    for facts in analysis.functions.values() {
        for transfer in facts.transfers.iter().filter(|t| t.target.is_none()) {
            println!(
                "unresolved\t{:#010x}\t{}\t{:?}\t{:?}\t{:?}",
                transfer.site,
                facts.function.label(),
                dwarf.inline_chain(transfer.site)?,
                transfer.load,
                facts.target_loads.get(&transfer.site),
            );
            if let Some(loads) = facts.target_loads.get(&transfer.site) {
                for load in loads {
                    let proof =
                        audit.load_field(facts.function.address, load.site, &mut control)?;
                    if let Some(proof) = proof {
                        discovered.insert(proof.field);
                    }
                    println!(
                        "load-field\t{:#010x}\t{:#010x}\t{:?}\t{:?}",
                        transfer.site,
                        load.site,
                        proof,
                        facts.load_bases.get(&load.site),
                    );
                }
            }
        }
    }
    let stored: BTreeSet<_> = audit
        .uses()
        .flat_map(|(_, uses)| uses.iter())
        .filter_map(|use_| {
            if let StorageEvidence::Typed(fields) = &use_.evidence {
                Some(fields)
            } else {
                None
            }
        })
        .flatten()
        .copied()
        .collect();
    for field in discovered {
        for &storage in &stored {
            println!(
                "field-match\t{field:?}\t{storage:?}\t{}",
                audit.compatible_storage(field, storage, &mut control)?
            );
        }
    }
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
#[path = "support/cli.rs"]
mod cli;
use clap::Parser;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn previous_optional_companion_and_summary_forms_parse() {
        Cli::command().debug_assert();
        for arguments in [
            vec!["callback-audit", "image.elf"],
            vec!["callback-audit", "image.elf", "rom.elf"],
            vec!["callback-audit", "image.elf", "rom.elf", "summaries.toml"],
        ] {
            let cli = Cli::try_parse_from(&arguments).expect("accepted example command line");
            assert_eq!(cli.elf, std::path::Path::new("image.elf"));
            assert_eq!(cli.rom_elf.is_some(), arguments.len() >= 3);
            assert_eq!(cli.summaries_toml.is_some(), arguments.len() == 4);
        }
        assert!(Cli::try_parse_from(["callback-audit"]).is_err());
        assert!(
            Cli::try_parse_from([
                "callback-audit",
                "image.elf",
                "rom.elf",
                "summaries.toml",
                "extra",
            ])
            .is_err()
        );
        assert!(Cli::try_parse_from(["callback-audit", "image.elf", "--bad"]).is_err());
    }
}
