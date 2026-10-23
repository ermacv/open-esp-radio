//! Comparison of captured vendor and production calibrations under the
//! reviewed `phy_param` relation and reviewed tolerances.
//!
//! Cold calibrations differ from boot to boot, so the vendor's own spread on
//! the same board is the reference: a field's margin is the widest range its
//! vendor boots span in any one element, and each element passes when every
//! production capture lies within that element's vendor range widened by the
//! margin (user decision 2026-09-27). Every compared field carries a review:
//! whether its elements are signed, which bits the relation models, or that
//! it describes the environment rather than calibration and is excluded. A
//! field without a review leaves the comparison INCOMPLETE; reviews naming
//! no compared field are rejected.
//! Reviewed RX-DC gain pairs may also match their own boot's gain0 pair;
//! every use of that rule is retained separately in the field summary.
use crate::Result;
use crate::committed::{
    CALIBRATION, CALIBRATION_BYTES, OutputField, PARENT, PARENT_BYTES, TRACKING_PROGRESS_FIELD,
    committed,
};
use oer_vendor_phy::calibration_capture::boots::{
    Images, Lifecycle, ProductionBoot, Readings, VendorBoot,
};
use oer_vendor_phy::calibration_capture::space::Register;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod control;
mod initial_pair;
pub use control::{ControlRegister, RegisterReference, SameRunControl};
pub use initial_pair::{InitialPairMatch, PairMatchReason};

pub use crate::committed::VENDOR_OBJECT;

/// Summary format.
pub const SCHEMA: u16 = 6;
/// Tolerance file format.
const TOLERANCE_SCHEMA: u16 = 4;
pub(crate) const NO_MODELED_BITS: &str = "the source model declares no compared bits";
/// The reviewed tolerances, compiled in so a run binds the reviews it used.
pub(crate) const TOLERANCES: &str = include_str!("tolerances.toml");

/// The reviews file: every name carries one review, or several whose
/// lifecycle points differ.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewFile {
    schema: u16,
    #[serde(default)]
    fields: BTreeMap<String, Reviews>,
    /// Reviews of image registers by published name; unreviewed registers
    /// follow the vendor-spread rule.
    #[serde(default)]
    registers: BTreeMap<String, Reviews>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Reviews {
    One(Tolerance),
    Many(Vec<Tolerance>),
}

/// The reviews that apply at one lifecycle point.
pub(crate) struct Tolerances {
    pub fields: BTreeMap<String, Tolerance>,
    pub registers: BTreeMap<String, Tolerance>,
}

#[derive(Clone, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Tolerance {
    /// Whether the field's elements are two's-complement values.
    #[serde(default)]
    pub signed: bool,
    /// Per element, the bits the relation models; others are not compared.
    #[serde(default)]
    pub mask: Option<Vec<u64>>,
    /// Exclude the value from comparison. Registers still require a readable
    /// production sample in every boot where the vendor can read them.
    #[serde(default)]
    pub excluded: bool,
    /// Accept a reviewed RX-DC gain pair equal to this boot's gain0 pair.
    #[serde(default, rename = "initial-pair")]
    initial_pair: Option<initial_pair::Rule>,
    /// Select primary vendor bounds from the same complete alternating run.
    #[serde(default)]
    pub(crate) reference: Option<RegisterReference>,
    /// The only lifecycle point the review applies to; every point when
    /// absent.
    #[serde(default)]
    lifecycle: Option<Lifecycle>,
    pub reason: String,
}

