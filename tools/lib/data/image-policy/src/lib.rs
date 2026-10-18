//! The stack policy of an image family (`stack.toml`, schema 5): the stacks
//! the gate bounds, each by the function that runs on it, the compiler move
//! limit and the runtime headroom the firmware's stack painting enforces.
//!
//! A policy that names no stacks sets only the compiler's move limit: the
//! image is compiled with every image flag (frame records included), and the
//! stack check has nothing to bound. A policy that names stacks names the
//! coverage review and the CPU0 task stack, and the interrupt stacks' and a
//! bootstrap image's stack where the image has them.

use std::path::{Path, PathBuf};

use serde::Deserialize;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The policy schema this gate reads.
pub const SCHEMA: u32 = 5;

/// A target's stack policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StackPolicy {
    pub schema: u32,
    /// The review of every linked function without a compiler frame record,
    /// relative to the policy file that names it.
    #[serde(default)]
    pub coverage_policy: Option<PathBuf>,
    /// The largest implicit copy or move the compiler admits
    /// (`-Z move-size-limit` with `-D large-assignments`).
    pub max_move_bytes: u64,
    /// The runtime's primary task stack.
    #[serde(default)]
    pub cpu0_task_stack: Option<TaskStack>,
    #[serde(default)]
    pub interrupt_stacks: Option<InterruptStacks>,
    /// The bootstrap image's only stack.
    #[serde(default)]
    pub bootstrap_stack: Option<TaskStack>,
}

/// The stacks of a policy that names them.
pub struct Stacks<'a> {
    pub coverage_policy: &'a Path,
    pub cpu0_task_stack: &'a TaskStack,
    pub interrupt_stacks: Option<&'a InterruptStacks>,
    pub bootstrap_stack: Option<&'a TaskStack>,
}

/// One stack, the function that runs on it and the headroom it keeps.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TaskStack {
    /// The symbol of the function that runs on the stack from its top: the
    /// root of the call chains the gate bounds.
    pub root: String,
    /// The symbol whose size is the stack's capacity.
    #[serde(default)]
    pub storage_symbol: Option<String>,
    /// For a stack the linker script places as a range rather than an
    /// object: the symbols of its lowest address and of its top.
    #[serde(default)]
    pub bottom_symbol: Option<String>,
    #[serde(default)]
    pub top_symbol: Option<String>,
    /// Bytes the deepest call chain must leave unused; the runtime's stack
    /// painting holds every exercised workload to the same reserve.
    pub minimum_free_bytes: u32,
}

/// Where a stack's capacity comes from.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Storage<'a> {
    /// A sized symbol: the stack object.
    Symbol(&'a str),
    /// The linker script's bottom and top symbols.
    Range { bottom: &'a str, top: &'a str },
}

impl TaskStack {
    pub fn storage(&self) -> Storage<'_> {
        match (&self.storage_symbol, &self.bottom_symbol, &self.top_symbol) {
            (Some(symbol), None, None) => Storage::Symbol(symbol),
            (None, Some(bottom), Some(top)) => Storage::Range { bottom, top },
            _ => unreachable!("validated: one storage form"),
        }
    }

    fn validate(&self, name: &str) -> Result<()> {
        let form = match (&self.storage_symbol, &self.bottom_symbol, &self.top_symbol) {
            (Some(symbol), None, None) => !symbol.is_empty(),
            (None, Some(bottom), Some(top)) => !bottom.is_empty() && !top.is_empty(),
            _ => false,
        };
        if !form {
            return Err(format!(
                "`{name}` needs either `storage_symbol` or both `bottom_symbol` and `top_symbol`"
            )
            .into());
        }
        if self.root.is_empty() {
            return Err(format!("`{name}` names no root function").into());
        }
        if self.minimum_free_bytes == 0 {
            return Err(format!("`{name}` needs positive `minimum_free_bytes`").into());
        }
        Ok(())
    }
}

/// The dedicated SRAM interrupt stacks: their bound and margin are the
/// interrupt-stack gate's contract; this is the runtime painting's reserve.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterruptStacks {
    pub minimum_free_bytes: u32,
}

/// A policy that reuses a base policy (`extends`, relative to this file)
/// unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extension {
    schema: u32,
    extends: PathBuf,
}

