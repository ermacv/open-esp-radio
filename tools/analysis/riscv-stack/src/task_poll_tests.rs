//! Linked fixtures for the distinction between no tasks and missing poll symbols.
use super::*;

const NO_TASK_PROGRAM: &str = r#"
#![no_std]
#![no_main]
use embassy_executor::raw::SyncExecutor;
#[unsafe(no_mangle)]
pub unsafe extern "C" fn _start(executor: &SyncExecutor) {
    unsafe { executor.poll(); }
}
#[unsafe(no_mangle)]
pub unsafe fn keep(callback: unsafe fn()) { unsafe { callback(); } }
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }
"#;

fn poll_names(elf: &[u8]) -> Vec<String> {
    functions(elf)
        .unwrap()
        .iter()
        .flat_map(|function| &function.names)
        .map(|name| oer_analysis_elf::demangle(name))
        .filter(|name| {
            (name.contains("TaskStorage<") && name.ends_with("::poll"))
                || name == "embassy_executor::raw::poll_exited"
        })
        .collect()
}

fn resolve(elf: &[u8], analysis: &Analysis, dwarf: &Dwarf, types: &TypeFacts) -> Resolutions {
    with_facts(elf, |image| {
        let names: Vec<_> = image
            .functions()
            .iter()
            .flat_map(|function| {
                function
                    .names
                    .iter()
                    .map(|name| (function.address, oer_analysis_elf::demangle(name)))
            })
            .collect();
        task_poll_resolutions(
            image,
            analysis,
            dwarf,
            types,
            names
                .iter()
                .map(|(address, name)| (*address, name.as_str())),
            &mut ledger(),
            &mut || Ok(()),
        )
    })
    .unwrap()
}

fn entry(elf: &[u8]) -> u32 {
    oer_analysis_elf::Elf::parse(elf)
        .unwrap()
        .symbol("_start")
        .unwrap()
        .address as u32
}

#[test]
fn no_task_polls_proves_only_the_dequeue_call_empty_and_the_bound_conditional() {
    let elf = executor_image(NO_TASK_PROGRAM);
    assert!(poll_names(&elf).is_empty());
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dwarf, types) = debug(&elf);
    let root = entry(&elf);
    let before = analysis
        .bound(root, &mut crate::tests::ledger(), &mut || Ok(()))
        .unwrap();
    assert_eq!(before.bytes, None);
    assert_eq!(before.unresolved.len(), 1);
    let resolutions = resolve(&elf, &analysis, &dwarf, &types);
    assert_eq!(resolutions.len(), 1, "{resolutions:?}");
    let (site, resolution) = resolutions.iter().next().unwrap();
    assert_eq!(site, before.unresolved[0].0);
    assert!(resolution.targets.is_empty());
    assert_eq!(resolution.facts, BTreeSet::from([Fact::NoTasks]));
    assert!(resolution.resolves());
    let bound = analysis
        .bound_with(root, &resolutions, &mut crate::tests::ledger(), &mut || {
            Ok(())
        })
        .unwrap();
    assert_eq!(bound.bytes, Some(before.partial));
    assert!(bound.unresolved.is_empty());
    assert_eq!(
        bound.assumptions,
        BTreeSet::from([Assumption::ExecutorInvariant])
    );
    let unrelated = oer_analysis_elf::Elf::parse(&elf)
        .unwrap()
        .symbol("keep")
        .unwrap()
        .address as u32;
    assert_eq!(
        analysis
            .bound_with(
                unrelated,
                &resolutions,
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .unwrap()
            .bytes,
        None
    );
    assert_eq!(
        dwarf
            .has_instance(|_| false, &mut Units(0))
            .unwrap_err()
            .code,
        ErrorCode::ResourceLimited
    );
}

