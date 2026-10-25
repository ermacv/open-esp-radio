//! The ESP32-S31 chip root and its block types (`chips/esp32s31/registers/
//! {chip.toml,types/,layouts/}`) load through the block-type loader with
//! every evidence catalog beside them: the vendor blocks the import spec
//! names, the curated blocks no vendor header describes, the analog-I2C
//! space and the placed memory layouts.

use std::path::{Path, PathBuf};

use oer_registers_model::{
    RegisterEvidenceSet,
    chip_import::ImportSpec,
    chip_model::{ChipModel, Location},
};

fn registers() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/esp32s31/registers")
}

fn model() -> ChipModel {
    let registers = registers();
    let mut catalogs = std::fs::read_dir(registers.join("evidence"))
        .expect("the evidence directory reads")
        .map(|entry| entry.expect("an evidence entry reads").path())
        .collect::<Vec<_>>();
    catalogs.sort();
    let evidence = RegisterEvidenceSet::load_all(&catalogs).expect("the evidence catalogs load");
    ChipModel::load(&registers.join("chip.toml"), &evidence).expect("the chip root loads")
}

#[test]
fn the_esp32s31_root_loads_with_every_spec_block() {
    let registers = registers();
    let spec = ImportSpec::parse(
        &std::fs::read_to_string(registers.join("import.toml")).expect("the import spec reads"),
    )
    .expect("the import spec parses");
    let model = model();
    for block in &spec.types {
        assert!(
            model.types().contains_key(&block.name),
            "type {}",
            block.name
        );
        for instance in &block.instances {
            let placed = model
                .instances()
                .iter()
                .find(|placed| placed.name == instance.name)
                .unwrap_or_else(|| panic!("the root places {}", instance.name));
            assert_eq!(placed.block_type, block.name);
        }
    }
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
    let model = model();
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

#[test]
fn the_curated_radio_blocks_are_placed_and_own_their_interrupts() {
    let model = model();
    for (name, base) in [
        ("FE", 0x2010_0000),
        ("BT_MAC", 0x2010_1000),
        ("BT_BB", 0x2010_2000),
        ("IEEE802154", 0x2010_3000),
        ("WIFI_MAC", 0x2010_4000),
        ("WIFI_BB", 0x2010_7000),
        ("MODEM_PWR", 0x2010_D000),
        ("COEX", 0x2010_F400),
        ("CLIC", 0x1080_0000),
    ] {
        let instance = model
            .instances()
            .iter()
            .find(|instance| instance.name == name)
            .unwrap_or_else(|| panic!("the root places {name}"));
        assert_eq!(instance.base, base, "{name}");
        assert!(instance.extent.partial, "{name} is known only in part");
    }
    for (source, instance, output) in [
        (120, "WIFI_MAC", Some("PRIMARY")),
        (122, "MODEM_PWR", Some("WIFI_PWR")),
        (124, "BT_MAC", Some("PRIMARY")),
        (133, "BT_MAC", Some("NRT")),
        (132, "IEEE802154", None),
    ] {
        let bound = model
            .interrupts()
            .iter()
            .find(|interrupt| interrupt.source == source)
            .unwrap_or_else(|| panic!("source {source} is bound"));
        assert_eq!(bound.instance, instance);
        assert_eq!(bound.output.as_deref(), output);
    }
}

#[test]
fn curated_registers_keep_their_absolute_words() {
    let model = model();
    let address = |identity: &str| {
        model
            .registers()
            .iter()
            .find(|register| register.identity == identity)
            .map(|register| register.location)
            .unwrap_or_else(|| panic!("{identity} is expanded"))
    };
    // A curated slice merged into its vendor block keeps the vendor name.
    assert_eq!(
        address("PMU.HP_ACTIVE_HP_CK_POWER"),
        Location::Mmio {
            address: 0x2070_401C
        }
    );
    // A positional opaque name takes its offset from the block's base.
    assert_eq!(
        address("WIFI_BB_BRX.WORD_0010_OPAQUE"),
        Location::Mmio {
            address: 0x2010_8010
        }
    );
    assert!(
        model
            .analog_instances()
            .iter()
            .any(|instance| instance.name == "BBPLL")
    );
}
