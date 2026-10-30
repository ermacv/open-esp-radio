//! The workspace's HIL scenarios, from its Cargo metadata.

use std::path::Path;

use serde_json::Value;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The package name prefix of every scenario program.
pub const PREFIX: &str = "oer-hil-scenarios-";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Scenario {
    /// The package name without [`PREFIX`].
    pub name: String,
    pub package: String,
    pub description: String,
}

/// The workspace's `cargo metadata --no-deps`.
pub fn metadata(root: &Path) -> Result<Value> {
    let output =
        std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
            .current_dir(root)
            .args(["metadata", "--format-version", "1", "--no-deps", "--locked"])
            .output()?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// The scenarios among `metadata`'s packages, by name. A package that says
/// it is a scenario without the prefix is an error.
pub fn discover(metadata: &Value) -> Result<Vec<Scenario>> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata lists no packages")?;
    let mut scenarios = Vec::new();
    for package in packages {
        if package.pointer("/metadata/open-radio/hil-scenario") != Some(&Value::Bool(true)) {
            continue;
        }
        let name = package["name"].as_str().ok_or("a package without a name")?;
        let Some(short) = name.strip_prefix(PREFIX) else {
            return Err(format!("scenario package {name} does not start with {PREFIX}").into());
        };
        scenarios.push(Scenario {
            name: short.to_owned(),
            package: name.to_owned(),
            description: package["description"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
        });
    }
    scenarios.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(scenarios)
}

/// The scenario called `name`.
pub fn find<'a>(scenarios: &'a [Scenario], name: &str) -> Result<&'a Scenario> {
    scenarios
        .iter()
        .find(|scenario| scenario.name == name)
        .ok_or_else(|| {
            let names = scenarios
                .iter()
                .map(|scenario| scenario.name.as_str())
                .collect::<Vec<_>>();
            format!("no HIL scenario `{name}`; scenarios: {}", names.join(", ")).into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(name: &str, scenario: Option<bool>) -> Value {
        let mut package = serde_json::json!({"name": name, "description": format!("{name} text"), "metadata": null});
        if let Some(flag) = scenario {
            package["metadata"] = serde_json::json!({"open-radio": {"hil-scenario": flag}});
        }
        package
    }

    #[test]
    fn only_flagged_packages_are_scenarios_by_their_short_name() {
        let metadata = serde_json::json!({"packages": [
            package("oer-hil-scenarios-zeta", Some(true)),
            package("oer-hil-scenarios-images-esp32s31-rpc", None),
            package("oer-hil-harness", Some(false)),
            package("oer-hil-scenarios-aes", Some(true)),
        ]});
        let scenarios = discover(&metadata).unwrap();
        let names = scenarios
            .iter()
            .map(|scenario| scenario.name.as_str())
            .collect::<Vec<_>>();
        assert_eq!(names, ["aes", "zeta"]);
        assert_eq!(
            find(&scenarios, "aes").unwrap().package,
            "oer-hil-scenarios-aes"
        );
        let missing = find(&scenarios, "wifi").unwrap_err().to_string();
        assert!(missing.contains("aes, zeta"), "{missing}");
    }

    #[test]
    fn a_flagged_package_outside_the_prefix_is_refused() {
        let metadata = serde_json::json!({"packages": [package("oer-tool-other", Some(true))]});
        assert!(discover(&metadata).is_err());
    }
}
