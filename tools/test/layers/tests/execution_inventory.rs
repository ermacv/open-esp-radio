//! The application entry's execution inventory stays complete: every link
//! the code declares has a row, and every row names text the code holds.
//!
//! This holds the list to the code, not the links to their guarantees: a row's
//! `check`, `gap` and `wait` state what a reader found, and no test here proves
//! them. A declaration outside [`SCANNED`], or of a shape `DECLARATIONS` does
//! not name, goes unseen.
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const INVENTORY: &str = "chips/esp32s31/app/execution-inventory.toml";
const SCANNED: &[&str] = &[
    "chips/esp32s31/app/src",
    "chips/esp32s31/platform/runtime/src",
    "chips/esp32s31/radio/ieee80211/station/src",
    "chips/family/drivers/usb-serial-jtag/src",
    "chips/family/app/src",
    "hil/agent/src",
    "oer/src",
];
/// What declares a link, and the inventory tables that list it. A hand-written
/// software interrupt is `Esp32s31, <digit>`, whatever its surrounding type.
const DECLARATIONS: &[(&str, &str)] = &[
    ("IrqSignal::new::<", "signal"),
    ("PeerNotify::new(", "peer-notify"),
    ("Esp32s31, ", "executor swi-site task-set timer"),
    ("executor_observer::<", "swi-site"),
    (".run::<", "swi-site"),
    // No production image holds a deadline-bearing peer channel yet: the first
    // one has to add its table here.
    ("PeerClient<", "no-production-peer-channel"),
    ("PeerServer<", "no-production-peer-channel"),
];

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")
}

fn inventory() -> toml::Table {
    let text = std::fs::read_to_string(root().join(INVENTORY)).unwrap();
    text.parse().unwrap()
}

fn rows<'a>(inventory: &'a toml::Table, table: &str) -> &'a [toml::Value] {
    inventory
        .get(table)
        .and_then(toml::Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn text(row: &toml::Value, field: &str) -> String {
    row[field].as_str().unwrap().to_owned()
}

fn sources(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name != "tests") {
                sources(&path, found);
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "rs" || extension == "in")
            && !path.to_string_lossy().ends_with("tests.rs")
        {
            found.push(path);
        }
    }
}

/// Whether a line declares something rather than documents it.
fn code(line: &str) -> bool {
    let line = line.trim_start();
    !(line.starts_with("//") || line.starts_with("#!") || line.is_empty())
}

/// Whether `line` holds `declaration`; a chip name counts only with its number.
fn declares(line: &str, declaration: &str) -> bool {
    line.match_indices(declaration).any(|(start, _)| {
        !declaration.starts_with("Esp32s31")
            || line[start + declaration.len()..]
                .chars()
                .next()
                .is_some_and(|next| next.is_ascii_digit())
    })
}

#[test]
fn every_row_names_text_of_its_file() {
    let inventory = inventory();
    for table in [
        "interrupt",
        "executor",
        "swi-site",
        "task-set",
        "task-root",
        "signal",
        "peer-notify",
        "timer",
        "hart-start",
    ] {
        let listed = rows(&inventory, table);
        assert!(!listed.is_empty(), "{table}");
        for row in listed {
            let site = text(row, "site");
            let anchor = text(row, "anchor");
            let source = std::fs::read_to_string(root().join(&site)).unwrap();
            assert_eq!(
                source.lines().filter(|line| line.contains(&anchor)).count(),
                1,
                "`{anchor}` in {site}"
            );
        }
    }
}

#[test]
fn every_declaration_of_the_code_has_a_row() {
    let inventory = inventory();
    let root = root();
    let mut files = Vec::new();
    for directory in SCANNED {
        sources(&root.join(directory), &mut files);
    }
    let mut missing = Vec::new();
    for (declaration, tables) in DECLARATIONS {
        let listed: BTreeSet<(String, String)> = tables
            .split(' ')
            .flat_map(|table| rows(&inventory, table))
            .map(|row| (text(row, "site"), text(row, "anchor")))
            .collect();
        for file in &files {
            let relative = file
                .strip_prefix(&root)
                .unwrap()
                .display()
                .to_string()
                .replace("../", "");
            let source = std::fs::read_to_string(file).unwrap();
            for line in source.lines().filter(|line| code(line)) {
                if declares(line, declaration)
                    && !listed
                        .iter()
                        .any(|(site, anchor)| *site == relative && line.contains(anchor))
                {
                    missing.push(format!("{relative}: {}", line.trim()));
                }
            }
        }
    }
    assert!(missing.is_empty(), "no row:\n{}", missing.join("\n"));
}

#[test]
fn every_table_row_has_a_row_with_its_cfg() {
    let inventory = inventory();
    let path = "chips/esp32s31/app/src/interrupt_table.in";
    let table = std::fs::read_to_string(root().join(path)).unwrap();
    let mut cfg = String::new();
    let mut found = Vec::new();
    for line in table.lines().map(str::trim) {
        if let Some(attribute) = line
            .strip_prefix("#[cfg(")
            .and_then(|line| line.strip_suffix(")]"))
        {
            cfg = attribute.to_owned();
        } else if let Some((name, rest)) = line.split_once(": ") {
            if rest.contains("Token = ") {
                found.push((name.to_owned(), std::mem::take(&mut cfg)));
            }
        }
    }
    let listed: Vec<(String, String)> = rows(&inventory, "interrupt")
        .iter()
        .map(|row| (text(row, "name"), text(row, "cfg")))
        .collect();
    assert_eq!(found, listed);
}

#[test]
fn every_table_row_has_its_layout_entry() {
    let inventory = inventory();
    let facts = std::fs::read_to_string(root().join("chips/esp32s31/facts/chip.toml")).unwrap();
    let facts: toml::Table = facts.parse().unwrap();
    let layout: BTreeSet<String> = facts["interrupt-layout"]["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| text(entry, "name"))
        .collect();
    let listed: BTreeSet<String> = rows(&inventory, "interrupt")
        .iter()
        .map(|row| text(row, "name"))
        .collect();
    assert_eq!(layout, listed);
}

#[test]
fn a_chip_name_without_a_number_declares_nothing() {
    assert!(declares("Executor<Esp32s31, 0>", "Esp32s31, "));
    assert!(!declares("run::<Esp32s31, _, _>", "Esp32s31, "));
    assert!(declares("IrqSignal::new::<X>", "IrqSignal::new::<"));
}

#[test]
fn every_check_a_row_names_is_a_known_check() {
    let inventory = inventory();
    let text = std::fs::read_to_string(root().join(INVENTORY)).unwrap();
    for (name, value) in &inventory {
        let Some(list) = value.as_array() else {
            continue;
        };
        for row in list {
            for check in row
                .get("check")
                .and_then(toml::Value::as_array)
                .unwrap_or(&Vec::new())
            {
                let check = check.as_str().unwrap();
                assert!(text.contains(&format!("#   {check} ")), "{name}: {check}");
            }
        }
    }
}
