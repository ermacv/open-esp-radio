//! Memory layouts: structures in RAM whose format the hardware defines and
//! which the CPU and the hardware hand to each other
//! (`chips/<chip>/registers/model/layouts/<name>.toml`, see
//! `docs/hardware-memory.md`).
//!
//! A layout has no address. An owner places it in a RAM region of the memory
//! map that its `placement` names. A layout is either a sequence of 32-bit
//! words the hardware may touch while the CPU holds a reference (`access =
//! "shared"`, volatile cells) or byte ranges the CPU reads only after a
//! handover (`access = "owned"`, a decoder over a fixed-size array). The
//! generated PAC carries no protocol: no ordering, fences or ownership.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use crate::{
    Error, NamingRequirement, Position, Result, ReviewAnnotation, mark_description,
    opaque_position_name, review_annotation_is_assertion, validate_naming,
    validate_relative_fragment,
};

/// The largest layout the generator accepts, in bytes.
const MAX_LAYOUT_SIZE: u32 = 4096;

/// The schema of a layout file.
const LAYOUT_SCHEMA: u32 = 1;

/// The RAM region whose addresses the PAC types: `DmaWindowAddress` for a
/// buffer, `DmaRef<L>` for a layout placed there. An address field and the
/// layout it points to name this region and no other.
pub const DMA_REGION: &str = "dma-window";

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct LayoutFile {
    schema: u32,
    layout: LayoutSource,
    #[serde(default)]
    review: Vec<ReviewAnnotation>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct LayoutSource {
    name: String,
    description: String,
    size: u32,
    align: u32,
    #[serde(default)]
    placement: Vec<String>,
    /// A layout whose geometry this one repeats: the same size, alignment,
    /// placement, words or ranges, and bits for every field both name.
    #[serde(default)]
    same_geometry_as: Option<String>,
    #[serde(default)]
    words: Vec<UnitSource>,
    #[serde(default)]
    bytes: Vec<UnitSource>,
    #[serde(default)]
    array: Option<LayoutArray>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct UnitSource {
    name: String,
    offset: u32,
    /// Bytes of a `[[layout.bytes]]` range; a word is always four.
    #[serde(default)]
    size: Option<u32>,
    access: LayoutAccess,
    #[serde(default)]
    description: Option<String>,
    fields: Vec<FieldSource>,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct FieldSource {
    name: String,
    bits: String,
    writer: LayoutWriter,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    placement: Vec<String>,
    #[serde(default)]
    points_to: Option<String>,
    #[serde(default)]
    encoding: Option<PointerEncoding>,
    #[serde(default)]
    phases: Vec<LayoutPhase>,
}

/// A fixed-size array of another shared layout.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayoutArray {
    pub element: String,
    pub count: u32,
}

/// The controller's representation of a pointer field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PointerEncoding {
    /// Nonzero 20-bit word index, reconstructed with the 0x2f SRAM prefix.
    BleCompressed,
}

/// One named handover phase and its sole writer.
#[derive(Clone, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct LayoutPhase {
    pub name: String,
    pub writer: LayoutWriter,
}

/// How the CPU reaches a word or byte range.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutAccess {
    /// The hardware may touch it while the CPU holds the structure: volatile
    /// word access only.
    Shared,
    /// The CPU reads it only after a proven handover: plain bytes.
    Owned,
}

/// Who writes a field during the structure's use.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LayoutWriter {
    Cpu,
    Hardware,
    Phased,
}

impl LayoutWriter {
    fn label(self) -> &'static str {
        match self {
            Self::Cpu => "the CPU",
            Self::Hardware => "the hardware",
            Self::Phased => "the participants in the declared phases",
        }
    }
}

/// Whether a layout is made of shared words or owned byte ranges.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutKind {
    Words,
    Bytes,
    Array,
}

/// One validated memory layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MemoryLayout {
    pub name: String,
    pub description: String,
    pub size: u32,
    pub align: u32,
    /// RAM regions of the memory map the structure must lie in.
    pub placement: Vec<String>,
    /// The layout whose geometry this one repeats.
    pub same_geometry_as: Option<String>,
    pub kind: LayoutKind,
    pub array: Option<LayoutArray>,
    /// Words or byte ranges, in offset order.
    pub units: Vec<LayoutUnit>,
    pub review: Vec<ReviewAnnotation>,
}

/// A word or a byte range of a layout.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutUnit {
    pub name: String,
    pub offset: u32,
    /// Bytes: four for a word, one, two or four for a byte range.
    pub size: u32,
    pub access: LayoutAccess,
    pub description: Option<String>,
    /// Fields in bit order.
    pub fields: Vec<LayoutField>,
}

/// A bit field of a word or byte range, its bits counted in the unit's
/// little-endian value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayoutField {
    pub name: String,
    pub low: u32,
    pub width: u32,
    pub writer: LayoutWriter,
    pub phases: Vec<LayoutPhase>,
    pub description: Option<String>,
    /// RAM regions an address this field holds must lie in.
    pub placement: Vec<String>,
    /// The layout an address this field holds points at.
    pub points_to: Option<String>,
    pub encoding: Option<PointerEncoding>,
}

impl MemoryLayout {
    /// Whether a typed reference may point at this layout: a word layout
    /// placed in the DMA region only.
    pub fn is_dma_target(&self) -> bool {
        matches!(self.kind, LayoutKind::Words | LayoutKind::Array) && self.placement == [DMA_REGION]
    }
}

impl LayoutUnit {
    /// Whether the CPU writes any field of this range.
    pub fn is_cpu_written(&self) -> bool {
        self.fields.iter().any(LayoutField::is_cpu_written)
    }
}

impl LayoutField {
    fn is_cpu_written(&self) -> bool {
        self.writer == LayoutWriter::Cpu
            || self
                .phases
                .iter()
                .any(|phase| phase.writer == LayoutWriter::Cpu)
    }
    /// Whether this field holds an address: of a buffer or of a layout.
    pub fn is_address(&self) -> bool {
        !self.placement.is_empty() || self.points_to.is_some()
    }

    fn mask(&self) -> u64 {
        ((1u64 << self.width) - 1) << self.low
    }
}

fn invalid(path: &Path, reason: impl std::fmt::Display) -> Error {
    Error::manifest("memory layout", path, reason)
}

