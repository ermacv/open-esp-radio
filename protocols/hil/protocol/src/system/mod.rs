//! The system module: SoC diagnostics independent of any radio: the
//! watchdog, the panic and USB-off injections and stack high-water marks.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod payload;
pub use payload::*;

crate::messages! {
    endpoint WatchdogTest = "system/watchdog/test" => WatchdogArmed;
    topic WatchdogArmed = "system/watchdog/armed";
    endpoint InjectPanic = "system/panic/inject" => crate::system::PanicInjected;
    topic PanicInjected = "system/panic/injected";
    endpoint DisableUsb = "system/usb/disable" => crate::system::UsbDisabled;
    topic UsbDisabled = "system/usb/disabled";
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

/// Diagnostic: panic in thread context once the acknowledgement is out, so
/// the platform's panic entry records the panic and resets the chip.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct InjectPanic;

/// The panic follows this acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PanicInjected;

/// Diagnostic: once the acknowledgement is out, switch the USB Serial/JTAG
/// off its pads, so the board leaves USB until a reset brings the ROM back.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DisableUsb;

/// The USB Serial/JTAG leaves its pads after this acknowledgement.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct UsbDisabled;
