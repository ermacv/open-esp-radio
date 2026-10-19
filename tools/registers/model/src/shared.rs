//! Register layouts shared by several chips.
//!
//! A register library owns peripheral layouts that are identical on several
//! chips apart from their base address. Its manifest names the library, the
//! Rust crate generated from it and its layout fragments. A layout fragment is
//! an ordinary schema-2 fragment without `baseAddress` and without reviews: a
//! chip's device manifest places every peripheral of the fragment at its own
//! base address and supplies its own review annotations, which cite that
//! chip's evidence. Both chips therefore publish one layout, and the chip's
//! generated PAC re-exports the library's register blocks instead of
//! generating its own copy.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{Error, ModelDevice, RegisterModelFragment, Result, ReviewAnnotation};

/// Schema-1 manifest of one register library.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterLibraryManifest {
    pub schema: u32,
    /// Stable library identifier.
    pub name: String,
    /// Rust path of the raw crate generated from the library.
    pub crate_name: String,
    /// Metadata of the placeholder device used to render the library blocks.
    pub device: ModelDevice,
    /// Layout fragments, relative to this manifest.
    pub fragments: Vec<String>,
}

/// One layout fragment of a library, placed by a chip's device manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct SharedFragmentReference {
    /// Library manifest, relative to the device manifest.
    pub library: String,
    /// Layout fragment, relative to the library manifest.
    pub fragment: String,
    /// The chip's review annotations of the fragment, relative to the device
    /// manifest.
    pub review: String,
    /// Base address of every peripheral of the fragment on this chip.
    pub placement: BTreeMap<String, u64>,
}

/// Schema-1 file of one chip's review annotations of a layout fragment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct ReviewOverlay {
    schema: u32,
    #[serde(default)]
    review: Vec<ReviewAnnotation>,
}

/// The library a placed peripheral comes from.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedPeripheral {
    pub library: String,
    pub crate_name: String,
}

/// A library path relative to a chip manifest: parent components first, then
/// ordinary ones.
fn validate_library_path(value: &str) -> Result<()> {
    let path = PathBuf::from(value);
    let mut parents_done = false;
    for component in path.components() {
        match component {
            Component::ParentDir if !parents_done => {}
            Component::Normal(_) => parents_done = true,
            _ => {
                return Err(Error::message(format!(
                    "register library must be a relative path of parent components followed by names: {value:?}"
                )));
            }
        }
    }
    if !parents_done {
        return Err(Error::message(format!(
            "register library path names no file: {value:?}"
        )));
    }
    Ok(())
}

fn read(path: &Path, loaded: &mut BTreeMap<PathBuf, String>) -> Result<String> {
    let input = fs::read_to_string(path)?;
    loaded.insert(path.canonicalize()?, input.clone());
    Ok(input)
}

