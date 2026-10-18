//! Espressif PHY sessions: a linked captured PHY image with compiled
//! production and the shared stack-entry, parameter-setup and
//! callback-installation phases, over the installed chip's [`PhyLayout`].
pub mod contracts;
pub mod layout;

use crate::evidence::{outcomes, split_cases};
use crate::harness::direct;
use crate::harness::{Budget, Input, with_stack_fill};
use crate::harness::{Result, invalid, known, region, selection, symbol, words};
use crate::session::{Session, image_symbol, request};
use blobray_domain::{
    CallEndpoint, ComparisonVerdict, DataSelector, DeviceDeclaration, ExecutionCase,
    ExecutionEvidence, ExecutionRegion, ExecutionRequest, ExecutionStop, ExecutionTarget,
    ImageLayout, ImageRegion, Invocation, LinkRequest, MemorySelection, RegionLifetime,
};
use layout::{FILLS, MAX_EVENTS};
pub use layout::{PhyLayout, layout};
use oer_riscv_model::{ArtifactId, ObjectId, ObjectLocation, SymbolId};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

/// Which implementation, if any, is compared with the captured vendor side.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Right {
    /// Vendor characterization; relations and verdict are absent.
    None,
    Production,
    /// Vendor-boundary characterization against a second vendor invocation.
    Vendor,
}

pub struct Executed {
    pub records: Vec<ExecutionEvidence>,
    pub request: ExecutionRequest,
    pub identity: ArtifactId,
}

/// Parts of one request over several stack fills, each with its tag and its
/// records renumbered from case zero, and the submitted request.
pub struct FillResults<T> {
    pub parts: Vec<(T, Vec<ExecutionEvidence>)>,
    pub request: ExecutionRequest,
    pub identity: ArtifactId,
}

/// Linked captured PHY image, the production target and their retained runs.
pub struct PhyImage {
    pub session: Session,
    pub roots: BTreeMap<String, u32>,
    pub parameter: u32,
    /// The linked image the vendor target maps.
    pub image: ArtifactId,
    pub image_object: ObjectId,
    pub vendor: ExecutionTarget,
    pub production: ExecutionTarget,
}

impl std::ops::Deref for PhyImage {
    type Target = Session;
    fn deref(&self) -> &Session {
        &self.session
    }
}
impl std::ops::DerefMut for PhyImage {
    fn deref_mut(&mut self) -> &mut Session {
        &mut self.session
    }
}

