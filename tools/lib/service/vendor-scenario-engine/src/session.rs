//! Shared scenario lifecycle: run directory, linked image, execution
//! submission and failure reports. Capture, linking, data and execution run
//! in this process over the authenticated inputs, identified by content.
use crate::harness::{Budget, Input, ProbeCatalog, Result, invalid, seed};
use blobray_application::data::DataExport;
use blobray_application::in_process::Executable;
use blobray_domain::{
    ArtifactInventory, CallEndpoint, EffectContract, ExecutionEvidence, ExecutionRequest,
    ExecutionTarget, ImageManifest, LayoutProjection, LinkRequest, ReviewedCallBoundary,
};
use blobray_linker::ElfLinker;
use oer_riscv_model::{ArtifactId, CallAbi, ErrorCode, ObjectId, SymbolId, SymbolTableKind};
use oer_vendor_evidence_shard::LocationKind;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// One compared request and its in-memory result.
pub struct Artifact {
    pub label: String,
    /// Digest of the canonical request.
    pub identity: ArtifactId,
    pub request: ExecutionRequest,
    pub records: Vec<ExecutionEvidence>,
    pub verdict: Option<blobray_domain::ComparisonVerdict>,
    pub complete: bool,
    /// Vendor and production entries and the verdict of every compared case.
    pub compared: Vec<ComparedCase>,
    /// Executed production instructions and those a compared observation
    /// depends on.
    pub observed: blobray_application::in_process::ObservedInstructions,
}

/// One claimed pair: `symbol` of vendor `source` at `vendor`, compared with
/// the production probe `entry` at `production`.
struct Claimed<'a> {
    source: &'a str,
    symbol: &'a str,
    vendor: u32,
    entry: &'a str,
    production: u32,
}

/// Evidence entries of one scenario and its untriaged uncovered locations.
#[derive(Default)]
pub struct Claims {
    pub entries: Vec<oer_vendor_evidence_shard::Entry>,
    pub untriaged: std::collections::BTreeSet<oer_vendor_evidence_shard::Location>,
    /// Closure and uncovered locations of every claim.
    pub closures: Vec<crate::coverage::Closure>,
    /// Production PHY lines the claims' executions executed and observed.
    pub lines: crate::observation::Lines,
    /// Vendor bytes a claim's cases write without comparing them, with the
    /// claim's vendor root and production entry.
    pub unprojected: std::collections::BTreeSet<(String, String, crate::state::Byte)>,
    /// SHA-256 of every captured input a fetched pin identifies, by session
    /// input index: compiled production and firmware outputs are built on
    /// the host, and their recipes are the shard's sources instead.
    pub inputs: BTreeMap<String, String>,
    /// What the claims' executions depend on in the production probe.
    pub dependencies: crate::dependencies::Dependencies,
}

/// Production PHY lines of the claims of one scenario.
struct ClaimLines {
    map: crate::observation::LineMap,
    root: PathBuf,
    sources: crate::observation::Sources,
    /// Lines of every claim so far.
    lines: crate::observation::Lines,
    /// Unprojected vendor bytes of every claim so far, with its root and
    /// production entry.
    unprojected: std::collections::BTreeSet<(String, String, crate::state::Byte)>,
}

/// One compared case of a retained execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ComparedCase {
    pub vendor: u32,
    pub production: u32,
    pub verdict: Option<blobray_domain::ComparisonVerdict>,
}

/// Authenticated inputs, their inventory and the probe catalog.
pub struct Session {
    pub run: PathBuf,
    /// Budget of every Blobray operation of the session.
    pub budget: Budget,
    /// Inventory of every captured input, by input index.
    pub inventory: Vec<ArtifactInventory>,
    pub probes: ProbeCatalog,
    pub artifacts: Vec<Artifact>,
    /// Authenticated captured inputs, by input index.
    inputs: Vec<Executable>,
    /// Every linked image, by content.
    images: std::cell::RefCell<BTreeMap<ArtifactId, Executable>>,
    /// Effect contracts and projections the scenario's relations select,
    /// reviewed through git and identified by content.
    effects: Vec<EffectContract>,
    /// The same contracts with their names and rule selections so far.
    reviewed: Vec<crate::rule_use::Reviewed>,
    projections: Vec<LayoutProjection>,
    /// Guest instructions executed in process, and the time spent executing.
    pub executed: std::cell::Cell<(u64, f64)>,
    /// Point mutants of the loaded production image.
    patches: Vec<blobray_application::in_process::ImagePatch>,
}

/// A linked image and its resolved roots, including the entry.
pub struct LinkedImage {
    pub manifest: ImageManifest,
    pub roots: BTreeMap<String, u32>,
}

/// The linked image file of a run's `image` directory.
const IMAGE_ELF: &str = "image.elf";

/// Input index of the compiled production probe ELF.
const PROBE_INPUT: usize = 2;

impl Session {
    /// Retain `value` as the request document `name` next to the evidence.
    pub fn doc<T: serde::Serialize>(&self, name: &str, value: &T) -> Result<PathBuf> {
        write_doc(&self.run, name, value)
    }

    /// Content identity of the captured input `index`.
    pub fn input_id(&self, index: usize) -> Result<ArtifactId> {
        self.inputs
            .get(index)
            .map(|e| e.id().clone())
            .ok_or_else(|| invalid(format!("input {index} is not captured")))
    }

