//! Reviewed provenance catalog and coarse address-range evidence.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Error, Result};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterEvidenceCatalog {
    pub schema: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<RegisterEvidenceSource>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<RegisterEvidenceRange>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterEvidenceSource {
    pub id: String,
    pub description: String,
    /// What kind of observation the source is, when it is one awaiting a
    /// typed record: a hardware observation stays unattested until an
    /// attestation bound to its run replaces it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<SourceKind>,
}

/// The kinds of evidence sources that carry a typed marker.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    /// A hardware observation described in prose, not yet attested by a
    /// run record.
    UnattestedHilObservation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterEvidenceRange {
    pub name: String,
    pub start: u64,
    pub end_exclusive: u64,
    pub sources: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RegisterEvidenceSet {
    pub sources: Vec<RegisterEvidenceSource>,
    pub ranges: Vec<RegisterEvidenceRange>,
}

impl RegisterEvidenceCatalog {
    pub fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)?;
        let catalog: Self = toml_edit::de::from_str(&input)
            .map_err(|error| Error::manifest("register evidence catalog", path, error))?;
        catalog
            .validate(path)
            .map_err(|error| Error::manifest("register evidence catalog", path, error))?;
        Ok(catalog)
    }

    fn validate(&self, path: &Path) -> Result<()> {
        if self.schema != 1 {
            return Err(Error::message(format!(
                "register evidence catalog {} requires schema = 1",
                path.display()
            )));
        }
        let mut sources = BTreeSet::new();
        for source in &self.sources {
            validate_id(&source.id, "evidence source")?;
            if source.description.trim().is_empty() {
                return Err(Error::message(format!(
                    "evidence source {:?} has no description",
                    source.id
                )));
            }
            if !sources.insert(source.id.as_str()) {
                return Err(Error::message(format!(
                    "duplicate evidence source {:?}",
                    source.id
                )));
            }
        }
        let mut names = BTreeSet::new();
        let mut ranges = Vec::new();
        for range in &self.ranges {
            validate_id(&range.name, "evidence range")?;
            if !names.insert(range.name.as_str()) {
                return Err(Error::message(format!(
                    "duplicate evidence range {:?}",
                    range.name
                )));
            }
            if range.start >= range.end_exclusive {
                return Err(Error::message(format!(
                    "evidence range {:?} is empty or reversed",
                    range.name
                )));
            }
            if range.start % 4 != 0 || range.end_exclusive % 4 != 0 {
                return Err(Error::message(format!(
                    "evidence range {:?} is not word-aligned",
                    range.name
                )));
            }
            validate_sources(&range.name, &range.sources)?;
            if let Some(source) = range
                .sources
                .iter()
                .find(|source| !sources.contains(source.as_str()))
            {
                return Err(Error::message(format!(
                    "evidence range {:?} references undefined source {source:?}",
                    range.name
                )));
            }
            ranges.push((range.start, range.end_exclusive, range.name.as_str()));
        }
        ranges.sort_unstable();
        for pair in ranges.windows(2) {
            if pair[0].1 > pair[1].0 {
                return Err(Error::message(format!(
                    "evidence ranges {:?} and {:?} overlap",
                    pair[0].2, pair[1].2
                )));
            }
        }
        Ok(())
    }
}

impl RegisterEvidenceSet {
    pub fn load_all(paths: &[PathBuf]) -> Result<Self> {
        let mut sources = BTreeMap::new();
        let mut ranges = BTreeMap::new();
        for path in paths {
            let catalog = RegisterEvidenceCatalog::load(path)?;
            for source in catalog.sources {
                let id = source.id.clone();
                if sources.insert(id.clone(), source).is_some() {
                    return Err(Error::message(format!(
                        "duplicate evidence source {id:?} across catalogs"
                    )));
                }
            }
            for range in catalog.ranges {
                let name = range.name.clone();
                if ranges.insert(name.clone(), range).is_some() {
                    return Err(Error::message(format!(
                        "duplicate evidence range {name:?} across catalogs"
                    )));
                }
            }
        }
        let set = Self {
            sources: sources.into_values().collect(),
            ranges: ranges.into_values().collect(),
        };
        set.validate_combined_ranges()?;
        Ok(set)
    }

