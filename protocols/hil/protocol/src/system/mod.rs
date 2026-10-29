//! The system module: SoC diagnostics independent of any radio: the
//! watchdog and USB-off injections and stack high-water marks.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod payload;
pub use payload::*;
mod aes;
pub use aes::*;

crate::messages! {
    /// The image installs the HIL panic hook in its platform entry.
    property PanicHook = "system/panic-hook";
    /// The image mounts the USB Serial/JTAG-off diagnostic variant.
    property UsbJtagOff = "system/usb-jtag-off";
    endpoint WatchdogTest = "system/watchdog/test" => WatchdogArmed;
    topic WatchdogArmed = "system/watchdog/armed";
    endpoint DisableUsb = "system/usb/disable" => crate::system::UsbDisabled;
    topic UsbDisabled = "system/usb/disabled";
    endpoint AesKeyUnwrap = "system/aes/key-unwrap" => AesKeyUnwrapped;
    topic AesKeyUnwrapped = "system/aes/key-unwrapped";
}

/// Arm the SoC deadline watchdog and misbehave as `0` says. Tests the SoC
/// deadline service, not RF cessation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WatchdogTest(pub WatchdogTestMode);

/// The watchdog is armed for this mode; a misbehaving mode then ends in the
/// reset the next boot reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct WatchdogArmed(pub WatchdogTestMode);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum WatchdogTestMode {
    /// Complete normally, then continue serving commands beyond the budget.
    Complete,
    /// Never return from the synchronous task poll.
    BlockedPoll,
    /// Drop an armed future without completing its physical obligation.
    Cancelled,
    /// Keep executor progress but never acknowledge completion.
    LostCompletion,
    /// Attempt restoration after the hardware deadline.
    LateRestoration,
}

/// Diagnostic: once the acknowledgement is out, switch the USB Serial/JTAG
/// off its pads, so the board leaves USB until a reset brings the ROM back.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DisableUsb;

/// The USB Serial/JTAG leaves its pads after this acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UsbDisabled;
