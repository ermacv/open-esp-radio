use super::*;
use crate::interrupts::{
    Required, handler_code,
    placement::tests::facts,
    tests::{ledger, partial},
};
use oer_analysis_riscv_stack::{Fact, TableEntry, Transfer, TransferKind, TrapEntry};

fn transfer(site: u32, target: Option<u32>) -> Transfer {
    Transfer {
        site,
        target,
        kind: TransferKind::Call,
        source: None,
        table: None,
        load: None,
    }
}

/// A real synthetic analysis: root calls dequeue and shared directly;
/// dequeue polls task; task also calls shared. Exception is independent.
/// The table's one entry, on hart 0 at `priority`, is of `class`.
pub(in crate::interrupts) fn fixture(
    strict: Location,
    task: Location,
    priority: u32,
    class: Class,
) -> (super::super::InterruptStacks, u32, u32) {
    let mut stacks = partial(0);
    stacks.harts[0].bytes = Some(0);
    let memory = &stacks.contract.memory;
    let root = memory.sram.origin + 0x1000;
    let dequeue = root + 0x40;
    let exception = root + 0x80;
    let address = |location, offset| match location {
        Location::Sram => memory.sram.origin + offset,
        Location::Psram => memory.psram.unwrap().origin + offset,
        Location::Flash => memory.flash_xip.origin + offset,
        Location::Rom => 0x4000_0000 + offset,
        Location::Other => 0x1000 + offset,
    };
    let shared = address(strict, 0x2000);
    let task = address(task, 0x3000);
    let mut root_facts = facts(root, 32);
    root_facts.transfers = vec![
        transfer(root, Some(dequeue)),
        transfer(root + 4, Some(shared)),
    ];
    let mut dequeue_facts = facts(dequeue, 32);
    dequeue_facts.transfers = vec![transfer(dequeue, None)];
    let mut task_facts = facts(task, 32);
    task_facts.transfers = vec![transfer(task, Some(shared))];
    let analysis = Analysis {
        traps: Default::default(),
        functions: BTreeMap::from([
            (root, root_facts),
            (dequeue, dequeue_facts),
            (exception, facts(exception, 4)),
            (shared, facts(shared, 16)),
            (task, task_facts),
        ]),
        code: Vec::new(),
        summaries: BTreeSet::new(),
    };
    let mut resolutions = Resolutions::new();
    resolutions.add(dequeue, Fact::TaskPolls, [task]);
    let mut level = stacks.harts[0].exception.clone();
    level.level = priority;
    level.bound = analysis
        .bound_with(root, &resolutions, &mut ledger(), &mut || Ok(()))
        .unwrap();
    stacks.harts[0].levels = vec![level];
    stacks.harts[0].exception.bound = analysis
        .bound_with(exception, &resolutions, &mut ledger(), &mut || Ok(()))
        .unwrap();
    let own: Vec<_> = analysis
        .functions
        .values()
        .filter(|function| function.function.address != shared || strict != Location::Rom)
        .map(|function| function.function.clone())
        .collect();
    stacks.table = vec![TableEntry {
        source: 7,
        level: priority,
        class,
        core: 0,
        handler: Some(root),
    }];
    let inputs = Stacks {
        table: &stacks.table,
        cores: &[0],
        vectors: &[],
        exception: TrapEntry {
            entry: exception,
            handler: exception,
            frame: 0,
        },
        sources: 0,
        resolutions: &resolutions,
    };
    stacks.code_placement = CodePlacement::of(
        &analysis,
        &own,
        &stacks.harts,
        &inputs,
        BTreeSet::new(),
        memory,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap();
    let full = &stacks.harts[0].levels[0].bound.reached;
    stacks.handler_code = handler_code(&analysis, full, &stacks.names, memory);
    stacks.names.insert(shared, "sample::shared".into());
    stacks.names.insert(task, "sample::task".into());
    stacks.frames = analysis
        .functions
        .iter()
        .map(|(&address, facts)| (address, facts.frame))
        .collect();
    (stacks, shared, task)
}

#[test]
fn a_shared_executor_and_task_function_stays_strict_and_psram_fails() {
    let (stacks, shared, task) = fixture(Location::Psram, Location::Psram, 2, Class::Executor);
    let context = &stacks.code_placement.contexts[0];
    assert!(context.strict.contains(&shared));
    assert!(!context.tasks.contains(&shared));
    assert!(context.tasks.contains(&task));
    assert_eq!(stacks.code_placement.functions[&shared].role, Role::Strict);
    assert_eq!(context.task_roots, BTreeSet::from([task]));
    let error = stacks.check(Required::Product).unwrap_err().to_string();
    assert!(
        error.contains(
            "interrupt handler code runs sample::shared from cached memory, reached from hart 0 P2 (executor)"
        ),
        "{error}"
    );
    assert!(error.contains("strict handler code in cached memory: 1 functions, 16 bytes"));
    let warnings = stacks.check(Required::Diagnostic).unwrap();
    let placement: Vec<_> = warnings
        .iter()
        .filter(|w| w.contains("reported only"))
        .collect();
    assert_eq!(
        placement,
        [
            "hart 0 P2 (executor): 1 strict functions, 16 bytes in cached memory (diagnostic image, reported only); see interrupt-placement.txt"
        ]
    );
}

#[test]
fn p2_task_psram_and_strict_rom_are_allowed_but_task_flash_is_not() {
    for strict in [Location::Sram, Location::Rom] {
        let (stacks, shared, task) = fixture(strict, Location::Psram, 2, Class::Executor);
        assert_eq!(stacks.code_placement.functions[&shared].location, strict);
        assert_eq!(stacks.code_placement.functions[&task].role, Role::Task);
        let warnings = stacks.check(Required::Product).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("executor invariant"));
        let report = stacks.render();
        assert!(report.contains("strict handler code in cached memory: 0 functions, 0 bytes"));
        assert!(
            report.contains(
                "executor task code in PSRAM: 1 functions, 32 bytes; tasks: sample::task"
            )
        );
    }
    let (stacks, _, _) = fixture(Location::Sram, Location::Flash, 2, Class::Executor);
    let error = stacks.check(Required::Product).unwrap_err().to_string();
    assert!(error.contains(
        "executor task code runs sample::task from cached flash, reached from hart 0 P2 (executor)"
    ));
    let warnings = stacks.check(Required::Diagnostic).unwrap();
    assert!(warnings.iter().any(|w| w == "hart 0 P2 (executor): 1 executor task functions, 32 bytes in cached flash (diagnostic image, reported only); see interrupt-placement.txt"), "{warnings:?}");
}

