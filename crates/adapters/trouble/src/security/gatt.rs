//! Numeric-Comparison-only, bonded peripheral application.
//!
//! The caller owns the Host, bond store and confirmation exchange independently.
//! Poll the Host/hardware runners concurrently. No operation here resets PHY.
//! Dropping this future requests Trouble connection/advertising cancellation;
//! it does not prove Controller quiescence. After cancellation, reconstruct the
//! Host from the retained store before running this application again.

mod access;
mod decline;
mod profile;
#[cfg(test)]
mod restore_tests;
mod trust;

use super::{
    bonds::{BondStore, StoreError},
    comparison::{Challenge, Confirmation, NumericComparison},
};
use bt_hci::{cmd::info::ReadBdAddr, param::AdvChannelMap};
use core::{convert::Infallible, future::pending};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use trouble_host::{BleHostError, BondInformation, Controller, Stack, prelude::*};
use trust::Trust;

/// Public observations contain neither key material nor peer IRKs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    Ready([u8; 6]),
    Advertising,
    Connected,
    Comparison(Challenge),
    ComparisonAccepted,
    ComparisonRejected,
    BondStored,
    BondResumed,
    PairingFailed,
    Rejected,
    Read(u8),
    Written(u8),
    /// Queued to the Host; independent peer reception is not implied.
    NotificationQueued(u8),
    Disconnected(u8),
}

/// Application/storage errors are not radio-terminal failures.
#[derive(Debug)]
pub enum RunError<C, S> {
    Host(BleHostError<C>),
    Store(StoreError<S>),
    /// Initial Host resources must not contain unrelated working-set keys.
    HostNotEmpty,
    /// Invalid or duplicate trust metadata; do not advertise a partial restore.
    InvalidBond,
}

fn host<C, S>(error: trouble_host::Error) -> RunError<C, S> {
    RunError::Host(BleHostError::BleHost(error))
}

async fn restore<C: Controller, S: BondStore>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    store: &mut S,
) -> Result<usize, RunError<C::Error, S::Error>> {
    let mut count = 0;
    for slot in 0..store.capacity() {
        let Some(bond) = store.load(slot).await.map_err(RunError::Store)? else {
            continue;
        };
        if !trust::valid_bond(&bond)
            || stack.with_bond_information(|bonds| {
                bonds
                    .iter()
                    .any(|old| old.identity.match_identity(&bond.identity))
            })
        {
            return Err(RunError::InvalidBond);
        }
        stack.add_bond_information(bond).map_err(host)?;
        count += 1;
    }
    Ok(count)
}

