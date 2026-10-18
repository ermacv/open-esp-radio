//! The source-only PHY archive's policy over its compiled symbols, run as a
//! process (`cargo run -p oer-check-phy-archive -- RLIB`). ELF is read natively; LLVM bitcode members
//! use the llvm-nm shipped with the active Rust toolchain.
#![forbid(unsafe_code)]

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;
use oer_process as process;
use oer_process::Checkout;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[cfg(test)]
mod tests;

#[derive(Default)]
struct Symbols {
    defined: BTreeSet<String>,
    undefined: BTreeSet<String>,
}

/// Audit the PHY archive at `path`, whose source-only code may call into
/// the crates `sources` (by crate name); fails naming the first symbol
/// outside the policy.
pub fn audit_phy(ctx: &Checkout, path: &Path, sources: &[String]) -> Result<()> {
    let bytes = std::fs::read(path)
        .map_err(|error| format!("cannot read PHY archive {}: {error}", path.display()))?;
    let symbols = archive_symbols(ctx, &bytes)?;
    check_symbols(&symbols, sources)?;
    println!("source-only PHY archive symbols passed: {}", path.display());
    Ok(())
}

fn archive_symbols(ctx: &Checkout, bytes: &[u8]) -> Result<Symbols> {
    let members = oer_elf::members(bytes)
        .map_err(|error| format!("PHY archive must contain its own members: {error}"))?;
    let mut symbols = Symbols::default();
    let mut code_members = 0;
    let mut llvm_nm = None;
    let temporary = tempfile::tempdir()?;
    for (index, (name, data)) in members.iter().enumerate() {
        let (name, data) = (name.as_str(), *data);
        let metadata = matches!(name, "lib.rmeta" | "lib.rmeta-link");
        if data.starts_with(b"\x7fELF") {
            elf_symbols(data, &mut symbols, !metadata)
                .map_err(|error| format!("invalid ELF member {name}: {error}"))?;
        } else if is_bitcode(data) {
            let tool = match &llvm_nm {
                Some(tool) => tool,
                None => llvm_nm.insert(matched_llvm_nm()?),
            };
            // Never interpret archive names as extraction paths.
            let input = temporary.path().join(format!("member-{index}.bc"));
            std::fs::write(&input, data)?;
            bitcode_symbols(ctx, tool, &input, &mut symbols)
                .map_err(|error| format!("cannot inspect bitcode member {name}: {error}"))?;
        } else {
            return Err(format!(
                "unrecognized PHY archive member {name}; no symbols were assumed absent"
            )
            .into());
        }
        if !metadata {
            code_members += 1;
        }
    }
    if code_members == 0 {
        return Err("PHY archive contains no compiled code members".into());
    }
    Ok(symbols)
}

fn elf_symbols(bytes: &[u8], symbols: &mut Symbols, require_table: bool) -> Result<()> {
    let file = oer_elf::Elf::parse(bytes)?;
    if !file.is_relocatable() {
        return Err("expected a relocatable ELF archive member".into());
    }
    if require_table && !file.has_symbol_table() {
        return Err(
            "compiled ELF member has no symbol table; external references cannot be audited".into(),
        );
    }
    for symbol in file.symbols() {
        if matches!(
            symbol.kind,
            oer_elf::SymbolKind::File | oer_elf::SymbolKind::Section
        ) || symbol.name.is_empty()
        {
            continue;
        }
        if symbol.defined {
            symbols.defined.insert(symbol.name.to_owned());
        } else {
            symbols.undefined.insert(symbol.name.to_owned());
        }
    }
    Ok(())
}

fn is_bitcode(bytes: &[u8]) -> bool {
    // LLVM raw bitcode and the documented bitcode wrapper magic.
    bytes.starts_with(b"BC\xc0\xde") || bytes.starts_with(&[0xde, 0xc0, 0x17, 0x0b])
}

/// The `llvm-nm` of the active Rust toolchain: an LLVM on `PATH` need not
/// read the compiler's bitcode.
fn matched_llvm_nm() -> Result<PathBuf> {
    Ok(oer_toolchain::program(oer_toolchain::Tool::LlvmNm)?.into())
}

fn bitcode_symbols(ctx: &Checkout, tool: &Path, input: &Path, symbols: &mut Symbols) -> Result<()> {
    for (mode, destination) in [
        ("--defined-only", &mut symbols.defined),
        ("--undefined-only", &mut symbols.undefined),
    ] {
        let output = process::capture(
            ctx.command(tool)
                .args([mode, "--just-symbol-name", "--no-demangle"])
                .arg(input),
        )?;
        // llvm-nm can emit a diagnostic even with a successful status. Do not
        // accept incomplete observations or silently discard unknown warnings.
        if !output.stderr.is_empty() {
            return Err(format!(
                "llvm-nm reported: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        parse_nm_names(&String::from_utf8(output.stdout)?, destination)?;
    }
    Ok(())
}

fn parse_nm_names(text: &str, destination: &mut BTreeSet<String>) -> Result<()> {
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        if line.chars().any(char::is_whitespace) || line.ends_with(':') {
            return Err(format!("malformed llvm-nm symbol record: {line:?}").into());
        }
        destination.insert(line.to_owned());
    }
    Ok(())
}

fn check_symbols(symbols: &Symbols, sources: &[String]) -> Result<()> {
    for raw in symbols.defined.union(&symbols.undefined) {
        if forbidden_radio_symbol(raw) {
            return Err(
                format!("radio ROM/vendor ABI symbol survived source-only build: {raw}").into(),
            );
        }
    }
    for raw in symbols.undefined.difference(&symbols.defined) {
        let demangled = oer_elf::demangle(raw);
        if !allowed_external(&demangled, sources) {
            return Err(format!(
                "unexpected external symbol in source-only radio rlib: {demangled}"
            )
            .into());
        }
    }
    Ok(())
}

/// Whether `symbol` lies in one of the `sources` crates or `core::fmt`.
fn source_namespace(symbol: &str, sources: &[String]) -> bool {
    symbol.starts_with("core::fmt::")
        || sources.iter().any(|source| {
            symbol
                .strip_prefix(source.as_str())
                .is_some_and(|rest| rest.starts_with("::"))
        })
}

fn allowed_external(symbol: &str, sources: &[String]) -> bool {
    if source_namespace(symbol, sources)
        || matches!(
            symbol,
            "__divdi3" | "__udivdi3" | "memcmp" | "memcpy" | "memmove" | "memset"
        )
    {
        return true;
    }
    if let Some(subject) = symbol.strip_prefix('<') {
        let subject = subject
            .split_once(">::")
            .map_or(subject, |(subject, _)| subject);
        let trait_name = subject
            .rsplit_once(" as ")
            .map_or(subject, |(_, name)| name);
        if source_namespace(subject, sources) || source_namespace(trait_name, sources) {
            return true;
        }
    }
    symbol.starts_with("core::")
        && symbol
            .rsplit("::")
            .next()
            .is_some_and(|leaf| leaf.starts_with("panic") || leaf.starts_with("len_mismatch_fail"))
}

fn forbidden_radio_symbol(symbol: &str) -> bool {
    matches!(
        symbol,
        "phy_wifi_get_tx_gain" | "register_chipv7_phy" | "g_phyFuns" | "phy_param"
    ) || ["esp_wifi_", "pp_", "net80211_"]
        .iter()
        .any(|prefix| symbol.starts_with(prefix))
}
