//! The stack policy of an image family (`stack.toml`, schema 6): the stacks
//! the gate bounds, each by the function that runs on it, the compiler move
//! reporting limit and the runtime headroom the firmware's stack painting
//! enforces.
//!
//! A policy that names no stacks sets only the compiler move and
//! function-frame reporting limit: the image is compiled with every image
//! flag (frame records included), and the
//! whole-stack check has nothing to bound. A policy that names stacks names the
//! coverage review and, for every core of the chip (`[core.N.task_stack]` and
//! `[core.N.interrupt_stack]`), either the stack's proven root or
//! `unused = true`, and a bootstrap image's stack where the image has one.
//! The gate proves an unused stack: nothing in the image starts the core,
//! routes an interrupt to it or allocates it a byte.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// The policy schema this gate reads.
pub const SCHEMA: u32 = 6;

/// A target's stack policy.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StackPolicy {
    pub schema: u32,
    /// The review of every linked function without a compiler frame record,
    /// relative to the policy file that names it.
    #[serde(default)]
    pub coverage_policy: Option<PathBuf>,
    /// The reporting threshold for recorded function frames and implicit copies
    /// or moves (`-Z move-size-limit` with `-W large-assignments`).
    pub max_move_bytes: u64,
    /// Defaults for every hart, including explicit zero allocations.
    #[serde(default)]
    pub default_task_stack_bytes: BTreeMap<u32, u32>,
    /// Reviewed frames above the move limit, scoped to each ELF of the image.
    #[serde(default)]
    pub frame_exceptions: FrameExceptions,
    /// Demangled function-name patterns watched by the reachability report.
    /// `*` matches any substring; all other characters are literal.
    #[serde(default)]
    pub observed_functions: Vec<String>,
    /// The runtime's stacks of each core, by core number: every core of the
    /// chip when the policy names stacks.
    #[serde(default)]
    pub core: BTreeMap<u32, CoreStacks>,
    /// The bootstrap image's only stack.
    #[serde(default)]
    pub bootstrap_stack: Option<TaskStack>,
}

/// A frame exception applies to exactly one implementation in its ELF.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrameExceptions {
    #[serde(default)]
    pub runtime: Vec<FrameException>,
    #[serde(default)]
    pub bootstrap: Vec<FrameException>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FrameException {
    /// Exact demangled name without hashes, with concrete generic arguments.
    pub function: String,
    /// What in the current implementation occupies the frame.
    pub reason: String,
    /// The initial measured frame; subsequent edits may only lower it.
    pub ceiling_bytes: u64,
}

/// The stacks of a policy that names them.
pub struct Stacks<'a> {
    pub coverage_policy: &'a Path,
    pub bootstrap_stack: Option<&'a TaskStack>,
}

/// A physical task stack root, keyed by hart; bootstrap has no runtime hart.
#[derive(Debug)]
pub struct StackRoot<'a> {
    pub hart: Option<u32>,
    pub name: String,
    pub stack: &'a TaskStack,
}

impl<'a> StackRoot<'a> {
    pub fn bootstrap(stack: &'a TaskStack) -> Self {
        Self {
            hart: None,
            name: "bootstrap stack".into(),
            stack,
        }
    }
}

/// One core's task stack and interrupt stack.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CoreStacks {
    pub task_stack: Slot<TaskStack>,
    pub interrupt_stack: Slot<InterruptStack>,
}

/// A stack the image uses, proven from its root, or `unused = true`: no byte
/// is allocated and the gate proves that nothing runs on it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Slot<T> {
    Unused(Unused),
    Used(T),
}

/// The `unused = true` marker of a [`Slot`].
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Unused {
    #[serde(deserialize_with = "only_true")]
    pub unused: bool,
}

fn only_true<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<bool, D::Error> {
    if bool::deserialize(deserializer)? {
        Ok(true)
    } else {
        Err(serde::de::Error::custom(
            "`unused = false`: name the stack's root instead",
        ))
    }
}

impl<T> Slot<T> {
    pub fn used(&self) -> Option<&T> {
        match self {
            Self::Used(stack) => Some(stack),
            Self::Unused(_) => None,
        }
    }
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
    /// This hart's explicit PSRAM allocation; absent uses its chip default.
    #[serde(default)]
    pub allocation_bytes: Option<u32>,
    /// The bootstrap stack only: the symbol, in the stage-two ELF, of the
    /// entry the bootstrap jumps to. A transfer to it ends the stack's path,
    /// once the checks verify in the ELFs that the jump goes there and that
    /// the entry installs its own stack pointer before any stack access.
    #[serde(default)]
    pub handoff: Option<String>,
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
    /// The allocation of a stack whose bound is proven: the bound plus the
    /// reserve, rounded up to the 16-byte stack-pointer alignment. The
    /// stack's bottom is the 64-byte aligned start of its section and
    /// carries the watchpoint, so the size has no further constraint.
    pub fn proven_allocation_bytes(&self, bound: u64) -> Result<u32> {
        let bytes = bound
            .checked_add(u64::from(self.minimum_free_bytes))
            .and_then(|bytes| bytes.checked_add(15))
            .map(|bytes| bytes / 16 * 16)
            .ok_or("task stack allocation overflows u64")?;
        u32::try_from(bytes).map_err(|_| {
            format!(
                "task stack bound {bound} + {} free bytes does not fit a u32 allocation",
                self.minimum_free_bytes
            )
            .into()
        })
    }

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
        if self
            .allocation_bytes
            .is_some_and(|bytes| bytes % 16 != 0 || bytes <= self.minimum_free_bytes)
        {
            return Err(format!(
                "`{name}` allocation must be aligned to 16 bytes and exceed its reserve"
            )
            .into());
        }
        Ok(())
    }
}

