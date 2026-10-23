//! The stack policy of an image family (`stack.toml`, schema 5): the stacks
//! the gate bounds, each by the function that runs on it, the compiler move
//! limit and the runtime headroom the firmware's stack painting enforces.
//!
//! A policy that names no stacks sets only the compiler move and function-frame limit: the
//! image is compiled with every image flag (frame records included), and the
//! whole-stack check has nothing to bound. A policy that names stacks names the
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
    /// The largest recorded function frame and implicit copy or move admitted
    /// (`-Z move-size-limit` with `-D large-assignments`). Larger values live
    /// in placed static storage; futures and task arguments borrow them.
    pub max_move_bytes: u64,
    /// Reviewed frames above the move limit, scoped to each ELF of the image.
    #[serde(default)]
    pub frame_exceptions: FrameExceptions,
    /// Demangled function-name patterns watched by the reachability report.
    /// `*` matches any substring; all other characters are literal.
    #[serde(default)]
    pub observed_functions: Vec<String>,
    /// The runtime's primary task stack.
    #[serde(default)]
    pub cpu0_task_stack: Option<TaskStack>,
    #[serde(default)]
    pub interrupt_stacks: Option<InterruptStacks>,
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
    /// This image's CPU0 PSRAM stack allocation. Absent uses the chip layout.
    #[serde(default)]
    pub allocation_bytes: Option<u32>,
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

