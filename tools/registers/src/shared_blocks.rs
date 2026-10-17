//! Register blocks shared between a register library and chip PACs.
//!
//! svd2rust renders each peripheral as a `pub type X = crate::Periph<x::RegisterBlock,
//! ADDRESS>` alias, its `Debug` impl and a `pub mod x` of register types. A
//! library keeps the modules and makes each alias the bare register block, so
//! the library carries no address and its transactions take `&RegisterBlock`.
//! A chip keeps its addressed aliases and re-exports the library's modules in
//! place of its own copies, so its paths and the generated `Periph` stay
//! unchanged.

use std::collections::{BTreeMap, BTreeSet};

use oer_register_model::SharedPeripheral;
use quote::{ToTokens, format_ident, quote};

use crate::Result;

/// svd2rust's module name of a peripheral.
fn module_name(peripheral: &str) -> String {
    peripheral.to_ascii_lowercase()
}

fn parse(raw: &str) -> Result<syn::File> {
    Ok(syn::parse_file(raw).map_err(|error| format!("svd2rust output does not parse: {error}"))?)
}

/// The module of a `crate::Periph<module::RegisterBlock, ADDRESS>` alias.
fn periph_module(alias: &syn::ItemType) -> Option<syn::Ident> {
    let syn::Type::Path(path) = alias.ty.as_ref() else {
        return None;
    };
    let last = path.path.segments.last()?;
    if last.ident != "Periph" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &last.arguments else {
        return None;
    };
    let Some(syn::GenericArgument::Type(syn::Type::Path(block))) = arguments.args.first() else {
        return None;
    };
    let segments: Vec<_> = block.path.segments.iter().collect();
    (segments.len() == 2 && segments[1].ident == "RegisterBlock").then(|| segments[0].ident.clone())
}

/// Replace the chip's copies of library register modules with re-exports.
pub(crate) fn reexport_library_blocks(
    raw: &str,
    shared: &BTreeMap<String, SharedPeripheral>,
) -> Result<String> {
    if shared.is_empty() {
        return Ok(raw.to_owned());
    }
    let mut file = parse(raw)?;
    let mut pending: BTreeMap<String, &SharedPeripheral> = shared
        .iter()
        .map(|(peripheral, library)| (module_name(peripheral), library))
        .collect();
    for item in &mut file.items {
        let syn::Item::Mod(module) = item else {
            continue;
        };
        let Some(library) = pending.remove(&module.ident.to_string()) else {
            continue;
        };
        let crate_name = format_ident!("{}", library.crate_name);
        let ident = &module.ident;
        *item = syn::parse_quote! {
            #[doc = "Register block shared with other chips; its layout comes from a register library."]
            pub use #crate_name::#ident;
        };
    }
    if let Some(module) = pending.keys().next() {
        return Err(format!("svd2rust rendered no module {module} for a shared layout").into());
    }
    Ok(file.into_token_stream().to_string())
}

/// Reduce svd2rust output of a library's placeholder device to its register
/// modules, with every peripheral alias naming the bare register block.
pub(crate) fn library_blocks(raw: &str, peripherals: &[&str]) -> Result<String> {
    let file = parse(raw)?;
    let modules: BTreeSet<String> = peripherals.iter().map(|p| module_name(p)).collect();
    let mut aliases = BTreeSet::new();
    let mut items = Vec::new();
    for item in file.items {
        match item {
            syn::Item::Mod(ref module) if module.ident == "generic" => items.push(item),
            syn::Item::Mod(ref module) if modules.contains(&module.ident.to_string()) => {
                items.push(item);
            }
            syn::Item::Use(_) => items.push(item),
            syn::Item::Type(mut alias) => {
                let module = periph_module(&alias).ok_or_else(|| {
                    format!("unexpected type alias {} in svd2rust output", alias.ident)
                })?;
                alias.ty = Box::new(syn::parse_quote!(#module::RegisterBlock));
                aliases.insert(alias.ident.to_string());
                items.push(syn::Item::Type(alias));
            }
            syn::Item::Impl(ref implementation) => {
                let target = implementation.self_ty.to_token_stream().to_string();
                if aliases.contains(&target) {
                    items.push(item);
                } else if target != "Peripherals" {
                    return Err(format!("unexpected impl for {target} in svd2rust output").into());
                }
            }
            syn::Item::Struct(ref structure) if structure.ident == "Peripherals" => {}
            syn::Item::Static(ref value) if value.ident == "DEVICE_PERIPHERALS" => {}
            other => {
                return Err(format!(
                    "unexpected item in svd2rust output: {}",
                    other.to_token_stream()
                )
                .into());
            }
        }
    }
    let attrs = file.attrs;
    Ok(quote! {
        #(#attrs)*
        #(#items)*
    }
    .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const RAW: &str = r#"
        #![no_std]
        use generic::*;
        pub mod generic { pub struct Periph<RB, const A: usize>(RB); }
        #[doc = "Radio"]
        pub type Radio = crate::Periph<radio::RegisterBlock, 0x1000>;
        impl core::fmt::Debug for Radio {
            fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result { f.write_str("Radio") }
        }
        pub mod radio { pub struct RegisterBlock; }
        static mut DEVICE_PERIPHERALS: bool = false;
        pub struct Peripherals { pub radio: Radio }
        impl Peripherals { pub unsafe fn steal() -> Self { loop {} } }
    "#;

    #[test]
    fn a_library_keeps_blocks_without_addresses() {
        let library = library_blocks(RAW, &["RADIO"]).unwrap();
        assert!(
            library.contains("pub type Radio = radio :: RegisterBlock"),
            "{library}"
        );
        assert!(library.contains("pub mod radio"));
        assert!(!library.contains("0x1000"));
        assert!(!library.contains("Peripherals"));
        assert!(!library.contains("DEVICE_PERIPHERALS"));
    }

    #[test]
    fn a_chip_reexports_library_modules_and_keeps_its_address() {
        let shared = BTreeMap::from([(
            "RADIO".to_owned(),
            SharedPeripheral {
                library: "fixture".into(),
                crate_name: "fixture_pac_raw".into(),
            },
        )]);
        let chip = reexport_library_blocks(RAW, &shared).unwrap();
        assert!(chip.contains("pub use fixture_pac_raw :: radio"), "{chip}");
        assert!(!chip.contains("pub mod radio"));
        assert!(chip.contains("0x1000"));
        let missing = BTreeMap::from([("OTHER".to_owned(), shared["RADIO"].clone())]);
        assert!(reexport_library_blocks(RAW, &missing).is_err());
    }

    #[test]
    fn unexpected_library_items_fail_closed() {
        let raw = format!("{RAW}\npub fn stray() {{}}\n");
        assert!(library_blocks(&raw, &["RADIO"]).is_err());
    }
}
