#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
//! The chip facts that two sources state: each pair is held equal by value,
//! so the one that changes alone fails here.
//!
//! The sources are a chip's resolved facts (`oer-espressif-facts`: its
//! layers from the ISA to the board), its `LAYOUT` (the linker scripts and
//! the bootstrap read it), the stage-two header and the zeroed sections of
//! `oer-espressif-staged-layout`, the `two-harts` feature of the images and
//! the S31 reset-cause table of `oer-esp32s31-knowledge`.

use std::{
    collections::BTreeSet,
    fs,
    mem::offset_of,
    path::{Path, PathBuf},
};

use oer_chip_profile::Profile;
use oer_espressif_facts::{Facts, Region};
use oer_espressif_staged_layout::{
    Layout, Region as LayoutRegion,
    memory::BOOTSTRAP_PSRAM_BYTES,
    stage_two::{self, Header},
    zeroed::RUNTIME_ZEROED_REGIONS,
};

const CHIPS: [(&str, Layout); 2] = [
    ("esp32s31", oer_esp32s31_layout::LAYOUT),
    ("esp32c5", oer_esp32c5_layout::LAYOUT),
];

fn profile(id: &str) -> Facts {
    Profile::tracked(id).unwrap().facts
}

fn region(region: LayoutRegion) -> Region {
    Region {
        origin: region.origin,
        length: region.length,
    }
}

/// What differs between a chip's `chip.toml [memory]`, `[placement]` and
/// `[interrupts]` and its `LAYOUT`.
fn memory_mismatches(profile: &Facts, layout: &Layout) -> Vec<String> {
    let mut found = Vec::new();
    let mut check = |name: &str, toml: Option<Region>, layout: Region| {
        if toml != Some(layout) {
            found.push(format!("{name}: chip.toml {toml:x?}, LAYOUT {layout:x?}"));
        }
    };
    let memory = profile.memory.as_ref().expect("a staged chip has [memory]");
    check("sram", Some(memory.sram), region(layout.sram));
    check(
        "flash-xip",
        Some(memory.flash_xip),
        region(layout.flash_xip),
    );
    check("psram", memory.psram, region(layout.psram));
    check(
        "runtime-psram",
        memory.runtime_psram,
        region(layout.runtime_psram()),
    );

    let placement = profile
        .placement
        .as_ref()
        .expect("a staged chip has [placement]");
    let stack = placement
        .range
        .iter()
        .find(|rule| rule.range == ["_stack_end", "_stack_start"])
        .and_then(|rule| rule.bytes);
    if stack != Some(u64::from(layout.cpu0_task_stack_bytes)) {
        found.push(format!(
            "cpu0 task stack: chip.toml {stack:?}, LAYOUT {}",
            layout.cpu0_task_stack_bytes
        ));
    }

    let interrupts = profile
        .interrupts
        .as_ref()
        .expect("a staged chip has [interrupts]");
    if interrupts.irq_stack_bytes != layout.irq_stack_bytes {
        found.push(format!(
            "irq stack: chip.toml [interrupts] {}, LAYOUT {}",
            interrupts.irq_stack_bytes, layout.irq_stack_bytes
        ));
    }
    for hart in &interrupts.harts {
        let names = [
            format!("__runtime_cpu{hart}_irq_stack_bottom"),
            format!("__runtime_cpu{hart}_irq_stack_top"),
        ];
        let maximum = placement
            .range
            .iter()
            .find(|rule| rule.range == names)
            .and_then(|rule| rule.maximum_bytes);
        if maximum != Some(u64::from(layout.irq_stack_bytes)) {
            found.push(format!(
                "irq stack of hart {hart}: chip.toml [placement] {maximum:?}, LAYOUT {}",
                layout.irq_stack_bytes
            ));
        }
    }
    found
}

#[test]
fn memory_map_is_the_layout() {
    for (id, layout) in CHIPS {
        assert_eq!(
            memory_mismatches(&profile(id), &layout),
            Vec::<String>::new(),
            "{id}"
        );
    }
}

