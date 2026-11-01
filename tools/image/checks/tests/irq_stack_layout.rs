//! Exercise the production SRAM layout with the pinned RISC-V linker.

use std::{fs, process::Command};

use object::{
    Architecture, BinaryFormat, Endianness, Object as _, ObjectSection as _, ObjectSymbol as _,
    SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};

fn link(harts: u32, allocations: &[u32], origin: u32) -> Result<Vec<u8>, String> {
    link_with_task_stack(harts, allocations, origin, 196608)
}

fn link_with_task_stack(
    harts: u32,
    allocations: &[u32],
    origin: u32,
    task_stack: u32,
) -> Result<Vec<u8>, String> {
    let guards: Vec<_> = (0..harts)
        .map(|hart| if hart == 0 { 4096 } else { 1024 })
        .collect();
    let tasks: Vec<_> = allocations
        .iter()
        .map(|&bytes| if bytes == 0 { 0 } else { task_stack })
        .collect();
    link_stacks(allocations, &tasks, &guards, origin)
}

fn link_stacks(
    allocations: &[u32],
    tasks: &[u32],
    guards: &[u32],
    origin: u32,
) -> Result<Vec<u8>, String> {
    let harts = allocations.len() as u32;
    assert_eq!(tasks.len(), allocations.len());
    assert_eq!(guards.len(), allocations.len());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    fs::write(
        path.join("memory.x"),
        include_str!("../../../../chips/espressif/staged-layout/linker/runtime/memory.x"),
    )
    .unwrap();
    fs::write(
        path.join("sections.x"),
        include_str!("../../../../chips/espressif/staged-layout/linker/runtime/sections.x"),
    )
    .unwrap();
    let (facts, _) = oer_espressif_facts::generate::tracked("esp32s31").unwrap();
    let mut vectors = oer_espressif_facts::generate::Vectors::of(&facts).unwrap();
    vectors.harts = harts;
    vectors.boot_hart = 0;
    fs::write(
        path.join("oer-staged-runtime-stacks.x"),
        vectors.stack_linker(),
    )
    .unwrap();
    for fragment in ["strict-text.x", "hot-text.x", "dma-window.x"] {
        fs::write(path.join(fragment), "").unwrap();
    }
    fs::write(
        path.join("link.x"),
        "ENTRY(_runtime_start)\nINCLUDE memory.x\nINCLUDE sections.x\nINCLUDE oer-staged-runtime-stacks.x\n",
    )
    .unwrap();

    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    let text = object.add_section(Vec::new(), b".text._start".to_vec(), SectionKind::Text);
    object.append_section_data(text, &0x00008067_u32.to_le_bytes(), 4); // ret
    object.add_symbol(Symbol {
        name: b"_runtime_start".to_vec(),
        value: 0,
        size: 4,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let isr = object.add_section(Vec::new(), b".isr.text".to_vec(), SectionKind::Text);
    object.append_section_data(isr, &0x00008067_u32.to_le_bytes(), 4);
    let bss = object.add_section(
        Vec::new(),
        b".critical.bss".to_vec(),
        SectionKind::UninitializedData,
    );
    object.append_section_bss(bss, 64, 4);
    fs::write(path.join("fixture.o"), object.write().unwrap()).unwrap();

    let mut command = Command::new(oer_os_toolchain::lld().unwrap());
    command
        .current_dir(path)
        .args(["-T", "link.x", "fixture.o", "-o", "runtime.elf"]);
    for (name, value) in [
        ("SRAM_ORIGIN", origin),
        ("SRAM_LENGTH", 0x100000),
        ("RETAINED_ORIGIN", 0x2e000000),
        ("RETAINED_LENGTH", 0x8000),
        ("RETAINED_RESERVED_TAIL", 24),
        ("RUNTIME_PSRAM_ORIGIN", 0x50010000),
        ("RUNTIME_PSRAM_LENGTH", 0x400000),
        ("STAGE_TWO_MAGIC", 0x32475453),
        ("STAGE_TWO_ABI_VERSION", 2),
        ("STAGE_TWO_HEADER_BYTES", 48),
        ("HART_COUNT", harts),
        ("MMU_PAGE_BYTES", 0x10000),
        ("HANDOFF_SRAM_MARGIN_BYTES", 0x2000),
    ] {
        command.arg(format!("--defsym={name}={value}"));
    }
    for (hart, ((irq, task), guard)) in allocations.iter().zip(tasks).zip(guards).enumerate() {
        for (name, value) in [
            (format!("IRQ_STACK_HART{hart}_BYTES"), irq),
            (format!("TASK_STACK_HART{hart}_BYTES"), task),
            (format!("__runtime_cpu{hart}_irq_stack_guard_bytes"), guard),
        ] {
            command.arg(format!("--defsym={name}={value}"));
        }
    }
    let output = command.output().unwrap();
    if output.status.success() {
        Ok(fs::read(path.join("runtime.elf")).unwrap())
    } else {
        Err(String::from_utf8(output.stderr).unwrap())
    }
}

#[test]
fn exact_noload_stacks_precede_isr_code_and_leave_only_state_in_critical_bss() {
    let origin = 0x20000000;
    for harts in [1, 3, 7] {
        for allocation in [32768, 65536] {
            let bytes = link(harts, &vec![allocation; harts as usize], origin).unwrap();
            let elf = object::File::parse(&*bytes).unwrap();
            let address = |name: &str| elf.symbol_by_name(name).unwrap().address();
            let stacks = elf.section_by_name(".irq_stacks").unwrap();
            assert_eq!(stacks.kind(), SectionKind::UninitializedData);
            assert!(stacks.file_range().is_none());
            assert_eq!(stacks.address(), u64::from(origin));
            assert_eq!(stacks.size(), u64::from(harts * allocation));
            for hart in 0..harts {
                let bottom = address(&format!("__runtime_cpu{hart}_irq_stack_bottom"));
                let top = address(&format!("__runtime_cpu{hart}_irq_stack_top"));
                assert_eq!(bottom, u64::from(origin + hart * allocation));
                assert_eq!(top - bottom, u64::from(allocation));
                assert_eq!(bottom % if hart == 0 { 4096 } else { 1024 }, 0);
            }
            assert_eq!(
                elf.section_by_name(".isr.text").unwrap().address(),
                stacks.address() + stacks.size()
            );
            assert_eq!(elf.section_by_name(".critical.bss").unwrap().size(), 64);
        }
    }
}

#[test]
fn refuses_an_unaligned_hart_zero_bottom() {
    let error = link(1, &[32768], 0x20000040).unwrap_err();
    assert!(
        error.contains("hart 0 IRQ stack bottom must be guard aligned"),
        "{error}"
    );
}

#[test]
fn checks_hart_one_alignment_only_when_it_exists() {
    // The first stack fits its contract, but its nonstandard length puts
    // the second bottom 16 bytes beyond its guard's alignment.
    let error = link(3, &[32768 + 16; 3], 0x20000000).unwrap_err();
    assert!(
        error.contains("hart 1 IRQ stack bottom must be guard aligned"),
        "{error}"
    );
    link(1, &[32768 + 16], 0x20000000).unwrap();
}

#[test]
fn per_hart_allocations_keep_both_guard_bottoms_aligned_without_growing_other_state() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for chip in ["esp32s31"] {
        let profile = oer_chip_profile::Profile::load(&root, chip).unwrap();
        let contract = profile.facts.interrupts.unwrap();
        let mut sizes = vec![0; contract.harts.len()];
        for &hart in &contract.harts {
            sizes[hart as usize] = contract
                .irq_stack_allocation(hart, Some(if hart == 0 { 16192 } else { 1008 }))
                .unwrap()
                .bytes;
        }
        let bytes = link(
            contract.harts.len() as u32,
            &sizes,
            profile.facts.memory.as_ref().unwrap().sram.origin(),
        )
        .unwrap();
        let elf = oer_analysis_elf::Elf::parse(&bytes).unwrap();
        for &hart in &contract.harts {
            let bottom = elf
                .address(&format!("__runtime_cpu{hart}_irq_stack_bottom"))
                .unwrap();
            let top = elf
                .address(&format!("__runtime_cpu{hart}_irq_stack_top"))
                .unwrap();
            assert_eq!(bottom % u64::from(contract.irq_stack_guard_bytes[&hart]), 0);
            assert_eq!(top - bottom, u64::from(sizes[hart as usize]));
        }
        let object = object::File::parse(&*bytes).unwrap();
        let stacks = object.section_by_name(".irq_stacks").unwrap();
        let code = object.section_by_name(".isr.text").unwrap();
        assert_eq!(code.address() % 256, 0);
        assert!(code.address() - (stacks.address() + stacks.size()) <= 255);
        assert_eq!(object.section_by_name(".critical.bss").unwrap().size(), 64);
        assert_eq!(
            object.section_by_name(".irq_stacks").unwrap().size(),
            sizes.iter().map(|&x| u64::from(x)).sum::<u64>()
        );
    }
}

#[test]
fn a_task_stack_sized_from_its_bound_is_an_exact_range_below_bss() {
    for task_stack in [17264, 196608] {
        let bytes = link_with_task_stack(1, &[32768], 0x20000000, task_stack).unwrap();
        let elf = object::File::parse(&*bytes).unwrap();
        let address = |name: &str| elf.symbol_by_name(name).unwrap().address();
        let bottom = address(&format!("__runtime_cpu{}_task_stack_bottom", 0));
        let top = address(&format!("__runtime_cpu{}_task_stack_top", 0));
        assert_eq!(top - bottom, u64::from(task_stack));
        assert_eq!(bottom % 64, 0);
        assert!(elf.section_by_name(".bss").unwrap().address() >= top);
        assert!(elf.symbol_by_name("_stack_end").is_none());
    }
}

#[test]
fn nonadjacent_unused_harts_reserve_zero_bytes_with_heterogeneous_guards() {
    let irq = [32768, 0, 8192, 0, 0, 4096, 0];
    let task = [17264, 0, 8192, 0, 0, 16384, 0];
    let guards = [4096, 1024, 2048, 8192, 256, 1024, 4096];
    let bytes = link_stacks(&irq, &task, &guards, 0x20000000).unwrap();
    let elf = object::File::parse(&*bytes).unwrap();
    let address = |name: &str| elf.symbol_by_name(name).unwrap().address();
    assert_eq!(
        elf.section_by_name(".irq_stacks").unwrap().size(),
        irq.iter().map(|&n| u64::from(n)).sum::<u64>()
    );
    let mut previous_irq_top = 0;
    let mut previous_task_top = 0;
    for (hart, ((irq, task), guard)) in irq.iter().zip(task).zip(guards).enumerate() {
        for (kind, bytes, previous) in [
            ("irq", *irq, &mut previous_irq_top),
            ("task", task, &mut previous_task_top),
        ] {
            let bottom = address(&format!("__runtime_cpu{hart}_{kind}_stack_bottom"));
            let top = address(&format!("__runtime_cpu{hart}_{kind}_stack_top"));
            assert_eq!(top - bottom, u64::from(bytes));
            assert!(bottom >= *previous);
            if bytes == 0 {
                assert_eq!(bottom, *previous);
            }
            if kind == "irq" && bytes > 0 {
                assert_eq!(bottom % u64::from(guard), 0);
            }
            *previous = top;
        }
    }
    assert!(elf.section_by_name(".bss").unwrap().address() >= previous_task_top);
}

#[test]
fn task_irq_disagreement_and_out_of_memory_allocations_are_rejected() {
    assert!(
        link_stacks(
            &[32768, 0, 8192],
            &[16384, 4096, 8192],
            &[4096, 1024, 2048],
            0x20000000
        )
        .unwrap_err()
        .contains("task and IRQ use must agree")
    );
    assert!(
        link_stacks(&[32768], &[0], &[4096], 0x20000000)
            .unwrap_err()
            .contains("boot hart needs both stacks")
    );
    assert!(link_stacks(&[32768], &[0x400000], &[4096], 0x20000000).is_err());
}

#[test]
fn an_omitted_guard_links_without_dividing_by_zero_or_reserving_guard_storage() -> Result<(), String>
{
    let bytes = link_stacks(&[32768, 0], &[196608, 0], &[0, 1024], 0x20000000)?;
    let elf = object::File::parse(&*bytes).map_err(|error| error.to_string())?;
    assert_eq!(
        elf.symbol_by_name("__runtime_cpu0_irq_stack_guard_bytes")
            .ok_or("missing guard size")?
            .address(),
        0
    );
    assert_eq!(
        elf.section_by_name(".irq_stacks")
            .ok_or("missing stacks")?
            .size(),
        32768
    );
    Ok(())
}
