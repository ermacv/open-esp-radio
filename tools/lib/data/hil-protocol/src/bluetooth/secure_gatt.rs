//! Value-only observations and explicit UI decisions for the secure application.
//! No key material, HCI commands or fabricated security transitions cross HIL.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Diagnostic reader progress, not Controller quiescence or RF-stop evidence.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BluetoothGattResetReadGate {
    #[default]
    Disabled,
    Armed,
    ResetEntered,
    ReaderHeld,
    Released,
    FailureRequested,
    ReadFailed,
}

/// Redacted cause from the completed production Host epoch, not a test verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BluetoothGattStopCause {
    Requested,
    InjectedBondLoadFailure,
    Application,
    Host,
}

/// Redacted application failure class; never includes packet bytes or keys.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BluetoothGattApplicationFailure {
    Controller,
    HciStatus(u8),
    Disconnected,
    InvalidState,
    Busy,
    Resources,
    HostOther,
    Store,
    HostNotEmpty,
    InvalidBond,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BluetoothGattResetOutcome {
    Completed,
    BootstrapUnacknowledged,
    BootstrapDrainFailed,
    CommandFailed,
    ReceiveFailed,
    /// The HIL wrapper returned its distinct injected error to the real epoch runner.
    InjectedReceiveFailure,
}

/// Software disposition only; physical release is observed independently.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BluetoothGattShutdown {
    pub cause: BluetoothGattStopCause,
    pub reset: BluetoothGattResetOutcome,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BluetoothNumericChallenge {
    pub id: u64,
    /// Display as six decimal digits, including leading zeroes.
    pub number: u32,
}

/// An explicit application decision, bound to the boot by its command envelope.
/// A HIL test operator may supply it; this alone does not prove human consent.
/// Recording it does not prove pairing, encryption or bond-store completion.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BluetoothNumericDecision {
    pub challenge: BluetoothNumericChallenge,
    pub accept: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct BluetoothSecureGattEvidence {
    /// Boot-lifetime last recovered advertising rejection, not a terminal failure.
    #[serde(default)]
    pub advertising_start_rejection: Option<BluetoothAdvertisingStartRejection>,
    #[serde(default)]
    pub application_failure: Option<BluetoothGattApplicationFailure>,
    pub reset_read_gate: BluetoothGattResetReadGate,
    pub bond_load_fault_armed: bool,
    /// Actual injected backend failures, not requests to arm one.
    pub bond_load_failures: u32,
    pub shutdown: Option<BluetoothGattShutdown>,
    /// Application-owned sequence; boot identity is unchanged by cold restart.
    pub epoch: u32,
    pub restarting: bool,
    /// Checked physical cold-owner returns, not HCI Reset completions.
    pub cold_releases: u32,
    pub old_hci_closed: bool,
    pub traffic: crate::bluetooth::BluetoothGattEvidence,
    /// Live unanswered prompt, not the last prompt observed in a log.
    pub comparison: Option<BluetoothNumericChallenge>,
    pub comparisons: u32,
    pub accepted: u32,
    pub declined: u32,
    pub bonds_stored: u32,
    pub bonds_resumed: u32,
    pub pairing_failures: u32,
    pub rejected: u32,
    /// Host queue acceptance, not independent peer reception.
    pub notifications_queued: u32,
    /// Terminal Host/application outcome; does not by itself prove RF closure.
    pub application_stopped: bool,
}

/// Recovered advertising preparation failure, separate from the HCI status code.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum BluetoothAdvertisingStartRejection {
    Configuration,
    GenerationExhausted,
    PduFit,
    AdvertisingEventActive,
    PeripheralEventActive,
    MemoryPreparation,
    TimingWindow,
    Timeline,
    Sequence,
    EventFields,
    EmptyList,
}
