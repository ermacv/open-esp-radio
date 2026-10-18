//! The chip a scenario binary compares.
//!
//! Each chip's scenario crate owns its pins, ROM facts, stack placement and
//! reviewed decisions, and installs them once before any scenario runs. The
//! engine reads them from here and holds no chip facts of its own.
use std::sync::OnceLock;

/// One chip's scenario configuration.
pub struct Chip {
    /// Chip identifier, as `platform/<id>/chip.toml` names it.
    pub name: &'static str,
    /// The tracked artifact manifest, relative to the repository root.
    pub manifest: &'static str,
    /// Its text, compiled into the scenario binary.
    pub manifest_text: &'static str,
    /// Production hardware sources the observation analysis attributes
    /// executed lines to, relative to the repository root.
    pub hardware_scope: &'static str,
    /// Chip-neutral production hardware sources the chip's probes compile,
    /// such as the register blocks and transactions it shares with other
    /// chips, relative to the repository root.
    pub shared_scopes: &'static [&'static str],
    /// Session input index of the authenticated ROM ELF.
    pub rom_input: u64,
    /// ROM storage the scenarios address directly, as (symbol, address,
    /// size), checked against the captured ROM inventory.
    pub rom_symbols: &'static [(&'static str, u32, u64)],
    /// Caller-owned stack of every compared call, as (address, bytes).
    pub stack: (u32, u32),
    /// The reviewed coverage decisions of every scenario, relative to the
    /// repository root and outside the scenario crate, so that a shard
    /// depends only on the decisions that apply to its closures.
    pub coverage: &'static str,
    /// Reviewed decisions on unobserved production lines.
    pub observation: &'static [crate::observation::Decision],
    /// Reviewed decisions on unprojected vendor state.
    pub state: &'static [crate::state::Decision],
    /// PHY session layout, for chips whose scenarios link the PHY archive.
    pub phy: Option<&'static crate::phy::PhyLayout>,
    /// The instruction set both implementations run under.
    pub isa: Isa,
    /// The published register bindings the triage report names addresses
    /// and fields with, relative to the repository root.
    pub registers: &'static str,
}

/// The instruction set a chip's code is built for. The executor stops a
/// run at any instruction outside it, so code the chip could not execute
/// never produces evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Isa {
    /// RV32IMAFC with Zba, Zbb, Zbs, Zcb and Zcmp, as ESP-IDF builds the
    /// `riscv32imafc` chips (floating point stays unsupported by the
    /// executor).
    Rv32imafcZbaZbbZbsZcbZcmp,
    /// Base RV32IMAC, as ESP-IDF builds the `riscv32imac` chips.
    Rv32imac,
}

impl Isa {
    /// The executor that admits exactly this instruction set.
    pub fn executor(self) -> &'static dyn blobray_domain::Executor {
        match self {
            Self::Rv32imafcZbaZbbZbsZcbZcmp => &blobray_backend_riscv::RiscvExecutor,
            Self::Rv32imac => &blobray_backend_riscv::Rv32imacExecutor,
        }
    }
}

impl Chip {
    /// The chip's PHY session layout.
    pub fn phy(&self) -> &'static crate::phy::PhyLayout {
        self.phy
            .unwrap_or_else(|| panic!("{} declares no PHY session layout", self.name))
    }
}

static CHIP: OnceLock<&'static Chip> = OnceLock::new();

/// Install `chip` for this process; installing the same chip again is a
/// no-op, and a different one is refused.
pub fn install(chip: &'static Chip) {
    let installed = CHIP.get_or_init(|| chip);
    assert!(
        std::ptr::eq(*installed, chip),
        "scenario engine already runs for {}",
        installed.name
    );
}

/// The installed chip.
pub fn chip() -> &'static Chip {
    CHIP.get()
        .copied()
        .expect("the scenario crate installs its chip before running scenarios")
}

/// A chip for the engine's own tests, whose hardware scope is a test tree.
#[cfg(test)]
pub(crate) static TEST: Chip = Chip {
    name: "test",
    manifest: "verification/test/artifacts.toml",
    manifest_text: "schema = 1\nsource = []\nartifact = []\n",
    hardware_scope: "crates/hardware/test",
    shared_scopes: &["crates/hardware/shared-test"],
    rom_input: 1,
    rom_symbols: &[],
    stack: (0, 0),
    coverage: "verification/test/decisions/coverage.toml",
    observation: &[],
    state: &[],
    phy: Some(&TEST_PHY),
    isa: Isa::Rv32imac,
    registers: "registers/test/published/radio.bindings.toml",
};

/// The test chip's PHY layout: distinct transport and scratch addresses.
#[cfg(test)]
static TEST_PHY: crate::phy::PhyLayout = crate::phy::PhyLayout {
    library: "libphy",
    rom: "rom",
    phy_sdk: Some("phy-sdk"),
    input: 0x1000,
    output: 0x2000,
    parameter_copy: 0x1800,
    phy_param_bytes: 16,
    rom_interface_pointer: 0x3000,
    rom_parameter_pointer: 0x3004,
    rom_callback_table: 0x3100,
    image_code_start: 0x1_0000,
    image_data_start: 0x2_0000,
    image_region_bytes: 0x1_0000,
    i2c_ports: [0x4000, 0x4004],
    i2c_read_mask: 0x401c,
    i2c_host_map: 0x4020,
    radio_mmio: 0x4000,
    radio_mmio_bytes: 0x1000,
};