/// A core's dedicated SRAM interrupt stack: its bound and margin are the
/// interrupt-stack gate's contract, without an independent free-byte reserve.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterruptStack {
    /// This hart's maximum interrupt allocation; absent uses the chip contract.
    #[serde(default)]
    pub allocation_bytes: Option<u32>,
}

/// A policy that reuses a base policy (`extends`, relative to this file)
/// with optional additional frame exceptions. Stack allocations and compiler
/// limits remain those of the base.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Extension {
    schema: u32,
    extends: PathBuf,
    #[serde(default)]
    frame_exceptions: FrameExceptions,
    #[serde(default)]
    observed_functions: Vec<String>,
}

impl StackPolicy {
    pub fn load(path: &Path) -> Result<Self> {
        Self::load_with_inputs(path, &mut oer_os_durable::inputs::Inputs::default())
    }

    /// Record both this policy and any extended base at their actual reads.
    pub fn load_with_inputs(
        path: &Path,
        inputs: &mut oer_os_durable::inputs::Inputs,
    ) -> Result<Self> {
        let source = inputs
            .read_to_string(path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let table: toml::Table =
            toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
        let policy = if table.contains_key("extends") {
            let extension: Extension =
                toml::from_str(&source).map_err(|error| format!("{}: {error}", path.display()))?;
            let base_path = parent(path).join(&extension.extends);
            let base_source = inputs
                .read_to_string(&base_path)
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
            let mut base = Self::parse(&base_path, &base_source)?;
            if extension.schema != base.schema {
                return Err(format!(
                    "{} has schema {} but its base has schema {}",
                    path.display(),
                    extension.schema,
                    base.schema
                )
                .into());
            }
            base.frame_exceptions
                .runtime
                .extend(extension.frame_exceptions.runtime);
            base.frame_exceptions
                .bootstrap
                .extend(extension.frame_exceptions.bootstrap);
            base.observed_functions.extend(extension.observed_functions);
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
        if self
            .default_task_stack_bytes
            .values()
            .any(|bytes| bytes % 16 != 0)
        {
            return Err("task defaults must be aligned to 16 bytes".into());
        }
        let mut observed = std::collections::BTreeSet::new();
        for pattern in &self.observed_functions {
            if pattern.trim().is_empty() || !observed.insert(pattern) {
                return Err(
                    format!("empty or duplicate observed function pattern: {pattern:?}").into(),
                );
            }
        }
        for exceptions in [
            &self.frame_exceptions.runtime,
            &self.frame_exceptions.bootstrap,
        ] {
            let mut names = std::collections::BTreeSet::new();
            for exception in exceptions {
                if exception.function.trim().is_empty()
                    || exception.reason.trim().is_empty()
                    || exception.ceiling_bytes <= self.max_move_bytes
                {
                    return Err("a frame exception needs an exact function, a reason and a ceiling above the move limit".into());
                }
                if !names.insert(&exception.function) {
                    return Err(format!("duplicate frame exception: {}", exception.function).into());
                }
            }
        }
        if !self.default_task_stack_bytes.is_empty() {
            if self.default_task_stack_bytes.len() != self.core.len() {
                return Err("task defaults must cover every hart".into());
            }
            for (&hart, &bytes) in &self.default_task_stack_bytes {
                let stacks = self
                    .core
                    .get(&hart)
                    .ok_or("default names an unknown hart")?;
                if stacks.task_stack.used().is_some() != (bytes > 0) {
                    return Err("task defaults must preserve used/unused storage".into());
                }
            }
        }
        let named = [
            self.coverage_policy.is_some(),
            !self.core.is_empty(),
            self.bootstrap_stack.is_some(),
        ];
        if !named.iter().any(|named| *named) {
            return Ok(());
        }
        let stacks = self.stacks()?;
        self.validate_cores()?;
        if let Some(bootstrap) = stacks.bootstrap_stack {
            bootstrap.validate("bootstrap_stack")?;
            if bootstrap.handoff.as_deref() == Some("") {
                return Err("`bootstrap_stack` names an empty `handoff` symbol".into());
            }
            if bootstrap.allocation_bytes.is_some() {
                return Err(
                    "bootstrap stack allocation cannot be overridden by a runtime policy".into(),
                );
            }
        }
        Ok(())
    }

    /// Every hart has both stacks used or both explicitly unused.
    fn validate_cores(&self) -> Result<()> {
        for (expected, (&hart, stacks)) in self.core.iter().enumerate() {
            if hart as usize != expected {
                return Err(format!("`core.{expected}` is missing from the stack policy").into());
            }
            if stacks.task_stack.used().is_some() != stacks.interrupt_stack.used().is_some() {
                return Err(format!("hart {hart} needs both stacks used or unused").into());
            }
            if let Some(task) = stacks.task_stack.used() {
                task.validate(&format!("core.{hart}.task_stack"))?;
                if task.handoff.is_some() {
                    return Err("`handoff` belongs to the bootstrap stack".into());
                }
            }
            if stacks
                .interrupt_stack
                .used()
                .and_then(|stack| stack.allocation_bytes)
                .is_some_and(|bytes| bytes == 0 || bytes % 16 != 0)
            {
                return Err(
                    "interrupt stack maximum must be positive and aligned to 16 bytes".into(),
                );
            }
        }
        Ok(())
    }

    /// Match the complete fact geometry and require its boot hart's stacks.
    pub fn check_cores(&self, cores: u8, boot_hart: u32) -> Result<()> {
        if self.core.len() != usize::from(cores) {
            return Err(format!("the stack policy names {} cores but the chip has {cores}: every hart needs both stacks", self.core.len()).into());
        }
        if self.task_stack(boot_hart).is_none() {
            return Err(format!("boot hart {boot_hart} needs both stacks").into());
        }
        Ok(())
    }

    /// The cores whose task stack the image does not use.
    pub fn unused_task_cores(&self) -> Vec<u32> {
        self.core
            .iter()
            .filter(|(_, stacks)| stacks.task_stack.used().is_none())
            .map(|(&core, _)| core)
            .collect()
    }

    /// The cores whose interrupt stack the image does not use.
    pub fn unused_interrupt_cores(&self) -> Vec<u32> {
        self.core
            .iter()
            .filter(|(_, stacks)| stacks.interrupt_stack.used().is_none())
            .map(|(&core, _)| core)
            .collect()
    }

    pub fn task_stack(&self, hart: u32) -> Option<&TaskStack> {
        self.core.get(&hart)?.task_stack.used()
    }

    pub fn task_stack_mut(&mut self, hart: u32) -> Option<&mut TaskStack> {
        match &mut self.core.get_mut(&hart)?.task_stack {
            Slot::Used(stack) => Some(stack),
            Slot::Unused(_) => None,
        }
    }

    pub fn stacks(&self) -> Result<Stacks<'_>> {
        match &self.coverage_policy {
            Some(coverage_policy) if !self.core.is_empty() => Ok(Stacks {
                coverage_policy,
                bootstrap_stack: self.bootstrap_stack.as_ref(),
            }),
            _ => Err("a stack policy needs coverage_policy and every hart's stacks".into()),
        }
    }

    pub fn runtime_task_stacks(&self) -> Vec<StackRoot<'_>> {
        self.core
            .iter()
            .filter_map(|(&hart, stacks)| {
                Some(StackRoot {
                    hart: Some(hart),
                    name: format!("CPU{hart} task stack"),
                    stack: stacks.task_stack.used()?,
                })
            })
            .collect()
    }

