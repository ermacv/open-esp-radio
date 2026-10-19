//! Test fixtures of run sessions, shared with the image builder's record
//! tests.

use std::{
    fs::{self, File},
    path::{Path, PathBuf},
    sync::atomic::Ordering,
    time::Instant,
};

use oer_os_durable::{UNIQUE_FILE_COUNTER, atomic_json};

use super::*;

/// A fresh directory below the system temporary directory.
pub fn temporary_directory(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "open-radio-hil-{label}-{}-{}",
        std::process::id(),
        UNIQUE_FILE_COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

/// A completed manifest of a test run.
pub fn manifest() -> RunManifest {
    RunManifest {
        schema: RUN_SCHEMA,
        run_id: String::from("run<&>"),
        target: String::from("chip-a"),
        state: RunState::Completed,
        started_unix_millis: 1,
        finished_unix_millis: Some(2),
        duration_millis: Some(1),
        invocation: vec![String::from("cargo hil")],
        repository: RepositoryProvenance {
            commit: String::from("abc123"),
            dirty: false,
        },
        runner: RunnerProvenance {
            observer: None,
            package: String::from("runner"),
            version: String::from("1"),
            messages_lock_sha256: String::from("00"),
            host_os: String::from("linux"),
            host_arch: String::from("x86_64"),
            tools: Vec::new(),
        },
        cell: CellProvenance {
            cell_id: String::from("cell-1"),
            device_id: String::from("dut-1"),
            serial_device: PathBuf::from("/dev/ttyACM0"),
        },
        lab_provenance_path: None,
        firmware: Vec::new(),
        experiment: None,
        messages_used: Vec::new(),
    }
}

/// An unfinished session writing into `directory`, whose parent is both its
/// target directory and its repository checkout.
pub fn session(directory: &Path) -> RunSession {
    let mut manifest = manifest();
    manifest.run_id = directory
        .file_name()
        .expect("test run directory has a name")
        .to_string_lossy()
        .into_owned();
    manifest.state = RunState::Running;
    manifest.finished_unix_millis = None;
    manifest.duration_millis = None;
    atomic_json(&directory.join("manifest.json"), &manifest).unwrap();
    manifest.repository = RepositoryProvenance {
        commit: String::new(),
        dirty: true,
    };
    RunSession {
        target_directory: directory
            .parent()
            .expect("test run directory has a parent")
            .to_owned(),
        directory: directory.to_owned(),
        manifest,
        started: Instant::now(),
        events: File::create(directory.join("events.jsonl")).unwrap(),
        finished: false,
        interruption: None,
    }
}

/// A session in the runs directory of `target_directory`.
pub fn integrated_session(target_directory: &Path) -> RunSession {
    let directory = target_directory.join("runs").join(manifest().run_id);
    fs::create_dir_all(&directory).unwrap();
    let mut session = session(&directory);
    session.target_directory = target_directory.to_owned();
    session
}

/// The staged chip of the fixture images.
pub const TEST_CHIP: &str = "chip-a";

/// The fixture chip's profile (`portable/chip-a/platform/chip.toml`).
pub const TEST_CHIP_PROFILE: &str = "schema = 1\nid = \"chip-a\"\nfamily = \"f\"\n\
    rust-target = \"riscv32imafc-unknown-none-elf\"\nboot = \"staged\"\n\
    espflash-chip = \"esp32c6\"\nrevisions = []\n\
    [properties]\nwifi-bands = []\nbluetooth = []\nieee802154 = false\ncores = 1\n\
    [flash]\nbootloader = 0x2000\npartition-table = 0x8000\napplication = 0x10000\n\
    otadata = 0xd000\npartitions = \"portable/chip-a/platform/partitions/applications.csv\"\n\
    application-encoding = { mode = \"qio\", frequency-mhz = 80, size-mib = 16 }\n\
    bootloader-mode = \"dio\"\nstart = \"reset\"\n";

pub fn write_test_build_materials(root: &Path) {
    for relative in [
        "Cargo.lock",
        "firmware/hil/chip-a/Cargo.toml",
        "firmware/hil/chip-a/stack.toml",
        "portable/chip-a/platform/stack.toml",
        "portable/chip-a/platform/partitions/applications.csv",
    ] {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, format!("test material: {relative}\n")).unwrap();
    }
    // The HIL policy extends the platform's, as the provenance follows it.
    fs::write(
        root.join("firmware/hil/chip-a/stack.toml"),
        "extends = \"../../../portable/chip-a/platform/stack.toml\"\n",
    )
    .unwrap();
    fs::write(
        root.join("portable/chip-a/platform/chip.toml"),
        TEST_CHIP_PROFILE,
    )
    .unwrap();
}

/// A session writing into `directory` for `target_directory`.
pub fn session_for(directory: &Path, target_directory: &Path) -> RunSession {
    let mut session = session(directory);
    session.target_directory = target_directory.to_owned();
    session
}

/// One repetition of a fixture run, with its artifacts in
/// `scenarios/<scenario>/repetition-NNN`.
pub fn repetition(
    scenario: &str,
    number: u8,
    outcome: Outcome,
    failure: Option<Failure>,
    measurements: Vec<Measurement>,
) -> RepetitionResult {
    RepetitionResult {
        schema: RUN_SCHEMA,
        repetition: number,
        outcome,
        started_unix_millis: 1,
        duration_millis: 1,
        artifact_directory: PathBuf::from("scenarios")
            .join(scenario)
            .join(format!("repetition-{number:03}")),
        attachments: Vec::new(),
        measurements,
        failure,
    }
}

/// A fixture run published in `directory`, whose name is its id: its
/// manifest, edited by `edit`, and for a completed run the suite of
/// `scenarios`; nothing is finished. Returns the manifest.
pub fn write_run(
    directory: &Path,
    started_unix_millis: u64,
    state: RunState,
    scenarios: Vec<ScenarioResult>,
    edit: impl FnOnce(&mut RunManifest),
) -> RunManifest {
    fs::create_dir_all(directory).unwrap();
    let mut manifest = manifest();
    manifest.run_id = directory
        .file_name()
        .expect("fixture run directory has a name")
        .to_string_lossy()
        .into_owned();
    manifest.started_unix_millis = started_unix_millis;
    manifest.state = state;
    let ended = state != RunState::Running;
    manifest.finished_unix_millis = ended.then_some(started_unix_millis + 1);
    manifest.duration_millis = ended.then_some(1);
    edit(&mut manifest);
    atomic_json(&directory.join("manifest.json"), &manifest).unwrap();
    for scenario in &scenarios {
        for repetition in &scenario.repetitions {
            fs::create_dir_all(directory.join(&repetition.artifact_directory)).unwrap();
        }
    }
    if state == RunState::Completed {
        let outcome = if scenarios
            .iter()
            .all(|scenario| scenario.outcome.is_passed())
        {
            Outcome::Passed
        } else {
            Outcome::Failed
        };
        let suite = SuiteResult {
            schema: RUN_SCHEMA,
            run_id: manifest.run_id.clone(),
            target: manifest.target.clone(),
            outcome,
            started_unix_millis: manifest.started_unix_millis,
            finished_unix_millis: manifest.finished_unix_millis.unwrap(),
            duration_millis: manifest.duration_millis.unwrap(),
            counts: SuiteCounts::from_results(&scenarios),
            scenarios,
        };
        validation::validate_suite(&suite, &manifest).unwrap();
        atomic_json(&directory.join("suite.json"), &suite).unwrap();
    }
    manifest
}
