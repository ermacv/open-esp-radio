//! The import on synthetic SVDs and headers: the header winning, the
//! findings it records, instances of one type and variants, the overlay's
//! carried facts, the older header format, determinism and refusals.

use std::collections::BTreeMap;

use super::*;
use crate::chip_model::ChipModel;

type TestResult = Result<(), Box<dyn std::error::Error>>;

const REVISION: &str = "0123456789abcdef0123456789abcdef01234567";

/// The pinned files of a test, in memory.
struct Files {
    svd: String,
    idf: BTreeMap<String, String>,
}

fn pin(source: &str, path: &str) -> Pin {
    Pin {
        source: source.to_owned(),
        repository: format!("https://example.invalid/{source}"),
        revision: REVISION.to_owned(),
        path: path.to_owned(),
        sha256: "00".repeat(32),
    }
}

impl PinnedFiles for Files {
    fn svd(&self) -> Result<(String, Pin), ImportError> {
        Ok((self.svd.clone(), pin("vendor-svd", "chip.svd")))
    }

    fn idf(&self, path: &str) -> Result<(String, Pin), ImportError> {
        self.idf
            .get(path)
            .map(|text| (text.clone(), pin("esp-idf", path)))
            .ok_or_else(|| ImportError::new(path, "not pinned"))
    }
}

fn field(name: &str, offset: u32, width: u32) -> String {
    format!(
        "<field><name>{name}</name><bitOffset>{offset}</bitOffset><bitWidth>{width}</bitWidth><access>read-write</access></field>"
    )
}

fn register(name: &str, offset: u32, reset: u32, fields: &[String]) -> String {
    format!(
        "<register><name>{name}</name><addressOffset>{offset:#x}</addressOffset><size>32</size><resetValue>{reset:#x}</resetValue><fields>{}</fields></register>",
        fields.concat()
    )
}

fn peripheral(name: &str, base: u32, interrupts: &[(&str, u32)], registers: &[String]) -> String {
    let interrupts = interrupts
        .iter()
        .map(|(name, value)| {
            format!("<interrupt><name>{name}</name><value>{value}</value></interrupt>")
        })
        .collect::<String>();
    format!(
        "<peripheral><name>{name}</name><baseAddress>{base:#x}</baseAddress><addressBlock><offset>0</offset><size>0x10</size><usage>registers</usage></addressBlock>{interrupts}<registers>{}</registers></peripheral>",
        registers.concat()
    )
}

fn derived(name: &str, from: &str, base: u32, interrupts: &[(&str, u32)]) -> String {
    let interrupts = interrupts
        .iter()
        .map(|(name, value)| {
            format!("<interrupt><name>{name}</name><value>{value}</value></interrupt>")
        })
        .collect::<String>();
    format!(
        "<peripheral derivedFrom=\"{from}\"><name>{name}</name><baseAddress>{base:#x}</baseAddress>{interrupts}</peripheral>"
    )
}

fn device(peripherals: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><device schemaVersion=\"1.1\"><name>TEST</name><addressUnitBits>8</addressUnitBits><width>32</width><size>32</size><resetValue>0x0</resetValue><resetMask>0xFFFFFFFF</resetMask><peripherals>{}</peripherals></device>",
        peripherals.concat()
    )
}

const REG_BASE: &str = "#define DR_REG_TIMG0_BASE 0x1000\n#define DR_REG_TIMG1_BASE (DR_REG_TIMG0_BASE + 0x1000)\n#define DR_REG_MODEM_BASE 0x4000\n#define DR_REG_SYSCON_BASE (DR_REG_MODEM_BASE + 0x100)\n";

const INTERRUPTS: &str = "typedef enum {\n    ETS_ZERO_INTR_SOURCE = 0,\n    ETS_TG0_T0_INTR_SOURCE,\n    ETS_TIMERGRP1_T0_INTR_SOURCE,\n    ETS_MAX_INTR_SOURCE,\n} periph_interrupt_t;\n";

/// The timer group header: `CONF` with `EN` (R/W, default 1) and `GO`
/// (WT), a reserved lane, and `STAT` read-only.
const TIMG_H: &str = "\
/** TIMG_CONF_REG register
 *  Configuration.
 */
#define TIMG_CONF_REG(i) (REG_TIMG_BASE(i) + 0x0)
/** TIMG_EN : R/W; bitpos: [0]; default: 1;
 *  Enables the timer.
 */
/** TIMG_GO : WT; bitpos: [1]; default: 0;
 *  Starts the timer.
 */
/** TIMG_SPARE : R/W; bitpos: [7:4]; default: 0;
 *  Reserved
 */
/** TIMG_STAT_REG register
 *  Status.
 */
#define TIMG_STAT_REG(i) (REG_TIMG_BASE(i) + 0x4)
/** TIMG_COUNT : RO; bitpos: [15:0]; default:
 *  0;
 *  The count.
 */
";

