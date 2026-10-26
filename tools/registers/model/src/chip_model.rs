//! A chip as one root file and the block type files it includes.
//!
//! A block type is one hardware block without an address: its registers
//! have offsets from the block's base. The root places instances of the
//! types at absolute addresses and holds the chip's interrupt sources, its
//! analog-I2C space, its RAM regions and the placement of memory layouts.
//! Loading checks every rule that needs no generator and expands every
//! instance into registers and fields with absolute addresses and
//! `INSTANCE.REGISTER.FIELD` identities. The format is specified in the
//! crate README ("Block types and the chip root").

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{MemoryLayout, RegisterEvidenceSet, ReviewAnnotation, analog::AnalogAddress};

mod block_type;
mod controls;
mod error;
mod facts;
mod source;
mod topology;

pub(crate) use block_type::parse_bits;
pub use block_type::{BlockType, Element};
use block_type::{Context, ExpansionBudget, check_name};
pub use controls::{ControlKind, ExpandedControl, ExpandedGateGroup, ResolvedField};
pub use error::{Error, ErrorKind};
pub use source::{
    AnalogInstance, AnalogSpace, ClockSource, Command, Control, Dim, Extent, Field, FieldClass,
    GateGroup, Group, Instance, Interrupt, InterruptOutput, InterruptPair, InterruptSources,
    LayoutPlacement, Mux, OpaqueReview, Pointer, PointerEncoding, ROOT_SCHEMA, RamRegion, ReadSafe,
    Register, Reserved, Reset, ReviewOverride, RootFile, SharedControl, Sourced, Space,
    TYPE_SCHEMA, TypeFile, TypeHeader, TypeReview, UnboundInterrupt, Value, ValueRange, View,
    WriteRange,
};

/// Where an expanded register is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Location {
    /// A CPU address.
    Mmio { address: u64 },
    /// An analog-I2C byte: the block's code and the byte's index.
    AnalogI2c(AnalogAddress),
}

/// One field of an expanded register.
#[derive(Clone, Debug)]
pub struct ExpandedField {
    /// `INSTANCE.REGISTER.FIELD`.
    pub identity: String,
    pub name: String,
    pub low: u32,
    pub width: u32,
    pub class: FieldClass,
    pub review: ReviewAnnotation,
}

/// One register element of one instance.
#[derive(Clone, Debug)]
pub struct ExpandedRegister {
    /// `INSTANCE.REGISTER`, `INSTANCE.GROUP<i>.REGISTER` or the `dim`
    /// element's name.
    pub identity: String,
    pub instance: String,
    /// The type's template key (`REGISTER`, `GROUP.REGISTER`).
    pub key: String,
    pub location: Location,
    pub width: u32,
    pub reset: Option<Reset>,
    /// Reserved bits, zero when none.
    pub reserved: u32,
    pub fields: Vec<ExpandedField>,
    pub review: ReviewAnnotation,
}

/// A loaded, checked and expanded chip.
#[derive(Clone, Debug)]
pub struct ChipModel {
    path: PathBuf,
    root: RootFile,
    pub hart_geometry: Option<topology::Geometry>,
    types: BTreeMap<String, BlockType>,
    layouts: Vec<MemoryLayout>,
    registers: Vec<ExpandedRegister>,
    controls: Vec<ExpandedControl>,
    gate_groups: Vec<ExpandedGateGroup>,
}

