//! One instance's block: the SVD's registers joined with their header
//! registers, the header winning, and the overlay's facts applied.

use std::collections::{BTreeMap, BTreeSet};

use super::header::HeaderRegister;
use super::svd::{SvdArray, SvdBlock, SvdRegister};
use super::{
    Block, Cite, Cites, FindingKind, ImportError, OutCommand, OutField, OutRegister, OutValue,
    Report, Review,
};
use crate::chip_model::FieldClass;
use crate::soc_headers::{self, Class, debug_only, read_clears, reserved_description, sequential_port};
use crate::soc_overlay::{ReservedWords, same_field};
use crate::{FieldKind, FieldRule, OverlayAccess, RegisterRule, StrictnessOverlay};

pub(super) struct InstanceInputs<'a> {
    pub(super) name: &'a str,
    pub(super) svd: Option<&'a SvdBlock>,
    pub(super) svd_name: Option<&'a str>,
    pub(super) header_path: &'a str,
    pub(super) header: &'a [HeaderRegister],
    pub(super) layout: Option<Vec<ReservedWords>>,
    pub(super) overlay: Option<&'a StrictnessOverlay>,
}

fn mask(low: u32, width: u32) -> u32 {
    if width == 0 || low >= 32 {
        0
    } else if width >= 32 {
        u32::MAX << low
    } else {
        ((1u32 << width) - 1) << low
    }
}

fn bits(low: u32, width: u32) -> String {
    if width == 1 {
        format!("[{low}]")
    } else {
        format!("[{}:{low}]", low + width - 1)
    }
}

/// The class a header access text gives a field.
fn header_class(access: &str) -> Option<FieldClass> {
    Some(match soc_headers::class(access) {
        Class::Plain => FieldClass::Plain,
        Class::ReadOnly => FieldClass::ReadOnly,
        Class::Trigger => FieldClass::WriteTrigger,
        Class::SelfClearing => FieldClass::SelfClearing,
        Class::OneToClear => FieldClass::WriteOneToClear,
        Class::HardwareWritable => FieldClass::HardwareWritten,
        Class::SetOnly => FieldClass::SetOnly,
        Class::WriteOnly => FieldClass::WriteOnly,
        Class::Unknown => return None,
    })
}

/// An `esp-idf:<path>:<line>` citation as a cited line.
fn line(source: &str, context: &str) -> Result<Cite, ImportError> {
    source
        .strip_prefix("esp-idf:")
        .and_then(|rest| rest.rsplit_once(':'))
        .and_then(|(path, line)| {
            let line = line.parse::<usize>().ok().filter(|line| *line > 0)?;
            (!path.is_empty()).then(|| Cite::Line {
                path: path.to_owned(),
                line,
            })
        })
        .ok_or_else(|| {
            ImportError::new(
                context,
                format!("source {source:?} is no esp-idf:<path>:<line> citation"),
            )
        })
}

fn lines<'a>(
    sources: impl IntoIterator<Item = &'a String>,
    context: &str,
) -> Result<Cites, ImportError> {
    sources
        .into_iter()
        .map(|source| line(source, context))
        .collect()
}

/// The citations of the driver writes of `field` in `rule`.
fn driver_lines(rule: &RegisterRule, field: &str, context: &str) -> Result<Cites, ImportError> {
    let mut out = Cites::new();
    for write in rule.driver_writes.iter().filter(|w| w.field == field) {
        out.insert(line(&write.source, context)?);
    }
    for write in rule.driver_writes_word.iter().filter(|w| w.field == field) {
        out.insert(line(&write.value, context)?);
        out.insert(line(&write.store, context)?);
        if let Some(returned) = &write.returned {
            out.insert(line(returned, context)?);
        }
        out.extend(lines(&write.reviewed_chain, context)?);
    }
    Ok(out)
}

/// A description that says something: not empty, not only punctuation.
fn prose(text: &str) -> Option<String> {
    text.chars()
        .any(|c| c.is_alphanumeric())
        .then(|| text.to_owned())
}