/// Load the layout files a model manifest names, relative to `base`.
pub(crate) fn load_layouts(
    base: &Path,
    relative: &[String],
    loaded_inputs: &mut BTreeMap<PathBuf, String>,
    naming: NamingRequirement,
) -> Result<Vec<MemoryLayout>> {
    let mut seen = BTreeSet::new();
    let mut layouts = Vec::with_capacity(relative.len());
    for path in relative {
        validate_relative_fragment(path)?;
        if !seen.insert(path) {
            return Err(Error::message(format!("duplicate layout file {path:?}")));
        }
        let path = base.join(path);
        let input = fs::read_to_string(&path)?;
        loaded_inputs.insert(path.canonicalize()?, input.clone());
        let file: LayoutFile = toml_edit::de::from_str(&input).map_err(|error| {
            let span = error.span();
            Error::manifest_span("memory layout", &path, error, span)
        })?;
        layouts.push(parse_layout(&path, file, naming)?);
    }
    validate_layout_set(&layouts)?;
    Ok(layouts)
}

/// Parse and validate one layout file on its own.
fn parse_layout(path: &Path, file: LayoutFile, naming: NamingRequirement) -> Result<MemoryLayout> {
    if file.schema != LAYOUT_SCHEMA {
        return Err(invalid(path, format!("requires schema = {LAYOUT_SCHEMA}")));
    }
    let source = file.layout;
    validate_name(&source.name).map_err(|reason| invalid(path, reason))?;
    if source.description.trim().is_empty() {
        return Err(invalid(path, "a layout needs a description"));
    }
    if !source.align.is_power_of_two() || source.align > 4096 {
        return Err(invalid(path, "align must be a power of two up to 4096"));
    }
    if source.size == 0
        || source.size > MAX_LAYOUT_SIZE
        || !source.size.is_multiple_of(source.align)
    {
        return Err(invalid(
            path,
            format!("size must be a nonzero multiple of align up to {MAX_LAYOUT_SIZE} bytes"),
        ));
    }
    validate_regions(&source.placement).map_err(|reason| invalid(path, reason))?;
    let (kind, sources) = match (
        source.words.is_empty(),
        source.bytes.is_empty(),
        &source.array,
    ) {
        (false, true, None) => (LayoutKind::Words, source.words),
        (true, false, None) => (LayoutKind::Bytes, source.bytes),
        (true, true, Some(array)) => {
            validate_name(&array.element).map_err(|reason| invalid(path, reason))?;
            if array.count == 0 {
                return Err(invalid(path, "an array needs a nonzero count"));
            }
            (LayoutKind::Array, Vec::new())
        }
        _ => {
            return Err(invalid(
                path,
                "a layout declares either [[layout.words]], [[layout.bytes]] or [layout.array]",
            ));
        }
    };
    if kind == LayoutKind::Words && source.align < 4 {
        return Err(invalid(
            path,
            "a word layout is aligned to at least 4 bytes",
        ));
    }
    let mut units = Vec::with_capacity(sources.len());
    let mut unit_names = BTreeSet::new();
    for unit in sources {
        let unit = parse_unit(kind, source.size, unit).map_err(|reason| invalid(path, reason))?;
        if !unit_names.insert(unit.name.clone()) {
            return Err(invalid(path, format!("duplicate unit {}", unit.name)));
        }
        units.push(unit);
    }
    units.sort_by_key(|unit| unit.offset);
    for pair in units.windows(2) {
        if let [left, right] = pair
            && left.offset + left.size > right.offset
        {
            return Err(invalid(
                path,
                format!("{} overlaps {}", left.name, right.name),
            ));
        }
    }
    if kind == LayoutKind::Words {
        // A shared structure is a sequence of cells: every word is declared,
        // an unknown one as `WORD_<offset>_OPAQUE`.
        for (index, unit) in units.iter().enumerate() {
            if unit.offset != 4 * index as u32 {
                return Err(invalid(
                    path,
                    format!("word at offset {:#x} is not declared", 4 * index),
                ));
            }
        }
        if 4 * units.len() as u32 != source.size {
            return Err(invalid(path, "the words do not cover the layout's size"));
        }
    }
    let layout = MemoryLayout {
        name: source.name,
        description: source.description,
        size: source.size,
        align: source.align,
        placement: source.placement,
        same_geometry_as: source.same_geometry_as,
        kind,
        array: source.array,
        units,
        review: file.review,
    };
    validate_reviews(&layout, naming).map_err(|error| invalid(path, error))?;
    Ok(layout)
}

fn parse_unit(kind: LayoutKind, layout_size: u32, source: UnitSource) -> Result<LayoutUnit> {
    validate_name(&source.name)?;
    if matches!(source.name.as_str(), "NEW" | "BYTES" | "ZEROED") {
        return Err(Error::message(format!(
            "unit name {} collides with a generated method",
            source.name
        )));
    }
    let size = match (kind, source.size) {
        (LayoutKind::Words, None) => 4,
        (LayoutKind::Words, Some(_)) => {
            return Err(Error::message(format!(
                "word {} has no size; a word is four bytes",
                source.name
            )));
        }
        (LayoutKind::Bytes, Some(size @ (1 | 2 | 4))) => size,
        (LayoutKind::Bytes, _) => {
            return Err(Error::message(format!(
                "byte range {} needs size 1, 2 or 4",
                source.name
            )));
        }
        (LayoutKind::Array, _) => return Err(Error::message("an array has no units")),
    };
    let expected = match kind {
        LayoutKind::Words => LayoutAccess::Shared,
        LayoutKind::Bytes => LayoutAccess::Owned,
        LayoutKind::Array => return Err(Error::message("an array has no units")),
    };
    if source.access != expected {
        return Err(Error::message(format!(
            "{} must be accessed {}: words are shared, byte ranges owned",
            source.name,
            match expected {
                LayoutAccess::Shared => "shared",
                LayoutAccess::Owned => "owned",
            }
        )));
    }
    if kind == LayoutKind::Words && !source.offset.is_multiple_of(4) {
        return Err(Error::message(format!(
            "word {} is not at a 4-byte offset",
            source.name
        )));
    }
    if source
        .offset
        .checked_add(size)
        .is_none_or(|end| end > layout_size)
    {
        return Err(Error::message(format!(
            "{} lies outside the layout",
            source.name
        )));
    }
    if source.fields.is_empty() {
        return Err(Error::message(format!("{} has no fields", source.name)));
    }
    let mut fields = Vec::with_capacity(source.fields.len());
    let mut names = BTreeSet::new();
    let mut used = 0u64;
    for field in source.fields {
        let field = parse_field(kind, size, field)
            .map_err(|error| Error::message(format!("{}: {error}", source.name)))?;
        if !names.insert(field.name.clone()) {
            return Err(Error::message(format!(
                "{} declares field {} twice",
                source.name, field.name
            )));
        }
        if used & field.mask() != 0 {
            return Err(Error::message(format!(
                "{}.{} overlaps another field",
                source.name, field.name
            )));
        }
        used |= field.mask();
        fields.push(field);
    }
    fields.sort_by_key(|field| field.low);
    Ok(LayoutUnit {
        name: source.name,
        offset: source.offset,
        size,
        access: source.access,
        description: source.description,
        fields,
    })
}

