//! HIL image identity and reproducible build feature recipes.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ImageClass {
    SystemWatchdog,
    /// The radio-free image with the product panic entry alone, without the
    /// HIL panic report: a panic records itself and resets the chip.
    SystemPanicReset,
    BootSmoke,
    /// The system watchdog image that, on request, switches its USB
    /// Serial/JTAG off its pads: the board leaves USB, for the stand's
    /// recovery scenario.
    DiagnosticUsbJtagOff,
}

impl ImageClass {
    /// Effective Cargo feature selection used by both the builder and
    /// provenance, with `network`, the Cargo feature of the image's network
    /// integration.
    pub fn build_features(self, network: &str) -> String {
        if matches!(
            self,
            Self::SystemWatchdog | Self::SystemPanicReset | Self::DiagnosticUsbJtagOff
        ) {
            self.runtime_features().to_owned()
        } else {
            format!("{},{}", self.runtime_features(), network)
        }
    }

    pub const ALL: [Self; 4] = [
        Self::SystemWatchdog,
        Self::SystemPanicReset,
        Self::BootSmoke,
        Self::DiagnosticUsbJtagOff,
    ];

    /// Whether the image adds observers or probes that the product does
    /// not carry. Its observations can never qualify a product.
    pub const fn diagnostic(self) -> bool {
        matches!(self, Self::DiagnosticUsbJtagOff)
    }

    pub const fn id(self) -> &'static str {
        match self {
            Self::SystemWatchdog => "system-watchdog",
            Self::SystemPanicReset => "system-panic-reset",
            Self::BootSmoke => "boot-smoke",
            Self::DiagnosticUsbJtagOff => "diagnostic-usb-jtag-off",
        }
    }

    pub const fn runtime_features(self) -> &'static str {
        match self {
            Self::SystemWatchdog => "system-watchdog",
            Self::SystemPanicReset => "system-panic-reset",
            Self::DiagnosticUsbJtagOff => "system-watchdog,usb-jtag-off",
            Self::BootSmoke => "boot-smoke",
        }
    }

    /// The one stage-two placement every image uses: code, data and task
    /// stacks in PSRAM, interrupt stacks in SRAM.
    pub const fn runtime_profile(self) -> &'static str {
        "psram-code-psram-data-psram-stack"
    }
}

impl std::str::FromStr for ImageClass {
    type Err = String;

    fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|class| class.id() == value)
            .ok_or_else(|| format!("unknown image class `{value}`"))
    }
}
