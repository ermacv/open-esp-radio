//! Optional project policy over otherwise valid hardware-model names.

use std::{collections::BTreeSet, fs, path::Path};

use serde::{Deserialize, Serialize};
use svd_rs::{Device, RegisterCluster};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterLintPack {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forbidden_field_name_substrings: Vec<String>,
}

impl RegisterLintPack {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let pack: Self = toml_edit::de::from_str(&input)
            .map_err(|error| Error::manifest("register lint pack", path, error))?;
        pack.validate(path)
            .map_err(|error| Error::manifest("register lint pack", path, error))?;
        Ok(pack)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::message(format!(
                "register lint pack {} requires schema = 1",
                path.display()
            )));
        }
        let mut patterns = BTreeSet::new();
        if self
            .forbidden_field_name_substrings
            .iter()
            .any(|pattern| pattern.trim().is_empty() || !patterns.insert(pattern.as_str()))
        {
            return Err(Error::message(format!(
                "register lint pack {} contains an empty or duplicate field-name substring",
                path.display()
            )));
        }
        Ok(())
    }

    pub(super) fn validate_device(&self, device: &Device) -> Result<()> {
        for peripheral in &device.peripherals {
            if let Some(children) = &peripheral.registers {
                self.validate_children(&peripheral.name, children)?;
            }
        }
        Ok(())
    }

    fn validate_children(&self, parent: &str, children: &[RegisterCluster]) -> Result<()> {
        for child in children {
            match child {
                RegisterCluster::Register(register) => {
                    let identity = format!("{parent}.{}", register.name);
                    for field in register.fields.iter().flatten() {
                        if let Some(pattern) = self
                            .forbidden_field_name_substrings
                            .iter()
                            .find(|pattern| field.name.contains(pattern.as_str()))
                        {
                            return Err(Error::message(format!(
                                "register field {identity}.{} contains project-forbidden substring {pattern:?}",
                                field.name
                            )));
                        }
                    }
                }
                RegisterCluster::Cluster(cluster) => self
                    .validate_children(&format!("{parent}.{}", cluster.name), &cluster.children)?,
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_and_duplicate_project_patterns() {
        for patterns in [vec!["".to_owned()], vec!["RESERVED".to_owned(); 2]] {
            let pack = RegisterLintPack {
                schema: 1,
                forbidden_field_name_substrings: patterns,
            };
            assert!(pack.validate(Path::new("lints.toml")).is_err());
        }
    }
}
