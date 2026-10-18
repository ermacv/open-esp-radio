//! The prose half of vendor provenance: every obfuscated vendor symbol a
//! vendor document names must be defined by a pinned artifact.
//!
//! A name such as `r_sym_ble_<20 characters>` changes with every vendor
//! rebuild, so a document that keeps one after a pin update silently
//! describes code that no longer exists. A line that deliberately refers to
//! an earlier archive carries the [`HISTORICAL`] mark and is skipped.
use std::collections::BTreeSet;

/// Mark of a line that names symbols of an earlier, unpinned archive.
pub(super) const HISTORICAL: &str = "<!-- vendor-symbol: historical -->";

/// Length of the obfuscated suffix of a vendor symbol.
const OBFUSCATED_SUFFIX: usize = 20;

/// Whether `token` has the shape of an obfuscated vendor symbol:
/// `[brk_|r_]sym_<module>_<20 alphanumerics>`, optionally followed by a
/// compiler `.part.N` suffix.
fn is_obfuscated(token: &str) -> bool {
    let base = token.split(".part.").next().unwrap_or(token);
    let Some(rest) = ["brk_sym_", "r_sym_", "sym_"]
        .iter()
        .find_map(|prefix| base.strip_prefix(prefix))
    else {
        return false;
    };
    let Some((module, suffix)) = rest.rsplit_once('_') else {
        return false;
    };
    !module.is_empty()
        && module
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.')
        && suffix.len() == OBFUSCATED_SUFFIX
        && suffix.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Obfuscated vendor symbols of `line`, with a `.part.N` suffix removed.
fn obfuscated_symbols(line: &str) -> impl Iterator<Item = &str> {
    line.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.'))
        .map(|token| token.trim_end_matches('.'))
        .filter(|token| is_obfuscated(token))
        .map(|token| token.split(".part.").next().unwrap_or(token))
}

/// One violation per obfuscated symbol of `text` that `defined` lacks,
/// reported as `path:line`.
pub(super) fn undefined_symbols(
    path: &str,
    text: &str,
    defined: &BTreeSet<String>,
    artifacts: &str,
) -> Vec<String> {
    let mut problems = vec![];
    for (index, line) in text.lines().enumerate() {
        if line.contains(HISTORICAL) {
            continue;
        }
        for symbol in obfuscated_symbols(line) {
            if !defined.contains(symbol) {
                problems.push(format!(
                    "{path}:{}: {symbol} is not defined by the pinned {artifacts}",
                    index + 1
                ));
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defined() -> BTreeSet<String> {
        [
            "r_sym_ble_GlcyfUkkhUzGUt8un0d8",
            "brk_sym_sched_wfZseauvjWnjfiu10yWN",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    #[test]
    fn a_pinned_symbol_passes_and_a_stale_one_is_reported_with_its_line() {
        let text = "The body `r_sym_ble_GlcyfUkkhUzGUt8un0d8` and\n\
                    `brk_sym_sched_wfZseauvjWnjfiu10yWN.part.3`, then\n\
                    the stale r_sym_ble_mqh4OXzoN59kvnkKFMA1.\n";
        assert_eq!(
            undefined_symbols("doc.md", text, &defined(), "libble_app.a"),
            ["doc.md:3: r_sym_ble_mqh4OXzoN59kvnkKFMA1 is not defined by the pinned libble_app.a"]
        );
    }

    #[test]
    fn a_historical_line_and_ordinary_names_are_not_checked() {
        let text = format!(
            "Earlier `r_sym_ble_mqh4OXzoN59kvnkKFMA1` {HISTORICAL}\n\
             `r_ble_lll_adv_start`, `sym_controller`, `r_sym_ble_short`.\n"
        );
        assert!(undefined_symbols("doc.md", &text, &defined(), "a").is_empty());
    }
}
