use super::*;
use crate::interrupts::{
    Required, handler_code,
    placement::tests::facts,
    tests::{ledger, partial},
};
use oer_image_stack::{TableEntry, Transfer, TransferKind, TrapEntry};

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

/// The first handler has the worst stack bound; the other is independent.
/// Used by the linker's compiler-driven relocation regression.
pub(crate) fn same_level_handler_placement(handlers: [Function; 2]) -> CodePlacement {
    let mut stacks = partial(0);
    let exception = stacks.contract.memory.sram.origin() + 0x1000;
    let mut functions = BTreeMap::from([(exception, facts(exception, 4))]);
    for (index, handler) in handlers.iter().enumerate() {
        let mut value = facts(handler.address, handler.size);
        value.function = handler.clone();
        value.frame = Some(if index == 0 { 32 } else { 0 });
        functions.insert(handler.address, value);
    }
    let analysis = Analysis {
        traps: Default::default(),
        functions,
        code: Vec::new(),
        summaries: BTreeSet::new(),
    };
    let resolutions = Resolutions::new();
    let mut level = stacks.harts[0].exception.clone();
    level.level = 1;
    level.bound = analysis
        .bound_with(
            handlers[0].address,
            &resolutions,
            &mut ledger(),
            &mut || Ok(()),
        )
        .unwrap();
    stacks.harts[0].levels = vec![level];
    stacks.harts[0].exception.bound = analysis
        .bound_with(exception, &resolutions, &mut ledger(), &mut || Ok(()))
        .unwrap();
    let table: Vec<_> = handlers
        .iter()
        .enumerate()
        .map(|(index, handler)| TableEntry {
            source: index as u32,
            line: 16 + index as u32,
            level: 1,
            class: Class::Hardware,
            core: 0,
            handler: Some(handler.address),
        })
        .collect();
    let inputs = Stacks {
        table: &table,
        cores: &[0],
        vectors: &[],
        exception: TrapEntry {
            entry: exception,
            handler: exception,
            frame: 0,
        },
        exception_copies: 1,
        resolutions: &resolutions,
    };
    CodePlacement::of(
        &analysis,
        &handlers,
        &stacks.harts,
        &inputs,
        BTreeSet::new(),
        &stacks.contract.memory,
        &mut ledger(),
        &mut || Ok(()),
    )
    .unwrap()
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
    let root = memory.sram.origin() + 0x1000;
    let dequeue = root + 0x40;
    let exception = root + 0x80;
    let address = |location, offset| match location {
        Location::Sram => memory.sram.origin() + offset,
        Location::Psram => memory.psram.unwrap().origin() + offset,
        Location::Flash => memory.flash_xip.origin() + offset,
        Location::Rom => 0x6000_0000 + offset,
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
    dequeue_facts.transfers = vec![transfer(dequeue, Some(task))];
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
    resolutions.add_task_poll(dequeue);
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
        line: 16,
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
        exception_copies: 1,
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
    stacks.handler_code = stacks
        .code_placement
        .handler_code(&analysis, &stacks.names, memory);
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
        // A direct task poll assumes nothing.
        let warnings = stacks.check(Required::Product).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
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

/// A table-backed analysis, with stack maxima selected independently per hart/level.
struct MultipleHandlers {
    stacks: crate::interrupts::InterruptStacks,
    analysis: Analysis,
    resolutions: Resolutions,
    exception: u32,
}

impl MultipleHandlers {
    fn new() -> Self {
        let stacks = partial(0);
        let exception = stacks.contract.memory.sram.origin() + 0x1000;
        Self {
            stacks,
            analysis: Analysis {
                traps: Default::default(),
                functions: BTreeMap::from([(exception, facts(exception, 4))]),
                code: Vec::new(),
                summaries: BTreeSet::new(),
            },
            resolutions: Resolutions::new(),
            exception,
        }
    }

    fn function(&mut self, address: u32, frame: u64, callees: &[u32]) {
        let mut value = facts(address, 32);
        value.frame = Some(frame);
        value.transfers = callees
            .iter()
            .enumerate()
            .map(|(index, &callee)| transfer(address + index as u32 * 4, Some(callee)))
            .collect();
        self.analysis.functions.insert(address, value);
    }

    fn bind(&mut self, hart: u32, level: u32, class: Class, handler: u32) {
        let index = self.stacks.table.len() as u32;
        self.stacks.table.push(TableEntry {
            source: index,
            line: 16 + index,
            level,
            class,
            core: hart,
            handler: Some(handler),
        });
    }

    fn finish(mut self) -> (crate::interrupts::InterruptStacks, Analysis) {
        let cores: Vec<_> = self
            .stacks
            .table
            .iter()
            .map(|entry| entry.core)
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let vectors: Vec<_> = self
            .stacks
            .table
            .iter()
            .filter_map(|entry| {
                entry.handler.map(|handler| oer_image_stack::Vector {
                    hart: entry.core,
                    line: entry.line,
                    entry: TrapEntry {
                        entry: handler,
                        handler,
                        frame: 0,
                    },
                })
            })
            .collect();
        let inputs = Stacks {
            table: &self.stacks.table,
            cores: &cores,
            vectors: &vectors,
            exception: TrapEntry {
                entry: self.exception,
                handler: self.exception,
                frame: 0,
            },
            exception_copies: 1,
            resolutions: &self.resolutions,
        };
        self.stacks.harts =
            oer_image_stack::interrupt_stacks(&self.analysis, &inputs, &mut ledger(), &mut || {
                Ok(())
            })
            .unwrap();
        self.stacks.code_placement = CodePlacement::of(
            &self.analysis,
            &self
                .analysis
                .functions
                .values()
                .map(|value| value.function.clone())
                .collect::<Vec<_>>(),
            &self.stacks.harts,
            &inputs,
            BTreeSet::new(),
            &self.stacks.contract.memory,
            &mut ledger(),
            &mut || Ok(()),
        )
        .unwrap();
        self.stacks.handler_code = self.stacks.code_placement.handler_code(
            &self.analysis,
            &self.stacks.names,
            &self.stacks.contract.memory,
        );
        (self.stacks, self.analysis)
    }
}

#[test]
fn forbidden_operation_in_a_non_worst_handler_is_reported() {
    let mut fixture = MultipleHandlers::new();
    let worst = fixture.exception + 0x100;
    let other = worst + 0x100;
    let helper = fixture.stacks.contract.memory.psram.unwrap().origin() + 0x2000;
    let panic = worst + 0x200;
    fixture.function(worst, 256, &[]);
    fixture.function(other, 0, &[helper]);
    fixture.function(helper, 0, &[panic]);
    fixture.function(panic, 0, &[]);
    fixture
        .analysis
        .functions
        .get_mut(&helper)
        .unwrap()
        .float_sites
        .push(helper + 4);
    fixture
        .stacks
        .names
        .insert(helper, "sample::non_worst_helper".into());
    fixture
        .stacks
        .names
        .insert(panic, "core::panicking::panic_fmt".into());
    fixture.bind(0, 1, Class::Hardware, worst);
    fixture.bind(0, 1, Class::Hardware, other);
    let (stacks, analysis) = fixture.finish();
    let bound = &stacks.harts[0].levels[0].bound;
    assert_eq!(bound.root, worst);
    assert!(!bound.reached.contains(&helper));
    let old_inventory = handler_code(
        &analysis,
        &bound.reached,
        &stacks.names,
        &stacks.contract.memory,
    );
    assert!(old_inventory.float.is_empty());
    assert!(old_inventory.cached.is_empty());
    assert!(old_inventory.panics.is_empty());
    assert_eq!(stacks.handler_code.float, [(helper, helper + 4)]);
    assert_eq!(stacks.handler_code.cached, [helper]);
    assert_eq!(stacks.handler_code.panics, [(helper, panic)]);
    for policy in [Required::Product, Required::Diagnostic] {
        let error = stacks.check(policy).unwrap_err().to_string();
        assert!(error.contains("floating point"), "{error}");
        assert!(error.contains("sample::non_worst_helper"), "{error}");
    }
}

#[test]
fn a_non_worst_executor_root_keeps_its_task_side_after_the_poll_cut() {
    use oer_image_stack::Violation;
    let mut fixture = MultipleHandlers::new();
    let worst = fixture.exception + 0x100;
    let executor = worst + 0x100;
    let dispatch = worst + 0x200;
    let shared = worst + 0x300;
    let task = fixture.stacks.contract.memory.psram.unwrap().origin() + 0x2000;
    let leaf = task + 0x100;
    fixture.function(worst, 256, &[]);
    fixture.function(executor, 0, &[dispatch, shared]);
    fixture.function(dispatch, 0, &[task]);
    fixture.function(shared, 0, &[]);
    fixture.function(task, 0, &[leaf, shared]);
    fixture.function(leaf, 0, &[]);
    fixture.resolutions.add_task_poll(dispatch);
    fixture.bind(0, 2, Class::Exempt(Violation::ExecutorLevel), worst);
    fixture.bind(0, 2, Class::Executor, executor);
    let (stacks, _) = fixture.finish();
    assert_eq!(stacks.harts[0].levels[0].bound.root, worst);
    assert!(!stacks.harts[0].levels[0].bound.reached.contains(&task));
    let context = &stacks.code_placement.contexts[0];
    assert_eq!(
        context.strict,
        BTreeSet::from([worst, executor, dispatch, shared])
    );
    assert_eq!(context.tasks, BTreeSet::from([task, leaf]));
    assert_eq!(context.task_roots, BTreeSet::from([task]));
    assert_eq!(stacks.code_placement.functions[&task].role, Role::Task);
    assert_eq!(stacks.code_placement.functions[&shared].role, Role::Strict);
    assert!(!context.cached_paths.contains_key(&task));
    assert_eq!(stacks.handler_code.cached, [task, leaf]);
}

#[test]
fn every_hart_uses_its_own_complete_handler_roots() {
    let mut fixture = MultipleHandlers::new();
    let worst0 = fixture.exception + 0x100;
    let other0 = worst0 + 0x100;
    let worst1 = worst0 + 0x200;
    let other1 = worst0 + 0x300;
    let helper0 = fixture.stacks.contract.memory.psram.unwrap().origin() + 0x2000;
    let helper1 = helper0 + 0x100;
    for root in [worst0, worst1] {
        fixture.function(root, 256, &[]);
    }
    fixture.function(other0, 0, &[helper0]);
    fixture.function(other1, 0, &[helper1]);
    fixture.function(helper0, 0, &[]);
    fixture.function(helper1, 0, &[]);
    // The separate exception audit is not part of the interrupt-handler inventory.
    fixture
        .analysis
        .functions
        .get_mut(&fixture.exception)
        .unwrap()
        .float_sites
        .push(fixture.exception);
    fixture.bind(0, 1, Class::Hardware, worst0);
    fixture.bind(0, 1, Class::Hardware, other0);
    fixture.bind(1, 1, Class::Hardware, worst1);
    fixture.bind(1, 1, Class::Hardware, other1);
    let (stacks, _) = fixture.finish();
    for (hart, roots) in [
        (0, [worst0, other0, helper0]),
        (1, [worst1, other1, helper1]),
    ] {
        let context = stacks
            .code_placement
            .contexts
            .iter()
            .find(|context| context.hart == hart && context.level == 1)
            .unwrap();
        assert_eq!(context.strict, BTreeSet::from(roots));
    }
    assert_eq!(stacks.handler_code.cached, [helper0, helper1]);
    assert!(stacks.handler_code.float.is_empty());
}

#[test]
fn cached_paths_start_at_either_root_and_choose_the_shortest_route() {
    let mut fixture = MultipleHandlers::new();
    let worst = fixture.exception + 0x100;
    let other = worst + 0x100;
    let long = fixture.stacks.contract.memory.psram.unwrap().origin() + 0x2000;
    let shared = long + 0x100;
    let exclusive = long + 0x200;
    fixture.function(worst, 256, &[long]);
    fixture.function(other, 0, &[shared, exclusive]);
    fixture.function(long, 0, &[shared]);
    fixture.function(shared, 0, &[]);
    fixture.function(exclusive, 0, &[]);
    fixture.bind(0, 1, Class::Hardware, worst);
    fixture.bind(0, 1, Class::Hardware, other);
    let (stacks, _) = fixture.finish();
    let paths = &stacks.code_placement.contexts[0].cached_paths;
    assert_eq!(paths[&long], [worst, long]);
    assert_eq!(paths[&shared], [other, shared]);
    assert_eq!(paths[&exclusive], [other, exclusive]);
}
