use super::*;

fn requested(name: &str) -> Requested {
    Requested {
        name: name.into(),
        symbols: vec![name.into()],
        size: 4,
    }
}

fn section(name: &str, symbols: &[&str], bytes: u64) -> InputSection {
    InputSection {
        file: "actual.o".into(),
        index: 1,
        name: name.into(),
        size: bytes,
        symbols: symbols.iter().map(|name| (*name).into()).collect(),
    }
}

#[test]
fn the_symbol_table_selects_unlikely_and_assembly_sections_without_guessing() {
    for name in [".text.unlikely.unrelated_name", ".assembly_body"] {
        let inputs = [section(name, &["strict", "co_resident"], 12)];
        let plan = Placement::select(&[requested("strict")], &inputs, 12).unwrap();
        assert_eq!(plan.fragment(), format!("\"actual.o\"(\"{name}\");\n"));
        assert_eq!(plan.functions[0].other_symbols, ["co_resident"]);
        assert!(plan.render().contains("co_resident"));
        assert!(plan.render().contains("whole input sections"));
    }
}

#[test]
fn shared_sections_are_counted_and_selected_once() {
    let inputs = [section(".assembly_body", &["first", "second"], 12)];
    let plan = Placement::select(&[requested("first"), requested("second")], &inputs, 12).unwrap();
    assert_eq!(plan.fragment().lines().count(), 1);
    assert_eq!(plan.functions.len(), 2);
    assert_eq!(plan.functions[0].other_symbols, ["second"]);
}

#[test]
fn equally_named_sections_in_one_object_are_all_budgeted_and_reported() {
    let first = section(".assembly_body", &["strict"], 4);
    let mut second = section(".assembly_body", &["neighbour"], 8);
    second.index = 2;
    let inputs = [first, second];
    assert!(Placement::select(&[requested("strict")], &inputs, 11).is_err());
    let plan = Placement::select(&[requested("strict")], &inputs, 12).unwrap();
    assert_eq!(plan.functions[0].section_bytes, 12);
    assert_eq!(plan.functions[0].other_symbols, ["neighbour"]);
}

#[test]
fn unsupported_symbol_and_selector_names_fail_naming_the_function() {
    for name in [
        "bad*",
        "bad?",
        "bad[0]",
        "bad]",
        "bad\\escape",
        "bad\"quote",
        "bad(name)",
        "bad;command",
        "bad name",
    ] {
        let error = Placement::select(&[requested(name)], &[section(".code", &[name], 4)], 64)
            .unwrap_err()
            .to_string();
        assert!(error.contains(name), "{error}");
        assert!(error.contains("escaping"), "{error}");
    }
    for name in [".text.*", ".text[0]"] {
        let error = Placement::select(&[requested("strict")], &[section(name, &["strict"], 4)], 64)
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("strict") && error.contains("escaping"),
            "{error}"
        );
    }
}

#[test]
fn no_strict_functions_produce_an_empty_fragment_even_without_free_sram() {
    let plan = Placement::select(&[], &[section(".text", &["ordinary"], 4)], 0).unwrap();
    assert_eq!(plan.fragment(), "");
    assert_eq!(
        plan.summary(),
        "strict code placed by the image linker: 0 functions, 0 bytes\n"
    );
}

#[test]
fn the_whole_section_must_fit_before_a_second_link() {
    let error = Placement::select(
        &[requested("strict")],
        &[section(
            ".text.unlikely",
            &["strict", "larger_neighbour"],
            20,
        )],
        19,
    )
    .unwrap_err()
    .to_string();
    assert!(
        error.contains("20 bytes") && error.contains("19 bytes") && error.contains("short by 1"),
        "{error}"
    );
}

#[test]
fn missing_and_ambiguous_definitions_are_errors() {
    let inputs = [section(".code", &["strict"], 4)];
    assert!(
        Placement::select(&[requested("missing")], &inputs, 64)
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
    let mut duplicate = inputs[0].clone();
    duplicate.file = "second.o".into();
    assert!(
        Placement::select(&[requested("strict")], &[inputs[0].clone(), duplicate], 64)
            .unwrap_err()
            .to_string()
            .contains("2 defining input sections")
    );
}

#[test]
fn malformed_selection_metadata_is_not_silently_ignored() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join(PLACEMENT), "not a placement record").unwrap();
    assert!(Placement::read(directory.path()).is_err());
}

