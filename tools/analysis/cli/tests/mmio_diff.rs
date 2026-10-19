//! `mmio-diff` over synthetic static images.
use std::process::Command;

#[allow(dead_code)]
mod support;

/// `lui rd, imm20`.
fn lui(rd: u32, imm: u32) -> u32 {
    (imm << 12) | (rd << 7) | 0x37
}
/// `addi rd, rs, imm`.
fn addi(rd: u32, rs: u32, imm: i32) -> u32 {
    ((imm as u32 & 0xfff) << 20) | (rs << 15) | (rd << 7) | 0x13
}
/// `lw rd, imm(rs)`.
fn lw(rd: u32, rs: u32, imm: i32) -> u32 {
    ((imm as u32 & 0xfff) << 20) | (rs << 15) | (2 << 12) | (rd << 7) | 0x03
}
/// `sw rs2, imm(rs1)`.
fn sw(rs2: u32, rs1: u32, imm: i32) -> u32 {
    let imm = imm as u32 & 0xfff;
    ((imm >> 5) << 25) | (rs2 << 20) | (rs1 << 15) | (2 << 12) | ((imm & 0x1f) << 7) | 0x23
}
const RET: u32 = 0x0000_8067;
const A3: u32 = 13;
const A4: u32 = 14;
const A5: u32 = 15;

/// `*(0x20109c18) = value`.
fn poke(value: u32) -> Vec<u32> {
    let high = value.wrapping_add(0x800) >> 12;
    let low = value.wrapping_sub(high << 12) as i32;
    vec![
        lui(A4, high),
        addi(A4, A4, low),
        lui(A5, 0x2010a),
        sw(A4, A5, -1000),
        RET,
    ]
}

/// `*(0x20109c18) = *(data)`.
fn copy(data: u32) -> Vec<u32> {
    vec![
        lui(A3, data >> 12),
        lw(A4, A3, (data & 0xfff) as i32),
        lui(A5, 0x2010a),
        sw(A4, A5, -1000),
        RET,
    ]
}

/// A static image with `functions` laid out from 0x1000 in order and two
/// data words at `data`: the objects `DESCRIPTOR` and `SECOND`.
fn image(functions: &[(&str, Vec<u32>)], data: u32) -> Vec<u8> {
    image_sized(functions, data, &[])
}

