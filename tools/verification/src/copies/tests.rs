use super::*;
use object::write::{Object, Symbol, SymbolSection};
use object::{
    Architecture, BinaryFormat, Endianness, SectionKind, SymbolFlags, SymbolKind, SymbolScope,
};
use oer_analysis_blobray_domain::MmioFunctionState::{Analyzed, Blocked};

const RET: u32 = 0x0000_8067;
const BUDGET: Budget = Budget {
    timeout_secs: 60,
    working_memory_mib: 64,
    max_work_units: 1_000_000_000,
};
/// `lui a5, 0x20000; li a4, VALUE; sw a4, 0(a5); ret`.
fn store(value: u32) -> [u32; 4] {
    [0x2000_07b7, (value << 20) | 0x0000_0713, 0x00e7_a023, RET]
}

/// A relocatable object of the global functions `(name, code)`.
fn object(functions: &[(&str, &[u32])]) -> Vec<u8> {
    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    for (name, code) in functions {
        let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
        let offset = object.append_section_data(text, &bytes, 4);
        object.add_symbol(Symbol {
            name: name.as_bytes().to_vec(),
            value: offset,
            size: bytes.len() as u64,
            kind: SymbolKind::Text,
            scope: SymbolScope::Linkage,
            weak: false,
            section: SymbolSection::Section(text),
            flags: SymbolFlags::None,
        });
    }
    object.write().unwrap()
}

fn input(id: &str, bytes: Vec<u8>) -> Input {
    Input {
        id: id.to_owned(),
        bytes,
    }
}

#[test]
fn copies_that_store_other_bits_differ_and_equal_ones_show_no_difference() {
    let rom = object(&[("f", &store(1)), ("g", &store(2)), ("rom_only", &store(5))]);
    let library = object(&[("f", &store(3)), ("g", &store(2))]);
    let cited = BTreeMap::from([("f".to_owned(), vec!["lib[]::f".to_owned()])]);
    let report = compare(
        "chip",
        &[input("rom", rom)],
        &[input("lib", library)],
        &[],
        &[ImageRegion {
            start: 0x2000_0000,
            length: 0x1000,
        }],
        &[],
        &cited,
        &BUDGET,
    )
    .unwrap();
    let verdicts: Vec<_> = report
        .functions
        .iter()
        .map(|f| (f.name.as_str(), f.verdict, f.compared.as_str()))
        .collect();
    // A function only one side defines is no pair of copies.
    assert_eq!(
        verdicts,
        [
            ("f", Verdict::Differs, "archives"),
            ("g", Verdict::NoDifferenceSeen, "archives")
        ]
    );
    let f = &report.functions[0];
    assert_eq!(f.rom, "rom[]::f");
    assert_eq!(f.libraries, ["lib[]::f"]);
    assert_eq!(f.cited, ["lib[]::f"]);
    assert_eq!((f.rom_only.len(), f.library_only.len()), (1, 1));
    assert!(human(&report).contains("f Differs rom[]::f via archives, cited lib[]::f"));
}

fn change(state: [MmioFunctionState; 2], complete: [bool; 2], differs: bool) -> MmioFunctionChange {
    let access: MmioAccess = serde_json::from_value(serde_json::json!({
        "location": {"kind": "at", "address": 0x2000_0000u32},
        "operation": "load",
        "width": 4
    }))
    .unwrap();
    MmioFunctionChange {
        function: "f".into(),
        state,
        complete,
        removed: if differs { vec![access] } else { vec![] },
        added: vec![],
        unresolved: [0, 1],
    }
}

#[test]
fn a_verdict_never_claims_more_than_the_analyses_prove() {
    assert_eq!(verdict(None, None).0, Verdict::NoDifferenceSeen);
    let both = [Analyzed, Analyzed];
    // `change` differs by one load only.
    assert_eq!(
        verdict(Some(&change(both, [true, true], true)), None).0,
        Verdict::ReadsDiffer
    );
    assert_eq!(
        verdict(Some(&change(both, [true, false], true)), None).0,
        Verdict::DiffersIncomplete
    );
    assert_eq!(
        verdict(Some(&change(both, [true, true], false)), None).0,
        Verdict::UnresolvedDiffers
    );
    assert_eq!(
        verdict(
            Some(&change([Analyzed, Blocked], [true, false], false)),
            None
        )
        .0,
        Verdict::Unknown
    );
}

#[test]
fn each_rom_revision_is_compared_on_its_own() {
    // The same code in two ROM revisions and the library: compared at once,
    // the two ROM copies would double every access.
    let rom = || object(&[("g", &store(2))]);
    let report = compare(
        "chip",
        &[input("rom-rev0", rom()), input("rom-rev100", rom())],
        &[input("lib", object(&[("g", &store(2))]))],
        &[],
        &[ImageRegion {
            start: 0x2000_0000,
            length: 0x1000,
        }],
        &[],
        &BTreeMap::new(),
        &BUDGET,
    )
    .unwrap();
    let verdicts: Vec<_> = report
        .functions
        .iter()
        .map(|f| (f.rom.as_str(), f.verdict))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("rom-rev0[]::g", Verdict::NoDifferenceSeen),
            ("rom-rev100[]::g", Verdict::NoDifferenceSeen)
        ]
    );
}