/// The dedicated SRAM interrupt stacks: their bound and margin are the
/// interrupt-stack gate's contract, without an independent free-byte reserve.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InterruptStacks {
    /// This image's maximum allocation per hart. Absent uses the chip layout.
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
        if let Some(interrupts) = stacks.interrupt_stacks
            && let Some(bytes) = interrupts.allocation_bytes
            && (bytes == 0 || bytes % 16 != 0)
        {
            return Err("interrupt stack maximum must be positive and aligned to 16 bytes".into());
        }
        stacks.cpu0_task_stack.validate("cpu0_task_stack")?;
        if let Some(bootstrap) = stacks.bootstrap_stack {
            bootstrap.validate("bootstrap_stack")?;
            if bootstrap.allocation_bytes.is_some() {
                return Err(
                    "bootstrap stack allocation cannot be overridden by a runtime policy".into(),
                );
            }
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

    /// The headroom reserves the runtime's stack painting enforces, as the
    /// build environment hands them to the firmware.
    pub fn runtime_environment(&self) -> Vec<(&'static str, String)> {
        let mut variables = Vec::new();
        if let Some(cpu0) = &self.cpu0_task_stack {
            variables.push((
                "OPEN_RADIO_CPU0_STACK_MINIMUM_FREE_BYTES",
                cpu0.minimum_free_bytes.to_string(),
            ));
            if let Some(bytes) = cpu0.allocation_bytes {
                variables.push(("OPEN_RADIO_CPU0_TASK_STACK_BYTES", bytes.to_string()));
            }
        }
        if let Some(interrupts) = &self.interrupt_stacks {
            if let Some(bytes) = interrupts.allocation_bytes {
                variables.push(("OPEN_RADIO_IRQ_STACK_BYTES", bytes.to_string()));
            }
        }
        variables
    }

    /// Set the exact size of the CPU0 task stack's placement range: an
    /// explicit allocation, or the size the image linker chose from the
    /// stack's proven bound.
    pub fn set_task_stack_bytes(profile: &mut crate::Profile, bytes: u32) -> Result<()> {
        let memory = profile
            .memory
            .as_ref()
            .ok_or("a task stack allocation requires the chip memory map")?;
        if bytes
            >= memory
                .psram
                .ok_or("a task stack allocation requires PSRAM")?
                .length
        {
            return Err("task stack allocation does not fit PSRAM".into());
        }
        let placement = profile
            .placement
            .as_mut()
            .ok_or("a task stack allocation requires exact placement rules")?;
        let rule = placement
            .range
            .iter_mut()
            .find(|rule| rule.range == ["_stack_end", "_stack_start"])
            .ok_or("the chip has no staged CPU0 task stack placement rule")?;
        if rule.region != "psram" || rule.bytes.is_none() {
            return Err("the CPU0 task stack requires an exact PSRAM placement rule".into());
        }
        rule.bytes = Some(u64::from(bytes));
        Ok(())
    }

    /// The CPU0 task stack of an image that declares it proven. Its size
    /// comes from the proven bound, so the policy must not also name one.
    pub fn proven_task_stack(&self) -> Result<&TaskStack> {
        let stack = self
            .cpu0_task_stack
            .as_ref()
            .ok_or("the image declares its task stack proven, but its policy names none")?;
        if stack.allocation_bytes.is_some() {
            return Err(
                "the image declares its task stack proven and its policy sets \
                        `allocation_bytes`: the size has two sources"
                    .into(),
            );
        }
        Ok(stack)
    }

    /// Apply an image's explicit allocation to both its exact placement
    /// ranges and its interrupt-stack budget. SRAM, guards and margins stay
    /// those of the chip; the placement gate verifies the linked ranges.
    pub fn runtime_profile(&self, base: &crate::Profile) -> Result<crate::Profile> {
        self.validate()?;
        let mut profile = base.clone();
        if let Some(bytes) = self
            .cpu0_task_stack
            .as_ref()
            .and_then(|stack| stack.allocation_bytes)
        {
            Self::set_task_stack_bytes(&mut profile, bytes)?;
        }
        let Some(bytes) = self
            .interrupt_stacks
            .as_ref()
            .and_then(|stacks| stacks.allocation_bytes)
        else {
            return Ok(profile);
        };
        let memory = profile
            .memory
            .as_ref()
            .ok_or("an interrupt stack allocation requires the chip memory map")?;
        let interrupts = profile
            .interrupts
            .as_mut()
            .ok_or("an interrupt stack allocation requires the interrupt contract")?;
        if interrupts
            .irq_stack_guard_bytes
            .values()
            .any(|guard| bytes <= *guard || bytes % *guard != 0)
            || u64::from(bytes) * interrupts.harts.len() as u64 >= u64::from(memory.sram.length)
        {
            return Err("interrupt stack allocation does not fit the SRAM/guard contract".into());
        }
        let placement = profile
            .placement
            .as_mut()
            .ok_or("an interrupt stack allocation requires exact placement rules")?;
        for hart in &interrupts.harts {
            let names = [
                format!("__runtime_cpu{hart}_irq_stack_bottom"),
                format!("__runtime_cpu{hart}_irq_stack_top"),
            ];
            let rule = placement
                .range
                .iter_mut()
                .find(|rule| rule.range == names)
                .ok_or("the chip has no exact placement rule for an interrupt stack")?;
            if rule.region != "sram"
                || rule.bytes.is_some()
                || rule.maximum_bytes != Some(u64::from(interrupts.irq_stack_bytes))
            {
                return Err("interrupt stack placement and budget disagree".into());
            }
            rule.maximum_bytes = Some(u64::from(bytes));
        }
        interrupts.irq_stack_bytes = bytes;
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
            "schema=5\nmax_move_bytes=4096\nobserved_functions=['core::fmt::write']",
        )
        .unwrap();
        let extension = directory.path().join("image.toml");
        std::fs::write(
            &extension,
            "schema=5\nextends='base.toml'\nobserved_functions=['app::*']",
        )
        .unwrap();
        let policy = StackPolicy::load(&extension).unwrap();
        assert_eq!(policy.observed_functions, ["core::fmt::write", "app::*"]);
        for patterns in ["['']", "['core::fmt::write']"] {
            std::fs::write(
                &extension,
                format!("schema=5\nextends='base.toml'\nobserved_functions={patterns}"),
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
        let extension = "schema = 5\nextends = 'base.toml'\n\
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
        let platform = root.join("chips/esp32s31/platform");
        std::fs::create_dir_all(&platform).unwrap();
        let chip = platform.join("chip.toml");
        let policy = platform.join("policy.toml");
        let base = platform.join("base.toml");
        std::fs::write(
            &chip,
            include_str!("../../../../chips/esp32s31/platform/chip.toml"),
        )
        .unwrap();
        std::fs::write(&policy, "schema = 5\nextends = 'base.toml'\n").unwrap();
        std::fs::write(
            &base,
            include_str!("../../../../chips/esp32s31/platform/stack.toml"),
        )
        .unwrap();
        let mut inputs = oer_os_durable::inputs::Inputs::tracked();
        crate::Profile::load_with_inputs(root, "esp32s31", &mut inputs).unwrap();
        StackPolicy::load_with_inputs(&policy, &mut inputs).unwrap();
        let digests = inputs.into_digests();
        assert_eq!(digests.len(), 3);
        for path in [chip, policy, base] {
            assert_eq!(digests[&path], oer_os_durable::sha256_file(&path).unwrap());
        }
    }

    fn profile() -> crate::Profile {
        toml::from_str(include_str!(
            "../../../../chips/esp32s31/platform/chip.toml"
        ))
        .unwrap()
    }
    #[test]
    fn a_task_allocation_binds_the_link_environment_and_exact_psram_range() {
        let base = profile();
        let mut policy = policy(None);
        policy.cpu0_task_stack.as_mut().unwrap().allocation_bytes = Some(1 << 20);
        let adjusted = policy.runtime_profile(&base).unwrap();
        let rule = |profile: &crate::Profile| {
            profile
                .placement
                .as_ref()
                .unwrap()
                .range
                .iter()
                .find(|rule| rule.range == ["_stack_end", "_stack_start"])
                .unwrap()
                .bytes
        };
        assert_eq!(rule(&base), Some(0x30000));
        assert_eq!(rule(&adjusted), Some(1 << 20));
        assert_eq!(adjusted.interrupts, base.interrupts);
        assert!(
            policy
                .runtime_environment()
                .contains(&("OPEN_RADIO_CPU0_TASK_STACK_BYTES", "1048576".into()))
        );
        for bytes in [0, 4096, 1048577, 1 << 24] {
            policy.cpu0_task_stack.as_mut().unwrap().allocation_bytes = Some(bytes);
            assert!(policy.runtime_profile(&base).is_err(), "{bytes}");
        }
    }
    #[test]
    fn a_proven_task_stack_is_the_bound_plus_the_reserve_aligned_up() {
        let stack = policy(None).cpu0_task_stack.unwrap();
        assert_eq!(stack.minimum_free_bytes, 4096);
        assert_eq!(stack.proven_allocation_bytes(880).unwrap(), 4976);
        assert_eq!(stack.proven_allocation_bytes(881).unwrap(), 4992);
        assert_eq!(stack.proven_allocation_bytes(0).unwrap(), 4096);
        assert!(stack.proven_allocation_bytes(u64::MAX).is_err());
        assert!(stack.proven_allocation_bytes(u64::from(u32::MAX)).is_err());
    }
    #[test]
    fn a_proven_task_stack_and_an_explicit_allocation_are_two_sources() {
        let mut policy = policy(None);
        policy.proven_task_stack().unwrap();
        policy.cpu0_task_stack.as_mut().unwrap().allocation_bytes = Some(1 << 20);
        let error = policy.proven_task_stack().unwrap_err().to_string();
        assert!(error.contains("two sources"), "{error}");
        policy.cpu0_task_stack = None;
        assert!(policy.proven_task_stack().is_err());
    }
    #[test]
    fn the_linked_task_stack_size_replaces_the_exact_range() {
        let mut profile = profile();
        StackPolicy::set_task_stack_bytes(&mut profile, 17264).unwrap();
        let rule = profile
            .placement
            .as_ref()
            .unwrap()
            .range
            .iter()
            .find(|rule| rule.range == ["_stack_end", "_stack_start"])
            .unwrap();
        assert_eq!(rule.bytes, Some(17264));
        assert!(StackPolicy::set_task_stack_bytes(&mut profile, 1 << 24).is_err());
    }
    fn policy(bytes: Option<u32>) -> StackPolicy {
        StackPolicy {
            schema: SCHEMA,
            coverage_policy: Some("own-memory".into()),
            max_move_bytes: 4096,
            frame_exceptions: FrameExceptions::default(),
            observed_functions: Vec::new(),
            cpu0_task_stack: Some(TaskStack {
                root: "runtime_main".into(),
                storage_symbol: Some("task_stack".into()),
                bottom_symbol: None,
                top_symbol: None,
                minimum_free_bytes: 4096,
                allocation_bytes: None,
            }),
            interrupt_stacks: Some(InterruptStacks {
                allocation_bytes: bytes,
            }),
            bootstrap_stack: None,
        }
    }
    #[test]
    fn an_image_allocation_changes_placement_and_budget_together() {
        let base = profile();
        let before = base.clone();
        assert_eq!(policy(None).runtime_profile(&base).unwrap(), base);
        let requested = base.interrupts.as_ref().unwrap().irq_stack_bytes * 2;
        let image = policy(Some(requested)).runtime_profile(&base).unwrap();
        let contract = image.interrupts.as_ref().unwrap();
        assert_eq!(contract.irq_stack_bytes, requested);
        assert_eq!(
            contract.irq_stack_guard_bytes,
            base.interrupts.as_ref().unwrap().irq_stack_guard_bytes
        );
        assert_eq!(
            contract.irq_stack_margin_percent,
            base.interrupts.as_ref().unwrap().irq_stack_margin_percent
        );
        for hart in &contract.harts {
            let bottom = format!("__runtime_cpu{hart}_irq_stack_bottom");
            let range = image
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
        assert_eq!(
            policy(Some(requested)).runtime_profile(&image).unwrap(),
            image
        );
        assert!(
            policy(Some(requested))
                .runtime_environment()
                .contains(&("OPEN_RADIO_IRQ_STACK_BYTES", requested.to_string()))
        );
        assert!(
            !policy(None)
                .runtime_environment()
                .iter()
                .any(|(key, _)| *key == "OPEN_RADIO_IRQ_STACK_BYTES")
        );
    }
    #[test]
    fn a_larger_budget_requires_consistent_exact_sram_placement() {
        let mut base = profile();
        let requested = base.interrupts.as_ref().unwrap().irq_stack_bytes * 2;
        let name = "__runtime_cpu0_irq_stack_bottom";
        let rule = base
            .placement
            .as_mut()
            .unwrap()
            .range
            .iter_mut()
            .find(|rule| rule.range[0] == name)
            .unwrap();
        rule.bytes = Some(1);
        assert!(policy(Some(requested)).runtime_profile(&base).is_err());
        base.placement
            .as_mut()
            .unwrap()
            .range
            .retain(|rule| rule.range[0] != name);
        assert!(policy(Some(requested)).runtime_profile(&base).is_err());
    }
    #[test]
    fn bad_or_unaffordable_allocations_are_refused() {
        let base = profile();
        for bytes in [0, 4096, 65537, u32::MAX - 15] {
            assert!(policy(Some(bytes)).runtime_profile(&base).is_err());
        }
    }
}
