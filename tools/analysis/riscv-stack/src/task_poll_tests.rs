//! A linked fixture of the static executor's direct task polls.
use super::*;

/// The executor's boundary with its tasks, as `oer-espressif-executor`
/// names it.
const EXECUTOR_LIBRARY: &str = r#"
#![no_std]
pub mod task {
    pub trait Body { fn run(); }
    #[inline(never)]
    pub fn poll_task<F: Body>() -> bool { F::run(); core::hint::black_box(false) }
}
"#;

/// Two tasks polled by index, as a declared task set's poll is.
const EXECUTOR_PROGRAM: &str = r#"
#![no_std]
#![no_main]
use oer_espressif_executor::task::{Body, poll_task};
struct First;
struct Second;
impl Body for First { #[inline(never)] fn run() { core::hint::black_box([1u8; 256]); } }
impl Body for Second { #[inline(never)] fn run() { core::hint::black_box([2u8; 32]); } }
#[inline(never)]
fn poll(index: u8) {
    match index {
        0 => { poll_task::<First>(); }
        1 => { poll_task::<Second>(); }
        _ => {}
    }
}
#[unsafe(no_mangle)] pub extern "C" fn _start() -> ! {
    loop { poll(core::hint::black_box(0)); poll(core::hint::black_box(1)); }
}
#[unsafe(no_mangle)] pub fn keep() {}
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
"#;

fn named(elf: &[u8]) -> Vec<(u32, String)> {
    functions(elf)
        .unwrap()
        .iter()
        .flat_map(|function| {
            function
                .names
                .iter()
                .map(|name| (function.address, oer_analysis_elf::demangle(name)))
        })
        .collect()
}

#[test]
fn the_direct_task_polls_are_the_cut_and_their_bound_assumes_nothing() {
    let elf = compiled_named_library(
        EXECUTOR_LIBRARY,
        "2",
        EXECUTOR_PROGRAM,
        "oer_espressif_executor",
        "image",
    );
    let names = named(&elf);
    let polls: BTreeSet<u32> = names
        .iter()
        .filter(|(_, name)| is_task_poll(name))
        .map(|(address, _)| *address)
        .collect();
    assert_eq!(polls.len(), 2, "{names:?}");
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let resolutions = task_poll_resolutions(
        &analysis,
        names
            .iter()
            .map(|(address, name)| (*address, name.as_str())),
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    let sites = task_poll_sites(&resolutions);
    // Every task poll is a direct call of one boundary instance.
    let called: BTreeSet<u32> = analysis
        .functions
        .values()
        .flat_map(|facts| &facts.transfers)
        .filter(|transfer| sites.contains(&transfer.site))
        .filter_map(|transfer| transfer.target)
        .collect();
    assert_eq!(called, polls);
    assert!(resolutions.is_empty(), "no indirect site needs a fact");

    let root = oer_analysis_elf::Elf::parse(&elf)
        .unwrap()
        .symbol("_start")
        .unwrap()
        .address as u32;
    let bound = analysis
        .bound_with(root, &resolutions, &mut ledger(), &mut || Ok(()))
        .unwrap();
    assert!(bound.bytes.is_some(), "{bound:?}");
    assert!(bound.unresolved.is_empty());
    assert!(bound.assumptions.is_empty(), "{:?}", bound.assumptions);
    for poll in &polls {
        assert!(bound.reached.contains(poll));
    }

    // The executor's own code ends at the polls.
    let strict = analysis
        .reached_cut(root, &resolutions, &sites, &mut ledger(), &mut || Ok(()))
        .unwrap();
    assert!(strict.is_disjoint(&polls), "{strict:?}");
    assert!(strict.contains(&root));
}