/// Only the pinned compiler and its LLD run, each in this test's own directory.
#[test]
fn two_handlers_at_one_level_are_both_linked_into_sram() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path();
    let source = directory.join("fixture.rs");
    let object = directory.join("fixture.o");
    std::fs::write(&source, "#![no_std]\n#[unsafe(no_mangle)]\n#[unsafe(link_section=\".text.unlikely.actual_section\")]\npub extern \"C\" fn worst_handler() -> u32 { 42 }\n#[unsafe(no_mangle)]\npub extern \"C\" fn other_handler() -> u32 { 43 }\n").unwrap();
    let result = oer_os_toolchain::command(oer_os_toolchain::Tool::Rustc)
        .unwrap()
        .args([
            "--edition=2024",
            "--crate-type=lib",
            "--emit=obj",
            "--target",
            "riscv32imac-unknown-none-elf",
            "-C",
            "panic=abort",
        ])
        .arg(&source)
        .arg("-o")
        .arg(&object)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let script = directory.join("link.x");
    std::fs::write(&script, "ENTRY(worst_handler)\nSECTIONS { .isr.text 0x20000000 : { INCLUDE strict-text.x } .text 0x50000000 : { *(.text .text.*) } }\n").unwrap();
    let output = directory.join("image.elf");
    let link = |input: &Path| {
        let result = std::process::Command::new(oer_os_toolchain::lld().unwrap())
            .arg("-L")
            .arg(directory)
            .arg("-T")
            .arg(&script)
            .arg(input)
            .arg("-o")
            .arg(&output)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    };
    let bytes = std::fs::read(&object).unwrap();
    // Exercise both direct objects and archive-qualified input selectors.
    let archive_path = directory.join("libfixture.a");
    let mut archive = b"!<arch>\n".to_vec();
    archive.extend_from_slice(
        format!(
            "{:<16}{:<12}{:<6}{:<6}{:<8}{:<10}`\n",
            "fixture.o/",
            0,
            0,
            0,
            644,
            bytes.len()
        )
        .as_bytes(),
    );
    archive.extend_from_slice(&bytes);
    if bytes.len() % 2 != 0 {
        archive.push(b'\n');
    }
    std::fs::write(&archive_path, archive).unwrap();
    for input in [&object, &archive_path] {
        std::fs::write(directory.join(FRAGMENT), "").unwrap();
        link(input);
        let first_bytes = std::fs::read(&output).unwrap();
        let first = oer_analysis_elf::Elf::parse(&first_bytes).unwrap();
        let handlers = ["worst_handler", "other_handler"].map(|name| {
            let function = first.symbol(name).unwrap();
            assert!(function.address >= 0x50000000);
            oer_analysis_riscv_stack::Function {
                address: u32::try_from(function.address).unwrap(),
                size: u32::try_from(function.size).unwrap(),
                names: vec![name.into()],
                section: 0,
            }
        });
        let code = crate::interrupts::same_level_handler_placement(handlers.clone());
        let (_, sections) = crate::linker::inspect_inputs(&[input.into()], &[]).unwrap();
        let requests: Vec<_> = code
            .strict_cached()
            .map(|(address, size)| {
                let function = handlers.iter().find(|function| function.address == address).unwrap();
                Requested {
                    name: function.names[0].clone(),
                    symbols: function.names.clone(),
                    size: u64::from(size),
                }
            })
            .collect();
        let mut plan = Placement::select(&requests, &sections, 4096).unwrap();
        assert!(plan.fragment().contains(".text.unlikely.actual_section"));
        std::fs::write(directory.join(FRAGMENT), plan.fragment()).unwrap();
        link(input);
        let second_bytes = std::fs::read(&output).unwrap();
        let second = oer_analysis_elf::Elf::parse(&second_bytes).unwrap();
        for handler in &handlers {
            let symbol = second.symbol(&handler.names[0]).unwrap();
            assert!((0x20000000..0x20001000).contains(&symbol.address), "{} was not placed in SRAM", handler.names[0]);
        }
        plan.final_sizes(&output).unwrap();
        for function in &plan.functions {
            assert_eq!(function.size, second.symbol(&function.symbol).unwrap().size);
        }
    }
}
