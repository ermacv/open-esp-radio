//! The ROM exports are exactly the foreign functions declared by our S31
//! code. Generated PACs and vendor research facts are not ABI consumers.

use std::{collections::BTreeSet, path::Path};

use syn::visit::Visit;

#[derive(Default)]
struct Declarations(BTreeSet<String>);

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
                self.0.insert(name);
            }
        }
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
        if name.contains("esp32c5") || ["pac-raw", "vendor", "registers", "target"].contains(&name)
        {
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
    assert!(
        !declarations.0.is_empty(),
        "the scan finds ROM declarations"
    );
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31").expect("S31 facts");
    let rom = facts.rom.expect("S31 ROM");
    let exports: BTreeSet<_> = rom
        .functions
        .iter()
        .map(|function| function.name.clone())
        .collect();
    assert_eq!(declarations.0, exports, "missing or unconsumed ROM facts");
}

#[test]
fn s31_exports_cite_the_pinned_nonradio_rom_script() {
    let artifacts: toml::Table =
        toml::from_str(include_str!("../../../esp32s31/vendor/artifacts.toml"))
            .expect("artifact pins");
    let pin = artifacts["source"]
        .as_array()
        .expect("sources")
        .iter()
        .find(|source| source["id"].as_str() == Some("esp-idf"))
        .expect("ESP-IDF pin")["revision"]
        .as_str()
        .expect("revision");
    let prefix = format!("SOURCE: ESP-IDF {pin} components/esp_rom/esp32s31/ld/esp32s31.rom.ld:");
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31").expect("S31 facts");
    for function in facts.rom.expect("S31 ROM").functions {
        let line = function
            .source
            .strip_prefix(&prefix)
            .expect("pinned nonradio script citation");
        assert!(
            line.parse::<usize>().is_ok_and(|line| line > 0),
            "source line"
        );
    }
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
        declarations.0,
        BTreeSet::from(["rom_entry".to_string(), "delay".to_string()])
    );
}