    /// The captured bytes `request` selects, retaining the request as `name`.
    pub fn data(&self, name: &str, request: &blobray_domain::DataRequest) -> Result<DataExport> {
        self.doc(name, request)?;
        let executables = self.executables();
        Ok(blobray_application::data::export(
            request,
            &executables,
            &self.budget.memory()?,
            &mut self.budget.control(),
        )?)
    }

    /// Create a fresh `run-*` directory, authenticate the inputs and read
    /// their inventory and the probe catalog of the compiled production input.
    pub fn start(
        output: &Path,
        budget: Budget,
        inputs: &[Input<'_>],
        scope: &str,
        patches: &[blobray_application::in_process::ImagePatch],
    ) -> Result<Self> {
        let run = start_run(output)?;
        let (identities, contents) = crate::harness::authenticate(inputs)?;
        let roles: Vec<_> = inputs.iter().map(|i| i.role).collect();
        write_doc(
            &run,
            "identities",
            &serde_json::json!({"sha256": identities, "roles": roles, "scope": scope}),
        )?;
        let inputs: Vec<Executable> = contents.into_iter().map(Executable::new).collect();
        let inventory = inputs
            .iter()
            .map(|input| {
                blobray_application::captured::inventory(
                    input,
                    &budget.memory()?,
                    &mut budget.control(),
                )
            })
            .collect::<oer_riscv_model::Result<Vec<_>>>()?;
        let probes = ProbeCatalog::capture(&inventory, &inputs, PROBE_INPUT, &budget)?;
        if let Some(rom) = roles.iter().position(|role| *role == "rom") {
            verify_rom_symbols(&inventory, rom)?;
        }
        Ok(Self {
            run,
            budget,
            inventory,
            probes,
            artifacts: vec![],
            inputs,
            images: Default::default(),
            effects: vec![],
            reviewed: vec![],
            projections: vec![],
            executed: Default::default(),
            patches: patches.to_vec(),
        })
    }

    /// Exact code endpoint of the linked image symbol `name` at `address`
    /// in the image `object`.
    pub fn image_endpoint(
        &self,
        object: &ObjectId,
        name: &str,
        address: u32,
    ) -> Result<CallEndpoint> {
        let symbol = image_symbol_id(&self.run.join("image").join(IMAGE_ELF), object, name)?;
        Ok(CallEndpoint {
            object: object.clone(),
            symbol: Some(symbol),
            boundary: ReviewedCallBoundary::Code { address },
        })
    }

    /// Exact code endpoint of the captured symbol `name` in `input`.
    pub fn input_endpoint(&self, input: u64, name: &str) -> Result<CallEndpoint> {
        let record = crate::harness::symbol(&self.inventory, input as usize, name)?;
        Ok(CallEndpoint {
            object: record.id.object.clone(),
            symbol: Some(record.id.clone()),
            boundary: ReviewedCallBoundary::Code {
                address: u32::try_from(record.value)?,
            },
        })
    }

    /// Select `contract` by content for this scenario's relations. The
    /// contract is a typed value reviewed through git; `name`, `subject` and
    /// `reason` document it and are retained with the run.
    pub fn review_effects(
        &mut self,
        name: &str,
        subject: &str,
        contract: EffectContract,
        reason: &str,
    ) -> Result<ArtifactId> {
        self.doc(
            name,
            &serde_json::json!({"subject": subject, "reason": reason, "contract": &contract}),
        )?;
        let selection = blobray_application::in_process::effect_contract_id(&contract)?;
        if !self.effects.contains(&contract) {
            self.reviewed.push(crate::rule_use::Reviewed::new(
                name,
                selection.clone(),
                contract.clone(),
            ));
            self.effects.push(contract);
        }
        Ok(selection)
    }

    /// Digest of the reviewed content of the selected contract: its rules,
    /// classification, claim ceiling, applicability and reason. The call
    /// endpoints name the linked images by content, which changes with any
    /// linked input, so they stay out and the digest changes only with the
    /// reviewed content.
    fn review_digest(&self, selected: &oer_riscv_model::ArtifactId) -> Result<String> {
        for contract in &self.effects {
            let id = blobray_application::in_process::effect_contract_id(contract)?;
            if &id == selected {
                let reviewed = serde_json::json!({
                    "rules": contract.rules,
                    "unclassified": contract.unclassified,
                    "claim-ceiling": contract.claim_ceiling,
                    "applicability": contract.applicability,
                    "reason": contract.reason,
                });
                return Ok(oer_durable::sha256_bytes(&serde_json::to_vec(&reviewed)?));
            }
        }
        Err(invalid(format!(
            "no reviewed contract has identity {}",
            selected.as_str()
        )))
    }

    /// Digest of the reviewed content of the selected projection: its fields,
    /// branches, applicability and reason, without the endpoints that name
    /// the linked images by content.
    fn projection_digest(&self, selected: &oer_riscv_model::ArtifactId) -> Result<String> {
        for projection in &self.projections {
            let id = blobray_application::in_process::projection_id(projection)?;
            if &id == selected {
                let reviewed = serde_json::json!({
                    "fields": projection.fields,
                    "branches": projection.branches,
                    "applicability": projection.applicability,
                    "reason": projection.reason,
                });
                return Ok(oer_durable::sha256_bytes(&serde_json::to_vec(&reviewed)?));
            }
        }
        Err(invalid(format!(
            "no reviewed projection has identity {}",
            selected.as_str()
        )))
    }

    /// Select `projection` by content, as `review_effects` does for contracts.
    pub fn review_projection(
        &mut self,
        name: &str,
        subject: &str,
        projection: LayoutProjection,
        reason: &str,
    ) -> Result<ArtifactId> {
        self.doc(
            name,
            &serde_json::json!({"subject": subject, "reason": reason, "projection": &projection}),
        )?;
        let selection = blobray_application::in_process::projection_id(&projection)?;
        if !self.projections.contains(&projection) {
            self.projections.push(projection);
        }
        Ok(selection)
    }

    /// Every executable a request may name: the captured inputs and the
    /// exported images.
    fn executables(&self) -> Vec<Executable> {
        self.inputs
            .iter()
            .chain(self.images.borrow().values())
            .cloned()
            .collect()
    }

    /// The executables `target` maps, in load order.
    fn target_executables(&self, target: &ExecutionTarget) -> Vec<Executable> {
        let executables = self.executables();
        target
            .executables
            .iter()
            .filter_map(|id| executables.iter().find(|e| e.id() == id).cloned())
            .collect()
    }

    /// Execute and compare `request` in this process.
    fn verify(
        &self,
        request: &ExecutionRequest,
    ) -> oer_riscv_model::Result<blobray_application::in_process::InProcessResult> {
        let executables = self.executables();
        let memory = self.budget.memory()?;
        blobray_application::in_process::verify(
            &blobray_application::in_process::InProcessComparison {
                request,
                executables: &executables,
                effects: &self.effects,
                projections: &self.projections,
                vendor_results: None,
                dependence: Some(&oer_riscv_lift::RiscvDecoder),
                patches: &self.patches,
            },
            crate::chip().isa.executor(),
            &memory,
            &mut self.budget.control(),
        )
    }

    /// Every companion the request's closure needs, proposed by a trial link
    /// against the inputs `candidates` in priority order. An unresolved name
    /// fails.
    pub fn propose(
        &self,
        request: &LinkRequest,
        linker: &Path,
        candidates: &[u64],
    ) -> Result<Vec<SymbolId>> {
        self.doc("propose", request)?;
        let candidates = candidates
            .iter()
            .map(|index| self.input_id(*index as usize))
            .collect::<Result<Vec<_>>>()?;
        let proposal = blobray_application::linking::propose_companions(
            request,
            &candidates,
            &self.inputs,
            &std::path::absolute(linker)?,
            &ElfLinker,
            &self.run,
            &self.budget.memory()?,
            &mut self.budget.control(),
        )?;
        // The proposal is retained even when names stay unresolved.
        fs::write(
            self.run.join("propose.json"),
            serde_json::to_vec(&proposal)?,
        )?;
        if !proposal.unresolved.is_empty() {
            return Err(invalid(format!(
                "unresolved link names: {}",
                proposal.unresolved.join(", ")
            )));
        }
        Ok(proposal.resolved.into_iter().map(|c| c.symbol).collect())
    }

    /// Complete `request` with proposed companions from the inputs
    /// `candidates`, then link one image into the run's `image` directory with
    /// its link map. `entry` names the request's entry so it resolves like
    /// the other roots. The retained plan request lists every exact
    /// companion.
    pub fn link(
        &self,
        request: &LinkRequest,
        linker: &Path,
        entry: &str,
        candidates: &[u64],
    ) -> Result<LinkedImage> {
        let mut request = request.clone();
        request
            .companions
            .extend(self.propose(&request, linker, candidates)?);
        self.doc("plan", &request)?;
        let linked = blobray_application::linking::link(
            &request,
            &self.inputs,
            &std::path::absolute(linker)?,
            &ElfLinker,
            &self.run,
            &self.budget.memory()?,
            &mut self.budget.control(),
        )?;
        let directory = self.run.join("image");
        fs::create_dir_all(&directory)?;
        fs::write(directory.join(IMAGE_ELF), linked.elf.bytes())?;
        fs::write(directory.join(crate::state::LINK_MAP), &linked.map)?;
        fs::write(
            directory.join("manifest.json"),
            serde_json::to_vec_pretty(&linked.manifest)?,
        )?;
        let mut roots = BTreeMap::new();
        for root in &linked.manifest.roots {
            roots.insert(
                String::from_utf8(root.name.clone())?,
                u32::try_from(root.address)?,
            );
        }
        roots.insert(entry.to_owned(), u32::try_from(linked.manifest.entry)?);
        self.images
            .borrow_mut()
            .insert(linked.manifest.elf.clone(), linked.elf);
        Ok(LinkedImage {
            manifest: linked.manifest,
            roots,
        })
    }

    /// The linked vendor image with ROM and production companions, and the
    /// compiled production input with the ROM companion. Stack bytes stay
    /// unknown until a request selects a fill.
    pub fn targets(&self, image: &ArtifactId) -> Result<(ExecutionTarget, ExecutionTarget)> {
        let stack = seed(crate::chip().stack.0, crate::chip().stack.1, &[], None)?;
        if !self.images.borrow().contains_key(image) {
            return Err(invalid("image is not linked"));
        }
        let image = image.clone();
        let input = |index: usize| self.input_id(index);
        Ok((
            ExecutionTarget {
                executables: vec![image, input(1)?, input(2)?],
                abi: CallAbi::RiscvInteger,
                stack: stack.clone(),
            },
            ExecutionTarget {
                executables: vec![input(2)?, input(1)?],
                abi: CallAbi::RiscvInteger,
                stack,
            },
        ))
    }

    /// Submit a request, read its evidence, check the verdict and retain it.
    pub fn submit(
        &mut self,
        label: &str,
        request: &ExecutionRequest,
        verdict: Option<blobray_domain::ComparisonVerdict>,
    ) -> Result<&Artifact> {
        self.submit_records(label, request, verdict, true)
    }

    /// `submit`, reading guest event records only when `events` is set.
    pub fn submit_records(
        &mut self,
        label: &str,
        request: &ExecutionRequest,
        verdict: Option<blobray_domain::ComparisonVerdict>,
        events: bool,
    ) -> Result<&Artifact> {
        // Compared cases report the persistent vendor bytes they write.
        let mut request = request.clone();
        for case in &mut request.cases {
            if case.relation.is_some() && case.replacement.is_some() {
                case.vendor.observe_timeline.written = true;
            }
        }
        let request = &request;
        let started = std::time::Instant::now();
        let result = self
            .verify(request)
            .map_err(|e| invalid(format!("{label}: {e:?}")))?;
        let seconds = started.elapsed().as_secs_f64();
        println!("{label} 0 {seconds:.2}");
        let steps: u64 = result
            .records
            .iter()
            .map(|r| match r {
                ExecutionEvidence::Outcome { steps, .. } => *steps,
                _ => 0,
            })
            .sum();
        let (total, time) = self.executed.get();
        self.executed.set((total + steps, time + seconds));
        crate::rule_use::count(&mut self.reviewed, request, &result.records)?;
        if result.verdict != verdict {
            // Every departing case, both sides' events and their contract
            // classification, beside the run.
            let contracts = &self.effects;
            let report = crate::failure::cases(request, &result.records, verdict, |case| {
                let selected = request
                    .cases
                    .get(case as usize)?
                    .relation
                    .as_ref()?
                    .effects
                    .as_ref()?;
                contracts
                    .iter()
                    .find(|contract| {
                        blobray_application::in_process::effect_contract_id(contract)
                            .is_ok_and(|reference| &reference == selected)
                    })
                    .cloned()
            })
            .and_then(|cases| {
                // Every undeclared access, not only the first one per side.
                let missing = self.discover(request, &result.records)?;
                crate::failure::write(&self.run, label, verdict, &cases, &missing)
            })
            .map_or_else(
                |e| format!("no failure report: {e}"),
                |path| path.display().to_string(),
            );
            // Name the first case that departs from the expected verdict.
            let departing = result.records.iter().find_map(|r| match r {
                ExecutionEvidence::Comparison {
                    case,
                    result: compared,
                } if Some(compared.verdict) != verdict => Some((*case, compared.clone())),
                _ => None,
            });
            // The report holds both sides' stops, unfinished models and
            // classified events; the panic only points at it.
            match departing {
                Some((case, compared)) => panic!(
                    "{label}: {:?}, expected {verdict:?}; first departing case {case} `{}`: {:?}, difference {:x?}; report {report}",
                    result.verdict,
                    request.cases[case as usize].name,
                    compared.verdict,
                    compared.difference
                ),
                None => panic!(
                    "{label}: {:?}, expected {verdict:?}; report {report}",
                    result.verdict
                ),
            }
        }
        let identity = ArtifactId::of_bytes(&serde_json::to_vec(request)?);
        let records: Vec<ExecutionEvidence> = if events {
            result.records
        } else {
            result
                .records
                .into_iter()
                .filter(|r| !matches!(r, ExecutionEvidence::Event { .. }))
                .collect()
        };
        let compared = request
            .cases
            .iter()
            .enumerate()
            .filter(|(_, c)| c.relation.is_some())
            .filter_map(|(index, c)| {
                let production = c.replacement.as_ref()?.entry;
                let verdict = records.iter().find_map(|r| match r {
                    ExecutionEvidence::Comparison { case, result } if *case == index as u32 => {
                        Some(result.verdict)
                    }
                    _ => None,
                });
                Some(ComparedCase {
                    vendor: c.vendor.entry,
                    production,
                    verdict,
                })
            })
            .collect();
        self.artifacts.push(Artifact {
            label: label.into(),
            identity,
            request: request.clone(),
            records,
            verdict: result.verdict,
            complete: result.complete,
            compared,
            observed: result.observed.unwrap_or_default(),
        });
        Ok(self.artifacts.last().unwrap())
    }

    /// The evidence entry of one claim: `symbol` of vendor `source` at
    /// `vendor` compared with production `entry` at `production`. Only
    /// executions whose every case of that pair is MATCH count; a claim
    /// without one such execution fails.
    fn claim(
        &self,
        suite: &str,
        claimed: Claimed<'_>,
        decisions: &[crate::coverage::Decision],
        observed: &mut crate::coverage::Observed,
        lines: &mut ClaimLines,
    ) -> Result<oer_vendor_evidence_shard::Entry> {
        let Claimed {
            source,
            symbol,
            vendor,
            entry,
            production,
        } = claimed;
        let mut cases = 0;
        let mut executions = vec![];
        // Executions whose request differs where the scenario expected it
        // to: they cover the vendor code they ran, but carry no verdict.
        let mut differing = vec![];
        let mut reviews = std::collections::BTreeSet::new();
        for artifact in &self.artifacts {
            let selected: Vec<_> = artifact
                .compared
                .iter()
                .filter(|c| c.vendor == vendor && c.production == production)
                .collect();
            if selected.is_empty() {
                continue;
            }
            if selected
                .iter()
                .any(|c| c.verdict != Some(blobray_domain::ComparisonVerdict::Match))
            {
                // `submit` rejects a verdict other than the expected one.
                if artifact.verdict == Some(blobray_domain::ComparisonVerdict::Diff)
                    && selected.iter().all(|c| {
                        matches!(
                            c.verdict,
                            Some(
                                blobray_domain::ComparisonVerdict::Match
                                    | blobray_domain::ComparisonVerdict::Diff
                            )
                        )
                    })
                {
                    differing.push(artifact.identity.as_str().to_owned());
                }
                continue;
            }
            cases += selected.len() as u64;
            executions.push(artifact.identity.as_str().to_owned());
            // Contracts and projections the pair's comparisons selected.
            for case in &artifact.request.cases {
                let Some(relation) = &case.relation else {
                    continue;
                };
                if case.vendor.entry != vendor
                    || case.replacement.as_ref().map(|r| r.entry) != Some(production)
                {
                    continue;
                }
                if let Some(contract) = &relation.effects {
                    reviews.insert(self.review_digest(contract)?);
                }
                if let Some(projection) = &relation.projection {
                    reviews.insert(self.projection_digest(projection)?);
                }
            }
        }
        if executions.is_empty() {
            return Err(invalid(format!(
                "{suite}: no MATCH execution compares {symbol} with {entry}"
            )));
        }
        let selected: Vec<&Artifact> = self
            .artifacts
            .iter()
            .filter(|a| executions.contains(&a.identity.as_str().to_owned()))
            .collect();
        let pairs: Vec<(ArtifactId, &ExecutionRequest, &[ExecutionEvidence])> = self
            .artifacts
            .iter()
            .filter(|a| {
                let identity = a.identity.as_str().to_owned();
                executions.contains(&identity)
                    || (differing.contains(&identity)
                        && a.request.vendor == selected[0].request.vendor)
            })
            .map(|a| (a.identity.clone(), &a.request, a.records.as_slice()))
            .collect();
        let executables = self.target_executables(&selected[0].request.vendor);
        let memory = self.budget.memory()?;
        let report = blobray_application::in_process::coverage(
            &pairs,
            &executables,
            &oer_riscv_lift::RiscvDecoder,
            &memory,
            &mut self.budget.control(),
        )
        .map_err(|e| invalid(format!("{suite} {symbol} coverage: {e:?}")))?;
        let root = report
            .roots
            .iter()
            .find(|r| r.entry == vendor)
            .ok_or_else(|| invalid(format!("{suite}: {symbol} is not a coverage root")))?;
        // Uncovered locations by absolute address and kind: a block shared by
        // several closure functions is named once, by the lowest entry.
        let mut located = BTreeMap::new();
        let mut functions = std::collections::BTreeSet::new();
        for function in report
            .functions
            .iter()
            .filter(|f| root.functions.contains(&f.entry))
        {
            let name = function
                .name
                .clone()
                .unwrap_or_else(|| format!("{:#x}", function.entry));
            observed.functions.insert(name.clone());
            functions.insert(name.clone());
            let mut add = |address: u32, kind: LocationKind| {
                located.entry((address, kind)).or_insert_with(|| {
                    crate::coverage::location(&name, function.entry, address, kind)
                });
            };
            for block in &function.uncovered_blocks {
                add(*block, LocationKind::Block);
            }
            for site in &function.followed {
                add(*site, LocationKind::Followed);
            }
            for site in &function.unresolved {
                add(*site, LocationKind::Unresolved);
            }
            for direction in &function.uncovered_directions {
                add(
                    direction.site,
                    if direction.taken {
                        LocationKind::Taken
                    } else {
                        LocationKind::Fallthrough
                    },
                );
            }
        }
        let locations: std::collections::BTreeSet<_> = located.into_values().collect();
        // Closure functions only excluded code reaches share its exclusion.
        let closure_functions: Vec<_> = report
            .functions
            .iter()
            .filter(|f| root.functions.contains(&f.entry))
            .collect();
        let named = |entry: u32| {
            closure_functions
                .iter()
                .find(|f| f.entry == entry)
                .map(|f| f.name.clone().unwrap_or_else(|| format!("{entry:#x}")))
        };
        let graph: BTreeMap<String, crate::coverage::CallNode> = closure_functions
            .iter()
            .map(|f| {
                (
                    f.name.clone().unwrap_or_else(|| format!("{:#x}", f.entry)),
                    crate::coverage::CallNode {
                        entered: f.blocks.reached > 0,
                        callees: f.callees.iter().filter_map(|c| named(*c)).collect(),
                    },
                )
            })
            .collect();
        let root_name = named(vendor).unwrap_or_else(|| format!("{vendor:#x}"));
        let consequences = crate::coverage::consequences(decisions, &root_name, &graph);
        let consequential: std::collections::BTreeSet<_> = locations
            .iter()
            .filter(|l| consequences.contains(&l.function))
            .cloned()
            .collect();
        let (excluded, untriaged) = crate::coverage::Observed::classify(decisions, &locations);
        let (excluded, untriaged): (std::collections::BTreeSet<_>, std::collections::BTreeSet<_>) = (
            excluded
                .into_iter()
                .chain(
                    untriaged
                        .iter()
                        .filter(|l| consequential.contains(*l))
                        .cloned(),
                )
                .collect(),
            untriaged
                .into_iter()
                .filter(|l| !consequential.contains(l))
                .collect(),
        );
        let count = |c: &blobray_domain::CoverageCount| oer_vendor_evidence_shard::Count {
            reached: c.reached,
            total: c.total,
        };
        let open = locations
            .iter()
            .filter(|l| matches!(l.kind, LocationKind::Followed | LocationKind::Unresolved))
            .count() as u64;
        let coverage = oer_vendor_evidence_shard::Coverage {
            blocks: count(&root.blocks),
            directions: count(&root.directions),
            open,
            excluded: excluded.len() as u64,
            untriaged: untriaged.len() as u64,
        };
        observed.uncovered.extend(locations.iter().cloned());
        let mut per_function = BTreeMap::<String, usize>::new();
        for location in &untriaged {
            *per_function.entry(location.function.clone()).or_default() += 1;
        }
        const GATEWAYS: usize = 5;
        for gateway in crate::coverage::gateways(&root_name, &graph, &per_function)
            .into_iter()
            .take(GATEWAYS)
        {
            observed.gateways.push(format!(
                "{symbol} -> {entry}: {} leads alone to {} untriaged locations in {} functions",
                gateway.function, gateway.locations, gateway.functions
            ));
        }
        observed.closures.push(crate::coverage::Closure {
            functions,
            uncovered: locations,
            consequential,
        });
        let mut instructions = blobray_application::in_process::ObservedInstructions::default();
        for artifact in &selected {
            let o = &artifact.observed;
            instructions.executed.extend(&o.executed);
            instructions.observed.extend(&o.observed);
            instructions.effect.extend(&o.effect);
            instructions.state.extend(&o.state);
        }
        // Vendor state the pair's cases write without comparing it.
        let sections = std::fs::read_to_string(self.run.join("image").join(crate::state::LINK_MAP))
            .map(|map| crate::state::link_map_sections(&map))
            .unwrap_or_default();
        let vendor_bytes: Vec<&[u8]> = executables.iter().map(Executable::bytes).collect();
        let symbols = crate::state::Symbols::of(&vendor_bytes)?.with_sections(&sections);
        let (mut written, mut unprojected) = (
            std::collections::BTreeSet::new(),
            std::collections::BTreeSet::new(),
        );
        for artifact in &selected {
            let cases: std::collections::BTreeSet<u32> = artifact
                .request
                .cases
                .iter()
                .enumerate()
                .filter(|(_, c)| {
                    c.relation.is_some()
                        && c.vendor.entry == vendor
                        && c.replacement.as_ref().map(|r| r.entry) == Some(production)
                })
                .map(|(i, _)| i as u32)
                .collect();
            let (w, u) = crate::state::written(
                &artifact.request.cases,
                &cases,
                &artifact.records,
                &self.projections,
            )?;
            written.extend(w.into_iter().map(|a| symbols.name(a)));
            unprojected.extend(u.into_iter().map(|a| symbols.name(a)));
        }
        let compared = written.len() - unprojected.len();
        let (reviewed_state, untriaged_state) =
            crate::state::classify(crate::chip().state, (symbol, entry), &unprojected);
        let state = oer_vendor_evidence_shard::State {
            written: written.len() as u64,
            compared: compared as u64,
            reviewed: reviewed_state.len() as u64,
            untriaged: untriaged_state.len() as u64,
        };
        lines.unprojected.extend(
            unprojected
                .into_iter()
                .map(|b| (symbol.to_owned(), entry.to_owned(), b)),
        );
        let claim_lines = lines.map.lines(&instructions);
        let (reviewed, untriaged) = lines.sources.classify(
            &lines.root,
            crate::chip().observation,
            &claim_lines.unobserved(),
        )?;
        let observation = oer_vendor_evidence_shard::Observation {
            executed: claim_lines.executed.len() as u64,
            observed: claim_lines.observed.len() as u64,
            reviewed: reviewed.len() as u64,
            untriaged: untriaged.len() as u64,
        };
        lines.lines.extend(&claim_lines);
        Ok(oer_vendor_evidence_shard::Entry {
            suite: suite.into(),
            source: source.into(),
            symbol: symbol.into(),
            production: entry.into(),
            verdict: oer_vendor_evidence_shard::MATCH.into(),
            cases,
            reviews: reviews.into_iter().collect(),
            coverage: Some(coverage),
            observation: Some(observation),
            state: Some(state),
        })
    }

    /// Evidence entries of `list` (vendor source, root symbol, production
    /// probe): archive roots resolve in `roots`, ROM roots in the ROM input.
    /// Coverage of every claimed root closure is classified under
    /// `decisions`; the complete run checks that each still excludes
    /// something in some scenario.
    /// Addresses of each defined function symbol of the linked image, by
    /// name: several for a name that static functions share.
    fn image_functions(&self) -> Result<BTreeMap<String, Vec<u32>>> {
        let elf = fs::read(self.run.join("image").join(IMAGE_ELF))?;
        let file = oer_elf::Elf::parse(&elf)?;
        let mut functions = BTreeMap::<String, Vec<u32>>::new();
        for symbol in file.symbols() {
            if symbol.kind == oer_elf::SymbolKind::Text && symbol.size > 0 && symbol.defined {
                functions
                    .entry(symbol.name.to_owned())
                    .or_default()
                    .push(u32::try_from(symbol.address)?);
            }
        }
        Ok(functions)
    }

    pub fn claims(
        &self,
        suite: &str,
        roots: &BTreeMap<String, u32>,
        list: &[(&str, &str, &str)],
        decisions: &[crate::coverage::Decision],
        report: &dyn crate::findings::RunReport,
    ) -> Result<Claims> {
        let mut observed = crate::coverage::Observed::default();
        let executed: std::collections::BTreeSet<u32> = self
            .artifacts
            .iter()
            .flat_map(|a| a.observed.executed.iter().copied())
            .collect();
        let reads: std::collections::BTreeSet<(u32, u8)> = self
            .artifacts
            .iter()
            .flat_map(|a| a.observed.reads.iter().copied())
            .collect();
        for line in crate::mutant::report(&self.patches, &executed) {
            println!("{suite} {line}");
        }
        let root = oer_process::built_root();
        let dependencies =
            crate::dependencies::of(self.inputs[PROBE_INPUT].bytes(), &executed, &reads, &root)?;
        let mut lines = ClaimLines {
            map: crate::observation::LineMap::new(
                self.inputs[PROBE_INPUT].bytes(),
                &executed,
                &root,
            )?,
            root,
            sources: Default::default(),
            lines: Default::default(),
            unprojected: Default::default(),
        };
        let functions = self.image_functions()?;
        // The (vendor, production) addresses of every claim: a compared pair
        // is matched by address, so an unnamed ROM function is claimed too.
        let mut claimed = std::collections::BTreeSet::new();
        let entries = list
            .iter()
            .map(|(source, symbol, production)| {
                let vendor = match *source {
                    // A static function the cases enter at its linked address
                    // is claimed there too.
                    "archive" => match roots.get(*symbol) {
                        Some(address) => *address,
                        None => match functions.get(*symbol).map(Vec::as_slice) {
                            Some([address]) => *address,
                            _ => {
                                return Err(invalid(format!(
                                    "{symbol} is neither a linked root nor one function of the image"
                                )));
                            }
                        },
                    },
                    "rom" => u32::try_from(
                        crate::harness::symbol(
                            &self.inventory,
                            crate::chip().rom_input as usize,
                            symbol,
                        )?
                        .value,
                    )?,
                    other => return Err(invalid(format!("unknown vendor source {other}"))),
                };
                let address = self.probes.entry(production)?;
                claimed.insert((vendor, address));
                self.claim(
                    suite,
                    Claimed {
                        source,
                        symbol,
                        vendor,
                        entry: production,
                        production: address,
                    },
                    decisions,
                    &mut observed,
                    &mut lines,
                )
            })
            .collect::<Result<Vec<_>>>()?;
        // Every pair a case compares is a claim; a pair no claim names has
        // its coverage and verdicts dropped from the evidence.
        let names: BTreeMap<u32, &str> = functions
            .iter()
            .flat_map(|(name, addresses)| addresses.iter().map(|a| (*a, name.as_str())))
            .chain(
                roots
                    .iter()
                    .map(|(name, address)| (*address, name.as_str())),
            )
            .collect();
        let probes: BTreeMap<u32, &str> = self
            .probes
            .entries()
            .map(|(name, address)| (address, name))
            .collect();
        let name = |names: &BTreeMap<u32, &str>, address: u32| {
            names
                .get(&address)
                .map_or_else(|| format!("{address:#x}"), |n| (*n).to_owned())
        };
        let unclaimed: std::collections::BTreeSet<(String, String)> = self
            .artifacts
            .iter()
            .flat_map(|artifact| artifact.compared.iter())
            // Comparisons whose production side is no probe entry prepare
            // state, such as ROM copies on both sides.
            .filter(|pair| probes.contains_key(&pair.production))
            // A probe compared with another probe checks the harness, not
            // a vendor root.
            .filter(|pair| !probes.contains_key(&pair.vendor))
            .filter(|pair| !claimed.contains(&(pair.vendor, pair.production)))
            .map(|pair| (name(&names, pair.vendor), name(&probes, pair.production)))
            .collect();
        let (steps, seconds) = self.executed.get();
        println!(
            "{suite} interpreter {steps} guest instructions in {seconds:.2} s ({:.1} M/s)",
            steps as f64 / seconds.max(f64::EPSILON) / 1e6
        );
        let (_, untriaged) = crate::coverage::Observed::classify(decisions, &observed.uncovered);
        let image = std::fs::read(self.run.join("image/image.elf")).unwrap_or_default();
        let rom = self
            .inputs
            .get(crate::chip().rom_input as usize)
            .map_or(&[][..], Executable::bytes);
        let listed = crate::coverage::uncovered_everywhere(&observed.closures, untriaged.clone());
        let consequential: std::collections::BTreeSet<_> = observed
            .closures
            .iter()
            .flat_map(|closure| closure.consequential.iter().cloned())
            .collect();
        let untriaged_findings = if untriaged.is_empty() {
            None
        } else {
            Some(crate::findings::Untriaged {
                images: [&image, rom],
                decisions,
                listed: &listed,
                uncovered: &observed.uncovered,
                consequential: &consequential,
            })
        };
        report.report(&crate::findings::Findings {
            suite,
            directory: &self.run,
            unclaimed: &unclaimed,
            gateways: &observed.gateways,
            untriaged: untriaged_findings,
        })?;
        let rules = crate::rule_use::check(&self.run, suite, &self.reviewed)?;
        println!("{suite} effect rule selections: {}", rules.display());
        Ok(Claims {
            entries,
            untriaged,
            closures: observed.closures,
            lines: lines.lines,
            unprojected: lines.unprojected,
            inputs: self
                .inputs
                .iter()
                .enumerate()
                .map(|(index, input)| (format!("input-{index}"), input.id().as_str().to_owned()))
                .filter(|(_, digest)| crate::artifacts::is_fetched_pin(digest))
                .collect(),
            dependencies,
        })
    }

    /// Every undeclared access of `request`, whose run gave `records`.
    fn discover(
        &self,
        request: &ExecutionRequest,
        records: &[ExecutionEvidence],
    ) -> Result<Vec<crate::discovery::Missing>> {
        let images = self.images.borrow();
        let elfs: Vec<&[u8]> = images
            .values()
            .chain(&self.inputs)
            .map(Executable::bytes)
            .collect();
        let registers = crate::registers::Registers::load(
            &oer_process::built_root().join(crate::chip().registers),
        )?;
        let symbols = crate::discovery::Symbols::of(&elfs, registers);
        Ok(crate::discovery::discover(
            &request.cases,
            records,
            &symbols,
            |cases| {
                let mut rerun = request.clone();
                rerun.cases = cases.to_vec();
                self.verify(&rerun)
                    .map(|result| result.records)
                    .map_err(|e| invalid(format!("{e:?}")))
            },
        ))
    }

    /// Run a request that must fail for capacity.
    pub fn capacity_failure(&self, label: &str, request: &ExecutionRequest) -> Result<()> {
        let failed = self
            .verify(request)
            .err()
            .ok_or_else(|| invalid(format!("{label}: capacity was not exhausted")))?;
        assert_eq!(
            failed.code,
            ErrorCode::ResourceLimited,
            "{label}: {failed:?}"
        );
        Ok(())
    }
}

/// The named ROM storage constants are the pinned ROM's own symbols.
pub fn verify_rom_symbols(inventory: &[ArtifactInventory], rom: usize) -> Result<()> {
    for &(name, address, size) in crate::chip().rom_symbols {
        let symbol = crate::harness::symbol(inventory, rom, name)?;
        if symbol.value != u64::from(address) || symbol.size != size {
            return Err(invalid(format!(
                "ROM symbol {name} differs from its named address"
            )));
        }
    }
    Ok(())
}

/// Write `value` as the request document `name` in `run`.
fn write_doc<T: serde::Serialize>(run: &Path, name: &str, value: &T) -> Result<PathBuf> {
    let path = run.join(format!("{name}.request.json"));
    fs::write(&path, serde_json::to_vec(value)?)?;
    Ok(path)
}

/// Create a fresh `run-*` directory below `output` and record it as `latest`.
pub fn start_run(output: &Path) -> Result<PathBuf> {
    fs::create_dir_all(output)?;
    let run = tempfile::Builder::new()
        .prefix("run-")
        .tempdir_in(std::path::absolute(output)?)?
        .keep();
    fs::write(output.join("latest"), run.as_os_str().as_encoded_bytes())?;
    Ok(run)
}

/// Resolve one uniquely named defined symbol in an exported image and retain
/// the complete defined-symbol listing next to the evidence.
pub fn image_symbol(elf: &Path, listing: &Path, name: &str) -> Result<(u32, u64)> {
    let bytes = fs::read(elf)?;
    let file = oer_elf::Elf::parse(&bytes)?;
    let mut lines = String::new();
    let mut matches = vec![];
    for symbol in file.symbols().filter(|s| s.defined) {
        let symbol_name = symbol.name;
        if symbol_name.is_empty() {
            continue;
        }
        lines.push_str(&format!(
            "{symbol_name} {:x} {:x}\n",
            symbol.address, symbol.size
        ));
        if symbol_name == name {
            matches.push((u32::try_from(symbol.address)?, symbol.size));
        }
    }
    fs::write(listing, lines)?;
    match matches[..] {
        [one] => Ok(one),
        _ => Err(invalid(format!(
            "{name}: {} image definitions",
            matches.len()
        ))),
    }
}

/// Static-table identity of the unique defined `name` in an exported image
/// ELF whose payload is `object`.
pub fn image_symbol_id(elf: &Path, object: &ObjectId, name: &str) -> Result<SymbolId> {
    let bytes = fs::read(elf)?;
    let file = oer_elf::Elf::parse(&bytes)?;
    let table = file
        .section_by_name(".symtab")
        .ok_or_else(|| invalid("image has no static symbol table"))?;
    let matches: Vec<_> = file
        .symbols()
        .filter(|s| s.defined && s.name == name)
        .collect();
    match matches[..] {
        [ref one] => Ok(SymbolId {
            object: object.clone(),
            table: SymbolTableKind::Static,
            table_section: u32::try_from(table.index)?,
            index: one.index as u64,
        }),
        _ => Err(invalid(format!(
            "{name}: {} image definitions",
            matches.len()
        ))),
    }
}

/// Request over explicit targets; `fill` initializes both stacks when given.
pub fn request(
    vendor: &ExecutionTarget,
    replacement: Option<&ExecutionTarget>,
    fill: Option<u8>,
    cases: Vec<blobray_domain::ExecutionCase>,
    max_events: u32,
) -> ExecutionRequest {
    let with_fill = |target: &ExecutionTarget| {
        let mut target = target.clone();
        if fill.is_some() {
            target.stack.fill = fill;
        }
        target
    };
    ExecutionRequest {
        schema: blobray_domain::EXECUTION_SCHEMA,
        vendor: with_fill(vendor),
        binding: replacement.map(|_| blobray_domain::CompiledBinding::SharedCore),
        replacement: replacement.map(with_fill),
        cases,
        max_events,
    }
}
