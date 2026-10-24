use std::collections::BTreeMap;

use super::{HeaderTree, check_overlay, import_overlay};
use crate::{Error, Result, StrictnessOverlay};

const PIN: &str = "4d59230ddff16327812782151ef0afef202dc6d7";
const REG: &str = "components/soc/chip/register/soc/timer_reg.h";
const STRUCT: &str = "components/soc/chip/register/soc/timer_struct.h";
const INTERRUPTS: &str = "components/soc/chip/include/soc/interrupts.h";

const REG_H: &str = "\
/** TIMER_CTRL_REG register
 *  control
 */
#define TIMER_CTRL_REG (DR_REG_TIMER_BASE + 0x0)
/** TIMER_START : WT; bitpos: [0]; default: 0;
 *  start
 */
#define TIMER_START    (BIT(0))
/** TIMER_MODE : R/W; bitpos: [3:1]; default: 0;
 *  mode
 */
#define TIMER_MODE    0x00000007U

/** TIMER_INT_RAW_REG register
 *  raw status
 */
#define TIMER_INT_RAW_REG (DR_REG_TIMER_BASE + 0x4)
/** TIMER_DONE_INT_RAW : R/WTC/SS; bitpos: [0]; default: 0;
 *  done
 */
#define TIMER_DONE_INT_RAW    (BIT(0))

/** TIMER_STATUS_REG register
 *  status
 */
#define TIMER_STATUS_REG (DR_REG_TIMER_BASE + 0x8)
/** TIMER_BUSY : R/W/SC; bitpos: [0]; default: 0;
 *  busy
 */
#define TIMER_BUSY    (BIT(0))
";

const STRUCT_H: &str = "\
typedef union {
    struct {
        uint32_t start:1;
        uint32_t mode:3;
        uint32_t reserved_4:28;
    };
    uint32_t val;
} timer_ctrl_reg_t;

typedef union {
    struct {
        uint32_t done_int_raw:1;
        uint32_t reserved_1:31;
    };
    uint32_t val;
} timer_int_raw_reg_t;
";

const INTERRUPTS_H: &str = "\
typedef enum {
    ETS_TIMER_INTR_SOURCE = 0,
    ETS_EXTRA_INTR_SOURCE,
    ETS_MAX_INTR_SOURCE,
} periph_interrupt_t;
";

const SVD: &str = "<?xml version=\"1.0\"?><device schemaVersion=\"1.3\"><name>V</name>\
<addressUnitBits>8</addressUnitBits><width>32</width><size>32</size><access>read-write</access>\
<peripherals><peripheral><name>TIMER</name><baseAddress>0x60000000</baseAddress>\
<interrupt><name>TIMER</name><value>0</value></interrupt><registers>\
<register><name>CTRL</name><addressOffset>0x0</addressOffset><fields>\
<field><name>START</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth><access>write-only</access></field>\
<field><name>MODE</name><bitOffset>1</bitOffset><bitWidth>3</bitWidth><access>write-only</access></field></fields></register>\
<register><name>INT_RAW</name><addressOffset>0x4</addressOffset><fields>\
<field><name>DONE</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field></fields></register>\
<register><name>STATUS</name><addressOffset>0x8</addressOffset><fields>\
<field><name>BUSY</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field></fields></register>\
</registers></peripheral></peripherals></device>";

struct Tree(BTreeMap<String, String>);

impl Tree {
    fn new(reg: &str, structs: &str) -> Self {
        Self(BTreeMap::from([
            (REG.to_owned(), reg.to_owned()),
            (STRUCT.to_owned(), structs.to_owned()),
            (INTERRUPTS.to_owned(), INTERRUPTS_H.to_owned()),
        ]))
    }
}

impl HeaderTree for Tree {
    fn read(&self, path: &str) -> Result<String> {
        self.0
            .get(path)
            .cloned()
            .ok_or_else(|| Error::Invalid(format!("no file {path}")))
    }

    fn list(&self, directory: &str) -> Result<Vec<String>> {
        Ok(self
            .0
            .keys()
            .filter_map(|path| path.strip_prefix(&format!("{directory}/")))
            .map(str::to_owned)
            .collect())
    }
}

fn line(text: &str, needle: &str) -> usize {
    text.lines()
        .position(|l| l.contains(needle))
        .map(|i| i + 1)
        .unwrap_or_default()
}

/// An overlay whose every entry is right for the headers above.
fn overlay_text() -> String {
    let ctrl = line(REG_H, "TIMER_CTRL_REG (");
    let raw = line(REG_H, "TIMER_INT_RAW_REG (");
    let status = line(REG_H, "TIMER_STATUS_REG (");
    let start = line(REG_H, "TIMER_START :");
    let mode = line(REG_H, "TIMER_MODE :");
    let ctrl_union = line(STRUCT_H, "timer_ctrl_reg_t");
    let raw_union = line(STRUCT_H, "timer_int_raw_reg_t");
    let extra = line(INTERRUPTS_H, "ETS_EXTRA");
    let max = line(INTERRUPTS_H, "ETS_MAX");
    format!(
        "schema = 1\nesp-idf = \"{PIN}\"\n\n\
[[interrupts]]\nname = \"EXTRA\"\nsource = 1\nsources = [\"esp-idf:{INTERRUPTS}:{extra}\"]\n\n\
[[interrupts]]\nname = \"MAX\"\nsource = 2\nsources = [\"esp-idf:{INTERRUPTS}:{max}\"]\n\n\
[[registers]]\nperipheral = \"TIMER\"\nregister = \"CTRL\"\nreserved = 0xfffffff0\n\
sources = [\"esp-idf:{REG}:{ctrl}\", \"esp-idf:{STRUCT}:{ctrl_union}\"]\n\n\
[[fields]]\nperipheral = \"TIMER\"\nregister = \"CTRL\"\nfield = \"START\"\nkind = \"write-trigger\"\n\
access = \"read-write\"\nsources = [\"esp-idf:{REG}:{start}\"]\n\n\
[[fields]]\nperipheral = \"TIMER\"\nregister = \"CTRL\"\nfield = \"MODE\"\nkind = \"plain\"\naccess = \"read-write\"\n\
sources = [\"esp-idf:{REG}:{mode}\"]\n\n\
[[registers]]\nperipheral = \"TIMER\"\nregister = \"INT_RAW\"\nwrite-one-to-clear = true\n\
raw-status-writable = true\nreserved = 0xfffffffe\n\
sources = [\"esp-idf:{REG}:{raw}\", \"esp-idf:{STRUCT}:{raw_union}\"]\n\n\
[[registers]]\nperipheral = \"TIMER\"\nregister = \"STATUS\"\naccess = \"read-only\"\n\
sources = [\"esp-idf:{REG}:{status}\"]\n"
    )
}

