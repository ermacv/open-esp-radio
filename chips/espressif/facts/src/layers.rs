//! Resolution of the layer files: `extends`, overrides and their checks.
//!
//! A layer is a TOML table with `schema = 2`. `extends` names its parents
//! (one path or a list, relative to the file), each resolved first; parents
//! share no key. The layer then adds its own keys: a key a parent already
//! sets is an override, and every override names its source in the layer's
//! `[override]` table (`"dotted.key" = "SOURCE: ..."`). Tables merge key by
//! key; any other value, an array included, is one fact.

use alloc::{
    collections::BTreeMap,
    format,
    string::{String, ToString},
    vec::Vec,
};

use toml::{Table, Value};

use crate::{Error, Result, SCHEMA};

/// Where the layer files are read from: paths are relative to the repository
/// root, with `/` separators.
pub trait Source {
    /// The text of the file at `path`.
    fn read(&mut self, path: &str) -> Result<String>;
    /// The names of the files in `directory`, none when it does not exist.
    fn files(&mut self, directory: &str) -> Result<Vec<String>>;
}

/// The resolved layers of one file: the merged table, without the layer
/// keys (`schema`, `extends`, `override`), and every file read, parents
/// first, the resolved file last.
#[derive(Clone, Debug, PartialEq)]
pub struct Layers {
    pub table: Table,
    pub files: Vec<String>,
}

impl Layers {
    /// The resolved file: the top layer.
    pub fn top(&self) -> &str {
        self.files.last().map_or("", String::as_str)
    }
}

/// Resolve the file at `path` with every layer it extends.
pub fn resolve(source: &mut dyn Source, path: &str) -> Result<Layers> {
    let mut files = Vec::new();
    let merged = resolve_file(source, path, &mut Vec::new(), &mut files)?;
    Ok(Layers {
        table: merged.table,
        files,
    })
}

/// The files the resolution of `path` reads, parents first, from their
/// `extends` alone: no schema, key or override is checked.
pub fn chain(source: &mut dyn Source, path: &str) -> Result<Vec<String>> {
    fn walk(
        source: &mut dyn Source,
        path: &str,
        stack: &mut Vec<String>,
        files: &mut Vec<String>,
    ) -> Result<()> {
        enter(path, stack, files)?;
        let mut table = parse(source, path)?;
        stack.push(path.to_string());
        for parent in extends(&mut table, path)? {
            walk(source, &parent, stack, files)?;
        }
        stack.pop();
        files.push(path.to_string());
        Ok(())
    }
    let mut files = Vec::new();
    walk(source, path, &mut Vec::new(), &mut files)?;
    Ok(files)
}

/// The merged table and the file that set each of its leaf values, by
/// dotted key.
#[derive(Default)]
struct Merged {
    table: Table,
    origin: BTreeMap<String, String>,
}

fn resolve_file(
    source: &mut dyn Source,
    path: &str,
    stack: &mut Vec<String>,
    files: &mut Vec<String>,
) -> Result<Merged> {
    enter(path, stack, files)?;
    let mut table = parse(source, path)?;
    match table.remove("schema") {
        Some(Value::Integer(schema)) if schema == i64::from(SCHEMA) => {}
        Some(Value::Integer(schema)) => {
            return Err(format!(
                "{path} has schema {schema}; the facts model reads schema {SCHEMA}"
            )
            .into());
        }
        Some(_) => return Err(format!("{path}: `schema` is not a number").into()),
        None => return Err(format!("{path} names no schema").into()),
    }
    let parents = extends(&mut table, path)?;
    let mut overrides = overrides(&mut table, path)?;
    stack.push(path.to_string());
    let mut merged = Merged::default();
    for parent in parents {
        let layer = resolve_file(source, &parent, stack, files)?;
        join(
            &mut merged.table,
            layer.table,
            "",
            &merged.origin,
            &layer.origin,
        )?;
        merged.origin.extend(layer.origin);
    }
    stack.pop();
    apply(
        &mut merged.table,
        table,
        "",
        path,
        &mut merged.origin,
        &mut overrides,
    )?;
    if let Some(key) = overrides.keys().next() {
        return Err(
            format!("{path}: `[override]` names `{key}`, which no layer it extends sets").into(),
        );
    }
    files.push(path.to_string());
    Ok(merged)
}

