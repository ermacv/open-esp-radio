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
    /// This image's allocation per hart. Absent uses the chip layout.
    #[serde(default)]
    pub allocation_bytes: Option<u32>,
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
        if let Some(interrupts) = stacks.interrupt_stacks
            && let Some(bytes) = interrupts.allocation_bytes
            && (bytes % 16 != 0 || bytes <= interrupts.minimum_free_bytes)
        {
            return Err(
                "interrupt stack allocation must be aligned to 16 bytes and exceed its reserve"
                    .into(),
            );
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
            if let Some(bytes) = interrupts.allocation_bytes {
                variables.push(("OPEN_RADIO_IRQ_STACK_BYTES", bytes.to_string()));
            }
        }
        variables
    }

    /// Apply an image's explicit allocation to both its exact placement
    /// ranges and its interrupt-stack budget. SRAM, guards and margins stay
    /// those of the chip; the placement gate verifies the linked ranges.
    pub fn runtime_profile(&self, base: &crate::Profile) -> Result<crate::Profile> {
        self.validate()?;
        let mut profile = base.clone();
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
        let reserve = self
            .interrupt_stacks
            .as_ref()
            .ok_or("interrupt stack policy missing")?
            .minimum_free_bytes;
        if interrupts
            .irq_stack_guard_bytes
            .values()
            .any(|guard| bytes <= guard.saturating_add(reserve))
            || u64::from(bytes) * interrupts.harts.len() as u64 >= u64::from(memory.sram.length)
        {
            return Err(
                "interrupt stack allocation does not fit the SRAM/guard/reserve contract".into(),
            );
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
            if rule.region != "sram" || rule.bytes != Some(u64::from(interrupts.irq_stack_bytes)) {
                return Err("interrupt stack placement and budget disagree".into());
            }
            rule.bytes = Some(u64::from(bytes));
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
    fn policy(bytes: Option<u32>) -> StackPolicy {
        StackPolicy {
            schema: SCHEMA,
            coverage_policy: Some("own-memory".into()),
            max_move_bytes: 4096,
            cpu0_task_stack: Some(TaskStack {
                root: "runtime_main".into(),
                storage_symbol: Some("task_stack".into()),
                bottom_symbol: None,
                top_symbol: None,
                minimum_free_bytes: 4096,
            }),
            interrupt_stacks: Some(InterruptStacks {
                minimum_free_bytes: 4096,
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
            assert_eq!(range.bytes, Some(u64::from(requested)));
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
