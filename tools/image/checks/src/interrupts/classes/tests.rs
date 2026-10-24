use super::*;
use crate::interrupts::Required;
use crate::interrupts::code::{Location, tests::fixture};
use std::collections::BTreeMap;

fn entry(source: u32, level: u32, class: Class, core: u32) -> TableEntry {
    TableEntry {
        source,
        level,
        class,
        core,
        handler: Some(0x4080_0000 + source * 0x10),
    }
}

#[test]
fn an_executor_s_level_on_its_core_takes_no_hardware_handler() {
    let shared = [
        entry(1, 2, Class::Executor, 0),
        entry(2, 2, Class::Hardware, 0),
    ];
    assert_eq!(context_class(&shared, 0, 2), Class::Executor);
    assert_eq!(
        misplaced(&shared),
        ["the hardware handler of source 2 shares level 2 with core 0's executor"]
    );
    // The same level on another core, another level, an entry the build
    // leaves out: no conflict.
    let mut apart = shared;
    apart[1].core = 1;
    assert!(misplaced(&apart).is_empty());
    assert_eq!(context_class(&apart, 1, 2), Class::Hardware);
    apart[1] = entry(2, 3, Class::Hardware, 0);
    assert!(misplaced(&apart).is_empty());
    let mut absent = shared;
    absent[1].handler = None;
    assert!(misplaced(&absent).is_empty());
    let two = [
        entry(1, 2, Class::Executor, 0),
        entry(2, 2, Class::Executor, 0),
    ];
    assert_eq!(misplaced(&two).len(), 2);
    assert!(misplaced(&two)[0].contains("one of 2 executors"));
    let alone = [entry(3, 2, Class::Exempt, 0)];
    assert!(misplaced(&alone)[0].contains("no executor of core 0"));
}

#[test]
fn a_misplaced_class_fails_every_policy() {
    let (mut stacks, _, _) = fixture(Location::Sram, Location::Psram, 2, Class::Executor);
    stacks.table.push(entry(2, 2, Class::Hardware, 0));
    stacks.misplaced = misplaced(&stacks.table);
    for required in [Required::Product, Required::Diagnostic] {
        let error = stacks.check(required).unwrap_err().to_string();
        assert!(
            error.contains("shares level 2 with core 0's executor"),
            "{error}"
        );
    }
    assert!(stacks.render().contains("1 misplaced"));
}

#[test]
fn hardware_code_neither_polls_tasks_nor_calls_wakers_nor_reads_cached_data() {
    // The fixture's dequeue polls its task: on a hardware level that is a
    // breach, wherever the code lies.
    let (mut stacks, shared, _) = fixture(Location::Sram, Location::Sram, 3, Class::Hardware);
    let memory = &stacks.contract.memory;
    let root = memory.sram.origin + 0x1000;
    let dequeue = root + 0x40;
    let psram = u64::from(memory.psram.unwrap().origin) + 0x100;
    let context = &stacks.code_placement.contexts[0];
    assert_eq!(context.to_string(), "hart 0 P3 (hardware)");
    let mut analysis = Analysis {
        traps: Default::default(),
        functions: BTreeMap::new(),
        code: Vec::new(),
        summaries: BTreeSet::new(),
    };
    let mut dequeue_facts = crate::interrupts::placement::tests::facts(dequeue, 32);
    dequeue_facts.transfers = vec![oer_analysis_riscv_stack::Transfer {
        site: dequeue,
        target: None,
        kind: oer_analysis_riscv_stack::TransferKind::Call,
        source: None,
        table: None,
        load: None,
    }];
    analysis.functions.insert(dequeue, dequeue_facts);
    let mut resolutions = Resolutions::new();
    resolutions.add(dequeue, Fact::TaskPolls, [shared]);
    let reached = BTreeSet::from([dequeue]);
    let found = hardware_code(
        "hart 0 P3 (hardware)",
        &analysis,
        &reached,
        &resolutions,
        &[(dequeue, dequeue + 8, psram), (root, root, psram)],
    );
    assert_eq!(
        found
            .iter()
            .map(|finding| finding.breach)
            .collect::<Vec<_>>(),
        [Breach::TaskPoll, Breach::CachedData(psram)]
    );
    let mut wakers = Resolutions::new();
    wakers.add(dequeue, Fact::WakerVtables, [shared]);
    let found = hardware_code("waker", &analysis, &reached, &wakers, &[]);
    assert_eq!(found[0].breach, Breach::WakerCall);

    stacks.hardware = hardware_code(
        "hart 0 P3 (hardware)",
        &analysis,
        &reached,
        &resolutions,
        &[],
    );
    let error = stacks.check(Required::Product).unwrap_err().to_string();
    assert!(
        error.contains("hardware code of hart 0 P3 (hardware) polls tasks"),
        "{error}"
    );
    let warnings = stacks.check(Required::Diagnostic).unwrap();
    assert!(
        warnings.iter().any(|warning| warning
            == "hart 0 P3 (hardware): 1 breaches of the hardware class (diagnostic image, \
                reported only); see interrupt-stack.txt"),
        "{warnings:?}"
    );
}

#[test]
fn an_exempt_entry_beside_the_executor_passes_with_its_reason_in_the_report() {
    let (mut stacks, _, _) = fixture(Location::Sram, Location::Psram, 2, Class::Executor);
    let exempt = entry(9, 2, Class::Exempt, 0);
    stacks.table.push(exempt);
    stacks.misplaced = misplaced(&stacks.table);
    assert!(stacks.misplaced.is_empty());
    let slot = exempt.handler.unwrap();
    stacks.names.insert(slot, "MODEM_ZB_MAC".into());
    stacks.exemptions = vec![(slot, "stays on P2 until its port".into())];
    stacks.check(Required::Product).unwrap();
    let report = stacks.render();
    assert!(
        report.contains(
            "  MODEM_ZB_MAC (source 9): core 0 level 2, exempt (shares its core's executor \
             level): stays on P2 until its port\n"
        ),
        "{report}"
    );
    assert!(report.contains("level 2 (executor)"), "{report}");
}