    /// Explicit per-hart inputs; absent allocations use that hart's chip default.
    pub fn runtime_environment(&self) -> Vec<(String, String)> {
        let mut variables = Vec::new();
        for (&hart, stacks) in &self.core {
            if let Some(task) = stacks.task_stack.used() {
                variables.push((
                    format!("OPEN_RADIO_TASK_STACK_HART{hart}_MINIMUM_FREE_BYTES"),
                    task.minimum_free_bytes.to_string(),
                ));
            }
            for (kind, allocation) in [
                (
                    "TASK",
                    stacks.task_stack.used().map_or(Some(0), |stack| {
                        stack
                            .allocation_bytes
                            .or_else(|| self.default_task_stack_bytes.get(&hart).copied())
                    }),
                ),
                (
                    "IRQ",
                    stacks
                        .interrupt_stack
                        .used()
                        .map_or(Some(0), |stack| stack.allocation_bytes),
                ),
            ] {
                if let Some(bytes) = allocation {
                    variables.push((
                        format!("OPEN_RADIO_{kind}_STACK_HART{hart}_BYTES"),
                        bytes.to_string(),
                    ));
                }
            }
        }
        variables
    }

    pub fn set_task_stack_bytes(profile: &mut crate::Profile, hart: u32, bytes: u32) -> Result<()> {
        let memory = profile
            .facts
            .memory
            .as_ref()
            .ok_or("task stacks require a memory map")?;
        if bytes == 0
            || bytes % 16 != 0
            || bytes >= memory.psram.ok_or("task stacks require PSRAM")?.length
        {
            return Err("task stack allocation does not fit aligned PSRAM storage".into());
        }
        let names = [
            format!("__runtime_cpu{hart}_task_stack_bottom"),
            format!("__runtime_cpu{hart}_task_stack_top"),
        ];
        let rule = profile
            .facts
            .placement
            .as_mut()
            .ok_or("task stacks require exact placement rules")?
            .range
            .iter_mut()
            .find(|rule| rule.range == names)
            .ok_or("missing per-hart task placement rule")?;
        if rule.region != "psram" || rule.bytes.is_none() {
            return Err("task stack requires exact PSRAM placement".into());
        }
        rule.bytes = Some(u64::from(bytes));
        Ok(())
    }