impl RegisterLibraryManifest {
    pub fn load(path: &Path, loaded: &mut BTreeMap<PathBuf, String>) -> Result<Self> {
        let input = read(path, loaded)?;
        let manifest: Self = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span();
            Error::manifest_span("register library manifest", path, error, span)
        })?;
        if manifest.schema != 1 {
            return Err(Error::manifest(
                "register library manifest",
                path,
                "requires schema = 1",
            ));
        }
        if manifest.fragments.is_empty() {
            return Err(Error::manifest(
                "register library manifest",
                path,
                "requires at least one layout fragment",
            ));
        }
        let mut seen = BTreeSet::new();
        for fragment in &manifest.fragments {
            crate::validate_relative_fragment(fragment)
                .map_err(|error| Error::manifest("register library manifest", path, error))?;
            if !seen.insert(fragment) {
                return Err(Error::manifest(
                    "register library manifest",
                    path,
                    format!("duplicate layout fragment {fragment:?}"),
                ));
            }
        }
        crate::validate_pac_crate_name(&manifest.crate_name)
            .map_err(|error| Error::manifest("register library manifest", path, error))?;
        Ok(manifest)
    }

    /// Parse one layout fragment with every peripheral at `placement`.
    ///
    /// Layout fragments carry neither a base address nor reviews; `placement`
    /// must name exactly the fragment's peripherals.
    pub fn place(
        &self,
        library_path: &Path,
        fragment: &str,
        placement: &BTreeMap<String, u64>,
        loaded: &mut BTreeMap<PathBuf, String>,
    ) -> Result<RegisterModelFragment> {
        if !self.fragments.iter().any(|known| known == fragment) {
            return Err(Error::manifest(
                "register library manifest",
                library_path,
                format!("declares no layout fragment {fragment:?}"),
            ));
        }
        let path = library_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(fragment);
        let input = read(&path, loaded)?;
        let mut document = input.parse::<toml_edit::DocumentMut>().map_err(|error| {
            let span = error.span();
            Error::manifest_span("register layout fragment", &path, error, span)
        })?;
        if document.contains_key("review") {
            return Err(Error::manifest(
                "register layout fragment",
                &path,
                "carries reviews; each chip reviews a layout in its own review file",
            ));
        }
        let mut placed = BTreeSet::new();
        let peripherals = document
            .get_mut("peripherals")
            .and_then(toml_edit::Item::as_array_of_tables_mut)
            .ok_or_else(|| {
                Error::manifest("register layout fragment", &path, "contains no peripherals")
            })?;
        for peripheral in peripherals.iter_mut() {
            let name = peripheral
                .get("name")
                .and_then(toml_edit::Item::as_str)
                .ok_or_else(|| {
                    Error::manifest(
                        "register layout fragment",
                        &path,
                        "peripheral without a name",
                    )
                })?
                .to_owned();
            if peripheral.contains_key("baseAddress") {
                return Err(Error::manifest(
                    "register layout fragment",
                    &path,
                    format!("peripheral {name:?} has a baseAddress; chips place layouts"),
                ));
            }
            let base = placement.get(&name).ok_or_else(|| {
                Error::manifest(
                    "register layout fragment",
                    &path,
                    format!("the chip places no peripheral {name:?}"),
                )
            })?;
            let base = i64::try_from(*base).map_err(|_| {
                Error::manifest(
                    "register layout fragment",
                    &path,
                    format!("base address of {name:?} is out of range"),
                )
            })?;
            peripheral.insert("baseAddress", toml_edit::value(base));
            placed.insert(name);
        }
        if let Some(extra) = placement.keys().find(|name| !placed.contains(*name)) {
            return Err(Error::manifest(
                "register layout fragment",
                &path,
                format!("the chip places {extra:?}, which the fragment does not declare"),
            ));
        }
        toml_edit::de::from_str(&document.to_string()).map_err(|error| {
            let span = error.span();
            Error::manifest_span("register layout fragment", &path, error, span)
        })
    }
}

/// A register library: its layouts, each at a placeholder base address.
///
/// The placeholder bases only render and validate the layouts; the library
/// publishes register blocks without addresses.
pub struct RegisterLibrary {
    manifest: RegisterLibraryManifest,
    loaded_inputs: BTreeMap<PathBuf, String>,
    device: svd_rs::Device,
}

/// Spacing of placeholder bases; larger than any layout.
const PLACEHOLDER_STRIDE: u64 = 0x0010_0000;

impl RegisterLibrary {
    pub fn load(path: &Path) -> Result<Self> {
        let mut loaded_inputs = BTreeMap::new();
        let manifest = RegisterLibraryManifest::load(path, &mut loaded_inputs)?;
        let base = path.parent().unwrap_or_else(|| Path::new("."));
        let mut placed = Vec::new();
        let mut index = 0;
        for fragment in &manifest.fragments {
            let input = fs::read_to_string(base.join(fragment))?;
            let document = input.parse::<toml_edit::DocumentMut>()?;
            let names: Vec<String> = document
                .get("peripherals")
                .and_then(toml_edit::Item::as_array_of_tables)
                .into_iter()
                .flat_map(toml_edit::ArrayOfTables::iter)
                .filter_map(|peripheral| peripheral.get("name").and_then(toml_edit::Item::as_str))
                .map(str::to_owned)
                .collect();
            let placement = names
                .into_iter()
                .map(|name| {
                    index += 1;
                    (name, index * PLACEHOLDER_STRIDE)
                })
                .collect();
            let layout = manifest.place(path, fragment, &placement, &mut loaded_inputs)?;
            placed.extend(layout.peripherals);
        }
        crate::validate_peripheral_names(&placed)
            .map_err(|error| Error::manifest("register library", path, error))?;
        let device = crate::build_device(&manifest.device, placed)
            .map_err(|error| Error::manifest("register library", path, error))?;
        crate::model_validation::validate_device(&device)
            .map_err(|error| Error::manifest("register library", path, error))?;
        Ok(Self {
            manifest,
            loaded_inputs,
            device,
        })
    }