fn parse_field(kind: LayoutKind, unit_size: u32, source: FieldSource) -> Result<LayoutField> {
    validate_name(&source.name)?;
    if matches!(source.name.as_str(), "BITS" | "FROM_BITS" | "ZERO")
        || source.name.starts_with("WITH_")
    {
        return Err(Error::message(format!(
            "field name {} collides with a generated method",
            source.name
        )));
    }
    let (high, low) = parse_bits(&source.bits).ok_or_else(|| {
        Error::message(format!(
            "{}: bits must be \"N\" or \"HIGH:LOW\"",
            source.name
        ))
    })?;
    if high < low || high >= 8 * unit_size {
        return Err(Error::message(format!(
            "{}: bits {} do not fit the {}-bit unit",
            source.name,
            source.bits,
            8 * unit_size
        )));
    }
    let width = high - low + 1;
    if source.encoding.is_some() && source.points_to.is_none() && source.placement.is_empty() {
        return Err(Error::message("encoding requires placement or points-to"));
    }
    if !source.placement.is_empty() || source.points_to.is_some() {
        let expected_width = if source.encoding.is_some() { 20 } else { 32 };
        if kind != LayoutKind::Words || width != expected_width {
            return Err(Error::message(format!(
                "{}: only a whole-word address (or 20-bit ble-compressed pointer) of a word layout has a placement or points to a layout",
                source.name
            )));
        }
        if !source.placement.is_empty() && source.points_to.is_some() {
            return Err(Error::message(format!(
                "{}: an address of a layout takes that layout's placement",
                source.name
            )));
        }
        if !source.placement.is_empty() && source.placement != [DMA_REGION] {
            return Err(Error::message(format!(
                "{}: a typed address lies in the {DMA_REGION} region only",
                source.name
            )));
        }
    }
    validate_regions(&source.placement)?;
    if let Some(layout) = &source.points_to {
        validate_name(layout)?;
    }
    if source.writer == LayoutWriter::Phased {
        let mut names = BTreeSet::new();
        if source.phases.is_empty() {
            return Err(Error::message("a phased writer needs nonempty phases"));
        }
        for phase in &source.phases {
            if phase.name.trim().is_empty() || !names.insert(&phase.name) {
                return Err(Error::message("phase names must be nonempty and unique"));
            }
            if phase.writer == LayoutWriter::Phased {
                return Err(Error::message("a phase writer must be cpu or hardware"));
            }
        }
    } else if !source.phases.is_empty() {
        return Err(Error::message("only a phased writer declares phases"));
    }
    Ok(LayoutField {
        name: source.name,
        low,
        width,
        writer: source.writer,
        phases: source.phases,
        description: source.description,
        placement: source.placement,
        points_to: source.points_to,
        encoding: source.encoding,
    })
}

fn parse_bits(bits: &str) -> Option<(u32, u32)> {
    let number = |text: &str| {
        (!text.is_empty() && text.bytes().all(|b| b.is_ascii_digit()))
            .then(|| text.parse::<u32>().ok())
            .flatten()
    };
    match bits.split_once(':') {
        Some((high, low)) => Some((number(high)?, number(low)?)),
        None => number(bits).map(|bit| (bit, bit)),
    }
}

fn validate_regions(regions: &[String]) -> Result<()> {
    let mut seen = BTreeSet::new();
    for region in regions {
        if region.is_empty()
            || !region
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !seen.insert(region)
        {
            return Err(Error::message(format!(
                "invalid or duplicate placement region {region:?}"
            )));
        }
    }
    Ok(())
}

/// Rust keywords: a name whose method would be one is rejected.
const KEYWORDS: [&str; 52] = [
    "as", "break", "const", "continue", "crate", "else", "enum", "extern", "false", "fn", "for",
    "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub", "ref", "return",
    "self", "static", "struct", "super", "trait", "true", "type", "unsafe", "use", "where",
    "while", "async", "await", "dyn", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield", "try", "gen", "union",
];

/// A layout, unit or field name: `UPPER_SNAKE`, starting with a letter.
fn validate_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
        && !name.contains("__")
        && !name.ends_with('_');
    if !valid {
        return Err(Error::message(format!(
            "{name:?} is not an UPPER_SNAKE name"
        )));
    }
    if KEYWORDS.contains(&snake(name).as_str()) {
        return Err(Error::message(format!("{name:?} is a Rust keyword")));
    }
    Ok(())
}

/// Every layout a field links to exists and is a word layout, and no name
/// is declared twice.
fn validate_layout_set(layouts: &[MemoryLayout]) -> Result<()> {
    let mut names = BTreeMap::new();
    for layout in layouts {
        if names.insert(layout.name.as_str(), layout.kind).is_some() {
            return Err(Error::message(format!(
                "duplicate memory layout {}",
                layout.name
            )));
        }
    }
    for layout in layouts {
        if let Some(array) = &layout.array {
            let element = layouts
                .iter()
                .find(|candidate| {
                    candidate.name == array.element && candidate.kind == LayoutKind::Words
                })
                .ok_or_else(|| {
                    Error::message(format!(
                        "{} array element {} is not a word layout",
                        layout.name, array.element
                    ))
                })?;
            if element.size.checked_mul(array.count) != Some(layout.size)
                || element.align != layout.align
                || element.placement != layout.placement
            {
                return Err(Error::message(format!(
                    "{} array size, alignment or placement differs from its elements",
                    layout.name
                )));
            }
        }
        if let Some(other) = &layout.same_geometry_as {
            let other = layouts
                .iter()
                .find(|candidate| &candidate.name == other && candidate.name != layout.name)
                .ok_or_else(|| {
                    Error::message(format!(
                        "{} repeats the geometry of {other}, which is not another layout of this model",
                        layout.name
                    ))
                })?;
            validate_same_geometry(layout, other)?;
        }
        for unit in &layout.units {
            for field in &unit.fields {
                let Some(target) = &field.points_to else {
                    continue;
                };
                let target_layout = layouts.iter().find(|candidate| {
                    &candidate.name == target && candidate.kind != LayoutKind::Bytes
                });
                match target_layout {
                    None => {
                        return Err(Error::message(format!(
                            "{}.{}.{} points to {target}, which is not a word layout of this model",
                            layout.name, unit.name, field.name
                        )));
                    }
                    Some(target_layout) if !target_layout.is_dma_target() => {
                        return Err(Error::message(format!(
                            "{}.{}.{} points to {target}, which is not placed in the {DMA_REGION} region only",
                            layout.name, unit.name, field.name
                        )));
                    }
                    Some(_) => {}
                }
            }
        }
    }
    Ok(())
}

