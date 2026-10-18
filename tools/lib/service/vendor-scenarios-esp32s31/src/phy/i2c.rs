//! Authenticated PHY I2C command memory, transport, calibration leaves/prefix
//! and RFPLL against compiled production.
//!
//! Software comparison under explicit peripheral assumptions, never hardware
//! qualification. Private inputs, requests and evidence stay in the selected
//! ignored output.
use crate::evidence::{events, output, stop};
use crate::gain_state::Unmet;
use crate::harness::{
    Budget, Buffer, Input, Result, case, data_request, direct, filled, invalid, invocation,
    named_object, named_section, selection, symbol,
};
use crate::layout::*;
use crate::phy::image_layout;
use crate::session::{Artifact, Session, image_symbol, request};
use blobray_domain::{
    CallEndpoint, ComparisonVerdict, DataSelector, DeviceBehavior, DeviceDeclaration, EffectRule,
    ExecutionCase, ExecutionEvent, ExecutionEvidence, ExecutionGap, ExecutionStop, ExecutionTarget,
    LinkRequest, MemoryAccess, ModelStatus, RegionLifetime, RegisterCell, SessionReset,
};
use oer_riscv_model::{ArtifactId, ObjectId, ObjectLocation};
use std::{collections::BTreeMap, path::PathBuf};

/// Production destination of the initialized 400-byte parameter prefix.
const SETUP_DESTINATION: u32 = 0x3fff_2000;
/// Six caller-owned command-memory parameters of the production probe.
const COMMAND_PARAMETERS: u32 = 0x3fff_1000;
/// Event capacity of one command-memory phase (45 writes per side).
const COMMAND_EVENTS: u32 = 512;

pub const OBJECT_SHA: &str = "7e6ebb1353d1bd2c53b4b5b1176bbf795c57d899c3f07a26ce56e74b5803e9d9";
pub const TABLE_SHA: &str = "927b3305a35468bb52f3de4e3305f4b4d0674831014376a094ceb00022bab183";

/// Independent instruction reading of the authenticated `phy_i2c.o` root and ROM
/// encode/fill leaves. Low words specify block/register in call order; these are
/// research expectations, not a production implementation or generated PAC test.
pub const COMMAND_LOW: [u32; 45] = [
    0x0267, 0x016b, 0x026b, 0x036b, 0x046b, 0x056b, 0x066b, 0x076b, 0x086b, 0x096b, 0x0a6b, 0x0b6b,
    0x0c6b, 0x0d6b, 0x0e6b, 0x0f6b, 0x0062, 0x0462, 0x0b62, 0x0d62, 0x0f62, 0x1562, 0x0266, 0x0267,
    0x0467, 0x0567, 0x0667, 0x0767, 0x0c67, 0x0d67, 0x0e67, 0x0f67, 0x1467, 0x1567, 0x1667, 0x1767,
    0x1867, 0x1967, 0x1c67, 0x1d67, 0x1e67, 0x1f67, 0x0663, 0x006a, 0x016a,
];
pub const FIXED_PREFIX: [u32; 20] = [
    7, 1, 0x73, 0xba, 0x88, 1, 0x11, 0xfd, 0xbf, 2, 8, 4, 0xa7, 0x77, 0xf4, 0x81, 0x68, 0xa8, 0x44,
    10,
];
/// Explicit expected parameter-derived values at indices 20 and 24..41. Cases
/// include both saturation bounds, byte wrapping and the forced filter bit.
pub const PROFILES: [(&str, [u8; 6], [u32; 19]); 4] = [
    (
        "zero",
        [0; 6],
        [0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6, 2, 0, 2, 2, 0, 0, 64, 0],
    ),
    (
        "mixed",
        [165, 33, 67, 10, 254, 85],
        [
            165, 33, 33, 67, 67, 33, 33, 67, 67, 16, 16, 8, 10, 0, 0, 85, 85, 85, 85,
        ],
    ),
    (
        "lower",
        [255, 255, 255, 2, 255, 255],
        [
            255, 255, 255, 255, 255, 255, 255, 255, 255, 8, 8, 2, 2, 1, 1, 255, 255, 255, 255,
        ],
    ),
    (
        "upper",
        [1, 2, 3, 255, 253, 1],
        [
            1, 2, 2, 3, 3, 2, 2, 3, 3, 60, 60, 60, 255, 255, 255, 1, 1, 65, 1,
        ],
    ),
];
const PARAMETER_OFFSETS: [usize; 6] = [0x18e, 0xe9, 0xea, 0xed, 0xee, 0xf0];

