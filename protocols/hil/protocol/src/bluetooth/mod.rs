//! The Bluetooth module: the Controller's test modes and raw HCI, and the
//! GATT applications' observation and control, with the properties Bluetooth
//! images advertise.

#[cfg(feature = "bluetooth")]
use postcard_schema::Schema;
#[cfg(feature = "bluetooth")]
use serde::{Deserialize, Serialize};

#[cfg(feature = "bluetooth")]
mod gatt;
#[cfg(feature = "bluetooth")]
mod payload;
#[cfg(feature = "bluetooth")]
mod secure_gatt;
#[cfg(feature = "bluetooth")]
pub use gatt::*;
#[cfg(feature = "bluetooth")]
pub use payload::*;
#[cfg(feature = "bluetooth")]
pub use secure_gatt::*;

crate::messages! {
    /// Numeric-Comparison-only application, RAM bonds and explicit UI
    /// decisions.
    property SecureGatt = "bluetooth/secure-gatt";
    /// Plaintext Trouble GATT application with observation-only HIL control.
    property Gatt = "bluetooth/gatt";
    property Dtm = "bluetooth/dtm";
    /// Raw HCI exchanges with the image's Controller.
    property Hci = "bluetooth/hci";
    /// The Bluetooth LE trace: the Controller, radio role and runtime
    /// record their events.
    property Trace = "bluetooth/trace";
    #[cfg(feature = "bluetooth")]
    endpoint FailGattResetRead = "bluetooth/secure-gatt/fail-reset-read" => crate::bluetooth::SecureGattState;
    #[cfg(feature = "bluetooth")]
    endpoint GattResetReadGate = "bluetooth/secure-gatt/reset-read-gate" => crate::bluetooth::SecureGattState;
    #[cfg(feature = "bluetooth")]
    endpoint FailNextGattBondLoad = "bluetooth/secure-gatt/fail-next-bond-load" => crate::bluetooth::SecureGattState;
    #[cfg(feature = "bluetooth")]
    endpoint RestartGatt = "bluetooth/secure-gatt/restart" => crate::bluetooth::SecureGattState;
    #[cfg(feature = "bluetooth")]
    endpoint GetSecureGatt = "bluetooth/secure-gatt/get" => crate::bluetooth::SecureGattState;
    #[cfg(feature = "bluetooth")]
    endpoint ConfirmGatt = "bluetooth/secure-gatt/confirm" => crate::bluetooth::GattDecisionRecorded;
    #[cfg(feature = "bluetooth")]
    endpoint GetGatt = "bluetooth/gatt/get" => crate::bluetooth::GattState;
    #[cfg(feature = "bluetooth")]
    endpoint RunDtm = "bluetooth/dtm/run" => crate::bluetooth::DtmResult;
    #[cfg(feature = "bluetooth")]
    endpoint ExchangeHci = "bluetooth/hci/exchange" => crate::bluetooth::HciResponse;
    #[cfg(feature = "bluetooth")]
    topic GattState = "bluetooth/gatt/state";
    #[cfg(feature = "bluetooth")]
    topic SecureGattState = "bluetooth/secure-gatt/state";
    #[cfg(feature = "bluetooth")]
    topic GattDecisionRecorded = "bluetooth/secure-gatt/decision-recorded";
    #[cfg(feature = "bluetooth")]
    topic DtmResult = "bluetooth/dtm/result";
    #[cfg(feature = "bluetooth")]
    topic HciResponse = "bluetooth/hci/response";
}

/// Inject a terminal read error only at the reached shutdown reader gate.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FailGattResetRead {
    pub epoch: u32,
}

/// Secure HIL only: delay the shutdown Reset reader, without fabricating a response.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GattResetReadGate {
    pub epoch: u32,
    pub release: bool,
}

/// Secure HIL composition only: fail one real bond-store load after disconnect.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FailNextGattBondLoad {
    pub epoch: u32,
}

/// Restart the secure application Controller epoch, retaining RAM bonds.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RestartGatt {
    pub epoch: u32,
}

/// `bluetooth/secure-gatt/get`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetSecureGatt;

/// `bluetooth/secure-gatt/confirm`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ConfirmGatt(pub crate::bluetooth::BluetoothNumericDecision);

/// Observe the shared Trouble application; does not access HCI directly.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetGatt;

/// `bluetooth/dtm/run`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunDtm(pub crate::bluetooth::BluetoothDtmOperation);

/// Raw HCI exchange with the Controller of a `bluetooth_hci` image.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ExchangeHci(pub crate::bluetooth::BluetoothHciRequest);

/// `bluetooth/gatt/state`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GattState(pub crate::bluetooth::BluetoothGattEvidence);

/// `bluetooth/secure-gatt/state`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SecureGattState(pub crate::bluetooth::BluetoothSecureGattEvidence);

/// UI decision queued once, not a successful pairing acknowledgement.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GattDecisionRecorded(pub crate::bluetooth::BluetoothNumericDecision);

/// `bluetooth/dtm/result`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DtmResult(pub crate::bluetooth::BluetoothDtmEvidence);

/// `bluetooth/hci/response`.
#[cfg(feature = "bluetooth")]
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HciResponse(pub crate::bluetooth::BluetoothHciResponse);