fn timg_registers() -> Vec<String> {
    vec![
        register("CONF", 0, 0x1, &[field("EN", 0, 1), field("GO", 1, 1)]),
        register("STAT", 4, 0, &[field("COUNT", 0, 16)]),
    ]
}

fn files(svd: String, extra: &[(&str, &str)]) -> Files {
    let mut idf = BTreeMap::from([
        ("reg_base.h".to_owned(), REG_BASE.to_owned()),
        ("interrupts.h".to_owned(), INTERRUPTS.to_owned()),
        ("timg_reg.h".to_owned(), TIMG_H.to_owned()),
    ]);
    for (path, text) in extra {
        idf.insert((*path).to_owned(), (*text).to_owned());
    }
    Files { svd, idf }
}

fn spec(types: &str) -> ImportSpec {
    let text = format!(
        "schema = 1\nchip = \"testchip\"\nsvd = \"vendor-svd/chip.svd\"\nesp-idf = \"esp-idf\"\nroot = \"chip.toml\"\ntypes-directory = \"types\"\nevidence = \"evidence.toml\"\ninterrupts = \"interrupts.h\"\nbases = [\"reg_base.h\"]\n{types}"
    );
    ImportSpec::parse(&text).expect("the test spec parses")
}

const TIMG_TYPE: &str = "
[[types]]
name = \"TIMG\"
description = \"A timer group.\"
registers = \"timg_reg.h\"

[[types.instances]]
name = \"TIMG0\"
svd = \"TIMG0\"
base = \"DR_REG_TIMG0_BASE\"

[[types.instances]]
name = \"TIMG1\"
svd = \"TIMG1\"
base = \"DR_REG_TIMG1_BASE\"
interrupts = { TG1_T0 = \"TIMERGRP1_T0\" }
";

const TIMG_ONLY: &str = "
[[types]]
name = \"TIMG\"
description = \"A timer group.\"
registers = \"timg_reg.h\"

[[types.instances]]
name = \"TIMG0\"
svd = \"TIMG0\"
base = \"DR_REG_TIMG0_BASE\"
";

/// Write an output to a temporary directory and load its root.
fn load(output: &ImportOutput) -> Result<ChipModel, Box<dyn std::error::Error>> {
    let directory = tempfile::tempdir()?;
    std::fs::create_dir_all(directory.path().join("types"))?;
    for (path, text) in &output.types {
        std::fs::write(directory.path().join(path), text)?;
    }
    let root = directory.path().join("chip.toml");
    std::fs::write(&root, &output.root)?;
    let evidence = directory.path().join("evidence.toml");
    std::fs::write(&evidence, &output.evidence)?;
    let set = crate::RegisterEvidenceSet::load_all(&[evidence])?;
    Ok(ChipModel::load(&root, &set)?)
}

fn has(report: &Report, entity: &str, kind: FindingKind) -> bool {
    report
        .findings
        .iter()
        .any(|finding| finding.entity == entity && finding.kind == kind)
}

#[test]
fn derived_instances_share_one_type_and_load() -> TestResult {
    let svd = device(&[
        peripheral("TIMG0", 0x1000, &[("TG0_T0", 1)], &timg_registers()),
        derived("TIMG1", "TIMG0", 0x2000, &[("TG1_T0", 2)]),
    ]);
    let output = import(&spec(TIMG_TYPE), None, &files(svd, &[]))?;
    assert_eq!(output.types.keys().collect::<Vec<_>>(), ["types/timg.toml"]);
    let model = load(&output)?;
    assert_eq!(model.instances().len(), 2);
    let conf = model.register("TIMG1.CONF").expect("TIMG1 has CONF");
    assert_eq!(
        conf.reserved, 0xffff_fffc,
        "the header's reserved lane and undeclared bits"
    );
    let classes = conf
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.class))
        .collect::<Vec<_>>();
    assert_eq!(
        classes,
        [("EN", FieldClass::Plain), ("GO", FieldClass::WriteTrigger)]
    );
    // The header's name of source 2 wins over the SVD's; the default on
    // the comment's next line is read.
    let names = model
        .interrupts()
        .iter()
        .map(|interrupt| {
            (
                interrupt.name.as_str(),
                interrupt.source,
                interrupt.instance.as_str(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [("TG0_T0", 1, "TIMG0"), ("TIMERGRP1_T0", 2, "TIMG1")]
    );
    assert_eq!(model.interrupt_sources().count, 3);
    assert_eq!(model.unbound_interrupts().len(), 1);
    assert_eq!(model.unbound_interrupts()[0].name, "ZERO");
    assert!(has(&output.report, "TIMG1", FindingKind::Interrupt));
    assert!(
        !output
            .report
            .findings
            .iter()
            .any(|f| f.kind == FindingKind::Default)
    );
    Ok(())
}

#[test]
fn the_header_wins_and_every_disagreement_is_reported() -> TestResult {
    // The SVD: EN two bits wide, a field in the header's reserved lane, a
    // reset the header's default contradicts, no SPARE2 and a register the
    // header lacks.
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[],
        &[
            register("CONF", 0, 0x0, &[field("EN", 0, 2), field("GHOST", 5, 1)]),
            register("STAT", 4, 0, &[field("COUNT", 0, 16)]),
            register("EXTRA", 8, 0, &[field("VALUE", 0, 8)]),
        ],
    )]);
    let types = "
