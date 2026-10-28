use super::*;
use crate::interrupts::Required;
use crate::interrupts::code::{Location, tests::fixture};
use std::collections::BTreeMap;

fn entry(source: u32, level: u32, class: Class, core: u32) -> TableEntry {
    TableEntry {
        source,
        line: 16 + source,
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
    let alone = [entry(3, 2, Class::Exempt(Violation::ExecutorLevel), 0)];
    assert!(misplaced(&alone)[0].contains("no executor of core 0"));
    // A waker exemption sits where a hardware handler may.
    let waker = [
        entry(1, 2, Class::Executor, 0),
        entry(2, 2, Class::Exempt(Violation::WakerCall), 0),
    ];
    assert!(misplaced(&waker)[0].contains("shares level 2"));
    assert!(misplaced(&waker[1..]).is_empty());
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
fn hardware_code_neither_polls_tasks_nor_calls_wakers_and_its_data_is_reported() {
    // The fixture's dequeue polls its task: on a hardware level that is a
    // breach, wherever the code lies.
    let (mut stacks, shared, _) = fixture(Location::Sram, Location::Sram, 3, Class::Hardware);
    let memory = &stacks.contract.memory;
    let root = memory.sram.origin() + 0x1000;
    let dequeue = root + 0x40;
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
        target: Some(shared),
        kind: oer_analysis_riscv_stack::TransferKind::Call,
        source: None,
        table: None,
        load: None,
    }];
    analysis.functions.insert(dequeue, dequeue_facts);
    let mut resolutions = Resolutions::new();
    resolutions.add_task_poll(dequeue);
    let reached = BTreeSet::from([dequeue]);
    let found = hardware_code("hart 0 P3 (hardware)", &analysis, &reached, &resolutions);
    assert_eq!(
        found
            .iter()
            .map(|finding| finding.breach)
            .collect::<Vec<_>>(),
        [Breach::TaskPoll]
    );
    // Code the context does not reach is not its breach.
    let other = hardware_code("other", &analysis, &BTreeSet::from([root]), &resolutions);
    assert!(other.is_empty());
    let mut wakers = Resolutions::new();
    wakers.add(dequeue, Fact::WakerVtables, [shared]);
    let found = hardware_code("waker", &analysis, &reached, &wakers);
    assert_eq!(found[0].breach, Breach::WakerCall);

    // Cached data is reported, never failed.
    stacks.strict_data = vec![CachedData {
        subject: "hart 0 P3 (hardware)".into(),
        references: 3,
        writable: BTreeSet::from(["WRITER".to_owned()]),
        read_only: BTreeSet::from([".Lanon.1".to_owned(), ".Lswitch.table.X".to_owned()]),
    }];
    stacks.check(Required::Product).unwrap();
    let report = stacks.render();
    assert!(
        report.contains(
            "strict cached data of hart 0 P3 (hardware) (reported only): 3 references, 1 \
             writable objects, 2 read-only objects\n    writable: WRITER\n    read-only: \
             .Lanon.1, .Lswitch.table.X\n"
        ),
        "{report}"
    );

    stacks.hardware = hardware_code("hart 0 P3 (hardware)", &analysis, &reached, &resolutions);
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
    let exempt = entry(9, 2, Class::Exempt(Violation::ExecutorLevel), 0);
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

#[test]
fn a_waker_exemption_waives_only_its_own_handler_s_waker_calls() {
    let finding = |function, breach| Finding {
        subject: "hart 0 P1 (hardware)".into(),
        function,
        site: function,
        breach,
    };
    let (usb, wake, other) = (0x100, 0x200, 0x300);
    let found = vec![
        finding(wake, Breach::WakerCall),
        finding(usb, Breach::TaskPoll),
    ];
    let exempt = Class::Exempt(Violation::WakerCall);
    // Only the exempt USB handler reaches the waker call: waived. Its task
    // poll is not the named violation: it fails.
    let (failed, waived) = waive(
        found.clone(),
        &BTreeSet::from([other]),
        &[
            (exempt, BTreeSet::from([usb, wake])),
            (Class::Hardware, BTreeSet::from([other])),
        ],
    );
    assert_eq!(waived, [finding(wake, Breach::WakerCall)]);
    assert_eq!(failed, [finding(usb, Breach::TaskPoll)]);
    // A hardware entry, or the dispatch path, reaching the same call fails.
    let (failed, waived) = waive(
        found.clone(),
        &BTreeSet::new(),
        &[
            (exempt, BTreeSet::from([usb, wake])),
            (Class::Hardware, BTreeSet::from([wake])),
        ],
    );
    assert!(waived.is_empty());
    assert_eq!(failed.len(), 2);
    let (failed, _) = waive(
        found.clone(),
        &BTreeSet::from([wake]),
        &[(exempt, BTreeSet::from([usb, wake]))],
    );
    assert_eq!(failed.len(), 2);
    // An executor-level exemption waives no waker call.
    let (failed, _) = waive(
        found,
        &BTreeSet::new(),
        &[(
            Class::Exempt(Violation::ExecutorLevel),
            BTreeSet::from([usb, wake]),
        )],
    );
    assert_eq!(failed.len(), 2);
}

#[test]
fn a_waived_breach_passes_and_is_reported_with_the_reason() {
    let (mut stacks, _, _) = fixture(Location::Sram, Location::Sram, 1, Class::Hardware);
    let usb = entry(2, 1, Class::Exempt(Violation::WakerCall), 0);
    stacks.table.push(usb);
    let slot = usb.handler.unwrap();
    stacks.names.insert(slot, "USB_DEVICE".into());
    stacks.exemptions = vec![(slot, "its console wakes an AtomicWaker".into())];
    stacks.waived = vec![Finding {
        subject: "hart 0 P1 (hardware)".into(),
        function: slot,
        site: slot + 4,
        breach: Breach::WakerCall,
    }];
    let warnings = stacks.check(Required::Product).unwrap();
    assert!(
        warnings
            .iter()
            .all(|warning| !warning.contains("hardware class"))
    );
    let report = stacks.render();
    assert!(
        report.contains(
            "  USB_DEVICE (source 2): core 0 level 1, exempt (calls a waker): its console wakes \
             an AtomicWaker\n"
        ),
        "{report}"
    );
    assert!(
        report.contains("  waived: hart 0 P1 (hardware): USB_DEVICE at 0x40800024 calls a waker\n"),
        "{report}"
    );
}

#[test]
fn an_exemption_waives_only_its_named_violation() {
    // A waker exemption does not lift the cached-memory rule.
    let (stacks, _, _) = fixture(
        Location::Sram,
        Location::Psram,
        1,
        Class::Exempt(Violation::WakerCall),
    );
    let error = stacks.check(Required::Product).unwrap_err().to_string();
    assert!(error.contains("from cached memory"), "{error}");
    // Nor does it lift a task poll.
    let (mut stacks, shared, _) = fixture(
        Location::Sram,
        Location::Sram,
        1,
        Class::Exempt(Violation::WakerCall),
    );
    stacks.hardware = vec![Finding {
        subject: "hart 0 P1 (hardware)".into(),
        function: shared,
        site: shared,
        breach: Breach::TaskPoll,
    }];
    let error = stacks.check(Required::Product).unwrap_err().to_string();
    assert!(error.contains("polls tasks"), "{error}");
    // An executor-level exemption does not lift the cached-memory rule.
    let (stacks, _, _) = fixture(
        Location::Sram,
        Location::Psram,
        2,
        Class::Exempt(Violation::ExecutorLevel),
    );
    let error = stacks.check(Required::Product).unwrap_err().to_string();
    assert!(error.contains("from cached memory"), "{error}");
}
