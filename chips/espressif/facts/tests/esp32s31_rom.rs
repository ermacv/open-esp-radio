//! The ROM exports are exactly the foreign functions declared by our S31
//! code: the HAL's two ROM printf slot installers. Generated PACs and vendor
//! research facts are not ABI consumers, and a declaration of a symbol our
//! own `global_asm!` defines (a probe's thunk) names no ROM function.

use std::{collections::BTreeSet, path::Path};

use syn::visit::Visit;

#[derive(Default)]
struct Declarations {
    /// The C foreign functions our code declares.
    foreign: BTreeSet<String>,
    /// The symbols our `global_asm!` blocks make global.
    assembled: BTreeSet<String>,
}

impl<'ast> Visit<'ast> for Declarations {
    fn visit_item_foreign_mod(&mut self, block: &'ast syn::ItemForeignMod) {
        if block
            .abi
            .name
            .as_ref()
            .is_some_and(|abi| abi.value() != "C")
        {
            return;
        }
        for item in &block.items {
            if let syn::ForeignItem::Fn(function) = item {
                let link_name = function
                    .attrs
                    .iter()
                    .find(|attr| attr.path().is_ident("link_name"));
                let name = match link_name.map(|attr| &attr.meta) {
                    Some(syn::Meta::NameValue(value)) => match &value.value {
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(name),
                            ..
                        }) => name.value(),
                        _ => panic!("link_name is a string"),
                    },
                    Some(_) => panic!("link_name has a value"),
                    None => function.sig.ident.to_string(),
                };
                self.foreign.insert(name);
            }
        }
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if mac
            .path
            .segments
            .last()
            .is_some_and(|segment| segment.ident == "global_asm")
        {
            for token in mac.tokens.clone() {
                let proc_macro2::TokenTree::Literal(literal) = token else {
                    continue;
                };
                let syn::Lit::Str(text) = syn::Lit::new(literal) else {
                    continue;
                };
                for line in text.value().lines() {
                    let mut words = line.split_whitespace();
                    if let (Some(".global" | ".globl"), Some(symbol)) = (words.next(), words.next())
                    {
                        self.assembled.insert(symbol.to_owned());
                    }
                }
            }
        }
        syn::visit::visit_macro(self, mac);
    }
}

impl Declarations {
    /// The declared functions our own assembly does not define: the ROM's.
    fn rom(&self) -> BTreeSet<String> {
        self.foreign.difference(&self.assembled).cloned().collect()
    }
}

fn scan(directory: &Path, declarations: &mut Declarations) {
    for entry in std::fs::read_dir(directory).expect("source directory") {
        let path = entry.expect("source entry").path();
        let name = path
            .file_name()
            .expect("file name")
            .to_str()
            .expect("UTF-8 path");
        if ["pac-raw", "vendor", "registers", "target"].contains(&name) {
            continue;
        }
        if path.is_dir() {
            scan(&path, declarations);
        } else if path.extension().is_some_and(|extension| extension == "rs")
            && name != "generated.rs"
        {
            let source = std::fs::read_to_string(&path).expect("Rust source");
            let file = syn::parse_file(&source).expect("Rust source parses");
            declarations.visit_file(&file);
        }
    }
}

#[test]
fn every_declared_rom_function_and_only_those_functions_has_a_fact() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut declarations = Declarations::default();
    for directory in ["chips/esp32s31", "chips/espressif", "firmware"] {
        scan(&root.join(directory), &mut declarations);
    }
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31").expect("S31 facts");
    let rom = facts.rom.expect("S31 ROM");
    let exports: BTreeSet<_> = rom
        .functions
        .iter()
        .map(|function| function.name.clone())
        .collect();
    assert_eq!(
        declarations.rom(),
        exports,
        "missing or unconsumed ROM facts"
    );
}

#[test]
fn s31_exports_only_the_rom_printf_slot_installers() {
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31").expect("S31 facts");
    let rom = facts.rom.expect("S31 ROM");
    let names: BTreeSet<_> = rom
        .functions
        .iter()
        .map(|function| function.name.as_str())
        .collect();
    assert_eq!(
        names,
        BTreeSet::from(["ets_install_putc1", "ets_install_putc2"])
    );
    let script = oer_espressif_facts::generate::rom_script(&rom).expect("S31 ROM script");
    for name in [
        "ets_delay_us",
        "ets_update_cpu_frequency",
        "software_reset",
        "ets_set_appcpu_boot_addr",
    ] {
        assert!(!rom.functions.iter().any(|function| function.name == name));
        assert!(
            !script
                .lines()
                .any(|line| line.starts_with(&format!("{name} = ")))
        );
    }
    assert!(!script.contains("s_ticks_per_us"));
}

#[test]
fn declarations_use_link_names_and_ignore_definitions_and_linker_data() {
    let source = syn::parse_file(
        r#"
        unsafe extern "C" {
            #[link_name = "rom_entry"] fn wrapper();
            static linker_start: u8;
        }
        mod nested { unsafe extern "C" { fn delay(); } }
        pub extern "C" fn probe_stub() {}
        unsafe extern "Rust" { fn rust_entry(); }
    "#,
    )
    .expect("fixture");
    let mut declarations = Declarations::default();
    declarations.visit_file(&source);
    assert_eq!(
        declarations.rom(),
        BTreeSet::from(["rom_entry".to_string(), "delay".to_string()])
    );
}

#[test]
fn a_declaration_of_a_symbol_our_assembly_defines_is_no_rom_function() {
    let source = syn::parse_file(
        r##"
        core::arch::global_asm!(r#"
        .global probe_thunk
        probe_thunk:
        ret
        "#);
        global_asm!(".globl other_thunk\nother_thunk:\nret");
        unsafe extern "C" {
            fn probe_thunk();
            fn other_thunk();
            fn rom_entry();
        }
    "##,
    )
    .expect("fixture");
    let mut declarations = Declarations::default();
    declarations.visit_file(&source);
    assert_eq!(
        declarations.rom(),
        BTreeSet::from(["rom_entry".to_string()])
    );
}
