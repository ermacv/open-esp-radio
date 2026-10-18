//! Cargo lock files read as text: the `[[package]]` entries a workspace
//! resolved.

use toml::Value;

use crate::Result;

/// One `[[package]]` entry of a lock file.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Locked {
    pub name: String,
    pub version: String,
    /// `registry+…` or `git+…#<revision>`; `None` for a path package.
    pub source: Option<String>,
    /// Its dependencies as the lock names them: `name`, `name version` or
    /// `name version (source)`.
    pub dependencies: Vec<String>,
}

impl Locked {
    /// The git revision a `git+…#<revision>` source pins.
    pub fn revision(&self) -> Option<&str> {
        let source = self.source.as_deref()?;
        source
            .starts_with("git+")
            .then(|| source.rsplit_once('#').map(|(_, revision)| revision))
            .flatten()
    }
}

/// Every entry of the lock file `text`; `name` labels errors.
pub fn parse(text: &str, name: &str) -> Result<Vec<Locked>> {
    let document: toml::Table = toml::from_str(text).map_err(|error| format!("{name}: {error}"))?;
    let packages = document
        .get("package")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{name}: no [[package]] entries"))?;
    packages
        .iter()
        .map(|entry| {
            let field = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
            Ok(Locked {
                name: field("name").ok_or_else(|| format!("{name}: an entry without a name"))?,
                version: field("version")
                    .ok_or_else(|| format!("{name}: an entry without a version"))?,
                source: field("source"),
                dependencies: entry
                    .get("dependencies")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_carry_their_source_revision_and_dependencies() {
        let locked = parse(
            "version = 4\n[[package]]\nname = \"a\"\nversion = \"0.1.0\"\ndependencies = [\"b\"]\n\
             [[package]]\nname = \"b\"\nversion = \"1.2.3\"\nsource = \"git+https://x/y?rev=abc#abcdef1\"\n",
            "Cargo.lock",
        )
        .unwrap();
        assert_eq!(locked[0].dependencies, ["b"]);
        assert_eq!(locked[0].revision(), None);
        assert_eq!(locked[1].revision(), Some("abcdef1"));
        assert!(
            parse("version = 4\n", "x/Cargo.lock")
                .unwrap_err()
                .contains("x/Cargo.lock")
        );
    }
}
