//! Pre-initialization copy diagnostics with preserved partial observations.

use crate::Result;
use oer_hil_protocol::{
    system::MemoryBenchmark, system::MemoryBenchmarkCompleted, system::MemoryBenchmarkEvidence,
    system::MemoryBenchmarkMode, system::MemoryBenchmarkRequest, system::MemoryBenchmarkSource,
    system::MemoryBenchmarkStop, system::RunMemoryBenchmark,
};
use oer_hil_workload::{boots::boot_directory, context::Context, require_keys};
use serde::Serialize;
use std::{path::Path, time::Duration};

pub struct Config<'a> {
    pub boots: u8,
    pub iterations: u16,
    pub sizes: &'a [u16],
    pub batch_sizes: &'a [u8],
}

#[derive(Serialize)]
struct CaseReport {
    boot: u8,
    requested: MemoryBenchmarkRequest,
    target: MemoryBenchmarkEvidence,
}

pub fn run(config: Config<'_>, output: &Path, context: &Context<'_>) -> Result<()> {
    context.results.claim(
        "memory-copy-diagnostic",
        &[
            "cpu-utilization",
            "elapsed-counters-exclude-other-activity-between-async-polls",
        ],
    );
    let requests = requests(&config);
    let mut cases = 0;
    for boot in 1..=config.boots {
        context.with_capture(&boot_directory(output, boot), |capture| {
            require_keys::<MemoryBenchmark>(capture)?;
            for &requested in &requests {
                let MemoryBenchmarkCompleted(target) =
                    capture.request(0, RunMemoryBenchmark(requested), Duration::from_secs(15))?;
                cases += 1;
                // Record the terminal target observation before evaluating it.
                context.results.observe(
                    format!("case-{cases:03}"),
                    &CaseReport {
                        boot,
                        requested,
                        target,
                    },
                );
                validate(requested, target)?;
            }
            Ok(())
        })?;
    }
    eprintln!("memory_benchmark=PASS cases={cases}");
    Ok(())
}

fn requests(config: &Config<'_>) -> Vec<MemoryBenchmarkRequest> {
    let mut requests = Vec::with_capacity(config.sizes.len() * config.batch_sizes.len() * 6);
    for source in [MemoryBenchmarkSource::Sram, MemoryBenchmarkSource::Psram] {
        for &bytes in config.sizes {
            for &frames in config.batch_sizes {
                for mode in [
                    MemoryBenchmarkMode::CpuCopy,
                    MemoryBenchmarkMode::GdmaBlocking,
                    MemoryBenchmarkMode::GdmaAsync,
                ] {
                    requests.push(MemoryBenchmarkRequest {
                        mode,
                        source,
                        bytes,
                        frames,
                        iterations: config.iterations,
                    });
                }
            }
        }
    }
    requests
}

fn validate(request: MemoryBenchmarkRequest, target: MemoryBenchmarkEvidence) -> Result<()> {
    if !request.validate() || target.request != request {
        return Err("memory benchmark response does not match its bounded request".into());
    }
    if target.stop != MemoryBenchmarkStop::Completed {
        return Err(format!("memory benchmark stopped at {:?}: {target:?}", target.stop).into());
    }
    if target.completed_iterations != request.iterations {
        return Err("memory benchmark did not complete every requested iteration".into());
    }
    if target.elapsed_cycles == 0
        || target.elapsed_instructions == 0
        || target.foreground_cycles == 0
        || target.foreground_instructions == 0
        || target.foreground_cycles > target.elapsed_cycles
        || target.foreground_instructions > target.elapsed_instructions
    {
        return Err("memory benchmark counter scopes are inconsistent".into());
    }
    match request.mode {
        MemoryBenchmarkMode::GdmaAsync if target.polls < u32::from(request.iterations) => {
            Err("async memory benchmark did not poll every transfer".into())
        }
        MemoryBenchmarkMode::CpuCopy | MemoryBenchmarkMode::GdmaBlocking
            if target.polls != 0
                || target.foreground_cycles != target.elapsed_cycles
                || target.foreground_instructions != target.elapsed_instructions =>
        {
            Err("synchronous memory benchmark counter scope is not the whole operation".into())
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests;
