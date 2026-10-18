#![cfg(not(target_arch = "riscv32"))]

//! Link the actual scope/IRQ paths; reject cached code, callback data or stacks.
use oer_riscv_model::{Ledger, WorkingMemory};
use oer_riscv_program::facts::ImageFacts;
use oer_riscv_stack::{
    analyze, function_pointer_resolutions, read_debug, taken_addresses, waker_resolutions,
    waker_vtables,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn build(
    root: &Path,
    output: &Path,
    script: &str,
    variant: &str,
    diagnostics: bool,
) -> Result<Vec<u8>> {
    let build = oer_toolchain::image::host_build_root()?;
    let cache = oer_toolchain::image::compile_cache(&build);
    let linker = oer_toolchain::image::linker_target(&build);
    fs::create_dir_all(output)?;
    let script_path = output.join(format!("{variant}.x"));
    fs::write(&script_path, script)?;
    let mut command = oer_toolchain::cargo_in(root);
    command.arg("rustc");
    if diagnostics {
        command.args([
            "--features",
            "oer-esp32s31-ieee80211-mac/task-poll-telemetry",
        ]);
    }
    command
        .args([
            "--release",
            "-p",
            "oer-esp32s31-ieee80211-form",
            "--example",
            "scope_placement",
            "--target",
            "riscv32imafc-unknown-none-elf",
            "--target-dir",
        ])
        .arg(&cache)
        .args(["--", "-Clink-arg=--emit-relocs"])
        .arg(format!("-Clink-arg=-T{}", script_path.display()));
    oer_toolchain::image::configure(
        &mut command,
        &oer_toolchain::image::ImageCompiler {
            root,
            linker_target: &linker,
            rust_target: "riscv32imafc-unknown-none-elf",
            max_move_bytes: 4096,
            runtime_environment: vec![],
        },
    )?;
    // The host-wide cache lock covers the copy, not only cargo's build.
    let _lock = oer_toolchain::image::lock_compile_cache(&cache)?;
    let result = command.output()?;
    fs::write(output.join(format!("{variant}.build.log")), &result.stderr)?;
    if !result.status.success() {
        return Err(format!(
            "placement fixture failed: {}",
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    let elf_path = output.join(format!("{variant}.elf"));
    fs::copy(
        cache.join("riscv32imafc-unknown-none-elf/release/examples/scope_placement"),
        &elf_path,
    )?;
    Ok(fs::read(elf_path)?)
}

fn audit(root: &Path, bytes: &[u8]) -> Result<(Vec<String>, String)> {
    let profile = oer_chip_profile::Profile::load(root, "esp32s31")?;
    let elf = oer_elf::Elf::executable(bytes)?;
    let memory = WorkingMemory::new(512 * 1024 * 1024)?;
    let mut control = || Ok(());
    let facts = ImageFacts::new(bytes, &[], &memory, &mut control)?;
    let mut keep = Ledger::new(&memory, &mut control)?;
    let analysis = analyze(&facts, &[], &[], &mut keep, &mut control)?;
    let (dwarf, types) = read_debug(&Rc::from(bytes), &mut keep, &mut control)?;
    let vtables = waker_vtables(&facts, &dwarf, &analysis, &mut keep, &mut control)?;
    assert!(
        !vtables.is_empty(),
        "the linked image must retain a real executor waker"
    );
    let mut resolutions = waker_resolutions(&analysis, &dwarf, &vtables, &mut keep, &mut control)?;
    let taken = taken_addresses(&facts, &mut keep, &mut control)?;
    resolutions.extend(function_pointer_resolutions(
        &analysis,
        &types,
        &taken,
        &mut keep,
        &mut control,
    )?);
    // Header's private result types identify the whole callback family in this
    // ELF. The registration proof forbids a forged Header or callback cast.
    // Thus this uses the same ExecutorInvariant premise as waker resolution.
    for (entry, result_type) in [
        (
            "mac_interrupt",
            "oer_esp32s31_ieee80211_form::scope::MacPublication",
        ),
        (
            "power_interrupt",
            "oer_esp32s31_ieee80211_form::scope::PowerPublication",
        ),
        (
            "notify_driver",
            "oer_esp32s31_ieee80211_form::scope::Scheduled",
        ),
    ] {
        let targets: Vec<_> = taken
            .iter()
            .copied()
            .filter(|address| dwarf.return_type(*address) == Some(result_type))
            .collect();
        assert!(
            !targets.is_empty(),
            "missing typed callback family {result_type}"
        );
        let mut entries = 0;
        for function in analysis.functions.values().filter(|function| {
            function.function.names.iter().any(|name| {
                let name = oer_elf::demangle(name);
                name.contains("ScopeSlot") && name.ends_with(entry)
            })
        }) {
            entries += 1;
            let unresolved: Vec<_> = function
                .transfers
                .iter()
                .filter(|transfer| {
                    transfer.target.is_none() && resolutions.get(transfer.site).is_none()
                })
                .collect();
            assert_eq!(
                unresolved.len(),
                1,
                "Header must be the only unresolved call in {entry}"
            );
            resolutions.add(
                unresolved[0].site,
                oer_riscv_stack::Fact::FieldType,
                targets.iter().copied(),
            );
        }
        assert_eq!(entries, 1, "missing or duplicated {entry} root");
    }
    let sram = profile
        .memory
        .as_ref()
        .ok_or("missing chip memory map")?
        .sram;
    let mut reached = BTreeSet::new();
    let mut data = BTreeMap::new();
    let mut report = String::new();
    let mut failures = Vec::new();
    let mut roots = Vec::new();
    for root in ["s31_form_mac_irq", "s31_form_power_irq"] {
        roots.push(u32::try_from(
            elf.address(root).ok_or("missing retained IRQ root")?,
        )?);
    }
    // Every entry in every retained waker vtable is executable, including its
    // clone/drop paths; a successful direct wake does not prove those paths.
    roots.extend(vtables.iter().flatten().copied());
    while let Some(root) = roots.pop() {
        if reached.contains(&root) {
            continue;
        }
        let bound = analysis.bound_with(root, &resolutions)?;
        report.push_str(&format!(
            "root {root:#010x}: stack={:?}, assumptions={:?}, unresolved={:?}\n",
            bound.bytes, bound.assumptions, bound.unresolved
        ));
        if !bound.unresolved.is_empty() {
            failures.push(format!(
                "{root:#010x} has unresolved callees: {:?}",
                bound.unresolved
            ));
        }
        let new: Vec<_> = bound.reached.difference(&reached).copied().collect();
        reached.extend(bound.reached);
        for address in new {
            let function = &analysis.functions[&address].function;
            report.push_str(&format!(
                "code {address:#010x} +{} {}\n",
                function.size,
                function.names.join(", ")
            ));
            if !sram.contains_range(
                u64::from(address),
                u64::from(address) + u64::from(function.size),
            ) {
                failures.push(format!("cached code: {}", function.names.join(", ")));
            }
            // A linked relocation from reachable code or data discovers its
            // next data dependency. Walk all such dependencies transitively,
            // including function pointers retained in a data/vtable object.
            let mut ranges = vec![(
                function.section,
                u64::from(address),
                u64::from(address) + u64::from(function.size),
            )];
            while let Some((section, start, end)) = ranges.pop() {
                for relocation in elf.relocations(section)? {
                    if !(start..end).contains(&relocation.at)
                        || oer_elf::rv32::kind(relocation.r_type).role == oer_elf::rv32::Role::Hint
                    {
                        continue;
                    }
                    let target = elf.target_address(&relocation)?;
                    let Some(destination) = elf.sections().find(|section| {
                        section.allocated
                            && (section.address..section.address + section.size).contains(&target)
                    }) else {
                        continue;
                    };
                    if destination.executable {
                        if let Some((&entry, _)) =
                            analysis.functions.iter().find(|(_, function)| {
                                let function = &function.function;
                                (u64::from(function.address)
                                    ..u64::from(function.address) + u64::from(function.size))
                                    .contains(&target)
                            })
                        {
                            roots.push(entry);
                        } else {
                            failures.push(format!("unmodelled code target {target:#010x}"));
                        }
                        continue;
                    }
                    let symbol = elf
                        .symbols()
                        .filter(|symbol| {
                            symbol.size != 0
                                && symbol.section == Some(destination.index)
                                && (symbol.address..symbol.address + symbol.size).contains(&target)
                        })
                        // LLVM can reference a merged-global base, then use
                        // an instruction offset to reach its other members.
                        // The smallest alias would cover WAKES but omit SLOT.
                        .max_by_key(|symbol| symbol.size);
                    let (begin, finish, name) = match symbol {
                        Some(symbol) => (
                            symbol.address,
                            symbol.address + symbol.size,
                            symbol.demangled(),
                        ),
                        // Anonymous constants are conservatively covered by
                        // their entire allocated section, never omitted.
                        None => (
                            destination.address,
                            destination.address + destination.size,
                            destination.name.to_owned(),
                        ),
                    };
                    if data.insert((begin, finish), name.clone()).is_none() {
                        report.push_str(&format!("data {begin:#010x}..{finish:#010x} {name}\n"));
                        if !sram.contains_range(begin, finish) {
                            failures.push(format!("cached data: {name}"));
                        }
                        ranges.push((destination.index, begin, finish));
                    }
                }
            }
        }
    }
    for fragment in [
        "scope_placement::image::SLOT",
        "scope_placement::image::WAKES",
        "scope_placement::image::VTABLE",
    ] {
        let symbol = elf
            .symbols()
            .find(|symbol| symbol.demangled() == fragment)
            .ok_or_else(|| format!("missing callback data {fragment}"))?;
        assert!(data.keys().any(|&(start, end)| start <= symbol.address && symbol.address + symbol.size <= end), "untraced callback data {fragment}");
    }
    // The address installed at run time points into this permanent object.
    // Its type contains Header, DriverRegion and the separate IRQ region and
    // atomic handoffs. Check the entire linked object, not the caller frame.
    let backing = elf
        .symbols()
        .find(|symbol| symbol.demangled() == "scope_placement::image::BACKING")
        .ok_or("missing permanent scope backing")?;
    if !sram.contains_range(backing.address, backing.address + backing.size) {
        failures.push("cached scope backing".into());
    }
    report.push_str(&format!("permanent_backing_bytes={}\n", backing.size));
    let command_layout = elf
        .address("S31_COMMAND_STORAGE_BYTES")
        .ok_or("missing command ABI measurement")?;
    let command_bytes = elf
        .sections()
        .find_map(|section| section.word(command_layout))
        .ok_or("unreadable command ABI measurement")?;
    report.push_str(&format!("command_storage_bytes={command_bytes}\n"));
    let bridge = elf
        .symbols()
        .find(|symbol| symbol.demangled() == "scope_placement::image::BRIDGE_BACKING")
        .ok_or("missing permanent command records")?;
    if !sram.contains_range(bridge.address, bridge.address + bridge.size) {
        failures.push("cached command execution backing".into());
    }
    // Key copies in records may outlive a drain timeout or a key command
    // published after the stop; they
    // must not outlive a reset. The staged runtime entry re-copies the
    // `__runtime_critical_data_*` range from the image and zeroes the
    // `__runtime_critical_bss_*` range at every reset; `.noinit` and
    // RTC-retained memory would keep them.
    let mut reset_ranges = Vec::new();
    for (start, end) in [
        (
            "__runtime_critical_data_start",
            "__runtime_critical_data_end",
        ),
        ("__runtime_critical_bss_start", "__runtime_critical_bss_end"),
    ] {
        reset_ranges.push((
            elf.address(start).ok_or("missing reset range start")?,
            elf.address(end).ok_or("missing reset range end")?,
        ));
    }
    if !reinitialized_at_reset(
        (bridge.address, bridge.address + bridge.size),
        &reset_ranges,
    ) {
        failures.push("command records lie outside the ranges a reset reinitializes".into());
    }
    let bridge_section = elf
        .sections()
        .find(|section| section.allocated && section.contains(bridge.address))
        .map(|section| section.name.to_owned())
        .ok_or("command records lie in no allocated section")?;
    report.push_str(&format!("bridge_backing_section={bridge_section}\n"));
    report.push_str(&format!("bridge_backing_bytes={}\n", bridge.size));
    for (symbol, label) in [
        ("S31_BRIDGE_HELD_BYTES", "bridge_held_bytes"),
        ("S31_BRIDGE_EXECUTOR_BYTES", "bridge_executor_bytes"),
    ] {
        let address = elf
            .address(symbol)
            .ok_or("missing bridge ABI measurement")?;
        let value = elf
            .sections()
            .find_map(|section| section.word(address))
            .ok_or("unreadable bridge ABI measurement")?;
        report.push_str(&format!("{label}={value}\n"));
    }
    let stack_begin = elf
        .address("__scope_stack_bottom")
        .ok_or("no scope stack")?;
    let stack_end = elf
        .address("__scope_stack_top")
        .ok_or("no scope stack limit")?;
    if !sram.contains_range(stack_begin, stack_end) {
        failures.push("cached scope stack".into());
    }
    let entry = u32::try_from(elf.address("scope_main").ok_or("no fixture root")?)?;
    let frame = analysis.functions[&entry]
        .frame
        .ok_or("unmeasured scope frame")?;
    assert!(frame < stack_end - stack_begin);
    let text: u64 = elf
        .sections()
        .filter(|section| section.allocated && section.executable)
        .map(|section| section.size)
        .sum();
    let static_ram: u64 = elf
        .sections()
        .filter(|section| section.allocated && section.writable && section.name != ".scope.stack")
        .map(|section| section.size)
        .sum();
    report.push_str(&format!("code_bytes={text}\nstatic_ram_bytes={static_ram}\nscope_root_frame={frame}\nstack_reservation={}\n", stack_end - stack_begin));
    Ok((failures, report))
}

#[test]
fn linked_irq_graph_and_callback_data_reside_in_sram() -> Result<()> {
    let package = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let root = package
        .ancestors()
        .nth(4)
        .ok_or("package is outside the repository")?;
    let output = root.join("target/agent-a/form-placement");
    let script = include_str!("placement.x");
    let bytes = build(root, &output, script, "scope_placement", false)?;
    let (failures, report) = audit(root, &bytes)?;
    fs::write(output.join("placement.txt"), &report)?;
    assert!(failures.is_empty(), "{}\n{report}", failures.join("\n"));
    let bytes = build(root, &output, script, "scope_placement_diagnostics", true)?;
    let (failures, report) = audit(root, &bytes)?;
    fs::write(output.join("placement-diagnostics.txt"), &report)?;
    assert!(failures.is_empty(), "{}\n{report}", failures.join("\n"));
    // Linked negative twins prove the gate checks final addresses. Each moves
    // one whole category outside SRAM while leaving the source attributes.
    for (section, expected) in [
        (".rwtext :", "cached code"),
        (".critical.rodata :", "cached data"),
        (".critical.data :", "cached data"),
        (".scope.stack (NOLOAD)", "cached scope stack"),
        (".scope.backing :", "cached scope backing"),
    ] {
        let changed = script
            .lines()
            .map(|line| {
                if line.contains(section) {
                    line.replace("> SRAM", "> CACHED")
                } else {
                    line.to_owned()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let variant = format!(
            "cached-{}",
            section
                .split_whitespace()
                .next()
                .ok_or("empty section")?
                .trim_start_matches('.')
        );
        let bytes = build(root, &output, &changed, &variant, true)?;
        let (failures, report) = audit(root, &bytes)?;
        fs::write(output.join(format!("{variant}.txt")), report)?;
        assert!(
            failures.iter().any(|failure| failure.contains(expected)),
            "negative linked twin was accepted: {variant}"
        );
    }
    Ok(())
}

/// Whether `range` lies inside one of the ranges the staged runtime entry
/// re-copies or zeroes at every reset.
fn reinitialized_at_reset(range: (u64, u64), reset_ranges: &[(u64, u64)]) -> bool {
    reset_ranges
        .iter()
        .any(|&(start, end)| start <= range.0 && range.1 <= end)
}

#[test]
fn records_outside_the_reset_ranges_are_rejected() {
    let ranges = [(0x100, 0x200), (0x300, 0x400)];
    assert!(reinitialized_at_reset((0x120, 0x180), &ranges));
    assert!(reinitialized_at_reset((0x300, 0x400), &ranges));
    // A `.noinit`-like or straddling placement fails the audit.
    assert!(!reinitialized_at_reset((0x500, 0x540), &ranges));
    assert!(!reinitialized_at_reset((0x1f0, 0x210), &ranges));
}
