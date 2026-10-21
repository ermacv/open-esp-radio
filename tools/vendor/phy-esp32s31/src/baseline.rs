//! A generated reference of vendor bounds, with the live comparator's rules.
use crate::{
    Result, committed,
    compare::{self, Verdict},
    family::Esp32s31,
};
use oer_vendor_phy::{
    calibration_capture::boots::{Images, Lifecycle, ProductionBoot, Readings, VendorBoot},
    comparison::{Comparison, RegisterImages},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

pub mod runs;
#[cfg(test)]
mod tests;

pub const SCHEMA: u16 = 1;
const CHIP: &str = "esp32s31";
const SCENARIO: &str = "phy-vendor-calibration-cold";

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Signature {
    pub idf_revision: String,
    pub scenario_sha256: String,
    pub model_sha256: String,
    pub relation_sha256: String,
    pub review_sha256: String,
    pub recipe_sha256: String,
}
fn digest(root: &Path, files: &[&str]) -> Result<String> {
    let mut bytes = Vec::new();
    for file in files {
        let data = std::fs::read(root.join(file))?;
        bytes.extend_from_slice(file.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&(data.len() as u64).to_le_bytes());
        bytes.extend(data);
    }
    Ok(oer_os_durable::sha256_bytes(&bytes))
}
impl Signature {
    /// Bind interpretation and capture recipe without fetching or building vendor code.
    pub fn current(root: &Path) -> Result<Self> {
        let pins: toml::Value = toml::from_str(&std::fs::read_to_string(
            root.join("chips/esp32s31/vendor/artifacts.toml"),
        )?)?;
        let revision = pins
            .get("source")
            .and_then(toml::Value::as_array)
            .and_then(|sources| {
                sources
                    .iter()
                    .find(|s| s.get("id").and_then(toml::Value::as_str) == Some("esp-idf"))
            })
            .and_then(|s| s.get("revision").and_then(toml::Value::as_str))
            .ok_or("no IDF pin")?;
        // The comparator uses compiled reviews and relations, so stale executables
        // must not sign a newer checkout's files.
        for (file, compiled) in [
            (
                "tools/vendor/phy-esp32s31/src/tolerances.toml",
                compare::TOLERANCES.as_bytes(),
            ),
            (
                "chips/esp32s31/knowledge/src/phy_relation.rs",
                include_bytes!("../../../../chips/esp32s31/knowledge/src/phy_relation.rs")
                    .as_slice(),
            ),
            (
                "chips/esp32s31/knowledge/src/phy_relation/committed.rs",
                include_bytes!(
                    "../../../../chips/esp32s31/knowledge/src/phy_relation/committed.rs"
                )
                .as_slice(),
            ),
        ] {
            if std::fs::read(root.join(file))? != compiled {
                return Err(format!("stale comparison executable: {file}").into());
            }
        }
        Ok(Self {
            idf_revision: revision.into(),
            scenario_sha256: digest(
                root,
                &["tools/hil/scenarios/phy/phy-vendor-calibration-cold.toml"],
            )?,
            model_sha256: digest(
                root,
                &[
                    "chips/esp32s31/registers/published/radio.svd",
                    "chips/esp32s31/registers/policy/api.toml",
                ],
            )?,
            relation_sha256: digest(
                root,
                &[
                    "chips/esp32s31/knowledge/src/phy_relation.rs",
                    "chips/esp32s31/knowledge/src/phy_relation/committed.rs",
                ],
            )?,
            review_sha256: oer_os_durable::sha256_bytes(compare::TOLERANCES.as_bytes()),
            recipe_sha256: digest(
                root,
                &[
                    "chips/esp32s31/vendor/artifacts.toml",
                    "firmware/vendor/esp32s31/calibration/main/main.c",
                    "firmware/vendor/esp32s31/calibration/sdkconfig.defaults",
                    "firmware/vendor/esp32s31/calibration/CMakeLists.txt",
                    "firmware/vendor/esp32s31/calibration/firmware.toml",
                    "firmware/vendor/esp32s31/calibration/main/CMakeLists.txt",
                    "firmware/vendor/esp32s31/calibration/main/reported.h.in",
                    "tools/vendor/phy/src/calibration_capture.rs",
                    "tools/vendor/phy/src/calibration_capture/boots.rs",
                    "tools/vendor/phy/src/calibration_capture/vendor.rs",
                    "tools/vendor/phy/src/calibration_capture/space.rs",
                    "tools/vendor/phy/src/workload/calibration.rs",
                    "tools/vendor/phy-esp32s31/src/compare.rs",
                    "tools/vendor/phy-esp32s31/src/compare/initial_pair.rs",
                ],
            )?,
        })
    }
    pub fn path(&self, root: &Path) -> PathBuf {
        root.join("chips/esp32s31/vendor/phy-baselines")
            .join(&self.idf_revision)
            .join("wifi-cold.json")
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Source {
    pub run_id: String,
    pub scenario: String,
    pub repetition: u32,
    pub repository_commit: String,
    pub repository_dirty: bool,
    pub date: String,
    pub samples: usize,
    pub observations_sha256: String,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Board {
    pub revision: u16,
    pub samples: usize,
    pub sources: Vec<Source>,
}
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Space {
    Field,
    Mmio,
    Analog,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(tag = "status", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Sample {
    Read { ranges: Vec<[i64; 2]>, margin: i64 },
    Unreadable,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Entry {
    pub space: Space,
    pub width: u8,
    pub count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<u32>,
    pub signed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mask: Option<Vec<u64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub excluded: Option<String>,
    /// Present only for one element constant across every recorded board/boot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shared: Option<i64>,
    pub boards: BTreeMap<String, Sample>,
}
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub struct Reference {
    pub schema: u16,
    pub generated_by: String,
    pub chip: String,
    pub lifecycle: Lifecycle,
    pub scenario: String,
    pub signature: Signature,
    pub vendor_application_sha256: String,
    pub channel: u8,
    pub vendor_object_bytes: usize,
    pub boards: BTreeMap<String, Board>,
    pub entries: BTreeMap<String, Entry>,
}
impl Entry {
    fn for_board(&self, board: &str) -> Option<Sample> {
        self.shared
            .map(|v| sample(vec![[v, v]]))
            .or_else(|| self.boards.get(board).cloned())
    }
}

/// Verified primary boots of one run; discovery never enters this input.
pub struct Input {
    pub label: String,
    pub revision: u16,
    pub source: Source,
    pub identity: Images,
    pub channel: u8,
    pub boots: Vec<VendorBoot>,
}
fn label(label: &str) -> bool {
    label.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && label
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}
fn sample(ranges: Vec<[i64; 2]>) -> Sample {
    let margin = ranges.iter().map(|[a, b]| b - a).max().unwrap_or(0);
    Sample::Read { ranges, margin }
}
fn complete(
    readings: &Readings,
    image: &[oer_vendor_phy::calibration_capture::space::Register],
    allow_unreadable: bool,
) -> Result<()> {
    let addresses: BTreeSet<_> = image.iter().map(|r| r.address).collect();
    let read: BTreeSet<_> = readings.values.keys().copied().collect();
    if !read.is_disjoint(&readings.unreadable)
        || read
            .union(&readings.unreadable)
            .copied()
            .collect::<BTreeSet<_>>()
            != addresses
        || (!allow_unreadable && !readings.unreadable.is_empty())
    {
        return Err("missing, unknown or inconsistent register coverage".into());
    }
    Ok(())
}
fn metadata(lifecycle: Lifecycle, images: &RegisterImages) -> Result<BTreeMap<String, Entry>> {
    let reviews = compare::ReviewFile::reviewed()?.at(lifecycle)?;
    let mut entries = BTreeMap::new();
    for f in compare::fields() {
        let r = reviews
            .fields
            .get(f.name)
            .ok_or_else(|| format!("unreviewed field {}", f.name))?;
        entries.insert(
            f.name.into(),
            Entry {
                space: Space::Field,
                width: f.width,
                count: f.count,
                address: None,
                signed: r.signed,
                mask: r.mask.clone(),
                excluded: r.excluded.then(|| r.reason.clone()),
                shared: None,
                boards: BTreeMap::new(),
            },
        );
    }
    for (space, image, width) in [
        (Space::Mmio, &images.registers, 4),
        (Space::Analog, &images.analog, 1),
    ] {
        for reg in image {
            if entries
                .insert(
                    reg.name.clone(),
                    Entry {
                        space,
                        width,
                        count: 1,
                        address: Some(reg.address),
                        signed: false,
                        mask: Some(vec![u64::from(reg.mask)]),
                        excluded: if reg.mask == 0 {
                            Some(compare::NO_MODELED_BITS.into())
                        } else {
                            reviews
                                .registers
                                .get(&reg.name)
                                .filter(|r| r.excluded)
                                .map(|r| r.reason.clone())
                        },
                        shared: None,
                        boards: BTreeMap::new(),
                    },
                )
                .is_some()
            {
                return Err("duplicate reference identity".into());
            }
        }
    }
    Ok(entries)
}

pub fn generate(
    signature: Signature,
    inputs: Vec<Input>,
    images: &RegisterImages,
) -> Result<Reference> {
    let first = inputs.first().ok_or("no primary vendor captures")?;
    let mut reference = Reference {
        schema: SCHEMA,
        generated_by: "cargo vendor-check phy-baseline generate".into(),
        chip: CHIP.into(),
        lifecycle: Lifecycle::Cold,
        scenario: SCENARIO.into(),
        signature,
        vendor_application_sha256: first.identity.vendor_application_sha256.clone(),
        channel: first.channel,
        vendor_object_bytes: first
            .boots
            .first()
            .and_then(|b| b.calibration.as_ref())
            .ok_or("missing vendor calibration")?
            .len(),
        boards: BTreeMap::new(),
        entries: metadata(Lifecycle::Cold, images)?,
    };
    let mut boots: BTreeMap<String, Vec<VendorBoot>> = BTreeMap::new();
    let mut source_ids = BTreeSet::new();
    for input in inputs {
        if !label(&input.label)
            || input.identity.vendor_idf_revision != reference.signature.idf_revision
            || input.identity.vendor_application_sha256 != reference.vendor_application_sha256
            || input.identity.vendor_project != "calibration"
            || input.channel != reference.channel
            || input.source.samples != input.boots.len()
            || input.boots.is_empty()
            || !source_ids.insert((
                input.source.run_id.clone(),
                input.source.scenario.clone(),
                input.source.repetition,
            ))
        {
            return Err("inconsistent vendor source, label, channel or image identity".into());
        }
        let board = reference
            .boards
            .entry(input.label.clone())
            .or_insert(Board {
                revision: input.revision,
                samples: 0,
                sources: vec![],
            });
        if board.revision != input.revision {
            return Err("board revision changed".into());
        }
        board.samples += input.boots.len();
        board.sources.push(input.source);
        for boot in &input.boots {
            complete(&boot.registers, &images.registers, true)?;
            complete(&boot.analog, &images.analog, false)?;
            if boot
                .calibration
                .as_ref()
                .is_none_or(|b| b.len() != reference.vendor_object_bytes)
            {
                return Err("vendor object length changed".into());
            }
        }
        boots.entry(input.label).or_default().extend(input.boots);
    }
    let reviews = compare::ReviewFile::reviewed()?.at(Lifecycle::Cold)?;
    for (name, entry) in &mut reference.entries {
        for (label, boots) in &boots {
            let state = match entry.space {
                Space::Field => {
                    let f = compare::fields()
                        .into_iter()
                        .find(|f| f.name == name)
                        .ok_or("missing relation field")?;
                    let objects = boots
                        .iter()
                        .map(|b| b.calibration.clone().ok_or("missing calibration"))
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    sample(compare::field_ranges(
                        &f,
                        reviews.fields.get(name),
                        &objects,
                        f.parameter,
                    )?)
                }
                Space::Mmio | Space::Analog => {
                    let address = entry.address.ok_or("missing register address")?;
                    let reads = boots
                        .iter()
                        .map(|b| {
                            if entry.space == Space::Mmio {
                                &b.registers
                            } else {
                                &b.analog
                            }
                        })
                        .collect::<Vec<_>>();
                    let unreadable = reads
                        .first()
                        .ok_or("no primary reads")?
                        .unreadable
                        .contains(&address);
                    if reads
                        .iter()
                        .any(|r| r.unreadable.contains(&address) != unreadable)
                    {
                        return Err(format!("readability changed for {name}").into());
                    }
                    if unreadable {
                        Sample::Unreadable
                    } else {
                        let values = reads
                            .iter()
                            .map(|r| {
                                r.values
                                    .get(&address)
                                    .copied()
                                    .map(|value| {
                                        value
                                            & entry.mask.as_ref().expect("register mask")[0] as u32
                                    })
                                    .map(i64::from)
                                    .ok_or("missing primary register")
                            })
                            .collect::<std::result::Result<Vec<_>, _>>()?;
                        sample(vec![[
                            *values.iter().min().ok_or("no values")?,
                            *values.iter().max().ok_or("no values")?,
                        ]])
                    }
                }
            };
            entry.boards.insert(label.clone(), state);
        }
        if reference.boards.len() > 1 && entry.count == 1 {
            let mut constants = entry.boards.values().map(|s| match s {
                Sample::Read { ranges, .. } => {
                    ranges.first().filter(|[a, b]| a == b).map(|[a, _]| *a)
                }
                _ => None,
            });
            let first = constants.next().flatten();
            if first.is_some() && constants.all(|v| v == first) {
                entry.shared = first;
                entry.boards.clear();
            }
        }
    }
    for board in reference.boards.values_mut() {
        if board.samples < 10 {
            return Err("each board requires at least ten primary boots".into());
        }
        board.sources.sort_by(|a, b| {
            a.run_id
                .cmp(&b.run_id)
                .then(a.scenario.cmp(&b.scenario))
                .then(a.repetition.cmp(&b.repetition))
        });
    }
    Ok(reference)
}
#[derive(Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Check {
    pub verdict: Verdict,
    pub reasons: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comparison: Option<compare::Summary>,
}
impl Check {
    pub fn incomplete(reason: impl ToString) -> Self {
        Self {
            verdict: Verdict::Incomplete,
            reasons: vec![reason.to_string()],
            comparison: None,
        }
    }
}
impl Reference {
    pub fn read(path: &Path) -> Result<Self> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
    pub fn write(&self, path: &Path) -> Result<()> {
        oer_os_durable::atomic_json(path, self)
    }
    /// Validate the complete document, including unselected boards, before capture.
    pub fn validate(
        &self,
        signature: &Signature,
        images: &RegisterImages,
        board: &str,
        revision: u16,
    ) -> Result<()> {
        if self.schema != SCHEMA
            || self.chip != CHIP
            || self.lifecycle != Lifecycle::Cold
            || self.scenario != SCENARIO
            || &self.signature != signature
            || !(1..=14).contains(&self.channel)
        {
            return Err("unknown schema or stale source/configuration identity".into());
        }
        let selected = self.boards.get(board).ok_or("unknown board label")?;
        if selected.revision != revision {
            return Err("board revision differs from the reference".into());
        }
        for (name, board) in &self.boards {
            if !label(name)
                || board.samples < 10
                || board
                    .sources
                    .iter()
                    .try_fold(0_usize, |n, s| n.checked_add(s.samples))
                    != Some(board.samples)
            {
                return Err("invalid board sample provenance".into());
            }
        }
        let expected = metadata(self.lifecycle, images)?;
        if expected.len() != self.entries.len() {
            return Err("missing reference coverage".into());
        }
        for (name, mut expected) in expected {
            let entry = self
                .entries
                .get(&name)
                .ok_or("missing reference field/register")?;
            expected.boards = entry.boards.clone();
            expected.shared = entry.shared;
            if &expected != entry
                || (entry.shared.is_none() && entry.boards.keys().ne(self.boards.keys()))
            {
                return Err(
                    format!("stale field metadata or missing board coverage: {name}").into(),
                );
            }
            let states = if let Some(shared) = entry.shared {
                vec![sample(vec![[shared, shared]])]
            } else {
                entry.boards.values().cloned().collect()
            };
            for state in &states {
                if let Sample::Read { ranges, margin } = state {
                    if ranges.len() != entry.count as usize
                        || ranges.iter().any(|[a, b]| a > b)
                        || *margin < 0
                    {
                        return Err(format!("invalid reference bounds: {name}").into());
                    }
                    let bits = u32::from(entry.width) * 8;
                    if bits == 0 || bits > 32 {
                        return Err("unsupported reference width".into());
                    }
                    let (low, high) = if entry.signed {
                        (-(1_i64 << (bits - 1)), (1_i64 << (bits - 1)) - 1)
                    } else {
                        (0, (1_i64 << bits) - 1)
                    };
                    if ranges.iter().any(|[a, b]| *a < low || *b > high) {
                        return Err("reference value exceeds its field width".into());
                    }
                    if *state != sample(ranges.clone()) {
                        return Err("reference margin differs from its bounds".into());
                    }
                } else if entry.space != Space::Mmio {
                    return Err("only discovered MMIO may be unreadable".into());
                }
            }
            if entry.shared.is_some()
                && (self.boards.len() < 2 || entry.count != 1 || !entry.boards.is_empty())
            {
                return Err("shared scalar is not constant across all boards".into());
            }
        }
        Ok(())
    }
    pub fn readable(&self, board: &str, images: &RegisterImages) -> Result<Vec<usize>> {
        images
            .registers
            .iter()
            .enumerate()
            .filter_map(
                |(i, r)| match self.entries.get(&r.name).and_then(|e| e.for_board(board)) {
                    Some(Sample::Read { .. }) => Some(Ok(i)),
                    Some(Sample::Unreadable) => None,
                    None => Some(Err("missing readability coverage".into())),
                },
            )
            .collect()
    }
    fn endpoints(&self, board: &str, space: Space) -> Result<[Readings; 2]> {
        let mut endpoints = [Readings::default(), Readings::default()];
        for entry in self.entries.values().filter(|e| e.space == space) {
            let address = entry.address.ok_or("missing address")?;
            match entry.for_board(board).ok_or("missing board")? {
                Sample::Read { ranges, .. } => {
                    for (i, reading) in endpoints.iter_mut().enumerate() {
                        let range = ranges.first().ok_or("missing register bounds")?;
                        reading.values.insert(
                            address,
                            u32::try_from(*range.get(i).ok_or("missing endpoint")?)?,
                        );
                    }
                }
                Sample::Unreadable => {
                    for reading in &mut endpoints {
                        reading.unreadable.insert(address);
                    }
                }
            }
        }
        Ok(endpoints)
    }
    pub fn check(
        &self,
        signature: &Signature,
        images: &RegisterImages,
        board: &str,
        revision: u16,
        identity: &Images,
        production: &[ProductionBoot],
    ) -> Check {
        match self.check_complete(signature, images, board, revision, identity, production) {
            Ok(summary) => Check {
                verdict: summary.verdict,
                reasons: vec![],
                comparison: Some(summary),
            },
            Err(e) => Check::incomplete(e),
        }
    }
    fn check_complete(
        &self,
        signature: &Signature,
        images: &RegisterImages,
        board: &str,
        revision: u16,
        identity: &Images,
        production: &[ProductionBoot],
    ) -> Result<compare::Summary> {
        self.validate(signature, images, board, revision)?;
        if identity.vendor_application_sha256 != self.vendor_application_sha256
            || identity.vendor_idf_revision != self.signature.idf_revision
            || identity.vendor_project != "calibration"
            || identity.production_image != "diagnostic-phy"
        {
            return Err("capture image identity differs from the reference".into());
        }
        if production.is_empty() {
            return Err("no production captures".into());
        }
        let readable = self.readable(board, images)?;
        let selected = readable
            .iter()
            .map(|&i| {
                images
                    .registers
                    .get(i)
                    .cloned()
                    .ok_or("unknown register index")
            })
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut objects = vec![];
        for boot in production {
            complete(&boot.registers, &selected, false)?;
            complete(&boot.analog, &images.analog, false)?;
            objects.push(
                boot.calibration
                    .clone()
                    .ok_or("missing production projection")?,
            );
        }
        let reviews = compare::ReviewFile::reviewed()?.at(self.lifecycle)?;
        let mut fields = vec![];
        let mut excluded = vec![];
        for f in compare::fields() {
            let r = reviews.fields.get(f.name).ok_or("unreviewed field")?;
            if r.excluded {
                excluded.push(compare::Excluded {
                    name: f.name.into(),
                    reason: r.reason.clone(),
                });
                continue;
            }
            let Sample::Read { ranges, .. } = self
                .entries
                .get(f.name)
                .ok_or("missing field")?
                .for_board(board)
                .ok_or("missing board bounds")?
            else {
                return Err("unreadable calibration field".into());
            };
            fields.push(compare::compare_field_captures(
                &f,
                Some(r),
                ranges,
                &objects,
            )?);
        }
        excluded.push(compare::Excluded {
            name: committed::TRACKING_PROGRESS_FIELD.into(),
            reason: "counts tracking work, not calibration".into(),
        });
        // The endpoints are an interval encoding, not synthetic measured boots.
        // Register comparison depends only on these minima/maxima.
        let mmio = self.endpoints(board, Space::Mmio)?;
        let analog = self.endpoints(board, Space::Analog)?;
        let registers = compare::compare_registers(
            &images.registers,
            mmio.iter(),
            production.iter().map(|b| &b.registers),
            &reviews.registers,
        );
        let analog = compare::compare_registers(
            &images.analog,
            analog.iter(),
            production.iter().map(|b| &b.analog),
            &reviews.registers,
        );
        let verdict = if fields.iter().any(|f| f.verdict == Verdict::Diff)
            || registers.verdict == Verdict::Diff
            || analog.verdict == Verdict::Diff
        {
            Verdict::Diff
        } else if fields.iter().any(|f| f.verdict == Verdict::Incomplete)
            || registers.verdict == Verdict::Incomplete
            || analog.verdict == Verdict::Incomplete
        {
            Verdict::Incomplete
        } else {
            Verdict::Match
        };
        Ok(compare::Summary {
            schema: compare::SCHEMA,
            date: compare::utc_date(identity.started_unix_seconds),
            lifecycle: self.lifecycle,
            verdict,
            vendor_object: compare::VENDOR_OBJECT.into(),
            images: identity.clone(),
            vendor_captures: self.boards.get(board).ok_or("missing board")?.samples,
            production_captures: production.len(),
            fields,
            excluded,
            uncovered: compare::uncovered(self.vendor_object_bytes, &compare::fields()),
            registers,
            analog,
        })
    }
}
pub fn images(root: &Path) -> Result<RegisterImages> {
    Esp32s31::images(root)
}
