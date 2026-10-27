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
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
};

mod initial_pair;
pub use initial_pair::{InitialPairMatch, PairMatchReason};

pub use crate::committed::VENDOR_OBJECT;

/// Summary format.
pub const SCHEMA: u16 = 5;
/// Tolerance file format.
const TOLERANCE_SCHEMA: u16 = 3;
pub(crate) const NO_MODELED_BITS: &str = "the source model declares no compared bits";
/// The reviewed tolerances, compiled in so a run binds the reviews it used.
pub(crate) const TOLERANCES: &str = include_str!("tolerances.toml");

/// The reviews file: every name carries one review, or several whose
/// lifecycle points differ.
#[derive(Clone)]
pub struct ReviewFile {
    fields: BTreeMap<String, Reviews>,
    registers: BTreeMap<String, Reviews>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewDocument {
    schema: u16,
    #[serde(default)]
    fields: BTreeMap<String, Reviews>,
    /// Reviews of image registers by published name; unreviewed registers
    /// follow the vendor-spread rule.
    #[serde(default)]
    registers: BTreeMap<String, Reviews>,
}

#[derive(Clone, Deserialize)]
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
    };
    for register in image {
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
    } else if summary.compared == 0 || !summary.not_read.is_empty() {
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
    /// The compiled reviews checked against the relation and the full current
    /// register catalog, including registers outside the active PHY image.
    pub fn reviewed(root: &Path) -> Result<Self> {
        Self::read(TOLERANCES, &crate::registers::review_names(root)?)
    }

    /// Read only reviews whose names belong to the supplied current model.
    pub fn read(text: &str, register_names: &BTreeSet<String>) -> Result<Self> {
        let reviews: ReviewDocument = toml::from_str(text)?;
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
        if let Some(unknown) = reviews
            .registers
            .keys()
            .find(|name| !register_names.contains(*name))
        {
            return Err(format!("tolerance for unknown register {unknown}").into());
        }
        Ok(Self {
            fields: reviews.fields,
            registers: reviews.registers,
        })
    }
}

#[cfg(test)]
pub(crate) fn fixture_reviews() -> ReviewFile {
    // Algorithm tests use synthetic images and a catalog declaring the fixture's
    // review names. The committed-reference test checks the real model catalog.
    let document: ReviewDocument = toml::from_str(TOLERANCES).unwrap();
    let names = document.registers.keys().cloned().collect();
    ReviewFile::read(TOLERANCES, &names).unwrap()
}