/// [`image`] whose functions named in `sizeless` have no symbol size.
fn image_sized(functions: &[(&str, Vec<u32>)], data: u32, sizeless: &[&str]) -> Vec<u8> {
    use object::elf;
    use object::write::elf::{FileHeader, ProgramHeader, SectionHeader, Sym, Writer};
    let code: Vec<u8> = functions
        .iter()
        .flat_map(|(_, words)| words.iter().flat_map(|w| w.to_le_bytes()))
        .collect();
    let words: Vec<u8> = [0x1234_5678u32, 0x9abc_def0]
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let mut out = Vec::new();
    let mut w = Writer::new(object::Endianness::Little, false, &mut out);
    w.reserve_file_header();
    w.reserve_program_headers(2);
    let text_name = w.add_section_name(b".text");
    let text = w.reserve_section_index();
    let text_offset = w.reserve(code.len(), 4);
    let data_name = w.add_section_name(b".rodata");
    let data_section = w.reserve_section_index();
    let data_offset = w.reserve(8, 4);
    w.reserve_null_symbol_index();
    let descriptor = w.add_string(b"_RNvCs1a_4demo10DESCRIPTOR");
    w.reserve_symbol_index(None);
    let second = w.add_string(b"_RNvCs1a_4demo6SECOND");
    w.reserve_symbol_index(None);
    let names: Vec<_> = functions
        .iter()
        .map(|(name, _)| {
            let name = w.add_string(name.as_bytes());
            w.reserve_symbol_index(None);
            name
        })
        .collect();
    w.reserve_symtab_section_index();
    w.reserve_symtab();
    w.reserve_strtab_section_index();
    w.reserve_strtab();
    w.reserve_shstrtab_section_index();
    w.reserve_shstrtab();
    w.reserve_section_headers();
    w.write_file_header(&FileHeader {
        os_abi: 0,
        abi_version: 0,
        e_type: elf::ET_EXEC,
        e_machine: elf::EM_RISCV,
        e_entry: 0x1000,
        e_flags: 0,
    })
    .unwrap();
    w.write_align_program_headers();
    for (offset, address, size, flags) in [
        (text_offset, 0x1000, code.len(), elf::PF_R | elf::PF_X),
        (data_offset, u64::from(data), 8, elf::PF_R),
    ] {
        w.write_program_header(&ProgramHeader {
            p_type: elf::PT_LOAD,
            p_flags: flags,
            p_offset: offset as u64,
            p_vaddr: address,
            p_paddr: address,
            p_filesz: size as u64,
            p_memsz: size as u64,
            p_align: 4,
        });
    }
    w.write_align(4);
    w.write(&code);
    w.write_align(4);
    w.write(&words);
    w.write_null_symbol();
    w.write_symbol(&Sym {
        name: Some(descriptor),
        section: Some(data_section),
        st_info: (elf::STB_GLOBAL << 4) | elf::STT_OBJECT,
        st_other: 0,
        st_shndx: 0,
        st_value: u64::from(data),
        st_size: 4,
    });
    w.write_symbol(&Sym {
        name: Some(second),
        section: Some(data_section),
        st_info: (elf::STB_GLOBAL << 4) | elf::STT_OBJECT,
        st_other: 0,
        st_shndx: 0,
        st_value: u64::from(data) + 4,
        st_size: 4,
    });
    let mut at = 0x1000u64;
    for ((function, words), name) in functions.iter().zip(names) {
        let size = 4 * words.len() as u64;
        let declared = if sizeless.contains(function) { 0 } else { size };
        w.write_symbol(&Sym {
            name: Some(name),
            section: Some(text),
            st_info: (elf::STB_GLOBAL << 4) | elf::STT_FUNC,
            st_other: 0,
            st_shndx: 0,
            st_value: at,
            st_size: declared,
        });
        at += size;
    }
    w.write_strtab();
    w.write_shstrtab();
    w.write_null_section_header();
    for (name, address, offset, size, flags) in [
        (
            text_name,
            0x1000,
            text_offset,
            code.len(),
            elf::SHF_ALLOC | elf::SHF_EXECINSTR,
        ),
        (data_name, u64::from(data), data_offset, 8, elf::SHF_ALLOC),
    ] {
        w.write_section_header(&SectionHeader {
            name: Some(name),
            sh_type: elf::SHT_PROGBITS,
            sh_flags: u64::from(flags),
            sh_addr: address,
            sh_offset: offset as u64,
            sh_size: size as u64,
            sh_link: 0,
            sh_info: 0,
            sh_addralign: 4,
            sh_entsize: 0,
        });
    }
    w.write_symtab_section_header(1);
    w.write_strtab_section_header();
    w.write_shstrtab_section_header();
    assert_eq!(w.reserved_len(), w.len());
    out
}

/// `mmio-diff`'s output for the files `old` and `new` in `format`.
fn run(old: &[u8], new: &[u8], format: &str) -> std::process::Output {
    let dir = tempfile::tempdir().unwrap();
    let (old_path, new_path) = (dir.path().join("old"), dir.path().join("new"));
    std::fs::write(&old_path, old).unwrap();
    std::fs::write(&new_path, new).unwrap();
    Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            format,
            "mmio-diff",
            "--range",
            "0x20000000:0x1000000",
            "--old",
        ])
        .arg(format!("code={}", old_path.display()))
        .arg("--new")
        .arg(format!("code={}", new_path.display()))
        .output()
        .unwrap()
}

fn diff(old: Vec<u8>, new: Vec<u8>) -> oer_analysis_blobray_domain::MmioDiff {
    let dir = tempfile::tempdir().unwrap();
    let (old_path, new_path) = (dir.path().join("old.elf"), dir.path().join("new.elf"));
    std::fs::write(&old_path, old).unwrap();
    std::fs::write(&new_path, new).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "mmio-diff",
            "--range",
            "0x20000000:0x1000000",
            "--old",
        ])
        .arg(format!("image={}", old_path.display()))
        .arg("--new")
        .arg(format!("image={}", new_path.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::MmioDiffDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.schema,
        oer_tool_analysis_cli::wire::MMIO_DIFF_SCHEMA
    );
    document.diff
}

