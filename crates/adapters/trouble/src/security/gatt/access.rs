//! Guard all ATT value-access forms, including reads not reported as Read events.
use trouble_host::{
    att::{AttClient, AttCmd, AttReq},
    prelude::Uuid,
};

pub(super) fn protected(request: &AttClient<'_>, value: u16, cccd: u16) -> bool {
    match request {
        AttClient::Command(AttCmd::Write { handle, .. }) => *handle == value || *handle == cccd,
        AttClient::Confirmation(_) => false,
        AttClient::Request(request) => match request {
            AttReq::Read { handle } | AttReq::ReadBlob { handle, .. } => *handle == value,
            AttReq::Write { handle, .. } => *handle == value || *handle == cccd,
            AttReq::ReadMultiple { handles } => handles
                .chunks_exact(2)
                .any(|h| u16::from_le_bytes([h[0], h[1]]) == value),
            AttReq::ReadByType {
                start,
                end,
                attribute_type,
            } => {
                *start <= value
                    && value <= *end
                    && *attribute_type == Uuid::new_short(crate::TROUBLE_GATT_VALUE_UUID)
            }
            AttReq::ReadByGroupType {
                start,
                end,
                group_type,
            } => {
                *start <= value
                    && value <= *end
                    && *group_type == Uuid::new_short(crate::TROUBLE_GATT_VALUE_UUID)
            }
            AttReq::FindByTypeValue {
                start_handle,
                end_handle,
                att_type,
                ..
            } => {
                *start_handle <= value
                    && value <= *end_handle
                    && *att_type == crate::TROUBLE_GATT_VALUE_UUID
            }
            // Queued writes are not used by this profile; never accumulate one
            // during unauthorised enrollment, even if enabled by another feature.
            AttReq::PrepareWrite { .. } | AttReq::ExecuteWrite { .. } => true,
            AttReq::ExchangeMtu { .. } | AttReq::FindInformation { .. } => false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composite_and_direct_value_accesses_are_guarded_but_discovery_is_open() {
        let value = 17;
        let cccd = 18;
        let handles = [1, 0, 17, 0];
        let accesses = [
            AttReq::Read { handle: value },
            AttReq::ReadBlob {
                handle: value,
                offset: 0,
            },
            AttReq::ReadMultiple { handles: &handles },
            AttReq::ReadByType {
                start: 1,
                end: 0xffff,
                attribute_type: Uuid::new_short(crate::TROUBLE_GATT_VALUE_UUID),
            },
            AttReq::Write {
                handle: cccd,
                data: &[1, 0],
            },
            AttReq::Write {
                handle: value,
                data: &[3],
            },
            AttReq::FindByTypeValue {
                start_handle: 1,
                end_handle: 0xffff,
                att_type: crate::TROUBLE_GATT_VALUE_UUID,
                att_value: &[0],
            },
        ];
        for request in accesses {
            assert!(protected(&AttClient::Request(request), value, cccd));
        }
        assert!(protected(
            &AttClient::Command(AttCmd::Write {
                handle: value,
                data: &[3]
            }),
            value,
            cccd
        ));
        assert!(!protected(
            &AttClient::Request(AttReq::ReadByType {
                start: 1,
                end: 0xffff,
                attribute_type: Uuid::new_short(0x2803)
            }),
            value,
            cccd
        ));
        assert!(!protected(
            &AttClient::Request(AttReq::Read { handle: cccd }),
            value,
            cccd
        ));
    }
}