impl StackPolicy {
    pub fn load(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let table: toml::Table =
            toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        let policy = if table.contains_key("extends") {
            let extension: Extension =
                toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
            let base_path = parent(path).join(&extension.extends);
            let base_source = std::fs::read_to_string(&base_path)
                .map_err(|error| format!("cannot read {}: {error}", base_path.display()))?;
            if toml::from_str::<toml::Table>(&base_source)
                .is_ok_and(|base| base.contains_key("extends"))
            {
                return Err(format!(
                    "{} extends {}, which itself extends another policy; extend the base directly",
                    path.display(),
                    base_path.display()
                )
                .into());
            }
            let base = Self::parse(&base_path, &base_source)?;
            if extension.schema != base.schema {
                return Err(format!(
                    "{} has schema {} but its base has schema {}",
                    path.display(),
                    extension.schema,
                    base.schema
                )
                .into());
            }
            base
        } else {
            Self::parse(path, &source)?
        };
        policy.validate()?;
        Ok(policy)
    }

    fn parse(path: &Path, source: &str) -> Result<Self> {
        let mut policy: Self =
            toml::from_str(source).map_err(|error| format!("{}: {error}", path.display()))?;
        policy.coverage_policy = policy
            .coverage_policy
            .map(|coverage| parent(path).join(coverage));
        Ok(policy)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema != SCHEMA {
            return Err(format!(
                "unsupported stack policy schema {}; expected {SCHEMA}",
                self.schema
            )
            .into());
        }
        if self.max_move_bytes == 0 {
            return Err("the move limit must be positive".into());
        }
        let named = [
            self.coverage_policy.is_some(),
            self.cpu0_task_stack.is_some(),
            self.interrupt_stacks.is_some(),
            self.bootstrap_stack.is_some(),
        ];
        if !named.iter().any(|named| *named) {
            return Ok(());
        }
        let stacks = self.stacks()?;
        if stacks
            .interrupt_stacks
            .is_some_and(|interrupts| interrupts.minimum_free_bytes == 0)
        {
            return Err("the interrupt stacks' reserve must be positive".into());
        }
        stacks.cpu0_task_stack.validate("cpu0_task_stack")?;
        if let Some(bootstrap) = stacks.bootstrap_stack {
            bootstrap.validate("bootstrap_stack")?;
        }
        Ok(())
    }

    /// The stacks the gate bounds, or an error for a policy that sets only
    /// the move limit.
    pub fn stacks(&self) -> Result<Stacks<'_>> {
        match (&self.coverage_policy, &self.cpu0_task_stack) {
            (Some(coverage_policy), Some(cpu0)) => Ok(Stacks {
                coverage_policy,
                cpu0_task_stack: cpu0,
                interrupt_stacks: self.interrupt_stacks.as_ref(),
                bootstrap_stack: self.bootstrap_stack.as_ref(),
            }),
            _ => Err(
                "a stack policy that names stacks names `coverage_policy` and \
                 `cpu0_task_stack`"
                    .into(),
            ),
        }
    }

    /// The runtime's task stacks, by name.
    pub fn runtime_task_stacks(&self) -> Vec<(&'static str, &TaskStack)> {
        self.cpu0_task_stack
            .iter()
            .map(|stack| ("CPU0 task stack", stack))
            .collect()
    }

    /// The image compiler setup of an image under this policy, built from
    /// the tree at `root` with the image linker's build in `linker_target`.
    pub fn image_compiler<'a>(
        &self,
        root: &'a Path,
        linker_target: &'a Path,
        rust_target: &'a str,
    ) -> oer_toolchain::image::ImageCompiler<'a> {
        oer_toolchain::image::ImageCompiler {
            root,
            linker_target,
            rust_target,
            max_move_bytes: self.max_move_bytes,
            runtime_environment: self.runtime_environment(),
        }
    }

    /// The headroom reserves the runtime's stack painting enforces, as the
    /// build environment hands them to the firmware.
    pub fn runtime_environment(&self) -> Vec<(&'static str, String)> {
        let mut variables = Vec::new();
        if let Some(cpu0) = &self.cpu0_task_stack {
            variables.push((
                "OPEN_RADIO_CPU0_STACK_MINIMUM_FREE_BYTES",
                cpu0.minimum_free_bytes.to_string(),
            ));
        }
        if let Some(interrupts) = &self.interrupt_stacks {
            variables.push((
                "OPEN_RADIO_IRQ_STACK_MINIMUM_FREE_BYTES",
                interrupts.minimum_free_bytes.to_string(),
            ));
        }
        variables
    }
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}
