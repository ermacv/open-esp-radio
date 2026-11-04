//! Conditional observations of the pinned multicast BA caller's scalar
//! pointers and the HAL leaf's array contract. This is vendor-only execution,
//! without a production equivalence or on-air correctness claim.
use crate::esp32s31::{artifacts, engine::layout};
use crate::harness::{Budget, Input, Result, authenticate, invalid, symbol};
use oer_tools_analysis_blobray_application::{
    captured,
    in_process::{Executable, InProcessComparison, verify},
    linking,
};
use oer_tools_analysis_blobray_domain::*;
use oer_tools_analysis_blobray_linker::ElfLinker;
use oer_tools_analysis_blobray_riscv::RiscvExecutor;
use oer_tools_registers_model::{Chip, chip_model::Location};
use serde::Serialize;
use std::{fs, path::PathBuf, process::ExitCode};

const CALLER: &str = "ampdu_process_multicast_address_qos_frame";
const CALLEE: &str = "hal_ba_session_restore_by_hw_index";
// Synthetic normal-memory placements, not chip RAM claims.
const STATE: u32 = 0x1200_0000;
const ARRAYS: u32 = 0x1200_1000;
const STACK: u32 = 0x1300_0000;
const FILL: u32 = 0xa5a5_a5a5;

#[derive(clap::Args)]
pub struct Options {
    #[arg(long, default_value_os_t = artifacts::default_path("libpp"))]
    libpp: PathBuf,
    #[arg(long, default_value_os_t = artifacts::default_path("libnet80211"))]
    libnet80211: PathBuf,
    #[arg(long)]
    linker: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[command(flatten)]
    budget: Budget,
}

#[derive(Clone, Copy)]
struct Bank {
    control: u32,
    sequence: u32,
    low: u32,
    high: u32,
    valid: u32,
}

fn banks(chip: &Chip) -> Result<Vec<Bank>> {
    let word = |physical: &str, suffix: &str| -> Result<u32> {
        let name = format!("{physical}_{suffix}");
        let register = chip.model.register(&name).ok_or_else(|| invalid(&name))?;
        match register.location {
            Location::Mmio { address } if register.width == 32 => Ok(u32::try_from(address)?),
            _ => Err(invalid(format!("not a 32-bit MMIO word: {name}"))),
        }
    };
    let mut result = Vec::new();
    for register in chip
        .model
        .registers()
        .iter()
        .filter(|r| r.instance == "WIFI_MAC" && r.key == "RX_BLOCK_ACK_ENTRY%s_CONTROL")
    {
        let physical = register
            .identity
            .strip_suffix("_CONTROL")
            .ok_or_else(|| invalid("BA control name"))?;
        let field = register
            .fields
            .iter()
            .find(|f| f.name == "VALID" && f.width == 1)
            .ok_or_else(|| invalid("BA VALID field"))?;
        result.push(Bank {
            control: word(physical, "CONTROL")?,
            sequence: word(physical, "START_SEQUENCE_LOAD")?,
            low: word(physical, "BITMAP_LOW_LOAD")?,
            high: word(physical, "BITMAP_HIGH_LOAD")?,
            valid: 1u32
                .checked_shl(field.low)
                .ok_or_else(|| invalid("BA VALID position"))?,
        });
    }
    // SOURCE: BLOB_LIBPP_HAL_MAC_RX_BLOCK_ACK: logical hardware indices
    // increase in reverse physical-address order.
    result.sort_by_key(|b| std::cmp::Reverse(b.control));
    if result.len() < 2 {
        return Err(invalid("BA counterexample needs two banks"));
    }
    Ok(result)
}

#[derive(Clone, Copy)]
enum Expected {
    Loaded { sequence: u32, low: u32, high: u32 },
    Inactive,
    UnknownHigh,
}

struct Case {
    execution: ExecutionCase,
    bank: Bank,
    expected: Expected,
}

fn invocation(
    entry: u32,
    bank: Bank,
    active: bool,
    arguments: Vec<Option<u32>>,
    seed: MemorySeed,
) -> Invocation {
    Invocation {
        entry,
        goal: ExecutionGoal::Return,
        arguments,
        memory: vec![ExecutionRegion {
            seed,
            lifetime: RegionLifetime::Phase,
        }],
        preload: vec![],
        models: vec![DeviceDeclaration {
            id: "ba-storage".into(),
            applicability: "Synthetic ordinary BA storage; no hardware update timing is modeled"
                .into(),
            lifetime: RegionLifetime::Phase,
            behavior: DeviceBehavior::RegisterBank {
                cells: [bank.control, bank.sequence, bank.low, bank.high]
                    .into_iter()
                    .map(|address| RegisterCell {
                        address,
                        width: 4,
                        value: if address == bank.control && active {
                            bank.valid
                        } else {
                            0
                        },
                    })
                    .collect(),
            },
        }],
        calls: vec![],
        observe_memory: vec![],
        observe_calls: None,
        observe_timeline: TimelineCapture {
            reads: true,
            writes: true,
            ..Default::default()
        },
    }
}