#[test]
fn hashes_layout_and_data_addresses_make_no_difference() {
    let old = image(
        &[
            ("_RNvCs1a_4demo4poke", poke(0xefff_ffff)),
            ("_RNvCs1a_4demo4copy", copy(0x3000)),
        ],
        0x3000,
    );
    // Other crate disambiguators, the other order, the data word elsewhere.
    let new = image(
        &[
            ("_RNvCs9z_4demo4copy", copy(0x3100)),
            ("_RNvCs9z_4demo4poke", poke(0xefff_ffff)),
        ],
        0x3100,
    );
    let diff = diff(old, new);
    assert_eq!(diff.functions, [2, 2]);
    assert!(diff.changes.is_empty(), "{:?}", diff.changes);
}

#[test]
fn a_changed_stored_mask_is_exactly_that_store_of_that_function() {
    let old = image(
        &[
            ("_RNvCs1a_4demo4poke", poke(0xefff_ffff)),
            ("_RNvCs1a_4demo4copy", copy(0x3000)),
        ],
        0x3000,
    );
    let new = image(
        &[
            ("_RNvCs1a_4demo4poke", poke(0x0fff_ffff)),
            ("_RNvCs1a_4demo4copy", copy(0x3000)),
        ],
        0x3000,
    );
    let diff = diff(old, new);
    assert_eq!(diff.changes.len(), 1, "{:?}", diff.changes);
    let change = &diff.changes[0];
    assert_eq!(change.function, "demo::poke");
    assert_eq!(
        (change.removed.len(), change.added.len()),
        (1, 1),
        "{change:?}"
    );
    let constant = |access: &oer_analysis_blobray_domain::MmioAccess| match access.stored.as_slice()
    {
        [
            oer_analysis_blobray_domain::MmioStoredBits {
                low: 0,
                width: 32,
                source: oer_analysis_blobray_domain::MmioStoredSource::Constant { value, .. },
            },
        ] => *value,
        other => panic!("{other:?}"),
    };
    assert_eq!(
        change.removed[0].location,
        oer_analysis_blobray_domain::MmioLocation::At {
            address: 0x2010_9c18
        }
    );
    assert_eq!(constant(&change.removed[0]), 0xefff_ffff);
    assert_eq!(constant(&change.added[0]), 0x0fff_ffff);
}

#[test]
fn a_function_that_stops_writing_mmio_is_a_change_not_a_removal() {
    let old = image(&[("_RNvCs1a_4demo4poke", poke(0xefff_ffff))], 0x3000);
    // The same function now writes RAM outside the window.
    let new = image(&[("_RNvCs1a_4demo4poke", copy_to_ram())], 0x3000);
    let diff = diff(old, new);
    assert_eq!(diff.changes.len(), 1, "{:?}", diff.changes);
    let change = &diff.changes[0];
    assert_eq!(
        change.state,
        [oer_analysis_blobray_domain::MmioFunctionState::Analyzed; 2],
        "it is defined in both builds"
    );
    assert_eq!((change.removed.len(), change.added.len()), (1, 0));
}

#[test]
fn a_blocked_function_reports_its_state_and_no_accesses() {
    // A second function keeps the new build analyzed.
    let functions = [
        ("_RNvCs1a_4demo4poke", poke(0xefff_ffff)),
        ("_RNvCs1a_4demo4copy", copy(0x3000)),
    ];
    let old = image(&functions, 0x3000);
    let new = image_sized(&functions, 0x3000, &["_RNvCs1a_4demo4poke"]);
    let diff = diff(old, new);
    assert_eq!(diff.blocked, [0, 1]);
    let change = &diff.changes[0];
    assert_eq!(
        change.state,
        [
            oer_analysis_blobray_domain::MmioFunctionState::Analyzed,
            oer_analysis_blobray_domain::MmioFunctionState::Blocked
        ]
    );
    assert!(
        change.removed.is_empty() && change.added.is_empty(),
        "{change:?}"
    );
    // The blocked build has no complete analysis of it.
    assert_eq!(change.complete, [true, false]);
}

