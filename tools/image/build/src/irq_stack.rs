//! An image's choices for its interrupt stacks, in
//! `[package.metadata.open-radio]`:
//!
//! - `irq-stack-margin-percent`: the margin above a proven bound, a whole
//!   number of per cent, zero included; absent, the stack policy's.
//! - `irq-stack`: the size of each stack whose bound is not proven, by hart
//!   as `harts` names them (`boot`, or the hart's number): the bytes the
//!   stack holds above its guard. A declaration for a hart whose bound is
//!   proven fails the link, naming the proof: a size has one source.
//! - `irq-stack-guard`: `"placed"` (the default) or `"declined"`. Declined,
//!   no guard bytes are placed, hart 0's stack is not rounded to hart 1's
//!   guard and the runtime compiles no arming code; an overflow of hart 0's
//!   stack then runs below the SRAM's start and one of hart 1's into the top
//!   of hart 0's stack.

use oer_tools_chip_profile::{Profile, policy::IrqStackGuard};
use oer_tools_image_checks::linker::irq_stacks::{DECLARATION, DeclaredIrqStack};
use serde_json::Value;

use crate::Result;

const MARGIN: &str = "irq-stack-margin-percent";
const GUARD: &str = "irq-stack-guard";

/// What an image chose for its interrupt stacks.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IrqStackChoices {
    /// The margin above a proven bound; `None` takes the stack policy's.
    pub margin_percent: Option<u32>,
    /// The sizes of stacks whose bound is not proven.
    pub declared: Vec<DeclaredIrqStack>,
    /// Whether the image places the stacks' guards.
    pub guard: IrqStackGuard,
}

/// The choices `package` makes in the workspace `metadata`.
///
/// # Errors
///
/// The workspace has no `package`, or a key is malformed or names a hart
/// outside the chip.
pub(crate) fn of_metadata(
    metadata: &Value,
    package: &str,
    profile: &Profile,
) -> Result<IrqStackChoices> {
    let margin_percent = match crate::manifest::declared(metadata, package, MARGIN)? {
        None => None,
        Some(value) => Some(
            value
                .as_u64()
                .and_then(|percent| u32::try_from(percent).ok())
                .ok_or_else(|| {
                    format!("{package}: `{MARGIN}` is a whole number of per cent, not {value}")
                })?,
        ),
    };
    let guard = match crate::manifest::declared(metadata, package, GUARD)? {
        None => IrqStackGuard::Placed,
        Some(serde_json::Value::String(value)) if value == "placed" => IrqStackGuard::Placed,
        Some(serde_json::Value::String(value)) if value == "declined" => IrqStackGuard::Declined,
        Some(other) => {
            return Err(
                format!("{package}: `{GUARD}` is \"placed\" or \"declined\", not {other}").into(),
            );
        }
    };
    let mut declared = Vec::new();
    if let Some(value) = crate::manifest::declared(metadata, package, DECLARATION)? {
        let interrupts = profile
            .facts
            .interrupts
            .as_ref()
            .ok_or("`irq-stack` needs the chip's hart facts")?;
        let table = value.as_object().ok_or_else(|| {
            format!("{package}: `{DECLARATION}` is a table of sizes by hart (`boot` or its number)")
        })?;
        for (name, bytes) in table {
            let hart = match name.as_str() {
                "boot" => interrupts.boot_hart,
                number => number.parse::<u32>().map_err(|_| {
                    format!(
                        "{package}: `{DECLARATION}` names `{name}`, not `boot` or a hart number"
                    )
                })?,
            };
            if !interrupts.harts.contains(&hart) {
                return Err(format!(
                    "{package}: `{DECLARATION}` names hart {hart} outside the chip facts"
                )
                .into());
            }
            if declared
                .iter()
                .any(|stack: &DeclaredIrqStack| stack.hart == hart)
            {
                return Err(format!("{package}: `{DECLARATION}` names hart {hart} twice").into());
            }
            let bytes = bytes
                .as_u64()
                .and_then(|bytes| u32::try_from(bytes).ok())
                .filter(|bytes| *bytes > 0 && bytes % 16 == 0)
                .ok_or_else(|| {
                    format!(
                        "{package}: `{DECLARATION}` sizes `{name}` at {bytes}, not a positive \
                         number of bytes aligned to 16"
                    )
                })?;
            declared.push(DeclaredIrqStack { hart, bytes });
        }
        declared.sort_unstable_by_key(|stack| stack.hart);
    }
    Ok(IrqStackChoices {
        margin_percent,
        declared,
        guard,
    })
}