/// `layout` repeats `other`'s geometry: kind, size, alignment, placement,
/// the words or ranges, and the bits of every field both units name.
fn validate_same_geometry(layout: &MemoryLayout, other: &MemoryLayout) -> Result<()> {
    let differs = |what: &str| {
        Error::message(format!(
            "{} repeats the geometry of {} but its {what} differs",
            layout.name, other.name
        ))
    };
    if layout.kind != other.kind || layout.size != other.size || layout.align != other.align {
        return Err(differs("size, alignment or kind"));
    }
    if layout.placement != other.placement {
        return Err(differs("placement"));
    }
    if layout.array != other.array {
        return Err(differs("array"));
    }
    if layout.units.len() != other.units.len() {
        return Err(differs("set of words"));
    }
    for (unit, peer) in layout.units.iter().zip(&other.units) {
        if unit.name != peer.name || unit.offset != peer.offset || unit.size != peer.size {
            return Err(differs(&format!("word {}", unit.name)));
        }
        for field in &unit.fields {
            if let Some(peer) = peer.fields.iter().find(|peer| peer.name == field.name)
                && (peer.low, peer.width) != (field.low, field.width)
            {
                return Err(differs(&format!("field {}.{}", unit.name, field.name)));
            }
        }
    }
    Ok(())
}

/// Reviews name entities of this layout; for publication the layout and
/// every unit and field carry a complete review, and every unit and field a
/// naming, by the same rules as registers and fields.
fn validate_reviews(layout: &MemoryLayout, naming: NamingRequirement) -> Result<()> {
    let mut entities = BTreeMap::new();
    entities.insert(layout.name.clone(), None);
    for unit in &layout.units {
        let path = format!("{}.{}", layout.name, unit.name);
        let position = match layout.kind {
            LayoutKind::Words => opaque_position_name(
                Position::Register {
                    offset: unit.offset,
                },
                false,
            ),
            LayoutKind::Bytes => format!("BYTE_{:04X}{}", unit.offset, crate::OPAQUE_SUFFIX),
            LayoutKind::Array => return Err(Error::message("an array has no units")),
        };
        entities.insert(path.clone(), Some(position));
        for field in &unit.fields {
            entities.insert(
                format!("{path}.{}", field.name),
                Some(opaque_position_name(
                    Position::Field {
                        offset: field.low,
                        width: field.width,
                    },
                    false,
                )),
            );
        }
    }
    let mut reviews = BTreeMap::new();
    for review in &layout.review {
        if !entities.contains_key(&review.entity) {
            return Err(Error::message(format!(
                "review entity {:?} does not exist in layout {}",
                review.entity, layout.name
            )));
        }
        if reviews.insert(review.entity.as_str(), review).is_some() {
            return Err(Error::message(format!(
                "duplicate review entity {:?}",
                review.entity
            )));
        }
        let mut sources = BTreeSet::new();
        if review
            .sources
            .iter()
            .any(|source| source.is_empty() || !sources.insert(source))
        {
            return Err(Error::message(format!(
                "review entity {:?} has empty or duplicate sources",
                review.entity
            )));
        }
        let classification = usize::from(review.provenance.is_some())
            + usize::from(review.accuracy.is_some())
            + usize::from(review.completeness.is_some());
        if classification != 0 && classification != 3 {
            return Err(Error::message(format!(
                "review entity {:?} must define provenance, accuracy and completeness together",
                review.entity
            )));
        }
    }
    for (entity, position) in &entities {
        let review = reviews.get(entity.as_str()).copied();
        match position {
            None => {
                if review.is_some_and(|review| review.naming.is_some()) {
                    return Err(Error::message(format!(
                        "review entity {entity:?} is a layout; naming applies to its words, byte ranges and fields"
                    )));
                }
            }
            Some(position) => {
                if review.is_some_and(|review| review.naming.is_some())
                    || naming == NamingRequirement::Complete
                {
                    validate_naming(entity, review, position)?;
                }
            }
        }
        if naming == NamingRequirement::Complete
            && !review.is_some_and(review_annotation_is_assertion)
        {
            return Err(Error::message(format!(
                "{entity} has no complete review: sources, provenance (not a hint), accuracy and completeness"
            )));
        }
    }
    Ok(())
}

/// `UPPER_SNAKE` to `snake_case`.
fn snake(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// `UPPER_SNAKE` to `UpperCamel`.
pub(crate) fn camel(name: &str) -> String {
    name.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_ascii_uppercase().to_string() + &chars.as_str().to_ascii_lowercase()
                }
                None => String::new(),
            }
        })
        .collect()
}

/// A region name as a constant: `dma-window` to `DMA_WINDOW`.
fn region_constant(region: &str) -> String {
    region.replace('-', "_").to_ascii_uppercase()
}

/// The unsigned type holding `bits` bits.
fn storage(bits: u32) -> &'static str {
    match bits {
        0..=8 => "u8",
        9..=16 => "u16",
        _ => "u32",
    }
}

fn push_doc(output: &mut String, text: &str, indent: &str) {
    for line in text.trim().lines() {
        let line = line.trim();
        if line.is_empty() {
            let _ = writeln!(output, "{indent}///");
        } else {
            let _ = writeln!(output, "{indent}/// {line}");
        }
    }
}

