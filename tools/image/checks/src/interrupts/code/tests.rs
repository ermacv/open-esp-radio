use super::*;
use crate::interrupts::{Required, handler_code, placement::tests::facts, tests::partial};
use oer_analysis_riscv_stack::{Fact, Transfer, TransferKind, TrapEntry};

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
fn fixture(
    strict: Location,
    task: Location,
    priority: u32,
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
    level.bound = analysis.bound_with(root, &resolutions).unwrap();
    stacks.harts[0].levels = vec![level];
    stacks.harts[0].exception.bound = analysis.bound_with(exception, &resolutions).unwrap();
    let own: Vec<_> = analysis
        .functions
        .values()
        .filter(|function| function.function.address != shared || strict != Location::Rom)
        .map(|function| function.function.clone())
        .collect();
    let inputs = Stacks {
        table: &[],
        cores: &[0],
        always: &[priority],
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
    )
    .unwrap();
    let full = &stacks.harts[0].levels[0].bound.reached;
    stacks.handler_code = handler_code(&analysis, full, &stacks.names, memory);
    stacks.names.insert(shared, "sample::shared".into());
    stacks.names.insert(task, "sample::task".into());
    (stacks, shared, task)
}

#[test]
fn a_shared_executor_and_task_function_stays_strict_and_psram_fails() {
    let (stacks, shared, task) = fixture(Location::Psram, Location::Psram, 2);
    let context = &stacks.code_placement.contexts[0];
    assert!(context.strict.contains(&shared));
    assert!(!context.tasks.contains(&shared));
    assert!(context.tasks.contains(&task));
    assert_eq!(stacks.code_placement.functions[&shared].role, Role::Strict);
    assert_eq!(context.task_roots, BTreeSet::from([task]));
    for policy in [Required::Proven, Required::Partial] {
        let error = stacks.check(policy).unwrap_err().to_string();
        assert!(error.contains("interrupt handler code runs sample::shared from cached memory, reached from hart 0 P2"), "{error}");
        assert!(error.contains("strict handler code in cached memory: 1 functions, 16 bytes"));
    }
}

#[test]
fn p2_task_psram_and_strict_rom_are_allowed_but_task_flash_is_not() {
    for strict in [Location::Sram, Location::Rom] {
        let (stacks, shared, task) = fixture(strict, Location::Psram, 2);
        assert_eq!(stacks.code_placement.functions[&shared].location, strict);
        assert_eq!(stacks.code_placement.functions[&task].role, Role::Task);
        let warnings = stacks.check(Required::Proven).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("executor invariant"));
        let report = stacks.render();
        assert!(report.contains("strict handler code in cached memory: 0 functions, 0 bytes"));
        assert!(
            report.contains("P2 task code in PSRAM: 1 functions, 32 bytes; tasks: sample::task")
        );
    }
    let (stacks, _, _) = fixture(Location::Sram, Location::Flash, 2);
    let error = stacks.check(Required::Proven).unwrap_err().to_string();
    assert!(
        error.contains("P2 task code runs sample::task from cached flash, reached from hart 0 P2")
    );
}

#[test]
fn another_priority_cannot_borrow_p2s_psram_permission() {
    let (stacks, _, task) = fixture(Location::Sram, Location::Psram, 1);
    assert!(stacks.code_placement.contexts[0].strict.contains(&task));
    assert!(stacks.code_placement.contexts[0].tasks.is_empty());
    assert!(
        stacks
            .check(Required::Proven)
            .unwrap_err()
            .to_string()
            .contains("reached from hart 0 P1")
    );
}

#[test]
fn unknown_image_memory_is_not_rom_or_an_admitted_task_region() {
    let (stacks, _, _) = fixture(Location::Other, Location::Psram, 2);
    assert!(
        stacks
            .check(Required::Proven)
            .unwrap_err()
            .to_string()
            .contains("outside SRAM/ROM")
    );
    let (stacks, _, _) = fixture(Location::Sram, Location::Other, 2);
    assert!(
        stacks
            .check(Required::Proven)
            .unwrap_err()
            .to_string()
            .contains("outside SRAM/ROM/PSRAM")
    );
}