    pub fn name(&self) -> &str {
        &self.manifest.name
    }

    pub fn crate_name(&self) -> &str {
        &self.manifest.crate_name
    }

    /// Names of every peripheral layout of the library.
    pub fn peripherals(&self) -> impl Iterator<Item = &str> {
        self.device
            .peripherals
            .iter()
            .map(|peripheral| peripheral.name.as_str())
    }

    /// Exact input text parsed by this library.
    pub fn loaded_inputs(&self) -> &BTreeMap<PathBuf, String> {
        &self.loaded_inputs
    }

    /// SVD of the layouts at their placeholder bases, for rendering only.
    pub fn render_layout_svd(&self) -> Result<String> {
        let mut output = svd_encoder::encode(&self.device).map_err(|error| {
            Error::message(format!("failed to encode register library as SVD: {error}"))
        })?;
        if !output.ends_with('\n') {
            output.push('\n');
        }
        Ok(output)
    }
}

/// Load one placed layout fragment of a chip manifest at `manifest`, with the
/// chip's reviews, and the library of each of its peripherals.
pub(crate) fn load_placed_fragment(
    manifest: &Path,
    reference: &SharedFragmentReference,
    loaded: &mut BTreeMap<PathBuf, String>,
) -> Result<(RegisterModelFragment, BTreeMap<String, SharedPeripheral>)> {
    validate_library_path(&reference.library)
        .map_err(|error| Error::manifest("register model manifest", manifest, error))?;
    crate::validate_relative_fragment(&reference.review)
        .map_err(|error| Error::manifest("register model manifest", manifest, error))?;
    let base = manifest.parent().unwrap_or_else(|| Path::new("."));
    let library_path = base.join(&reference.library);
    let library = RegisterLibraryManifest::load(&library_path, loaded)?;
    let mut fragment = library.place(
        &library_path,
        &reference.fragment,
        &reference.placement,
        loaded,
    )?;
    let review_path = base.join(&reference.review);
    let input = read(&review_path, loaded)?;
    let overlay: ReviewOverlay = toml_edit::de::from_str(&input).map_err(|error| {
        let span = error.span();
        Error::manifest_span("register review file", &review_path, error, span)
    })?;
    if overlay.schema != 1 {
        return Err(Error::manifest(
            "register review file",
            &review_path,
            "requires schema = 1",
        ));
    }
    fragment.review = overlay.review;
    let members = reference
        .placement
        .keys()
        .map(|name| {
            (
                name.clone(),
                SharedPeripheral {
                    library: library.name.clone(),
                    crate_name: library.crate_name.clone(),
                },
            )
        })
        .collect();
    Ok((fragment, members))
}

/// Every file a placed layout fragment reads, for input tracking.
pub(crate) fn placed_fragment_inputs(
    manifest: &Path,
    reference: &SharedFragmentReference,
) -> Result<Vec<PathBuf>> {
    validate_library_path(&reference.library)
        .map_err(|error| Error::manifest("register model manifest", manifest, error))?;
    let base = manifest.parent().unwrap_or_else(|| Path::new("."));
    let library = base.join(&reference.library);
    let fragment = library
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(&reference.fragment);
    Ok(vec![library, fragment, base.join(&reference.review)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_paths_climb_then_descend() {
        assert!(validate_library_path("../../ieee80211/model/library.toml").is_ok());
        assert!(validate_library_path("library.toml").is_ok());
        assert!(validate_library_path("../a/../b.toml").is_err());
        assert!(validate_library_path("/abs/library.toml").is_err());
        assert!(validate_library_path("..").is_err());
    }
}