[[types]]
name = \"TIMG\"
description = \"A timer group.\"
registers = \"timg_reg.h\"

[[types.instances]]
name = \"TIMG0\"
svd = \"TIMG0\"
base = \"DR_REG_TIMG0_BASE\"
";
    let output = import(&spec(types), None, &files(svd, &[]))?;
    let report = &output.report;
    assert!(has(report, "TIMG0.CONF.EN", FindingKind::FieldDropped));
    assert!(has(report, "TIMG0.CONF.GHOST", FindingKind::FieldDropped));
    assert!(has(report, "TIMG0.CONF.EN", FindingKind::FieldAdded));
    assert!(has(report, "TIMG0.CONF.GO", FindingKind::FieldAdded));
    assert!(has(report, "TIMG0.CONF", FindingKind::Reset));
    assert!(has(report, "TIMG0.EXTRA", FindingKind::RegisterSvdOnly));
    let model = load(&output)?;
    let conf = model.register("TIMG0.CONF").expect("CONF");
    let bits = conf
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.low, field.width))
        .collect::<Vec<_>>();
    assert_eq!(bits, [("EN", 0, 1), ("GO", 1, 1)]);
    assert_eq!(
        conf.reset.map(|reset| reset.value),
        Some(0x1),
        "the header's default"
    );
    let extra = model
        .register("TIMG0.EXTRA")
        .expect("the SVD-only register stays");
    assert_eq!(extra.fields[0].class, FieldClass::ReadOnly);
    Ok(())
}

#[test]
fn two_svd_fields_on_one_bit_are_decided_by_the_header() -> TestResult {
    let header = "\
#define TIMG_MMU_REG(i) (REG_TIMG_BASE(i) + 0x0)
/** TIMG_ITEM : R/W; bitpos: [31:0]; default: 0;
 *  The item.
 */
";
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[],
        &[register(
            "MMU",
            0,
            0,
            &[field("VALID", 12, 1), field("SPIRAM_SENSITIVE", 12, 1)],
        )],
    )]);
    let types = "
[[types]]
name = \"MMU\"
description = \"An MMU.\"
registers = \"mmu_reg.h\"

[[types.instances]]
name = \"TIMG0\"
svd = \"TIMG0\"
base = \"DR_REG_TIMG0_BASE\"
";
    let output = import(&spec(types), None, &files(svd, &[("mmu_reg.h", header)]))?;
    assert!(has(&output.report, "TIMG0.MMU", FindingKind::FieldOverlap));
    let model = load(&output)?;
    let mmu = model.register("TIMG0.MMU").expect("MMU");
    assert_eq!(mmu.fields.len(), 1);
    assert_eq!((mmu.fields[0].low, mmu.fields[0].width), (0, 32));
    Ok(())
}

#[test]
fn variants_are_two_types_never_one() -> TestResult {
    let mut other = timg_registers();
    other.push(register("MORE", 8, 0, &[field("VALUE", 0, 4)]));
    let header = format!(
        "{TIMG_H}#define TIMG_MORE_REG(i) (REG_TIMG_BASE(i) + 0x8)\n/** TIMG_VALUE : R/W; bitpos: [3:0]; default: 0;\n *  More.\n */\n"
    );
    let svd = device(&[
        peripheral("TIMG0", 0x1000, &[], &timg_registers()),
        peripheral("TIMG1", 0x2000, &[], &other),
    ]);
    let pinned = files(svd, &[("timg_more_reg.h", &header)]);
    let one = "
[[types]]
name = \"TIMG\"
description = \"A timer group.\"
registers = \"timg_reg.h\"

[[types.instances]]
name = \"TIMG0\"
svd = \"TIMG0\"
base = \"DR_REG_TIMG0_BASE\"

[[types.instances]]
name = \"TIMG1\"
svd = \"TIMG1\"
base = \"DR_REG_TIMG1_BASE\"
registers = \"timg_more_reg.h\"
";
    let error = import(&spec(one), None, &pinned).expect_err("variants are refused as one type");
    assert!(error.reason.contains("differ"), "{error}");
    let two = "
[[types]]
name = \"TIMG\"
description = \"A timer group.\"
registers = \"timg_reg.h\"

[[types.instances]]
name = \"TIMG0\"
svd = \"TIMG0\"
base = \"DR_REG_TIMG0_BASE\"

[[types]]
name = \"TIMG_MORE\"
description = \"A timer group with one more register.\"
registers = \"timg_more_reg.h\"

[[types.instances]]
name = \"TIMG1\"
svd = \"TIMG1\"
base = \"DR_REG_TIMG1_BASE\"
";
    let output = import(&spec(two), None, &pinned)?;
    assert_eq!(output.types.len(), 2);
    let model = load(&output)?;
    assert!(model.register("TIMG1.MORE").is_some());
    assert!(model.register("TIMG0.MORE").is_none());
    Ok(())
}

