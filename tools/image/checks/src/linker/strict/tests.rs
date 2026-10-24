use super::*;

fn requested(name: &str) -> Requested {
    Requested {
        name: name.into(),
        symbols: vec![name.into()],
        size: 4,
    }
}

fn select(requested: &[Requested], sections: &[InputSection], free: u64) -> Result<Placement> {
    Placement::select(requested, &[], sections, free)
}

fn data(name: &str, symbols: &[&str]) -> RequestedData {
    RequestedData {
        name: name.into(),
        symbols: symbols.iter().map(|name| (*name).into()).collect(),
    }
}

fn section(name: &str, symbols: &[&str], bytes: u64) -> InputSection {
    InputSection {
        file: "actual.o".into(),
        index: 1,
        name: name.into(),
        size: bytes,
        kind: InputKind::Code,
        symbols: symbols.iter().map(|name| (*name).into()).collect(),
    }
}

#[test]
fn the_symbol_table_selects_unlikely_and_assembly_sections_without_guessing() {
    for name in [".text.unlikely.unrelated_name", ".assembly_body"] {
        let inputs = [section(name, &["strict", "co_resident"], 12)];
        let plan = select(&[requested("strict")], &inputs, 12).unwrap();
        assert_eq!(plan.fragments()[0].1, format!("\"actual.o\"(\"{name}\");\n"));
        assert_eq!(plan.functions[0].other_symbols, ["co_resident"]);
        assert!(plan.render().contains("co_resident"));
        assert!(plan.render().contains("whole input sections"));
    }
}

#[test]
fn shared_sections_are_counted_and_selected_once() {
    let inputs = [section(".assembly_body", &["first", "second"], 12)];
    let plan = select(&[requested("first"), requested("second")], &inputs, 12).unwrap();
    assert_eq!(plan.fragments()[0].1.lines().count(), 1);
    assert_eq!(plan.functions.len(), 2);
    assert_eq!(plan.functions[0].other_symbols, ["second"]);
}

#[test]
fn equally_named_sections_in_one_object_are_all_budgeted_and_reported() {
    let first = section(".assembly_body", &["strict"], 4);
    let mut second = section(".assembly_body", &["neighbour"], 8);
    second.index = 2;
    let inputs = [first, second];
    assert!(select(&[requested("strict")], &inputs, 11).is_err());
    let plan = select(&[requested("strict")], &inputs, 12).unwrap();
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
        let error = select(&[requested(name)], &[section(".code", &[name], 4)], 64)
            .unwrap_err()
            .to_string();
        assert!(error.contains(name), "{error}");
        assert!(error.contains("escaping"), "{error}");
    }
    for name in [".text.*", ".text[0]"] {
        let error = select(&[requested("strict")], &[section(name, &["strict"], 4)], 64)
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
    let plan = select(&[], &[section(".text", &["ordinary"], 4)], 0).unwrap();
    assert_eq!(plan.fragments()[0].1, "");
    assert_eq!(
        plan.summary(),
        "strict code placed by the image linker: 0 functions, 0 bytes\n"
    );
}

