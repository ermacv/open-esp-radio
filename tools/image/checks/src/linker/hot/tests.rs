use super::*;
use crate::linker::strict::Placement;

fn function(name: &str, address: u32, size: u32, section: &str) -> ProfiledFunction {
    ProfiledFunction {
        name: name.into(),
        address,
        size,
        section: section.into(),
    }
}

#[test]
fn a_profile_lists_the_densest_cached_functions_until_the_budget() {
    let functions = [
        function("step", 0x5000_0000, 0x100, ".text"),
        function("ingest", 0x5000_0100, 0x80, ".text"),
        function("huge", 0x5000_0200, 0x400, ".text"),
        function("handler", 0x2f00_0000, 0x40, ".isr.text"),
        function("cold", 0x5000_0600, 0x40, ".text"),
        function("lonely", 0x5000_0700, 0x10, ".text"),
        function("tiny", 0x5000_0800, 0x80, ".text"),
    ];
    let pcs = [
        // `huge` holds the most samples, but `ingest` and `tiny` hold more
        // per byte (they tie, then go in by name), then `step`; `huge` does
        // not fit what is left. Samples in SRAM code, between functions,
        // past a function's end and a function's only sample count for
        // nothing.
        0x5000_0200,
        0x5000_0204,
        0x5000_0208,
        0x5000_020c,
        0x5000_0000,
        0x5000_0010,
        0x5000_0100,
        0x5000_0110,
        0x5000_0800,
        0x5000_0810,
        0x5000_0640,
        0x5000_0700,
        0x2f00_0000,
        0x2f00_0004,
        0x2f00_0008,
        0x4000_0000,
    ];
    let list = HotCode::from_samples(&functions, pcs, 0x400);
    assert_eq!(
        list,
        HotCode {
            budget_bytes: 0x400,
            functions: vec!["ingest".into(), "tiny".into(), "step".into()],
        }
    );
}

#[test]
fn a_function_an_earlier_list_moved_ranks_on_its_samples_and_handler_code_does_not() {
    let functions = [
        function("step", 0x5000_0000, 0x100, ".text"),
        function("moved", 0x2f01_0000, 0x40, ".hot.text"),
        function("handler", 0x2f00_0000, 0x40, ".isr.text"),
    ];
    let pcs = [
        0x2f01_0000,
        0x2f01_0010,
        0x2f01_0020,
        0x5000_0000,
        0x5000_0010,
        0x2f00_0000,
        0x2f00_0004,
        0x2f00_0008,
    ];
    let list = HotCode::from_samples(&functions, pcs, 0x400);
    assert_eq!(list.functions, ["moved", "step"]);
}

#[test]
fn a_list_reads_back_from_the_file_it_renders() {
    let list = HotCode {
        budget_bytes: 4096,
        functions: vec!["<a::B>::c::<1>".into(), "memcpy".into()],
    };
    let text = list.render("from a profile\nof one level").unwrap();
    assert!(text.starts_with("# from a profile\n# of one level\n"));
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("hot-code.toml");
    std::fs::write(&path, text).unwrap();
    let mut inputs = Inputs::tracked();
    assert_eq!(HotCode::read(&path, &mut inputs).unwrap(), list);
    std::fs::write(&path, "functions = []\nunknown = 1\n").unwrap();
    assert!(HotCode::read(&path, &mut Inputs::tracked()).is_err());
}

#[test]
fn rust_symbols_share_one_name_without_hashes() {
    assert_eq!(
        key(&[
            "_ZN4core3fmt5write17h0123456789abcdefE".into(),
            "_ZN4core3fmt5write17hfedcba9876543210E".into()
        ]),
        Some("core::fmt::write".into())
    );
    assert_eq!(key(&[]), None);
}

fn section(name: &str, index: usize, size: u64, symbols: &[&str]) -> InputSection {
    InputSection {
        file: "image.o".into(),
        index,
        name: name.into(),
        size,
        symbols: symbols.iter().map(|symbol| (*symbol).into()).collect(),
    }
}

fn candidate(key: &str, symbol: &str, in_sram: bool) -> Candidate {
    Candidate {
        key: key.into(),
        symbols: vec![symbol.into()],
        size: 8,
        in_sram,
    }
}

#[test]
fn the_selection_moves_what_fits_and_reports_every_entry_it_leaves() {
    let sections = [
        section(".text.step", 1, 0x100, &["s_step", "s_step_alias"]),
        section(".text.ingest", 2, 0x80, &["s_ingest"]),
        section(".text.strict", 3, 0x20, &["s_strict"]),
        section(".text.big", 4, 0x400, &["s_big"]),
        section(".text.twice", 5, 0x10, &["s_twice"]),
        section(".text.twice2", 6, 0x10, &["s_twice"]),
    ];
    let candidates = [
        candidate("step", "s_step", false),
        candidate("ingest", "s_ingest", false),
        candidate("strict", "s_strict", false),
        candidate("big", "s_big", false),
        candidate("twice", "s_twice", false),
        candidate("isr", "s_isr", true),
    ];
    let list = HotCode {
        budget_bytes: 0x1000,
        functions: ["step", "absent", "isr", "strict", "big", "twice", "ingest"]
            .map(String::from)
            .to_vec(),
    };
    let strict = BTreeSet::from([("image.o".to_owned(), ".text.strict".to_owned())]);
    // The free SRAM, smaller than the budget, bounds the selection: `big`
    // does not fit after `step`, `ingest` still does.
    let selection = select(&list, &candidates, &sections, &strict, 0x200);
    let placed: Vec<_> = selection
        .placed
        .iter()
        .map(|function| (function.name.as_str(), function.section_bytes))
        .collect();
    assert_eq!(placed, [("step", 0x100), ("ingest", 0x80)]);
    assert_eq!(selection.placed[0].other_symbols, ["s_step_alias"]);
    let skipped: Vec<_> = selection
        .skipped
        .iter()
        .map(|skipped| (skipped.name.as_str(), skipped.reason.clone()))
        .collect();
    assert_eq!(
        skipped,
        [
            ("absent", SkipReason::Unmatched),
            ("isr", SkipReason::InSram),
            ("strict", SkipReason::Strict),
            ("big", SkipReason::NoRoom),
            ("twice", SkipReason::NoSingleSection),
        ]
    );
    // The budget bounds it too, and the fragment names each section once.
    let tight = HotCode {
        budget_bytes: 0x100,
        ..list
    };
    let selection = select(&tight, &candidates, &sections, &strict, 0x1000);
    assert_eq!(selection.placed.len(), 1);
    let placement = Placement {
        hot: selection.placed,
        hot_skipped: selection.skipped,
        ..Placement::default()
    };
    assert_eq!(placement.hot_fragment(), "\"image.o\"(\".text.step\");\n");
    assert_eq!(placement.fragment(), "");
    assert!(
        placement
            .render()
            .contains("hot code placed by the image linker: 1 functions, 256 section bytes")
    );
}
