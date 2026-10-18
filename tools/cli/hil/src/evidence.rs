//! `cargo hil evidence`: this checkout's pending evidence.
//!
//! `cargo hil run` never writes tracked files: a clean run's passed
//! scenarios are noted in this checkout's pending list
//! ([`oer_hil_run_bundle_format::pending`]), and the qualification
//! evaluator records them as tracked shards with
//! `cargo qualification hil-evidence --hil-target CHIP --pending`, which
//! reads the runs itself; the stand never runs the evaluator.

use std::ffi::OsString;

use oer_hil_run_bundle::RunStore;
use oer_hil_run_bundle_format::pending;

use crate::Result;
use oer_process::Checkout;

const HELP: &str = "\
usage: cargo hil evidence pending
       cargo hil evidence dismiss --run ID ...

pending  list this checkout's clean runs whose evidence is not recorded;
         `cargo qualification hil-evidence --hil-target CHIP --pending`
         records them as tracked shards in hil/evidence/<chip>/
dismiss  drop runs whose evidence will not be recorded, such as runs whose
         inputs changed since, from the pending list";

/// `cargo hil evidence ...`.
pub fn command(ctx: &Checkout, args: &[OsString]) -> Result<std::process::ExitCode> {
    let args = args
        .iter()
        .map(|arg| arg.to_str().ok_or("arguments must be UTF-8"))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    match args.split_first() {
        Some((&"dismiss", rest)) => {
            let runs = dismiss(&ctx.root, rest)?;
            println!("hil: dismissed {runs} pending run(s)");
        }
        Some((&"pending", [])) => {
            let store = RunStore::shared()?;
            let pending = pending::load(&ctx.root)?
                .into_iter()
                // Pruned runs cannot be recorded.
                .filter(|entry| store.run(&entry.run).is_dir())
                .collect::<Vec<_>>();
            if pending.is_empty() {
                println!("no pending HIL evidence in this checkout");
            }
            for entry in pending {
                println!(
                    "{}  {}  ({})",
                    entry.run,
                    entry.scenarios.join(", "),
                    entry.owner
                );
            }
        }
        _ => {
            println!("{HELP}");
        }
    }
    Ok(std::process::ExitCode::SUCCESS)
}

/// Drop the `--run` runs whose evidence will not be recorded from the pending
/// list; returns how many were named.
fn dismiss(root: &std::path::Path, args: &[&str]) -> Result<usize> {
    let mut runs = Vec::new();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match *arg {
            "--run" => runs.push(rest.next().ok_or("--run needs a run ID")?.to_string()),
            other => return Err(format!("unknown argument {other}\n{HELP}").into()),
        }
    }
    if runs.is_empty() {
        return Err(format!("dismiss needs --run ID\n{HELP}").into());
    }
    pending::forget(root, &runs)?;
    Ok(runs.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dismissing_drops_only_the_named_runs_and_needs_one() {
        let root = tempfile::tempdir().unwrap();
        let entry = |run: &str| pending::Pending {
            run: run.into(),
            scenarios: vec![String::from("a")],
            owner: String::from("wifi"),
        };
        pending::remember(root.path(), &[entry("r1"), entry("r2"), entry("r3")]).unwrap();
        assert!(dismiss(root.path(), &[]).is_err());
        assert!(dismiss(root.path(), &["r1"]).is_err());
        assert_eq!(
            dismiss(root.path(), &["--run", "r1", "--run", "r3"]).unwrap(),
            2
        );
        assert_eq!(pending::load(root.path()).unwrap(), [entry("r2")]);
    }
}
