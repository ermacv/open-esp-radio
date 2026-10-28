//! Without `cfg(oer_trace)` an observation point leaves no machine code: an
//! owner with points compiles, for the firmware target and with the firmware
//! profile's code generation, to the same assembly as the owner without them.
#![forbid(unsafe_code)]
#![allow(clippy::disallowed_macros, clippy::expect_used)]

use std::{fs, path::Path, process::Command};

/// An owner with two observation points whose events read a clock-like
/// counter. Its entry points are `inline(never)`, so the library emits them. Each `//POINT ` line is the point: kept for the observed build
/// and blanked (keeping line numbers) for the reference build.
const OWNER: &str = r#"#![no_std]
use core::sync::atomic::{AtomicU32, Ordering};
use oer_trace::{Channel, Domain, Event, Kind, Noop, Observer};

/// Stands for a clock or register read: a call with a side effect.
pub static READS: AtomicU32 = AtomicU32::new(0);

#[inline(never)]
pub fn read_clock() -> u32 {
    READS.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy)]
pub struct Step {
    pub state: u32,
    pub at: u32,
}

impl Event for Step {
    const KIND: Kind = Kind::new(Domain::Phy, 1);
    const CHANNEL: Channel = match Channel::new(Domain::Phy, 0) {
        Ok(channel) => channel,
        Err(_) => panic!("invalid channel"),
    };
    fn encode(&self) -> [u32; 2] {
        [self.state, self.at]
    }
    fn decode(words: [u32; 2]) -> Option<Self> {
        let [state, at] = words;
        Some(Self { state, at })
    }
}

pub struct Core<O: Observer = Noop> {
    observer: O,
    state: u32,
}

impl<O: Observer> Core<O> {
    #[inline]
    fn advance(&mut self, input: u32) -> u32 {
        self.state = self.state.rotate_left(5) ^ input;
//POINT oer_trace::observe!(self.observer, Step { state: self.state, at: read_clock() });
        if self.state & 1 == 1 {
            self.state = self.state.wrapping_mul(3);
        }
//POINT oer_trace::observe!(self.observer, Step { state: self.state, at: read_clock() });
        self.state
    }
}

#[inline(never)]
pub fn core() -> Core {
    Core { observer: Noop, state: 0 }
}

#[inline(never)]
pub fn run(core: &mut Core, inputs: &[u32]) -> u32 {
    let mut sum = 0u32;
    for &input in inputs {
        sum = sum.wrapping_add(core.advance(input));
    }
    sum
}
"#;

const MARKER: &str = "//POINT ";

/// The firmware target and the code generation of `[profile.firmware]`.
const TARGET: &str = "riscv32imafc-unknown-none-elf";

/// Builds `source` as the case crate into its own build directory and
/// returns the emitted assembly.
fn assembly(scratch: &Path, variant: &str, source: &str, rustc_args: &[&str]) -> String {
    fs::write(scratch.join("case.rs"), source).expect("write case source");
    let build = scratch.join(format!("build-{variant}"));
    let listing = scratch.join(format!("{variant}.s"));
    // The case sees only the flags and directories given here, never the
    // outer test's or the user's configured ones.
    let output = Command::new(env!("CARGO"))
        .current_dir(scratch)
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_RUSTFLAGS")
        .env_remove("CARGO_BUILD_TARGET")
        .env("CARGO_TARGET_DIR", &build)
        .env("CARGO_BUILD_BUILD_DIR", &build)
        .args([
            "rustc",
            "--offline",
            "--release",
            "--lib",
            "--target",
            TARGET,
        ])
        .arg("--manifest-path")
        .arg(scratch.join("Cargo.toml"))
        .arg("--")
        .arg(format!("--emit=asm={}", listing.display()))
        .args(rustc_args)
        .output()
        .expect("run pinned cargo");
    fs::write(scratch.join(format!("{variant}.log")), &output.stderr)
        .expect("save compiler diagnostics");
    assert!(
        output.status.success(),
        "{variant}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(listing).expect("read case assembly")
}

#[test]
fn points_without_the_trace_cfg_compile_to_the_code_of_an_owner_without_them() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).expect("own compiler scratch");
    fs::write(
        scratch.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"observe-codegen\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [workspace]\n[lib]\npath = \"case.rs\"\n\
             [dependencies]\noer-trace = {{ path = {manifest:?} }}\n\
             [lints.rust]\nunexpected_cfgs = {{ level = \"warn\", check-cfg = [\"cfg(oer_trace)\"] }}\n\
             [profile.release]\nopt-level = 3\ncodegen-units = 1\npanic = \"abort\"\ndebug = false\n"
        ),
    )
    .expect("write isolated compiler manifest");

    let observed = OWNER.replace(MARKER, "");
    let reference = OWNER
        .lines()
        .map(|line| if line.starts_with(MARKER) { "" } else { line })
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(observed.lines().count(), reference.lines().count());
    assert_eq!(observed.matches("observe!").count(), 2);
    assert!(!reference.contains("observe!"));

    let without_points = assembly(scratch.path(), "reference", &reference, &[]);
    let points_off = assembly(scratch.path(), "observed", &observed, &[]);
    assert!(without_points.contains("3run"), "the owner is compiled");
    assert_eq!(
        points_off, without_points,
        "observation points without cfg(oer_trace) changed the machine code"
    );

    // The comparison sees a point: with the cfg the same owner, observer
    // `Noop` included, evaluates its event and reads the clock.
    let points_on = assembly(scratch.path(), "trace", &observed, &["--cfg", "oer_trace"]);
    assert_ne!(
        points_on, without_points,
        "an enabled observation point left no code"
    );
    assert_eq!(clock_calls(&without_points), 0);
    assert_eq!(clock_calls(&points_off), 0);
    assert!(clock_calls(&points_on) > 0, "{points_on}");
}

/// The call instructions to the clock-like read in an assembly listing.
fn clock_calls(assembly: &str) -> usize {
    assembly
        .lines()
        .map(str::trim_start)
        .filter(|line| line.starts_with("call") || line.starts_with("tail"))
        .filter(|line| line.contains("read_clock"))
        .count()
}