fn failures(overlay: &str, reg: &str, structs: &str) -> Vec<String> {
    let overlay = StrictnessOverlay::parse(overlay).unwrap();
    check_overlay(&overlay, SVD, &Tree::new(reg, structs))
        .unwrap()
        .failures
}

#[test]
fn an_overlay_that_matches_its_headers_passes() {
    let overlay = StrictnessOverlay::parse(&overlay_text()).unwrap();
    let verdict = check_overlay(&overlay, SVD, &Tree::new(REG_H, STRUCT_H)).unwrap();
    assert!(verdict.failures.is_empty(), "{:?}", verdict.failures);
    assert_eq!(verdict.checked, 7);
}

#[test]
fn a_stale_source_line_fails() {
    // One line added above every definition: each cited line moves.
    let shifted = format!("// banner\n{REG_H}");
    let found = failures(&overlay_text(), &shifted, STRUCT_H);
    assert!(found.iter().any(|f| f.contains("CTRL")), "{found:?}");
    assert!(found.iter().any(|f| f.contains("START")), "{found:?}");
}

#[test]
fn a_field_whose_header_access_class_differs_fails() {
    let changed = REG_H.replace("TIMER_START : WT;", "TIMER_START : R/W;");
    let found = failures(&overlay_text(), &changed, STRUCT_H);
    assert!(
        found
            .iter()
            .any(|f| f.contains("START") && f.contains("access")),
        "{found:?}"
    );
}

#[test]
fn a_register_that_is_not_write_one_to_clear_in_the_header_fails() {
    let changed = REG_H.replace("R/WTC/SS", "R/W/SS");
    let found = failures(&overlay_text(), &changed, STRUCT_H);
    assert!(
        found
            .iter()
            .any(|f| f.contains("INT_RAW") && f.contains("write-one-to-clear")),
        "{found:?}"
    );
}

#[test]
fn field_bits_that_differ_from_the_svd_fail() {
    let changed = REG_H.replace("bitpos: [3:1]", "bitpos: [4:2]");
    let found = failures(&overlay_text(), &changed, STRUCT_H);
    assert!(
        found
            .iter()
            .any(|f| f.contains("MODE") && f.contains("bits")),
        "{found:?}"
    );
}

#[test]
fn reserved_bits_come_from_the_union_members() {
    let changed = STRUCT_H.replace(
        "uint32_t mode:3;\n        uint32_t reserved_4:28;",
        "uint32_t mode:2;\n        uint32_t reserved_3:29;",
    );
    let found = failures(&overlay_text(), REG_H, &changed);
    assert!(
        found
            .iter()
            .any(|f| f.contains("CTRL") && f.contains("reserved")),
        "{found:?}"
    );
}

#[test]
fn a_read_only_register_needs_a_hardware_modified_field() {
    let changed = REG_H.replace("R/W/SC", "R/W");
    let found = failures(&overlay_text(), &changed, STRUCT_H);
    assert!(found.iter().any(|f| f.contains("STATUS")), "{found:?}");
}

#[test]
fn an_interrupt_number_must_be_its_position_in_the_enum() {
    let text = overlay_text().replace("source = 1\n", "source = 5\n");
    let found = failures(&text, REG_H, STRUCT_H);
    assert!(found.iter().any(|f| f.contains("EXTRA")), "{found:?}");
}

#[test]
fn import_rebuilds_the_entries_the_headers_state() {
    let text = overlay_text();
    let existing = StrictnessOverlay::parse(&text).unwrap();
    // Import from an overlay that states only the peripheral's registers
    // with a wrong reserved mask and a missing field.
    let stale = text
        .replace("reserved = 0xfffffff0", "reserved = 0xffffff00")
        .replace("kind = \"write-trigger\"\n", "kind = \"plain\"\n");
    let stale_overlay = StrictnessOverlay::parse(&stale).unwrap();
    let rebuilt = import_overlay(
        &stale,
        &stale_overlay,
        "chip",
        SVD,
        &[],
        &Tree::new(REG_H, STRUCT_H),
    )
    .unwrap();
    let rebuilt = StrictnessOverlay::parse(&rebuilt).unwrap();
    assert_eq!(
        format!("{:?}", rebuilt.registers),
        format!("{:?}", existing.registers)
    );
    assert_eq!(
        format!("{:?}", rebuilt.fields),
        format!("{:?}", existing.fields)
    );
    assert_eq!(
        format!("{:?}", rebuilt.interrupts),
        format!("{:?}", existing.interrupts)
    );
    // And what it built passes the check.
    let verdict = check_overlay(&rebuilt, SVD, &Tree::new(REG_H, STRUCT_H)).unwrap();
    assert!(verdict.failures.is_empty(), "{:?}", verdict.failures);
}