/// A name the block-type format takes: `UPPER_SNAKE` from a letter.
fn valid_name(name: &str) -> bool {
    name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

fn digitless(text: &str) -> String {
    text.replace(|c: char| c.is_ascii_digit(), "")
}

/// A header name without the block's prefix; in an array, without the
/// element's number in any word.
fn own_name(name: &str, prefix: &str, array: bool) -> String {
    let own = name
        .strip_prefix(prefix)
        .filter(|rest| rest.starts_with(|c: char| c.is_ascii_uppercase()))
        .unwrap_or(name);
    if !array {
        return own.to_owned();
    }
    own.split('_')
        .map(|word| {
            let stem = word.trim_end_matches(|c: char| c.is_ascii_digit());
            if stem.is_empty() { word } else { stem }
        })
        .collect::<Vec<_>>()
        .join("_")
}

/// The prefix the header's register names put before the SVD's: the most
/// frequent at matching offsets, the shorter on a tie.
fn svd_prefix(svd: &SvdBlock, header: &[HeaderRegister]) -> String {
    let mut prefixes = BTreeMap::<String, usize>::new();
    for register in &svd.registers {
        for candidate in header.iter().filter(|h| h.offset == register.offset) {
            if let Some(prefix) = candidate.name.strip_suffix(register.name.as_str())
                && (prefix.is_empty() || prefix.ends_with('_'))
            {
                *prefixes.entry(prefix.to_owned()).or_default() += 1;
            }
        }
    }
    prefixes
        .into_iter()
        .max_by_key(|(prefix, count)| (*count, std::cmp::Reverse(prefix.len())))
        .map(|(prefix, _)| prefix)
        .unwrap_or_default()
}

/// The prefix of a header's names for a block the SVD lacks: the file's
/// name (`modem_syscon_reg.h` is `MODEM_SYSCON_`) where every register
/// starts with it, else the longest prefix ending in `_` they share.
fn header_prefix(path: &str, header: &[HeaderRegister]) -> String {
    let stem = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .trim_end_matches(".h")
        .trim_end_matches("_reg")
        .to_ascii_uppercase();
    let prefix = format!("{stem}_");
    if header
        .iter()
        .all(|register| register.name.starts_with(&prefix) && register.name.len() > prefix.len())
    {
        return prefix;
    }
    common_prefix(header)
}

/// The longest prefix ending in `_` that every register name of a header
/// shares.
fn common_prefix(header: &[HeaderRegister]) -> String {
    let Some(first) = header.first() else {
        return String::new();
    };
    let mut prefix = first.name.as_str();
    for register in header {
        while !register.name.starts_with(prefix) {
            prefix = &prefix[..prefix.len() - 1];
        }
    }
    match prefix.rfind('_') {
        Some(end) => prefix[..=end].to_owned(),
        None => String::new(),
    }
}

enum Lookup {
    Matched(usize),
    Moved(usize),
    Absent { reserved: Option<usize> },
}

/// The header register of an SVD register element, by the rules of the old
/// soc path: the same name at the same offset, an array element or the
/// SVD's name as the header's tail at the same offset, the same name at
/// another offset, or the same last word at the same offset.
fn lookup(
    header: &[HeaderRegister],
    layout: Option<&[ReservedWords]>,
    prefix: &str,
    name: &str,
    offset: u32,
    array: bool,
) -> Lookup {
    let full = format!("{prefix}{name}");
    let at = |offset: u32| {
        header
            .iter()
            .enumerate()
            .filter(move |(_, r)| r.offset == offset)
    };
    if let Some((index, _)) = at(offset).find(|(_, r)| r.name == full) {
        return Lookup::Matched(index);
    }
    let last = |text: &str| {
        text.rsplit('_')
            .next()
            .unwrap_or("")
            .trim_end_matches(|c: char| c.is_ascii_digit())
            .to_owned()
    };
    if let Some((index, _)) = at(offset).find(|(_, r)| {
        (array && digitless(&r.name) == digitless(&full)) || r.name.ends_with(&format!("_{name}"))
    }) {
        return Lookup::Matched(index);
    }
    if let Some(index) = header.iter().position(|r| r.name == full) {
        return Lookup::Moved(index);
    }
    if let Some((index, _)) = at(offset).find(|(_, r)| last(&r.name) == last(name)) {
        return Lookup::Matched(index);
    }
    let reserved = layout.and_then(|words| {
        words
            .iter()
            .find(|w| {
                let offset = u64::from(offset);
                offset >= w.offset && offset < w.offset + w.words * 4
            })
            .map(|w| w.line)
    });
    Lookup::Absent { reserved }
}

/// The overlay's rules of one SVD peripheral.
struct Rules<'a> {
    registers: BTreeMap<&'a str, &'a RegisterRule>,
    fields: BTreeMap<(&'a str, &'a str), &'a FieldRule>,
    used_registers: BTreeSet<&'a str>,
    used_fields: BTreeSet<(&'a str, &'a str)>,
}

impl<'a> Rules<'a> {
    fn new(overlay: Option<&'a StrictnessOverlay>, peripheral: Option<&str>) -> Self {
        let mut registers = BTreeMap::new();
        let mut fields = BTreeMap::new();
        if let (Some(overlay), Some(peripheral)) = (overlay, peripheral) {
            for rule in overlay.registers.iter().filter(|r| r.peripheral == peripheral) {
                registers.insert(rule.register.as_str(), rule);
            }
            for rule in overlay.fields.iter().filter(|r| r.peripheral == peripheral) {
                fields.insert((rule.register.as_str(), rule.field.as_str()), rule);
            }
        }
        Self {
            registers,
            fields,
            used_registers: BTreeSet::new(),
            used_fields: BTreeSet::new(),
        }
    }

    /// The register rule of an SVD element or of its array's template.
    fn register(&mut self, element: &str, template: Option<&str>) -> Option<(&'a str, &'a RegisterRule)> {
        let found = self
            .registers
            .get_key_value(element)
            .or_else(|| template.and_then(|t| self.registers.get_key_value(t)))
            .map(|(key, rule)| (*key, *rule));
        if let Some((key, _)) = found {
            self.used_registers.insert(key);
        }
        found
    }

    fn fields_of(&mut self, element: &str, template: Option<&str>) -> Vec<&'a FieldRule> {
        let keys = self
            .fields
            .keys()
            .filter(|(register, _)| *register == element || Some(*register) == template)
            .copied()
            .collect::<Vec<_>>();
        keys.into_iter()
            .filter_map(|key| {
                self.used_fields.insert(key);
                self.fields.get(&key).copied()
            })
            .collect()
    }
}

/// One element of the block before arrays fold: its register, the SVD
/// array it came from and the SVD names of it and its fields.
struct Element {
    register: OutRegister,
    array: Option<SvdArray>,
    svd_name: Option<String>,
    /// SVD field name to imported field name.
    svd_fields: BTreeMap<String, String>,
    from_header: bool,
}