#[test]
fn a_function_both_builds_block_is_uncertain_not_unchanged() {
    // `copy` keeps both builds analyzed; `gone` only the old one defines.
    let functions = [
        ("_RNvCs1a_4demo4poke", poke(0xefff_ffff)),
        ("_RNvCs1a_4demo4copy", copy(0x3000)),
        ("_RNvCs1a_4demo4gone", copy(0x3000)),
    ];
    let old = image_sized(&functions, 0x3000, &["_RNvCs1a_4demo4poke"]);
    let new = image_sized(&functions[..2], 0x3000, &["_RNvCs1a_4demo4poke"]);
    let diff = diff(old, new);
    assert!(
        diff.changes.iter().all(|c| c.function != "demo::poke"),
        "{diff:?}"
    );
    let uncertain: Vec<_> = diff.uncertain.iter().map(|u| u.function.as_str()).collect();
    // An unchanged complete function, or one a build lacks, is not uncertain.
    assert_eq!(uncertain, ["demo::poke"]);
    assert_eq!(
        diff.uncertain[0].state,
        [oer_analysis_blobray_domain::MmioFunctionState::Blocked; 2]
    );
    assert_eq!(diff.uncertain[0].complete, [false, false]);
}

/// `*(0x3000) = 1`: a store outside the window.
fn copy_to_ram() -> Vec<u32> {
    vec![addi(A4, 0, 1), lui(A5, 0x3), sw(A4, A5, 0), RET]
}

#[test]
fn a_build_without_analyzed_functions_is_an_error_not_a_removal() {
    let dir = tempfile::tempdir().unwrap();
    let old = dir.path().join("old.elf");
    let new = dir.path().join("firmware.bin");
    std::fs::write(
        &old,
        image(&[("_RNvCs1a_4demo4poke", poke(0xefff_ffff))], 0x3000),
    )
    .unwrap();
    std::fs::write(&new, b"not an ELF or archive").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "mmio-diff",
            "--range",
            "0x20000000:0x1000000",
            "--old",
        ])
        .arg(format!("image={}", old.display()))
        .arg("--new")
        .arg(format!("image={}", new.display()))
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("the new build has no analyzed function"),
        "{output:?}"
    );
}

#[test]
fn a_build_whose_functions_are_all_blocked_says_so() {
    let functions = [("_RNvCs1a_4demo4poke", poke(0xefff_ffff))];
    let old = image(&functions, 0x3000);
    let new = image_sized(&functions, 0x3000, &["_RNvCs1a_4demo4poke"]);
    let output = run(&old, &new, "json");
    assert!(!output.status.success(), "{output:?}");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("the new build has no analyzed function: its 1 functions are blocked"),
        "{output:?}"
    );
}

