//! Path-package closures: the repository packages a package reaches through
//! its dependencies, and the packages that reach a set.
//!
//! [`Model::closure`] follows path dependencies from manifests as text, the
//! way Cargo would resolve them:
//!
//! - [`Edges`] selects the dependency kinds: normal and build dependencies
//!   (what a build of the roots compiles), or every kind;
//! - a [`Target`] keeps the dependencies a `[target.'cfg(…)']` table
//!   declares only when the cfg holds for that triple (without one, every
//!   table counts);
//! - [`Features`] either follows every optional dependency, or resolves the
//!   features of every reached package from the roots' request, as Cargo
//!   unifies them (`default`, `dep:x`, `x/f`, `x?/f`, implicit features).
//!
//! Registry and Git packages are outside the repository and never part of a
//! closure.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Model, Result,
    manifest::{Dependency, Kind, Package},
};

/// The dependency kinds a closure follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Edges {
    /// Normal and build dependencies: what building the roots compiles.
    Build,
    /// Every kind, dev dependencies of every reached package included.
    All,
}

impl Edges {
    fn follows(self, kind: Kind) -> bool {
        self == Self::All || kind != Kind::Development
    }
}

/// Which optional dependencies a closure follows.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Features {
    /// Every optional dependency, whatever enables it.
    All,
    /// Those the features enabled from the roots reach.
    Resolved {
        /// Whether the roots' `default` feature is on.
        default: bool,
        /// Features requested on the roots.
        features: Vec<String>,
    },
}

/// The facts of a target triple a `cfg(…)` may test.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Target {
    pub triple: String,
    pub arch: String,
    pub vendor: String,
    pub os: String,
    pub env: String,
    /// `unix` or `windows`; empty for bare-metal targets.
    pub family: String,
}

impl Target {
    /// The facts of `triple` (`<arch>-<vendor>-<os>[-<env>]`); a RISC-V
    /// architecture drops its ISA extensions (`riscv32imafc` is `riscv32`).
    pub fn new(triple: &str) -> Result<Self> {
        let parts: Vec<&str> = triple.split('-').collect();
        let (arch, vendor, os, env) = match parts.as_slice() {
            [arch, vendor, os] => (*arch, *vendor, *os, ""),
            [arch, vendor, os, env] => (*arch, *vendor, *os, *env),
            _ => {
                return Err(format!(
                    "`{triple}` is not an <arch>-<vendor>-<os>[-<env>] triple"
                ));
            }
        };
        let arch = ["riscv32", "riscv64"]
            .into_iter()
            .find(|base| arch.starts_with(base))
            .unwrap_or(arch);
        let family = match os {
            "linux" | "macos" | "freebsd" | "netbsd" | "openbsd" | "android" | "ios" => "unix",
            "windows" => "windows",
            _ => "",
        };
        Ok(Self {
            triple: triple.to_owned(),
            arch: arch.to_owned(),
            vendor: vendor.to_owned(),
            os: os.to_owned(),
            env: env.to_owned(),
            family: family.to_owned(),
        })
    }

    /// Whether the key of a `[target.'…']` table — a `cfg(…)` expression or
    /// a triple — selects this target. A cfg this model cannot evaluate is
    /// an error, never a guess.
    pub fn selects(&self, key: &str) -> Result<bool> {
        let Some(expression) = key
            .trim()
            .strip_prefix("cfg(")
            .and_then(|rest| rest.strip_suffix(')'))
        else {
            return Ok(key.trim() == self.triple);
        };
        let mut parser = Cfg {
            text: expression,
            target: self,
        };
        let value = parser.predicate()?;
        parser.skip_space();
        if parser.text.is_empty() {
            Ok(value)
        } else {
            Err(format!("cannot evaluate `{key}`"))
        }
    }
}

/// A recursive-descent evaluator of a `cfg(…)` predicate.
struct Cfg<'a> {
    text: &'a str,
    target: &'a Target,
}