pub(super) fn instance(
    inputs: &InstanceInputs<'_>,
    report: &mut Report,
) -> Result<Block, ImportError> {
    let name = inputs.name;
    let header = inputs.header;
    if header.is_empty() {
        return Err(ImportError::new(
            inputs.header_path,
            "defines no register this import reads",
        ));
    }
    let layout = inputs.layout.as_deref();
    let prefix = match inputs.svd {
        Some(svd) => svd_prefix(svd, header),
        None => header_prefix(inputs.header_path, header),
    };
    let mut rules = Rules::new(inputs.overlay, inputs.svd_name);
    let mut elements = Vec::new();
    let mut used = BTreeSet::new();

    for register in inputs.svd.map(|svd| svd.registers.as_slice()).unwrap_or_default() {
        let entity = format!("{name}.{}", register.name);
        let array = register.array.is_some();
        let template = register.array.as_ref().map(|a| a.name.as_str());
        let found = lookup(header, layout, &prefix, &register.name, register.offset, array);
        let index = match found {
            Lookup::Matched(index) | Lookup::Moved(index) if used.contains(&index) => {
                report.add(
                    &entity,
                    FindingKind::RegisterSvdOnly,
                    format!(
                        "its header register {} is another SVD register's; the SVD's fields, read-only",
                        header[index].name
                    ),
                );
                elements.push(svd_only(register, &entity)?);
                continue;
            }
            Lookup::Matched(index) => index,
            Lookup::Moved(index) => {
                report.add(
                    &entity,
                    FindingKind::RegisterMoved,
                    format!(
                        "the SVD places it at {:#x}, {}:{} at {:#x}; the header's offset",
                        register.offset, inputs.header_path, header[index].line, header[index].offset
                    ),
                );
                index
            }
            Lookup::Absent { reserved: Some(line) } => {
                report.add(
                    &entity,
                    FindingKind::RegisterReserved,
                    format!(
                        "the header's layout reserves its word {:#x} (line {line}); not imported",
                        register.offset
                    ),
                );
                continue;
            }
            Lookup::Absent { reserved: None } => {
                report.add(
                    &entity,
                    FindingKind::RegisterSvdOnly,
                    format!(
                        "{} defines no register at {:#x}; the SVD's fields, read-only",
                        inputs.header_path, register.offset
                    ),
                );
                elements.push(svd_only(register, &entity)?);
                continue;
            }
        };
        used.insert(index);
        let h = &header[index];
        let full = format!("{prefix}{}", register.name);
        if h.name != full {
            report.add(
                &entity,
                FindingKind::RegisterName,
                format!("the header names it {}; the SVD's name stays", h.name),
            );
        }
        let rule = rules.register(&register.name, template).map(|(_, rule)| rule);
        let field_rules = rules.fields_of(&register.name, template);
        elements.push(from_header(
            &HeaderInputs {
                entity: &entity,
                name: register.name.clone(),
                header: h,
                header_path: inputs.header_path,
                svd: Some(register),
                rule,
                field_rules: &field_rules,
                prefix: &prefix,
            },
            report,
        )?);
    }
    for (index, h) in header.iter().enumerate() {
        if used.contains(&index) {
            continue;
        }
        let register_name = own_name(&h.name, &prefix, false);
        let entity = format!("{name}.{register_name}");
        if inputs.svd.is_some() {
            report.add(
                &entity,
                FindingKind::RegisterAdded,
                format!(
                    "{}:{} defines it at {:#x}, the SVD does not; imported from the header",
                    inputs.header_path, h.line, h.offset
                ),
            );
        }
        let rule = rules.register(&register_name, None).map(|(_, rule)| rule);
        let field_rules = rules.fields_of(&register_name, None);
        elements.push(from_header(
            &HeaderInputs {
                entity: &entity,
                name: register_name,
                header: h,
                header_path: inputs.header_path,
                svd: None,
                rule,
                field_rules: &field_rules,
                prefix: &prefix,
            },
            report,
        )?);
    }

    // A header register and an SVD-only one at the same word: the header's.
    let header_words = elements
        .iter()
        .filter(|e| e.from_header)
        .map(|e| e.register.offset)
        .collect::<BTreeSet<_>>();
    elements.retain(|element| {
        let shadowed = !element.from_header && header_words.contains(&element.register.offset);
        if shadowed {
            report.add(
                format!("{name}.{}", element.register.name),
                FindingKind::RegisterSvdOnly,
                format!(
                    "the header defines another register at {:#x}; the header's stands",
                    element.register.offset
                ),
            );
        }
        !shadowed
    });

    let (mut registers, names) = fold(name, elements, report);
    let commands = commands(inputs, &mut registers, &names, report)?;

    for key in rules.registers.keys() {
        if !rules.used_registers.contains(key) {
            report.add(
                format!("{name}.{key}"),
                FindingKind::OverlayUnapplied,
                "the overlay's register entry names no imported register".to_owned(),
            );
        }
    }
    for key in rules.fields.keys() {
        if !rules.used_fields.contains(key) {
            report.add(
                format!("{name}.{}.{}", key.0, key.1),
                FindingKind::OverlayUnapplied,
                "the overlay's field entry names no imported register".to_owned(),
            );
        }
    }

    registers.sort_by_key(|register| register.offset);
    let mut seen = BTreeSet::new();
    for register in &registers {
        if !seen.insert(register.name.as_str()) {
            return Err(ImportError::new(
                format!("{name}.{}", register.name),
                "two imported registers share the name",
            ));
        }
    }
    let mut words = registers
        .iter()
        .flat_map(|register| {
            let (count, stride) = register.dim.unwrap_or((1, 0));
            (0..count).map(move |index| {
                let start = u64::from(register.offset) + u64::from(index) * u64::from(stride);
                (start, start + u64::from(register.width / 8), register.name.as_str())
            })
        })
        .collect::<Vec<_>>();
    words.sort_unstable();
    for pair in words.windows(2) {
        if let [first, second] = pair
            && first.1 > second.0
        {
            return Err(ImportError::new(
                format!("{name}.{}", second.2),
                format!("overlaps register {}", first.2),
            ));
        }
    }
    Ok(Block {
        registers,
        commands,
    })
}

