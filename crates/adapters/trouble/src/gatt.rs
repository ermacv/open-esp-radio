//! Plaintext GATT application shared by the standalone example and HIL.
//!
//! This is not a pairing or protected-ATT profile. The caller retains the
//! Trouble stack and polls its Host and hardware runners for this entire
//! future. Dropping this future requests normal Trouble advertising/connection
//! cancellation; it does not establish Controller retirement or RF shutdown.

use bt_hci::{cmd::info::ReadBdAddr, param::AdvChannelMap};
use core::convert::Infallible;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use trouble_host::{BleHostError, Controller, Stack, prelude::*};

use crate::{
    TROUBLE_GATT_DEVICE_NAME, TROUBLE_GATT_SERVICE_UUID, TROUBLE_GATT_VALUE_UUID,
    encode_trouble_gatt_advertising,
};

/// Value-only observations; observers neither own HCI nor supply ATT replies.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    /// Actual Controller public address in HCI byte order, after Host startup.
    Ready([u8; 6]),
    /// Advertising command completed; RF reception is not implied.
    Advertising,
    Connected,
    Read(u8),
    Written(u8),
    Disconnected(u8),
}

fn profile(storage: &mut [u8; 1]) -> (AttributeTable<'_, NoopRawMutex, 10>, Characteristic<u8>) {
    let mut table = AttributeTable::new();
    let mut gap = table.add_service(Service::new(0x1800_u16));
    gap.add_characteristic_ro(0x2a00_u16, TROUBLE_GATT_DEVICE_NAME);
    gap.add_characteristic_ro(0x2a01_u16, &[0, 0]);
    gap.build();
    table.add_service(Service::new(0x1801_u16));
    let value = table
        .add_service(Service::new(TROUBLE_GATT_SERVICE_UUID))
        .add_characteristic(
            TROUBLE_GATT_VALUE_UUID,
            [CharacteristicProp::Read, CharacteristicProp::Write],
            0_u8,
            storage,
        )
        .build();
    (table, value)
}

/// Advertise, serve the one-byte value, and re-advertise after disconnect.
///
/// The attribute storage survives connection changes, but not application
/// restart. Errors are returned with their actual Host/Controller origin.
/// The synchronous observer must not block or access the HCI transport.
pub async fn run<C: Controller>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    mut observe: impl FnMut(Observation),
) -> Result<Infallible, BleHostError<C::Error>> {
    let address = stack.command(ReadBdAddr::new()).await?;
    observe(Observation::Ready(address.into_inner()));
    let mut peripheral = stack.peripheral();
    let mut storage = [0];
    let (table, value) = profile(&mut storage);
    let server = AttributeServer::<NoopRawMutex, DefaultPacketPool, 10, 1>::new(table);
    let mut adv = [0; 31];
    let mut scan = [0; 31];
    let (adv_len, scan_len) = encode_trouble_gatt_advertising(&mut adv, &mut scan);
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
            .await?;
        observe(Observation::Advertising);
        let connection = advertiser.accept().await.map_err(BleHostError::BleHost)?;
        let connection = connection
            .with_attribute_server(&server)
            .map_err(BleHostError::BleHost)?;
        observe(Observation::Connected);
        loop {
            match connection.next().await {
                GattConnectionEvent::Disconnected { reason } => {
                    observe(Observation::Disconnected(reason.into_inner()));
                    break;
                }
                GattConnectionEvent::Gatt { event } => {
                    let read = matches!(&event, GattEvent::Read(e) if e.handle() == value.handle);
                    let write = matches!(&event, GattEvent::Write(e) if e.handle() == value.handle);
                    event.accept().map_err(BleHostError::BleHost)?.send().await;
                    if read || write {
                        let current = connection.get(&value).map_err(BleHostError::BleHost)?;
                        observe(if write {
                            Observation::Written(current)
                        } else {
                            Observation::Read(current)
                        });
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actual_profile_preserves_value_and_rejects_oversized_storage_writes() {
        let mut storage = [0xff];
        let (table, value) = profile(&mut storage);
        assert_eq!(table.get(&value), Ok(0));
        table.set(&value, &0x37).unwrap();
        assert_eq!(table.get(&value), Ok(0x37));
        assert!(table.write(value.handle, 0, &[1, 2]).is_err());
        assert_eq!(table.get(&value), Ok(0x37));
    }
}