#[test]
fn the_overlay_carries_commands_ports_and_driver_writes() -> TestResult {
    let header = "\
#define SPI_CMD_REG (DR_REG_TIMG0_BASE + 0x0)
/** SPI_USR : R/W/SC; bitpos: [0]; default: 0;
 *  Starts a user command.
 */
/** SPI_FLASH_READ : R/W/SC; bitpos: [1]; default: 0;
 *  Reads. This field is only for internal debugging purposes. Do not use it in applications.
 */
#define SPI_W0_REG (DR_REG_TIMG0_BASE + 0x4)
/** SPI_BUF0 : R/W/SS; bitpos: [31:0]; default: 0;
 *  Data.
 */
#define SPI_CLOCK_REG (DR_REG_TIMG0_BASE + 0x8)
/** SPI_CLKCNT : R/W; bitpos: [7:0]; default: 3;
 *  Divider.
 */
/** SPI_SPARE : R/W; bitpos: [8]; default: 0;
 *  This field is only for internal debugging purposes. Do not use it in applications.
 */
";
    let driver = "line one\nline two\nline three\n";
    let svd = device(&[peripheral(
        "SPI1",
        0x1000,
        &[],
        &[
            register(
                "CMD",
                0,
                0,
                &[field("USR", 0, 1), field("FLASH_READ", 1, 1)],
            ),
            register("W0", 4, 0, &[field("BUF0", 0, 32)]),
            register(
                "CLOCK",
                8,
                3,
                &[field("CLKCNT", 0, 8), field("SPARE", 8, 1)],
            ),
        ],
    )]);
    let overlay = crate::StrictnessOverlay::parse(&format!(
        "schema = 1\nesp-idf = \"{REVISION}\"\n\n\
         [[commands]]\nperipheral = \"SPI1\"\nregister = \"CMD\"\nfields = [\"USR\"]\nsources = [\"esp-idf:spi_reg.h:2\"]\n\n\
         [[registers]]\nperipheral = \"SPI1\"\nregister = \"W0\"\ndata-port = true\nsources = [\"esp-idf:spi_reg.h:9\"]\n\n\
         [[registers]]\nperipheral = \"SPI1\"\nregister = \"CLOCK\"\ndriver-writes = [{{ field = \"SPARE\", source = \"esp-idf:driver.h:2\" }}]\nsources = [\"esp-idf:spi_reg.h:13\"]\n"
    ))?;
    let types = "
[[types]]
name = \"SPI1_MEM_C\"
description = \"A flash command port.\"
registers = \"spi_reg.h\"

[[types.instances]]
name = \"SPI1\"
svd = \"SPI1\"
base = \"DR_REG_TIMG0_BASE\"
";
    let pinned = files(svd, &[("spi_reg.h", header), ("driver.h", driver)]);
    let output = import(&spec(types), Some(&overlay), &pinned)?;
    let file = &output.types["types/spi1-mem-c.toml"];
    assert!(
        file.contains("commands = [\"USR\"]\nheld = [\"FLASH_READ\"]"),
        "{file}"
    );
    assert!(
        file.contains("data-port = { sources = [\"ESP_IDF_01234567_SPI_REG_H_L9\"] }"),
        "{file}"
    );
    assert!(
        output.evidence.contains("esp-idf:driver.h:2"),
        "{}",
        output.evidence
    );
    let model = load(&output)?;
    let spare = model.field("SPI1.CLOCK.SPARE").expect("SPARE");
    assert_eq!(spare.class, FieldClass::Plain, "a driver writes it");
    assert!(
        spare
            .review
            .sources
            .iter()
            .any(|source| source == "ESP_IDF_01234567_DRIVER_H_L2")
    );
    let read = model.field("SPI1.CMD.FLASH_READ").expect("FLASH_READ");
    assert_eq!(read.class, FieldClass::SelfClearing, "held");
    Ok(())
}