/// All 45 command-memory words for one parameter profile.
pub fn command_words(dynamic: &[u32; 19]) -> Vec<u32> {
    let mut values = FIXED_PREFIX.to_vec();
    values.extend([dynamic[0], 8, 0x70, 0x27]);
    values.extend(&dynamic[1..]);
    values.extend([0, 0xaf, 0x7f]);
    assert_eq!(values.len(), COMMAND_LOW.len());
    COMMAND_LOW
        .iter()
        .zip(values)
        .map(|(low, value)| low + (value << 16))
        .collect()
}

pub struct Options {
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: Budget,
    /// Linked SDK firmware; enables calibration leaves (12.1) and the PBus/DCODE prefix (12.2).
    pub sdk: Option<PathBuf>,
    /// SDK firmware with the RFPLL diagnostics symbol; enables RFPLL (12.3).
    pub phy_sdk: Option<PathBuf>,
    /// Point mutants applied to the loaded production image of every comparison.
    pub patches: Vec<blobray_application::in_process::ImagePatch>,
}

pub const LEAVES_OBLIGATION: Unmet = Unmet {
    id: "calibration-leaves",
    unit: "12.1",
    reason: "authenticated SDK firmware was not supplied; the four calibration leaves did not execute",
};
pub const PREFIX_OBLIGATION: Unmet = Unmet {
    id: "calibration-prefix",
    unit: "12.2",
    reason: "authenticated SDK firmware was not supplied; PBus clear and DCODE did not execute",
};
pub const RFPLL_OBLIGATION: Unmet = Unmet {
    id: "rfpll",
    unit: "12.3",
    reason: "authenticated PHY SDK firmware was not supplied; RFPLL search and maintenance did not execute",
};

/// Obligations that cannot execute with the supplied optional inputs.
pub fn unmet(sdk: bool, phy_sdk: bool) -> Vec<Unmet> {
    let mut result = vec![];
    if !sdk {
        result.extend([LEAVES_OBLIGATION, PREFIX_OBLIGATION]);
    }
    if !phy_sdk {
        result.push(RFPLL_OBLIGATION);
    }
    result
}

/// Linked PHY I2C image with its captured roots and both execution targets.
pub struct I2c {
    pub session: Session,
    pub roots: BTreeMap<String, u32>,
    /// The linked image, whose roots are vendor endpoints.
    pub image_object: ObjectId,
    pub parameter: u32,
    pub vendor: ExecutionTarget,
    pub replacement: ExecutionTarget,
}