#[test]
fn a_hardware_level_cannot_borrow_the_executor_s_psram_permission() {
    // The class, not the level number, grants the permission: P2 without
    // an executor is a hardware level, P1 with one an executor's.
    for priority in [1, 2] {
        let (stacks, _, task) = fixture(Location::Sram, Location::Psram, priority, Class::Hardware);
        assert!(stacks.code_placement.contexts[0].strict.contains(&task));
        assert!(stacks.code_placement.contexts[0].tasks.is_empty());
        let error = stacks.check(Required::Product).unwrap_err().to_string();
        assert!(
            error.contains(&format!(
                "interrupt handler code runs sample::task from cached memory, reached from \
                 hart 0 P{priority} (hardware)"
            )),
            "{error}"
        );
    }
    let (stacks, _, task) = fixture(Location::Sram, Location::Psram, 1, Class::Executor);
    assert!(stacks.code_placement.contexts[0].tasks.contains(&task));
    assert!(stacks.check(Required::Product).is_ok());
}

#[test]
fn unknown_image_memory_is_not_rom_or_an_admitted_task_region() {
    let (stacks, _, _) = fixture(Location::Other, Location::Psram, 2, Class::Executor);
    assert!(
        stacks
            .check(Required::Product)
            .unwrap_err()
            .to_string()
            .contains("outside SRAM/ROM")
    );
    let warnings = stacks.check(Required::Diagnostic).unwrap();
    assert!(warnings.iter().any(|w| w.contains("hart 0 P2 (executor): 1 strict functions, 16 bytes outside SRAM/ROM (diagnostic image, reported only); see interrupt-placement.txt")), "{warnings:?}");
    let (stacks, _, _) = fixture(Location::Sram, Location::Other, 2, Class::Executor);
    assert!(
        stacks
            .check(Required::Product)
            .unwrap_err()
            .to_string()
            .contains("outside SRAM/ROM/PSRAM")
    );
    let warnings = stacks.check(Required::Diagnostic).unwrap();
    assert!(warnings.iter().any(|w| w.contains("hart 0 P2 (executor): 1 executor task functions, 32 bytes outside SRAM/ROM/PSRAM (diagnostic image, reported only); see interrupt-placement.txt")), "{warnings:?}");
}

#[test]
fn diagnostic_warnings_group_functions_by_violation_and_context() {
    let (mut stacks, shared, _) = fixture(Location::Psram, Location::Flash, 2, Class::Executor);
    stacks.code_placement.functions.insert(
        shared + 0x40,
        PlacedFunction {
            size: 24,
            role: Role::Strict,
            location: Location::Flash,
        },
    );
    stacks.code_placement.contexts[0]
        .strict
        .insert(shared + 0x40);
    let mut other_context = ContextCode {
        hart: 1,
        level: 1,
        root: 0,
        class: Some(Class::Hardware),
        panics: Vec::new(),
        strict: BTreeSet::from([shared]),
        tasks: BTreeSet::new(),
        task_roots: BTreeSet::new(),
        cached_paths: BTreeMap::new(),
    };
    // A duplicate function in a context contributes once; the same function
    // reached from another context belongs in that context's warning too.
    other_context.strict.insert(shared);
    stacks.code_placement.contexts.push(other_context);
    let warnings = stacks.check(Required::Diagnostic).unwrap();
    let placement: Vec<_> = warnings
        .iter()
        .filter(|w| w.contains("reported only"))
        .map(String::as_str)
        .collect();
    assert_eq!(
        placement,
        [
            "hart 0 P2 (executor): 2 strict functions, 40 bytes in cached memory (diagnostic image, reported only); see interrupt-placement.txt",
            "hart 0 P2 (executor): 1 executor task functions, 32 bytes in cached flash (diagnostic image, reported only); see interrupt-placement.txt",
            "hart 1 P1 (hardware): 1 strict functions, 16 bytes in cached memory (diagnostic image, reported only); see interrupt-placement.txt",
        ]
    );
}