/// A unit or field's published description, with its name origin.
fn described(review: &[ReviewAnnotation], entity: &str, description: Option<&str>) -> String {
    let mut text = description.map(str::to_owned);
    let naming = review
        .iter()
        .find(|review| review.entity == entity)
        .and_then(|review| review.naming);
    mark_description(&mut text, naming.as_ref());
    text.unwrap_or_default()
}

fn sources_line(review: &[ReviewAnnotation], entity: &str) -> Option<String> {
    let review = review.iter().find(|review| review.entity == entity)?;
    (!review.sources.is_empty()).then(|| format!("SOURCE: {}.", review.sources.join(", ")))
}

/// Render the layouts as one `layouts` module of the PAC API: the RAM
/// regions they name, a type per layout, a cell and a value per shared word,
/// a value per owned byte range. `regions` gives each named RAM region's
/// `start..end`.
pub(crate) fn render(
    layouts: &[MemoryLayout],
    regions: &BTreeMap<String, (u64, u64)>,
) -> Result<String> {
    let mut output = String::new();
    if layouts.is_empty() {
        return Ok(output);
    }
    output.push_str(
        "/// Memory layouts: structures in RAM whose format the hardware defines,\n\
         /// generated from the register model's `layouts/`. They carry no protocol:\n\
         /// no ordering, fences or ownership.\n\
         pub mod layouts {\n",
    );
    let mut named = BTreeSet::new();
    for layout in layouts {
        named.extend(layout.placement.iter());
        for unit in &layout.units {
            for field in &unit.fields {
                named.extend(field.placement.iter());
            }
        }
    }
    for region in named {
        let (start, end) = regions.get(region).ok_or_else(|| {
            Error::message(format!("placement region {region} is not a RAM region"))
        })?;
        if *end > 1 << 32 {
            return Err(Error::message(format!(
                "placement region {region} does not fit a 32-bit address"
            )));
        }
        let _ = writeln!(
            output,
            "/// The `{region}` RAM region of the memory map.\n\
             pub const {}: core::ops::Range<u32> = 0x{start:08x}..0x{end:08x};\n",
            region_constant(region)
        );
    }
    let addressed = layouts
        .iter()
        .flat_map(|layout| &layout.units)
        .flat_map(|unit| &unit.fields)
        .any(LayoutField::is_address);
    if addressed {
        render_addresses(&mut output, layouts);
    }
    for layout in layouts {
        match layout.kind {
            LayoutKind::Words => render_words(&mut output, layout),
            LayoutKind::Bytes => render_bytes(&mut output, layout),
            LayoutKind::Array => render_array(&mut output, layout),
        }
    }
    output.push_str("}\n");
    Ok(output)
}

/// The typed addresses of the DMA region: a buffer address and a reference
/// to a layout placed there. Their constructors are `unsafe`: the caller
/// promises an address inside a static the HAL placed in the region.
fn render_addresses(output: &mut String, layouts: &[MemoryLayout]) {
    let region = region_constant(DMA_REGION);
    let _ = writeln!(
        output,
        "/// An address inside [`{region}`] whose memory stays reserved for the\n\
         /// hardware while it may read the address, as the hardware reads it.\n\
         #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n\
         pub struct DmaWindowAddress(u32);\n\n\
         impl DmaWindowAddress {{\n\
         /// The address `address`.\n\
         ///\n\
         /// # Safety\n\
         ///\n\
         /// `address` lies inside [`{region}`], in a static placed there or a\n\
         /// buffer lent to the hardware, which stays reserved for as long as\n\
         /// the hardware may read the address.\n\
         #[allow(unsafe_code, reason = \"the caller promises a placed address\")]\n\
         pub const unsafe fn new_unchecked(address: u32) -> Self {{\nSelf(address)\n}}\n\n\
         /// The address as the hardware reads it.\n\
         pub const fn get(self) -> u32 {{\nself.0\n}}\n\
         }}\n\n\
         mod sealed {{\npub trait Sealed {{}}\n}}\n\n\
         /// A layout placed in [`{region}`] only, which a [`DmaRef`] may point at.\n\
         pub trait DmaLayout: sealed::Sealed {{}}\n\n\
         /// The address of an `L` inside [`{region}`].\n\
         pub struct DmaRef<L: DmaLayout> {{\n\
         address: DmaWindowAddress,\n\
         layout: core::marker::PhantomData<fn() -> L>,\n\
         }}\n\n\
         impl<L: DmaLayout> DmaRef<L> {{\n\
         /// The reference to the `L` at `address`.\n\
         ///\n\
         /// # Safety\n\
         ///\n\
         /// An `L`, aligned as `L` requires, lies at `address` for as long as\n\
         /// the hardware may follow the reference.\n\
         #[allow(unsafe_code, reason = \"the caller promises a placed layout\")]\n\
         pub const unsafe fn new_unchecked(address: DmaWindowAddress) -> Self {{\n\
         Self {{ address, layout: core::marker::PhantomData }}\n\
         }}\n\n\
         /// The referenced address.\n\
         pub const fn address(self) -> DmaWindowAddress {{\nself.address\n}}\n\n\
         /// The address as the hardware reads it.\n\
         pub const fn bits(self) -> u32 {{\nself.address.0\n}}\n\
         }}\n\n\
         impl<L: DmaLayout> Clone for DmaRef<L> {{\nfn clone(&self) -> Self {{\n*self\n}}\n}}\n\n\
         impl<L: DmaLayout> Copy for DmaRef<L> {{}}\n\n\
         impl<L: DmaLayout> PartialEq for DmaRef<L> {{\nfn eq(&self, other: &Self) -> bool {{\nself.address == other.address\n}}\n}}\n\n\
         impl<L: DmaLayout> Eq for DmaRef<L> {{}}\n\n\
         impl<L: DmaLayout> core::fmt::Debug for DmaRef<L> {{\n\
         fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {{\n\
         formatter.debug_tuple(\"DmaRef\").field(&self.address).finish()\n\
         }}\n\
         }}\n"
    );
    for layout in layouts.iter().filter(|layout| layout.is_dma_target()) {
        let ty = camel(&layout.name);
        let _ = writeln!(
            output,
            "impl sealed::Sealed for {ty} {{}}\nimpl DmaLayout for {ty} {{}}\n"
        );
    }
    if layouts
        .iter()
        .flat_map(|layout| &layout.units)
        .flat_map(|unit| &unit.fields)
        .any(|field| field.encoding == Some(PointerEncoding::BleCompressed))
    {
        output.push_str(
            "impl DmaWindowAddress {\n\
            /// The nonzero BLE word index, if the address is encodable.\n\
            pub const fn ble_compressed(self) -> Option<u32> {\n\
            let address = self.get();\n\
            let index = (address >> 2) & 0x000f_ffff;\n\
            if address & 0xffc0_0003 != 0x2f00_0000 || index == 0 { None } else { Some(index) }\n\
            }\n}\n\
            impl<L: DmaLayout> DmaRef<L> {\n\
            /// The nonzero BLE word index, if the address is encodable.\n\
            pub const fn ble_compressed(self) -> Option<u32> {\n\
            self.address().ble_compressed()\n\
            }\n}\n",
        );
    }
}