#[test]
fn a_changed_memory_fact_on_either_side_is_reported() {
    for (id, layout) in CHIPS {
        let base = profile(id);
        let moved = |mutate: &dyn Fn(&mut Facts)| {
            let mut profile = base.clone();
            mutate(&mut profile);
            memory_mismatches(&profile, &layout)
        };
        assert!(moved(&|p| p.memory.as_mut().unwrap().sram.length += 16).len() == 1);
        assert!(moved(&|p| p.memory.as_mut().unwrap().flash_xip.origin += 16).len() == 1);
        assert!(
            !moved(&|p| p.memory.as_mut().unwrap().psram.as_mut().unwrap().length += 16).is_empty()
        );
        assert!(moved(&|p| p.memory.as_mut().unwrap().runtime_psram = None).len() == 1);
        assert!(!moved(&|p| p.interrupts.as_mut().unwrap().irq_stack_bytes += 16).is_empty());

        let mut other = layout;
        other.sram = LayoutRegion::new(layout.sram.origin, layout.sram.length + 16);
        other.cpu0_task_stack_bytes += 16;
        other.irq_stack_bytes += 16;
        assert!(memory_mismatches(&base, &other).len() >= 3, "{id}");
    }
}

/// What differs between `[staged]` and the code that states the same facts.
fn staged_mismatches(profile: &Facts) -> Vec<String> {
    let mut found = Vec::new();
    let staged = profile.staged.as_ref().expect("a staged chip has [staged]");

    let code: BTreeSet<&str> = RUNTIME_ZEROED_REGIONS
        .iter()
        .flat_map(|region| region.inputs.iter().copied())
        .collect();
    let toml: BTreeSet<&str> = staged.zeroed_inputs.iter().map(String::as_str).collect();
    if code != toml {
        found.push(format!(
            "zeroed inputs: only in chip.toml {:?}, only in zeroed.rs {:?}",
            toml.difference(&code).collect::<Vec<_>>(),
            code.difference(&toml).collect::<Vec<_>>()
        ));
    }

    if staged.bootstrap_psram_bytes != BOOTSTRAP_PSRAM_BYTES {
        found.push(format!(
            "bootstrap PSRAM page: facts {:#x}, memory.rs {BOOTSTRAP_PSRAM_BYTES:#x}",
            staged.bootstrap_psram_bytes
        ));
    }

    let header = &staged.stage_two;
    for (name, toml, code) in [
        ("magic", header.magic, stage_two::MAGIC),
        ("abi-version", header.abi_version, stage_two::ABI_VERSION),
        (
            "header-bytes",
            header.header_bytes,
            stage_two::HEADER_BYTES as u32,
        ),
        (
            "magic-offset",
            header.magic_offset,
            offset_of!(Header, magic) as u32,
        ),
        (
            "abi-version-offset",
            header.abi_version_offset,
            offset_of!(Header, abi_version) as u32,
        ),
        (
            "header-size-offset",
            header.header_size_offset,
            offset_of!(Header, header_size) as u32,
        ),
        (
            "crc-offset",
            header.crc_offset,
            stage_two::CRC_OFFSET as u32,
        ),
    ] {
        if toml != code {
            found.push(format!(
                "stage-two {name}: chip.toml {toml:#x}, stage_two.rs {code:#x}"
            ));
        }
    }
    found
}

#[test]
fn staged_boot_contract_is_the_code() {
    for (id, _) in CHIPS {
        assert_eq!(
            staged_mismatches(&profile(id)),
            Vec::<String>::new(),
            "{id}"
        );
    }
}

#[test]
fn a_changed_staged_fact_on_either_side_is_reported() {
    let base = profile("esp32s31");
    let changed = |mutate: &dyn Fn(&mut Facts)| {
        let mut profile = base.clone();
        mutate(&mut profile);
        staged_mismatches(&profile)
    };
    assert_eq!(
        changed(&|p| p
            .staged
            .as_mut()
            .unwrap()
            .zeroed_inputs
            .pop()
            .map(drop)
            .unwrap())
        .len(),
        1
    );
    assert_eq!(
        changed(&|p| p
            .staged
            .as_mut()
            .unwrap()
            .zeroed_inputs
            .push(".extra.bss".into()))
        .len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.magic ^= 1).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().bootstrap_psram_bytes += 4).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.abi_version += 1).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.header_bytes += 4).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.crc_offset += 4).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.magic_offset += 4).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.abi_version_offset += 4).len(),
        1
    );
    assert_eq!(
        changed(&|p| p.staged.as_mut().unwrap().stage_two.header_size_offset += 4).len(),
        1
    );
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