impl std::ops::Deref for I2c {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}
impl std::ops::DerefMut for I2c {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

pub const LEAVES: [&str; 5] = [
    "phy_i2c_master_mem_cfg",
    "phy_i2c_master_command_mem_cfg",
    "phy_get_i2c_data",
    "phy_i2c_enter_critical",
    "phy_i2c_exit_critical",
];

impl I2c {
    pub fn new(options: &Options) -> Result<Self> {
        if options.phy_sdk.is_some() && options.sdk.is_none() {
            return Err(invalid(
                "--phy-sdk requires --sdk: RFPLL runs inside the calibration prefix",
            ));
        }
        let mut inputs = vec![
            Input {
                role: "phy",
                path: &options.library,
                sha256: Some(crate::artifacts::sha256("libphy")?),
            },
            Input {
                role: "rom",
                path: &options.rom,
                sha256: Some(crate::artifacts::sha256("rom")?),
            },
            Input {
                role: "production",
                path: &options.production,
                sha256: None,
            },
        ];
        if let Some(sdk) = &options.sdk {
            inputs.push(Input {
                role: "sdk",
                path: sdk,
                sha256: Some(crate::artifacts::sha256("sdk")?),
            });
        }
        if let Some(phy_sdk) = &options.phy_sdk {
            inputs.push(Input {
                role: "phy-sdk",
                path: phy_sdk,
                sha256: Some(crate::artifacts::sha256("phy-sdk")?),
            });
        }
        let session = Session::start(
            &options.output,
            options.budget,
            &inputs,
            "I2C software comparison",
            &options.patches,
        )?;
        let (run, inventory) = (&session.run, &session.inventory);
        let object = named_object(inventory, 0, "phy_i2c.o")?;
        let section = named_section(object, ".rodata.CSWTCH.51")?;
        let request = data_request(
            object,
            DataSelector::Section {
                section: section.index,
                offset: 0,
                length: 200,
            },
        );
        let exported = session.data("table", &request)?;
        let table = exported.bytes;
        if exported.payload.as_str() != OBJECT_SHA || oer_durable::sha256_bytes(&table) != TABLE_SHA
        {
            return Err(invalid("I2C object or table identity mismatch"));
        }
        let leaves = options.sdk.is_some();
        let rfpll = options.phy_sdk.is_some();
        let mut roots: Vec<&str> = LEAVES.to_vec();
        roots.extend(["phy_get_i2c_read_mask_new", "phy_get_i2c_hostid_new"]);
        if leaves {
            roots.extend([
                "phy_txgain_comp_pacfg_new",
                "phy_force_dig_gain",
                "phy_temp_to_power_new",
                "phy_reg_update_new",
            ]);
        }
        if rfpll {
            roots.extend([
                "phy_rfpll_cap_init_cal_track",
                "phy_rfpll_cap_track_new",
                "phy_set_rfpll_freq_new",
            ]);
        }
        let select = |input: usize, name: &str| -> Result<oer_riscv_model::SymbolId> {
            Ok(symbol(inventory, input, name)?.id.clone())
        };
        // ROM first; the SDK firmware supplies the crystal-clock and
        // diagnostics symbols the ROM lacks.
        let mut candidates = vec![ROM_INPUT];
        if leaves {
            candidates.push(3);
        }
        if rfpll {
            candidates.push(4);
        }
        let link = LinkRequest {
            companions: vec![],
            inputs: vec![session.input_id(0)?],
            entry: select(0, "phy_i2c_master_cmd_mem_init")?,
            roots: roots.iter().map(|n| select(0, n)).collect::<Result<_>>()?,
            layout: image_layout(),
            absent: vec![],
        };
        let linked = session.link(
            &link,
            &options.linker,
            "phy_i2c_master_cmd_mem_init",
            &candidates,
        )?;
        // Independent current-ELF symbol selection: an inexact source mapping is not
        // sufficient to assert a physical mutable-data address.
        let (parameter, size) = image_symbol(
            &run.join("image/image.elf"),
            &run.join("image-symbols.txt"),
            "phy_param",
        )?;
        assert_eq!(size, u64::from(PHY_PARAM_BYTES));
        assert!(
            linked
                .manifest
                .mappings
                .iter()
                .any(|m| m.address == u64::from(parameter) && m.size == u64::from(PHY_PARAM_BYTES))
        );
        let (vendor, replacement) = session.targets(&linked.manifest.elf)?;
        Ok(Self {
            image_object: ObjectId {
                artifact: linked.manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            session,
            roots: linked.roots,
            parameter,
            vendor,
            replacement,
        })
    }

    pub fn root(&self, name: &str) -> u32 {
        *self
            .roots
            .get(name)
            .unwrap_or_else(|| panic!("missing root {name}"))
    }

    /// Exact code endpoint of the linked image root `name`.
    pub fn root_endpoint(&self, name: &str) -> Result<CallEndpoint> {
        self.session
            .image_endpoint(&self.image_object, name, self.root(name))
    }

    /// Propose and accept `rules` as the effect contract between `vendor` and
    /// `replacement` under `unclassified: required`.
    pub fn review_pair(
        &mut self,
        name: &str,
        vendor: CallEndpoint,
        replacement: CallEndpoint,
        rules: Vec<EffectRule>,
        applicability: &str,
    ) -> Result<ArtifactId> {
        let contract = crate::contracts::phy_contract(vendor, replacement, rules, applicability);
        self.session.review_effects(
            name,
            &format!("esp32s31.phy.{name}.effects"),
            contract,
            applicability,
        )
    }

    /// Address of a declared production probe.
    pub fn probe(&self, name: &str) -> u32 {
        self.probes.entry(name).unwrap_or_else(|e| panic!("{e}"))
    }

    /// Address of a captured (not declared) symbol in one input.
    pub fn captured(&self, input: usize, name: &str) -> u32 {
        let value = symbol(&self.inventory, input, name)
            .unwrap_or_else(|e| panic!("{e}"))
            .value;
        u32::try_from(value).expect("RV32 symbol")
    }

    /// Submit over explicit targets and return the evidence records.
    #[allow(clippy::too_many_arguments)]
    pub fn submit_with(
        &mut self,
        label: &str,
        vendor: &ExecutionTarget,
        replacement: Option<&ExecutionTarget>,
        fill: Option<u8>,
        cases: Vec<ExecutionCase>,
        max_events: u32,
        verdict: Option<ComparisonVerdict>,
    ) -> Result<Vec<ExecutionEvidence>> {
        let request = request(vendor, replacement, fill, cases, max_events);
        Ok(self
            .session
            .submit(label, &request, verdict)?
            .records
            .clone())
    }

    /// Captured vendor image against compiled production, stacks unknown.
    pub fn compare(
        &mut self,
        label: &str,
        cases: Vec<ExecutionCase>,
        verdict: ComparisonVerdict,
        max_events: u32,
    ) -> Result<Vec<ExecutionEvidence>> {
        let (vendor, replacement) = (self.vendor.clone(), self.replacement.clone());
        self.submit_with(
            label,
            &vendor,
            Some(&replacement),
            None,
            cases,
            max_events,
            Some(verdict),
        )
    }

    pub fn last(&self) -> &Artifact {
        self.artifacts.last().expect("retained execution")
    }

    /// Command memory, descriptors, leaves and their negative outcomes.
    pub fn command_memory(&mut self) -> Result<usize> {
        let init = self.root("phy_i2c_master_cmd_mem_init");
        let bank = DeviceDeclaration {
            id: "command-ram".into(),
            applicability: "45 aligned command slots; passive register bank".into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: (0..45)
                    .map(|i| RegisterCell {
                        address: COMMAND_RAM + i * 4,
                        width: 4,
                        value: 0,
                    })
                    .collect(),
            },
        };
        let mut cases = vec![];
        let mut expected_commands = vec![];
        for (name, parameters, dynamic) in &PROFILES {
            let mut full = vec![0u8; 400];
            for (offset, value) in PARAMETER_OFFSETS.iter().zip(parameters) {
                full[*offset] = *value;
            }
            let source = || Buffer::new(INPUT, full.clone()).session();
            let left = self.probes.invoke(
                "open_phy_trace_initialize_parameters",
                vec![
                    ("destination", i64::from(self.parameter).into()),
                    ("source", source().into()),
                ],
                vec![],
                vec![selection(self.parameter, 400)],
            )?;
            let right = self.probes.invoke(
                "open_phy_trace_initialize_parameters",
                vec![
                    (
                        "destination",
                        Buffer::new(SETUP_DESTINATION, []).session().into(),
                    ),
                    ("source", source().into()),
                ],
                vec![],
                vec![selection(SETUP_DESTINATION, 400)],
            )?;
            cases.push(case(
                format!("{name}-setup"),
                left,
                Some(right),
                SessionReset::Cold,
                true,
            ));
            let left = direct(init, &[0], vec![], vec![bank.clone()], vec![]);
            let probe = self.probes.invoke(
                "open_phy_trace_command_memory",
                vec![(
                    "parameters",
                    Buffer::new(COMMAND_PARAMETERS, *parameters).into(),
                )],
                vec![bank.clone()],
                vec![],
            )?;
            let right = direct(
                probe.entry,
                &[probe.arguments[0].expect("known command parameters")],
                probe.memory,
                probe.models,
                probe.observe_memory,
            );
            expected_commands.push((cases.len() as u32, command_words(dynamic)));
            cases.push(case(*name, left, Some(right), SessionReset::Warm, false));
        }
        let mut descriptors = vec![];
        for (name, length, expected) in [
            (LEAVES[0], 6u32, vec![0u8, 0, 1, 1, 44, 1]),
            (LEAVES[1], 12, vec![0, 0, 0, 1, 1, 1, 44, 1, 2, 0, 0, 0]),
        ] {
            for fill in [0u8, 255] {
                let arguments = vec![Some(INPUT), Some(INPUT + 8)];
                let memory = vec![filled(INPUT, length, fill)?];
                let observe = vec![selection(INPUT, length)];
                descriptors.push((cases.len() as u32, expected.clone()));
                cases.push(case(
                    format!("{name}{fill}"),
                    invocation(
                        self.root(name),
                        arguments.clone(),
                        memory.clone(),
                        vec![],
                        observe.clone(),
                    ),
                    Some(invocation(
                        self.probe(&format!("open_phy_trace_{name}")),
                        arguments,
                        memory,
                        vec![],
                        observe,
                    )),
                    SessionReset::Cold,
                    true,
                ));
            }
        }
        for name in &LEAVES[2..] {
            cases.push(case(
                *name,
                invocation(self.root(name), vec![], vec![], vec![], vec![]),
                Some(invocation(
                    self.probe(&format!("open_phy_trace_{name}")),
                    vec![],
                    vec![],
                    vec![],
                    vec![],
                )),
                SessionReset::Cold,
                false,
            ));
        }
        let positive = self.artifacts.len();
        let records = self.compare(
            "compare",
            cases.clone(),
            ComparisonVerdict::Match,
            COMMAND_EVENTS,
        )?;
        let outcomes: Vec<_> = records
            .iter()
            .filter(|r| matches!(r, ExecutionEvidence::Outcome { .. }))
            .collect();
        assert_eq!(outcomes.len(), 2 * cases.len());
        assert!(outcomes.iter().all(|r| matches!(
            r,
            ExecutionEvidence::Outcome {
                stop: ExecutionStop::Returned { .. },
                ..
            }
        )));
        let comparisons: Vec<_> = records
            .iter()
            .filter_map(|r| match r {
                ExecutionEvidence::Comparison { result, .. } => Some(result.verdict),
                _ => None,
            })
            .collect();
        assert_eq!(comparisons.len(), cases.len());
        assert!(comparisons.iter().all(|v| *v == ComparisonVerdict::Match));
        assert!(
            !records
                .iter()
                .any(|r| matches!(r, ExecutionEvidence::Event { case, .. } if *case >= 8))
        );
        let models: Vec<_> = records
            .iter()
            .filter_map(|r| match r {
                ExecutionEvidence::Model { observation, .. } => Some(observation),
                _ => None,
            })
            .collect();
        assert_eq!(models.len(), 8);
        assert!(models.iter().all(|m| m.writes == 45
            && m.reads == 0
            && m.closed
            && m.status == ModelStatus::Complete
            && m.issue.is_none()));
        for (index, words) in &expected_commands {
            for side in [false, true] {
                let expected: Vec<_> = words
                    .iter()
                    .enumerate()
                    .map(|(i, w)| ExecutionEvent::Write {
                        address: COMMAND_RAM + 4 * i as u32,
                        width: 4,
                        value: *w,
                    })
                    .collect();
                assert_eq!(events(&records, *index, side), expected, "{index} {side}");
            }
        }
        for (index, expected) in &descriptors {
            for side in [false, true] {
                assert_eq!(&output(&records, *index, side), expected, "{index} {side}");
            }
        }
        // A changed replacement input must not be hidden by code-goal completion.
        let mut different = cases[..2].to_vec();
        different[1].replacement.as_mut().unwrap().memory[0]
            .seed
            .bytes[1] = 1;
        self.compare(
            "different",
            different,
            ComparisonVerdict::Diff,
            COMMAND_EVENTS,
        )?;
        let mut unknown = cases[1].clone();
        unknown.reset = SessionReset::Cold;
        // The command-parameter pointer (`a0`) is unknown.
        unknown.replacement.as_mut().unwrap().arguments[0] = None;
        self.compare(
            "unknown",
            vec![unknown],
            ComparisonVerdict::Incomplete,
            COMMAND_EVENTS,
        )?;
        let mut missing = cases[1].clone();
        missing.reset = SessionReset::Cold;
        // The vendor image without the ROM, which `Session::targets` maps
        // second.
        let mut vendor = self.vendor.clone();
        vendor.executables.remove(1);
        let replacement = self.replacement.clone();
        let records = self.submit_with(
            "missing-rom",
            &vendor,
            Some(&replacement),
            None,
            vec![missing],
            COMMAND_EVENTS,
            Some(ComparisonVerdict::Incomplete),
        )?;
        let encode = self.captured(1, "phy_encode_i2c_master");
        assert!(matches!(
            stop(&records, 0, false),
            ExecutionStop::Incomplete { reason: ExecutionGap::Memory { address, access: MemoryAccess::Fetch }, .. } if address == encode
        ));
        let limited = request(&self.vendor, Some(&self.replacement), None, cases, 1);
        self.capacity_failure("capacity", &limited)?;
        Ok(positive)
    }
}

/// Returned low word of one case side.
pub fn returned_low(records: &[ExecutionEvidence], case: u32, side: bool) -> Option<u32> {
    match stop(records, case, side) {
        ExecutionStop::Returned { low, .. } => low,
        other => panic!("case {case} side {side} did not return: {other:?}"),
    }
}

/// Word-sized MMIO writes of one case side; every event must be a word read or write.
pub fn word_writes(records: &[ExecutionEvidence], case: u32, side: bool) -> Vec<(u32, u32)> {
    events(records, case, side)
        .iter()
        .filter_map(|event| match event {
            ExecutionEvent::Read { width: 4, .. } => None,
            ExecutionEvent::Write {
                address,
                width: 4,
                value,
            } => Some((*address, *value)),
            other => panic!("unexpected event {other:?}"),
        })
        .collect()
}

/// Model observations of one case side.
pub fn models(
    records: &[ExecutionEvidence],
    case: u32,
    side: bool,
) -> Vec<&blobray_domain::ModelObservation> {
    records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Model {
                case: c,
                replacement,
                observation,
            } if *c == case && *replacement == side => Some(observation),
            _ => None,
        })
        .collect()
}

