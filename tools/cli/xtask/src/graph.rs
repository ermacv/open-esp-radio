//! The shape `cargo metadata` output must have before it is parsed.

use crate::Result;
use serde_json::Value;
use std::{collections::BTreeSet, path::Path};

fn required<'a>(value: &'a Value, name: &str) -> Result<&'a Value> {
    value
        .get(name)
        .ok_or_else(|| format!("Cargo metadata field missing: {name}").into())
}
fn string(value: &Value) -> Result<&str> {
    value
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| "Cargo metadata expected nonempty string".into())
}
fn array(value: &Value) -> Result<&Vec<Value>> {
    value
        .as_array()
        .ok_or_else(|| "Cargo metadata expected array".into())
}
fn nullable_string(value: &Value) -> Result<()> {
    if !value.is_null() && !value.is_string() {
        return Err("Cargo metadata expected nullable string".into());
    }
    Ok(())
}
fn kind(value: &Value) -> Result<()> {
    if value.is_null() || matches!(value.as_str(), Some("build" | "dev")) {
        return Ok(());
    }
    Err("Cargo metadata dependency kind missing or unknown".into())
}

pub(crate) fn validate_packages(document: &Value) -> Result<()> {
    let mut ids = BTreeSet::new();
    for package in array(required(document, "packages")?)? {
        for name in ["id", "name", "version", "manifest_path"] {
            string(required(package, name)?)?;
        }
        if !Path::new(string(required(package, "manifest_path")?)?).is_absolute() {
            return Err("package manifest path must be absolute".into());
        }
        if !ids.insert(string(required(package, "id")?)?) {
            return Err("duplicate package identity".into());
        }
        nullable_string(required(package, "source")?)?;
        if !required(package, "features")?.is_object() {
            return Err("declared feature map missing".into());
        }
        for dependency in array(required(package, "dependencies")?)? {
            string(required(dependency, "name")?)?;
            kind(required(dependency, "kind")?)?;
            nullable_string(required(dependency, "source")?)?;
            if !required(dependency, "optional")?.is_boolean() {
                return Err("declared optional dependency status invalid".into());
            }
            if let Some(path) = dependency.get("path").filter(|path| !path.is_null())
                && !Path::new(string(path)?).is_absolute()
            {
                return Err("declared dependency path must be absolute".into());
            }
        }
    }
    Ok(())
}