#[test]
fn the_whole_section_must_fit_before_a_second_link() {
    let error = select(&[requested("strict")],
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
        select(&[requested("missing")], &inputs, 64)
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
    let mut duplicate = inputs[0].clone();
    duplicate.file = "second.o".into();
    assert!(
        select(&[requested("strict")], &[inputs[0].clone(), duplicate], 64)
            .unwrap_err()
            .to_string()
            .contains("2 defining input sections")
    );
}

#[test]
fn own_data_moves_by_kind_into_the_data_and_zeroed_fragments() {
    let mut zeroed = section(".bss.WRITER", &["WRITER", "NEIGHBOUR"], 8);
    zeroed.kind = InputKind::Zeroed;
    let mut table = section(".rodata..Lswitch.table.service", &[".Lswitch.table.service"], 12);
    table.kind = InputKind::Initialized;
    table.index = 2;
    let code = section(".text.strict", &["strict"], 4);
    let inputs = [zeroed, table, code];
    let objects = [
        data("WRITER", &["WRITER"]),
        data(".Lswitch.table.service", &[".Lswitch.table.service"]),
    ];
    let plan = Placement::select(&[requested("strict")], &objects, &inputs, 24).unwrap();
    let [text, initialized, zero] = plan.fragments();
    assert_eq!(text, (TEXT_FRAGMENT, "\"actual.o\"(\".text.strict\");\n".to_owned()));
    assert_eq!(
        initialized,
        (
            DATA_FRAGMENT,
            "\"actual.o\"(\".rodata..Lswitch.table.service\");\n".to_owned()
        )
    );
    assert_eq!(zero, (ZEROED_FRAGMENT, "\"actual.o\"(\".bss.WRITER\");\n".to_owned()));
    assert_eq!(plan.data[0].other_symbols, ["NEIGHBOUR"]);
    assert!(plan.render().contains(".critical.bss"));
    // Data counts against the same free SRAM as code.
    assert!(
        Placement::select(&[requested("strict")], &objects, &inputs, 23)
            .unwrap_err()
            .to_string()
            .contains("short by 1")
    );
}

#[test]
fn data_without_one_movable_section_stays_for_the_gate_to_report() {
    let mut placed = section(".psram.bss.BUFFER", &["BUFFER"], 4);
    placed.kind = InputKind::Zeroed;
    // A function's name never selects a data section.
    let code = section(".text.FUNCTION", &["FUNCTION"], 4);
    let plan = Placement::select(
        &[],
        &[
            data("BUFFER", &["BUFFER"]),
            data("FUNCTION", &["FUNCTION"]),
            data("0x50000000", &[]),
        ],
        &[placed, code],
        64,
    )
    .unwrap();
    assert!(plan.moves_nothing());
    assert_eq!(
        plan.unmoved,
        [
            "BUFFER: its input section .psram.bss.BUFFER is placed by name",
            "FUNCTION: 0 input sections define it; expected exactly one",
            "0x50000000: 0 input sections define it; expected exactly one",
        ]
    );
    assert!(plan.fragments().iter().all(|(_, text)| text.is_empty()));
    assert!(plan.summary().contains("3 own objects not moved"));
}

#[test]
fn malformed_selection_metadata_is_not_silently_ignored() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join(PLACEMENT), "not a placement record").unwrap();
    assert!(Placement::read(directory.path()).is_err());
}

/// Only the pinned compiler and its LLD run, each in this test's own directory.
#[test]
fn the_pinned_toolchain_moves_the_actual_input_section_from_psram_to_sram() {
    let directory = tempfile::tempdir().unwrap();
    let directory = directory.path();
    let source = directory.join("fixture.rs");
    let object = directory.join("fixture.o");
    std::fs::write(&source, "#![no_std]\n#[unsafe(no_mangle)]\n#[unsafe(link_section=\".text.unlikely.actual_section\")]\npub extern \"C\" fn strict_function() -> u32 { 42 }\n").unwrap();
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
    std::fs::write(&script, "ENTRY(strict_function)\nSECTIONS { .isr.text 0x20000000 : { INCLUDE strict-text.x } .text 0x50000000 : { *(.text .text.*) } }\n").unwrap();
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
        std::fs::write(directory.join(TEXT_FRAGMENT), "").unwrap();
        link(input);
        let first_bytes = std::fs::read(&output).unwrap();
        let first = oer_analysis_elf::Elf::parse(&first_bytes).unwrap();
        let function = first.symbol("strict_function").unwrap();
        assert_eq!(function.address, 0x50000000);
        let (_, sections) = crate::linker::inspect_inputs(&[input.into()], &[]).unwrap();
        let mut request = requested("strict_function");
        request.size = function.size;
        let mut plan = select(&[request], &sections, 4096).unwrap();
        assert!(plan.fragments()[0].1.contains(".text.unlikely.actual_section"));
        std::fs::write(directory.join(TEXT_FRAGMENT), plan.fragments()[0].1).unwrap();
        link(input);
        let second_bytes = std::fs::read(&output).unwrap();
        let second = oer_analysis_elf::Elf::parse(&second_bytes).unwrap();
        assert_eq!(
            second.symbol("strict_function").unwrap().address,
            0x20000000
        );
        plan.final_sizes(&output).unwrap();
        assert_eq!(
            plan.functions[0].size,
            second.symbol("strict_function").unwrap().size
        );
    }
}