fn store_at(address: u32, source: serde_json::Value) -> MmioAccess {
    serde_json::from_value(serde_json::json!({
        "location": {"kind": "at", "address": address},
        "operation": "store",
        "width": 4,
        "stored": [{"low": 0, "width": 32, "source": source}]
    }))
    .unwrap()
}

#[test]
fn a_store_whose_value_one_side_cannot_resolve_is_a_weak_difference() {
    let one = store_at(
        0x2000_0000,
        serde_json::json!({"kind": "constant", "value": 1}),
    );
    let three = store_at(
        0x2000_0000,
        serde_json::json!({"kind": "constant", "value": 3}),
    );
    let unknown = store_at(0x2000_0000, serde_json::json!({"kind": "unknown"}));
    let elsewhere = store_at(
        0x2000_0004,
        serde_json::json!({"kind": "constant", "value": 1}),
    );
    assert_eq!(
        writes(std::slice::from_ref(&one), &[three]),
        Verdict::Differs
    );
    assert_eq!(
        writes(std::slice::from_ref(&one), &[unknown]),
        Verdict::StoredValueUnknown
    );
    assert_eq!(writes(&[one], &[elsewhere]), Verdict::Differs);
}

#[test]
fn an_unlisted_function_a_build_blocks_or_sees_incompletely_is_no_proven_match() {
    let uncertain = |state, complete| MmioUncertainFunction {
        function: "f".into(),
        state,
        complete,
    };
    assert_eq!(
        verdict(None, Some(&uncertain([Blocked, Blocked], [false, false]))),
        (Verdict::Unknown, [false, false])
    );
    assert_eq!(
        verdict(None, Some(&uncertain([Analyzed, Analyzed], [false, true]))),
        (Verdict::NoDifferenceSeen, [false, true])
    );
}

fn window() -> [ImageRegion; 1] {
    [ImageRegion {
        start: 0x2000_0000,
        length: 0x1000,
    }]
}

#[test]
fn identical_copies_in_several_libraries_are_no_write_difference() {
    let lib = || object(&[("f", &store(2))]);
    let report = compare(
        "chip",
        &[input("rom", object(&[("f", &store(2))]))],
        &[input("liba", lib()), input("libb", lib())],
        &[],
        &window(),
        &[],
        &BTreeMap::new(),
        &BUDGET,
    )
    .unwrap();
    assert_eq!(report.functions[0].verdict, Verdict::SeveralCopies);
    assert_eq!(report.functions[0].libraries, ["liba[]::f", "libb[]::f"]);
}

/// An object Blobray does not analyze (not RV32) defining `f`.
fn foreign() -> Vec<u8> {
    let mut foreign = Object::new(BinaryFormat::Elf, Architecture::X86_64, Endianness::Little);
    let text = foreign.add_section(Vec::new(), b".text".to_vec(), SectionKind::Text);
    foreign.append_section_data(text, &[0xc3], 1);
    foreign.add_symbol(Symbol {
        name: b"f".to_vec(),
        value: 0,
        size: 1,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    foreign.write().unwrap()
}

#[test]
fn a_copy_in_code_the_analysis_skips_is_unknown_not_matched() {
    let report = compare(
        "chip",
        &[input("rom", object(&[("f", &store(2)), ("g", &store(2))]))],
        &[
            input("lib", object(&[("g", &store(2))])),
            input("foreign", foreign()),
        ],
        &[],
        &window(),
        &[],
        &BTreeMap::new(),
        &BUDGET,
    )
    .unwrap();
    let verdicts: Vec<_> = report
        .functions
        .iter()
        .map(|f| (f.name.as_str(), f.verdict, f.complete))
        .collect();
    assert_eq!(
        verdicts,
        [
            ("f", Verdict::Unknown, [true, false]),
            ("g", Verdict::NoDifferenceSeen, [true, true])
        ]
    );
}

#[test]
fn several_copies_with_one_skipped_leave_that_side_incomplete() {
    let report = compare(
        "chip",
        &[input("rom", object(&[("f", &store(2))]))],
        &[
            input("lib", object(&[("f", &store(2))])),
            input("foreign", foreign()),
        ],
        &[],
        &window(),
        &[],
        &BTreeMap::new(),
        &BUDGET,
    )
    .unwrap();
    assert_eq!(report.functions[0].verdict, Verdict::SeveralCopies);
    assert_eq!(report.functions[0].complete, [true, false]);
}