    pub fn proven_task_stacks(&self) -> Result<Vec<StackRoot<'_>>> {
        let stacks = self.runtime_task_stacks();
        if stacks.is_empty() {
            return Err("the image declares its task stacks proven but names none".into());
        }
        if stacks
            .iter()
            .any(|root| root.stack.allocation_bytes.is_some())
        {
            return Err("proven task stacks and allocation_bytes give the size two sources".into());
        }
        Ok(stacks)
    }

    pub fn runtime_profile(&self, base: &crate::Profile) -> Result<crate::Profile> {
        self.validate()?;
        let contract = base
            .facts
            .interrupts
            .as_ref()
            .ok_or("stack geometry requires interrupt facts")?;
        self.check_cores(base.facts.properties.cores, contract.boot_hart)?;
        let mut profile = base.clone();
        for (&hart, stacks) in &self.core {
            if let Some(bytes) = stacks.task_stack.used().and_then(|stack| {
                stack
                    .allocation_bytes
                    .or_else(|| self.default_task_stack_bytes.get(&hart).copied())
            }) {
                Self::set_task_stack_bytes(&mut profile, hart, bytes)?;
            }
        }
        let placement = profile
            .facts
            .placement
            .as_mut()
            .ok_or("stacks require placement rules")?;
        let interrupts = profile
            .facts
            .interrupts
            .as_mut()
            .ok_or("stacks require interrupt facts")?;
        let unused = self.unused_task_cores();
        placement.range.retain(|rule| {
            !unused.iter().any(|hart| {
                ["task", "irq"]
                    .iter()
                    .any(|kind| rule.range[0] == format!("__runtime_cpu{hart}_{kind}_stack_bottom"))
            })
        });
        interrupts.harts.retain(|hart| !unused.contains(hart));
        let mut maximum = interrupts.irq_stack_bytes;
        let mut total = 0_u64;
        for (&hart, stacks) in &self.core {
            let Some(irq) = stacks.interrupt_stack.used() else {
                continue;
            };
            let bytes = irq.allocation_bytes.unwrap_or(interrupts.irq_stack_bytes);
            let guard = interrupts
                .irq_stack_guard_bytes
                .get(&hart)
                .ok_or("missing hart guard")?;
            if bytes <= *guard || bytes % *guard != 0 {
                return Err("interrupt stack allocation does not fit the guard contract".into());
            }
            total += u64::from(bytes);
            maximum = maximum.max(bytes);
            let names = [
                format!("__runtime_cpu{hart}_irq_stack_bottom"),
                format!("__runtime_cpu{hart}_irq_stack_top"),
            ];
            let rule = placement
                .range
                .iter_mut()
                .find(|rule| rule.range == names)
                .ok_or("missing per-hart IRQ placement rule")?;
            if rule.region != "sram"
                || rule.bytes.is_some()
                || rule.maximum_bytes != Some(u64::from(interrupts.irq_stack_bytes))
            {
                return Err("interrupt stack placement and budget disagree".into());
            }
            rule.maximum_bytes = Some(u64::from(bytes));
        }
        if total
            >= u64::from(
                profile
                    .facts
                    .memory
                    .as_ref()
                    .ok_or("stacks require memory")?
                    .sram
                    .length,
            )
        {
            return Err("interrupt allocations do not fit SRAM".into());
        }
        interrupts.irq_stack_bytes = maximum;
        Ok(profile)
    }
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(Path::new("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_image_appends_observed_functions_and_invalid_selectors_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let base = directory.path().join("base.toml");
        std::fs::write(
            &base,
            "schema=6\nmax_move_bytes=4096\nobserved_functions=['core::fmt::write']",
        )
        .unwrap();
        let extension = directory.path().join("image.toml");
        std::fs::write(
            &extension,
            "schema=6\nextends='base.toml'\nobserved_functions=['app::*']",
        )
        .unwrap();
        let policy = StackPolicy::load(&extension).unwrap();
        assert_eq!(policy.observed_functions, ["core::fmt::write", "app::*"]);
        for patterns in ["['']", "['core::fmt::write']"] {
            std::fs::write(
                &extension,
                format!("schema=6\nextends='base.toml'\nobserved_functions={patterns}"),
            )
            .unwrap();
            assert!(StackPolicy::load(&extension).is_err());
        }
    }

    #[test]
    fn an_extension_adds_exceptions_without_changing_the_base_contract() {
        let directory = tempfile::tempdir().unwrap();
        let base_path = directory.path().join("base.toml");
        let base_source = include_str!("../../../../chips/esp32s31/platform/stack.toml");
        std::fs::write(&base_path, base_source).unwrap();
        let base = StackPolicy::load(&base_path).unwrap();
        let path = directory.path().join("stack.toml");
        let extension = "schema = 6\nextends = 'base.toml'\n\
            [[frame_exceptions.runtime]]\nfunction = 'test::frame::<u8>'\n\
            reason = 'the local packet buffer'\nceiling_bytes = 8192\n";
        std::fs::write(&path, extension).unwrap();
        let mut policy = StackPolicy::load(&path).unwrap();
        assert_eq!(policy.frame_exceptions.runtime.len(), 1);
        assert_eq!(policy.frame_exceptions.runtime[0].ceiling_bytes, 8192);
        policy.frame_exceptions = FrameExceptions::default();
        assert_eq!(policy, base);

        for invalid in [
            extension.replace("test::frame::<u8>", " "),
            extension.replace("the local packet buffer", " "),
            extension.replace("8192", "4096"),
            format!(
                "{extension}[[frame_exceptions.runtime]]{}",
                extension
                    .split_once("[[frame_exceptions.runtime]]")
                    .unwrap()
                    .1
            ),
        ] {
            std::fs::write(&path, invalid).unwrap();
            assert!(StackPolicy::load(&path).is_err());
        }
    }

    #[test]
    fn tracked_loads_record_the_chip_and_both_policy_files_at_their_reads() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        for (path, text) in oer_espressif_facts::tracked::FILES {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let platform = root.join("chips/esp32s31/platform");
        let policy = platform.join("policy.toml");
        let base = platform.join("base.toml");
        std::fs::write(&policy, "schema = 6\nextends = 'base.toml'\n").unwrap();
        std::fs::write(
            &base,
            include_str!("../../../../chips/esp32s31/platform/stack.toml"),
        )
        .unwrap();
        let mut inputs = oer_os_durable::inputs::Inputs::tracked();
        crate::Profile::load_with_inputs(root, "esp32s31", &mut inputs).unwrap();
        StackPolicy::load_with_inputs(&policy, &mut inputs).unwrap();
        let digests = inputs.into_digests();
        let layers = [
            "chips/espressif/platform/isa/rv32imafc.toml",
            "chips/espressif/platform/family.toml",
            "chips/esp32s31/platform/chip.toml",
            "chips/esp32s31/platform/rom-rev0.toml",
            "chips/esp32s31/platform/modules/esp32-s31-wroom.toml",
            "chips/esp32s31/platform/boards/esp32-s31-function-coreboard-1.toml",
            // The chip's stack policy sizes the default task stack.
            "chips/esp32s31/platform/stack.toml",
        ]
        .map(|path| root.join(path));
        assert_eq!(digests.len(), layers.len() + 2);
        for path in layers.into_iter().chain([policy, base]) {
            assert_eq!(digests[&path], oer_os_durable::sha256_file(&path).unwrap());
        }
    }

    fn profile() -> crate::Profile {
        crate::Profile::tracked("esp32s31").unwrap()
    }
    #[test]
    fn a_task_allocation_binds_the_link_environment_and_exact_psram_range() {
        let base = profile();
        let mut policy = policy(None);
        policy.task_stack_mut(0).unwrap().allocation_bytes = Some(1 << 20);
        let adjusted = policy.runtime_profile(&base).unwrap();
        let rule = |profile: &crate::Profile| {
            profile
                .facts
                .placement
                .as_ref()
                .unwrap()
                .range
                .iter()
                .find(|rule| {
                    rule.range
                        == [
                            format!("__runtime_cpu{}_task_stack_bottom", 0),
                            format!("__runtime_cpu{}_task_stack_top", 0),
                        ]
                })
                .unwrap()
                .bytes
        };
        assert_eq!(rule(&base), Some(0x30000));
        assert_eq!(rule(&adjusted), Some(1 << 20));
        assert_eq!(adjusted.facts.interrupts.as_ref().unwrap().harts, [0]);
        assert!(policy.runtime_environment().contains(&(
            format!("OPEN_RADIO_TASK_STACK_HART{}_BYTES", 0),
            "1048576".into()
        )));
        for bytes in [0, 4096, 1048577, 1 << 24] {
            policy.task_stack_mut(0).unwrap().allocation_bytes = Some(bytes);
            assert!(policy.runtime_profile(&base).is_err(), "{bytes}");
        }
    }
    #[test]
    fn a_proven_task_stack_is_the_bound_plus_the_reserve_aligned_up() {
        let stack = policy(None).task_stack(0).unwrap().clone();
        assert_eq!(stack.minimum_free_bytes, 4096);
        assert_eq!(stack.proven_allocation_bytes(880).unwrap(), 4976);
        assert_eq!(stack.proven_allocation_bytes(881).unwrap(), 4992);
        assert_eq!(stack.proven_allocation_bytes(0).unwrap(), 4096);
        assert!(stack.proven_allocation_bytes(u64::MAX).is_err());
        assert!(stack.proven_allocation_bytes(u64::from(u32::MAX)).is_err());
    }
    #[test]
    fn a_handoff_belongs_to_the_bootstrap_stack_alone() {
        let bootstrap = |handoff: Option<&str>| TaskStack {
            root: "_start_rust".into(),
            storage_symbol: None,
            bottom_symbol: Some("_stack_end".into()),
            top_symbol: Some("_stack_start".into()),
            minimum_free_bytes: 4096,
            allocation_bytes: None,
            handoff: handoff.map(str::to_owned),
        };
        let mut policy = policy(None);
        policy.bootstrap_stack = Some(bootstrap(Some("_runtime_start")));
        policy.validate().unwrap();
        policy.bootstrap_stack = Some(bootstrap(Some("")));
        assert!(policy.validate().is_err());
        policy.bootstrap_stack = Some(bootstrap(None));
        policy.task_stack_mut(0).unwrap().handoff = Some("_runtime_start".into());
        let error = policy.validate().unwrap_err().to_string();
        assert!(error.contains("belongs to the bootstrap stack"), "{error}");
    }
    #[test]
    fn a_proven_task_stack_and_an_explicit_allocation_are_two_sources() {
        let mut policy = policy(None);
        policy.proven_task_stacks().unwrap();
        policy.task_stack_mut(0).unwrap().allocation_bytes = Some(1 << 20);
        let error = policy.proven_task_stacks().unwrap_err().to_string();
        assert!(error.contains("two sources"), "{error}");
        policy.core.clear();
        assert!(policy.proven_task_stacks().is_err());
    }
    #[test]
    fn the_linked_task_stack_size_replaces_the_exact_range() {
        let mut profile = profile();
        StackPolicy::set_task_stack_bytes(&mut profile, 0, 17264).unwrap();
        let rule = profile
            .facts
            .placement
            .as_ref()
            .unwrap()
            .range
            .iter()
            .find(|rule| {
                rule.range
                    == [
                        format!("__runtime_cpu{}_task_stack_bottom", 0),
                        format!("__runtime_cpu{}_task_stack_top", 0),
                    ]
            })
            .unwrap();
        assert_eq!(rule.bytes, Some(17264));
        assert!(StackPolicy::set_task_stack_bytes(&mut profile, 0, 1 << 24).is_err());
    }
    fn unused_core() -> CoreStacks {
        CoreStacks {
            task_stack: Slot::Unused(Unused { unused: true }),
            interrupt_stack: Slot::Unused(Unused { unused: true }),
        }
    }
    fn policy(bytes: Option<u32>) -> StackPolicy {
        StackPolicy {
            schema: SCHEMA,
            coverage_policy: Some("own-memory".into()),
            max_move_bytes: 4096,
            default_task_stack_bytes: BTreeMap::new(),
            frame_exceptions: FrameExceptions::default(),
            observed_functions: Vec::new(),
            core: BTreeMap::from([
                (1, unused_core()),
                (
                    0,
                    CoreStacks {
                        task_stack: Slot::Used(TaskStack {
                            root: "runtime_main".into(),
                            storage_symbol: Some("task_stack".into()),
                            bottom_symbol: None,
                            top_symbol: None,
                            minimum_free_bytes: 4096,
                            allocation_bytes: None,
                            handoff: None,
                        }),
                        interrupt_stack: Slot::Used(InterruptStack {
                            allocation_bytes: bytes,
                        }),
                    },
                ),
            ]),
            bootstrap_stack: None,
        }
    }
    #[test]
    fn an_image_allocation_changes_placement_and_budget_together() {
        let base = profile();
        let before = base.clone();
        let unchanged = policy(None).runtime_profile(&base).unwrap();
        assert_eq!(unchanged.facts.interrupts.as_ref().unwrap().harts, [0]);
        let requested = base.facts.interrupts.as_ref().unwrap().irq_stack_bytes * 2;
        let image = policy(Some(requested)).runtime_profile(&base).unwrap();
        let contract = image.facts.interrupts.as_ref().unwrap();
        assert_eq!(contract.irq_stack_bytes, requested);
        assert_eq!(
            contract.irq_stack_guard_bytes,
            base.facts
                .interrupts
                .as_ref()
                .unwrap()
                .irq_stack_guard_bytes
        );
        assert_eq!(
            contract.irq_stack_margin_percent,
            base.facts
                .interrupts
                .as_ref()
                .unwrap()
                .irq_stack_margin_percent
        );
        assert_eq!(contract.harts, [0]);
        for hart in &contract.harts {
            let bottom = format!("__runtime_cpu{hart}_irq_stack_bottom");
            let range = image
                .facts
                .placement
                .as_ref()
                .unwrap()
                .range
                .iter()
                .find(|rule| rule.range[0] == bottom)
                .unwrap();
            assert_eq!(range.maximum_bytes, Some(u64::from(requested)));
            assert_eq!(range.region, "sram");
        }
        assert_eq!(base, before);
        assert!(policy(Some(requested)).runtime_environment().contains(&(
            format!("OPEN_RADIO_IRQ_STACK_HART{}_BYTES", 0),
            requested.to_string()
        )));
        assert!(
            !policy(None)
                .runtime_environment()
                .iter()
                .any(|(key, _)| *key == format!("OPEN_RADIO_IRQ_STACK_HART{}_BYTES", 0))
        );
    }
    #[test]
    fn a_larger_budget_requires_consistent_exact_sram_placement() {
        let mut base = profile();
        let requested = base.facts.interrupts.as_ref().unwrap().irq_stack_bytes * 2;
        let name = format!("__runtime_cpu{}_irq_stack_bottom", 0);
        let rule = base
            .facts
            .placement
            .as_mut()
            .unwrap()
            .range
            .iter_mut()
            .find(|rule| rule.range[0] == name)
            .unwrap();
        rule.bytes = Some(1);
        assert!(policy(Some(requested)).runtime_profile(&base).is_err());
        base.facts
            .placement
            .as_mut()
            .unwrap()
            .range
            .retain(|rule| rule.range[0] != name);
        assert!(policy(Some(requested)).runtime_profile(&base).is_err());
    }
    #[test]
    fn bad_or_unaffordable_allocations_are_refused() {
        let base = profile();
        let maximum = base.facts.interrupts.as_ref().unwrap().irq_stack_bytes;
        for bytes in [0, 4096, maximum + 16, 65537, u32::MAX - 15] {
            assert!(policy(Some(bytes)).runtime_profile(&base).is_err());
        }
    }

    fn parse(source: &str) -> Result<StackPolicy> {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("stack.toml");
        std::fs::write(&path, source).unwrap();
        StackPolicy::load(&path)
    }
    const CORE_0: &str = "schema = 6\nmax_move_bytes = 4096\ncoverage_policy = 'c.toml'\n\
        [core.0.task_stack]\nroot = 'runtime_main'\nstorage_symbol = 's'\n\
        minimum_free_bytes = 4096\n[core.0.interrupt_stack]\n";
    #[test]
    fn a_policy_names_every_hart_of_synthetic_geometries_used_or_unused() {
        for harts in [1, 3, 7] {
            let mut source = CORE_0.to_string();
            for hart in 1..harts {
                source.push_str(&format!("[core.{hart}.task_stack]\nunused=true\n[core.{hart}.interrupt_stack]\nunused=true\n"));
            }
            let policy = parse(&source).unwrap();
            policy.check_cores(harts, 0).unwrap();
            assert!(policy.check_cores(harts + 1, 0).is_err());
            assert_eq!(
                policy.unused_task_cores(),
                (1..u32::from(harts)).collect::<Vec<_>>()
            );
            assert_eq!(policy.unused_interrupt_cores(), policy.unused_task_cores());
            if harts > 1 {
                assert!(policy.check_cores(harts, u32::from(harts - 1)).is_err());
            }
        }
    }
    #[test]
    fn an_unused_stack_names_nothing_and_a_gap_or_half_a_core_is_refused() {
        let core_1 = |task: &str, interrupt: &str| {
            format!("{CORE_0}[core.1.task_stack]\n{task}\n[core.1.interrupt_stack]\n{interrupt}\n")
        };
        for refused in [
            // unused together with a root, or not unused at all
            core_1("unused = true\nroot = 'start'", "unused = true"),
            core_1("unused = false", "unused = true"),
            core_1("unused = true", "unused = false"),
            // a core that runs a task takes interrupts
            core_1(
                "root = 'core1_main'\nstorage_symbol = 's1'\nminimum_free_bytes = 4096",
                "unused = true",
            ),
            // only core 0 sizes the interrupt stacks
            core_1("unused = true", "allocation_bytes = 4096"),
        ] {
            assert!(parse(&refused).is_err(), "{refused}");
        }
        // core 1 without core 0, core 0 unused, or a core number skipped
        let gap = CORE_0.replace("core.0", "core.1");
        assert!(parse(&gap).is_err());
        let unused_zero = CORE_0.replace(
            "[core.0.task_stack]\nroot = 'runtime_main'\nstorage_symbol = 's'\n\
        minimum_free_bytes = 4096\n",
            "[core.0.task_stack]\nunused = true\n",
        );
        assert!(parse(&unused_zero).is_err());
        let used = core_1(
            "root = 'core1_main'\nstorage_symbol = 's1'\nminimum_free_bytes = 4096",
            "",
        );
        let policy = parse(&used).unwrap();
        assert!(policy.unused_task_cores().is_empty());
    }
    #[test]
    fn unused_interrupt_stacks_leave_the_contract_and_the_link_environment_names_them() {
        let base = profile();
        let image = policy(None).runtime_profile(&base).unwrap();
        let contract = image.facts.interrupts.as_ref().unwrap();
        assert_eq!(contract.harts, [0]);
        let placed = |profile: &crate::Profile, hart: u32| {
            profile
                .facts
                .placement
                .as_ref()
                .unwrap()
                .range
                .iter()
                .any(|rule| rule.range[0] == format!("__runtime_cpu{hart}_irq_stack_bottom"))
        };
        assert!(placed(&base, 1) && !placed(&image, 1) && placed(&image, 0));
        assert!(
            policy(None)
                .runtime_environment()
                .contains(&(format!("OPEN_RADIO_IRQ_STACK_HART{}_BYTES", 1), "0".into()))
        );
        let mut both = policy(None);
        both.core.remove(&1);
        assert!(both.runtime_profile(&base).is_err());
        let environment = both.runtime_environment();
        assert!(environment.iter().all(|(key, _)| !key.contains("UNUSED")));
    }
}

