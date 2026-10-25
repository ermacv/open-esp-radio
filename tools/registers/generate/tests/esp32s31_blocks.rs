//! The ESP32-S31 chip root and its block types (`chips/esp32s31/registers/
//! {chip.toml,types/,layouts/}`) load through the block-type loader with
//! every evidence catalog beside them: the vendor blocks the import spec
//! names, the curated blocks no vendor header describes, the analog-I2C
//! space and the placed memory layouts. The ownership policy
//! (`policy.toml`) loads over that root.

use std::path::{Path, PathBuf};

use oer_registers_model::{
    RegisterEvidenceSet,
    chip_import::ImportSpec,
    chip_model::{ChipModel, Location},
    chip_policy::{ChipPolicy, Claim, Holder},
};

fn registers() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/esp32s31/registers")
}

fn evidence() -> RegisterEvidenceSet {
    let mut catalogs = std::fs::read_dir(registers().join("evidence"))
        .expect("the evidence directory reads")
        .map(|entry| entry.expect("an evidence entry reads").path())
        .collect::<Vec<_>>();
    catalogs.sort();
    RegisterEvidenceSet::load_all(&catalogs).expect("the evidence catalogs load")
}

fn model() -> ChipModel {
    ChipModel::load(&registers().join("chip.toml"), &evidence()).expect("the chip root loads")
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

#[test]
fn unestablished_facts_stay_unestablished() {
    use oer_registers_contracts::{FactAccuracy, FactCompleteness, FactProvenance, NameOrigin};
    use oer_registers_model::chip_model::FieldClass;

    let model = model();
    // A curated name without its own source cites the record that found the
    // entity, its register's or its slice's; its behavior and name are
    // reconstructed, so accuracy and completeness stay unknown.
    let found_by = |identity: &str, source: &str| {
        let review = model
            .register(identity)
            .map(|register| &register.review)
            .or_else(|| model.field(identity).map(|field| &field.review))
            .expect("the entity exists");
        assert!(review.sources.iter().any(|s| s == source), "{identity}");
        assert_eq!(
            review.provenance,
            Some(FactProvenance::Observed),
            "{identity}"
        );
        assert_eq!(review.accuracy, Some(FactAccuracy::Unknown), "{identity}");
        assert_eq!(
            review.completeness,
            Some(FactCompleteness::Unknown),
            "{identity}"
        );
        assert_eq!(review.naming, Some(NameOrigin::Descriptive), "{identity}");
    };
    found_by(
        "WIFI_MAC.CTS_INTERRUPT.VALUE",
        "BLOB_LIBPP_PP_DEBUG_HW_STATISTICS",
    );
    found_by("WIFI_MAC.BITMAP_HIGH", "BLOB_LIBPP_DBG_READ_INTERNAL_TXBA");
    found_by(
        "WIFI_MAC.BITMAP_HIGH.BITMAP",
        "BLOB_LIBPP_DBG_READ_INTERNAL_TXBA",
    );
    let class = |identity: &str| model.field(identity).expect("the field exists").class;
    // No source states the access of this analog field.
    assert_eq!(
        class("MPLL.I2C_MPLL_IR_CAL_EXT_CAP.I2C_MPLL_IR_CAL_EXT_CAP"),
        FieldClass::Unknown
    );
    // The header's class stands against an observed read-modify-write.
    for field in [
        "TIE_HIGH_XPD_BB_I2C",
        "TIE_HIGH_XPD_XTALX2",
        "TIE_HIGH_XPD_XTAL",
    ] {
        assert_eq!(
            class(&format!("PMU.IMM_HP_CK_POWER_0.{field}")),
            FieldClass::WriteTrigger
        );
    }
    // A header-reserved bit the ROM writes and never reads.
    for host in ["I2C0_CTRL", "I2C1_CTRL"] {
        let field = model
            .field(&format!("I2C_ANA_MST.{host}.START_OR_RESET"))
            .expect("the reset bit exists");
        assert_eq!(field.class, FieldClass::WriteOnly);
        assert_eq!(field.review.sources, ["ROM_REV0_PHY_I2C"]);
    }
}

#[test]
fn the_esp32s31_policy_loads_over_the_root() {
    use oer_registers_contracts::FactAccuracy;
    use oer_registers_model::chip_model::FieldClass;

    let evidence = evidence();
    let model =
        ChipModel::load(&registers().join("chip.toml"), &evidence).expect("the chip root loads");
    let policy = ChipPolicy::load(&registers().join("policy.toml"), &model, &evidence)
        .unwrap_or_else(|error| panic!("the policy loads: {error}"));
    let owner = |transaction: &str| {
        let id = policy
            .owner(transaction)
            .unwrap_or_else(|| panic!("{transaction} has an owner"));
        let partition = &policy.partitions()[id.partition];
        let part = policy.part(id).expect("the owner is a part");
        (partition.name.as_str(), part.name.as_str())
    };
    // A status-to-clear snapshot belongs to the part holding its words.
    assert_eq!(
        owner("mac_power_interrupt"),
        ("WifiInterruptPeripherals", "WifiMacPowerInterrupt")
    );
    // The interrupt handler's side holds the event word.
    let Some(Claim::Whole(Holder::Part(irq))) = policy.claim("IEEE802154.EVENT_STATUS") else {
        panic!("a part holds the IEEE 802.15.4 event word");
    };
    assert!(policy.part(*irq).expect("the holder is a part").irq_side);
    // A former header subfield modify sets bits of the header's fields.
    assert_eq!(
        owner("prepare_modem_syscon_clock_map"),
        ("RadioPhyPeripherals", "ModemSysconRadio")
    );
    // The split trigger word: each channel's owner holds its own bits.
    let Some(Claim::Fields(fields)) = policy.claim("MODEM_ETM.CHANNEL_ENABLE_SET") else {
        panic!("the channel-enable set word is split by field");
    };
    let holder = |field: &str| match fields[field] {
        Holder::Part(id) => policy.part(id).expect("a part").name.as_str(),
        Holder::Unclaimed(_) => panic!("{field} is held"),
    };
    assert_eq!(holder("CH0"), "ModemEtmIeee802154");
    assert_eq!(holder("CH2"), "ModemEtmBluetoothPhy");
    assert_eq!(holder("CH7"), "ModemEtmBluetooth");
    // The counter latch every context reads stays shared: software only
    // reads its valid flag, which the update strobe restarts.
    let Some(Claim::Whole(Holder::Part(counter))) = policy.claim("SYSTIMER.UNIT0_OP") else {
        panic!("a part holds the counter latch");
    };
    assert!(policy.part(*counter).expect("a part").shared);
    assert_eq!(
        model
            .field("SYSTIMER.UNIT0_OP.TIMER_UNIT0_VALUE_VALID")
            .expect("the flag exists")
            .class,
        FieldClass::ReadOnly
    );
    // The ROM writes the read mask's high byte; that it is stored is not
    // established.
    let high = model
        .field("I2C_ANA_MST.ANA_CONF1.ANA_STATUS1")
        .expect("the high byte exists");
    assert_eq!(high.class, FieldClass::Plain);
    assert!(high.review.sources.iter().any(|s| s == "ROM_REV0_PHY_I2C"));
    assert_eq!(high.review.accuracy, Some(FactAccuracy::Unknown));
    // A vendor block without a driver is unclaimed, not a catch-all part.
    assert!(matches!(
        policy.claim("UART0.FIFO"),
        Some(Claim::Whole(Holder::Unclaimed(_)))
    ));
}