fn render_array(output: &mut String, layout: &MemoryLayout) {
    let Some(array) = &layout.array else { return };
    let ty = camel(&layout.name);
    let element = camel(&array.element);
    let count = array.count;
    push_doc(output, &layout.description, "");
    if let Some(line) = sources_line(&layout.review, &layout.name) {
        let _ = writeln!(output, "/// {line}");
    }
    let _ = writeln!(
        output,
        "#[repr(C, align({}))]\npub struct {ty} {{ entries: [{element}; {count}] }}\n\
        const _: () = {{ assert!(core::mem::size_of::<{ty}>() == {});\n\
        assert!(core::mem::align_of::<{ty}>() == {}); }};\n\
        #[allow(unsafe_code, reason = \"an array of zeroable layouts is zeroable\")]\n\
        // SAFETY: each element is a zeroable generated layout and there is no extra state.\n\
        unsafe impl bytemuck::Zeroable for {ty} {{}}\n\
        impl {ty} {{\n\
        pub const SIZE: usize = {};\n\
        pub const ALIGN: usize = {};\n\
        pub const PLACEMENT: &'static [core::ops::Range<u32>] = &[{}];\n\
        pub const fn zeroed() -> Self {{ Self {{ entries: [const {{ {element}::zeroed() }}; {count}] }} }}\n\
        pub fn entry(&self, index: usize) -> Option<&{element}> {{ self.entries.get(index) }}\n\
        }}\n",
        layout.align,
        layout.size,
        layout.align,
        layout.size,
        layout.align,
        placement_list(&layout.placement)
    );
}

fn placement_list(regions: &[String]) -> String {
    regions
        .iter()
        .map(|region| region_constant(region))
        .collect::<Vec<_>>()
        .join(", ")
}

fn render_words(output: &mut String, layout: &MemoryLayout) {
    let ty = camel(&layout.name);
    push_doc(output, &layout.description, "");
    let _ = writeln!(
        output,
        "///\n/// {} bytes, aligned to {}; its zero bytes are an all-zero structure.",
        layout.size, layout.align
    );
    if let Some(line) = sources_line(&layout.review, &layout.name) {
        let _ = writeln!(output, "/// {line}");
    }
    let _ = writeln!(
        output,
        "#[repr(C, align({}))]\npub struct {ty} {{",
        layout.align
    );
    for unit in &layout.units {
        let _ = writeln!(
            output,
            "    {}: {ty}{}Cell,",
            snake(&unit.name),
            camel(&unit.name)
        );
    }
    let _ = writeln!(
        output,
        "}}\n\n\
         const _: () = {{\n\
         assert!(core::mem::size_of::<{ty}>() == {});\n\
         assert!(core::mem::align_of::<{ty}>() == {});\n\
         }};\n",
        layout.size, layout.align
    );
    let _ = writeln!(
        output,
        "#[allow(unsafe_code, reason = \"a layout of volatile words is valid as zero bytes\")]\n\
         // SAFETY: every field is a `VolatileCell<u32>`, a transparent\n\
         // `UnsafeCell<u32>` wrapper, so the all-zero bit pattern is a valid value.\n\
         unsafe impl bytemuck::Zeroable for {ty} {{}}\n"
    );
    let _ = writeln!(
        output,
        "impl {ty} {{\n\
         /// The structure's size in bytes.\n\
         pub const SIZE: usize = {};\n\
         /// The alignment the hardware requires, in bytes.\n\
         pub const ALIGN: usize = {};\n\
         /// The RAM regions the structure must lie in.\n\
         pub const PLACEMENT: &'static [core::ops::Range<u32>] = &[{}];\n\n\
         /// The all-zero structure.\n\
         pub const fn zeroed() -> Self {{\n\
         Self {{",
        layout.size,
        layout.align,
        placement_list(&layout.placement)
    );
    for unit in &layout.units {
        let _ = writeln!(
            output,
            "{}: {ty}{}Cell(vcell::VolatileCell::new(0)),",
            snake(&unit.name),
            camel(&unit.name)
        );
    }
    output.push_str("}\n}\n");
    for unit in &layout.units {
        let value = format!("{ty}{}", camel(&unit.name));
        let _ = writeln!(output);
        push_doc(
            output,
            &format!("The `{}` word at offset {:#x}.", unit.name, unit.offset),
            "    ",
        );
        let _ = writeln!(
            output,
            "    pub const fn {}(&self) -> &{value}Cell {{\n&self.{}\n}}",
            snake(&unit.name),
            snake(&unit.name)
        );
    }
    output.push_str("}\n\n");
    for unit in &layout.units {
        render_word(output, layout, unit, &ty);
    }
}