/// Refuse a cycle and a layer reached twice.
fn enter(path: &str, stack: &[String], files: &[String]) -> Result<()> {
    if let Some(at) = stack.iter().position(|file| file == path) {
        let mut cycle = stack.get(at..).unwrap_or_default().join(" → ");
        cycle.push_str(" → ");
        cycle.push_str(path);
        return Err(format!("the layers extend in a cycle: {cycle}").into());
    }
    if files.iter().any(|file| file == path) {
        return Err(format!("{path} is reached twice; extend each layer once").into());
    }
    Ok(())
}

fn parse(source: &mut dyn Source, path: &str) -> Result<Table> {
    let text = source.read(path)?;
    toml::from_str(&text).map_err(|error| Error::from(format!("{path}: {error}")))
}

/// Remove `extends` and resolve its paths against the file's directory.
fn extends(table: &mut Table, path: &str) -> Result<Vec<String>> {
    let parents = match table.remove("extends") {
        None => Vec::new(),
        Some(Value::String(parent)) => alloc::vec![parent],
        Some(Value::Array(parents)) => parents
            .into_iter()
            .map(|parent| match parent {
                Value::String(parent) => Ok(parent),
                other => Err(Error::from(format!(
                    "{path}: `extends` names {other:?}, not a path"
                ))),
            })
            .collect::<Result<_>>()?,
        Some(other) => {
            return Err(
                format!("{path}: `extends` is {other:?}, not a path or a list of paths").into(),
            );
        }
    };
    parents
        .iter()
        .map(|parent| relative(path, parent))
        .collect()
}

/// Remove `[override]`: each overridden dotted key and its source.
fn overrides(table: &mut Table, path: &str) -> Result<BTreeMap<String, String>> {
    let Some(overrides) = table.remove("override") else {
        return Ok(BTreeMap::new());
    };
    let Value::Table(overrides) = overrides else {
        return Err(format!("{path}: `override` is not a table").into());
    };
    overrides
        .into_iter()
        .map(|(key, source)| match source {
            Value::String(source) if source.starts_with("SOURCE:") && source.len() > 7 => {
                Ok((key, source))
            }
            _ => Err(Error::from(format!(
                "{path}: the override of `{key}` names no `SOURCE: ...`"
            ))),
        })
        .collect()
}

/// `relative` against the directory of `path`, both below the root.
fn relative(path: &str, relative: &str) -> Result<String> {
    if relative.starts_with('/') {
        return Err(format!("{path}: `extends` names absolute path {relative}").into());
    }
    let mut parts: Vec<&str> = path.split('/').collect();
    parts.pop();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(format!("{path}: `extends` leaves the root: {relative}").into());
                }
            }
            part => parts.push(part),
        }
    }
    Ok(parts.join("/"))
}

fn dotted(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}

/// The file that set `key` or a value below it.
fn origin_of<'a>(origin: &'a BTreeMap<String, String>, key: &str) -> &'a str {
    let below = format!("{key}.");
    origin
        .get(key)
        .or_else(|| {
            origin
                .range(below.clone()..)
                .next()
                .filter(|(found, _)| found.starts_with(&below))
                .map(|(_, file)| file)
        })
        .map_or("a layer below", String::as_str)
}

/// Record `file` as the origin of every leaf of `value` at `key`.
fn record(value: &Value, key: &str, file: &str, origin: &mut BTreeMap<String, String>) {
    match value {
        Value::Table(table) => {
            for (child, value) in table {
                record(value, &dotted(key, child), file, origin);
            }
        }
        _ => {
            origin.insert(key.to_string(), file.to_string());
        }
    }
}

/// Merge the layer `from` of one parent into those of the parents before it:
/// they share no value.
fn join(
    into: &mut Table,
    from: Table,
    prefix: &str,
    into_origin: &BTreeMap<String, String>,
    from_origin: &BTreeMap<String, String>,
) -> Result<()> {
    for (key, value) in from {
        let path = dotted(prefix, &key);
        match (into.get_mut(&key), value) {
            (None, value) => {
                into.insert(key, value);
            }
            (Some(Value::Table(into)), Value::Table(from)) => {
                join(into, from, &path, into_origin, from_origin)?;
            }
            (Some(_), _) => {
                return Err(format!(
                    "`{path}` is set by both {} and {}; parents share no fact",
                    origin_of(into_origin, &path),
                    origin_of(from_origin, &path)
                )
                .into());
            }
        }
    }
    Ok(())
}