#[test]
fn a_block_the_svd_lacks_comes_from_its_older_format_header() -> TestResult {
    let header = "\
#define MODEM_SYSCON_CLK_CONF_REG          (DR_REG_SYSCON_BASE + 0x4)
/* MODEM_SYSCON_CLK_ETM_EN : R/W ;bitpos:[2] ;default: 1'b1 ; */
/*description: .*/
#define MODEM_SYSCON_CLK_ETM_EN    (BIT(2))
/* MODEM_SYSCON_CLK_DIV : RO ;bitpos:[15:8] ;default: 8'h10 ; */
/*description: The divider,
 read only.*/
";
    let svd = device(&[peripheral("TIMG0", 0x1000, &[], &timg_registers())]);
    let types = "
[[types]]
name = \"MODEM_SYSCON\"
description = \"The modem system controller.\"
registers = \"modem_syscon_reg.h\"

[[types.instances]]
name = \"MODEM_SYSCON\"
base = \"DR_REG_SYSCON_BASE\"
";
    let output = import(
        &spec(types),
        None,
        &files(svd, &[("modem_syscon_reg.h", header)]),
    )?;
    let model = load(&output)?;
    let conf = model.register("MODEM_SYSCON.CLK_CONF").expect("CLK_CONF");
    assert_eq!(
        conf.location,
        crate::chip_model::Location::Mmio { address: 0x4104 }
    );
    let fields = conf
        .fields
        .iter()
        .map(|field| (field.name.as_str(), field.class))
        .collect::<Vec<_>>();
    assert_eq!(
        fields,
        [
            ("CLK_ETM_EN", FieldClass::Plain),
            ("CLK_DIV", FieldClass::ReadOnly)
        ]
    );
    let reset = conf.reset.expect("the defaults");
    assert_eq!((reset.value, reset.mask), (0x1004, Some(0xff04)));
    assert!(!output.types["types/modem-syscon.toml"].contains("description = \".\""));
    Ok(())
}

#[test]
fn the_same_inputs_import_the_same_bytes() -> TestResult {
    let svd = device(&[
        peripheral("TIMG0", 0x1000, &[("TG0_T0", 1)], &timg_registers()),
        derived("TIMG1", "TIMG0", 0x2000, &[("TG1_T0", 2)]),
    ]);
    let pinned = files(svd, &[]);
    let first = import(&spec(TIMG_TYPE), None, &pinned)?;
    let second = import(&spec(TIMG_TYPE), None, &pinned)?;
    assert_eq!(first.root, second.root);
    assert_eq!(first.types, second.types);
    assert_eq!(first.evidence, second.evidence);
    assert_eq!(first.report, second.report);
    Ok(())
}

#[test]
fn malformed_inputs_are_errors_not_panics() {
    let types = TIMG_TYPE;
    let broken_svd = files("<device><peripherals>".to_owned(), &[]);
    assert!(import(&spec(types), None, &broken_svd).is_err());

    let svd = device(&[
        peripheral("TIMG0", 0x1000, &[], &timg_registers()),
        derived("TIMG1", "TIMG0", 0x2000, &[]),
    ]);
    let mut empty_header = files(svd.clone(), &[]);
    empty_header
        .idf
        .insert("timg_reg.h".to_owned(), "garbage\n".to_owned());
    assert!(import(&spec(types), None, &empty_header).is_err());

    let mut unknown_class = files(svd.clone(), &[]);
    unknown_class.idf.insert(
        "timg_reg.h".to_owned(),
        TIMG_H.replace("TIMG_EN : R/W;", "TIMG_EN : XYZ;"),
    );
    let error = import(&spec(types), None, &unknown_class).expect_err("an unknown class");
    assert!(error.reason.contains("no rule"), "{error}");

    let missing_base = files(svd.clone(), &[]);
    let wrong = types.replace("DR_REG_TIMG1_BASE", "DR_REG_NOWHERE_BASE");
    assert!(import(&spec(&wrong), None, &missing_base).is_err());

    let missing_peripheral = files(svd, &[]);
    let wrong = types.replace("svd = \"TIMG1\"", "svd = \"TIMG9\"");
    assert!(import(&spec(&wrong), None, &missing_peripheral).is_err());

    assert!(ImportSpec::parse("schema = 2").is_err());
}

#[test]
fn header_parsers_read_both_formats_and_their_literals() {
    assert_eq!(header::parse_literal("1'b1"), Some(1));
    assert_eq!(header::parse_literal("32'h1F"), Some(0x1f));
    assert_eq!(header::parse_literal("8'd10"), Some(10));
    assert_eq!(header::parse_literal("0x10U"), Some(0x10));
    assert_eq!(header::parse_literal("x'q1"), None);
    let mut bases = header::Bases::default();
    bases.add("reg_base.h", REG_BASE);
    assert_eq!(
        bases.resolve("DR_REG_SYSCON_BASE"),
        Some((0x4100, "reg_base.h".to_owned(), 4))
    );
    let table = header::parse_interrupts("interrupts.h", INTERRUPTS)
        .expect("the test interrupt enumeration parses");
    let sources = &table.sources;
    assert_eq!(sources.len(), 3);
    assert_eq!(
        (sources[2].number, sources[2].name.as_str()),
        (2, "TIMERGRP1_T0")
    );
    let registers = header::parse_registers(TIMG_H);
    assert_eq!(registers[0].description.as_deref(), Some("Configuration."));
    assert_eq!(registers[1].fields[0].default, Some(0));
    assert_eq!(registers[1].fields[0].description, "The count.");
}