fn render_word(output: &mut String, layout: &MemoryLayout, unit: &LayoutUnit, ty: &str) {
    let entity = format!("{}.{}", layout.name, unit.name);
    let value = format!("{ty}{}", camel(&unit.name));
    let description = described(&layout.review, &entity, unit.description.as_deref());
    let _ = writeln!(
        output,
        "/// The `{}.{}` word as the hardware may change it: volatile access only.\n\
         pub struct {value}Cell(vcell::VolatileCell<u32>);\n\n\
         impl {value}Cell {{\n\
         /// One volatile read of the word.\n\
         #[inline]\n\
         pub fn read(&self) -> {value} {{\n{value}(self.0.get())\n}}\n\n\
         /// One volatile write of the whole word.\n\
         #[inline]\n\
         pub fn write(&self, value: {value}) {{\nself.0.set(value.0);\n}}\n\
         }}\n",
        layout.name, unit.name
    );
    if !description.is_empty() {
        push_doc(output, &description, "");
        output.push_str("///\n");
    }
    let _ = writeln!(
        output,
        "/// A value of the `{}.{}` word.",
        layout.name, unit.name
    );
    if let Some(line) = sources_line(&layout.review, &entity) {
        let _ = writeln!(output, "/// {line}");
    }
    let _ = writeln!(
        output,
        "#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]\n\
         pub struct {value}(u32);\n\n\
         impl {value} {{\n\
         /// The all-zero word.\n\
         pub const ZERO: Self = Self(0);\n\n\
         /// The value of a word image; every bit pattern is one.\n\
         pub const fn from_bits(bits: u32) -> Self {{ Self(bits) }}\n\n\
         /// The word image.\n\
         pub const fn bits(self) -> u32 {{ self.0 }}\n"
    );
    for field in &unit.fields {
        render_field(output, layout, unit, field, "u32", true);
    }
    output.push_str("}\n\n");
}