/// Every `Cargo.toml` below `directory`.
fn manifests(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            manifests(&path, found);
        } else if path.file_name().is_some_and(|name| name == "Cargo.toml") {
            found.push(path);
        }
    }
}

/// The chip feature and whether `two-harts` is on, for each image manifest
/// that depends on the staged runtime.
fn runtime_features() -> Vec<(PathBuf, String, bool)> {
    let mut manifests_found = Vec::new();
    manifests(&repository_root().join("firmware"), &mut manifests_found);
    let mut images = Vec::new();
    for path in manifests_found {
        let manifest: toml::Table = fs::read_to_string(&path).unwrap().parse().unwrap();
        let Some(runtime) = manifest
            .get("dependencies")
            .and_then(|dependencies| dependencies.get("oer-espressif-staged-runtime"))
            .and_then(|runtime| runtime.get("features"))
            .and_then(|features| features.as_array())
        else {
            continue;
        };
        let features: Vec<&str> = runtime
            .iter()
            .filter_map(|feature| feature.as_str())
            .collect();
        let chip = features
            .iter()
            .find(|feature| CHIPS.iter().any(|(id, _)| id == *feature))
            .unwrap_or_else(|| panic!("{}: no chip feature", path.display()));
        images.push((path, chip.to_string(), features.contains(&"two-harts")));
    }
    images
}

/// What differs between a chip's core count, `LAYOUT.harts`, the interrupt
/// contract's harts and the `two-harts` feature of its images.
fn core_mismatches(
    profile: &Facts,
    layout: &Layout,
    images: &[(PathBuf, String, bool)],
) -> Vec<String> {
    let mut found = Vec::new();
    let cores = u32::from(profile.properties.cores);
    if layout.harts != cores {
        found.push(format!(
            "cores: chip.toml {cores}, LAYOUT.harts {}",
            layout.harts
        ));
    }
    let harts = &profile.interrupts.as_ref().unwrap().harts;
    if *harts != (0..cores).collect::<Vec<_>>() {
        found.push(format!("[interrupts] harts {harts:?} for {cores} cores"));
    }
    for (path, chip, two_harts) in images {
        if *chip == profile.id && *two_harts != (cores > 1) {
            found.push(format!(
                "{}: two-harts is {two_harts} on a chip with {cores} cores",
                path.display()
            ));
        }
    }
    found
}

#[test]
fn cores_harts_and_the_two_harts_feature_agree() {
    let images = runtime_features();
    for (id, layout) in CHIPS {
        assert!(
            images.iter().any(|(_, chip, _)| chip == id),
            "{id}: no image uses the staged runtime"
        );
        assert_eq!(
            core_mismatches(&profile(id), &layout, &images),
            Vec::<String>::new(),
            "{id}"
        );
    }
}

#[test]
fn a_changed_core_fact_on_any_side_is_reported() {
    let (id, layout) = CHIPS[0];
    let base = profile(id);
    let images = vec![(PathBuf::from("image"), id.to_string(), true)];
    assert!(core_mismatches(&base, &layout, &images).is_empty());

    let mut cores = base.clone();
    cores.properties.cores = 1;
    assert!(core_mismatches(&cores, &layout, &images).len() >= 2);

    let mut harts = base.clone();
    harts.interrupts.as_mut().unwrap().harts.pop();
    assert_eq!(core_mismatches(&harts, &layout, &images).len(), 1);

    let mut one_hart = layout;
    one_hart.harts = 1;
    assert_eq!(core_mismatches(&base, &one_hart, &images).len(), 1);

    let feature = vec![(PathBuf::from("image"), id.to_string(), false)];
    assert_eq!(core_mismatches(&base, &layout, &feature).len(), 1);
}

#[test]
fn s31_power_on_code_is_the_reset_cause_table() {
    use oer_esp32s31_knowledge::reset_cause::ResetCause;
    let reset_cause = profile("esp32s31")
        .reset_cause
        .expect("the S31 stand reads the reset cause");
    let power_on = u8::try_from(reset_cause.power_on).expect("a reset-cause code fits a byte");
    assert_eq!(ResetCause::from_code(power_on), ResetCause::ChipPowerOn);
}
