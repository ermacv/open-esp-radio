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
    chip_model::{ChipModel, ControlKind, FieldClass, Location},
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
    // The counter latch every context reads stays shared: its update
    // strobe writes zero into the write-one-to-clear valid flag.
    let Some(Claim::Whole(Holder::Part(counter))) = policy.claim("SYSTIMER.UNIT0_OP") else {
        panic!("a part holds the counter latch");
    };
    assert!(policy.part(*counter).expect("a part").shared);
    let class = |identity: &str| model.field(identity).expect("the field exists").class;
    assert_eq!(
        class("SYSTIMER.UNIT0_OP.TIMER_UNIT0_VALUE_VALID"),
        FieldClass::WriteOneToClear
    );
    // The ROM stores the read mask whole; the header's read-only high byte
    // takes the fixed ones of that store and no setter.
    assert_eq!(
        class("I2C_ANA_MST.ANA_CONF1.ANA_STATUS1"),
        FieldClass::ReadOnly
    );
    let file = policy.file();
    let mask = file
        .zero_based_field_writes
        .iter()
        .find(|write| write.name == "publish_phy_i2c_read_mask")
        .expect("the read-mask publication exists");
    assert_eq!(mask.fields, ["I2C_ANA_MST.ANA_CONF1.ANA_CONF1"]);
    assert_eq!(
        mask.whole_word_bits.as_ref().map(|bits| bits.value),
        Some(0xff00_0000)
    );
    // The TX completion acknowledgement writes its sample and the selected
    // queue's consumed event.
    let completion = file
        .interrupt_snapshots
        .iter()
        .find(|snapshot| snapshot.name == "acknowledge_mac_tx_completion")
        .expect("the completion snapshot exists");
    assert_eq!(completion.consumed_fields.len(), 4);
    assert_eq!(
        owner("acknowledge_mac_tx_completion"),
        ("WifiMacPeripherals", "WifiMacTxCommon")
    );
    // Words no transaction reaches are unclaimed, not an owner's.
    assert!(matches!(
        policy.claim("MODEM_ETM.WORD_0144_OPAQUE"),
        Some(Claim::Whole(Holder::Unclaimed(_)))
    ));
    // A vendor block without a driver is unclaimed, not a catch-all part.
    assert!(matches!(
        policy.claim("UART0.FIFO"),
        Some(Claim::Whole(Holder::Unclaimed(_)))
    ));
}

#[test]
fn timg0_keeps_its_bus_timer_watchdog_gates_and_group_reset() {
    let model = model();
    for (kind, role, field) in [
        (ControlKind::Clock, "bus", "REG_TIMERGRP0_APB_CLK_EN"),
        (ControlKind::Clock, "t0", "REG_TIMERGRP0_T0_CLK_EN"),
        (ControlKind::Clock, "t1", "REG_TIMERGRP0_T1_CLK_EN"),
        (ControlKind::Clock, "wdt", "REG_TIMERGRP0_WDT_CLK_EN"),
        (ControlKind::Reset, "group", "REG_TIMERGRP0_RST_EN"),
    ] {
        let link = model
            .controls()
            .iter()
            .find(|link| link.instance == "TIMG0" && link.kind == kind && link.role == role)
            .expect("TIMG0 has the sourced control role");
        assert_eq!(
            link.field.identity,
            format!("HP_SYS_CLKRST.TIMERGRP0_CTRL0.{field}")
        );
        assert!(!link.sources.is_empty());
    }
    let timer_mux = model
        .controls()
        .iter()
        .find(|link| link.instance == "TIMG0" && link.kind == ControlKind::Mux && link.role == "t0")
        .expect("TIMG0 has its T0 source selector");
    assert_eq!(
        timer_mux
            .inputs
            .iter()
            .map(|(code, name)| (*code, name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "XTAL"), (1, "RC_FAST"), (2, "REF_F80M")]
    );
    // The LL's XTAL assertion never writes a selector encoding, and its
    // default-clock comment names P4. Neither establishes an S31 input.
    for instance in ["TIMG0", "TIMG1"] {
        assert!(!model.controls().iter().any(|link| {
            link.instance == instance && link.kind == ControlKind::Mux && link.role == "wdt"
        }));
    }
}