fn case(
    name: String,
    vendor: Invocation,
    bank: Bank,
    expected: Expected,
    stack_fill: Option<u8>,
) -> Case {
    Case {
        execution: ExecutionCase {
            name,
            reset: SessionReset::Cold,
            stack_fill,
            relation: None,
            vendor,
            replacement: None,
        },
        bank,
        expected,
    }
}

fn cases(banks: &[Bank], caller: u32, callee: u32) -> Result<Vec<Case>> {
    let mut result = Vec::new();
    let count = u32::try_from(banks.len())?;
    let array_bytes = count
        .checked_mul(4)
        .ok_or_else(|| invalid("BA array length"))?;
    let mut arrays = Vec::new();
    for base in [100u32, 0x1111_0000, 0x2222_0000] {
        for index in 0..count {
            arrays.extend_from_slice(
                &base
                    .checked_add(index)
                    .ok_or_else(|| invalid("BA fixture value"))?
                    .to_le_bytes(),
            );
        }
    }
    for (index, bank) in banks.iter().copied().enumerate() {
        let index = u8::try_from(index)?;
        let i = u32::from(index);
        let seed = MemorySeed {
            address: ARRAYS,
            length: u32::try_from(arrays.len())?,
            fill: None,
            bytes: arrays.clone(),
        };
        let low_array = ARRAYS
            .checked_add(array_bytes)
            .ok_or_else(|| invalid("BA low-array address"))?;
        let high_array = low_array
            .checked_add(array_bytes)
            .ok_or_else(|| invalid("BA high-array address"))?;
        let arguments = vec![Some(i), Some(ARRAYS), Some(low_array), Some(high_array)];
        result.push(case(
            format!("arrays-{index}"),
            invocation(callee, bank, true, arguments, seed),
            bank,
            Expected::Loaded {
                sequence: 100 + i,
                low: 0x1111_0000 + i,
                high: 0x2222_0000 + i,
            },
            None,
        ));
        // SOURCE: BLOB_MAC_BA_SCALAR_ARRAY_CONTRACT_20261104. The captured
        // software state is 40 bytes: SSN +12, window +14, hardware index +32.
        let mut state = vec![0; 12];
        state.extend_from_slice(&100u16.to_le_bytes());
        state.extend_from_slice(&64u16.to_le_bytes());
        state.resize(32, 0);
        state.push(index);
        state.resize(40, 0);
        let seed = MemorySeed {
            address: STATE,
            length: 40,
            fill: None,
            bytes: state,
        };
        let arguments = vec![Some(STATE), Some(228)];
        result.push(case(
            format!("inactive-{index}"),
            invocation(caller, bank, false, arguments.clone(), seed.clone()),
            bank,
            Expected::Inactive,
            None,
        ));
        if index < 2 {
            let expected = if index == 0 {
                Expected::Loaded {
                    sequence: 100,
                    low: 0,
                    high: 0,
                }
            } else {
                Expected::Loaded {
                    sequence: 0,
                    low: 0,
                    high: FILL,
                }
            };
            result.push(case(
                format!("scalars-filled-{index}"),
                invocation(caller, bank, true, arguments.clone(), seed.clone()),
                bank,
                expected,
                Some(0xa5),
            ));
        }
        if index == 1 {
            result.push(case(
                "scalars-unknown-1".into(),
                invocation(caller, bank, true, arguments, seed),
                bank,
                Expected::UnknownHigh,
                None,
            ));
        }
    }
    Ok(result)
}

#[derive(Serialize)]
struct Observation {
    name: String,
    stop: ExecutionStop,
    writes: Vec<(u32, u8, u32)>,
}

fn check(case: &Case, index: u32, records: &[ExecutionEvidence]) -> Result<Observation> {
    let mut outcomes = records.iter().filter_map(|r| match r {
        ExecutionEvidence::Outcome {
            case,
            replacement: false,
            stop,
            ..
        } if *case == index => Some(stop),
        _ => None,
    });
    let stop = outcomes
        .next()
        .ok_or_else(|| invalid("missing BA outcome"))?;
    if outcomes.next().is_some() {
        return Err(invalid("duplicate BA outcome"));
    }
    let writes: Vec<_> = records
        .iter()
        .filter_map(|r| match r {
            ExecutionEvidence::Event {
                case,
                replacement: false,
                event:
                    ExecutionEvent::Write {
                        address,
                        width,
                        value,
                    },
            } if *case == index => Some((*address, *width, *value)),
            _ => None,
        })
        .collect();
    let b = case.bank;
    let expected = match case.expected {
        Expected::Loaded {
            sequence,
            low,
            high,
        } => vec![
            (b.control, 4, 0),
            (b.low, 4, low),
            (b.high, 4, high),
            (b.sequence, 4, sequence),
            (b.control, 4, b.valid),
        ],
        Expected::Inactive => vec![],
        Expected::UnknownHigh => vec![(b.control, 4, 0), (b.low, 4, 0)],
    };
    let stop_matches = match case.expected {
        Expected::UnknownHigh => matches!(
            stop,
            ExecutionStop::Incomplete {
                reason: ExecutionGap::UnknownRegister { .. },
                ..
            }
        ),
        _ => matches!(stop, ExecutionStop::Returned { .. }),
    };
    if !stop_matches || writes != expected {
        return Err(invalid(format!(
            "{} differs from the reviewed conditional observation: {stop:?}, writes={writes:?}",
            case.execution.name
        )));
    }
    Ok(Observation {
        name: case.execution.name.clone(),
        stop: stop.clone(),
        writes,
    })
}