/// Private inputs and budget of a scenario over the pinned archive, ROM and
/// compiled production.
pub struct PhyOptions {
    pub library: PathBuf,
    pub rom: PathBuf,
    pub production: PathBuf,
    pub linker: PathBuf,
    pub output: PathBuf,
    pub budget: Budget,
    /// Point mutants applied to the loaded production image of every comparison.
    pub patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// Start a session over the authenticated archive (input 0), ROM (input 1),
/// production (input 2) and `extra` inputs from index 3.
pub fn start_session(options: &PhyOptions, extra: &[Input<'_>], purpose: &str) -> Result<Session> {
    let mut inputs = vec![
        Input {
            role: "phy",
            path: &options.library,
            sha256: Some(crate::artifacts::sha256(layout().library)?),
        },
        Input {
            role: "rom",
            path: &options.rom,
            sha256: Some(crate::artifacts::sha256(layout().rom)?),
        },
        Input {
            role: "production",
            path: &options.production,
            sha256: None,
        },
    ];
    inputs.extend_from_slice(extra);
    Session::start(
        &options.output,
        options.budget,
        &inputs,
        purpose,
        &options.patches,
    )
}

/// Requested-delay ABI at `address`: every call returns zero and records its
/// first argument as requested microseconds. The call count is evidence, not
/// a declared budget; scenarios compare the ordered delay values instead.
pub fn delay_calls(id: &str, address: u32) -> Vec<blobray_domain::CallDeclaration> {
    use blobray_domain::{
        CallBinding, CallBoundary, CallDeclaration, CallRepetition, CallResponse, CallValue,
    };
    vec![CallDeclaration {
        repetition: CallRepetition::Unbounded,
        id: id.into(),
        applicability: "declared delay ABI; requested microseconds only".into(),
        lifetime: RegionLifetime::Phase,
        binding: CallBinding {
            address,
            boundary: CallBoundary::CapturedCode,
            allow_tail: true,
        },
        argument_words: 1,
        responses: vec![CallResponse {
            return_words: [Some(0), Some(0)],
            outputs: vec![],
            allocation: None,
            delay_micros: Some(CallValue::Argument { word: 0 }),
        }],
    }]
}

/// Authenticated PHY SDK firmware: supplies the never-executed diagnostics
/// symbols (`phy_printf`) that co-located archive sections reference.
pub fn phy_sdk_input(path: &Path) -> Result<Input<'_>> {
    Ok(Input {
        role: "phy-sdk",
        path,
        sha256: Some(crate::artifacts::sha256(
            layout()
                .phy_sdk
                .expect("the chip's PHY layout names its SDK firmware"),
        )?),
    })
}

/// Code and data placement of linked PHY images.
pub fn image_layout() -> ImageLayout {
    ImageLayout {
        code: ImageRegion {
            start: layout().image_code_start,
            length: layout().image_region_bytes,
        },
        data: ImageRegion {
            start: layout().image_data_start,
            length: layout().image_region_bytes,
        },
    }
}

/// Exact captured symbol of one input as a link selection.
pub fn select(session: &Session, input: usize, name: &str) -> Result<SymbolId> {
    Ok(symbol(&session.inventory, input, name)?.id.clone())
}

impl PhyImage {
    /// Link the image, check the captured `phy_param` extent and prepare targets.
    pub fn link(
        session: Session,
        link: &LinkRequest,
        linker: &Path,
        entry: &str,
        candidates: &[u64],
    ) -> Result<Self> {
        let linked = session.link(link, linker, entry, candidates)?;
        let (parameter, size) = image_symbol(
            &session.run.join("image/image.elf"),
            &session.run.join("image-symbols.txt"),
            "phy_param",
        )?;
        if size != u64::from(layout().phy_param_bytes) {
            return Err(invalid(format!(
                "phy_param does not have the expected {}-byte extent",
                layout().phy_param_bytes
            )));
        }
        let (vendor, production) = session.targets(&linked.manifest.elf)?;
        Ok(Self {
            image: linked.manifest.elf.clone(),
            image_object: ObjectId {
                artifact: linked.manifest.elf.clone(),
                location: ObjectLocation::Standalone,
            },
            roots: linked.roots,
            parameter,
            vendor,
            production,
            session,
        })
    }

    pub fn sym(&self, input: usize, name: &str) -> u32 {
        let value = symbol(&self.inventory, input, name)
            .unwrap_or_else(|e| panic!("{e}"))
            .value;
        u32::try_from(value).expect("RV32 symbol")
    }

    pub fn root(&self, name: &str) -> u32 {
        *self
            .roots
            .get(name)
            .unwrap_or_else(|| panic!("missing root {name}"))
    }

    /// Exact code endpoint of the linked image root `name`.
    pub fn vendor_endpoint(&self, name: &str) -> Result<CallEndpoint> {
        self.session
            .image_endpoint(&self.image_object, name, self.root(name))
    }

    /// Exact code endpoint of the compiled production function `name`.
    pub fn production_endpoint(&self, name: &str) -> Result<CallEndpoint> {
        self.session.input_endpoint(2, name)
    }

    /// Enter `target` directly with explicit ABI words.
    pub fn enter(
        &self,
        target: u32,
        arguments: &[u32],
        memory: Vec<ExecutionRegion>,
        observe: Vec<MemorySelection>,
        models: Vec<DeviceDeclaration>,
    ) -> Invocation {
        direct(target, arguments, memory, models, observe)
    }

    /// A prepared probe invocation with unused argument registers at zero.
    pub fn enter_probe(&self, mut probe: Invocation) -> Invocation {
        if probe.arguments.len() < 8 {
            probe.arguments.resize(8, Some(0));
        }
        probe
    }