    pub fn source_ids(&self) -> BTreeSet<&str> {
        self.sources
            .iter()
            .map(|source| source.id.as_str())
            .collect()
    }

    pub fn validate_references<'a>(
        &self,
        context: &str,
        references: impl IntoIterator<Item = &'a str>,
    ) -> Result<()> {
        let sources = self.source_ids();
        if let Some(reference) = references
            .into_iter()
            .find(|reference| !sources.contains(reference))
        {
            return Err(Error::message(format!(
                "{context} references undefined evidence source {reference:?}"
            )));
        }
        Ok(())
    }

    fn validate_combined_ranges(&self) -> Result<()> {
        let mut ranges = self
            .ranges
            .iter()
            .map(|range| (range.start, range.end_exclusive, range.name.as_str()))
            .collect::<Vec<_>>();
        ranges.sort_unstable();
        for pair in ranges.windows(2) {
            if pair[0].1 > pair[1].0 {
                return Err(Error::message(format!(
                    "evidence ranges {:?} and {:?} overlap across catalogs",
                    pair[0].2, pair[1].2
                )));
            }
        }
        Ok(())
    }
}

fn validate_sources(owner: &str, sources: &[String]) -> Result<()> {
    if sources.is_empty() {
        return Err(Error::message(format!("{owner:?} has no evidence sources")));
    }
    let mut unique = BTreeSet::new();
    if sources
        .iter()
        .any(|source| source.is_empty() || !unique.insert(source))
    {
        return Err(Error::message(format!(
            "{owner:?} has an empty or duplicate evidence source"
        )));
    }
    Ok(())
}

fn validate_id(value: &str, kind: &str) -> Result<()> {
    if value.is_empty()
        || !value
            .bytes()
            .all(|byte| byte == b'_' || byte == b'-' || byte.is_ascii_alphanumeric())
    {
        return Err(Error::message(format!("invalid {kind} id {value:?}")));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_unknown_range_sources_and_overlaps() {
        let path = Path::new("fixture.toml");
        let mut catalog = RegisterEvidenceCatalog {
            schema: 1,
            sources: vec![RegisterEvidenceSource {
                id: "REVIEW".to_owned(),
                description: "reviewed source".to_owned(),
                kind: None,
            }],
            ranges: vec![RegisterEvidenceRange {
                name: "BLOCK_A".to_owned(),
                start: 0x1000,
                end_exclusive: 0x1010,
                sources: vec!["MISSING".to_owned()],
            }],
        };
        assert!(catalog.validate(path).is_err());
        catalog.ranges[0].sources[0] = "REVIEW".to_owned();
        catalog.ranges.push(RegisterEvidenceRange {
            name: "BLOCK_B".to_owned(),
            start: 0x100c,
            end_exclusive: 0x1020,
            sources: vec!["REVIEW".to_owned()],
        });
        assert!(catalog.validate(path).is_err());
    }

    #[test]
    fn unattested_hil_observations_carry_a_typed_kind() {
        let catalog: RegisterEvidenceCatalog = toml_edit::de::from_str(
            "schema = 1\n[[sources]]\nid = \"HIL_X\"\ndescription = \"seen\"\nkind = \"unattested-hil-observation\"\n[[sources]]\nid = \"DOC\"\ndescription = \"read\"\n",
        )
        .unwrap();
        assert_eq!(
            catalog.sources[0].kind,
            Some(SourceKind::UnattestedHilObservation)
        );
        assert_eq!(catalog.sources[1].kind, None);
        assert!(
            toml_edit::de::from_str::<RegisterEvidenceCatalog>(
                "schema = 1\n[[sources]]\nid = \"X\"\ndescription = \"d\"\nkind = \"rumor\"\n"
            )
            .is_err()
        );
    }
}
