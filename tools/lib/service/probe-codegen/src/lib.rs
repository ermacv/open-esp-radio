//! Single declaration grammar for probe compilation, linker roots and catalogs.
//!
//! This host-only tool owns verification ABI publication, never production
//! behavior. Generated files live in Cargo's output directory. The catalog
//! describes declared Rust boundary types; it does not infer private layouts.

use proc_macro2::TokenStream;
use quote::{ToTokens, quote};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};
use syn::{
    Attribute, Block, Expr, FnArg, Item, Pat, Signature, Token, Visibility,
    parse::{Parse, ParseStream},
};

/// Catalog version owned by this compiler and its request-preparation consumer.
pub const SCHEMA: u32 = 1;
/// ELF section containing precisely one UTF-8 JSON catalog.
pub const SECTION: &str = ".blobray.probes";
/// Host build/inspection errors, including source location for parse failures.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Parsed entry declaration shared by macro expansion and source collection.
pub struct Declaration {
    attributes: Vec<Attribute>,
    visibility: Visibility,
    signature: Signature,
    body: Block,
    adapter: String,
}

impl Parse for Declaration {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let attributes = input.call(Attribute::parse_outer)?;
        validate_attributes(&attributes)?;
        let visibility = input.parse()?;
        let mut signature: Signature = input.parse()?;
        if signature.asyncness.is_some()
            || signature.constness.is_some()
            || !signature.generics.params.is_empty()
            || signature.generics.where_clause.is_some()
            || signature.variadic.is_some()
            || signature.abi.is_some()
        {
            return Err(syn::Error::new_spanned(
                &signature,
                "probe requires a synchronous, non-generic Rust signature; C ABI is generated",
            ));
        }
        for argument in &signature.inputs {
            match argument {
                FnArg::Typed(argument) if matches!(argument.pat.as_ref(), Pat::Ident(_)) => {}
                _ => {
                    return Err(syn::Error::new_spanned(
                        argument,
                        "probe arguments require named parameters",
                    ));
                }
            }
        }
        let (body, adapter) = if input.peek(Token![=>]) {
            input.parse::<Token![=>]>()?;
            let expression: Expr = input.parse()?;
            input.parse::<Token![;]>()?;
            (syn::parse_quote!({ #expression }), "call")
        } else {
            (input.parse()?, "body")
        };
        signature.abi = Some(syn::parse_quote!(extern "C"));
        Ok(Self {
            attributes,
            visibility,
            signature,
            body,
            adapter: adapter.into(),
        })
    }
}

fn validate_attributes(attributes: &[Attribute]) -> syn::Result<()> {
    for attribute in attributes {
        let path = attribute.path();
        if !(path.is_ident("doc")
            || path.is_ident("allow")
            || path.is_ident("expect")
            || (path.is_ident("unsafe")
                && attribute.meta.to_token_stream().to_string() == "unsafe (naked)"))
        {
            return Err(syn::Error::new_spanned(
                attribute,
                "unsupported probe attribute; export and inlining are generated, conditional declarations are not supported",
            ));
        }
    }
    Ok(())
}

impl Declaration {
    /// Generate the exported ABI boundary while leaving its behavior unchanged.
    pub fn expand(&self) -> TokenStream {
        let Self {
            attributes,
            visibility,
            signature,
            body,
            ..
        } = self;
        let inline = if attributes.iter().any(|a| a.path().is_ident("unsafe")) {
            quote!()
        } else {
            quote!(#[inline(never)])
        };
        quote! { #(#attributes)* #[unsafe(no_mangle)] #inline #visibility #signature #body }
    }

    fn entry(&self) -> Entry {
        Entry {
            symbol: self.signature.ident.to_string(),
            signature: self.signature.to_token_stream().to_string(),
            adapter: self.adapter.clone(),
            arguments: self
                .signature
                .inputs
                .iter()
                .map(|argument| {
                    let FnArg::Typed(argument) = argument else {
                        unreachable!()
                    };
                    Argument {
                        name: argument.pat.to_token_stream().to_string(),
                        rust_type: argument.ty.to_token_stream().to_string(),
                    }
                })
                .collect(),
        }
    }
}

/// One named parameter. Rust type spelling is descriptive, not a layout proof.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Argument {
    /// Parameter name in the declaration.
    pub name: String,
    /// Declared boundary type, including explicit references and array lengths.
    pub rust_type: String,
}

/// An exported probe, scoped to a single image.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Exact unmangled linker symbol.
    pub symbol: String,
    /// Generated C ABI signature, including return type.
    pub signature: String,
    /// `call` for expression adapters, `body` for explicit adapters.
    pub adapter: String,
    /// Arguments in declaration order.
    pub arguments: Vec<Argument>,
}

/// Catalog embedded in the same ELF as the code it describes.
#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    /// Exact supported schema version.
    pub schema: u32,
    /// Owning ELF package, not a global symbol namespace.
    pub image: String,
    /// Entries sorted by symbol for reproducible generation.
    pub entries: Vec<Entry>,
}

/// Collect top-level and nested module declarations. Conditional modules and
/// externally generated probe declarations are unsupported and fail explicitly.
/// Returns every input file so callers can emit Cargo change tracking.
pub fn collect(root: &Path, image: &str) -> Result<(Catalog, Vec<PathBuf>)> {
    let mut entries = Vec::new();
    let mut files = Vec::new();
    collect_file(
        root,
        root.parent().ok_or("source has no parent")?,
        &mut entries,
        &mut files,
    )?;
    entries.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    let catalog = Catalog {
        schema: SCHEMA,
        image: image.into(),
        entries,
    };
    validate_catalog(&catalog)?;
    Ok((catalog, files))
}

fn collect_file(
    path: &Path,
    directory: &Path,
    entries: &mut Vec<Entry>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    let canonical = path.canonicalize()?;
    if files.contains(&canonical) {
        return Err(format!("repeated module: {}", path.display()).into());
    }
    files.push(canonical);
    let source = std::fs::read_to_string(path)?;
    let parsed = syn::parse_file(&source).map_err(|e| format!("{}: {e}", path.display()))?;
    collect_items(parsed.items, directory, entries, files)
}

fn collect_items(
    items: Vec<Item>,
    directory: &Path,
    entries: &mut Vec<Entry>,
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    for item in items {
        match item {
            Item::Macro(item)
                if item
                    .mac
                    .path
                    .segments
                    .last()
                    .is_some_and(|s| s.ident == "probe") =>
            {
                validate_attributes(&item.attrs)?;
                entries.push(syn::parse2::<Declaration>(item.mac.tokens)?.entry());
            }
            Item::Fn(item)
                if item.sig.ident.to_string().starts_with("open_") && item.sig.abi.is_some() =>
            {
                return Err(format!("unregistered probe {}", item.sig.ident).into());
            }
            Item::Mod(module) => {
                // Host tests have no guest entry points and are not compiled in release.
                if module
                    .attrs
                    .iter()
                    .any(|a| a.meta.to_token_stream().to_string() == "cfg (test)")
                {
                    continue;
                }
                validate_attributes(&module.attrs)?;
                let nested = directory.join(module.ident.to_string());
                if let Some((_, items)) = module.content {
                    collect_items(items, &nested, entries, files)?;
                } else {
                    let flat = directory.join(format!("{}.rs", module.ident));
                    let grouped = nested.join("mod.rs");
                    if flat.exists() && grouped.exists() {
                        return Err(format!("ambiguous module {}", module.ident).into());
                    }
                    collect_file(
                        if flat.exists() { &flat } else { &grouped },
                        &nested,
                        entries,
                        files,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

fn validate_catalog(catalog: &Catalog) -> Result<()> {
    if catalog.schema != SCHEMA || catalog.image.is_empty() || catalog.entries.is_empty() {
        return Err("unsupported or empty probe catalog".into());
    }
    let mut names = BTreeSet::new();
    for entry in &catalog.entries {
        if entry.symbol.is_empty()
            || !entry
                .symbol
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
            || !names.insert(&entry.symbol)
            || !matches!(entry.adapter.as_str(), "call" | "body")
        {
            return Err(format!("invalid or duplicate probe {}", entry.symbol).into());
        }
    }
    Ok(())
}

/// Emit root flags, change tracking and the catalog Rust source in `out_dir`.
/// The ELF main must include `probe_catalog.rs` and explicitly link its library.
/// All paths are supplied at build-script runtime so relocated caches remain valid.
pub fn build(root: &Path, image: &str, out_dir: &Path) -> Result<()> {
    let (catalog, files) = collect(root, image)?;
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    for entry in &catalog.entries {
        println!("cargo:rustc-link-arg=--undefined={}", entry.symbol);
    }
    let json = serde_json::to_vec(&catalog)?;
    let length = json.len();
    let bytes = proc_macro2::Literal::byte_string(&json);
    std::fs::write(
        out_dir.join("probe_catalog.rs"),
        quote! {
            #[used]
            #[unsafe(link_section = ".blobray.probes")]
            static PROBE_CATALOG: [u8; #length] = *#bytes;
        }
        .to_string(),
    )?;
    Ok(())
}

/// Validate the final linked ELF against its embedded catalog. Reject missing,
/// undefined, duplicate or non-executable entries, including malformed catalogs.
/// Code folding may give different symbols the same address; that is valid.
pub fn validate_elf(bytes: &[u8], image: &str) -> Result<Catalog> {
    let file = oer_elf::Elf::executable(bytes)
        .map_err(|_| "probe image must be a linked little-endian RV32 ELF")?;
    let sections: Vec<_> = file.sections().filter(|s| s.name == SECTION).collect();
    if sections.len() != 1 {
        return Err("expected one probe catalog section".into());
    }
    let catalog: Catalog = serde_json::from_slice(sections[0].data)?;
    validate_catalog(&catalog)?;
    if catalog.image != image {
        return Err("probe catalog image mismatch".into());
    }
    for entry in &catalog.entries {
        let symbols: Vec<_> = file
            .symbols()
            .filter(|s| s.name == entry.symbol.as_str() && s.defined)
            .collect();
        if symbols.len() != 1 {
            return Err(format!("missing or ambiguous entry {}", entry.symbol).into());
        }
        let symbol = &symbols[0];
        let section = file.section(symbol.section.ok_or("entry has no section")?)?;
        let executable_mapping = file.segments().any(|segment| {
            segment.executable
                && symbol.address >= segment.address
                && symbol.address < segment.address.saturating_add(segment.data.len() as u64)
        });
        if !executable_mapping
            || symbol.kind != oer_elf::SymbolKind::Text
            || !section.executable
            || section.nobits
            || symbol.address < section.address
            || symbol.address >= section.address.saturating_add(section.size)
        {
            return Err(format!("entry is not executable: {}", entry.symbol).into());
        }
    }
    Ok(catalog)
}

#[cfg(test)]
mod tests;