#[test]
fn a_task_poll_with_a_lost_symbol_keeps_the_dequeue_call_unresolved() {
    let mut elf = executor_image(
        &EXECUTOR_PROGRAM.replace("E0.run(&FIRST); E1.run(&SECOND);", "E0.run(&FIRST);"),
    );
    let names = poll_names(&elf);
    assert_eq!(names.len(), 1, "{names:?}");
    // Lose only the symbol's recognizable spelling; retain its code and
    // unchanged linkage name in .debug_str, as after symbol merging/renaming.
    let at = {
        let file = oer_analysis_elf::Elf::parse(&elf).unwrap();
        let symbol = file
            .symbols()
            .find(|symbol| symbol.demangled() == names[0])
            .unwrap();
        let strings = file.section_by_name(".strtab").unwrap();
        strings.file_offset as usize
            + strings
                .data
                .windows(symbol.name.len())
                .position(|bytes| bytes == symbol.name.as_bytes())
                .unwrap()
    };
    elf[at] = b'?';
    assert!(poll_names(&elf).is_empty());
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dwarf, types) = debug(&elf);
    assert!(!dwarf.instances(&[&names[0]])[&names[0]].is_empty());
    let resolutions = resolve(&elf, &analysis, &dwarf, &types);
    assert!(resolutions.is_empty(), "{resolutions:?}");
    assert_eq!(
        analysis
            .bound_with(
                entry(&elf),
                &resolutions,
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .unwrap()
            .bytes,
        None
    );
}

#[test]
fn an_inlined_task_poll_or_exited_poll_keeps_the_dequeue_call_unresolved() {
    for exited in [false, true] {
        let (library, call, name) = if exited {
            (
                EXECUTOR_LIBRARY.replace(
                    "pub mod raw {",
                    "pub mod raw { #[inline(always)] pub unsafe fn poll_exited() { core::hint::black_box(7u32); }",
                ),
                "embassy_executor::raw::poll_exited()",
                "embassy_executor::raw::poll_exited",
            )
        } else {
            (
                EXECUTOR_LIBRARY
                    .replace(
                        "#[inline(never)]\n        pub unsafe fn poll",
                        "#[inline(always)]\n        pub unsafe fn poll",
                    )
                    .replace(
                        "{ F::poll(); }",
                        "{ core::hint::black_box(9u32); F::poll(); }",
                    ),
                "embassy_executor::raw::TaskStorage::<Only>::poll(core::ptr::null())",
                "<embassy_executor::raw::TaskStorage<oer_espressif_executor::Only>>::poll",
            )
        };
        let program = NO_TASK_PROGRAM.replace(
            "pub unsafe fn keep(callback: unsafe fn()) { unsafe { callback(); } }",
            &format!("pub unsafe fn keep() {{ unsafe {{ {call}; }} }}"),
        ) + "\nstruct Only; impl embassy_executor::raw::Future for Only { #[inline(never)] fn poll() { core::hint::black_box(3u32); } }";
        let elf = compiled_named_library(
            &library,
            "2",
            &program,
            "embassy_executor",
            "oer_espressif_executor",
        );
        assert!(poll_names(&elf).is_empty(), "{:?}", poll_names(&elf));
        let analysis = analyze(&elf, &[], &[]).unwrap();
        let (dwarf, types) = debug(&elf);
        assert!(!dwarf.instances(&[name])[name].is_empty(), "{name}");
        let resolutions = resolve(&elf, &analysis, &dwarf, &types);
        assert!(resolutions.is_empty(), "{resolutions:?}");
        assert_eq!(
            analysis
                .bound_with(
                    entry(&elf),
                    &resolutions,
                    &mut crate::tests::ledger(),
                    &mut || Ok(())
                )
                .unwrap()
                .bytes,
            None
        );
    }
}

#[test]
fn one_task_keeps_its_existing_task_polls_fact_and_candidate() {
    let elf = executor_image(
        &EXECUTOR_PROGRAM.replace("E0.run(&FIRST); E1.run(&SECOND);", "E0.run(&FIRST);"),
    );
    let names = poll_names(&elf);
    assert_eq!(names.len(), 1, "{names:?}");
    let analysis = analyze(&elf, &[], &[]).unwrap();
    let (dwarf, types) = debug(&elf);
    let resolutions = resolve(&elf, &analysis, &dwarf, &types);
    assert_eq!(resolutions.len(), 1, "{resolutions:?}");
    let resolution = resolutions.iter().next().unwrap().1;
    assert_eq!(resolution.facts, BTreeSet::from([Fact::TaskPolls]));
    let poll = functions(&elf)
        .unwrap()
        .into_iter()
        .find(|function| {
            function
                .names
                .iter()
                .any(|name| oer_analysis_elf::demangle(name) == names[0])
        })
        .unwrap()
        .address;
    assert_eq!(resolution.targets, BTreeSet::from([poll]));
    assert!(!Fact::TaskPolls.proves_empty());
    assert!(
        analysis
            .bound_with(
                entry(&elf),
                &resolutions,
                &mut crate::tests::ledger(),
                &mut || Ok(())
            )
            .unwrap()
            .bytes
            .is_some()
    );
}