/// A relocatable object defining `demo::poke`.
fn poke_object() -> Vec<u8> {
    use object::write::{Object, Symbol, SymbolSection};
    use object::{Architecture, BinaryFormat, Endianness, SymbolFlags, SymbolKind, SymbolScope};
    let code: Vec<u8> = poke(0xefff_ffff)
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.section_id(object::write::StandardSection::Text);
    object.append_section_data(text, &code, 4);
    object.add_symbol(Symbol {
        name: b"_RNvCs1a_4demo4poke".to_vec(),
        value: 0,
        size: code.len() as u64,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    object.write().unwrap()
}

#[test]
fn a_member_the_build_cannot_analyze_is_named_with_its_input_and_ordinal() {
    let old = support::archive(&[(b"poke.o", &poke_object())], false);
    let new = support::archive(
        &[
            (b"poke.o", &poke_object()),
            (b"notes.txt", b"not an object"),
        ],
        false,
    );
    let output = run(&old, &new, "json");
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::MmioDiffDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert!(document.diff.gaps[0].is_empty());
    let gap = &document.diff.gaps[1][0];
    assert_eq!(gap.input, 0);
    assert_eq!(
        gap.object.as_ref().map(|object| object.location.clone()),
        Some(oer_analysis_riscv_model::ObjectLocation::ArchiveMember { ordinal: 1 })
    );
    let human = run(&old, &new, "human");
    let text = String::from_utf8_lossy(&human.stdout);
    assert!(
        text.contains("new build input 0 member 1 not analyzed: "),
        "{text}"
    );
}

#[test]
fn a_store_that_overlaps_a_window_only_partly_still_compares() {
    let old = image(&[("_RNvCs1a_4demo4poke", poke(0xefff_ffff))], 0x3000);
    let new = image(&[("_RNvCs1a_4demo4poke", poke(0x0fff_ffff))], 0x3000);
    let dir = tempfile::tempdir().unwrap();
    let (old_path, new_path) = (dir.path().join("old.elf"), dir.path().join("new.elf"));
    std::fs::write(&old_path, old).unwrap();
    std::fs::write(&new_path, new).unwrap();
    // The window holds only the upper half of the stored word.
    let output = Command::new(env!("CARGO_BIN_EXE_blobray"))
        .args([
            "--format",
            "json",
            "mmio-diff",
            "--range",
            "0x20109c1a:2",
            "--old",
        ])
        .arg(format!("image={}", old_path.display()))
        .arg("--new")
        .arg(format!("image={}", new_path.display()))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let document: oer_tool_analysis_cli::wire::MmioDiffDocument =
        serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        document.diff.changes.len(),
        1,
        "{:?}",
        document.diff.changes
    );
    let change = &document.diff.changes[0];
    assert_eq!(
        (change.removed.len(), change.added.len()),
        (1, 1),
        "{change:?}"
    );
}

#[test]
fn a_stored_address_of_a_moved_object_makes_no_difference() {
    // The function writes the descriptor's address to the register; the
    // descriptor lies elsewhere in the other build.
    let old = image(&[("_RNvCs1a_4demo4poke", poke(0x3000))], 0x3000);
    let new = image(&[("_RNvCs1a_4demo4poke", poke(0x3100))], 0x3100);
    let moved = diff(old.clone(), new);
    assert!(moved.changes.is_empty(), "{:?}", moved.changes);
    // A pointer to somewhere other than the descriptor is a change.
    let elsewhere = image(&[("_RNvCs1a_4demo4poke", poke(0x7000))], 0x3000);
    assert_eq!(diff(old, elsewhere).changes.len(), 1);
}

#[test]
fn a_stored_address_past_every_sized_symbol_compares_by_value() {
    // The descriptor's end is no address inside it: a moved descriptor
    // moves the stored value, as the reference documents.
    let old = image(&[("_RNvCs1a_4demo4poke", poke(0x3008))], 0x3000);
    let new = image(&[("_RNvCs1a_4demo4poke", poke(0x3108))], 0x3100);
    assert_eq!(diff(old, new).changes.len(), 1);
}

#[test]
fn the_same_stored_value_is_no_change_whatever_symbol_it_lands_in() {
    // Both builds store 0x3000; only the old one has the descriptor there.
    let old = image(&[("_RNvCs1a_4demo4poke", poke(0x3000))], 0x3000);
    let new = image(&[("_RNvCs1a_4demo4poke", poke(0x3000))], 0x3100);
    let diff = diff(old, new);
    assert!(diff.changes.is_empty(), "{:?}", diff.changes);
}

#[test]
fn objects_moved_onto_each_others_addresses_make_no_difference() {
    // The function stores both objects' addresses; in the new build both
    // move by one word, so `SECOND`'s old address is `DESCRIPTOR`'s new one.
    let both = |descriptor: u32| {
        let mut words = poke(descriptor);
        words.pop();
        words.extend(poke(descriptor + 4));
        words
    };
    let old = image(&[("_RNvCs1a_4demo4poke", both(0x3000))], 0x3000);
    let new = image(&[("_RNvCs1a_4demo4poke", both(0x3004))], 0x3004);
    let diff = diff(old, new);
    assert!(diff.changes.is_empty(), "{:?}", diff.changes);
}