#[test]
fn define_fields_reserve_the_remaining_lanes_with_overlay_evidence() -> TestResult {
    let header = "\
#define TIMG_ITEM_REG (DR_REG_TIMG0_BASE + 0x0)
/** TIMG_VALUE : R/W; bitpos: [31:0]; default: 0;
 *  The complete entry.
 */
";
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[],
        &[register("ITEM", 0, 0, &[field("VALUE", 0, 32)])],
    )]);
    let overlay = crate::StrictnessOverlay::parse(&format!(
        r#"schema = 1
esp-idf = "{REVISION}"
[[registers]]
peripheral = "TIMG0"
register = "ITEM"
define-fields = [
    {{ name = "PAGE", offset = 0, width = 3, description = "The page." }},
    {{ name = "VALID", offset = 6, width = 1, description = "The entry is valid." }},
]
sources = ["esp-idf:entry_layout.h:2"]
"#
    ))?;
    let output = import(
        &spec(TIMG_ONLY),
        Some(&overlay),
        &files(
            svd,
            &[
                ("timg_reg.h", header),
                ("entry_layout.h", "\nentry layout\n"),
            ],
        ),
    )?;
    let model = load(&output)?;
    let item = model.register("TIMG0.ITEM").expect("the entry imports");
    let described = item.fields.iter().fold(0, |mask, field| {
        mask | (((1_u32 << field.width) - 1) << field.low)
    });
    assert_eq!(item.fields.len(), 2);
    assert_eq!(item.reserved, !described, "no lane becomes absent");
    let reservation = model.types()["TIMG"].templates["ITEM"]
        .reserved
        .as_ref()
        .expect("the remaining lanes are reserved");
    assert_eq!(
        reservation.sources,
        [
            "ESP_IDF_01234567_ENTRY_LAYOUT_H_L2",
            "ESP_IDF_01234567_TIMG_REG_H",
        ]
    );
    assert!(output.evidence.contains("esp-idf:entry_layout.h:2"));
    Ok(())
}

#[test]
fn hostile_svd_field_ranges_return_errors() {
    for name in ["CONF", "EXTRA"] {
        for (low, width) in [(u32::MAX, 2), (0, u32::MAX), (0, 0), (32, 1)] {
            let svd = device(&[peripheral(
                "TIMG0",
                0x1000,
                &[],
                &[register(
                    name,
                    if name == "CONF" { 0 } else { 8 },
                    0,
                    &[field("INVALID", low, width)],
                )],
            )]);
            let error = import(&spec(TIMG_ONLY), None, &files(svd, &[]))
                .expect_err("invalid SVD geometry must return an error, with or without a header");
            assert!(error.context.contains("SVD"), "{error}");
        }
    }
}

#[test]
fn hostile_device_layout_spans_return_errors() {
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[],
        &[register("EXTRA", 8, 0, &[field("VALUE", 0, 1)])],
    )]);
    let mut spec = spec(TIMG_ONLY);
    spec.types[0].layout = Some("timg_struct.h".to_owned());
    for member in [
        "uint32_t reserved_0[0xffffffffffffffff];",
        "uint32_t reserved_fffffffffffffffc[1];",
    ] {
        let layout = format!("typedef struct {{\n{member}\n}} timg_dev_t;\n");
        let error = import(
            &spec,
            None,
            &files(svd.clone(), &[("timg_struct.h", &layout)]),
        )
        .expect_err("overflow in a reserved span must return an error");
        assert!(error.context.contains("device layout line 2"), "{error}");
        assert!(error.reason.contains("span"), "{error}");
    }
}

#[test]
fn hostile_interrupt_values_return_contextual_errors() {
    let svd = device(&[peripheral("TIMG0", 0x1000, &[], &timg_registers())]);
    for declaration in [
        "ETS_OTHER_INTR_SOURCE = 18446744073709551615,",
        "OTHER = 18446744073709551615,",
        "ETS_OTHER_INTR_SOURCE = 65536,",
        "ETS_OTHER_INTR_SOURCE = 18446744073709551616,",
        "ETS_OTHER_INTR_SOURCE = invalid,",
        "OTHER = invalid,",
        "ETS_LAST_INTR_SOURCE = 65535,\nETS_OTHER_INTR_SOURCE,",
        "ETS_OTHER_INTR_SOURCE = 1,\nETS_DUPLICATE_INTR_SOURCE = 1,",
        "ETS_OTHER_INTR_SOURCE = 1,\nETS_OTHER_INTR_SOURCE = 2,",
    ] {
        let header = format!(
            "typedef enum {{\n{declaration}\nETS_MAX_INTR_SOURCE,\n}} periph_interrupt_t;\n"
        );
        let error = import(
            &spec(TIMG_ONLY),
            None,
            &files(svd.clone(), &[("interrupts.h", &header)]),
        )
        .expect_err("unrepresentable enumeration values must return an error");
        assert!(error.context.starts_with("interrupts.h:"), "{error}");
    }
}

