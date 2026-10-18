//! Runtime features added to or removed from an image class's own, for an
//! experiment that compares firmware differing only in them.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

/// Features added to (`+name`) and removed from (`-name`) an image class's
/// runtime features. An image built with a non-empty delta is not its class's
/// image: it serves experiments only and never qualifies.
#[derive(Clone, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FeatureDelta {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub add: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub remove: BTreeSet<String>,
}

impl FeatureDelta {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }

    /// `base`, a comma-separated feature list, with this delta applied: the
    /// removed features left out and the added ones appended in order.
    pub fn apply(&self, base: &str) -> String {
        base.split(',')
            .filter(|feature| !feature.is_empty() && !self.remove.contains(*feature))
            .map(str::to_owned)
            .chain(
                self.add
                    .iter()
                    .filter(|feature| !base.split(',').any(|known| known == feature.as_str()))
                    .cloned(),
            )
            .collect::<Vec<_>>()
            .join(",")
    }

    /// The artifact directory suffix of a build with this delta, so it never
    /// reuses the class's own artifacts.
    pub fn suffix(&self) -> String {
        if self.is_empty() {
            return String::new();
        }
        format!(
            "-features-{}",
            &oer_durable::sha256_bytes(self.to_string().as_bytes())[..12]
        )
    }
}

impl std::fmt::Display for FeatureDelta {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let parts = self
            .add
            .iter()
            .map(|feature| format!("+{feature}"))
            .chain(self.remove.iter().map(|feature| format!("-{feature}")))
            .collect::<Vec<_>>();
        f.write_str(&parts.join(","))
    }
}

impl std::str::FromStr for FeatureDelta {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let mut delta = Self::default();
        for part in text
            .split(',')
            .map(str::trim)
            .filter(|part| !part.is_empty())
        {
            let (set, name) = match part.split_at(1) {
                ("+", name) => (&mut delta.add, name),
                ("-", name) => (&mut delta.remove, name),
                _ => return Err(format!("`{part}` is neither +feature nor -feature")),
            };
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/')
            {
                return Err(format!("`{part}` does not name a Cargo feature"));
            }
            if !set.insert(name.to_owned()) {
                return Err(format!("`{name}` is given twice"));
            }
        }
        if let Some(both) = delta.add.intersection(&delta.remove).next() {
            return Err(format!("`{both}` is both added and removed"));
        }
        Ok(delta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_delta_adds_and_removes_runtime_features() {
        let delta: FeatureDelta = "+trace, -psram-stack".parse().unwrap();
        assert_eq!(delta.to_string(), "+trace,-psram-stack");
        assert_eq!(
            delta.apply("open-radio-hil,psram-stack,owned-xarxa"),
            "open-radio-hil,owned-xarxa,trace"
        );
        // Adding a feature the class already has changes nothing.
        let present: FeatureDelta = "+owned-xarxa".parse().unwrap();
        assert_eq!(
            present.apply("open-radio-hil,owned-xarxa"),
            "open-radio-hil,owned-xarxa"
        );
        assert!(FeatureDelta::default().suffix().is_empty());
        assert!(delta.suffix().starts_with("-features-"));
        for invalid in ["trace", "+", "+a,+a", "+a,-a", "+a b"] {
            assert!(invalid.parse::<FeatureDelta>().is_err(), "{invalid}");
        }
    }
}