/// Compare the `vendor` and `production` boots captured at `lifecycle`
/// under `reviews`, over the register image `image` and the analog image
/// `analog`. An IEEE 802.15.4 point compares register state only: the
/// reference firmware reports no calibration object.
pub fn compare(
    lifecycle: Lifecycle,
    images: &Images,
    vendor: &[VendorBoot],
    production: &[ProductionBoot],
    reviews: ReviewFile,
    (image, analog_image): (&[Register], &[Register]),
) -> Result<Summary> {
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
    );
    let analog = compare_registers(
        analog_image,
        vendor.iter().map(|b| &b.analog),
        production.iter().map(|b| &b.analog),
        &tolerances.registers,
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
mod tests {
    use super::*;
    use crate::committed::field;

    #[test]
    fn reading_rejects_unknown_register_reviews_before_lifecycle_selection() {
        let names = ["KNOWN.REGISTER".to_owned()].into();
        let valid = "schema=3\n[registers.\"KNOWN.REGISTER\"]\nexcluded=true\nreason='reviewed'\n";
        assert!(ReviewFile::read(valid, &names).is_ok());
        let unknown = valid.replace("KNOWN.REGISTER", "UNKNOWN.REGISTER");
        let error = ReviewFile::read(&unknown, &names).err().unwrap();
        assert!(
            error
                .to_string()
                .contains("unknown register UNKNOWN.REGISTER")
        );
        let inactive = unknown + "lifecycle='ieee802154-restart'\n";
        assert!(ReviewFile::read(&inactive, &names).is_err());
    }

    fn review() -> Tolerance {
        Tolerance {
            reason: String::from("reviewed"),
            ..Tolerance::default()
        }
    }

    #[test]
    fn production_within_the_vendor_spread_matches() {
        let field = field("value", 0, 1, 1, 2);
        // The second element's vendor boots span 2, so every element's
        // range widens by 2.
        let vendor = [vec![10, 20], vec![10, 22]];
        let within = [vec![0, 12, 24]];
        let summary = compare_field(&field, Some(&review()), &vendor, &within).unwrap();
        assert_eq!((summary.verdict, summary.margin), (Verdict::Match, 2));
        let beyond = [vec![0, 13, 20]];
        let summary = compare_field(&field, Some(&review()), &vendor, &beyond).unwrap();
        assert_eq!(summary.verdict, Verdict::Diff);
        let unreviewed = compare_field(&field, None, &vendor, &within).unwrap();
        assert_eq!(unreviewed.verdict, Verdict::Incomplete);
    }

    #[test]
    fn masks_compare_only_modeled_bits() {
        let field = field("status", 0, 0, 1, 1);
        let masked = Tolerance {
            mask: Some(vec![0x80]),
            ..review()
        };
        let summary = compare_field(&field, Some(&masked), &[vec![0xa8]], &[vec![0x80]]).unwrap();
        assert_eq!(summary.verdict, Verdict::Match);
        let whole = compare_field(&field, Some(&review()), &[vec![0xa8]], &[vec![0x80]]).unwrap();
        assert_eq!(whole.verdict, Verdict::Diff);
    }

    #[test]
    fn signed_elements_extend_their_sign() {
        let field = field("temperature", 0, 0, 2, 1);
        assert_eq!(element(&[0xfe, 0xff], 0, &field, 0, true).unwrap(), -2);
        assert_eq!(element(&[0xfe, 0xff], 0, &field, 0, false).unwrap(), 0xfffe);
    }

    #[test]
    fn uncovered_ranges_are_the_gaps_between_fields() {
        let fields = [field("a", 2, 0, 2, 1), field("b", 5, 0, 1, 1)];
        assert_eq!(uncovered(7, &fields), [[0, 2], [4, 5], [6, 7]]);
    }

    #[test]
    fn registers_compare_within_their_own_vendor_spread() {
        let image = |name: &str, address| oer_vendor_phy::calibration_capture::space::Register {
            mask: u32::MAX,
            name: name.into(),
            address,
        };
        let registers = [
            image("A.STABLE", 0x10),
            image("A.VARYING", 0x14),
            image("A.CLOCKED_OFF", 0x18),
            image("A.EXCLUDED", 0x1c),
            image("A.MISSED", 0x20),
        ];
        let vendor_boot = |varying| Readings {
            values: BTreeMap::from([(0x10, 7), (0x14, varying), (0x1c, 1), (0x20, 1)]),
            unreadable: [0x18].into(),
        };
        let production = Readings {
            values: BTreeMap::from([(0x10, 7), (0x14, 13), (0x1c, 9)]),
            unreadable: Default::default(),
        };
        let reviews = BTreeMap::from([(
            "A.EXCLUDED".to_owned(),
            Tolerance {
                excluded: true,
                reason: "environment".into(),
                ..Tolerance::default()
            },
        )]);
        let summary = compare_registers(
            &registers,
            &[vendor_boot(10), vendor_boot(11)],
            [&production],
            &reviews,
        );
        // 13 lies beyond 11 + (11 - 10).
        assert_eq!((summary.compared, summary.matched), (2, 1));
        assert_eq!(summary.differing[0].name, "A.VARYING");
        assert_eq!(summary.unreadable, ["A.CLOCKED_OFF"]);
        assert_eq!(summary.excluded[0].name, "A.EXCLUDED");
        assert_eq!(summary.not_read, ["A.MISSED"]);
        assert_eq!(summary.verdict, Verdict::Diff);
    }

    #[test]
    fn unassigned_lanes_neither_differ_nor_widen_vendor_spread() {
        let image = [Register {
            name: "SPARSE".into(),
            address: 4,
            mask: 0x101,
        }];
        let readings = |value| Readings {
            values: [(4, value)].into(),
            unreadable: Default::default(),
        };
        let vendor = [readings(1), readings(0x8000_0001)];
        let harmless = readings(0x4000_0001);
        let changed = readings(0x101);
        assert_eq!(
            compare_registers(&image, &vendor, [&harmless], &BTreeMap::new()).verdict,
            Verdict::Match
        );
        let diff = compare_registers(&image, &vendor, [&changed], &BTreeMap::new());
        assert_eq!(diff.verdict, Verdict::Diff);
        assert_eq!(diff.differing[0].vendor, [1, 1]);
        assert_eq!(diff.differing[0].production, [0x101, 0x101]);
    }

    #[test]
    fn zero_mask_is_explicitly_excluded_and_still_requires_a_read() {
        let image = [Register {
            name: "EMPTY".into(),
            address: 4,
            mask: 0,
        }];
        let readings = Readings {
            values: [(4, 7)].into(),
            unreadable: Default::default(),
        };
        let summary = compare_registers(&image, [&readings], [&readings], &BTreeMap::new());
        assert_eq!(summary.compared, 0);
        assert_eq!(summary.excluded[0].reason, NO_MODELED_BITS);
        assert_eq!(summary.verdict, Verdict::Incomplete);
        let missing = Readings::default();
        assert_eq!(
            compare_registers(&image, [&readings], [&missing], &BTreeMap::new()).not_read,
            ["EMPTY"]
        );
    }

    fn excluded_register_summary(production: &[Readings]) -> RegisterSummary {
        let image = [
            Register {
                mask: u32::MAX,
                name: "STABLE".into(),
                address: 0x10,
            },
            Register {
                mask: u32::MAX,
                name: "EXCLUDED".into(),
                address: 0x14,
            },
        ];
        let vendor = Readings {
            values: [(0x10, 7), (0x14, 4)].into(),
            unreadable: Default::default(),
        };
        let reviews = BTreeMap::from([(
            "EXCLUDED".into(),
            Tolerance {
                excluded: true,
                reason: "measurement result".into(),
                ..Tolerance::default()
            },
        )]);
        compare_registers(&image, [&vendor], production, &reviews)
    }

    #[test]
    fn excluded_register_without_a_production_read_is_incomplete() {
        let missing = Readings {
            values: [(0x10, 7)].into(),
            unreadable: Default::default(),
        };
        let read = Readings {
            values: [(0x10, 7), (0x14, 4)].into(),
            unreadable: Default::default(),
        };
        for boots in [vec![missing.clone()], vec![read, missing]] {
            let summary = excluded_register_summary(&boots);
            assert_eq!(summary.verdict, Verdict::Incomplete);
            assert_eq!(summary.not_read, ["EXCLUDED"]);
            assert!(summary.excluded.is_empty());
        }
    }

    #[test]
    fn excluded_unreadable_production_register_is_incomplete() {
        let unreadable = Readings {
            values: [(0x10, 7)].into(),
            unreadable: [0x14].into(),
        };
        let read = Readings {
            values: [(0x10, 7), (0x14, 4)].into(),
            unreadable: Default::default(),
        };
        for boots in [vec![unreadable.clone()], vec![read, unreadable]] {
            let summary = excluded_register_summary(&boots);
            assert_eq!(summary.verdict, Verdict::Incomplete);
            assert_eq!(summary.not_read, ["EXCLUDED"]);
            assert!(summary.excluded.is_empty());
        }
    }

    #[test]
    fn excluded_readable_register_values_do_not_affect_match() {
        let boots = [0, 4, u32::MAX].map(|value| Readings {
            values: [(0x10, 7), (0x14, value)].into(),
            unreadable: Default::default(),
        });
        let summary = excluded_register_summary(&boots);
        assert_eq!(summary.verdict, Verdict::Match);
        assert_eq!((summary.compared, summary.matched), (1, 1));
        assert_eq!(summary.excluded[0].name, "EXCLUDED");
        assert!(summary.not_read.is_empty());
        assert!(summary.differing.is_empty());
    }

    #[test]
    fn an_ieee802154_point_compares_register_state_only() {
        let image = [Register {
            mask: u32::MAX,
            name: "A.X".into(),
            address: 0x10,
        }];
        let readings = |value| Readings {
            values: BTreeMap::from([(0x10, value)]),
            unreadable: Default::default(),
        };
        let vendor = [
            VendorBoot {
                registers: readings(4),
                analog: readings(1),
                ..VendorBoot::default()
            },
            VendorBoot {
                registers: readings(6),
                analog: readings(1),
                ..VendorBoot::default()
            },
        ];
        let production = [ProductionBoot {
            calibration: None,
            registers: readings(8),
            analog: readings(1),
        }];
        let images = Images {
            started_unix_seconds: 1_790_467_200,
            production_image: "boot-smoke".into(),
            ..Images::default()
        };
        let summary = compare(
            Lifecycle::Ieee802154,
            &images,
            &vendor,
            &production,
            fixture_reviews(),
            (&image, &image),
        )
        .unwrap();
        assert_eq!(summary.verdict, Verdict::Match);
        assert!(summary.fields.is_empty() && summary.excluded.is_empty());
        assert_eq!(summary.date, "2026-09-27");
        assert_eq!(
            (summary.registers.compared, summary.registers.matched),
            (1, 1)
        );
        // The typed result round-trips through its run-bundle form.
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(serde_json::from_value::<Summary>(json).unwrap(), summary);
        // A cold point needs every boot's calibration.
        assert!(
            compare(
                Lifecycle::Cold,
                &images,
                &vendor,
                &production,
                fixture_reviews(),
                (&image, &image),
            )
            .is_err()
        );
    }

    #[test]
    fn sar2_mismatch_fails_restart_while_the_wifi_cold_point_remains_excluded() {
        let registers = [
            Register {
                mask: u32::MAX,
                name: "analog-i2c.69.04".into(),
                address: 0x10,
            },
            Register {
                mask: u32::MAX,
                name: "A.CONTROL".into(),
                address: 0x14,
            },
        ];
        let readings = |value| Readings {
            values: BTreeMap::from([(0x10, value), (0x14, 7)]),
            unreadable: Default::default(),
        };
        let vendor = [readings(0x60)];
        for lifecycle in [Lifecycle::Restart, Lifecycle::Ieee802154Restart] {
            let reviews = fixture_reviews().at(lifecycle).unwrap();
            let mismatch = readings(0x65);
            let summary = compare_registers(&registers, &vendor, [&mismatch], &reviews.registers);
            assert_eq!(summary.verdict, Verdict::Diff);
            assert_eq!((summary.compared, summary.matched), (2, 1));
            assert_eq!(summary.differing[0].name, "analog-i2c.69.04");
            assert!(summary.excluded.is_empty());

            let matching = readings(0x60);
            let summary = compare_registers(&registers, &vendor, [&matching], &reviews.registers);
            assert_eq!(summary.verdict, Verdict::Match);
            assert_eq!((summary.compared, summary.matched), (2, 2));
        }

        let reviews = fixture_reviews().at(Lifecycle::Cold).unwrap();
        let cold_registration = readings(0x65);
        let summary = compare_registers(
            &registers,
            &vendor,
            [&cold_registration],
            &reviews.registers,
        );
        assert_eq!(summary.verdict, Verdict::Match);
        assert_eq!((summary.compared, summary.matched), (1, 1));
        assert_eq!(summary.excluded[0].name, "analog-i2c.69.04");
    }

    #[test]
    fn dates_are_utc_days() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(1_790_467_200), "2026-09-27");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
    }

    #[test]
    fn the_progress_counter_is_not_compared() {
        assert!(fields().iter().all(|f| f.name != TRACKING_PROGRESS_FIELD));
        assert_eq!(
            fields().len(),
            CALIBRATION.len() + PARENT.len() + committed(0).len() - 1
        );
    }

    #[test]
    fn a_review_scoped_to_a_lifecycle_point_applies_only_there() {
        let text = "schema = 3\n\
             [registers.A]\nexcluded = true\nlifecycle = \"cold\"\nreason = \"cold only\"\n\
             [registers.B]\nexcluded = true\nreason = \"always\"\n\
             [[registers.C]]\nexcluded = true\nlifecycle = \"cold\"\nreason = \"cold\"\n\
             [[registers.C]]\nexcluded = true\nlifecycle = \"restart\"\nreason = \"woken\"\n";
        let reviews =
            || ReviewFile::read(text, &["A".into(), "B".into(), "C".into()].into()).unwrap();
        let restart = reviews().at(Lifecycle::Restart).unwrap();
        assert!(!restart.registers.contains_key("A"));
        assert!(restart.registers.contains_key("B"));
        assert_eq!(restart.registers["C"].reason, "woken");
        assert_eq!(
            reviews().at(Lifecycle::Cold).unwrap().registers["C"].reason,
            "cold"
        );
        let ambiguous =
            "schema = 3\n[[registers.D]]\nreason = \"a\"\n[[registers.D]]\nreason = \"b\"\n";
        assert!(
            ReviewFile::read(ambiguous, &["D".into()].into())
                .unwrap()
                .at(Lifecycle::Cold)
                .is_err()
        );
    }
}

#[cfg(test)]
mod library_tests {
    use super::*;
    #[test]
    fn utc_dates_preserve_day_boundaries_and_the_full_timestamp_domain() {
        for (seconds, expected) in [
            (86_399, "1970-01-01"),
            (86_400, "1970-01-02"),
            (951_782_400, "2000-02-29"),
            (4_107_542_400, "2100-03-01"),
            (253_402_300_800, "10000-01-01"),
            (u64::MAX, "584554051223-11-09"),
        ] {
            assert_eq!(utc_date(seconds), expected);
        }
    }
}
