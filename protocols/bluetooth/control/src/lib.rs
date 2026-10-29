#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects,
        clippy::disallowed_macros,
        clippy::disallowed_methods
    )
)]
//! The Bluetooth module: the Controller's test modes and raw HCI, and the
//! GATT applications' observation and control, with the properties Bluetooth
//! images advertise.

use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

mod gatt;
mod payload;
mod secure_gatt;
pub use gatt::*;
pub use payload::*;
pub use secure_gatt::*;

oer_hil_protocol::messages! {
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
        endpoint FailGattResetRead = "bluetooth/secure-gatt/fail-reset-read" => crate::SecureGattState;
        endpoint GattResetReadGate = "bluetooth/secure-gatt/reset-read-gate" => crate::SecureGattState;
        endpoint FailNextGattBondLoad = "bluetooth/secure-gatt/fail-next-bond-load" => crate::SecureGattState;
        endpoint RestartGatt = "bluetooth/secure-gatt/restart" => crate::SecureGattState;
        endpoint GetSecureGatt = "bluetooth/secure-gatt/get" => crate::SecureGattState;
        endpoint ConfirmGatt = "bluetooth/secure-gatt/confirm" => crate::GattDecisionRecorded;
        endpoint GetGatt = "bluetooth/gatt/get" => crate::GattState;
        endpoint RunDtm = "bluetooth/dtm/run" => crate::DtmResult;
        endpoint ExchangeHci = "bluetooth/hci/exchange" => crate::HciResponse;
        topic GattState = "bluetooth/gatt/state";
        topic SecureGattState = "bluetooth/secure-gatt/state";
        topic GattDecisionRecorded = "bluetooth/secure-gatt/decision-recorded";
        topic DtmResult = "bluetooth/dtm/result";
        topic HciResponse = "bluetooth/hci/response";
}

/// Inject a terminal read error only at the reached shutdown reader gate.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FailGattResetRead {
    pub epoch: u32,
}

/// Secure HIL only: delay the shutdown Reset reader, without fabricating a response.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GattResetReadGate {
    pub epoch: u32,
    pub release: bool,
}

/// Secure HIL composition only: fail one real bond-store load after disconnect.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct FailNextGattBondLoad {
    pub epoch: u32,
}

/// Restart the secure application Controller epoch, retaining RAM bonds.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RestartGatt {
    pub epoch: u32,
}

/// `bluetooth/secure-gatt/get`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetSecureGatt;

/// `bluetooth/secure-gatt/confirm`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ConfirmGatt(pub crate::BluetoothNumericDecision);

/// Observe the shared Trouble application; does not access HCI directly.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GetGatt;

/// `bluetooth/dtm/run`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunDtm(pub crate::BluetoothDtmOperation);

/// Raw HCI exchange with the Controller of a `bluetooth_hci` image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct ExchangeHci(pub crate::BluetoothHciRequest);

/// `bluetooth/gatt/state`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GattState(pub crate::BluetoothGattEvidence);

/// `bluetooth/secure-gatt/state`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct SecureGattState(pub crate::BluetoothSecureGattEvidence);

/// UI decision queued once, not a successful pairing acknowledgement.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct GattDecisionRecorded(pub crate::BluetoothNumericDecision);

/// `bluetooth/dtm/result`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct DtmResult(pub crate::BluetoothDtmEvidence);

/// `bluetooth/hci/response`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct HciResponse(pub crate::BluetoothHciResponse);

#[cfg(test)]
mod framing_tests;

/// Reviewed paths, schemas (through keys) and message kinds.
#[cfg(feature = "descriptors")]
pub const MESSAGES_LOCK: &str = include_str!("../messages.lock");

#[cfg(all(test, feature = "descriptors"))]
#[test]
fn the_reviewed_lock_lists_exactly_this_crates_descriptors() {
    assert_eq!(
        oer_hil_protocol::check_lock(MESSAGES_LOCK, &[MESSAGES]),
        Ok(())
    );
}