impl Cfg<'_> {
    fn skip_space(&mut self) {
        self.text = self.text.trim_start();
    }

    fn identifier(&mut self) -> Result<&str> {
        self.skip_space();
        let end = self
            .text
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(self.text.len());
        if end == 0 {
            return Err(format!("expected a cfg name at `{}`", self.text));
        }
        let (name, rest) = self.text.split_at(end);
        self.text = rest;
        Ok(name)
    }

    fn eat(&mut self, token: char) -> bool {
        self.skip_space();
        match self.text.strip_prefix(token) {
            Some(rest) => {
                self.text = rest;
                true
            }
            None => false,
        }
    }

    fn predicate(&mut self) -> Result<bool> {
        let name = self.identifier()?.to_owned();
        if matches!(name.as_str(), "all" | "any" | "not") {
            if !self.eat('(') {
                return Err(format!("`{name}` without arguments"));
            }
            let mut values = Vec::new();
            while !self.eat(')') {
                values.push(self.predicate()?);
                if !self.eat(',') && !self.text.trim_start().starts_with(')') {
                    return Err(format!("malformed `{name}(…)`"));
                }
            }
            return match name.as_str() {
                "all" => Ok(values.iter().all(|value| *value)),
                "any" => Ok(values.iter().any(|value| *value)),
                _ => match values.as_slice() {
                    [value] => Ok(!value),
                    _ => Err("`not` takes one predicate".to_owned()),
                },
            };
        }
        if self.eat('=') {
            self.skip_space();
            let literal = self
                .text
                .strip_prefix('"')
                .and_then(|rest| rest.split_once('"'))
                .ok_or_else(|| format!("`{name} =` without a string"))?;
            let (value, rest) = literal;
            self.text = rest;
            let fact = match name.as_str() {
                "target_arch" => &self.target.arch,
                "target_vendor" => &self.target.vendor,
                "target_os" => &self.target.os,
                "target_env" => &self.target.env,
                "target_family" => &self.target.family,
                _ => return Err(format!("cannot evaluate cfg `{name}` for a target")),
            };
            return Ok(fact == value);
        }
        match name.as_str() {
            "unix" | "windows" => Ok(self.target.family == name),
            _ => Err(format!("cannot evaluate cfg `{name}` for a target")),
        }
    }
}

/// What a package's enabled features do: the dependencies they activate
/// and the features they request on them.
#[derive(Default)]
struct Expansion {
    /// Its enabled features, closed under its own `[features]`.
    features: BTreeSet<String>,
    /// Optional dependencies (by key) the features activate.
    activated: BTreeSet<String>,
    /// `(dependency key, feature)` requests; `weak` ones (`x?/f`) apply only
    /// when the dependency is active anyway.
    requests: Vec<(String, String, bool)>,
}

/// The features `requested` on `package`, expanded through its
/// `[features]`; an undeclared feature is an error, as in Cargo.
fn expand(package: &Package, requested: &BTreeSet<String>) -> Result<Expansion> {
    let mut expansion = Expansion::default();
    let mut pending: Vec<String> = requested.iter().cloned().collect();
    while let Some(feature) = pending.pop() {
        if !expansion.features.insert(feature.clone()) {
            continue;
        }
        let Some(entries) = package.features.get(&feature) else {
            if package.implicit_feature(&feature) {
                expansion.activated.insert(feature);
                continue;
            }
            return Err(format!(
                "package {} has no feature `{feature}`",
                package.name
            ));
        };
        for entry in entries {
            if let Some(key) = entry.strip_prefix("dep:") {
                expansion.activated.insert(key.to_owned());
            } else if let Some((key, wanted)) = entry.split_once('/') {
                match key.strip_suffix('?') {
                    Some(key) => {
                        expansion
                            .requests
                            .push((key.to_owned(), wanted.to_owned(), true));
                    }
                    None => {
                        expansion.activated.insert(key.to_owned());
                        expansion
                            .requests
                            .push((key.to_owned(), wanted.to_owned(), false));
                    }
                }
            } else {
                pending.push(entry.clone());
            }
        }
    }
    Ok(expansion)
}