/// An SVD register the header does not define: its fields, read-only.
fn svd_only(register: &SvdRegister, entity: &str) -> Result<Element, ImportError> {
    let mut fields = Vec::new();
    let mut taken = 0_u32;
    for field in &register.fields {
        let field_mask = mask(field.low, field.width);
        if field.low + field.width > register.width || taken & field_mask != 0 {
            return Err(ImportError::new(
                format!("{entity}.{}", field.name),
                "overlaps another SVD field or the register's end, and no header decides",
            ));
        }
        taken |= field_mask;
        fields.push(OutField {
            name: field.name.clone(),
            low: field.low,
            width: field.width,
            class: FieldClass::ReadOnly,
            header_class: FieldClass::ReadOnly,
            description: field.description.clone(),
            values: Vec::new(),
            review: Review {
                sources: Cites::from([Cite::Svd]),
                derived: false,
            },
        });
    }
    fields.sort_by_key(|field| field.low);
    let svd_fields = fields
        .iter()
        .map(|field| (field.name.clone(), field.name.clone()))
        .collect();
    Ok(Element {
        register: OutRegister {
            name: register.name.clone(),
            offset: register.offset,
            width: register.width,
            dim: None,
            description: register.description.clone(),
            reset: svd_reset(register),
            reserved: 0,
            fifo: None,
            data_port: None,
            read_side_effect: None,
            fields,
            review: Review {
                sources: Cites::from([Cite::Svd]),
                derived: false,
            },
        },
        array: register.array.clone(),
        svd_name: Some(register.name.clone()),
        svd_fields,
        from_header: false,
    })
}

fn width_mask(width: u32) -> u64 {
    if width >= 32 {
        u64::from(u32::MAX)
    } else {
        (1u64 << width) - 1
    }
}

fn svd_reset(register: &SvdRegister) -> Option<(u32, u32)> {
    let (value, reset_mask) = register.reset?;
    let full = width_mask(register.width);
    let reset_mask = u32::try_from(reset_mask & full).ok()?;
    let value = u32::try_from(value & full).ok()? & reset_mask;
    (reset_mask != 0).then_some((value, reset_mask))
}

struct HeaderInputs<'a> {
    entity: &'a str,
    name: String,
    header: &'a HeaderRegister,
    header_path: &'a str,
    svd: Option<&'a SvdRegister>,
    rule: Option<&'a RegisterRule>,
    field_rules: &'a [&'a FieldRule],
    prefix: &'a str,
}

/// The class of a header field: its access class, `debug-only` where the
/// header keeps it to debugging and no driver writes it, `read-clears`
/// where a read clears it.
fn field_class(
    access: &str,
    descriptions: &[&str],
    driver: bool,
    entity: &str,
) -> Result<(FieldClass, FieldClass), ImportError> {
    let class = header_class(access).ok_or_else(|| {
        ImportError::new(
            entity,
            format!("the header's access class {access:?} has no rule"),
        )
    })?;
    let refined = if descriptions.iter().any(|text| read_clears(text)) {
        FieldClass::ReadClears
    } else if descriptions.iter().any(|text| debug_only(text)) && !driver {
        FieldClass::DebugOnly
    } else {
        class
    };
    Ok((refined, class))
}

