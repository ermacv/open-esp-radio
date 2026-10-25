//! The ESP32-S31 block types and chip root `cargo registers import` wrote
//! (`chips/esp32s31/registers/{chip.toml,types/}`) load through the
//! block-type loader with the evidence catalog written beside them, and
//! hold every block the import spec names.

use std::path::Path;

use oer_registers_model::{RegisterEvidenceSet, chip_import::ImportSpec, chip_model::ChipModel};

#[test]
fn the_imported_esp32s31_root_loads_with_every_spec_block() {
    let registers = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/esp32s31/registers");
    let spec = ImportSpec::parse(
        &std::fs::read_to_string(registers.join("import.toml")).expect("the import spec reads"),
    )
    .expect("the import spec parses");
    let evidence = RegisterEvidenceSet::load_all(&[registers.join(&spec.evidence)])
        .expect("the imported evidence catalog loads");
    let model = ChipModel::load(&registers.join(&spec.root), &evidence)
        .expect("the imported chip root loads");

    let types = model.types().keys().cloned().collect::<Vec<_>>();
    let mut expected = spec
        .types
        .iter()
        .map(|block| block.name.clone())
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(types, expected);
    for block in &spec.types {
        for instance in &block.instances {
            let placed = model
                .instances()
                .iter()
                .find(|placed| placed.name == instance.name)
                .unwrap_or_else(|| panic!("the root places {}", instance.name));
            assert_eq!(placed.block_type, block.name);
        }
    }
    assert!(!model.interrupts().is_empty());
    let mut covered = model
        .interrupts()
        .iter()
        .map(|source| u32::from(source.source))
        .chain(
            model
                .unbound_interrupts()
                .iter()
                .map(|source| u32::from(source.source)),
        )
        .collect::<Vec<_>>();
    covered.sort();
    assert_eq!(
        covered,
        (0..model.interrupt_sources().count).collect::<Vec<_>>()
    );
    assert!(
        model
            .unbound_interrupts()
            .iter()
            .all(|source| !source.reason.trim().is_empty())
    );
}

#[test]
fn the_existing_uart0_contract_consumer_has_an_imported_interrupt_owner() {
    let registers = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/esp32s31/registers");
    let evidence = RegisterEvidenceSet::load_all(&[registers.join("evidence/imported.toml")])
        .expect("the imported evidence catalog loads");
    let model = ChipModel::load(&registers.join("chip.toml"), &evidence)
        .expect("the imported chip root loads");
    let uart = model
        .instances()
        .iter()
        .find(|instance| instance.name == "UART0")
        .expect("the PAC soc_tests UART0 consumer is in the import boundary");
    assert!(!model.types()[&uart.block_type].elements.is_empty());
    assert!(
        model
            .interrupts()
            .iter()
            .any(|source| source.name == "UART0" && source.instance == uart.name)
    );
}
