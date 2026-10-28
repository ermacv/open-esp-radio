use bt_hci::cmd::{
    Cmd,
    controller_baseband::{
        HostBufferSize, Reset, SetControllerToHostFlowControl, SetEventMask, SetEventMaskPage2,
    },
    info::{ReadBdAddr, ReadLocalSupportedCmds},
    le::{
        LeReadBufferSize, LeReadFilterAcceptListSize, LeReadLocalSupportedFeatures, LeSetAdvEnable,
        LeSetEventMask, LeSetRandomAddr,
    },
};

use super::{
    BluetoothPublicDeviceAddress, BootstrapCommand, BootstrapConfigError,
    LeControllerBootstrapConfig,
};

#[test]
fn capability_table_excludes_link_layer_and_optional_page_two_commands() {
    for opcode in [
        Reset::OPCODE,
        SetEventMask::OPCODE,
        SetControllerToHostFlowControl::OPCODE,
        HostBufferSize::OPCODE,
        ReadBdAddr::OPCODE,
        ReadLocalSupportedCmds::OPCODE,
        LeSetEventMask::OPCODE,
        LeReadBufferSize::OPCODE,
        LeReadLocalSupportedFeatures::OPCODE,
        LeSetRandomAddr::OPCODE,
        LeReadFilterAcceptListSize::OPCODE,
    ] {
        assert!(BootstrapCommand::supports(opcode));
    }
    assert!(!BootstrapCommand::supports(SetEventMaskPage2::OPCODE));
    assert!(!BootstrapCommand::supports(LeSetAdvEnable::OPCODE));
}

#[test]
fn bootstrap_config_rejects_profiles_without_acl_capacity() {
    assert_eq!(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            0,
            1,
        ),
        Err(BootstrapConfigError::ZeroAclDataPacketLength)
    );
    assert_eq!(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            27,
            0,
        ),
        Err(BootstrapConfigError::ZeroAclDataPacketCount)
    );
    assert_eq!(
        LeControllerBootstrapConfig::new(
            BluetoothPublicDeviceAddress::from_canonical_bytes([0; 6]),
            252,
            1,
        ),
        Err(BootstrapConfigError::AclDataPacketLengthTooLarge {
            length: 252,
            maximum: 251,
        })
    );
}