/// Check captured behavior, including an expected unknown-data stop. Success
/// is not a declaration that the vendor caller has a correct pointer ABI.
pub fn run(options: Options) -> Result<ExitCode> {
    let run = crate::session::start_run(&options.output)?;
    let ids = [
        artifacts::sha256("libpp")?,
        artifacts::sha256("libnet80211")?,
    ];
    let (_, contents) = authenticate(&[
        Input {
            role: "libpp",
            path: &options.libpp,
            sha256: Some(ids[0]),
        },
        Input {
            role: "libnet80211",
            path: &options.libnet80211,
            sha256: Some(ids[1]),
        },
    ])?;
    let executables: Vec<_> = contents.into_iter().map(Executable::new).collect();
    let inventory = executables
        .iter()
        .map(|exe| -> Result<_> {
            Ok(options
                .budget
                .run(|memory, control| captured::inventory(exe, memory, control))?
                .0)
        })
        .collect::<Result<Vec<_>>>()?;
    let plan = LinkRequest {
        inputs: executables.iter().map(|e| e.id().clone()).collect(),
        entry: symbol(&inventory, 1, CALLER)?.id.clone(),
        roots: vec![symbol(&inventory, 0, CALLEE)?.id.clone()],
        companions: vec![],
        absent: vec![],
        layout: ImageLayout {
            code: ImageRegion {
                start: layout::IMAGE_CODE_START,
                length: 0x10_0000,
            },
            data: ImageRegion {
                start: layout::IMAGE_DATA_START,
                length: 0x10_0000,
            },
        },
    };
    let linker = std::path::absolute(&options.linker)?;
    let (linked, _) = options.budget.run(|memory, control| {
        linking::link(
            &plan,
            &executables,
            &linker,
            &ElfLinker,
            &run,
            memory,
            control,
        )
    })?;
    fs::write(run.join("plan.json"), serde_json::to_vec_pretty(&plan)?)?;
    fs::write(
        run.join("manifest.json"),
        serde_json::to_vec_pretty(&linked.manifest)?,
    )?;
    fs::write(run.join("image.elf"), linked.elf.bytes())?;
    let entry = |name: &str| {
        linked
            .manifest
            .roots
            .iter()
            .find(|r| r.name == name.as_bytes())
            .map(|r| r.address)
            .ok_or_else(|| invalid(name))
    };
    let model = Chip::load(&oer_tools_os_process::built_root(), "esp32s31")?;
    let cases = cases(
        &banks(&model)?,
        u32::try_from(entry(CALLER)?)?,
        u32::try_from(entry(CALLEE)?)?,
    )?;
    let request = ExecutionRequest {
        schema: EXECUTION_SCHEMA,
        vendor: ExecutionTarget {
            executables: vec![linked.elf.id().clone()],
            abi: oer_tools_analysis_riscv_model::CallAbi::RiscvInteger,
            stack: MemorySeed {
                address: STACK,
                length: 65536,
                fill: None,
                bytes: vec![],
            },
        },
        replacement: None,
        binding: None,
        cases: cases.iter().map(|c| c.execution.clone()).collect(),
        max_events: 4096,
    };
    fs::write(
        run.join("request.json"),
        serde_json::to_vec_pretty(&request)?,
    )?;
    let images = [linked.elf];
    let input = InProcessComparison {
        request: &request,
        executables: &images,
        effects: &[],
        projections: &[],
        vendor_results: None,
        dependence: None,
        patches: &[],
    };
    let (result, _) = options
        .budget
        .run(|memory, control| verify(&input, &RiscvExecutor, memory, control))?;
    fs::write(
        run.join("records.json"),
        serde_json::to_vec_pretty(&result.records)?,
    )?;
    let observations = cases
        .iter()
        .enumerate()
        .map(|(i, c)| check(c, u32::try_from(i)?, &result.records))
        .collect::<Result<Vec<_>>>()?;
    fs::write(
        run.join("observations.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "matches_reviewed_observations": true,
            "execution_complete": result.complete,
            "scope": "captured caller/callee with explicit storage models; not whole-firmware reachability or on-air behavior",
            "cases": observations,
        }))?,
    )?;
    println!(
        "{} BA contract observations match, including one expected unknown-data stop: {}",
        observations.len(),
        run.display()
    );
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests;
