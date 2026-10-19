//! The prose of a chip's register model: every `description` its model and
//! evidence files carry, at any depth. Provenance cites the vendor functions
//! these texts name, so the register model owns which files they come from.

use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// The directories of `chip`'s register model whose descriptions cite
/// vendor code, relative to the repository root: the evidence sources and
/// the model's peripherals, registers and fields.
pub fn description_directories(chip: &str) -> [PathBuf; 2] {
    [
        PathBuf::from(format!("chips/{chip}/registers/evidence")),
        PathBuf::from(format!("chips/{chip}/registers/model")),
    ]
}

/// Every `description` string of the TOML files below `chip`'s
/// [`description_directories`], in path order, with the file it is in,
/// relative to `root`.
pub fn descriptions(root: &Path, chip: &str) -> Result<Vec<(PathBuf, String)>> {
    let mut texts = Vec::new();
    for directory in description_directories(chip) {
        let mut files = Vec::new();
        toml_files(&root.join(directory), &mut files)?;
        files.sort();
        for file in files {
            let text = std::fs::read_to_string(&file)?;
            let document = text
                .parse::<toml_edit::DocumentMut>()
                .map_err(|error| Error::manifest("register model file", &file, error))?;
            let mut found = Vec::new();
            item_descriptions(document.as_item(), &mut found);
            let relative = file.strip_prefix(root).unwrap_or(&file).to_path_buf();
            texts.extend(found.into_iter().map(|text| (relative.clone(), text)));
        }
    }
    Ok(texts)
}

fn toml_files(directory: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            toml_files(&path, out)?;
        } else if path.extension().and_then(|e| e.to_str()) == Some("toml") {
            out.push(path);
        }
    }
    Ok(())
}

fn item_descriptions(item: &toml_edit::Item, out: &mut Vec<String>) {
    match item {
        toml_edit::Item::Table(table) => {
            for (key, item) in table.iter() {
                match item.as_str() {
                    Some(text) if key == "description" => out.push(text.to_owned()),
                    _ => item_descriptions(item, out),
                }
            }
        }
        toml_edit::Item::ArrayOfTables(tables) => {
            for table in tables.iter() {
                for (key, item) in table.iter() {
                    match item.as_str() {
                        Some(text) if key == "description" => out.push(text.to_owned()),
                        _ => item_descriptions(item, out),
                    }
                }
            }
        }
        toml_edit::Item::Value(value) => value_descriptions(value, out),
        toml_edit::Item::None => {}
    }
}

fn value_descriptions(value: &toml_edit::Value, out: &mut Vec<String>) {
    match value {
        toml_edit::Value::InlineTable(table) => {
            for (key, value) in table.iter() {
                match value.as_str() {
                    Some(text) if key == "description" => out.push(text.to_owned()),
                    _ => value_descriptions(value, out),
                }
            }
        }
        toml_edit::Value::Array(values) => {
            for value in values.iter() {
                value_descriptions(value, out);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_are_found_at_any_depth_and_names_are_not() {
        let root = tempfile::tempdir().unwrap();
        let model = root.path().join("chips/chip/registers/model/peripherals");
        std::fs::create_dir_all(&model).unwrap();
        std::fs::create_dir_all(root.path().join("chips/chip/registers/evidence")).unwrap();
        std::fs::write(
            model.join("a.toml"),
            "[[sources]]\ndescription = \"complete bt_bb_v2_init_cmplx\"\n\
             [[peripherals.registers]]\n\
             [peripherals.registers.register]\nname = \"rcGetRate\"\n\
             [[peripherals.registers.register.fields]]\n\
             description = \"bt_bb_rx_dpo_set replaces this field\"\n\
             values = [{ name = \"on\", description = \"inline phy_enable\" }]\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("chips/chip/registers/evidence/e.toml"),
            "[[source]]\nid = \"x\"\ndescription = \"libpp[pm.o]::pm_on\"\n",
        )
        .unwrap();
        let model = PathBuf::from("chips/chip/registers/model/peripherals/a.toml");
        let evidence = PathBuf::from("chips/chip/registers/evidence/e.toml");
        assert_eq!(
            descriptions(root.path(), "chip").unwrap(),
            [
                (evidence, "libpp[pm.o]::pm_on".to_owned()),
                (model.clone(), "complete bt_bb_v2_init_cmplx".to_owned()),
                (
                    model.clone(),
                    "bt_bb_rx_dpo_set replaces this field".to_owned()
                ),
                (model, "inline phy_enable".to_owned()),
            ]
        );
    }
}
