//! Exercise the production SRAM layout with the pinned RISC-V linker.

use std::{fs, process::Command};

use object::{
    Architecture, BinaryFormat, Endianness, Object as _, ObjectSection as _, ObjectSymbol as _,
    SectionKind, SymbolFlags, SymbolKind, SymbolScope,
    write::{Object, Symbol, SymbolSection},
};

fn link(harts: u32, allocations: [u32; 2], origin: u32) -> Result<Vec<u8>, String> {
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
    for fragment in ["strict-text.x", "dma-window.x"] {
        fs::write(path.join(fragment), "").unwrap();
    }
    fs::write(
        path.join("link.x"),
        "ENTRY(_runtime_start)\nINCLUDE memory.x\nINCLUDE sections.x\n",
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
        ("SRAM_LENGTH", 0x40000),
        ("RETAINED_ORIGIN", 0x2e000000),
        ("RETAINED_LENGTH", 0x8000),
        ("RETAINED_RESERVED_TAIL", 24),
        ("RUNTIME_PSRAM_ORIGIN", 0x50010000),
        ("RUNTIME_PSRAM_LENGTH", 0x100000),
        ("STAGE_TWO_MAGIC", 0x32475453),
        ("STAGE_TWO_ABI_VERSION", 1),
        ("STAGE_TWO_HEADER_BYTES", 44),
        ("IRQ_STACK_HART0_BYTES", allocations[0]),
        ("IRQ_STACK_HART1_BYTES", allocations[1]),
        ("CPU0_PSRAM_TASK_STACK_BYTES", 196608),
        ("HART_COUNT", harts),
        ("__runtime_cpu0_irq_stack_guard_bytes", 4096),
    ] {
        command.arg(format!("--defsym={name}={value}"));
    }
    // A one-hart image supplies no contract or symbol for hart 1.
    if harts > 1 {
        command.arg("--defsym=__runtime_cpu1_irq_stack_guard_bytes=1024");
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
    for harts in [1, 2] {
        for allocation in [32768, 65536] {
            let bytes = link(harts, [allocation; 2], origin).unwrap();
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
    let error = link(1, [32768; 2], 0x20000040).unwrap_err();
    assert!(
        error.contains("hart 0 IRQ stack bottom must be guard aligned"),
        "{error}"
    );
}

#[test]
fn checks_hart_one_alignment_only_when_it_exists() {
    // The first stack fits its contract, but its nonstandard length puts
    // the second bottom 16 bytes beyond its guard's alignment.
    let error = link(2, [32768 + 16; 2], 0x20000000).unwrap_err();
    assert!(
        error.contains("hart 1 IRQ stack bottom must be guard aligned"),
        "{error}"
    );
    link(1, [32768 + 16; 2], 0x20000000).unwrap();
}

#[test]
fn per_hart_allocations_keep_both_guard_bottoms_aligned_without_growing_other_state() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    for chip in ["esp32s31", "esp32c5"] {
        let profile = oer_chip_profile::Profile::load(&root, chip).unwrap();
        let contract = profile.interrupts.unwrap();
        let mut sizes = [0; 2];
        for &hart in &contract.harts {
            sizes[hart as usize] = contract
                .irq_stack_size(hart, Some(if hart == 0 { 16192 } else { 1008 }))
                .unwrap();
        }
        let bytes = link(
            contract.harts.len() as u32,
            sizes,
            profile.memory.as_ref().unwrap().sram.origin,
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
