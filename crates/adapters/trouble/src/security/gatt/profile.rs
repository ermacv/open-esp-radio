use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use trouble_host::prelude::*;

pub(super) fn table(
    storage: &mut [u8; 1],
) -> (AttributeTable<'_, NoopRawMutex, 11>, Characteristic<u8>) {
    let mut table = AttributeTable::new();
    let mut gap = table.add_service(Service::new(0x1800_u16));
    gap.add_characteristic_ro(0x2a00_u16, crate::TROUBLE_GATT_DEVICE_NAME);
    gap.add_characteristic_ro(0x2a01_u16, &[0, 0]);
    gap.build();
    table.add_service(Service::new(0x1801_u16));
    let value = table
        .add_service(Service::new(crate::TROUBLE_GATT_SERVICE_UUID))
        .add_characteristic(
            crate::TROUBLE_GATT_VALUE_UUID,
            [
                CharacteristicProp::Read,
                CharacteristicProp::Write,
                CharacteristicProp::Notify,
            ],
            0_u8,
            storage,
        )
        .read_permission(PermissionLevel::AuthenticationRequired)
        .write_permission(PermissionLevel::AuthenticationRequired)
        .cccd_permission(PermissionLevel::AuthenticationRequired)
        .build();
    (table, value)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn real_value_and_cccd_require_authenticated_encryption() {
        let mut storage = [0];
        let (table, value) = table(&mut storage);
        let permissions = table.permissions(value.handle).unwrap();
        assert_eq!(permissions.read, PermissionLevel::AuthenticationRequired);
        assert_eq!(permissions.write, PermissionLevel::AuthenticationRequired);
        assert_eq!(
            table.permissions(value.cccd_handle.unwrap()).unwrap().write,
            PermissionLevel::AuthenticationRequired
        );
        assert_eq!(table.get(&value), Ok(0));
    }
}