/// The choices `package`, in the workspace at `root`, makes.
///
/// # Errors
///
/// As [`of_metadata`], or `cargo metadata` fails.
pub(crate) fn declared(
    root: &std::path::Path,
    package: &str,
    profile: &Profile,
) -> Result<IrqStackChoices> {
    of_metadata(
        &crate::manifest::workspace_metadata(root)?,
        package,
        profile,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(open_radio: Value) -> Value {
        serde_json::json!({ "packages": [
            { "name": "image", "metadata": { "open-radio": open_radio } },
        ] })
    }

    fn profile() -> Profile {
        Profile::tracked("esp32s31").unwrap()
    }

    #[test]
    fn nothing_declared_takes_the_policy_s_margin_and_declares_no_size() {
        let choices = of_metadata(&metadata(serde_json::json!({})), "image", &profile()).unwrap();
        assert_eq!(choices, IrqStackChoices::default());
    }

    #[test]
    fn a_margin_of_zero_and_sizes_by_the_harts_names_are_read() {
        let choices = of_metadata(
            &metadata(serde_json::json!({
                "irq-stack-margin-percent": 0,
                "irq-stack": { "boot": 32768, "1": 4096 },
            })),
            "image",
            &profile(),
        )
        .unwrap();
        assert_eq!(choices.margin_percent, Some(0));
        assert_eq!(
            choices.declared,
            [
                DeclaredIrqStack {
                    hart: 0,
                    bytes: 32768
                },
                DeclaredIrqStack {
                    hart: 1,
                    bytes: 4096
                },
            ]
        );
    }

    #[test]
    fn the_guard_is_placed_unless_the_image_declines_it() {
        for (open_radio, guard) in [
            (serde_json::json!({}), IrqStackGuard::Placed),
            (
                serde_json::json!({ "irq-stack-guard": "placed" }),
                IrqStackGuard::Placed,
            ),
            (
                serde_json::json!({ "irq-stack-guard": "declined" }),
                IrqStackGuard::Declined,
            ),
        ] {
            let choices = of_metadata(&metadata(open_radio), "image", &profile()).unwrap();
            assert_eq!(choices.guard, guard);
        }
    }

    #[test]
    fn a_malformed_choice_is_refused_naming_its_key() {
        for (open_radio, key) in [
            (
                serde_json::json!({ "irq-stack-margin-percent": -1 }),
                MARGIN,
            ),
            (
                serde_json::json!({ "irq-stack-margin-percent": "10" }),
                MARGIN,
            ),
            (serde_json::json!({ "irq-stack": [32768] }), DECLARATION),
            (
                serde_json::json!({ "irq-stack": { "main": 32768 } }),
                DECLARATION,
            ),
            (
                serde_json::json!({ "irq-stack": { "7": 32768 } }),
                DECLARATION,
            ),
            (
                serde_json::json!({ "irq-stack": { "boot": 0 } }),
                DECLARATION,
            ),
            (
                serde_json::json!({ "irq-stack": { "boot": 100 } }),
                DECLARATION,
            ),
            (
                serde_json::json!({ "irq-stack": { "boot": 16, "0": 16 } }),
                DECLARATION,
            ),
            (serde_json::json!({ "irq-stack-guard": "none" }), GUARD),
            (serde_json::json!({ "irq-stack-guard": false }), GUARD),
        ] {
            let error = of_metadata(&metadata(open_radio.clone()), "image", &profile())
                .unwrap_err()
                .to_string();
            assert!(error.contains(&format!("`{key}`")), "{open_radio}: {error}");
        }
    }
}