    /// A zero-length captured `memcpy`: the production counterpart of a
    /// vendor-only phase such as callback installation.
    pub fn noop(&self) -> Invocation {
        self.enter(
            self.sym(1, "memcpy"),
            &[layout().parameter_copy, layout().parameter_copy, 0],
            vec![],
            vec![],
            vec![],
        )
    }

    /// Copy 492 parameter bytes into the captured `phy_param` or a production buffer.
    pub fn setup(&self, data: &[u8], production: bool) -> Invocation {
        let memcpy = self.sym(1, "memcpy");
        if production {
            self.enter(
                memcpy,
                &[
                    layout().parameter_copy,
                    layout().input,
                    layout().phy_param_bytes,
                ],
                vec![
                    known(layout().input, layout().phy_param_bytes, data).unwrap(),
                    region(
                        layout().parameter_copy,
                        layout().phy_param_bytes,
                        &[],
                        None,
                        RegionLifetime::Session,
                    )
                    .unwrap(),
                ],
                vec![],
                vec![],
            )
        } else {
            self.enter(
                memcpy,
                &[self.parameter, layout().input, layout().phy_param_bytes],
                vec![known(layout().input, layout().phy_param_bytes, data).unwrap()],
                vec![],
                vec![],
            )
        }
    }

    pub fn execute(
        &mut self,
        label: &str,
        rows: Vec<ExecutionCase>,
        fill: u8,
        right: Right,
        verdict: ComparisonVerdict,
        maximum: u32,
    ) -> Result<Executed> {
        self.run(label, rows, fill, right, verdict, maximum, true)
    }