#[test]
fn interrupt_enumeration_keeps_implicit_values_and_the_last_u16_source() -> TestResult {
    let table = header::parse_interrupts(
        "interrupts.h",
        "typedef enum {\nOTHER = 65533,\nETS_FIRST_INTR_SOURCE,\nETS_LAST_INTR_SOURCE,\nETS_MAX_INTR_SOURCE,\n} periph_interrupt_t;\n",
    )?;
    assert_eq!(
        table
            .sources
            .iter()
            .map(|source| source.number)
            .collect::<Vec<_>>(),
        [u16::MAX - 1, u16::MAX]
    );
    assert_eq!(table.count, u32::from(u16::MAX) + 1);
    Ok(())
}

#[test]
fn every_header_source_is_bound_or_unbound_with_line_evidence() -> TestResult {
    let header = "\
typedef enum {
ETS_CACHE_INTR_SOURCE = 0,
ETS_GPIO_INTR0_SOURCE,
ETS_GPIO_INTR1_SOURCE,
ETS_CPU_INTR_FROM_CPU_0_SOURCE,
ETS_CPU_INTR_FROM_CPU_1_SOURCE,
ETS_MAX_INTR_SOURCE,
} periph_interrupt_t;
";
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[("CACHE", 0)],
        &timg_registers(),
    )]);
    let output = import(
        &spec(TIMG_ONLY),
        None,
        &files(svd, &[("interrupts.h", header)]),
    )?;
    let model = load(&output)?;
    assert_eq!(model.interrupt_sources().count, 5);
    assert_eq!(model.interrupts().len(), 1);
    assert_eq!(model.interrupts()[0].name, "CACHE");
    let unbound = model.unbound_interrupts();
    assert_eq!(
        unbound
            .iter()
            .map(|source| source.name.as_str())
            .collect::<Vec<_>>(),
        [
            "GPIO_INTR0",
            "GPIO_INTR1",
            "CPU_INTR_FROM_CPU_0",
            "CPU_INTR_FROM_CPU_1"
        ]
    );
    for (index, source) in unbound.iter().enumerate() {
        assert_eq!(usize::from(source.source), index + 1);
        assert!(!source.reason.trim().is_empty());
        assert_eq!(
            source.sources,
            [format!("ESP_IDF_01234567_INTERRUPTS_H_L{}", index + 3)]
        );
    }
    let mut expected = (2..=7)
        .map(|line| format!("ESP_IDF_01234567_INTERRUPTS_H_L{line}"))
        .collect::<Vec<_>>();
    expected.sort();
    assert_eq!(model.interrupt_sources().sources, expected);
    for line in 2..=7 {
        assert!(
            output
                .evidence
                .contains(&format!("esp-idf:interrupts.h:{line} "))
        );
    }
    Ok(())
}

#[test]
fn incomplete_or_invalid_interrupt_bounds_return_errors() {
    let svd = device(&[peripheral("TIMG0", 0x1000, &[], &timg_registers())]);
    for declarations in [
        "ETS_ZERO_INTR_SOURCE = 0,",
        "ETS_ZERO_INTR_SOURCE = 0,\nETS_MAX_INTR_SOURCE = invalid,",
        "ETS_ZERO_INTR_SOURCE = 0,\nETS_MAX_INTR_SOURCE = 65537,",
        "ETS_ZERO_INTR_SOURCE = 0,\nETS_MAX_INTR_SOURCE = 0,",
        "ETS_ZERO_INTR_SOURCE = 0,\nETS_MAX_INTR_SOURCE = 2,",
        "ETS_ONE_INTR_SOURCE = 1,\nETS_MAX_INTR_SOURCE,",
        "ETS_SAME_INTR_SOURCE = 0,\nETS_SAME_SOURCE,\nETS_MAX_INTR_SOURCE,",
    ] {
        let header = format!("typedef enum {{\n{declarations}\n}} periph_interrupt_t;\n");
        let error = import(
            &spec(TIMG_ONLY),
            None,
            &files(svd.clone(), &[("interrupts.h", &header)]),
        )
        .expect_err("the root requires a complete, bounded enumeration with unique names");
        assert!(error.context.starts_with("interrupts.h"), "{error}");
    }
}