/// Serve authenticated read/write/notify with explicit enrollment confirmation.
///
/// This profile requires bonding. Imported records must originate from trusted
/// Numeric Comparison enrollment: `BondInformation` alone cannot prove method
/// provenance. A full store admits known peers only, without eviction. Bond loss
/// or another pairing method disconnects; replacing a bond requires a separate
/// explicit application operation. CCCD subscriptions are connection-local.
///
/// Store operations may await but must not block other runners. A cancelled
/// insert may have committed; recover by restoring a fresh Host from the store.
/// Returning an error never authorizes another run with the partially used Host.
pub async fn run<C: Controller, S: BondStore>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    store: &mut S,
    comparison: &NumericComparison,
    mut observe: impl FnMut(Observation),
) -> Result<Infallible, RunError<C::Error, S::Error>> {
    stack.set_io_capabilities(IoCapabilities::DisplayYesNo);
    stack
        .set_pairing_policy(PairingPolicy::NumericComparisonOnly)
        .map_err(host)?;
    if !stack.with_bond_information(|bonds| bonds.is_empty()) {
        return Err(RunError::HostNotEmpty);
    }
    let mut stored = restore(stack, store).await?;
    let address = stack
        .command(ReadBdAddr::new())
        .await
        .map_err(RunError::Host)?;
    observe(Observation::Ready(address.into_inner()));
    let mut peripheral = stack.peripheral();
    let mut storage = [0];
    let (table, value) = profile::table(&mut storage);
    let server = AttributeServer::<NoopRawMutex, DefaultPacketPool, 11, 1>::new(table);
    let mut adv = [0; 31];
    let mut scan = [0; 31];
    let (adv_len, scan_len) = crate::encode_trouble_gatt_advertising(&mut adv, &mut scan);
    let parameters = AdvertisementParameters {
        channel_map: Some(AdvChannelMap::CHANNEL_37),
        ..Default::default()
    };
    loop {
        let advertiser = peripheral
            .advertise(
                &parameters,
                Advertisement::ConnectableScannableUndirected {
                    adv_data: &adv[..adv_len],
                    scan_data: &scan[..scan_len],
                },
            )
            .await
            .map_err(RunError::Host)?;
        observe(Observation::Advertising);
        let connection = advertiser.accept().await.map_err(host)?;
        let identity = connection.peer_identity();
        let known = stack.with_bond_information(|bonds| {
            bonds
                .iter()
                .find(|b| b.identity.match_identity(&identity))
                .cloned()
        });
        let mut trust = Trust::new(known, stored < store.capacity());
        connection.set_bondable(trust.can_enroll()).map_err(host)?;
        let connection = connection.with_attribute_server(&server).map_err(host)?;
        let mut prompt: Option<Confirmation<'_>> = None;
        observe(Observation::Connected);
        if trust.rejected() {
            connection.raw().disconnect();
            observe(Observation::Rejected);
        }
        loop {
            let input = select(connection.next(), async {
                match prompt.as_mut() {
                    Some(prompt) => prompt.wait().await,
                    None => pending().await,
                }
            })
            .await;
            let event = match input {
                Either::First(event) => event,
                Either::Second(accept) => {
                    prompt = None;
                    if accept && trust.confirm() {
                        connection.pass_key_confirm().map_err(host)?;
                        observe(Observation::ComparisonAccepted);
                    } else {
                        trust.reject();
                        let result = decline::result(connection.pass_key_cancel());
                        connection.raw().disconnect();
                        result.map_err(host)?;
                        observe(Observation::ComparisonRejected);
                    }
                    continue;
                }
            };
            match event {
                GattConnectionEvent::Disconnected { reason } => {
                    drop(prompt.take());
                    observe(Observation::Disconnected(reason.into_inner()));
                    break;
                }
                GattConnectionEvent::PassKeyConfirm(number)
                    if trust.can_enroll() && prompt.is_none() =>
                {
                    match comparison.begin(number.value()) {
                        Ok(lease) => {
                            prompt = Some(lease);
                            observe(Observation::Comparison(
                                comparison.pending().expect("new prompt"),
                            ));
                        }
                        Err(_) => {
                            trust.reject();
                            connection.raw().disconnect();
                            observe(Observation::Rejected);
                        }
                    }
                }
                GattConnectionEvent::PairingComplete {
                    security_level,
                    bond,
                } => {
                    drop(prompt.take());
                    if let Some(bond) =
                        bond.filter(|b| trust.admit_pairing(security_level, b, &identity))
                    {
                        // Trouble may report PairingComplete despite an internal
                        // working-set insertion error. Verify/install it explicitly.
                        ensure_working_bond(stack, &bond).map_err(host)?;
                        if let Err(error) = store.insert(bond).await {
                            connection.raw().disconnect();
                            return Err(RunError::Store(error));
                        }
                        trust.saved();
                        observe(Observation::BondStored);
                    } else {
                        trust.reject();
                        connection.raw().disconnect();
                        observe(Observation::Rejected);
                    }
                }
                GattConnectionEvent::Encrypted {
                    security_level,
                    bond,
                } => {
                    if trust.expects_resume() {
                        if trust.resume(security_level, bond.as_ref()) {
                            observe(Observation::BondResumed);
                        } else {
                            trust.reject();
                            connection.raw().disconnect();
                            observe(Observation::Rejected);
                        }
                    }
                }
                GattConnectionEvent::PairingFailed(_) => {
                    drop(prompt.take());
                    trust.reject();
                    connection.raw().disconnect();
                    observe(Observation::PairingFailed);
                }
                GattConnectionEvent::PassKeyConfirm(_)
                | GattConnectionEvent::PassKeyDisplay(_)
                | GattConnectionEvent::PassKeyInput
                | GattConnectionEvent::OobRequest
                | GattConnectionEvent::BondLost => {
                    drop(prompt.take());
                    trust.reject();
                    connection.raw().disconnect();
                    observe(Observation::Rejected);
                }
                GattConnectionEvent::Gatt { event } => {
                    let read = matches!(&event, GattEvent::Read(e) if e.handle() == value.handle);
                    let write = matches!(&event, GattEvent::Write(e) if e.handle() == value.handle);
                    if access::protected(
                        &event.payload().incoming(),
                        value.handle,
                        value.cccd_handle.expect("notify CCCD"),
                    ) {
                        let level = match connection.raw().security_level() {
                            Ok(level) => level,
                            Err(error) => {
                                // Unhandled Trouble events accept on Drop. Consume
                                // the event explicitly even if the link disappeared.
                                event
                                    .reject(AttErrorCode::INSUFFICIENT_AUTHENTICATION)
                                    .map_err(host)?
                                    .send()
                                    .await;
                                return Err(host(error));
                            }
                        };
                        if !trust.authorized(level) {
                            let error = if level == SecurityLevel::EncryptedAuthenticated {
                                AttErrorCode::INSUFFICIENT_AUTHORISATION
                            } else {
                                AttErrorCode::INSUFFICIENT_AUTHENTICATION
                            };
                            event.reject(error).map_err(host)?.send().await;
                            continue;
                        }
                    }
                    event.accept().map_err(host)?.send().await;
                    if read || write {
                        let current = connection.get(&value).map_err(host)?;
                        observe(if write {
                            Observation::Written(current)
                        } else {
                            Observation::Read(current)
                        });
                        if write
                            && value.should_notify(&connection)
                            && trust.authorized(connection.raw().security_level().map_err(host)?)
                        {
                            value
                                .notify(&connection, &current, false)
                                .await
                                .map_err(host)?;
                            observe(Observation::NotificationQueued(current));
                        }
                    }
                }
                _ => {}
            }
        }
        drop(connection);
        // Only the caller's store defines trust across connections. Remove any
        // Host-only enrollment before restoring its complete bounded working set.
        while let Some(identity) = stack.with_bond_information(|b| b.first().map(|b| b.identity)) {
            stack.remove_bond_information(identity).map_err(host)?;
        }
        stored = restore(stack, store).await?;
    }
}

fn ensure_working_bond<C: Controller>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    bond: &BondInformation,
) -> Result<(), trouble_host::Error> {
    if stack.with_bond_information(|bonds| bonds.iter().any(|old| old == bond)) {
        return Ok(());
    }
    stack.add_bond_information(bond.clone())
}