#[cfg(test)]
mod default_tests {
    use super::*;

    #[test]
    fn per_hart_defaults_seed_proven_stacks_and_unused_slots_still_emit_zero() {
        for harts in [3, 7] {
            let last = harts - 1;
            let mut source = String::from(
                "schema=6\ncoverage_policy='coverage.toml'\nmax_move_bytes=4096\ndefault_task_stack_bytes={",
            );
            source.push_str(
                &(0..harts)
                    .map(|hart| {
                        format!(
                            "{hart}={}",
                            if hart == 0 || hart == last { 65536 } else { 0 }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(","),
            );
            source.push_str("}\n");
            for hart in 0..harts {
                if hart == 0 || hart == last {
                    source.push_str(&format!("[core.{hart}.task_stack]\nroot='entry_{hart}'\nstorage_symbol='task_{hart}'\nminimum_free_bytes=16\n[core.{hart}.interrupt_stack]\n"));
                } else {
                    source.push_str(&format!("[core.{hart}.task_stack]\nunused=true\n[core.{hart}.interrupt_stack]\nunused=true\n"));
                }
            }
            let policy: StackPolicy = toml::from_str(&source).unwrap();
            policy.validate().unwrap();
            assert_eq!(policy.proven_task_stacks().unwrap().len(), 2);
            let environment = policy.runtime_environment();
            let mut base = crate::Profile::tracked("esp32s31").unwrap();
            base.facts.properties.cores = harts as u8;
            let contract = base.facts.interrupts.as_mut().unwrap();
            let guard = contract.irq_stack_guard_bytes[&contract.boot_hart];
            contract.harts = (0..harts).collect();
            contract.irq_stack_guard_bytes = (0..harts).map(|hart| (hart, guard)).collect();
            let placement = base.facts.placement.as_mut().unwrap();
            let task_template = placement
                .range
                .iter()
                .find(|rule| rule.range[0].ends_with("task_stack_bottom"))
                .unwrap()
                .clone();
            let irq_template = placement
                .range
                .iter()
                .find(|rule| rule.range[0].ends_with("irq_stack_bottom"))
                .unwrap()
                .clone();
            placement
                .range
                .retain(|rule| !rule.range[0].ends_with("stack_bottom"));
            for hart in 0..harts {
                for (kind, template) in [("task", &task_template), ("irq", &irq_template)] {
                    let mut rule = template.clone();
                    rule.range = [
                        format!("__runtime_cpu{hart}_{kind}_stack_bottom"),
                        format!("__runtime_cpu{hart}_{kind}_stack_top"),
                    ];
                    placement.range.push(rule);
                }
            }
            let adjusted = policy.runtime_profile(&base).unwrap();
            assert_eq!(adjusted.facts.interrupts.as_ref().unwrap().harts, [0, last]);
            let rule = adjusted
                .facts
                .placement
                .as_ref()
                .unwrap()
                .range
                .iter()
                .find(|rule| rule.range[0] == format!("__runtime_cpu{last}_task_stack_bottom"))
                .unwrap();
            assert_eq!(rule.bytes, Some(65536));
            for hart in 0..harts {
                assert!(
                    environment.contains(&(
                        format!("OPEN_RADIO_TASK_STACK_HART{hart}_BYTES"),
                        if hart == 0 || hart == last {
                            "65536"
                        } else {
                            "0"
                        }
                        .into()
                    ))
                );
            }
        }
    }
}