/// Add the layer `from` of `file` over its parents' merged table.
fn apply(
    into: &mut Table,
    from: Table,
    prefix: &str,
    file: &str,
    origin: &mut BTreeMap<String, String>,
    overrides: &mut BTreeMap<String, String>,
) -> Result<()> {
    for (key, value) in from {
        let path = dotted(prefix, &key);
        match (into.get_mut(&key), value) {
            (None, value) => {
                record(&value, &path, file, origin);
                into.insert(key, value);
            }
            (Some(Value::Table(into)), Value::Table(from)) => {
                apply(into, from, &path, file, origin, overrides)?;
            }
            (Some(slot), value) => {
                if overrides.remove(&path).is_none() {
                    return Err(format!(
                        "{file} sets `{path}`, which {} sets; an override names its \
                         `SOURCE:` in `[override]`",
                        origin_of(origin, &path)
                    )
                    .into());
                }
                let below = format!("{path}.");
                origin.retain(|key, _| *key != path && !key.starts_with(&below));
                record(&value, &path, file, origin);
                *slot = value;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::string::ToString;

    /// Layer files in memory.
    struct Files(Vec<(&'static str, &'static str)>);

    impl Source for Files {
        fn read(&mut self, path: &str) -> Result<String> {
            self.0
                .iter()
                .find(|(file, _)| *file == path)
                .map(|(_, text)| text.to_string())
                .ok_or_else(|| format!("no file {path}").into())
        }

        fn files(&mut self, _: &str) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    fn resolve_in(files: Vec<(&'static str, &'static str)>, path: &str) -> Result<Layers> {
        resolve(&mut Files(files), path)
    }

    fn error(files: Vec<(&'static str, &'static str)>, path: &str) -> String {
        resolve_in(files, path)
            .expect_err("the layers are refused")
            .to_string()
    }

    const BASE: &str = "schema = 2\n[memory]\nsram = 1\nflash = 2\n";

    #[test]
    fn a_layer_adds_to_its_parents_and_reads_them_first() {
        let layers = resolve_in(
            alloc::vec![
                ("a/base.toml", BASE),
                ("a/isa.toml", "schema = 2\nisa = 'rv32imac'\n"),
                (
                    "a/b/top.toml",
                    "schema = 2\nextends = ['../isa.toml', '../base.toml']\n\
                     [memory]\npsram = 3\n",
                ),
            ],
            "a/b/top.toml",
        )
        .expect("the layers resolve");
        assert_eq!(layers.files, ["a/isa.toml", "a/base.toml", "a/b/top.toml"]);
        assert_eq!(layers.top(), "a/b/top.toml");
        let memory = layers.table["memory"].as_table().expect("a table");
        assert_eq!(memory["sram"].as_integer(), Some(1));
        assert_eq!(memory["psram"].as_integer(), Some(3));
        assert_eq!(layers.table["isa"].as_str(), Some("rv32imac"));
        for key in ["schema", "extends", "override"] {
            assert!(!layers.table.contains_key(key), "{key}");
        }
    }

    #[test]
    fn an_override_takes_effect_only_with_its_source() {
        let top = "schema = 2\nextends = 'base.toml'\n[memory]\nsram = 9\n\
                   [override]\n'memory.sram' = 'SOURCE: datasheet v1.0, table 3'\n";
        let layers = resolve_in(
            alloc::vec![("base.toml", BASE), ("top.toml", top)],
            "top.toml",
        )
        .expect("a sourced override resolves");
        assert_eq!(layers.table["memory"]["sram"].as_integer(), Some(9));
        assert_eq!(layers.table["memory"]["flash"].as_integer(), Some(2));

        let silent = "schema = 2\nextends = 'base.toml'\n[memory]\nsram = 9\n";
        let refused = error(
            alloc::vec![("base.toml", BASE), ("top.toml", silent)],
            "top.toml",
        );
        assert!(refused.contains("`memory.sram`"), "{refused}");
        assert!(refused.contains("base.toml"), "{refused}");

        for reason in ["''", "'datasheet'", "'SOURCE:'", "1"] {
            let unsourced = format!(
                "schema = 2\nextends = 'base.toml'\n[memory]\nsram = 9\n\
                 [override]\n'memory.sram' = {reason}\n"
            );
            let unsourced: &'static str = alloc::boxed::Box::leak(unsourced.into_boxed_str());
            let refused = error(
                alloc::vec![("base.toml", BASE), ("top.toml", unsourced)],
                "top.toml",
            );
            assert!(
                refused.contains("names no `SOURCE: ...`"),
                "{reason}: {refused}"
            );
        }
    }

    #[test]
    fn an_override_of_a_key_no_parent_sets_is_refused() {
        let top = "schema = 2\nextends = 'base.toml'\n[memory]\npsram = 9\n\
                   [override]\n'memory.psram' = 'SOURCE: datasheet'\n";
        let refused = error(
            alloc::vec![("base.toml", BASE), ("top.toml", top)],
            "top.toml",
        );
        assert!(refused.contains("`memory.psram`"), "{refused}");
        assert!(refused.contains("no layer it extends sets"), "{refused}");
    }

    #[test]
    fn a_table_replaced_by_a_value_is_an_override() {
        let top = "schema = 2\nextends = 'base.toml'\nmemory = 'none'\n";
        let refused = error(
            alloc::vec![("base.toml", BASE), ("top.toml", top)],
            "top.toml",
        );
        assert!(refused.contains("`memory`"), "{refused}");
        assert!(refused.contains("base.toml"), "{refused}");
    }

    #[test]
    fn parents_that_share_a_fact_are_refused() {
        let refused = error(
            alloc::vec![
                ("a.toml", BASE),
                ("b.toml", "schema = 2\n[memory]\nflash = 5\n"),
                ("top.toml", "schema = 2\nextends = ['a.toml', 'b.toml']\n"),
            ],
            "top.toml",
        );
        assert!(
            refused.contains("`memory.flash` is set by both a.toml and b.toml"),
            "{refused}"
        );
    }

    #[test]
    fn a_cycle_is_refused_with_its_path() {
        let refused = error(
            alloc::vec![
                ("a.toml", "schema = 2\nextends = 'b.toml'\n"),
                ("b.toml", "schema = 2\nextends = 'a.toml'\n"),
            ],
            "a.toml",
        );
        assert!(
            refused.contains("cycle: a.toml → b.toml → a.toml"),
            "{refused}"
        );
        let refused = error(
            alloc::vec![("self.toml", "schema = 2\nextends = 'self.toml'\n")],
            "self.toml",
        );
        assert!(refused.contains("cycle"), "{refused}");
        let chained = chain(
            &mut Files(alloc::vec![
                ("a.toml", "extends = 'b.toml'\n"),
                ("b.toml", "extends = 'a.toml'\n"),
            ]),
            "a.toml",
        );
        assert!(chained.is_err());
    }

    #[test]
    fn a_layer_reached_twice_is_refused() {
        let refused = error(
            alloc::vec![
                ("base.toml", BASE),
                ("a.toml", "schema = 2\nextends = 'base.toml'\n"),
                (
                    "top.toml",
                    "schema = 2\nextends = ['a.toml', 'base.toml']\n"
                ),
            ],
            "top.toml",
        );
        assert!(refused.contains("base.toml is reached twice"), "{refused}");
    }

    #[test]
    fn a_missing_layer_schema_or_root_escape_is_refused() {
        let missing = error(
            alloc::vec![("top.toml", "schema = 2\nextends = 'gone.toml'\n")],
            "top.toml",
        );
        assert!(missing.contains("gone.toml"), "{missing}");
        let unversioned = error(alloc::vec![("top.toml", "id = 'x'\n")], "top.toml");
        assert!(unversioned.contains("names no schema"), "{unversioned}");
        let old = error(alloc::vec![("top.toml", "schema = 1\n")], "top.toml");
        assert!(old.contains("schema 1"), "{old}");
        let escape = error(
            alloc::vec![("top.toml", "schema = 2\nextends = '../up.toml'\n")],
            "top.toml",
        );
        assert!(escape.contains("leaves the root"), "{escape}");
    }

    #[test]
    fn the_chain_follows_extends_without_checking_the_layers() {
        let files = chain(
            &mut Files(alloc::vec![
                ("x/top.toml", "schema = 9\nextends = 'base.toml'\n"),
                ("x/base.toml", "anything = 1\n"),
            ]),
            "x/top.toml",
        )
        .expect("the chain is read");
        assert_eq!(files, ["x/base.toml", "x/top.toml"]);
    }
}