#[test]
fn hostile_overlay_field_ranges_return_errors() -> TestResult {
    let svd = device(&[peripheral("TIMG0", 0x1000, &[], &timg_registers())]);
    let mut overlay = crate::StrictnessOverlay::parse(&format!(
        r#"schema = 1
esp-idf = "{REVISION}"
[[registers]]
peripheral = "TIMG0"
register = "CONF"
define-fields = [{{ name = "EN", offset = 0, width = 1, description = "Enable." }}]
sources = ["esp-idf:timg_reg.h:5"]
"#
    ))?;
    // The public API also accepts an overlay constructed or changed by a
    // caller after parsing; it must check its geometry itself.
    for (low, width) in [(u32::MAX, 2), (0, u32::MAX), (0, 0)] {
        overlay.registers[0].define_fields[0].offset = low;
        overlay.registers[0].define_fields[0].width = width;
        let error = import(&spec(TIMG_ONLY), Some(&overlay), &files(svd.clone(), &[]))
            .expect_err("unrepresentable overlay fields must return an error");
        assert!(error.context.contains("CONF.EN"), "{error}");
        assert!(error.reason.contains("outside"), "{error}");
    }
    Ok(())
}

#[test]
fn header_interrupt_identity_wins_over_stale_svd_number() -> TestResult {
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[("TG0_T0", 0)],
        &timg_registers(),
    )]);
    let output = import(&spec(TIMG_ONLY), None, &files(svd, &[]))?;
    let model = load(&output)?;
    let interrupt = &model.interrupts()[0];
    assert_eq!(
        (
            interrupt.name.as_str(),
            interrupt.source,
            interrupt.instance.as_str()
        ),
        ("TG0_T0", 1, "TIMG0")
    );
    assert!(has(&output.report, "TIMG0", FindingKind::Interrupt));
    Ok(())
}

#[test]
fn explicit_interrupt_spelling_uses_the_header_number_and_evidence() -> TestResult {
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[("OLD_TIMER", 0)],
        &timg_registers(),
    )]);
    let types = format!("{TIMG_ONLY}interrupts = {{ OLD_TIMER = \"TG0_T0\" }}\n");
    let output = import(&spec(&types), None, &files(svd, &[]))?;
    let model = load(&output)?;
    let interrupt = &model.interrupts()[0];
    assert_eq!((interrupt.name.as_str(), interrupt.source), ("TG0_T0", 1));
    assert!(
        interrupt
            .sources
            .iter()
            .any(|source| source == "ESP_IDF_01234567_INTERRUPTS_H_L3")
    );
    assert!(
        interrupt
            .sources
            .iter()
            .any(|source| source.starts_with("VENDOR_SVD_"))
    );
    Ok(())
}

#[test]
fn an_unknown_interrupt_identity_is_an_error_even_at_a_known_number() {
    let svd = device(&[peripheral(
        "TIMG0",
        0x1000,
        &[("GHOST", 0)],
        &timg_registers(),
    )]);
    let error = import(&spec(TIMG_ONLY), None, &files(svd.clone(), &[]))
        .expect_err("source zero is not an identity for GHOST");
    assert!(error.reason.contains("GHOST"), "{error}");
    let types = format!("{TIMG_ONLY}interrupts = {{ GHOST = \"TYPO\" }}\n");
    let error = import(&spec(&types), None, &files(svd, &[]))
        .expect_err("an explicit spelling must exist in the header");
    assert!(error.reason.contains("TYPO"), "{error}");
}

#[test]
fn an_interrupt_mapping_must_name_an_existing_svd_binding() {
    let svd = device(&[peripheral("TIMG0", 0x1000, &[], &timg_registers())]);
    let types = format!("{TIMG_ONLY}interrupts = {{ GHOST = \"TG0_T0\" }}\n");
    let error = import(&spec(&types), None, &files(svd, &[]))
        .expect_err("a stale mapping must not silently stand");
    assert!(error.reason.contains("names no SVD binding"), "{error}");
}

#[test]
fn an_entire_wrapped_default_clause_keeps_its_reset_knowledge() -> TestResult {
    let svd = device(&[peripheral("TIMG0", 0x1000, &[], &timg_registers())]);
    let types = TIMG_ONLY.replace("svd = \"TIMG0\"\n", "");
    for value in [0, 2] {
        for clause in [
            format!("default: {value};"),
            format!("default:\n * {value};"),
        ] {
            let header = format!(
                "\
#define TIMG_CONF_REG (DR_REG_TIMG0_BASE + 0x0)
/** TIMG_EN : R/W; bitpos: [0]; default: 1;
 *  Enables the timer.
 */
/** TIMG_LEVEL : R/W; bitpos: [9:8];
 *  {clause}
 *  The remap level.
 */
"
            );
            let output = import(
                &spec(&types),
                None,
                &files(svd.clone(), &[("timg_reg.h", &header)]),
            )?;
            let model = load(&output)?;
            let reset = model
                .register("TIMG0.CONF")
                .expect("configuration imports")
                .reset
                .expect("both defaults are known");
            assert_eq!(
                (reset.value, reset.mask),
                ((value << 8) | 1, Some((3 << 8) | 1))
            );
            let parsed = header::parse_registers(&header);
            assert_eq!(parsed[0].fields[1].description, "The remap level.");
            assert!(
                !output
                    .report
                    .findings
                    .iter()
                    .any(|f| f.kind == FindingKind::Default)
            );
        }
    }
    Ok(())
}