fn from_header(inputs: &HeaderInputs<'_>, report: &mut Report) -> Result<Element, ImportError> {
    let entity = inputs.entity;
    let h = inputs.header;
    let width = inputs.svd.map_or(32, |svd| svd.width);
    if width != 32 {
        return Err(ImportError::new(
            entity,
            format!("the SVD states {width} bits; a header register is 32"),
        ));
    }
    let array = inputs.svd.is_some_and(|svd| svd.array.is_some());
    let svd_fields = inputs.svd.map(|svd| svd.fields.as_slice()).unwrap_or_default();
    for (index, a) in svd_fields.iter().enumerate() {
        for b in svd_fields.iter().skip(index + 1) {
            let shared = mask(a.low, a.width) & mask(b.low, b.width);
            if shared != 0 {
                report.add(
                    entity,
                    FindingKind::FieldOverlap,
                    format!(
                        "the SVD's fields {} {} and {} {} share bits {shared:#010x}; the header decides",
                        a.name,
                        bits(a.low, a.width),
                        b.name,
                        bits(b.low, b.width)
                    ),
                );
            }
        }
    }
    let mut declared = 0_u32;
    for field in &h.fields {
        if !reserved_description(&field.description) {
            declared |= mask(field.low, field.width);
        }
    }
    let written = inputs
        .rule
        .map(RegisterRule::driver_written)
        .unwrap_or_default();
    let header_cite = Cites::from([Cite::Header]);
    let both = Cites::from([Cite::Svd, Cite::Header]);
    let mut fields = Vec::new();
    let mut names = BTreeMap::new();

    let defined = inputs.rule.filter(|rule| !rule.define_fields.is_empty());
    if let Some(rule) = defined {
        let rule_lines = lines(&rule.sources, entity)?;
        for defined in &rule.define_fields {
            let field_entity = format!("{entity}.{}", defined.name);
            if defined.width == 0 || defined.offset + defined.width > 32 {
                return Err(ImportError::new(
                    &field_entity,
                    "the overlay defines it outside the register",
                ));
            }
            let defined_mask = mask(defined.offset, defined.width);
            let driver = written.contains(defined.name.as_str());
            let holder = h
                .fields
                .iter()
                .find(|f| mask(f.low, f.width) & defined_mask == defined_mask);
            let Some(holder) = holder else {
                return Err(ImportError::new(
                    &field_entity,
                    "the overlay defines it in bits no header field holds: no class",
                ));
            };
            if reserved_description(&holder.description) && !driver {
                report.add(
                    &field_entity,
                    FindingKind::OverlayConflict,
                    format!(
                        "the overlay defines it in {} the header calls reserved, and no driver write names it; not imported",
                        bits(defined.offset, defined.width)
                    ),
                );
                continue;
            }
            let (class, base) =
                field_class(&holder.access, &[&holder.description], driver, &field_entity)?;
            let mut sources = header_cite.clone();
            sources.extend(rule_lines.iter().cloned());
            sources.extend(driver_lines(rule, &defined.name, &field_entity)?);
            names.insert(defined.name.clone(), defined.name.clone());
            fields.push(OutField {
                name: defined.name.clone(),
                low: defined.offset,
                width: defined.width,
                class,
                header_class: base,
                description: Some(defined.description.clone()),
                values: Vec::new(),
                review: Review {
                    sources,
                    derived: true,
                },
            });
        }
        report.add(
            entity,
            FindingKind::OverlayCarried,
            format!(
                "define-fields: {} in place of the header's {} and the SVD's {}",
                rule.define_fields
                    .iter()
                    .map(|f| format!("{}{}", f.name, bits(f.offset, f.width)))
                    .collect::<Vec<_>>()
                    .join(", "),
                h.fields
                    .iter()
                    .map(|f| format!("{}{}", f.name, bits(f.low, f.width)))
                    .collect::<Vec<_>>()
                    .join(", "),
                svd_fields
                    .iter()
                    .map(|f| format!("{}{}", f.name, bits(f.low, f.width)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    } else {
        // A header that declares two views of the same bits: the view the
        // SVD names stands.
        let overlapping = |index: usize| {
            let own = &h.fields[index];
            h.fields.iter().enumerate().any(|(other, f)| {
                other != index
                    && !reserved_description(&f.description)
                    && mask(f.low, f.width) & mask(own.low, own.width) != 0
            })
        };
        let in_svd = |low: u32, width: u32| {
            svd_fields
                .iter()
                .any(|s| s.low == low && s.width == width)
        };
        for (index, field) in h.fields.iter().enumerate() {
            if reserved_description(&field.description) || !overlapping(index) {
                continue;
            }
            if inputs.svd.is_none() {
                return Err(ImportError::new(
                    format!("{entity}.{}", field.name),
                    "the header declares overlapping fields and no SVD names one view",
                ));
            }
            if !in_svd(field.low, field.width) {
                report.add(
                    entity,
                    FindingKind::FieldDropped,
                    format!(
                        "{}:{} also declares {} {}, overlapping fields the SVD names; the SVD's view, not imported",
                        inputs.header_path,
                        field.line,
                        field.name,
                        bits(field.low, field.width)
                    ),
                );
            }
        }
        for (index, field) in h.fields.iter().enumerate() {
            if overlapping(index)
                && !reserved_description(&field.description)
                && !in_svd(field.low, field.width)
            {
                continue;
            }
            let same = svd_fields
                .iter()
                .find(|s| s.low == field.low && s.width == field.width);
            let own = own_name(&field.name, inputs.prefix, array);
            let driver = same.is_some_and(|s| written.contains(s.name.as_str()))
                || written.contains(own.as_str());
            if reserved_description(&field.description) && !driver {
                if let Some(same) = same {
                    report.add(
                        format!("{entity}.{}", same.name),
                        FindingKind::FieldDropped,
                        format!(
                            "{}:{} calls {} reserved; not imported",
                            inputs.header_path,
                            field.line,
                            bits(field.low, field.width)
                        ),
                    );
                }
                continue;
            }
            let matches = |s: &str| {
                if array {
                    same_field(&digitless(&field.name), &digitless(s))
                } else {
                    same_field(&field.name, s)
                }
            };
            let field_name = match same {
                Some(s) if matches(&s.name) && valid_name(&s.name) => s.name.clone(),
                Some(s) => {
                    report.add(
                        format!("{entity}.{}", s.name),
                        FindingKind::FieldRenamed,
                        format!(
                            "{}:{} names {} {}; the header's name {own}",
                            inputs.header_path,
                            field.line,
                            bits(field.low, field.width),
                            field.name
                        ),
                    );
                    own.clone()
                }
                None => {
                    if inputs.svd.is_some() {
                        report.add(
                            format!("{entity}.{own}"),
                            FindingKind::FieldAdded,
                            format!(
                                "{}:{} declares {} {} {}, the SVD has no field at those bits; imported",
                                inputs.header_path,
                                field.line,
                                field.name,
                                field.access,
                                bits(field.low, field.width)
                            ),
                        );
                    }
                    own.clone()
                }
            };
            let field_entity = format!("{entity}.{field_name}");
            let mut descriptions = vec![field.description.as_str()];
            if let Some(description) = same.and_then(|s| s.description.as_deref()) {
                descriptions.push(description);
            }
            let (class, base) = field_class(&field.access, &descriptions, driver, &field_entity)?;
            let mut sources = if same.is_some() {
                both.clone()
            } else {
                header_cite.clone()
            };
            let mut derived = false;
            if driver && let Some(rule) = inputs.rule {
                let name = same.map_or(field_name.as_str(), |s| s.name.as_str());
                sources.extend(driver_lines(rule, name, &field_entity)?);
                derived = true;
                report.add(
                    &field_entity,
                    FindingKind::OverlayCarried,
                    format!(
                        "a vendor driver writes it, the header {}: class {}",
                        if reserved_description(&field.description) {
                            "calls it reserved"
                        } else {
                            "keeps it to debugging"
                        },
                        class.label()
                    ),
                );
            }
            if let Some(s) = same {
                names.insert(s.name.clone(), field_name.clone());
            }
            names.insert(field_name.clone(), field_name.clone());
            let description = same
                .and_then(|s| s.description.clone())
                .or_else(|| prose(&field.description));
            fields.push(OutField {
                name: field_name,
                low: field.low,
                width: field.width,
                class,
                header_class: base,
                description,
                values: Vec::new(),
                review: Review { sources, derived },
            });
        }
        for s in svd_fields {
            let kept = fields.iter().any(|f| f.low == s.low && f.width == s.width);
            let reported = h.fields.iter().any(|f| {
                f.low == s.low && f.width == s.width && reserved_description(&f.description)
            });
            if kept || reported {
                continue;
            }
            let s_mask = mask(s.low, s.width);
            let detail = if s_mask & declared == 0 {
                format!(
                    "{} declares no field at {}; the bits are reserved, not imported",
                    inputs.header_path,
                    bits(s.low, s.width)
                )
            } else {
                let holders = h
                    .fields
                    .iter()
                    .filter(|f| mask(f.low, f.width) & s_mask != 0)
                    .map(|f| format!("{}{}", f.name, bits(f.low, f.width)))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!(
                    "the SVD's {} disagrees with the header's {holders}; the header's fields",
                    bits(s.low, s.width)
                )
            };
            report.add(format!("{entity}.{}", s.name), FindingKind::FieldDropped, detail);
        }
    }
    fields.sort_by_key(|field| field.low);
    let mut taken = 0_u32;
    for field in &fields {
        let field_mask = mask(field.low, field.width);
        if taken & field_mask != 0 {
            return Err(ImportError::new(
                format!("{entity}.{}", field.name),
                "the header's fields overlap",
            ));
        }
        taken |= field_mask;
    }

    // Overlay field entries: values move; access, kind and rename are
    // checked against the header.
    for rule in inputs.field_rules {
        let Some(name) = names.get(&rule.field).cloned() else {
            report.add(
                format!("{entity}.{}", rule.field),
                FindingKind::OverlayUnapplied,
                "the overlay's field entry names no imported field".to_owned(),
            );
            continue;
        };
        let field_entity = format!("{entity}.{name}");
        let Some(field) = fields.iter_mut().find(|f| f.name == name) else {
            continue;
        };
        // `debug-only` refines the header's class: any stated class holds.
        let conflict = match (rule.kind, rule.access) {
            _ if field.class == FieldClass::DebugOnly => None,
            (Some(FieldKind::WriteTrigger), _) if field.class != FieldClass::WriteTrigger => {
                Some("write-trigger")
            }
            (Some(FieldKind::Plain), _) if field.class == FieldClass::WriteTrigger => {
                Some("plain")
            }
            (_, Some(OverlayAccess::ReadOnly)) if field.class.written() => Some("read-only"),
            (_, Some(OverlayAccess::ReadWrite | OverlayAccess::WriteOnly))
                if !field.class.written() =>
            {
                Some("writable")
            }
            _ => None,
        };
        if let Some(stated) = conflict {
            report.add(
                &field_entity,
                FindingKind::OverlayConflict,
                format!(
                    "the overlay states {stated}, the header's class is {}; the header's",
                    field.class.label()
                ),
            );
        }
        if let Some(rename) = &rule.rename
            && *rename != field.name
        {
            report.add(
                &field_entity,
                FindingKind::OverlayConflict,
                format!("the overlay renames it {rename}; the import names it {}", field.name),
            );
        }
        if !rule.values.is_empty() {
            field.values = rule
                .values
                .iter()
                .map(|value| OutValue {
                    name: value.name.clone(),
                    value: value.value,
                    description: value.description.clone(),
                })
                .collect();
            field.review.sources.extend(lines(&rule.sources, &field_entity)?);
            report.add(
                &field_entity,
                FindingKind::OverlayCarried,
                format!("{} named values", rule.values.len()),
            );
        }
    }

    // Reset: the header's defaults over the SVD's value.
    let mut header_value = 0_u32;
    let mut header_mask = 0_u32;
    for field in &h.fields {
        let field_mask = mask(field.low, field.width);
        match field.default {
            Some(default) if default >> field.width == 0 => {
                header_value |= u32::try_from(default).unwrap_or(0) << field.low;
                header_mask |= field_mask;
            }
            Some(default) => report.add(
                entity,
                FindingKind::Default,
                format!(
                    "{}:{} states default {default} for the {}-bit {}; not taken",
                    inputs.header_path, field.line, field.width, field.name
                ),
            ),
            None => {
                if let Some(text) = &field.unread_default {
                    report.add(
                        entity,
                        FindingKind::Default,
                        format!(
                            "{}:{} states default {text:?} for {}; not taken",
                            inputs.header_path, field.line, field.name
                        ),
                    );
                }
            }
        }
    }
    let reset = match inputs.svd.and_then(svd_reset) {
        Some((value, reset_mask)) => {
            let differ = (value ^ header_value) & header_mask & reset_mask;
            if differ != 0 {
                report.add(
                    entity,
                    FindingKind::Reset,
                    format!(
                        "the SVD's reset {value:#010x}, the header's defaults {header_value:#010x} on bits {differ:#010x}; the header's"
                    ),
                );
            }
            let reset_mask = reset_mask | header_mask;
            Some(((value & !header_mask) | header_value, reset_mask))
        }
        None => (header_mask != 0).then_some((header_value, header_mask)),
    };

    let mut fifo = None;
    let mut data_port = None;
    let mut read_side_effect = None;
    if let Some(rule) = inputs.rule {
        let rule_lines = lines(&rule.sources, entity)?;
        if rule.fifo {
            fifo = Some(rule_lines.clone());
            report.add(entity, FindingKind::OverlayCarried, "a FIFO port".to_owned());
        }
        if rule.data_port {
            data_port = Some(rule_lines.clone());
            report.add(entity, FindingKind::OverlayCarried, "a data port".to_owned());
        }
        if rule.read_side_effect == Some(true) {
            read_side_effect = Some(rule_lines.clone());
            report.add(
                entity,
                FindingKind::OverlayCarried,
                "a read changes the hardware".to_owned(),
            );
        }
        if rule.write_one_to_clear
            && fields
                .iter()
                .any(|field| {
                    !matches!(
                        field.class,
                        FieldClass::WriteOneToClear | FieldClass::DebugOnly
                    )
                })
        {
            report.add(
                entity,
                FindingKind::OverlayConflict,
                "the overlay states every field write-one-to-clear, the header does not; the header's classes"
                    .to_owned(),
            );
        }
        if rule.access == Some(OverlayAccess::ReadOnly)
            && !fields.is_empty()
            && fields.iter().all(|field| field.class == FieldClass::Plain)
        {
            report.add(
                entity,
                FindingKind::OverlayConflict,
                "the overlay makes it read-only, the header's fields are all R/W; the header's classes"
                    .to_owned(),
            );
        }
        if let Some(offset) = rule.offset
            && offset != h.offset
        {
            report.add(
                entity,
                FindingKind::OverlayConflict,
                format!(
                    "the overlay places it at {offset:#x}, the header at {:#x}; the header's",
                    h.offset
                ),
            );
        }
    }
    if fifo.is_none()
        && data_port.is_none()
        && read_side_effect.is_none()
        && h.fields
            .iter()
            .any(|field| sequential_port(&field.description))
    {
        read_side_effect = Some(header_cite.clone());
    }
    let taken = fields
        .iter()
        .fold(0_u32, |acc, field| acc | mask(field.low, field.width));
    let reserved = !declared & !taken;
    if let Some(rule) = inputs.rule
        && let Some(stated) = rule.reserved
        && stated & !reserved != 0
    {
        report.add(
            entity,
            FindingKind::OverlayConflict,
            format!(
                "the overlay reserves {stated:#010x}, the header's fields leave {reserved:#010x}; the header's"
            ),
        );
    }
    let svd_register = inputs.svd;
    let description = svd_register
        .and_then(|svd| svd.description.clone())
        .or_else(|| h.description.as_deref().and_then(prose));
    let svd_names = names
        .into_iter()
        .filter(|(svd, _)| svd_fields.iter().any(|s| &s.name == svd))
        .collect();
    Ok(Element {
        register: OutRegister {
            name: inputs.name.clone(),
            offset: h.offset,
            width,
            dim: None,
            description,
            reset,
            reserved,
            fifo,
            data_port,
            read_side_effect,
            fields,
            review: Review {
                sources: if svd_register.is_some() {
                    both
                } else {
                    header_cite
                },
                derived: false,
            },
        },
        array: svd_register.and_then(|svd| svd.array.clone()),
        svd_name: svd_register.map(|svd| svd.name.clone()),
        svd_fields: svd_names,
        from_header: true,
    })
}

/// Fold each SVD array whose elements import identically into one `dim`
/// register; any other array stays element by element. Returns the
/// registers and, per SVD register name, the imported register name and
/// its SVD field names.
#[allow(clippy::type_complexity)]
fn fold(
    instance: &str,
    elements: Vec<Element>,
    report: &mut Report,
) -> (Vec<OutRegister>, BTreeMap<String, (String, BTreeMap<String, String>)>) {
    let mut registers = Vec::new();
    let mut names = BTreeMap::new();
    let mut arrays: BTreeMap<String, Vec<Element>> = BTreeMap::new();
    for element in elements {
        match &element.array {
            None => {
                if let Some(svd) = &element.svd_name {
                    names.insert(
                        svd.clone(),
                        (element.register.name.clone(), element.svd_fields.clone()),
                    );
                }
                registers.push(element.register);
            }
            Some(array) => arrays.entry(array.name.clone()).or_default().push(element),
        }
    }
    for (template, mut members) in arrays {
        members.sort_by_key(|member| member.array.as_ref().map_or(0, |a| a.index));
        let first = members.first().and_then(|m| m.array.clone());
        let foldable = first.as_ref().is_some_and(|first| {
            let base = &members[0].register;
            let strip = |register: &OutRegister| {
                let mut register = register.clone();
                register.name.clear();
                register.offset = 0;
                register.description = None;
                for field in &mut register.fields {
                    field.description = None;
                }
                register
            };
            u32::try_from(members.len()).is_ok_and(|n| n == first.count)
                && first.count >= 2
                && members.iter().enumerate().all(|(index, member)| {
                    let expected = u64::from(base.offset)
                        + index as u64 * u64::from(first.stride);
                    u64::from(member.register.offset) == expected
                        && member.from_header == members[0].from_header
                        && strip(&member.register) == strip(base)
                        && member.svd_fields == members[0].svd_fields
                })
        });
        match (foldable, first) {
            (true, Some(first)) => {
                let mut register = members[0].register.clone();
                register.name = template.clone();
                register.dim = Some((first.count, first.stride));
                register.description = first.description.clone().or(register.description);
                for member in &members {
                    if let Some(svd) = &member.svd_name {
                        names.insert(svd.clone(), (template.clone(), member.svd_fields.clone()));
                    }
                }
                names.insert(
                    template.clone(),
                    (template.clone(), members[0].svd_fields.clone()),
                );
                registers.push(register);
            }
            _ => {
                report.add(
                    format!("{instance}.{template}"),
                    FindingKind::ArrayUnrolled,
                    "its elements do not import identically; one register each".to_owned(),
                );
                for member in members {
                    if let Some(svd) = &member.svd_name {
                        names.insert(
                            svd.clone(),
                            (member.register.name.clone(), member.svd_fields.clone()),
                        );
                    }
                    registers.push(member.register);
                }
            }
        }
    }
    (registers, names)
}

/// The overlay's command registers of the instance.
fn commands(
    inputs: &InstanceInputs<'_>,
    registers: &mut [OutRegister],
    names: &BTreeMap<String, (String, BTreeMap<String, String>)>,
    report: &mut Report,
) -> Result<Vec<OutCommand>, ImportError> {
    let (Some(overlay), Some(peripheral)) = (inputs.overlay, inputs.svd_name) else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for rule in overlay
        .commands
        .iter()
        .filter(|rule| rule.peripheral == peripheral)
    {
        let entity = format!("{}.{}", inputs.name, rule.register);
        let (register_name, field_names) = names.get(&rule.register).ok_or_else(|| {
            ImportError::new(&entity, "the overlay's command register is not imported")
        })?;
        let register = registers
            .iter_mut()
            .find(|register| &register.name == register_name)
            .ok_or_else(|| ImportError::new(&entity, "the command register is not imported"))?;
        let map = |svd: &String| {
            field_names.get(svd).cloned().ok_or_else(|| {
                ImportError::new(
                    format!("{entity}.{svd}"),
                    "the overlay's command field is not imported",
                )
            })
        };
        let commands = rule.fields.iter().map(map).collect::<Result<Vec<_>, _>>()?;
        let qualifiers = rule
            .qualifiers
            .iter()
            .map(map)
            .collect::<Result<Vec<_>, _>>()?;
        let named = commands
            .iter()
            .chain(&qualifiers)
            .collect::<BTreeSet<_>>();
        for name in &named {
            let one_bit = register.fields.iter().any(|field| {
                &&field.name == name && field.width == 1 && field.class == FieldClass::SelfClearing
            });
            if !one_bit {
                return Err(ImportError::new(
                    format!("{entity}.{name}"),
                    "the overlay's command bit is not one self-clearing bit by the header",
                ));
            }
        }
        // A one-bit self-clearing bit the header keeps to debugging is
        // held all the same: never started, zero in every start, and a
        // start refuses while it reads one.
        for field in &mut register.fields {
            if field.width == 1
                && field.class == FieldClass::DebugOnly
                && field.header_class == FieldClass::SelfClearing
                && !named.contains(&field.name)
            {
                field.class = FieldClass::SelfClearing;
                report.add(
                    format!("{entity}.{}", field.name),
                    FindingKind::OverlayCarried,
                    "a held bit of the command register: self-clearing, though the header keeps it to debugging"
                        .to_owned(),
                );
            }
        }
        let held = register
            .fields
            .iter()
            .filter(|field| {
                field.width == 1
                    && field.class == FieldClass::SelfClearing
                    && !named.contains(&field.name)
            })
            .map(|field| field.name.clone())
            .collect();
        out.push(OutCommand {
            register: register.name.clone(),
            commands,
            qualifiers,
            held,
            sources: lines(&rule.sources, &entity)?,
        });
    }
    out.sort_by(|a, b| a.register.cmp(&b.register));
    Ok(out)
}
