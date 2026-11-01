//! A linked fixture of the static executor's direct task polls, each
//! boundary marked by its zero-sized `PollBoundary` parameter.
use super::*;

/// The executor's boundary with its tasks, as `oer-espressif-executor`
/// shapes it: a never-inlined poll per task type whose first parameter is
/// the mark type. `BOUNDARY`, `INLINE` and `MARK` stand for the function's
/// name, its inlining and its first parameter's type.
const EXECUTOR_LIBRARY: &str = r#"
#![no_std]
pub mod task {
    pub trait Body { fn run(); }
    #[derive(Clone, Copy)]
    pub struct PollBoundary;
    #[derive(Clone, Copy)]
    pub struct Unmarked;
    #[INLINE]
    pub fn BOUNDARY<F: Body>(_: MARK) -> bool { F::run(); core::hint::black_box(false) }
    pub fn poll<F: Body>() -> bool { BOUNDARY::<F>(MARK) }
}
"#;

/// Two tasks polled by index, as a declared task set's poll is, and a third
/// whose poll nothing calls (`UNPOLLED`) and whose boundary `KEEP` may take
/// the address of.
const EXECUTOR_PROGRAM: &str = r#"
#![no_std]
#![no_main]
use oer_espressif_executor::task::{Body, poll};
struct First;
struct Second;
struct Third;
impl Body for First { #[inline(never)] fn run() { core::hint::black_box([1u8; 256]); } }
impl Body for Second { #[inline(never)] fn run() { core::hint::black_box([2u8; 32]); } }
impl Body for Third { #[inline(never)] fn run() { core::hint::black_box([3u8; 64]); } }
#[inline(never)]
fn polls(index: u8) {
    match index {
        0 => { poll::<First>(); }
        1 => { poll::<Second>(); }
        _ => {}
    }
}
#[inline(never)]
#[allow(dead_code)]
fn unpolled() { poll::<Third>(); }
#[unsafe(no_mangle)] pub extern "C" fn _start() -> ! {
    loop { polls(core::hint::black_box(0)); polls(core::hint::black_box(1)); }
}
#[unsafe(no_mangle)] pub fn keep() { KEEP }
#[panic_handler] fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
"#;

/// The boundary's address taken, never called.
const TAKEN: &str = "core::hint::black_box(oer_espressif_executor::task::BOUNDARY::<Third> \
                     as fn(oer_espressif_executor::task::MARK) -> bool);";

/// The fixture with its boundary named `boundary`, inlined or not, marked
/// by `mark`, and with the third task's boundary address taken or not.
fn executor_image(boundary: &str, inline: &str, mark: &str, taken: bool) -> Vec<u8> {
    let keep = if taken { TAKEN } else { "" };
    compiled_named_library(
        &EXECUTOR_LIBRARY
            .replace("BOUNDARY", boundary)
            .replace("INLINE", inline)
            .replace("MARK", mark),
        "2",
        &EXECUTOR_PROGRAM
            .replace("KEEP", keep)
            .replace("BOUNDARY", boundary)
            .replace("MARK", mark),
        "oer_espressif_executor",
        "image",
    )
}

/// The fixture as the executor builds it: `poll_task`, never inlined, marked.
fn marked_image() -> Vec<u8> {
    executor_image("poll_task", "inline(never)", "PollBoundary", false)
}

/// The task polls of `elf`, which links the executor's waker or not.
fn task_polls(elf: &[u8], analysis: &Analysis, executor: bool) -> Result<Resolutions> {
    let (dwarf, _) = debug(elf);
    task_poll_resolutions(analysis, &dwarf, executor, &mut ledger(), &mut || Ok(()))
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
    let elf = marked_image();
    let polls = instances(&elf, "poll_task");
    // The unpolled task's boundary is not linked.
    assert_eq!(polls.len(), 2);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let resolutions = task_polls(&elf, &analysis, true).unwrap();
    let sites = task_poll_sites(&resolutions);
    // Every task poll is a direct call of one marked boundary, which is the
    // task's root.
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
fn a_renamed_boundary_is_found_by_its_mark() {
    let renamed = executor_image("renamed_poll_task", "inline(never)", "PollBoundary", false);
    let found = |elf: &[u8]| {
        let analysis = analyze(elf, &[], &[]).unwrap();
        let resolutions = task_polls(elf, &analysis, true).unwrap();
        (
            task_poll_sites(&resolutions).len(),
            resolutions.task_roots().len(),
        )
    };
    assert_eq!(found(&marked_image()), (2, 2));
    assert_eq!(found(&renamed), (2, 2), "no name decides a task poll");
}

#[test]
fn an_inlined_boundary_fails_the_analysis() {
    // The executor's polls no longer call a boundary: its tasks' code would
    // count as the executor's.
    let elf = executor_image("poll_task", "inline(always)", "PollBoundary", false);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let error = task_polls(&elf, &analysis, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity, "{error}");
    assert!(error.to_string().contains("is inlined"), "{error}");
}

#[test]
fn a_boundary_taken_but_never_called_fails_the_analysis() {
    let elf = executor_image("poll_task", "inline(never)", "PollBoundary", true);
    assert_eq!(instances(&elf, "poll_task").len(), 3);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let error = task_polls(&elf, &analysis, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity, "{error}");
    assert!(error.to_string().contains("has no direct call"), "{error}");
}

#[test]
fn an_executor_without_a_marked_boundary_fails_the_analysis() {
    let elf = executor_image("poll_task", "inline(never)", "Unmarked", false);
    let analysis = analyze(&elf, &[], &[]).unwrap();
    // An image that links the executor's waker polls tasks only through
    // marked boundaries: none found is an error, never an empty set.
    let error = task_polls(&elf, &analysis, true).unwrap_err();
    assert_eq!(error.code, ErrorCode::Integrity, "{error}");
    assert!(
        error.to_string().contains("marks no task poll boundary"),
        "{error}"
    );
    // An image without the executor has no task polls to find.
    let resolutions = task_polls(&elf, &analysis, false).unwrap();
    assert!(task_poll_sites(&resolutions).is_empty());
    assert!(resolutions.task_roots().is_empty());
}