impl Model {
    /// The path packages `roots` reach through the dependencies `edges`
    /// selects, for `target` (every target table when `None`) with
    /// `features`; the roots included, ordered by directory.
    pub fn closure<'a>(
        &'a self,
        roots: &[&'a Package],
        edges: Edges,
        target: Option<&Target>,
        features: &Features,
    ) -> Result<Vec<&'a Package>> {
        let resolve = matches!(features, Features::Resolved { .. });
        let mut enabled: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for root in roots {
            let requested = enabled.entry(root.directory.clone()).or_default();
            if let Features::Resolved { default, features } = features {
                if *default && root.features.contains_key("default") {
                    requested.insert("default".to_owned());
                }
                requested.extend(features.iter().cloned());
            }
        }
        // Grow the packages and their enabled features to a fixed point.
        loop {
            let mut changed = false;
            let directories: Vec<String> = enabled.keys().cloned().collect();
            for directory in directories {
                let package = self
                    .package_at(&directory)
                    .ok_or_else(|| format!("no package at {directory}"))?;
                let expansion = if resolve {
                    expand(package, &enabled[&directory])?
                } else {
                    Expansion::default()
                };
                for dependency in &package.dependencies {
                    if !self.follows(package, dependency, edges, target)?
                        || (resolve
                            && dependency.optional
                            && !expansion.activated.contains(&dependency.key))
                    {
                        continue;
                    }
                    let Some(path) = &dependency.path else {
                        continue;
                    };
                    let reached = self.package_at(path).ok_or_else(|| {
                        format!(
                            "{}: path dependency {} names no package",
                            package.manifest, dependency.key
                        )
                    })?;
                    let mut requested: BTreeSet<String> = BTreeSet::new();
                    if resolve {
                        requested.extend(dependency.features.iter().cloned());
                        if dependency.default_features && reached.features.contains_key("default") {
                            requested.insert("default".to_owned());
                        }
                        requested.extend(
                            expansion
                                .requests
                                .iter()
                                .filter(|(key, ..)| *key == dependency.key)
                                .map(|(_, feature, _)| feature.clone()),
                        );
                    }
                    let entry = enabled.entry(path.clone()).or_insert_with(|| {
                        changed = true;
                        BTreeSet::new()
                    });
                    for feature in requested {
                        changed |= entry.insert(feature);
                    }
                }
                let own = enabled.get_mut(&directory).expect("listed above");
                for feature in expansion.features {
                    changed |= own.insert(feature);
                }
            }
            if !changed {
                break;
            }
        }
        Ok(enabled
            .keys()
            .filter_map(|directory| self.package_at(directory))
            .collect())
    }

    /// Whether a closure through `edges` for `target` follows `dependency`
    /// of `package`, whatever enables it.
    fn follows(
        &self,
        package: &Package,
        dependency: &Dependency,
        edges: Edges,
        target: Option<&Target>,
    ) -> Result<bool> {
        if !edges.follows(dependency.kind) {
            return Ok(false);
        }
        match (target, &dependency.target) {
            (Some(target), Some(key)) => target
                .selects(key)
                .map_err(|error| format!("{}: {error}", package.manifest)),
            _ => Ok(true),
        }
    }

    /// The packages of `candidates` that reach one of `selected` through
    /// path dependencies of any kind (every target, every optional one),
    /// `selected` included.
    pub fn dependents<'a>(
        &'a self,
        candidates: &[&'a Package],
        selected: &[&'a Package],
    ) -> Vec<&'a Package> {
        let mut found: BTreeSet<&str> = selected.iter().map(|p| p.directory.as_str()).collect();
        loop {
            let before = found.len();
            for package in candidates {
                if !found.contains(package.directory.as_str())
                    && package.dependencies.iter().any(|dependency| {
                        dependency
                            .path
                            .as_deref()
                            .is_some_and(|path| found.contains(path))
                    })
                {
                    found.insert(&package.directory);
                }
            }
            if found.len() == before {
                break;
            }
        }
        found
            .into_iter()
            .filter_map(|directory| self.package_at(directory))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{files::Repo, testing::tree};

    fn model(files: &[(&str, &str)]) -> (tempfile::TempDir, Model) {
        let dir = tree(files);
        let model = Model::load(&Repo::from_dir(dir.path()).unwrap()).unwrap();
        (dir, model)
    }

    fn names(packages: &[&Package]) -> Vec<String> {
        packages.iter().map(|p| p.name.clone()).collect()
    }

    const FIXTURE: &[(&str, &str)] = &[
        (
            "app/Cargo.toml",
            "[package]\nname = \"app\"\n\
             [dependencies]\nlib = { path = \"../lib\", default-features = false, features = [\"fast\"] }\n\
             net = { path = \"../net\", optional = true }\n\
             registry = \"1\"\n\
             [build-dependencies]\nbuild = { path = \"../build\" }\n\
             [dev-dependencies]\ntest = { path = \"../test\" }\n\
             [target.'cfg(target_arch = \"riscv32\")'.dependencies]\nchip = { path = \"../chip\" }\n\
             [target.'cfg(not(target_os = \"none\"))'.dependencies]\nhost = { path = \"../host\" }\n\
             [features]\ndefault = [\"net\"]\nwire = [\"net?/tcp\"]\n",
        ),
        (
            "lib/Cargo.toml",
            "[package]\nname = \"lib\"\n\
             [dependencies]\nslow = { path = \"../slow\", optional = true }\nsimd = { path = \"../simd\", optional = true }\n\
             [features]\ndefault = [\"slow\"]\nfast = [\"dep:simd\"]\n",
        ),
        (
            "net/Cargo.toml",
            "[package]\nname = \"net\"\n[dependencies]\ntcp = { path = \"../tcp\", optional = true }\n",
        ),
        ("tcp/Cargo.toml", "[package]\nname = \"tcp\"\n"),
        ("slow/Cargo.toml", "[package]\nname = \"slow\"\n"),
        ("simd/Cargo.toml", "[package]\nname = \"simd\"\n"),
        ("build/Cargo.toml", "[package]\nname = \"build\"\n"),
        ("test/Cargo.toml", "[package]\nname = \"test\"\n"),
        ("chip/Cargo.toml", "[package]\nname = \"chip\"\n"),
        ("host/Cargo.toml", "[package]\nname = \"host\"\n"),
    ];

    #[test]
    fn a_closure_follows_kinds_targets_and_features() {
        let (_dir, model) = model(FIXTURE);
        let app = model.package("app").unwrap();
        let riscv = Target::new("riscv32imafc-unknown-none-elf").unwrap();
        let resolved = |default: bool, features: &[&str]| Features::Resolved {
            default,
            features: features.iter().map(|f| (*f).to_owned()).collect(),
        };
        let build = |features: &Features, target: Option<&Target>| {
            names(
                &model
                    .closure(&[app], Edges::Build, target, features)
                    .unwrap(),
            )
        };
        assert_eq!(
            build(&resolved(true, &[]), Some(&riscv)),
            ["app", "build", "chip", "lib", "net", "simd"]
        );
        assert_eq!(
            build(&resolved(false, &["wire"]), Some(&riscv)),
            ["app", "build", "chip", "lib", "simd"]
        );
        assert_eq!(
            build(&resolved(true, &["wire"]), Some(&riscv)),
            ["app", "build", "chip", "lib", "net", "simd", "tcp"]
        );
        let linux = Target::new("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(
            build(&Features::All, Some(&linux)),
            ["app", "build", "host", "lib", "net", "simd", "slow", "tcp"]
        );
        assert_eq!(
            names(
                &model
                    .closure(&[app], Edges::All, None, &Features::All)
                    .unwrap()
            ),
            [
                "app", "build", "chip", "host", "lib", "net", "simd", "slow", "tcp", "test"
            ]
        );
        let error = model
            .closure(&[app], Edges::Build, None, &resolved(false, &["absent"]))
            .unwrap_err();
        assert!(
            error.contains("package app has no feature `absent`"),
            "{error}"
        );
    }

    #[test]
    fn dependents_reach_back_through_every_kind() {
        let (_dir, model) = model(FIXTURE);
        let all: Vec<&Package> = model.packages().iter().collect();
        let tcp = model.package("tcp").unwrap();
        assert_eq!(
            names(&model.dependents(&all, &[tcp])),
            ["app", "net", "tcp"]
        );
        let test = model.package("test").unwrap();
        assert_eq!(names(&model.dependents(&all, &[test])), ["app", "test"]);
    }

    #[test]
    fn cfg_predicates_evaluate_for_a_triple_and_fail_closed() {
        let riscv = Target::new("riscv32imafc-unknown-none-elf").unwrap();
        let linux = Target::new("x86_64-unknown-linux-gnu").unwrap();
        for (key, on_riscv, on_linux) in [
            ("cfg(target_arch = \"riscv32\")", true, false),
            ("cfg(not(target_os = \"none\"))", false, true),
            ("cfg(unix)", false, true),
            ("cfg(any(unix, target_arch = \"riscv32\"))", true, true),
            ("cfg(all(unix, target_env = \"gnu\",))", false, true),
            ("x86_64-unknown-linux-gnu", false, true),
        ] {
            assert_eq!(riscv.selects(key).unwrap(), on_riscv, "{key}");
            assert_eq!(linux.selects(key).unwrap(), on_linux, "{key}");
        }
        assert!(riscv.selects("cfg(feature = \"x\")").is_err());
        assert!(riscv.selects("cfg(target_has_atomic = \"32\")").is_err());
        assert!(riscv.selects("cfg(all(unix)").is_err());
        assert!(Target::new("riscv32").is_err());
    }
}
