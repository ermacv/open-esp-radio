//! The registry of reviewed vendor-function fingerprints,
//! `chips/<chip>/vendor/facts/provenance.toml`: its format and its
//! reader. `oer-vendor-provenance` maintains and checks it; the register
//! inventory reads which functions it registers.
use crate::Result;
use std::path::Path;

/// One registered function definition.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    pub artifact: String,
    pub member: String,
    pub symbol: String,
    pub code: String,
    /// The verification decision files that name the function: their
    /// exclusions describe its code at `code`.
    pub decisions: Vec<String>,
}

/// The entries of a registry file's `text`.
pub fn parse(text: &str) -> Result<Vec<Entry>> {
    let table: toml::Table = toml::from_str(text)?;
    if table.get("schema").and_then(|v| v.as_integer()) != Some(1) {
        return Err("unsupported provenance registry schema".into());
    }
    let mut entries = vec![];
    for value in table
        .get("function")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
    {
        let field = |key: &str| -> Result<String> {
            value
                .get(key)
                .and_then(|v| v.as_str())
                .map(str::to_owned)
                .ok_or_else(|| format!("registry function lacks `{key}`").into())
        };
        let decisions = value
            .get("decisions")
            .and_then(|v| v.as_array())
            .into_iter()
            .flatten()
            .map(|v| {
                v.as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| "registry decision is not a file name".into())
            })
            .collect::<Result<Vec<String>>>()?;
        entries.push(Entry {
            artifact: field("artifact")?,
            member: field("member")?,
            symbol: field("symbol")?,
            code: field("code")?,
            decisions,
        });
    }
    Ok(entries)
}

/// The registry file of `entries`.
pub fn render(entries: &[Entry]) -> String {
    let mut text = String::from(
        "# Code fingerprints of the vendor functions production `SOURCE:` blocks\n\
         # and the register model cite, and those observation and state decisions\n\
         # exclude code of, as reviewed. Maintained by\n\
         # `cargo vendor-check provenance`; checked by `cargo vendor-check\n\
         # check provenance`.\n\
         schema = 1\n",
    );
    for entry in entries {
        text.push_str(&format!(
            "\n[[function]]\nartifact = \"{}\"\nmember = \"{}\"\nsymbol = \"{}\"\ncode = \"{}\"\n",
            entry.artifact, entry.member, entry.symbol, entry.code
        ));
        if !entry.decisions.is_empty() {
            let files: Vec<String> = entry.decisions.iter().map(|f| format!("\"{f}\"")).collect();
            text.push_str(&format!("decisions = [{}]\n", files.join(", ")));
        }
    }
    text
}

/// The registered functions of `chip`: every vendor function production,
/// the register model or a verification decision cites.
pub fn registered(root: &Path, chip: &str) -> Result<Vec<Entry>> {
    let path = crate::artifacts::project::Project::new(root, chip)?.provenance_registry();
    parse(&std::fs::read_to_string(root.join(&path)).map_err(|e| format!("{path}: {e}"))?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registry_round_trips() {
        let entries = vec![
            Entry {
                artifact: "libbtbb".into(),
                member: "bt_bb_v2.o".into(),
                symbol: "bt_bb_v2_init_cmplx".into(),
                code: "ab".into(),
                decisions: vec![],
            },
            Entry {
                artifact: "libpp".into(),
                member: "hal_mac_tx.o".into(),
                symbol: "mac_tx_set_txop_q".into(),
                code: "abc".into(),
                decisions: vec!["state.rs".into()],
            },
        ];
        assert_eq!(parse(&render(&entries)).unwrap(), entries);
    }

    #[test]
    fn this_checkout_registers_functions() {
        let repository = crate::artifacts::repository();
        let root = repository.path();
        assert!(!registered(&root, "esp32s31").unwrap().is_empty());
    }
}