fn render_field(
    output: &mut String,
    layout: &MemoryLayout,
    unit: &LayoutUnit,
    field: &LayoutField,
    unit_type: &str,
    builders: bool,
) {
    let entity = format!("{}.{}.{}", layout.name, unit.name, field.name);
    let name = snake(&field.name);
    let unit_bits = 8 * unit.size;
    let bits = if field.width == 1 {
        format!("bit {}", field.low)
    } else {
        format!("bits {}:{}", field.low + field.width - 1, field.low)
    };
    let mut doc = described(&layout.review, &entity, field.description.as_deref());
    if !doc.is_empty() {
        doc.push('\n');
    }
    let _ = write!(
        doc,
        "\n`{}`, {bits}; written by {}.",
        field.name,
        field.writer.label()
    );
    for phase in &field.phases {
        let _ = write!(doc, " Phase `{}`: {}.", phase.name, phase.writer.label());
    }
    if !field.placement.is_empty() {
        let _ = write!(
            doc,
            " The address lies in {}.",
            field
                .placement
                .iter()
                .map(|region| format!("[`{}`]", region_constant(region)))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if let Some(target) = &field.points_to {
        let _ = write!(doc, " The address of a [`{}`], or zero.", camel(target));
    }
    if let Some(line) = sources_line(&layout.review, &entity) {
        let _ = write!(doc, "\n{line}");
    }
    let mask = (1u64 << field.width) - 1;
    let shifted = mask << field.low;
    let keep = (!shifted) & ((1u64 << unit_bits) - 1);
    push_doc(output, &doc, "    ");
    if field.width == 1 {
        let _ = writeln!(
            output,
            "    pub const fn {name}(self) -> bool {{ self.0 & 0x{shifted:x} != 0 }}\n"
        );
        if builders {
            let _ = writeln!(
                output,
                "    /// This value with `{}` set to `value`.\n\
                 pub const fn with_{name}(self, value: bool) -> Self {{\n\
                 if value {{ Self(self.0 | 0x{shifted:x}) }} else {{ Self(self.0 & 0x{keep:x}) }}\n\
                 }}\n",
                field.name
            );
        }
        return;
    }
    if field.is_address() {
        if field.encoding == Some(PointerEncoding::BleCompressed) {
            let low = field.low;
            let _ = writeln!(
                output,
                "    pub const fn {name}(self) -> u32 {{ (self.0 >> {low}) & 0x{mask:x} }}\n\
                /// The decoded address, without granting a reference to the memory.\n\
                pub const fn {name}_address(self) -> Option<u32> {{\n\
                let index = self.{name}();\n\
                if index == 0 {{ None }} else {{ Some(0x2f00_0000 | (index << 2)) }}\n\
                }}\n"
            );
            if builders {
                let (parameter, index) = match &field.points_to {
                    Some(target) => (
                        format!("Option<DmaRef<{}>>", camel(target)),
                        "match value { None => 0, Some(reference) => match reference.ble_compressed() { Some(index) => index, None => return None } }",
                    ),
                    None => (
                        "DmaWindowAddress".to_owned(),
                        "match value.ble_compressed() { Some(index) => index, None => return None }",
                    ),
                };
                let _ = writeln!(
                    output,
                    "    /// Set a BLE pointer, preserving the other fields; reject an unencodable address.\n\
                    pub const fn with_{name}(self, value: {parameter}) -> Option<Self> {{\n\
                    let index = {index};\n\
                    Some(Self((self.0 & 0x{keep:x}) | (index << {low})))\n\
                    }}\n"
                );
                if field.points_to.is_none() {
                    let _ = writeln!(output,
                        "    /// Clear the buffer pointer, preserving the other fields.\n\
                        pub const fn without_{name}(self) -> Self {{ Self(self.0 & 0x{keep:x}) }}\n");
                }
            }
            return;
        }
        let _ = writeln!(
            output,
            "    pub const fn {name}(self) -> u32 {{ self.0 }}\n"
        );
        if !builders {
            return;
        }
        let (parameter, image) = match &field.points_to {
            Some(target) => (
                format!("Option<DmaRef<{}>>", camel(target)),
                "match value { Some(reference) => reference.bits(), None => 0 }",
            ),
            None => ("DmaWindowAddress".to_owned(), "value.get()"),
        };
        let _ = writeln!(
            output,
            "    /// This value with `{}` set to `value`.\n\
             pub const fn with_{name}(self, value: {parameter}) -> Self {{ Self({image}) }}\n",
            field.name
        );
        return;
    }
    let field_type = storage(field.width);
    let field_type_bits = match field_type {
        "u8" => 8,
        "u16" => 16,
        _ => 32,
    };
    let read = if field.low == 0 && field.width == unit_bits {
        "self.0".to_owned()
    } else if field.low == 0 {
        format!("self.0 & 0x{mask:x}")
    } else if field.low + field.width == unit_bits {
        format!("self.0 >> {}", field.low)
    } else {
        format!("(self.0 >> {}) & 0x{mask:x}", field.low)
    };
    let read = if field_type == unit_type {
        read
    } else {
        format!("({read}) as {field_type}")
    };
    let _ = writeln!(
        output,
        "    pub const fn {name}(self) -> {field_type} {{ {read} }}\n"
    );
    if !builders {
        return;
    }
    let widened = if field_type == unit_type {
        "value".to_owned()
    } else {
        format!("value as {unit_type}")
    };
    let placed = if field.low == 0 {
        widened
    } else {
        format!("({widened}) << {}", field.low)
    };
    if field.width == field_type_bits {
        let combined = if field.width == unit_bits {
            "Self(value)".to_owned()
        } else {
            format!("Self((self.0 & 0x{keep:x}) | {placed})")
        };
        let _ = writeln!(
            output,
            "    /// This value with `{}` set to `value`.\n\
             pub const fn with_{name}(self, value: {field_type}) -> Self {{ {combined} }}\n",
            field.name
        );
    } else {
        let _ = writeln!(
            output,
            "    /// This value with `{}` set to `value`, or `None` when `value` does not\n\
             /// fit its {} bits.\n\
             pub const fn with_{name}(self, value: {field_type}) -> Option<Self> {{\n\
             if value > 0x{mask:x} {{ None }} else {{ Some(Self((self.0 & 0x{keep:x}) | {placed})) }}\n\
             }}\n",
            field.name, field.width
        );
    }
}

fn render_bytes(output: &mut String, layout: &MemoryLayout) {
    let ty = camel(&layout.name);
    push_doc(output, &layout.description, "");
    let _ = writeln!(
        output,
        "///\n/// A decoder over the {} bytes the hardware wrote, read only after the\n\
         /// handover that gives them to the CPU.",
        layout.size
    );
    if let Some(line) = sources_line(&layout.review, &layout.name) {
        let _ = writeln!(output, "/// {line}");
    }
    let _ = writeln!(
        output,
        "#[derive(Clone, Copy, Debug, Eq, PartialEq)]\n\
         pub struct {ty}<'a> {{\nbytes: &'a [u8; {size}],\n}}\n\n\
         impl<'a> {ty}<'a> {{\n\
         /// The structure's size in bytes.\n\
         pub const SIZE: usize = {size};\n\n\
         /// Decode the structure from its bytes.\n\
         pub const fn new(bytes: &'a [u8; {size}]) -> Self {{ Self {{ bytes }} }}\n\n\
         /// The bytes this decoder reads.\n\
         pub const fn bytes(self) -> &'a [u8; {size}] {{ self.bytes }}",
        size = layout.size
    );
    for unit in &layout.units {
        let value = format!("{ty}{}", camel(&unit.name));
        let unit_type = storage(8 * unit.size);
        let bytes = (unit.offset..unit.offset + unit.size)
            .map(|index| format!("self.bytes[{index}]"))
            .collect::<Vec<_>>()
            .join(", ");
        let range = if unit.size == 1 {
            format!("byte {:#x}", unit.offset)
        } else {
            format!(
                "bytes {:#x}..{:#x}, little-endian",
                unit.offset,
                unit.offset + unit.size
            )
        };
        let _ = writeln!(
            output,
            "\n    /// The `{}` range: {range}.\n\
             pub const fn {}(self) -> {value} {{ {value}({unit_type}::from_le_bytes([{bytes}])) }}",
            unit.name,
            snake(&unit.name)
        );
    }
    output.push_str("}\n\n");
    for unit in &layout.units {
        let entity = format!("{}.{}", layout.name, unit.name);
        let value = format!("{ty}{}", camel(&unit.name));
        let unit_type = storage(8 * unit.size);
        let description = described(&layout.review, &entity, unit.description.as_deref());
        if !description.is_empty() {
            push_doc(output, &description, "");
            output.push_str("///\n");
        }
        let _ = writeln!(output, "/// The `{}.{}` range.", layout.name, unit.name);
        if let Some(line) = sources_line(&layout.review, &entity) {
            let _ = writeln!(output, "/// {line}");
        }
        let _ = writeln!(
            output,
            "#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]\n\
             pub struct {value}({unit_type});\n\n\
             impl {value} {{\n\
             /// The range's little-endian value.\n\
             pub const fn bits(self) -> {unit_type} {{ self.0 }}\n"
        );
        if unit.is_cpu_written() {
            let _ = writeln!(
                output,
                "/// The value of a range image; every bit pattern is one.\n\
                 pub const fn from_bits(bits: {unit_type}) -> Self {{ Self(bits) }}\n"
            );
        }
        for field in &unit.fields {
            render_field(
                output,
                layout,
                unit,
                field,
                unit_type,
                field.is_cpu_written(),
            );
        }
        output.push_str("}\n\n");
    }
    if layout.units.iter().any(LayoutUnit::is_cpu_written) {
        render_bytes_mut(output, layout);
    }
}

/// The CPU-side view of a byte layout with CPU-written fields: typed getters
/// for every range and typed setters for the ranges the CPU writes.
fn render_bytes_mut(output: &mut String, layout: &MemoryLayout) {
    let ty = camel(&layout.name);
    let _ = writeln!(
        output,
        "/// A writer over the {size} bytes the CPU fills in: typed ranges, no raw\n\
         /// byte access outside this generated code.\n\
         #[derive(Debug, Eq, PartialEq)]\n\
         pub struct {ty}Mut<'a> {{\nbytes: &'a mut [u8; {size}],\n}}\n\n\
         impl<'a> {ty}Mut<'a> {{\n\
         /// Write through the structure's bytes.\n\
         pub const fn new(bytes: &'a mut [u8; {size}]) -> Self {{ Self {{ bytes }} }}\n\n\
         /// A read-only decoder over the bytes written so far.\n\
         pub const fn view(&self) -> {ty}<'_> {{ {ty}::new(self.bytes) }}",
        size = layout.size
    );
    for unit in layout.units.iter().filter(|unit| unit.is_cpu_written()) {
        let value = format!("{ty}{}", camel(&unit.name));
        let unit_type = storage(8 * unit.size);
        let name = snake(&unit.name);
        let stores = (0..unit.size)
            .map(|index| format!("self.bytes[{}] = bytes[{index}];", unit.offset + index))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            output,
            "\n    /// Store the `{}` range, little-endian.\n\
             pub fn set_{name}(&mut self, value: {value}) {{\n\
             let bytes = value.bits().to_le_bytes();\n{stores}\n}}\n\n\
             /// The `{}` range as last stored.\n\
             pub const fn {name}(&self) -> {value} {{ self.view().{name}() }}",
            unit.name, unit.name
        );
        let _ = unit_type;
    }
    output.push_str("}\n\n");
}

#[cfg(test)]
mod tests;