impl ReviewFile {
    /// The reviews that apply at `lifecycle`: a review without a point, or
    /// the one naming it. Two reviews of one name applying at one point fail.
    pub(crate) fn at(self, lifecycle: Lifecycle) -> Result<Tolerances> {
        if self.schema != TOLERANCE_SCHEMA {
            return Err(format!("unsupported tolerance schema {}", self.schema).into());
        }
        let select = |reviews: BTreeMap<String, Reviews>| -> Result<BTreeMap<String, Tolerance>> {
            let mut applied = BTreeMap::new();
            for (name, reviews) in reviews {
                let reviews = match reviews {
                    Reviews::One(review) => vec![review],
                    Reviews::Many(reviews) => reviews,
                };
                let mut applying = reviews
                    .into_iter()
                    .filter(|review| review.lifecycle.is_none_or(|point| point == lifecycle));
                if let Some(review) = applying.next() {
                    if applying.next().is_some() {
                        return Err(
                            format!("{name} has two reviews at the {lifecycle:?} point").into()
                        );
                    }
                    applied.insert(name, review);
                }
            }
            Ok(applied)
        };
        let tolerances = Tolerances {
            fields: select(self.fields)?,
            registers: select(self.registers)?,
        };
        for (name, review) in &tolerances.fields {
            if review.reference.is_some() {
                return Err(format!("register reference rule applied to field {name}").into());
            }
            if let Some(rule) = review.initial_pair {
                let field = fields()
                    .into_iter()
                    .find(|f| f.name == name)
                    .ok_or_else(|| format!("initial-pair rule names unknown field {name}"))?;
                rule.gains(&field, review)?;
            }
        }
        if tolerances
            .registers
            .values()
            .any(|r| r.initial_pair.is_some())
        {
            return Err("initial-pair rules apply only to RX-DC fields".into());
        }
        for (name, review) in &tolerances.registers {
            if review.reference.is_some()
                && (review.excluded
                    || review.signed
                    || review.mask.is_some()
                    || review.reason.trim().is_empty())
            {
                return Err(format!(
                    "same-run control requires unchanged register masks and a reason: {name}"
                )
                .into());
            }
        }
        Ok(tolerances)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Verdict {
    Match,
    Diff,
    Incomplete,
}

/// The cross-check's verdict and everything it rests on: the typed result
/// the workload records in the run bundle.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Summary {
    pub schema: u16,
    /// UTC day the captures started.
    pub date: String,
    pub lifecycle: Lifecycle,
    pub verdict: Verdict,
    pub vendor_object: String,
    pub images: Images,
    pub vendor_captures: usize,
    pub production_captures: usize,
    pub fields: Vec<FieldSummary>,
    /// Relation fields that are not compared, with their reasons.
    pub excluded: Vec<Excluded>,
    /// Vendor object byte ranges no compared field covers, `[start, end)`.
    pub uncovered: Vec<[usize; 2]>,
    /// The calibrated radio-PHY register image of both sides.
    pub registers: RegisterSummary,
    /// The calibrated analog image of both sides.
    pub analog: RegisterSummary,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterSummary {
    pub verdict: Verdict,
    /// Registers both sides read and the rule compared.
    pub compared: usize,
    pub matched: usize,
    /// Registers outside their vendor range widened by their own vendor
    /// spread.
    pub differing: Vec<RegisterDifference>,
    /// Reviewed registers left out, with their reasons.
    pub excluded: Vec<ExcludedRegister>,
    /// Registers whose read reset the chip in the calibrated state.
    pub unreadable: Vec<String>,
    /// Vendor-readable registers production did not report.
    pub not_read: Vec<String>,
    /// Explicit results for condition-dependent registers, including missing CONTROL.
    pub same_run_control: Vec<ControlRegister>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct RegisterDifference {
    pub name: String,
    pub vendor: [u32; 2],
    pub production: [u32; 2],
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct ExcludedRegister {
    pub name: String,
    pub reason: String,
}

/// Register values by address, and the addresses whose read reset the chip.
type RegisterValues = (BTreeMap<u32, Vec<u32>>, std::collections::BTreeSet<u32>);

/// Every register value of `boots` by address, and the unreadable ones.
fn register_values<'a>(boots: impl IntoIterator<Item = &'a Readings>) -> RegisterValues {
    let mut values: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    let mut unreadable = std::collections::BTreeSet::new();
    for readings in boots {
        for (address, value) in &readings.values {
            values.entry(*address).or_default().push(*value);
        }
        unreadable.extend(readings.unreadable.iter().copied());
    }
    (values, unreadable)
}

/// Compare the production register image with the vendor boots'. A
/// register passes when every production value lies within the vendor
/// range widened by the vendor range's own width, the fields' rule applied
/// to the declared bits of each register. Mask every sample before deriving
/// the range: unassigned variation must not widen the vendor margin.
pub(crate) fn compare_registers<'a>(
    image: &[Register],
    vendor: impl IntoIterator<Item = &'a Readings>,
    production: impl IntoIterator<Item = &'a Readings>,
    reviews: &BTreeMap<String, Tolerance>,
    control: Option<&[&Readings]>,
) -> RegisterSummary {
    let (vendor_values, unreadable) = register_values(vendor);
    let production: Vec<_> = production.into_iter().collect();
    let (production_values, _) = register_values(production.iter().copied());
    let range = |values: &[u32], mask: u32| {
        values.iter().fold([u32::MAX, u32::MIN], |[low, high], &v| {
            let v = v & mask;
            [low.min(v), high.max(v)]
        })
    };
    let mut summary = RegisterSummary {
        verdict: Verdict::Match,
        compared: 0,
        matched: 0,
        differing: vec![],
        excluded: vec![],
        unreadable: vec![],
        not_read: vec![],
        same_run_control: vec![],
    };
    for register in image {
        if let Some(review) = reviews
            .get(&register.name)
            .filter(|r| r.reference.is_some())
        {
            let checked = control::compare(register, &review.reason, control, &production);
            if checked.production.is_none() {
                summary.not_read.push(register.name.clone());
            }
            match checked.verdict {
                Verdict::Match => {
                    summary.compared += 1;
                    summary.matched += 1;
                }
                Verdict::Diff => {
                    summary.compared += 1;
                    if let (Some(vendor), Some(production)) = (checked.vendor, checked.production) {
                        summary.differing.push(RegisterDifference {
                            name: register.name.clone(),
                            vendor,
                            production,
                        });
                    }
                }
                Verdict::Incomplete => {}
            }
            summary.same_run_control.push(checked);
            continue;
        }
        if unreadable.contains(&register.address) {
            summary.unreadable.push(register.name.clone());
            continue;
        }
        let Some(vendor) = vendor_values.get(&register.address) else {
            continue;
        };
        let Some(values) = production_values.get(&register.address) else {
            summary.not_read.push(register.name.clone());
            continue;
        };
        if production.iter().any(|boot| {
            !boot.values.contains_key(&register.address)
                || boot.unreadable.contains(&register.address)
        }) {
            summary.not_read.push(register.name.clone());
            continue;
        }
        if register.mask == 0 {
            summary.excluded.push(ExcludedRegister {
                name: register.name.clone(),
                reason: NO_MODELED_BITS.into(),
            });
            continue;
        }
        if let Some(review) = reviews.get(&register.name).filter(|r| r.excluded) {
            summary.excluded.push(ExcludedRegister {
                name: register.name.clone(),
                reason: review.reason.clone(),
            });
            continue;
        }
        let (v, p) = (range(vendor, register.mask), range(values, register.mask));
        let margin = u64::from(v[1] - v[0]);
        summary.compared += 1;
        if u64::from(v[0]) <= u64::from(p[0]) + margin
            && u64::from(p[1]) <= u64::from(v[1]) + margin
        {
            summary.matched += 1;
        } else {
            summary.differing.push(RegisterDifference {
                name: register.name.clone(),
                vendor: v,
                production: p,
            });
        }
    }
    summary.verdict = if !summary.differing.is_empty() {
        Verdict::Diff
    } else if summary.compared == 0
        || !summary.not_read.is_empty()
        || summary
            .same_run_control
            .iter()
            .any(|r| r.verdict == Verdict::Incomplete)
    {
        Verdict::Incomplete
    } else {
        Verdict::Match
    };
    summary
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Excluded {
    pub name: String,
    pub reason: String,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct FieldSummary {
    pub name: String,
    pub verdict: Verdict,
    /// The widest vendor range of any element.
    pub margin: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Vec<u64>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Per element, the vendor and production `[min, max]`.
    pub vendor: Vec<[i64; 2]>,
    pub production: Vec<[i64; 2]>,
    /// Out-of-range gain pairs accepted by their same-boot initial pair.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_pairs: Vec<InitialPairMatch>,
}

/// The fields the cross-check compares: the parent root's relation without
/// its tracking progress, which counts tracking work rather than
/// calibration.
pub(crate) fn fields() -> Vec<OutputField> {
    CALIBRATION
        .into_iter()
        .chain(PARENT)
        .chain(committed(CALIBRATION_BYTES + PARENT_BYTES))
        .filter(|f| f.name != TRACKING_PROGRESS_FIELD)
        .collect()
}

/// Element `index` of `field` in `bytes` at `offset`.
fn element(
    bytes: &[u8],
    offset: u32,
    field: &OutputField,
    index: u32,
    signed: bool,
) -> Result<i64> {
    let width = usize::from(field.width);
    let start = offset as usize + index as usize * width;
    let slice = bytes
        .get(start..start + width)
        .ok_or_else(|| format!("{} lies outside the captured bytes", field.name))?;
    let mut raw = [0; 8];
    raw[..width].copy_from_slice(slice);
    let value = u64::from_le_bytes(raw);
    let bits = 8 * width as u32;
    Ok(if signed && value >> (bits - 1) & 1 == 1 {
        value as i64 - (1_i64 << bits)
    } else {
        value as i64
    })
}

fn range(values: impl Iterator<Item = i64>) -> [i64; 2] {
    values.fold([i64::MAX, i64::MIN], |[low, high], v| {
        [low.min(v), high.max(v)]
    })
}

fn compare_field(
    field: &OutputField,
    tolerance: Option<&Tolerance>,
    vendor: &[Vec<u8>],
    production: &[Vec<u8>],
) -> Result<FieldSummary> {
    let vendor_ranges = field_ranges(field, tolerance, vendor, field.parameter)?;
    compare_field_captures(field, tolerance, vendor_ranges, production)
}

/// Apply live or recorded vendor bounds to individual production boots.
pub(crate) fn compare_field_captures(
    field: &OutputField,
    tolerance: Option<&Tolerance>,
    vendor_ranges: Vec<[i64; 2]>,
    production: &[Vec<u8>],
) -> Result<FieldSummary> {
    let production_ranges = field_ranges(field, tolerance, production, field.output)?;
    let mut summary = compare_field_ranges(field.name, tolerance, vendor_ranges, production_ranges);
    if let Some(review) = tolerance.filter(|r| r.initial_pair.is_some()) {
        initial_pair::compare(field, review, production, &mut summary)?;
    }
    Ok(summary)
}

pub(crate) fn field_ranges(
    field: &OutputField,
    tolerance: Option<&Tolerance>,
    captures: &[Vec<u8>],
    offset: u32,
) -> Result<Vec<[i64; 2]>> {
    let signed = tolerance.is_some_and(|t| t.signed);
    let mask = tolerance.and_then(|t| t.mask.clone());
    if mask
        .as_ref()
        .is_some_and(|m| m.len() != field.count as usize)
    {
        return Err(format!("the mask of {} does not cover its elements", field.name).into());
    }
    let values = |captures: &[Vec<u8>], offset: u32, index: u32| -> Result<[i64; 2]> {
        let bits = mask.as_ref().map(|m| m[index as usize] as i64);
        Ok(range(
            captures
                .iter()
                .map(|bytes| {
                    element(bytes, offset, field, index, signed).map(|v| bits.map_or(v, |b| v & b))
                })
                .collect::<Result<Vec<_>>>()?
                .into_iter(),
        ))
    };
    let mut ranges = vec![];
    for index in 0..field.count {
        ranges.push(values(captures, offset, index)?);
    }
    Ok(ranges)
}

fn compare_field_ranges(
    name: &str,
    tolerance: Option<&Tolerance>,
    vendor_ranges: Vec<[i64; 2]>,
    production_ranges: Vec<[i64; 2]>,
) -> FieldSummary {
    let margin = vendor_ranges
        .iter()
        .map(|[low, high]| high - low)
        .max()
        .unwrap_or(0);
    let within = vendor_ranges
        .iter()
        .zip(&production_ranges)
        .all(|(v, p)| v[0] - margin <= p[0] && p[1] <= v[1] + margin);
    FieldSummary {
        name: name.to_owned(),
        verdict: match (tolerance, within) {
            (None, _) => Verdict::Incomplete,
            (Some(_), true) => Verdict::Match,
            (Some(_), false) => Verdict::Diff,
        },
        margin,
        mask: tolerance.and_then(|t| t.mask.clone()),
        reason: tolerance.map(|t| t.reason.clone()),
        vendor: vendor_ranges,
        production: production_ranges,
        initial_pairs: vec![],
    }
}

/// Vendor byte ranges of an object of `length` bytes that no field covers.
pub(crate) fn uncovered(length: usize, fields: &[OutputField]) -> Vec<[usize; 2]> {
    let mut covered = vec![false; length];
    for field in fields {
        let start = field.parameter as usize;
        let end = (start + usize::from(field.width) * field.count as usize).min(length);
        covered[start.min(end)..end].fill(true);
    }
    let mut ranges: Vec<[usize; 2]> = vec![];
    for (offset, covered) in covered.into_iter().enumerate() {
        if covered {
            continue;
        }
        match ranges.last_mut() {
            Some(last) if last[1] == offset => last[1] += 1,
            _ => ranges.push([offset, offset + 1]),
        }
    }
    ranges
}

/// `YYYY-MM-DD` of the UTC day holding `unix_seconds`.
pub(crate) fn utc_date(unix_seconds: u64) -> String {
    let seconds = unix_seconds;
    // Gregorian dates repeat every 400 years. Reduce into jiff's supported
    // range without narrowing the existing u64 timestamp domain.
    const CYCLE_SECONDS: u64 = 146_097 * 86_400;
    let cycles = seconds / CYCLE_SECONDS;
    let within_cycle = (seconds % CYCLE_SECONDS) as i64;
    let date = jiff::Timestamp::from_second(within_cycle)
        .expect("a 400-year cycle from 1970 fits jiff's timestamp range")
        .to_zoned(jiff::tz::TimeZone::UTC);
    let year = date.year() as u64 + cycles * 400;
    format!("{year:04}-{:02}-{:02}", date.month(), date.day())
}

impl ReviewFile {
    /// The reviewed tolerances this library was built with, checked
    /// against the relation: a review naming no compared field fails.
    pub fn reviewed() -> Result<Self> {
        let reviews: Self = toml::from_str(TOLERANCES)?;
        if reviews.schema != TOLERANCE_SCHEMA {
            return Err(format!("unsupported tolerance schema {}", reviews.schema).into());
        }
        let fields = fields();
        if let Some(unknown) = reviews
            .fields
            .keys()
            .find(|name| !fields.iter().any(|f| f.name == name.as_str()))
        {
            return Err(
                format!("tolerance for {unknown}, which the relation does not compare").into(),
            );
        }
        Ok(reviews)
    }
}

/// Compare fixed `vendor` bounds and `production` boots at `lifecycle`.
/// Reviewed condition-dependent bytes require explicit same-run `control`;
/// a reference series never implicitly supplies it, even when counts agree.
/// Apply `reviews` over the register image `image` and the analog image
/// `analog`. An IEEE 802.15.4 point compares register state only: the
/// reference firmware reports no calibration object.
pub fn compare(
    lifecycle: Lifecycle,
    images: &Images,
    vendor: &[VendorBoot],
    production: &[ProductionBoot],
    control: Option<&SameRunControl>,
    reviews: ReviewFile,
    (image, analog_image): (&[Register], &[Register]),
) -> Result<Summary> {
    if control.is_some_and(|c| !c.belongs_to(images, production.len())) {
        return Err("CONTROL belongs to a different capture identity or series length".into());
    }
    let tolerances = reviews.at(lifecycle)?;
    let calibrated = !lifecycle.ieee802154();
    if vendor.is_empty() || production.is_empty() {
        return Err("the captures hold no vendor or no production boot".into());
    }
    let objects = |sides: Vec<Option<&Vec<u8>>>, side: &str| -> Result<Vec<Vec<u8>>> {
        sides
            .into_iter()
            .map(|bytes| {
                bytes
                    .cloned()
                    .ok_or_else(|| format!("a {side} boot carries no calibration").into())
            })
            .collect()
    };
    let (vendor_objects, production_outputs) = if calibrated {
        (
            objects(
                vendor.iter().map(|b| b.calibration.as_ref()).collect(),
                "vendor",
            )?,
            objects(
                production.iter().map(|b| b.calibration.as_ref()).collect(),
                "production",
            )?,
        )
    } else {
        (vec![], vec![])
    };
    let registers = compare_registers(
        image,
        vendor.iter().map(|b| &b.registers),
        production.iter().map(|b| &b.registers),
        &tolerances.registers,
        control
            .map(|c| c.boots.iter().map(|b| &b.registers).collect::<Vec<_>>())
            .as_deref(),
    );
    let analog = compare_registers(
        analog_image,
        vendor.iter().map(|b| &b.analog),
        production.iter().map(|b| &b.analog),
        &tolerances.registers,
        control
            .map(|c| c.boots.iter().map(|b| &b.analog).collect::<Vec<_>>())
            .as_deref(),
    );
    let fields = if calibrated { fields() } else { vec![] };
    let length = vendor_objects.first().map_or(0, Vec::len);
    if vendor_objects.iter().any(|bytes| bytes.len() != length) {
        return Err(format!("{VENDOR_OBJECT} captures differ in length").into());
    }
    let excluded = fields
        .iter()
        .filter_map(|field| {
            let review = tolerances.fields.get(field.name).filter(|t| t.excluded)?;
            Some(Excluded {
                name: field.name.to_owned(),
                reason: review.reason.clone(),
            })
        })
        .chain(calibrated.then(|| Excluded {
            name: TRACKING_PROGRESS_FIELD.to_owned(),
            reason: "counts tracking work, not calibration".into(),
        }))
        .collect::<Vec<_>>();
    let summaries = fields
        .iter()
        .filter(|field| !excluded.iter().any(|e| e.name == field.name))
        .map(|field| {
            compare_field(
                field,
                tolerances.fields.get(field.name),
                &vendor_objects,
                &production_outputs,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let verdicts = || {
        summaries
            .iter()
            .map(|f| f.verdict)
            .chain([registers.verdict, analog.verdict])
    };
    let verdict = if verdicts().any(|v| v == Verdict::Diff) {
        Verdict::Diff
    } else if verdicts().any(|v| v == Verdict::Incomplete) {
        Verdict::Incomplete
    } else {
        Verdict::Match
    };
    Ok(Summary {
        schema: SCHEMA,
        date: utc_date(images.started_unix_seconds),
        lifecycle,
        verdict,
        vendor_object: VENDOR_OBJECT.to_owned(),
        images: images.clone(),
        vendor_captures: vendor.len(),
        production_captures: production.len(),
        fields: summaries,
        excluded,
        uncovered: uncovered(length, &fields),
        registers,
        analog,
    })
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod library_tests;