impl ChipModel {
    /// Load the root at `path`, the types and layouts it includes (relative
    /// to its directory), check every rule and expand every instance. Every
    /// `sources` entry must be an ID of `evidence`.
    pub fn load(path: &Path, evidence: &RegisterEvidenceSet) -> Result<Self, Error> {
        let ids = evidence
            .source_ids()
            .into_iter()
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        let text = std::fs::read_to_string(path)
            .map_err(|error| Error::new(path, "", ErrorKind::Read(error)))?;
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "kebab-case")]
        struct HartSource {
            hart_facts: Option<String>,
        }
        let source: HartSource = toml_edit::de::from_str(&text)
            .map_err(|e| Error::new(path, "", ErrorKind::Parse(e.to_string())))?;
        let base = path.parent().unwrap_or(Path::new("."));
        let harts = topology::count(base, source.hart_facts.as_deref())?;
        let resolved = topology::resolve(&text, harts, path)?;
        let mut root: RootFile = toml_edit::de::from_str(&resolved)
            .map_err(|error| Error::new(path, "", ErrorKind::Parse(error.to_string())))?;
        let context = Context {
            path,
            evidence: &ids,
        };
        let chip = root.chip.clone();
        if root.schema != ROOT_SCHEMA {
            return Err(context.error(
                &chip,
                ErrorKind::Schema {
                    expected: ROOT_SCHEMA,
                    found: root.schema,
                },
            ));
        }
        if chip.is_empty()
            || !chip
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
        {
            return Err(context.error(
                &chip,
                ErrorKind::InvalidName(format!("chip {chip:?} is not lowercase alphanumeric")),
            ));
        }
        let base = path.parent().unwrap_or(Path::new("."));

        topology::instances(&mut root, path)?;
        let types = load_types(context, base, &root.types, harts)?;
        let mut layouts = load_layouts(context, base, &root.layouts)?;
        let regions = check_ram_regions(context, &root.ram_regions)?;
        let mut names = BTreeSet::new();
        check_instances(context, &root, &types, &mut names)?;
        check_interrupts(context, &root, &types)?;
        check_analog(context, &root, &types, &mut names)?;
        place_layouts(context, &root, &regions, &mut layouts)?;
        crate::layouts::validate_layout_set(&layouts)
            .map_err(|error| context.error("", ErrorKind::Layouts(Box::new(error))))?;
        facts::check(context, &types, &layouts, &root.ram_regions)?;
        let mut budget = ExpansionBudget::default();
        let registers = expand(context, &root, &types, &mut budget)?;
        let controls = controls::expand(context, &root, &registers, &mut budget)?;
        Ok(Self {
            path: path.to_owned(),
            root,
            hart_geometry: harts,
            types,
            layouts,
            registers,
            controls: controls.links,
            gate_groups: controls.groups,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn chip(&self) -> &str {
        &self.root.chip
    }

    /// The types by name.
    pub fn types(&self) -> &BTreeMap<String, BlockType> {
        &self.types
    }

    pub fn instances(&self) -> &[Instance] {
        &self.root.instances
    }

    pub fn interrupts(&self) -> &[Interrupt] {
        &self.root.interrupts
    }

    pub fn interrupt_sources(&self) -> &InterruptSources {
        &self.root.interrupt_sources
    }

    pub fn unbound_interrupts(&self) -> &[UnboundInterrupt] {
        &self.root.unbound_interrupts
    }

    /// Fixed instance controls, with absolute field identities and addresses.
    pub fn controls(&self) -> &[ExpandedControl] {
        &self.controls
    }

    pub fn shared_controls(&self) -> &[SharedControl] {
        &self.root.shared_controls
    }

    pub fn gate_groups(&self) -> &[ExpandedGateGroup] {
        &self.gate_groups
    }

    pub fn clock_sources(&self) -> &[ClockSource] {
        &self.root.clock_sources
    }

    pub fn analog_instances(&self) -> &[AnalogInstance] {
        &self.root.analog_i2c.instances
    }

    pub fn ram_regions(&self) -> &[RamRegion] {
        &self.root.ram_regions
    }

    /// The memory layouts, each with the placement the root gives it.
    pub fn layouts(&self) -> &[MemoryLayout] {
        &self.layouts
    }

    /// Every register element of every instance, MMIO instances first, in
    /// root order and then offset order.
    pub fn registers(&self) -> &[ExpandedRegister] {
        &self.registers
    }

    /// The register element `INSTANCE.REGISTER`.
    pub fn register(&self, identity: &str) -> Option<&ExpandedRegister> {
        self.registers
            .iter()
            .find(|register| register.identity == identity)
    }

    /// The field `INSTANCE.REGISTER.FIELD`.
    pub fn field(&self, identity: &str) -> Option<&ExpandedField> {
        let (register, _) = identity.rsplit_once('.')?;
        self.register(register)?
            .fields
            .iter()
            .find(|field| field.identity == identity)
    }
}

fn safe_include(
    context: Context<'_>,
    include: &str,
    seen: &mut BTreeSet<String>,
) -> Result<(), Error> {
    if crate::validate_relative_fragment(include).is_err() {
        return Err(context.error(include, ErrorKind::UnsafePath(include.to_owned())));
    }
    if !seen.insert(include.to_owned()) {
        return Err(context.error(include, ErrorKind::DuplicateInclude(include.to_owned())));
    }
    Ok(())
}

fn load_types(
    context: Context<'_>,
    base: &Path,
    includes: &[String],
    harts: Option<topology::Geometry>,
) -> Result<BTreeMap<String, BlockType>, Error> {
    let mut seen = BTreeSet::new();
    let mut types = BTreeMap::new();
    let mut budget = ExpansionBudget::default();
    for include in includes {
        safe_include(context, include, &mut seen)?;
        let block_type =
            BlockType::load(&base.join(include), context.evidence, &mut budget, harts)?;
        let name = block_type.name().to_owned();
        if types.contains_key(&name) {
            return Err(Error::new(
                &block_type.path,
                &name,
                ErrorKind::Duplicate(format!("type {name}")),
            ));
        }
        types.insert(name, block_type);
    }
    Ok(types)
}

/// Memory layouts keep their own format; the root, not the file, places them.
fn load_layouts(
    context: Context<'_>,
    base: &Path,
    includes: &[String],
) -> Result<Vec<MemoryLayout>, Error> {
    let mut seen = BTreeSet::new();
    for include in includes {
        safe_include(context, include, &mut seen)?;
    }
    if includes.is_empty() {
        return Ok(Vec::new());
    }
    let layouts = crate::layouts::parse_layouts(base, includes, &mut BTreeMap::new())
        .map_err(|error| context.error("", ErrorKind::Layouts(Box::new(error))))?;
    for layout in &layouts {
        if !layout.placement.is_empty() {
            return Err(context.error(
                &layout.name,
                ErrorKind::Placement(
                    "the layout file places itself; the root's layout-placements place it"
                        .to_owned(),
                ),
            ));
        }
        for review in &layout.review {
            if !review.sources.is_empty() {
                context.sources(&review.entity, &review.sources)?;
            }
        }
    }
    Ok(layouts)
}

/// RAM regions by name: unique lowercase names, nonempty ranges, sources.
fn check_ram_regions(
    context: Context<'_>,
    regions: &[RamRegion],
) -> Result<BTreeMap<String, (u64, u64)>, Error> {
    let mut out = BTreeMap::new();
    for region in regions {
        let fail = |kind| context.error(&region.name, kind);
        if region.name.is_empty()
            || !region
                .name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        {
            return Err(fail(ErrorKind::InvalidName(format!(
                "RAM region {:?} is not lowercase-kebab",
                region.name
            ))));
        }
        let end = region
            .base
            .checked_add(region.size)
            .filter(|_| region.size > 0)
            .ok_or_else(|| fail(ErrorKind::RamRegion("is empty or overflows".to_owned())))?;
        if out
            .insert(region.name.clone(), (region.base, end))
            .is_some()
        {
            return Err(fail(ErrorKind::Duplicate(format!(
                "RAM region {}",
                region.name
            ))));
        }
        context.sources(&region.name, &region.sources)?;
    }
    Ok(out)
}

/// Each MMIO instance names an MMIO type, has a word-aligned nonempty
/// extent holding every word of its type and overlaps no other instance
/// or RAM region.
fn check_instances(
    context: Context<'_>,
    root: &RootFile,
    types: &BTreeMap<String, BlockType>,
    names: &mut BTreeSet<String>,
) -> Result<(), Error> {
    let mut ranges = Vec::new();
    for instance in &root.instances {
        let fail = |kind| context.error(&instance.name, kind);
        check_name(&instance.name, false).map_err(fail)?;
        if !names.insert(instance.name.clone()) {
            return Err(fail(ErrorKind::Duplicate(format!(
                "instance {}",
                instance.name
            ))));
        }
        let block_type = types
            .get(&instance.block_type)
            .ok_or_else(|| fail(ErrorKind::UnknownType(instance.block_type.clone())))?;
        if block_type.space() != Space::Mmio {
            return Err(fail(ErrorKind::WrongSpace {
                block_type: instance.block_type.clone(),
                expected: Space::Mmio.label(),
                found: block_type.space().label(),
            }));
        }
        let extent = &instance.extent;
        let end = instance
            .base
            .checked_add(extent.size)
            .filter(|_| {
                extent.size > 0 && instance.base.is_multiple_of(4) && extent.size.is_multiple_of(4)
            })
            .ok_or_else(|| {
                fail(ErrorKind::Extent(
                    "base and size are word-aligned, the size nonzero and the end in range"
                        .to_owned(),
                ))
            })?;
        if extent.partial || !extent.sources.is_empty() {
            context.sources(&instance.name, &extent.sources)?;
        }
        context.sources(&instance.name, &instance.sources)?;
        if let Some(element) = block_type
            .elements
            .iter()
            .find(|element| element.offset + u64::from(element.width / 8) > extent.size)
        {
            return Err(fail(ErrorKind::OutsideExtent(format!(
                "{}.{}",
                instance.name, element.name
            ))));
        }
        ranges.push((instance.base, end, format!("instance {}", instance.name)));
    }
    for region in &root.ram_regions {
        ranges.push((
            region.base,
            region.base.saturating_add(region.size),
            format!("RAM region {}", region.name),
        ));
    }
    ranges.sort();
    for pair in ranges.windows(2) {
        if let [first, second] = pair
            && first.1 > second.0
        {
            return Err(context.error(&second.2, ErrorKind::AddressOverlap(first.2.clone())));
        }
    }
    Ok(())
}

/// Interrupt sources have unique names and numbers and name an MMIO
/// instance and, when its type names outputs, one of them.
fn check_interrupts(
    context: Context<'_>,
    root: &RootFile,
    types: &BTreeMap<String, BlockType>,
) -> Result<(), Error> {
    let mut names = BTreeSet::new();
    let mut numbers = BTreeSet::new();
    let count = root.interrupt_sources.count;
    context.sources("interrupt-sources", &root.interrupt_sources.sources)?;
    if count > u32::from(u16::MAX) + 1 {
        return Err(context.error(
            "interrupt-sources",
            ErrorKind::InterruptCoverage("count exceeds the u16 source number space".to_owned()),
        ));
    }
    for (name, source) in root
        .interrupts
        .iter()
        .map(|irq| (&irq.name, irq.source))
        .chain(
            root.unbound_interrupts
                .iter()
                .map(|irq| (&irq.name, irq.source)),
        )
    {
        check_name(name, false).map_err(|kind| context.error(name, kind))?;
        if !names.insert(name.as_str()) {
            return Err(context.error(name, ErrorKind::Duplicate(format!("interrupt {name}"))));
        }
        if !numbers.insert(source) {
            return Err(context.error(name, ErrorKind::DuplicateSourceNumber(source)));
        }
        if u32::from(source) >= count {
            return Err(context.error(
                name,
                ErrorKind::InterruptCoverage(format!("source {source} is outside 0..{count}")),
            ));
        }
    }
    for interrupt in &root.interrupts {
        let fail = |kind| context.error(&interrupt.name, kind);
        let block_type = root
            .instances
            .iter()
            .find(|instance| instance.name == interrupt.instance)
            .and_then(|instance| types.get(&instance.block_type))
            .ok_or_else(|| fail(ErrorKind::UnknownInstance(interrupt.instance.clone())))?;
        let outputs = &block_type.file.interrupt_outputs;
        match &interrupt.output {
            None if !outputs.is_empty() => {
                return Err(fail(ErrorKind::Output(format!(
                    "type {} names its outputs; name one",
                    block_type.name()
                ))));
            }
            Some(output) if outputs.is_empty() => {
                return Err(fail(ErrorKind::Output(format!(
                    "type {} names no outputs, so not {output}",
                    block_type.name()
                ))));
            }
            Some(output) if !outputs.iter().any(|known| &known.name == output) => {
                return Err(fail(ErrorKind::Output(format!(
                    "type {} has no output {output}",
                    block_type.name()
                ))));
            }
            _ => {}
        }
        context.sources(&interrupt.name, &interrupt.sources)?;
    }
    for interrupt in &root.unbound_interrupts {
        if interrupt.reason.trim().is_empty() {
            return Err(context.error(
                &interrupt.name,
                ErrorKind::InterruptCoverage("unbound source needs a nonblank reason".to_owned()),
            ));
        }
        context.sources(&interrupt.name, &interrupt.sources)?;
    }
    for source in 0..count {
        // The count was checked against the source number space above.
        let source = source as u16;
        if !numbers.contains(&source) {
            return Err(context.error(
                "interrupt-sources",
                ErrorKind::MissingInterruptSource(source),
            ));
        }
    }
    Ok(())
}

/// Analog-I2C instances name analog types, take unique codes and keep
/// unambiguous request routes.
fn check_analog(
    context: Context<'_>,
    root: &RootFile,
    types: &BTreeMap<String, BlockType>,
    names: &mut BTreeSet<String>,
) -> Result<(), Error> {
    let mut codes = BTreeSet::new();
    let mut routes = BTreeSet::new();
    for instance in &root.analog_i2c.instances {
        let fail = |kind| context.error(&instance.name, kind);
        check_name(&instance.name, false).map_err(fail)?;
        if !names.insert(instance.name.clone()) {
            return Err(fail(ErrorKind::Duplicate(format!(
                "instance {}",
                instance.name
            ))));
        }
        let block_type = types
            .get(&instance.block_type)
            .ok_or_else(|| fail(ErrorKind::UnknownType(instance.block_type.clone())))?;
        if block_type.space() != Space::AnalogI2c {
            return Err(fail(ErrorKind::WrongSpace {
                block_type: instance.block_type.clone(),
                expected: Space::AnalogI2c.label(),
                found: block_type.space().label(),
            }));
        }
        if !codes.insert(instance.code) {
            return Err(fail(ErrorKind::DuplicateCode(instance.code)));
        }
        context.sources(&instance.name, &instance.sources)?;
        for route in &instance.routes {
            let entity = format!("{}.{}", instance.name, route.profile);
            if route.profile.is_empty()
                || !route
                    .profile
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
                || route.host > 1
                || route.read_select.count_ones() > 1
                || !matches!(route.read_mask_bits, 24 | 32)
            {
                return Err(context.error(
                    &entity,
                    ErrorKind::Route(
                        "needs a profile name, host 0 or 1, a one-hot or zero read-select and 24 or 32 read-mask bits"
                            .to_owned(),
                    ),
                ));
            }
            if !routes.insert((route.profile.clone(), route.request_code)) {
                return Err(context.error(
                    &entity,
                    ErrorKind::Route(format!(
                        "profile {} routes request code {:#04x} twice",
                        route.profile, route.request_code
                    )),
                ));
            }
            context.sources(&entity, &route.sources)?;
        }
    }
    Ok(())
}

/// Every placement names a layout once and existing RAM regions; an array
/// lies where its elements do, and an address field names RAM regions.
fn place_layouts(
    context: Context<'_>,
    root: &RootFile,
    regions: &BTreeMap<String, (u64, u64)>,
    layouts: &mut [MemoryLayout],
) -> Result<(), Error> {
    let mut placed = BTreeSet::new();
    for placement in &root.layout_placements {
        let fail = |reason: String| context.error(&placement.layout, ErrorKind::Placement(reason));
        if !placed.insert(placement.layout.as_str()) {
            return Err(context.error(
                &placement.layout,
                ErrorKind::Duplicate(format!("placement of {}", placement.layout)),
            ));
        }
        let distinct = placement.regions.iter().collect::<BTreeSet<_>>();
        if placement.regions.is_empty() || distinct.len() != placement.regions.len() {
            return Err(fail("names one or more regions, each once".to_owned()));
        }
        if let Some(region) = placement
            .regions
            .iter()
            .find(|region| !regions.contains_key(*region))
        {
            return Err(fail(format!("RAM region {region} is not in the root")));
        }
        let layout = layouts
            .iter_mut()
            .find(|layout| layout.name == placement.layout)
            .ok_or_else(|| fail(format!("no included layout is {}", placement.layout)))?;
        layout.placement = placement.regions.clone();
    }
    for layout in layouts.iter() {
        let fail = |reason: String| context.error(&layout.name, ErrorKind::Placement(reason));
        for field in layout.units.iter().flat_map(|unit| &unit.fields) {
            if let Some(region) = field
                .placement
                .iter()
                .find(|region| !regions.contains_key(*region))
            {
                return Err(fail(format!(
                    "field {} names RAM region {region}, which is not in the root",
                    field.name
                )));
            }
        }
    }
    Ok(())
}

/// Every instance's registers and fields at their addresses.
fn expand(
    context: Context<'_>,
    root: &RootFile,
    types: &BTreeMap<String, BlockType>,
    budget: &mut ExpansionBudget,
) -> Result<Vec<ExpandedRegister>, Error> {
    let mut out = Vec::new();
    let placed = root
        .instances
        .iter()
        .map(|instance| {
            (
                &instance.name,
                &instance.block_type,
                Placed::Mmio(instance.base),
            )
        })
        .chain(root.analog_i2c.instances.iter().map(|instance| {
            (
                &instance.name,
                &instance.block_type,
                Placed::Analog(instance.code),
            )
        }));
    let mut count = 0;
    for (instance, name, _) in placed.clone() {
        let block_type = types
            .get(name)
            .ok_or_else(|| context.error(instance, ErrorKind::UnknownType(name.clone())))?;
        for element in &block_type.elements {
            let fail = |kind| context.error(instance, kind);
            let register = block_type
                .templates
                .get(&element.key)
                .ok_or_else(|| fail(ErrorKind::UnknownType(element.key.clone())))?;
            let source_bytes = |key: &str| -> Result<u64, Error> {
                let review = block_type
                    .reviews
                    .get(key)
                    .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("review")))?;
                Ok(review.sources.iter().fold(0u64, |sum, source| {
                    sum.saturating_add(source.len() as u64)
                        .saturating_add(std::mem::size_of::<String>() as u64)
                }))
            };
            // Each identity is owned by both the record and its review.
            let identity = (instance.len() as u64)
                .saturating_add(element.name.len() as u64)
                .saturating_add(1);
            let mut text = identity
                .saturating_mul(2)
                .saturating_add(instance.len() as u64)
                .saturating_add(element.key.len() as u64)
                .saturating_add(source_bytes(&element.key)?);
            for field in &register.fields {
                text = text
                    .saturating_add(
                        identity
                            .saturating_add(1)
                            .saturating_add(field.name.len() as u64)
                            .saturating_mul(2),
                    )
                    .saturating_add(field.name.len() as u64)
                    .saturating_add(source_bytes(&format!("{}.{}", element.key, field.name))?);
            }
            budget.include(context, instance, 1, 1 + register.fields.len() as u64, text)?;
        }
        count += block_type.elements.len();
    }
    out.try_reserve_exact(count)
        .map_err(|error| context.error(&root.chip, ErrorKind::Allocation(error)))?;
    for (instance, block_type, placed) in placed {
        let block_type = types
            .get(block_type)
            .ok_or_else(|| context.error(instance, ErrorKind::UnknownType(block_type.clone())))?;
        for element in &block_type.elements {
            let identity = format!("{instance}.{}", element.name);
            let fail = |kind| context.error(&identity, kind);
            let location = match placed {
                Placed::Mmio(base) => Location::Mmio {
                    address: base + element.offset,
                },
                Placed::Analog(block) => Location::AnalogI2c(AnalogAddress {
                    block,
                    index: u8::try_from(element.offset).map_err(|_| {
                        fail(ErrorKind::RegisterGeometry(
                            "analog byte index is above 0xff".to_owned(),
                        ))
                    })?,
                }),
            };
            let register = block_type
                .templates
                .get(&element.key)
                .ok_or_else(|| fail(ErrorKind::UnknownType(element.key.clone())))?;
            let review = |key: &str, identity: &str| {
                block_type
                    .reviews
                    .get(key)
                    .map(|review| ReviewAnnotation {
                        entity: identity.to_owned(),
                        ..review.clone()
                    })
                    .ok_or_else(|| fail(ErrorKind::ReviewIncomplete("review")))
            };
            let mut fields = Vec::with_capacity(register.fields.len());
            for field in &register.fields {
                let (low, width) = parse_bits(&field.bits).map_err(fail)?;
                let field_identity = format!("{identity}.{}", field.name);
                fields.push(ExpandedField {
                    review: review(&format!("{}.{}", element.key, field.name), &field_identity)?,
                    identity: field_identity,
                    name: field.name.clone(),
                    low,
                    width,
                    class: field.class,
                });
            }
            fields.sort_by_key(|field| field.low);
            let reset = if register.element_resets.is_empty() {
                register.reset
            } else {
                let dim = register.dim.ok_or_else(|| {
                    fail(ErrorKind::Dim(
                        "element resets need an indexed register".into(),
                    ))
                })?;
                let ordinal = (0..dim.count)
                    .position(|i| {
                        register
                            .name
                            .replace("%s", &dim.first.wrapping_add(i).to_string())
                            == element.name
                    })
                    .ok_or_else(|| {
                        fail(ErrorKind::Dim("reset element has no family index".into()))
                    })?;
                Some(
                    register
                        .element_resets
                        .get(ordinal)
                        .copied()
                        .ok_or_else(|| {
                            fail(ErrorKind::Dim("reset list omits an element".into()))
                        })?,
                )
            };
            out.push(ExpandedRegister {
                review: review(&element.key, &identity)?,
                identity,
                instance: instance.clone(),
                key: element.key.clone(),
                location,
                width: element.width,
                reset,
                reserved: register
                    .reserved
                    .as_ref()
                    .map_or(0, |reserved| reserved.mask),
                fields,
            });
        }
    }
    Ok(out)
}

#[derive(Clone, Copy)]
enum Placed {
    Mmio(u64),
    Analog(u8),
}
