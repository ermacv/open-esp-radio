//! Handwritten PAC operations are single transactions.
//!
//! A radio PAC (`crates/*/pac/src`) never polls, retries or keeps a
//! step machine; those belong to the HAL, where every such loop carries an
//! explicit budget (see the chip PAC READMEs). A loop in a handwritten
//! PAC module is therefore rejected, except a `while` that walks an index
//! over a fixed-length table (`index < TABLE.len()` or `index != COUNT`),
//! which writes a straight-line register sequence. There is no exception
//! list.

use std::{fs, path::Path};

use crate::Result;

/// Why `line` is not a single-transaction construct, if it is a loop.
fn loop_violation(line: &str) -> Option<&'static str> {
    let code = line.trim_start();
    if code.starts_with("loop {") || code == "loop" {
        return Some("`loop` polls or retries");
    }
    let condition = code.strip_prefix("while ")?;
    let condition = condition.trim_end_matches('{').trim();
    let table_walk = [" < ", " != "].iter().any(|operator| {
        condition
            .split_once(operator)
            .is_some_and(|(index, bound)| {
                let index_is_name = index
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_');
                let bound = bound.trim();
                let bound_is_table = bound.ends_with(".len()")
                    || bound.chars().all(|character| {
                        character.is_ascii_uppercase()
                            || character.is_ascii_digit()
                            || character == '_'
                    });
                index_is_name && bound_is_table
            })
    });
    (!table_walk).then_some("`while` other than a fixed-length table walk")
}

fn handwritten(relative: &str) -> bool {
    relative.contains("/pac/src/")
        && relative.ends_with(".rs")
        && !relative.ends_with("/generated.rs")
        && !relative.contains("/tests")
        && !relative.ends_with("tests.rs")
}

/// Reject polling or retry loops in every handwritten PAC module.
pub fn check(root: &Path, files: &[String]) -> Result<()> {
    let mut problems = Vec::new();
    for relative in files.iter().filter(|relative| handwritten(relative)) {
        let text = fs::read_to_string(root.join(relative))?;
        for (number, line) in text.lines().enumerate() {
            if let Some(reason) = loop_violation(line) {
                problems.push(format!("{relative}:{}: {reason}", number + 1));
            }
        }
    }
    if problems.is_empty() {
        return Ok(());
    }
    Err(format!(
        "handwritten PAC operations must be single transactions; move polling, retries and step machines to the HAL:\n{}",
        problems.join("\n")
    )
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_walks_pass_and_polling_loops_fail() {
        assert!(
            loop_violation("        while index != PHY_I2C_COMMAND_MEMORY_ENTRY_COUNT {").is_none()
        );
        assert!(
            loop_violation("        while clock < self.phy_calibration_clocks.len() {").is_none()
        );
        assert!(loop_violation("    loop {").is_some());
        assert!(loop_violation("    while !ready() {").is_some());
        assert!(loop_violation("    while status.busy() {").is_some());
        assert!(loop_violation("    let loops = 3;").is_none());
    }

    #[test]
    fn only_handwritten_pac_sources_are_checked() {
        assert!(handwritten("crates/hardware/chip-a/pac/src/phy/i2c.rs"));
        assert!(!handwritten("crates/hardware/chip-a/pac/src/generated.rs"));
        assert!(!handwritten("crates/hardware/chip-a/pac/src/phy/tests.rs"));
        assert!(!handwritten("crates/hardware/chip-a/pac/raw/src/lib.rs"));
        assert!(!handwritten("crates/hardware/chip-a/hal/src/phy.rs"));
    }
}
