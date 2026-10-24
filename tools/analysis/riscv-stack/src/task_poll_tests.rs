//! A linked fixture of the static executor's direct task polls and the
//! boundary records its declared tasks export.
use super::*;

/// The executor's boundary with its tasks, as `oer-espressif-executor`
/// shapes it: a never-inlined poll per task type, and the record type that
/// names its instance. `BOUNDARY` and `INLINE` stand for the function's name
/// and its inlining.
const EXECUTOR_LIBRARY: &str = r#"
#![no_std]
pub mod task {
    pub trait Body { fn run(); }
    #[derive(Clone, Copy)]
    pub struct PollBoundary(pub fn() -> bool);
    #[INLINE]
    pub fn BOUNDARY<F: Body>() -> bool { F::run(); core::hint::black_box(false) }
}
"#;

/// Two tasks polled by index, as a declared task set's poll is, each with
/// its exported boundary record (`RECORDS`).
const EXECUTOR_PROGRAM: &str = r#"
#![no_std]
#![no_main]
use oer_espressif_executor::task::{Body, BOUNDARY};
struct First;
struct Second;
impl Body for First { #[inline(never)] fn run() { core::hint::black_box([1u8; 256]); } }
impl Body for Second { #[inline(never)] fn run() { core::hint::black_box([2u8; 32]); } }
RECORDS
#[inline(never)]
fn poll(index: u8) {
    match index {
        0 => { BOUNDARY::<First>(); }
        1 => { BOUNDARY::<Second>(); }
        _ => {}
    }
}
#[unsafe(no_mangle)] pub extern "C" fn _start() -> ! {
    loop { poll(core::hint::black_box(0)); poll(core::hint::black_box(1)); }
}
#[unsafe(no_mangle)] pub fn keep() {}
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
"#;

/// Each task's record, as `tasks!` exports it.
const RECORDS: &str = r#"
#[used]
#[unsafe(export_name = "__OER_TASK_POLL.image::first")]
static FIRST: oer_espressif_executor::task::PollBoundary =
    oer_espressif_executor::task::PollBoundary(BOUNDARY::<First>);
#[used]
#[unsafe(export_name = "__OER_TASK_POLL.image::second")]
static SECOND: oer_espressif_executor::task::PollBoundary =
    oer_espressif_executor::task::PollBoundary(BOUNDARY::<Second>);
"#;

/// The fixture with its boundary named `boundary`, inlined or not, and
/// with or without its records.
fn executor_image(boundary: &str, inline: &str, records: bool) -> Vec<u8> {
    compiled_named_library(
        &EXECUTOR_LIBRARY
            .replace("BOUNDARY", boundary)
            .replace("INLINE", inline),
        "2",
        &EXECUTOR_PROGRAM
            .replace("RECORDS", if records { RECORDS } else { "" })
            .replace("BOUNDARY", boundary),
        "oer_espressif_executor",
        "image",
    )
}

/// The task polls of `elf`, which links the executor's waker or not.
fn task_polls(elf: &[u8], analysis: &Analysis, executor: bool) -> Result<Resolutions> {
    with_facts(elf, |facts| {
        task_poll_resolutions(facts, analysis, executor, &mut ledger(), &mut || Ok(()))
    })
}

/// The addresses of the functions `elf` names `boundary::<…>`.
fn instances(elf: &[u8], boundary: &str) -> BTreeSet<u32> {
    let prefix = format!("oer_espressif_executor::task::{boundary}::<");
    functions(elf)
        .unwrap()
        .iter()
        .filter(|function| {
            function
                .names
                .iter()
                .any(|name| oer_analysis_elf::demangle(name).starts_with(&prefix))
        })
        .map(|function| function.address)
        .collect()
}

#[test]
fn the_direct_task_polls_are_the_cut_and_their_bound_assumes_nothing() {
    let elf = executor_image("poll_task", "inline(never)", true);
    let polls = instances(&elf, "poll_task");
    assert_eq!(polls.len(), 2);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let resolutions = task_polls(&elf, &analysis, true).unwrap();
    let sites = task_poll_sites(&resolutions);
    // Every task poll is a direct call of one recorded boundary, which is
    // the task's root.
    let called: BTreeSet<u32> = analysis
        .functions
        .values()
        .flat_map(|facts| &facts.transfers)
        .filter(|transfer| sites.contains(&transfer.site))
        .filter_map(|transfer| transfer.target)
        .collect();
    assert_eq!(called, polls);
    assert_eq!(resolutions.task_roots(), &polls);
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

#[test]
fn a_renamed_boundary_is_found_by_its_records() {
    let named = executor_image("poll_task", "inline(never)", true);
    let renamed = executor_image("renamed_poll_task", "inline(never)", true);
    let found = |elf: &[u8]| {
        let analysis = analyze(elf, &[], &[]).unwrap();
        let resolutions = task_polls(elf, &analysis, true).unwrap();
        (
            task_poll_sites(&resolutions).len(),
            resolutions.task_roots().len(),
        )
    };
    assert_eq!(found(&named), (2, 2));
    assert_eq!(found(&renamed), (2, 2), "no name decides a task poll");
}

#[test]
fn an_inlined_boundary_fails_the_analysis() {
    // The records keep an outlined instance, but the executor's polls no
    // longer call it: its tasks' code would count as the executor's.
    let elf = executor_image("poll_task", "inline(always)", true);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let error = task_polls(&elf, &analysis, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity, "{error}");
    assert!(error.to_string().contains("has no direct call"), "{error}");
}

#[test]
fn an_executor_without_boundary_records_fails_the_analysis() {
    let elf = executor_image("poll_task", "inline(never)", false);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    // An image that links the executor's waker polls tasks only through
    // recorded boundaries: none found is an error, never an empty set.
    let error = task_polls(&elf, &analysis, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity, "{error}");
    assert!(
        error.to_string().contains("records no task poll boundary"),
        "{error}"
    );
    // An image without the executor has no task polls to find.
    let resolutions = task_polls(&elf, &analysis, false).unwrap();
    assert!(task_poll_sites(&resolutions).is_empty());
    assert!(resolutions.task_roots().is_empty());
}