/// Every model and call-model of one case side completed.
pub fn all_complete(records: &[ExecutionEvidence], case: u32, side: bool) -> bool {
    records.iter().all(|r| match r {
        ExecutionEvidence::Model {
            case: c,
            replacement,
            observation,
        } if *c == case && *replacement == side => observation.status == ModelStatus::Complete,
        ExecutionEvidence::CallModel {
            case: c,
            replacement,
            observation,
        } if *c == case && *replacement == side => observation.status == ModelStatus::Complete,
        _ => true,
    })
}

pub fn manifest_complete(artifact: &Artifact) -> bool {
    artifact.complete
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_words_follow_block_order_and_profile_values() {
        let words = command_words(&PROFILES[1].2);
        assert_eq!(words.len(), 45);
        assert_eq!(words[0], 0x0267 | (7 << 16));
        assert_eq!(words[20], 0x0f62 | (165 << 16));
        assert_eq!(
            words[21..24],
            [
                0x1562 | (8 << 16),
                0x0266 | (0x70 << 16),
                0x0267 | (0x27 << 16)
            ]
        );
        assert_eq!(words[44], 0x016a | (0x7f << 16));
    }

    #[test]
    fn missing_optional_inputs_are_unmet_obligations() {
        assert_eq!(
            unmet(false, false),
            [LEAVES_OBLIGATION, PREFIX_OBLIGATION, RFPLL_OBLIGATION]
        );
        assert_eq!(unmet(true, false), [RFPLL_OBLIGATION]);
        assert!(unmet(true, true).is_empty());
    }
}