    /// `execute` for checks that need no guest events: Blobray still
    /// validates every record, but events are not read back.
    pub fn execute_without_events(
        &mut self,
        label: &str,
        rows: Vec<ExecutionCase>,
        fill: u8,
        right: Right,
        verdict: ComparisonVerdict,
        maximum: u32,
    ) -> Result<Executed> {
        self.run(label, rows, fill, right, verdict, maximum, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        label: &str,
        mut rows: Vec<ExecutionCase>,
        fill: u8,
        right: Right,
        verdict: ComparisonVerdict,
        maximum: u32,
        events: bool,
    ) -> Result<Executed> {
        if right == Right::None {
            for row in &mut rows {
                row.relation = None;
            }
        }
        let replacement = match right {
            Right::None => None,
            Right::Production => Some(self.production.clone()),
            Right::Vendor => Some(self.vendor.clone()),
        };
        let count = rows.len() * if replacement.is_some() { 2 } else { 1 };
        let request = request(
            &self.vendor,
            replacement.as_ref(),
            Some(fill),
            rows,
            maximum,
        );
        let expected = (right != Right::None).then_some(verdict);
        let artifact = self
            .session
            .submit_records(label, &request, expected, events)?;
        let complete = artifact.complete;
        let records = artifact.records.clone();
        let stops = outcomes(&records);
        assert_eq!(stops.len(), count, "{label}");
        if matches!(verdict, ComparisonVerdict::Match | ComparisonVerdict::Diff) {
            let unfinished: Vec<_> = records
                .iter()
                .filter_map(|r| match r {
                    blobray_domain::ExecutionEvidence::Model {
                        case, observation, ..
                    } if observation.status != observation.expected_status()
                        || observation.status == blobray_domain::ModelStatus::Incomplete =>
                    {
                        Some(format!("{case} {} {:?}", observation.id, observation.issue))
                    }
                    blobray_domain::ExecutionEvidence::CallModel {
                        case, observation, ..
                    } if observation.status != observation.expected_status() => {
                        Some(format!("{case} call {observation:?}"))
                    }
                    _ => None,
                })
                .collect();
            let unreturned: Vec<_> = stops
                .iter()
                .enumerate()
                .filter(|(_, stop)| !matches!(stop, ExecutionStop::Returned { .. }))
                .collect();
            assert!(
                complete,
                "{label}: unfinished models {unfinished:?}; unreturned {unreturned:?}"
            );
            assert!(
                stops
                    .iter()
                    .all(|s| matches!(s, ExecutionStop::Returned { .. })),
                "{label}: {stops:?}"
            );
        }
        let identity = artifact.identity.clone();
        Ok(Executed {
            records,
            request,
            identity,
        })
    }

    /// Run with the default comparison expectation and event capacity.
    pub fn compare(&mut self, label: &str, rows: Vec<ExecutionCase>, fill: u8) -> Result<Executed> {
        self.execute(
            label,
            rows,
            fill,
            Right::Production,
            ComparisonVerdict::Match,
            MAX_EVENTS,
        )
    }

    /// One MATCH request over parts that each set their own stack fill.
    /// Returns every part's tag with its records renumbered from case zero,
    /// so a check written for one part's own request applies unchanged.
    pub fn compare_fills<T>(
        &mut self,
        label: &str,
        parts: Vec<(u8, Vec<ExecutionCase>, T)>,
        maximum: u32,
    ) -> Result<FillResults<T>> {
        self.execute_fills(label, parts, Right::Production, maximum, true)
    }

    /// `compare_fills` for checks that need no guest events: Blobray still
    /// validates every record, but events are not read back.
    pub fn compare_fills_without_events<T>(
        &mut self,
        label: &str,
        parts: Vec<(u8, Vec<ExecutionCase>, T)>,
        maximum: u32,
    ) -> Result<FillResults<T>> {
        self.execute_fills(label, parts, Right::Production, maximum, false)
    }

    /// `compare_fills` for a vendor-only characterization that must complete.
    pub fn characterize_fills<T>(
        &mut self,
        label: &str,
        parts: Vec<(u8, Vec<ExecutionCase>, T)>,
    ) -> Result<FillResults<T>> {
        self.execute_fills(label, parts, Right::None, MAX_EVENTS, true)
    }

    fn execute_fills<T>(
        &mut self,
        label: &str,
        parts: Vec<(u8, Vec<ExecutionCase>, T)>,
        right: Right,
        maximum: u32,
        events: bool,
    ) -> Result<FillResults<T>> {
        let (mut rows, mut counts, mut tags) = (vec![], vec![], vec![]);
        for (fill, part, tag) in parts {
            counts.push(part.len() as u32);
            rows.extend(with_stack_fill(part, fill));
            tags.push(tag);
        }
        let executed = self.run(
            label,
            rows,
            FILLS[0],
            right,
            ComparisonVerdict::Match,
            maximum,
            events,
        )?;
        Ok(FillResults {
            parts: tags
                .into_iter()
                .zip(split_cases(&executed.records, &counts))
                .collect(),
            request: executed.request,
            identity: executed.identity,
        })
    }

    /// Vendor-only characterization that must complete.
    pub fn characterize_vendor(
        &mut self,
        label: &str,
        rows: Vec<ExecutionCase>,
        fill: u8,
    ) -> Result<Executed> {
        self.execute(
            label,
            rows,
            fill,
            Right::None,
            ComparisonVerdict::Match,
            MAX_EVENTS,
        )
    }

    pub fn last_manifest_complete(&self) -> bool {
        self.artifacts.last().expect("retained execution").complete
    }

    /// Exact image bytes at a linked address, checked against the source identity.
    pub fn image_data(&self, name: &str, address: u32, length: u64) -> Result<Vec<u8>> {
        let request = blobray_domain::DataRequest {
            object: self.image_object.clone(),
            symbol: None,
            ranges: vec![DataSelector::Image {
                address: u64::from(address),
                length,
            }],
        };
        Ok(self.session.data(name, &request)?.bytes)
    }

    /// Run the real ROM callback installer with the captured ROM table pointer.
    pub fn install_callbacks(&self, observed_slot: u32) -> Result<Invocation> {
        Ok(self.enter(
            self.root("phy_get_romfunc_addr"),
            &[],
            vec![
                region(
                    layout().rom_interface_pointer,
                    4,
                    &words(&[layout().rom_callback_table]),
                    None,
                    RegionLifetime::Session,
                )?,
                region(
                    layout().rom_parameter_pointer,
                    4,
                    &[],
                    None,
                    RegionLifetime::Session,
                )?,
            ],
            vec![selection(observed_slot, 4)],
            vec![],
        ))
    }
}