#[test]
fn root_controls_keep_policy_dependencies_and_unestablished_relations_absent() {
    let model = model();
    assert!(
        model
            .gate_groups()
            .iter()
            .all(|group| group.name != "SOC_PLL_SOURCE_CG")
    );
    let pad_gate = "HP_SYS_CLKRST.MSPI_PAD_CTRL0.REG_MSPI_PAD_CTRL_APB_CLK_EN";
    let pad_users = model
        .controls()
        .iter()
        .filter(|link| link.field.identity == pad_gate)
        .map(|link| (link.instance.as_str(), link.kind, link.role.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(pad_users, [("IOMUX_MSPI_PIN", ControlKind::Clock, "bus")]);
    assert!(
        model
            .shared_controls()
            .iter()
            .all(|shared| shared.field != pad_gate)
    );
    assert!(!model.controls().iter().any(|link| {
        link.instance == "EFUSE" && link.kind == ControlKind::Clock && link.role == "core"
    }));

    let spi1 = model
        .controls()
        .iter()
        .filter(|link| link.instance == "SPI1")
        .collect::<Vec<_>>();
    assert_eq!(spi1.len(), 3);
    for link in spi1 {
        assert_eq!(link.kind, ControlKind::Clock);
        assert!(
            link.sources
                .iter()
                .any(|source| source == "ESP_IDF_4D59230D_S31_SPIMEM_FLASH_LL_H_L810")
        );
        let shared = model
            .shared_controls()
            .iter()
            .find(|shared| shared.field == link.field.identity)
            .expect("the established FLASH gate has explicit users");
        assert_eq!(shared.instances, ["SPI0", "SPI1"]);
    }
}

#[test]
fn lp_watchdog_slow_clock_link_cites_its_consumer_and_all_established_inputs() {
    let model = model();
    let mux = model
        .controls()
        .iter()
        .find(|link| {
            link.instance == "LP_WDT" && link.kind == ControlKind::Mux && link.role == "slow"
        })
        .expect("RWDT timeout ticks use the RTC slow-clock frequency");
    assert!(
        mux.sources
            .iter()
            .any(|source| source == "ESP_IDF_4D59230D_S31_BOOTLOADER_INIT_C_L95")
    );
    assert_eq!(
        mux.inputs
            .iter()
            .map(|(code, name)| (*code, name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "RC_SLOW"), (1, "XTAL32K"), (3, "OSC_SLOW")]
    );
}

#[test]
fn bluetooth_and_ieee802154_share_the_bt_baseband_gate() {
    let model = model();
    let common = model
        .shared_controls()
        .iter()
        .find(|control| control.name == "BT_I154_COMMON_BB")
        .expect("the common BT/IEEE 802.15.4 baseband gate is explicit");
    assert_eq!(common.field, "MODEM_SYSCON.CLK_CONF1.CLK_BTBB_EN");
    assert_eq!(common.instances, ["BT_BB", "IEEE802154"]);
    for instance in &common.instances {
        assert!(model.controls().iter().any(|link| {
            &link.instance == instance
                && link.kind == ControlKind::Clock
                && link.role == "baseband"
                && link.field.identity == common.field
        }));
    }
}

#[test]
fn modem_power_and_coexistence_keep_pulse_resets_and_the_common_32k_mux() {
    let model = model();
    for (instance, role, field) in [
        ("MODEM_PWR", "wifi-power", "RST_WIFIPWR"),
        ("MODEM_PWR", "lp-timer", "RST_LP_TIMER"),
        ("COEX", "block", "RST_COEX"),
        ("I2C_ANA_MST", "master", "RST_I2C_MST"),
    ] {
        let reset = model
            .controls()
            .iter()
            .find(|link| {
                link.instance == instance && link.kind == ControlKind::Reset && link.role == role
            })
            .expect("the LPCON pulse reset is sourced");
        assert_eq!(
            reset.field.identity,
            format!("MODEM_LPCON.RST_CONF.{field}")
        );
        assert_eq!(reset.field.class, FieldClass::WriteOnly);
    }
    let common = model
        .shared_controls()
        .iter()
        .find(|control| control.name == "MODEM_32K")
        .expect("the 32 kHz selector is shared");
    assert_eq!(common.instances, ["COEX", "MODEM_PWR"]);
    for instance in &common.instances {
        let mux = model
            .controls()
            .iter()
            .find(|link| {
                &link.instance == instance
                    && link.kind == ControlKind::Mux
                    && link.field.identity == common.field
            })
            .expect("each user links to the common selector");
        assert_eq!(
            mux.inputs
                .iter()
                .map(|(code, name)| (*code, name.as_str()))
                .collect::<Vec<_>>(),
            [(0, "XTAL32K"), (1, "RC32K"), (2, "EXT32K")]
        );
    }
}

#[test]
fn the_thin_renderer_renders_the_esp32s31_root_and_policy() {
    let evidence = evidence();
    let model =
        ChipModel::load(&registers().join("chip.toml"), &evidence).expect("the chip root loads");
    let policy = ChipPolicy::load(&registers().join("policy.toml"), &model, &evidence)
        .unwrap_or_else(|error| panic!("the policy loads: {error}"));
    let rendered = oer_registers_model::chip_render::render(&model, &policy)
        .unwrap_or_else(|error| panic!("the thin PAC renders: {error}"));
    // The interrupt handlers' parts name their instances' one source.
    for (part, source) in [("Ieee802154MacIrq", 132), ("UsbSerialJtagIrq", 2)] {
        let item = format!(
            "impl<P: oer_mmio::Placement> oer_mmio::InterruptSource for {part}<P> {{\nconst SOURCE: u16 = {source};"
        );
        assert!(rendered.contains(&item), "{item}");
    }
    // The irreducible acknowledgements render; ordinary transactions do not.
    for item in [
        "pub fn mac_interrupt_clear(&mut self)",
        "pub fn sample_mac_interrupt(&self)",
        "pub fn acknowledge_acknowledge_mac_tx_completion(",
        "pub fn frequency_parameter_0(&mut self)",
    ] {
        assert!(rendered.contains(item), "{item}");
    }
    assert!(!rendered.contains("fn configure_bluetooth_cte_six_buffer_limit"));
}
